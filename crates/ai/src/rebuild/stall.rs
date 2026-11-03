//! A build that goes round and round with nothing written (#110, #44, #417, #481).
//!
//! A model that can't use the file tools (a small local one above all) answers "I fixed it" while
//! `app/` stays empty; the run checks, refines and asks again, each round costing a whole turn.
//! Each check compares `app/` with how it was at the last one: when it didn't change and the build
//! still isn't good, that's a stalled check; after [`STALL_LIMIT`] in a row the run stops and says
//! why. The count is kept in the project (`Manifest::stalled_checks`), so Try again gets one more
//! turn, not two more rounds, and anything written to `app/` starts it again.

use craft_anything_evidence::{ErrorCode, ToolError};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Checks in a row that found `app/` unchanged and the build not good before the run stops.
pub const STALL_LIMIT: u32 = 2;

/// Folders whose contents a build tool writes, not the builder: they say nothing about progress.
const OUTPUT_FOLDERS: &[&str] = &["node_modules", "target", ".git", "dist", "build", ".next", "__pycache__", ".venv", ".gradle"];

/// How many entries are looked at: a huge folder is not worth walking on every check.
const MAX_ENTRIES: usize = 50_000;

/// A number that changes when a file under `dir` is added, removed, resized or rewritten (build
/// outputs and links aren't looked at). Order doesn't matter, so the same files give the same number.
pub fn fingerprint(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(folder) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return total;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let name = entry.file_name();
            if kind.is_dir() {
                if !OUTPUT_FOLDERS.iter().any(|o| name.to_str() == Some(o)) {
                    todo.push(entry.path());
                }
            } else if kind.is_file() {
                let Ok(meta) = entry.metadata() else { continue };
                let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
                let mut h = DefaultHasher::new();
                entry.path().strip_prefix(dir).unwrap_or(&entry.path()).hash(&mut h);
                (meta.len(), modified).hash(&mut h);
                total = total.wrapping_add(h.finish());
            }
        }
    }
    total
}

/// The stalled count after a check: one more when `app/` is as it was, none when it changed.
pub fn next(count: u32, before: u64, after: u64) -> u32 {
    if before == after { count.saturating_add(1) } else { 0 }
}

/// Whether the run stops.
pub fn stopped(count: u32) -> bool {
    count >= STALL_LIMIT
}

/// What the run says when it stops.
pub fn error() -> ToolError {
    ToolError::new(
        ErrorCode::ProgramFailed,
        "The builder answered twice without writing anything to app/, so the build stopped here instead of checking again. The AI probably can't use the file tools.",
    )
    .with_remediation("Choose a stronger AI to build with in Settings → AI (a bigger local model, or Claude Code), then press Try again. Anything it writes starts the count again.")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #110: the number moves when the builder writes, and not for what a build tool writes.
    #[test]
    fn app_changes_show_in_the_fingerprint_and_build_outputs_do_not() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        let empty = fingerprint(app);
        std::fs::write(app.join("main.rs"), "fn main() {}").unwrap();
        let one = fingerprint(app);
        assert_ne!(empty, one, "a new file");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::write(app.join("src/lib.rs"), "").unwrap();
        let two = fingerprint(app);
        assert_ne!(one, two, "a file in a folder");
        std::fs::write(app.join("main.rs"), "fn main() { println!(); }").unwrap();
        assert_ne!(two, fingerprint(app), "a file rewritten");
        let before = fingerprint(app);
        for out in OUTPUT_FOLDERS {
            std::fs::create_dir_all(app.join(out).join("deep")).unwrap();
            std::fs::write(app.join(out).join("deep/x.bin"), "built").unwrap();
        }
        assert_eq!(before, fingerprint(app), "build outputs say nothing about progress");
        assert_eq!(fingerprint(&app.join("missing")), 0, "a folder that isn't there");
        std::fs::remove_file(app.join("src/lib.rs")).unwrap();
        assert_ne!(before, fingerprint(app), "a file removed");
    }

    #[test]
    fn two_unchanged_checks_in_a_row_stop_the_run_and_a_change_starts_again() {
        let mut count = 0;
        count = next(count, 7, 7);
        assert!(!stopped(count), "one is not enough");
        count = next(count, 7, 7);
        assert!(stopped(count));
        // Try again carries the count: one more unchanged check stops at once, a change starts over.
        assert!(stopped(next(count, 7, 7)));
        assert_eq!(next(count, 7, 8), 0);
        assert!(!stopped(next(count, 7, 8)));
        assert_eq!(next(u32::MAX, 1, 1), u32::MAX, "no overflow");
    }

    #[test]
    fn the_stop_says_what_happened_and_what_to_do() {
        let e = error();
        assert_eq!(e.code, ErrorCode::ProgramFailed);
        assert!(e.message.contains("without writing anything to app/"), "{}", e.message);
        let fix = e.remediation.unwrap_or_default();
        assert!(fix.contains("Settings → AI") && fix.contains("Try again"), "{fix}");
    }
}
