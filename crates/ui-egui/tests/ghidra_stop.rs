//! Stopping Ghidra's analysis from a project (#948). In a test program of its own: which analyses
//! are under way is the whole process's.

use craft_anything_engine::evidence::sha256_hex;
use craft_anything_engine::tools::deep;
use craft_anything_engine::workspace::{TargetRef, Workspace};
use craft_anything_engine::{Mode, settings::Settings};
use craft_anything_engines::ghidra;
use craft_anything_ui_egui::{CraftApp, demo};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::time::{Duration, Instant};

/// An analysis of `bytes` for `project` that waits for another process's lock, so it runs until
/// it's stopped.
fn waiting_analysis(d: &std::path::Path, project: &std::path::Path, bytes: &'static [u8]) -> std::thread::JoinHandle<Result<(), String>> {
    let launcher = d.join("analyzeHeadless");
    std::fs::write(&launcher, "").unwrap();
    let store = d.join("store");
    let mut settings = Settings::default();
    settings.engines.store_dir = Some(store.clone());
    settings.engines.ghidra.install_dir = Some(launcher);
    settings.engines.only_configured = true;
    let sha = sha256_hex(bytes);
    let digest = sha256_hex(b"craft-anything-ghidra-fast-profile-v2|lang=");
    let cache = ghidra::project_cache_dir(&store, &sha, &digest);
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(cache.with_extension("analysing"), "").unwrap();
    let (tx, waiting) = std::sync::mpsc::channel();
    let project = project.to_path_buf();
    let h = std::thread::spawn(move || {
        deep::prepare(&settings, bytes, &sha, &project, &move |_| {
            let _ = tx.send(());
        })
    });
    assert!(waiting.recv_timeout(Duration::from_secs(30)).is_ok(), "the analysis never began waiting");
    h
}

#[test]
fn the_project_offers_to_stop_ghidras_analysis_and_stops_it() {
    let d = tempfile::Builder::new().prefix("ca").tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Game", "A copy of a game", TargetRef::None).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 820.0)).with_max_steps(32).build_eframe(move |cc| {
        CraftApp::setup_context(&cc.egui_ctx);
        demo::app(demo::engine(Mode::Beginner, true))
    });
    h.run_steps(2);
    h.state_mut().open_workspace(ws.clone());
    h.run_steps(4);
    assert!(h.query_by_label("Stop the analysis").is_none(), "nothing to stop, nothing offered");

    let analysis = waiting_analysis(d.path(), &ws.root, b"a program the person stops analysing");
    h.run_steps(4);
    h.get_by_label("Stop the analysis").click();
    let until = Instant::now() + Duration::from_secs(30);
    while !analysis.is_finished() || h.query_by_label("Stop the analysis").is_some() {
        assert!(Instant::now() < until, "the analysis kept going, or Stop is still offered");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(analysis.join().unwrap().is_err(), "it stopped, it didn't finish");
    assert!(ws.root.join(craft_anything_engine::workspace::MANIFEST).is_file(), "the project is kept");
}
