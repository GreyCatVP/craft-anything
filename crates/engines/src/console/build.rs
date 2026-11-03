//! A recompile build: its stages, the `recomp.json` that records them (versioned, written only by
//! the build's own worker), the log beside it, and the worker that runs the stages in order.
//!
//! A build runs on its own thread and holds nothing of the engine, so a 90-minute compile never
//! blocks other tools. It resumes from the first stage that isn't done. Each new build takes the
//! backend's latest upstream source (cloned into the build's folder), and records the commit.

use super::Console;
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::cancel::CancelToken;
use craft_anything_platform::process::Process;
use craft_anything_platform::sync::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError};
use std::time::Duration;

pub const PROJECT_FILE: &str = "recomp.json";
pub const LOG_FILE: &str = "recomp.log";
pub const VERSION: u32 = 1;

/// The stages of every build, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Validate,
    Dependencies,
    Extract,
    Generate,
    Analyse,
    Recompile,
    Compile,
    Package,
}

impl Stage {
    pub const ALL: [Stage; 8] =
        [Stage::Validate, Stage::Dependencies, Stage::Extract, Stage::Generate, Stage::Analyse, Stage::Recompile, Stage::Compile, Stage::Package];

    pub fn title(self) -> &'static str {
        match self {
            Stage::Validate => "Game input and validation",
            Stage::Dependencies => "Dependency installation",
            Stage::Extract => "File extraction",
            Stage::Generate => "Native project generation",
            Stage::Analyse => "Executable analysis",
            Stage::Recompile => "Recompilation",
            Stage::Compile => "Compilation",
            Stage::Package => "Packaging",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Running,
    Done,
    Failed,
    /// It was running when the app or computer stopped; it runs again on resume.
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState {
    pub stage: Stage,
    pub status: StageStatus,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub started: Option<u64>,
    #[serde(default)]
    pub finished: Option<u64>,
}

/// `recomp.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub console: String,
    pub input: PathBuf,
    pub out: PathBuf,
    #[serde(default)]
    pub title: Option<String>,
    /// The backend's upstream repository and the commit this build used.
    #[serde(default)]
    pub backend_repository: String,
    #[serde(default)]
    pub backend_commit: Option<String>,
    pub stages: Vec<StageState>,
    /// What Play starts, with its arguments, working folder and environment.
    #[serde(default)]
    pub play: Option<Play>,
    /// The folder a packaged build was written to.
    #[serde(default)]
    pub package: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Play {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl Project {
    fn new(console: &dyn Console, input: &Path, out: &Path) -> Project {
        Project {
            version: VERSION,
            console: console.id().into(),
            input: input.to_path_buf(),
            out: out.to_path_buf(),
            title: None,
            backend_repository: console.backend().repository.into(),
            backend_commit: None,
            stages: Stage::ALL.iter().map(|s| StageState { stage: *s, status: StageStatus::Pending, message: None, started: None, finished: None }).collect(),
            play: None,
            package: None,
        }
    }

    pub fn stage(&mut self, s: Stage) -> Option<&mut StageState> {
        self.stages.iter_mut().find(|x| x.stage == s)
    }

    fn save(&self) -> Result<(), ToolError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| ToolError::new(ErrorCode::FileError, e.to_string()))?;
        craft_anything_platform::fs::write_atomic(&self.out.join(PROJECT_FILE), &bytes)
    }
}

/// Read a build's `recomp.json`. A stage left `running` by a build that isn't running in this
/// process was interrupted.
pub fn load(out: &Path) -> Result<Project, ToolError> {
    let path = out.join(PROJECT_FILE);
    let bytes = std::fs::read(&path).map_err(|e| ToolError::new(ErrorCode::BadInput, format!("No recompile build in {}: {e}", out.display())))?;
    let mut p: Project = serde_json::from_slice(&bytes).map_err(|e| ToolError::new(ErrorCode::FileError, format!("{} is damaged: {e}", path.display())))?;
    if p.version > VERSION {
        return Err(ToolError::new(ErrorCode::NotSupported, format!("{} was written by a newer Craft Anything (version {}).", path.display(), p.version)));
    }
    if !is_running(out) {
        for s in &mut p.stages {
            if s.status == StageStatus::Running {
                s.status = StageStatus::Interrupted;
            }
        }
    }
    Ok(p)
}

/// The last `n` lines of a build's log.
pub fn log_tail(out: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read(out.join(LOG_FILE)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let total = text.lines().count();
    text.lines().skip(total.saturating_sub(n)).map(str::to_owned).collect()
}

fn running() -> &'static Mutex<HashMap<PathBuf, CancelToken>> {
    static RUNNING: OnceLock<Mutex<HashMap<PathBuf, CancelToken>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether a build runs in `out`: in this process, or in another one (the tool server an AI
/// started it from) that still touches its heartbeat file.
pub fn is_running(out: &Path) -> bool {
    running().lock().unwrap_or_else(PoisonError::into_inner).contains_key(out)
        || std::fs::metadata(out.join(ALIVE_FILE)).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d < Duration::from_secs(10)))
}

/// Touched every two seconds while a build runs, so other processes can tell it's alive.
const ALIVE_FILE: &str = "recomp.alive";
const STOP_FILE: &str = "recomp.stop";

/// Ask a running build to stop; its current program is ended and the stage marked interrupted.
pub fn stop(out: &Path) -> bool {
    match running().lock().unwrap_or_else(PoisonError::into_inner).get(out) {
        Some(c) => {
            c.cancel();
            true
        }
        // Running in another process: its heartbeat thread sees this file and stops it.
        None => is_running(out) && std::fs::write(out.join(STOP_FILE), b"").is_ok(),
    }
}

/// Start (or resume) a build in `out` and return at once. A folder that already holds a build
/// must be the same console and game; a folder with other files is refused.
pub fn start(console_id: &str, input: &Path, out: &Path, from: Option<Stage>) -> Result<Project, ToolError> {
    let console = super::console(console_id).ok_or_else(|| ToolError::invalid(format!("No console {console_id:?}")))?;
    if !out.is_absolute() {
        return Err(ToolError::invalid("The output folder must be an absolute path."));
    }
    let mut project = if out.join(PROJECT_FILE).is_file() {
        let p = load(out)?;
        if p.console != console.id() || p.input != input {
            return Err(ToolError::invalid(format!("{} holds a build made from {} ({}).", out.display(), p.input.display(), p.console))
                .with_remediation("Give that same game path to carry on with it, or pick an empty folder for another game."));
        }
        p
    } else {
        if out.read_dir().is_ok_and(|mut d| d.next().is_some()) {
            return Err(ToolError::invalid(format!("{} isn't empty.", out.display())).with_remediation("Pick an empty or new folder for the build."));
        }
        std::fs::create_dir_all(out).map_err(|e| ToolError::io(out.display(), e))?;
        Project::new(console, input, out)
    };
    if is_running(out) {
        return Err(ToolError::invalid("This build is already running."));
    }
    // Run again from a stage (after the config changed): it and every later stage.
    if let Some(from) = from {
        let mut again = false;
        for s in &mut project.stages {
            again |= s.stage == from;
            if again {
                s.status = StageStatus::Pending;
            }
        }
    }
    let cancel = CancelToken::default();
    {
        let mut map = running().lock().unwrap_or_else(PoisonError::into_inner);
        if map.contains_key(out) {
            return Err(ToolError::invalid("This build is already running."));
        }
        map.insert(out.to_path_buf(), cancel.clone());
    }
    for s in &mut project.stages {
        if matches!(s.status, StageStatus::Failed | StageStatus::Interrupted | StageStatus::Running) {
            s.status = StageStatus::Pending;
        }
    }
    project.save()?;
    let snapshot = project.clone();
    let key = out.to_path_buf();
    let spawned = std::thread::Builder::new().name("recompile".into()).spawn(move || {
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (flag, alive, stop_file, stopper) = (done.clone(), key.join(ALIVE_FILE), key.join(STOP_FILE), cancel.clone());
        let _ = std::fs::remove_file(&stop_file);
        let _ = std::thread::Builder::new().name("recompile-alive".into()).spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = std::fs::write(&alive, b"");
                if std::fs::remove_file(&stop_file).is_ok() {
                    stopper.cancel();
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            let _ = std::fs::remove_file(&alive);
        });
        run(console, project, cancel);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        running().lock().unwrap_or_else(PoisonError::into_inner).remove(&key);
    });
    if let Err(e) = spawned {
        running().lock().unwrap_or_else(PoisonError::into_inner).remove(out);
        return Err(ToolError::new(ErrorCode::FileError, format!("Couldn't start the build: {e}")));
    }
    Ok(snapshot)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn run(console: &'static dyn Console, mut project: Project, cancel: CancelToken) {
    let log = Log::open(&project.out.join(LOG_FILE));
    let mut cx = StageCx { out: project.out.clone(), input: project.input.clone(), log, cancel, env: None, data: serde_json::Value::Null };
    for stage in Stage::ALL {
        if project.stage(stage).is_some_and(|s| s.status == StageStatus::Done) {
            continue;
        }
        if let Some(s) = project.stage(stage) {
            s.status = StageStatus::Running;
            s.started = Some(now());
            s.message = None;
        }
        let _ = project.save();
        cx.log.line(&format!("== {} ({})", stage.title(), console.name()));
        let result = if cx.cancel.is_cancelled() { Err(ToolError::cancelled()) } else { console.run_stage(stage, &mut cx, &mut project) };
        let cancelled = cx.cancel.is_cancelled();
        if let Some(s) = project.stage(stage) {
            s.finished = Some(now());
            match &result {
                Ok(msg) => {
                    s.status = StageStatus::Done;
                    s.message = Some(msg.clone());
                }
                Err(e) => {
                    s.status = if cancelled { StageStatus::Interrupted } else { StageStatus::Failed };
                    s.message = Some(match &e.remediation {
                        Some(r) => format!("{} {r}", e.message),
                        None => e.message.clone(),
                    });
                }
            }
            cx.log.line(&format!("-- {:?}: {}", s.status, s.message.clone().unwrap_or_default()));
        }
        let _ = project.save();
        if result.is_err() {
            return;
        }
    }
}

/// The log file, shared with the output threads of the programs a stage runs.
#[derive(Clone)]
pub struct Log(Arc<Mutex<Option<std::fs::File>>>);

impl Log {
    fn open(path: &Path) -> Log {
        Log(Arc::new(Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(path).ok())))
    }
    pub fn line(&self, text: &str) {
        if let Some(f) = self.0.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            let _ = writeln!(f, "{text}");
        }
    }
}

/// What a stage works with.
pub struct StageCx {
    pub out: PathBuf,
    pub input: PathBuf,
    pub log: Log,
    pub cancel: CancelToken,
    /// The environment the build's programs run in (the C++ toolchain's), once found.
    pub env: Option<Vec<(OsString, OsString)>>,
    /// Facts one stage leaves for the next within this run.
    pub data: serde_json::Value,
}

/// How long one program of a build may run, and stay silent.
pub const LONG: Duration = Duration::from_secs(6 * 3600);
pub const SILENT: Duration = Duration::from_secs(2 * 3600);

impl StageCx {
    /// Run a program to completion with its output in the log; a non-zero exit is an error.
    pub fn run(&self, p: Process) -> Result<String, ToolError> {
        let (a, b, c) = (self.log.clone(), self.log.clone(), self.cancel.clone());
        let mut p =
            p.timeout(LONG).idle_timeout(SILENT).cancel_when(move || c.is_cancelled()).on_stdout_line(move |l| a.line(l)).on_stderr_line(move |l| b.line(l));
        for (k, v) in self.env.iter().flatten() {
            p = p.env(k, v);
        }
        p.run_checked().map(|o| o.stdout_text() + &o.stderr_text())
    }

    /// A program by name, looked up in the build's environment (else this process's PATH).
    pub fn program(&self, names: &[&str]) -> Option<PathBuf> {
        let path = self
            .env
            .iter()
            .flatten()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var_os("PATH"))?;
        let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
        for name in names {
            for dir in std::env::split_paths(&path) {
                for ext in exts {
                    let p = dir.join(format!("{name}{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    /// [`program`](Self::program), or an error that says how to get it.
    pub fn need(&self, names: &[&str], how: &str) -> Result<PathBuf, ToolError> {
        self.program(names).ok_or_else(|| {
            ToolError::new(ErrorCode::EngineMissing, format!("{} isn't installed (or isn't found).", names.first().copied().unwrap_or("A program")))
                .with_remediation(how)
        })
    }

    /// Clone the backend's latest upstream source into `dir` (or update a clone already there),
    /// and return the commit.
    pub fn upstream(&self, repository: &str, dir: &Path) -> Result<String, ToolError> {
        let git = self.need(&["git"], "Install Git (https://git-scm.com) and try again.")?;
        if dir.join(".git").is_dir() {
            self.run(Process::new(&git).arg("-C").arg(dir).args(["fetch", "--depth", "1", "origin", "HEAD"]))?;
            self.run(Process::new(&git).arg("-C").arg(dir).args(["reset", "--hard", "FETCH_HEAD"]))?;
        } else {
            self.run(Process::new(&git).args(["clone", "--depth", "1", repository]).arg(dir))?;
        }
        Ok(self.run(Process::new(&git).arg("-C").arg(dir).args(["rev-parse", "HEAD"]))?.lines().next().unwrap_or_default().trim().to_string())
    }

    /// Windows: the environment of the newest Visual Studio with the C++ tools, with its bundled
    /// CMake, Ninja and Clang first on PATH. Other systems: nothing to set.
    pub fn toolchain_env(&mut self) -> Result<(), ToolError> {
        if self.env.is_some() || !cfg!(windows) {
            return Ok(());
        }
        let missing = || {
            ToolError::new(ErrorCode::EngineMissing, "Visual Studio with the C++ tools wasn't found.").with_remediation(
                "Install Visual Studio 2022 (Community is free) with \"Desktop development with C++\", \"C++ Clang tools for Windows\" and \"C++ CMake tools for Windows\".",
            )
        };
        let pf86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from).ok_or_else(missing)?;
        let vswhere = pf86.join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
        if !vswhere.is_file() {
            return Err(missing());
        }
        let vs = Process::new(&vswhere)
            .args(["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"])
            .timeout(Duration::from_secs(60))
            .run_checked()?
            .stdout_text();
        let vs = PathBuf::from(vs.lines().next().unwrap_or_default().trim());
        let devcmd = vs.join("Common7").join("Tools").join("VsDevCmd.bat");
        if !devcmd.is_file() {
            return Err(missing());
        }
        // A batch file in the build's folder: cmd's quoting of a path with spaces is fragile.
        let bat = self.out.join("vsenv.bat");
        std::fs::write(&bat, format!("@call \"{}\" -arch=amd64 -host_arch=amd64 >nul\r\n@set\r\n", devcmd.display()))
            .map_err(|e| ToolError::io(bat.display(), e))?;
        let text = Process::new("cmd").args(["/d", "/c"]).arg(&bat).timeout(Duration::from_secs(120)).run_checked()?.stdout_text();
        let mut env: Vec<(OsString, OsString)> = text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (OsString::from(k), OsString::from(v))).collect();
        let cmake = vs.join("Common7").join("IDE").join("CommonExtensions").join("Microsoft").join("CMake");
        let extra = [cmake.join("CMake").join("bin"), cmake.join("Ninja"), vs.join("VC").join("Tools").join("Llvm").join("x64").join("bin")];
        if let Some((_, path)) = env.iter_mut().find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH")) {
            let joined = std::env::join_paths(extra.iter().cloned().chain(std::env::split_paths(path)))
                .map_err(|e| ToolError::new(ErrorCode::FileError, e.to_string()))?;
            *path = joined;
        }
        self.log.line(&format!("Using Visual Studio at {}", vs.display()));
        self.env = Some(env);
        Ok(())
    }
}

/// Copy a folder's files into another, skipping files already there with the same size (so a
/// resumed copy picks up where it stopped). Returns how many files were copied.
pub fn copy_tree(from: &Path, to: &Path, cancel: &CancelToken, skip: &dyn Fn(&Path) -> bool) -> Result<usize, ToolError> {
    let mut copied = 0;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        cancel.check()?;
        for entry in std::fs::read_dir(&dir).map_err(|e| ToolError::io(dir.display(), e))?.flatten() {
            let path = entry.path();
            if skip(&path) {
                continue;
            }
            let rel = path.strip_prefix(from).unwrap_or(&path);
            let dest = to.join(rel);
            let kind = entry.file_type().map_err(|e| ToolError::io(path.display(), e))?;
            if kind.is_dir() {
                std::fs::create_dir_all(&dest).map_err(|e| ToolError::io(dest.display(), e))?;
                stack.push(path);
            } else if kind.is_file() {
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if dest.metadata().is_ok_and(|m| m.len() == len) {
                    continue;
                }
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| ToolError::io(parent.display(), e))?;
                }
                std::fs::copy(&path, &dest).map_err(|e| ToolError::io(dest.display(), e))?;
                copied += 1;
            }
        }
    }
    Ok(copied)
}

/// Fail a stage unless the game passes the console's checks; returns the inspection.
pub fn validate(console: &dyn Console, cx: &StageCx, project: &mut Project) -> Result<super::Inspection, ToolError> {
    let ins = console.inspect(&cx.input, &cx.cancel)?;
    for c in &ins.checks {
        cx.log.line(&format!("{:?}: {}", c.status, c.message));
    }
    project.title = ins.title.clone();
    if ins.verdict == super::Verdict::Blocked {
        let why: Vec<&str> = ins.checks.iter().filter(|c| c.status == super::Status::Fail).map(|c| c.message.as_str()).collect();
        return Err(ToolError::invalid(why.join(" ")));
    }
    Ok(ins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_game_fails_validation_and_the_build_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let out = dir.path().join("out");
        start("xbox", &game, &out, None).unwrap();
        // The worker stops at validation: no default.xbe.
        for _ in 0..200 {
            if !is_running(&out) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let p = load(&out).unwrap();
        assert_eq!(p.stages[0].status, StageStatus::Failed, "{:?}", p.stages[0]);
        assert_eq!(p.stages[1].status, StageStatus::Pending);
        assert!(log_tail(&out, 50).iter().any(|l| l.contains("Game input and validation")));
        // Resuming the same build is allowed; another game in the same folder isn't.
        assert!(start("xbox", &dir.path().join("other"), &out, None).is_err());
        // A folder with other files in it isn't taken.
        let busy = dir.path().join("busy");
        std::fs::create_dir(&busy).unwrap();
        std::fs::write(busy.join("x"), b"x").unwrap();
        assert!(start("xbox", &game, &busy, None).is_err());
    }

    #[test]
    fn an_interrupted_stage_reads_as_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let console = super::super::console("ps3").unwrap();
        let mut p = Project::new(console, dir.path(), dir.path());
        if let Some(s) = p.stage(Stage::Compile) {
            s.status = StageStatus::Running;
        }
        p.save().unwrap();
        assert_eq!(load(dir.path()).unwrap().stages[6].status, StageStatus::Interrupted);
    }

    #[test]
    fn copy_tree_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::write(a.join("sub").join("f"), b"123").unwrap();
        std::fs::write(a.join(".git"), b"x").unwrap();
        let skip = |p: &Path| p.file_name().is_some_and(|n| n == ".git");
        assert_eq!(copy_tree(&a, &b, &CancelToken::default(), &skip).unwrap(), 1);
        assert_eq!(copy_tree(&a, &b, &CancelToken::default(), &skip).unwrap(), 0);
        assert!(!b.join(".git").exists());
    }
}
