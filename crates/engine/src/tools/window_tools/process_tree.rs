//! A runner such as `go run` or `dotnet run` may own no window. Only inspect its descendants;
//! a process with the same name elsewhere on the desktop is not part of this capture.

use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::process::Process;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

const MAX_PROCESSES: usize = 32_768;

/// Read only numeric process relationships, with bounded output and a deadline.
pub(super) fn snapshot(cancel: &crate::tool::CancelToken) -> Result<String, ToolError> {
    #[cfg(windows)]
    let command = {
        let windir = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        Process::new(windir.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")).args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference = 'Stop'; Get-CimInstance Win32_Process | ForEach-Object { '{0} {1}' -f $_.ProcessId, $_.ParentProcessId }",
        ])
    };
    #[cfg(not(windows))]
    let command = Process::new("/bin/ps").args(["-A", "-o", "pid=,ppid="]);
    let cancel = cancel.clone();
    let out = command
        .timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(10))
        .stdout_cap(1024 * 1024)
        .cancel_when(move || cancel.is_cancelled())
        .run_checked()?;
    if out.stdout_truncated {
        return Err(ToolError::new(ErrorCode::ProgramFailed, "The process list was too large to identify the application's window safely."));
    }
    Ok(out.stdout_text())
}

/// Breadth-first descendants of `roots` (not the roots), regardless of snapshot order. Bound
/// malformed/cyclic input too.
fn descendants(roots: &[u32], text: &str) -> Vec<u32> {
    let mut children = BTreeMap::<u32, Vec<u32>>::new();
    for line in text.lines().take(MAX_PROCESSES) {
        let mut words = line.split_whitespace();
        if let (Some(pid), Some(parent), None) = (words.next(), words.next(), words.next())
            && let (Ok(pid), Ok(parent)) = (pid.parse::<u32>(), parent.parse::<u32>())
            && pid != 0
        {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut seen: BTreeSet<u32> = roots.iter().copied().collect();
    let mut pending: VecDeque<u32> = roots.iter().copied().collect();
    let mut result = Vec::new();
    while let Some(parent) = pending.pop_front() {
        for pid in children.remove(&parent).unwrap_or_default() {
            if seen.insert(pid) {
                result.push(pid);
                pending.push_back(pid);
            }
        }
    }
    result
}

/// Most processes a run may have started before its window can't be told apart safely.
const MAX_FAMILY: usize = 64;

/// Return the window owner's PID alongside its picture so accessibility uses that same process.
pub(super) fn capture<T>(
    root: u32,
    snapshot: impl FnOnce() -> Result<String, ToolError>,
    picture: impl FnMut(u32) -> Result<T, ToolError>,
) -> Result<(u32, T), ToolError> {
    Family::new(root).capture(snapshot, picture)
}

/// A started program and every process seen below it so far. A launcher that starts the real
/// program and exits orphans it: macOS and Linux give the child a new parent, so a later process
/// list no longer leads from the program to it. Looking again from everything seen before keeps
/// that child (and what it starts) part of the run.
pub(super) struct Family {
    root: u32,
    known: Vec<u32>,
}

impl Family {
    pub(super) fn new(root: u32) -> Self {
        Family { root, known: Vec::new() }
    }

    /// The first of the family (the program first, then the processes seen earliest) whose
    /// picture can be taken, with its PID.
    pub(super) fn capture<T>(
        &mut self,
        snapshot: impl FnOnce() -> Result<String, ToolError>,
        mut picture: impl FnMut(u32) -> Result<T, ToolError>,
    ) -> Result<(u32, T), ToolError> {
        let missing = match picture(self.root) {
            Ok(shot) => return Ok((self.root, shot)),
            Err(e) if e.code == ErrorCode::NotFound => e,
            Err(e) => return Err(e),
        };
        let mut roots = Vec::with_capacity(self.known.len() + 1);
        roots.push(self.root);
        roots.extend_from_slice(&self.known);
        for pid in descendants(&roots, &snapshot()?) {
            if pid != self.root && !self.known.contains(&pid) {
                self.known.push(pid);
            }
            if self.known.len() > MAX_FAMILY {
                return Err(ToolError::new(ErrorCode::ProgramFailed, "The program started too many processes to identify its window safely."));
            }
        }
        for &pid in &self.known {
            match picture(pid) {
                Ok(shot) => return Ok((pid, shot)),
                Err(e) if e.code == ErrorCode::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Err(missing)
    }
}

/// Look until something other than "no window yet" comes back, or `can_wait` says to stop.
pub(super) fn keep_looking<T>(mut look: impl FnMut() -> Result<T, ToolError>, mut can_wait: impl FnMut() -> bool, pause: Duration) -> Result<T, ToolError> {
    loop {
        match look() {
            Err(e) if e.code == ErrorCode::NotFound && can_wait() => std::thread::sleep(pause),
            done => return done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_window_is_waited_for_but_not_forever_and_other_errors_end_it() {
        let mut looks = 0;
        let found = keep_looking(
            || {
                looks += 1;
                if looks < 3 { Err(missing()) } else { Ok("picture") }
            },
            || true,
            Duration::ZERO,
        );
        assert_eq!(found.unwrap(), "picture");
        assert_eq!(looks, 3);

        let mut waits = 0;
        let gave_up = keep_looking::<()>(
            || Err(missing()),
            || {
                waits += 1;
                waits < 3
            },
            Duration::ZERO,
        );
        assert_eq!(gave_up.unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(waits, 3);

        let mut looks = 0;
        let denied = keep_looking::<()>(
            || {
                looks += 1;
                Err(ToolError::new(ErrorCode::ProgramFailed, "denied"))
            },
            || true,
            Duration::ZERO,
        );
        assert_eq!(denied.unwrap_err().message, "denied");
        assert_eq!(looks, 1, "only a missing window is worth another look");
    }

    fn missing() -> ToolError {
        ToolError::new(ErrorCode::NotFound, "No window")
    }

    #[test]
    fn runner_capture_returns_application_pid_and_ignores_unrelated_windows() {
        let mut looked_up = Vec::new();
        let (tree_pid, shot) = capture(
            10,
            || Ok("30 20\n99 1\n20 10\n".into()),
            |pid| {
                looked_up.push(pid);
                if pid == 30 || pid == 99 { Ok("application picture") } else { Err(missing()) }
            },
        )
        .unwrap();
        assert_eq!(looked_up, [10, 20, 30]);
        assert_eq!(tree_pid, 30, "UI tree must use the actual window owner");
        assert_eq!(shot, "application picture");
        assert!(capture::<()>(10, || Ok("99 1\n".into()), |_| Err(missing())).is_err());
    }

    #[test]
    fn direct_window_needs_no_process_list_and_capture_errors_are_preserved() {
        assert_eq!(capture(10, || panic!("unneeded process list"), |_| Ok("direct")).unwrap(), (10, "direct"));
        let error = capture::<()>(10, || panic!("capture permission failure"), |_| Err(ToolError::new(ErrorCode::ProgramFailed, "denied"))).unwrap_err();
        assert_eq!(error.message, "denied");
        assert!(capture::<()>(10, || Err(ToolError::cancelled()), |_| Err(missing())).is_err());
    }

    #[test]
    fn a_child_stays_the_program_after_its_launcher_exits() {
        // Look 1: the launcher (10) has started the game (20), which has no window yet.
        // Look 2: the launcher has exited and the game was given a new parent (1, as macOS and
        // Linux do); the game now has a window, and so does an unrelated program (99).
        let mut family = Family::new(10);
        let first = family.capture::<&str>(|| Ok("20 10\n99 1\n".into()), |_| Err(missing()));
        assert_eq!(first.unwrap_err().code, ErrorCode::NotFound);
        let mut looked_up = Vec::new();
        let (pid, shot) = family
            .capture(
                || Ok("20 1\n99 1\n30 20\n".into()),
                |pid| {
                    looked_up.push(pid);
                    if pid == 20 || pid == 99 { Ok("game") } else { Err(missing()) }
                },
            )
            .unwrap();
        assert_eq!((pid, shot), (20, "game"), "the orphaned child is still the program");
        assert_eq!(looked_up, [10, 20], "only the program's own processes are looked at");
        // Without that memory the orphan isn't found, and the unrelated window never is.
        assert!(capture::<()>(10, || Ok("20 1\n99 1\n".into()), |_| Err(missing())).is_err());
    }

    #[test]
    fn process_relationships_are_bounded_and_cycle_safe() {
        assert_eq!(descendants(&[10], "20 10\n30 20\n10 30\n20 10\n0 10\nbad\n40 10 extra\n"), [20, 30]);
        let text = format!("{}20 10\n", "bad\n".repeat(MAX_PROCESSES));
        assert!(descendants(&[10], &text).is_empty());
        let text = (20..85).map(|pid| format!("{pid} 10\n")).collect::<String>();
        let mut attempts = 0;
        let error = capture::<()>(
            10,
            || Ok(text),
            |_| {
                attempts += 1;
                Err(missing())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ProgramFailed);
        assert_eq!(attempts, 1, "oversized process families must not trigger repeated window scans");
    }

    /// Exercise the OS snapshot without opening any windows or requiring a language toolchain.
    #[cfg(unix)]
    #[test]
    fn snapshot_finds_an_owned_child_on_this_host() {
        let mut child = Process::new("/bin/sleep").arg("20").timeout(Duration::from_secs(30)).spawn().unwrap();
        let family = snapshot(&crate::CancelToken::default()).map(|text| descendants(&[std::process::id()], &text));
        assert!(child.kill_and_wait(Duration::from_secs(5)));
        assert!(family.unwrap().contains(&child.pid()));
    }
}
