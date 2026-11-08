//! Agent sessions a turn has started, kept on disk while the turn runs (#1115). A turn that ends
//! deletes its own unfinished session; this covers the turns that never got to: Craft Anything
//! killed, crashed or uninstalled mid-turn. Each turn holds an OS lock on its record, which the
//! system releases when the process ends, so a record nobody holds belongs to a turn that's gone,
//! and [`sweep`] deletes its session. One file per turn, so turns never write each other's.

use super::adapters::AgentAdapters;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const VERSION: u32 = 1;
/// Sweeps that try a record's delete before giving up on it (a host that's gone, a session the
/// person deleted already).
const TRIES: u32 = 3;
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The folder for the records inside the logs folder ([`crate::failure_log::dir_in`]).
pub fn dir_in(logs: &Path) -> PathBuf {
    logs.join("unfinished-sessions")
}

/// What it takes to delete one session later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub version: u32,
    /// The agent id, to find its adapter ([`AgentAdapters::for_agent`]).
    pub agent: String,
    pub program: String,
    /// The SSH host it ran on; None: this computer.
    pub host: Option<String>,
    pub id: String,
    #[serde(default)]
    pub tries: u32,
}

impl Entry {
    pub fn new(agent: &str, program: &str, host: Option<&str>, id: &str) -> Entry {
        Entry { version: VERSION, agent: agent.into(), program: program.into(), host: host.map(str::to_string), id: id.into(), tries: 0 }
    }
}

/// A running turn's record, locked until it's dropped.
#[derive(Debug)]
pub struct Held {
    file: File,
    path: PathBuf,
}

impl Held {
    /// Record `entry` in `dir`, locked before anything is written so no sweep can take it.
    pub fn record(dir: &Path, entry: &Entry) -> std::io::Result<Held> {
        std::fs::create_dir_all(dir)?;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // The time too: a dead process's leftover may carry a pid the system has handed out again.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let path = dir.join(format!("{}-{now}-{n}.json", std::process::id()));
        let mut file = OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
        file.lock()?;
        file.write_all(&serde_json::to_vec(entry).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        Ok(Held { file, path })
    }

    /// Its session is gone or finished: remove the record (still locked, so no sweep reads it).
    pub fn done(self) {
        let _ = std::fs::remove_file(&self.path);
        drop(self.file);
    }
}

/// Delete the session of every record no running turn holds. Returns what to tell the person:
/// a line per session deleted, or given up on.
pub fn sweep(dir: &Path, adapters: &AgentAdapters) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut said = Vec::new();
    for path in rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")) {
        // Held by a running turn, or deleted under us: not ours to touch.
        let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&path) else { continue };
        if file.try_lock().is_err() {
            continue;
        }
        let mut text = String::new();
        let entry = file.read_to_string(&mut text).ok().and_then(|_| serde_json::from_str::<Entry>(&text).ok());
        let Some(mut entry) = entry.filter(|e| e.version == VERSION) else {
            // Unreadable or from a newer version: leave it as it is.
            continue;
        };
        let on = entry.host.as_ref().map(|h| format!(" on {h}")).unwrap_or_default();
        match adapters.for_agent(&entry.agent).discard_unfinished(&entry.program, &entry.id, entry.host.as_deref()) {
            Ok(()) => {
                let _ = std::fs::remove_file(&path);
                said.push(format!("Removed session {}{on}, left unfinished by a build Craft Anything didn't get to stop.", entry.id));
            }
            Err(e) => {
                entry.tries += 1;
                if entry.tries >= TRIES {
                    let _ = std::fs::remove_file(&path);
                    let fix = e.remediation.map(|r| format!(" {r}")).unwrap_or_default();
                    said.push(format!("Couldn't remove session {}{on}, left unfinished by an earlier build: {}.{fix}", entry.id, e.message));
                } else if let Ok(bytes) = serde_json::to_vec(&entry) {
                    let _ = file.set_len(0).and_then(|_| file.rewind()).and_then(|_| file.write_all(&bytes));
                }
            }
        }
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_agents::adapters::AgentAdapter;
    use craft_anything_evidence::{ErrorCode, ToolError};
    use craft_anything_platform::sync::Mutex;
    use std::sync::{Arc, PoisonError};

    const OWNER_ENV: &str = "CRAFT_ANYTHING_TEST_UNFINISHED_OWNER";
    /// What the stub was asked to delete: program, session, host.
    type Asked = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

    /// Records what it was asked to delete; fails when told to.
    struct Stub {
        fail: bool,
        asked: Asked,
    }
    impl AgentAdapter for Stub {
        /// Register the fixture without replacing any built-in agent.
        fn ids(&self) -> &'static [&'static str] {
            &["test-sweep"]
        }
        fn args(&self, _cx: &crate::cli_agents::adapters::ArgsCx) -> Vec<String> {
            Vec::new()
        }
        fn discard_unfinished(&self, program: &str, id: &str, host: Option<&str>) -> Result<(), ToolError> {
            self.asked.lock().unwrap_or_else(PoisonError::into_inner).push((program.into(), id.into(), host.map(str::to_string)));
            if self.fail { Err(ToolError::new(ErrorCode::ProgramFailed, "it said no").with_remediation("Do it by hand.")) } else { Ok(()) }
        }
    }

    fn adapters(fail: bool) -> (AgentAdapters, Asked) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let mut a = AgentAdapters::default();
        a.register(Arc::new(Stub { fail, asked: asked.clone() }));
        (a, asked)
    }

    fn records(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|rd| rd.flatten().count()).unwrap_or(0)
    }

    /// The owner of `a_killed_turns_session_is_deleted_by_the_next_sweep`: records a session, then
    /// ends the process without cleaning up, as a crash or a kill does. Run as an ordinary test, it
    /// does nothing.
    #[test]
    fn killed_owner_child() {
        let Some(dir) = std::env::var_os(OWNER_ENV) else { return };
        let held = Held::record(Path::new(&dir), &Entry::new("test-sweep", "agent-cli", Some("studio"), "ses_killed"));
        assert!(held.is_ok(), "{held:?}");
        std::process::exit(0);
    }

    #[test]
    fn a_killed_turns_session_is_deleted_by_the_next_sweep() {
        let d = tempfile::tempdir().unwrap();
        let dir = dir_in(d.path());
        let run = craft_anything_platform::process::Process::new(std::env::current_exe().unwrap().as_os_str())
            .args(["--exact", "--test-threads=1", "cli_agents::unfinished::tests::killed_owner_child"])
            .env(OWNER_ENV, dir.as_os_str())
            .timeout(std::time::Duration::from_secs(60))
            .run();
        assert!(run.as_ref().is_ok_and(|o| o.success), "{run:?}");
        assert_eq!(records(&dir), 1, "the owner left its record");
        let (a, asked) = adapters(false);
        let said = sweep(&dir, &a);
        assert_eq!(*asked.lock().unwrap_or_else(PoisonError::into_inner), [("agent-cli".into(), "ses_killed".into(), Some("studio".into()))]);
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(records(&dir), 0);
    }

    #[test]
    fn a_running_turns_record_is_left_alone_and_done_removes_it() {
        let d = tempfile::tempdir().unwrap();
        let (a, asked) = adapters(false);
        let held = Held::record(d.path(), &Entry::new("test-sweep", "agent-cli", None, "ses_live")).unwrap();
        assert!(sweep(d.path(), &a).is_empty());
        assert!(asked.lock().unwrap_or_else(PoisonError::into_inner).is_empty(), "a held record isn't swept");
        held.done();
        assert_eq!(records(d.path()), 0);
        assert!(sweep(d.path(), &a).is_empty());
        assert!(asked.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    }

    #[test]
    fn a_failing_delete_is_tried_again_then_given_up_on() {
        let d = tempfile::tempdir().unwrap();
        let (a, asked) = adapters(true);
        // Dropped without `done`: its lock goes, as when the process ends.
        drop(Held::record(d.path(), &Entry::new("test-sweep", "agent-cli", None, "ses_stuck")).unwrap());
        for _ in 1..TRIES {
            assert!(sweep(d.path(), &a).is_empty());
            assert_eq!(records(d.path()), 1, "kept for the next sweep");
        }
        let said = sweep(d.path(), &a);
        assert!(said.len() == 1 && said[0].contains("ses_stuck") && said[0].contains("Do it by hand."), "{said:?}");
        assert_eq!(records(d.path()), 0);
        assert_eq!(asked.lock().unwrap_or_else(PoisonError::into_inner).len(), TRIES as usize);
    }

    #[test]
    fn a_record_it_cant_read_is_left_as_it_is() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("x.json"), "{\"version\":99}").unwrap();
        std::fs::write(d.path().join("y.json"), "not json").unwrap();
        let (a, asked) = adapters(false);
        assert!(sweep(d.path(), &a).is_empty());
        assert!(asked.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
        assert_eq!(records(d.path()), 2);
    }
}
