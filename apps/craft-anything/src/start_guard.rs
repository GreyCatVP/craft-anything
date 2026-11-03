//! A start that never reached a window (#74, #104, #33).
//!
//! A graphics driver that crashes natively as the window opens takes the whole process with it:
//! no Rust code runs afterwards, so no dialog and no log line can say what happened, and the app
//! just vanishes. What can be done is to leave a mark before trying and remove it once the window
//! is open and has shown its first frames (the driver may crash while presenting them, after
//! eframe's setup callback has run); a mark found at the next start means the last one never got
//! there, and the app draws without the graphics card that time (see `main`). A start that does
//! open the window clears the mark, so the next one is back to normal.

use std::path::{Path, PathBuf};

/// Beside the settings, with `desktop.log`.
const MARKER: &str = "starting";

fn marker(dir: &Path) -> PathBuf {
    dir.join(MARKER)
}

/// Whether the last start ended before its window opened.
pub fn last_start_failed(dir: &Path) -> bool {
    marker(dir).exists()
}

/// Called right before the window is asked for. Best effort: a folder that can't be written to
/// only means no fallback.
pub fn begin(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(marker(dir), std::process::id().to_string());
}

/// Called once the window is open.
pub fn opened(dir: &Path) {
    let _ = std::fs::remove_file(marker(dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #74: a start that dies before its window leaves the mark; one that opens it doesn't.
    #[test]
    fn a_start_that_never_opened_a_window_is_seen_by_the_next_one() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("config");
        assert!(!last_start_failed(&dir), "a first start has nothing to retry");
        begin(&dir);
        assert!(last_start_failed(&dir), "the process died before opened()");
        begin(&dir);
        assert!(last_start_failed(&dir), "a second start in a row still sees it");
        opened(&dir);
        assert!(!last_start_failed(&dir), "a window that opened clears it");
        opened(&dir);
        assert!(!last_start_failed(&dir), "clearing twice is fine");
    }

    #[test]
    fn a_folder_that_cannot_be_written_means_no_fallback_and_no_crash() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("not-a-folder");
        std::fs::write(&file, "").unwrap();
        begin(&file);
        assert!(!last_start_failed(&file));
        opened(&file);
    }
}
