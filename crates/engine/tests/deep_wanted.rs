//! A test program of its own: who wants an analysis is process-wide.

use craft_anything_engine::tools::deep;
use craft_anything_engine::workspace::MANIFEST;
use craft_anything_engines::ghidra;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A Ghidra stand-in whose analyses of `bytes` another process holds the lock for, so `prepare`
/// waits until it's told to stop. Returns a starter: a project named `name`, its build asking for
/// the analysis with Ghidra's language set to `lang` (`""`: detected).
fn waiting_analysis(d: &Path, bytes: &'static [u8]) -> impl Fn(&str, &str) -> (PathBuf, JoinHandle<Result<(), String>>) {
    let launcher = d.join("analyzeHeadless");
    std::fs::write(&launcher, "").unwrap();
    let store = d.join("store");
    let mut settings = craft_anything_engine::testing::session().settings.clone();
    settings.engines.store_dir = Some(store.clone());
    settings.engines.ghidra.install_dir = Some(launcher);
    settings.engines.ghidra.startup_timeout_secs = Some(120);
    let sha = craft_anything_evidence::sha256_hex(bytes);
    let d = d.to_path_buf();
    move |name, lang| {
        let digest = craft_anything_evidence::sha256_hex(format!("craft-anything-ghidra-fast-profile-v2|lang={lang}").as_bytes());
        let project = ghidra::project_cache_dir(&store, &sha, &digest);
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(project.with_extension("analysing"), "").unwrap();
        let root = d.join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(MANIFEST), "{}").unwrap();
        let mut settings = settings.clone();
        settings.engines.ghidra.language_id = (!lang.is_empty()).then(|| lang.to_string());
        let (sha, r) = (sha.clone(), root.clone());
        let (tx, waiting) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            deep::prepare(&settings, bytes, &sha, &r, &move |_| {
                let _ = tx.send(());
            })
        });
        // Its first line comes once it's recorded and waiting for the lock.
        assert!(waiting.recv_timeout(Duration::from_secs(30)).is_ok(), "the analysis never began waiting");
        (root, h)
    }
}

fn ends_within(h: &JoinHandle<Result<(), String>>, within: Duration) -> bool {
    let until = Instant::now() + within;
    while !h.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    h.is_finished()
}

/// #948: deleting the project whose build started an analysis stops it, here one waiting for
/// another's lock, within moments.
#[test]
fn deleting_the_project_stops_its_analysis() {
    let d = tempfile::Builder::new().prefix("ca").tempdir().unwrap();
    let start = waiting_analysis(d.path(), b"a program its project was deleted from");
    let (root, a) = start("game", "");
    assert!(!ends_within(&a, Duration::from_secs(3)), "it stopped while its project was there");
    std::fs::remove_file(root.join(MANIFEST)).unwrap();
    assert!(ends_within(&a, Duration::from_secs(30)), "the analysis kept going");
    assert!(a.join().unwrap().is_err(), "it stopped, it didn't finish");
}

/// …unless another project's build still waits for the same analysis.
#[test]
fn another_projects_build_keeps_the_analysis_going() {
    let d = tempfile::Builder::new().prefix("ca").tempdir().unwrap();
    let start = waiting_analysis(d.path(), b"a program two projects rebuild");
    let (first, a) = start("first", "");
    let (second, b) = start("second", "");
    std::fs::remove_file(first.join(MANIFEST)).unwrap();
    assert!(!ends_within(&a, Duration::from_secs(5)), "the second project still needs it");
    std::fs::remove_file(second.join(MANIFEST)).unwrap();
    assert!(ends_within(&a, Duration::from_secs(30)) && ends_within(&b, Duration::from_secs(30)), "the analysis kept going");
}

/// Real Ghidra, only where `CRAFT_ANYTHING_TEST_GHIDRA` names the engine store holding `ghidra/`
/// and `jdk/` (the app's is `%APPDATA%/craft-anything/engines`): deleting the project while Ghidra
/// analyses this test program stops it. Its analysis goes to a temp folder. Check afterwards that
/// no `java` is left running.
#[test]
fn live_ghidra_stops_when_its_project_is_deleted() {
    let Some(store) = std::env::var_os("CRAFT_ANYTHING_TEST_GHIDRA").map(PathBuf::from) else { return };
    // Ghidra rejects project paths with a folder that starts with a dot, as tempdir()'s do.
    let d = tempfile::Builder::new().prefix("ghidra-live").tempdir().unwrap();
    let mut settings = craft_anything_engine::testing::session().settings.clone();
    settings.engines.store_dir = Some(d.path().join("store"));
    settings.engines.ghidra.install_dir = Some(ghidra::get::headless(None, &store).expect("Ghidra in the engine store"));
    settings.engines.ghidra.java_home = ghidra::get::java_home(None, &store);
    let bytes: &'static [u8] = std::fs::read(std::env::current_exe().unwrap()).unwrap().leak();
    let root = d.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(MANIFEST), "{}").unwrap();
    let (tx, said) = std::sync::mpsc::channel::<String>();
    let r = root.clone();
    let h = std::thread::spawn(move || {
        let sha = craft_anything_evidence::sha256_hex(bytes);
        deep::prepare(&settings, bytes, &sha, &r, &move |m| {
            let _ = tx.send(m.to_string());
        })
    });
    // Its first line once Ghidra has run for 10 s.
    let until = Instant::now() + Duration::from_secs(120);
    while !said.recv_timeout(Duration::from_secs(1)).is_ok_and(|m| m.contains("min so far")) {
        if h.is_finished() {
            panic!("Ghidra ended before it got going: {:?}", h.join().unwrap());
        }
        assert!(Instant::now() < until, "Ghidra never got going");
    }
    let deleted = Instant::now();
    std::fs::remove_file(root.join(MANIFEST)).unwrap();
    assert!(ends_within(&h, Duration::from_secs(30)), "Ghidra kept going");
    let stopped = h.join().unwrap();
    assert!(stopped.as_ref().is_err_and(|e| e.contains("cancel")), "{stopped:?}");
    eprintln!("stopped {:.1} s after the project was deleted", deleted.elapsed().as_secs_f32());
}

/// A project waiting for another analysis of the same program (another Ghidra language) doesn't
/// keep this one going.
#[test]
fn another_analysis_of_the_program_doesnt_keep_it_going() {
    let d = tempfile::Builder::new().prefix("ca").tempdir().unwrap();
    let start = waiting_analysis(d.path(), b"a program analysed two ways");
    let (first, a) = start("first", "");
    let (second, b) = start("second", "x86:LE:32:default");
    std::fs::remove_file(first.join(MANIFEST)).unwrap();
    assert!(ends_within(&a, Duration::from_secs(30)), "the second project's analysis kept this one going");
    assert!(!b.is_finished(), "the second project's own analysis stopped too");
    std::fs::remove_file(second.join(MANIFEST)).unwrap();
    assert!(ends_within(&b, Duration::from_secs(30)), "the analysis kept going");
}
