//! Pictures of a running program's window, for studying how an app or game looks while it runs
//! (and comparing a rebuild with it). Uses the operating system's own capture through `xcap`, on
//! Windows and macOS; elsewhere it says it isn't available yet.

use craft_anything_evidence::{ErrorCode, ToolError};

/// Whether this system can picture a window: Windows and macOS for now (ROADMAP R2.4).
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

/// Why it can't, where it can't.
pub fn unsupported() -> ToolError {
    ToolError::new(ErrorCode::FeatureMissing, "Picturing a program's window works on Windows and macOS, not on this system yet.")
        .with_remediation("Add pictures of the original yourself (screenshots) instead.")
}

/// A window's picture: 8-bit RGBA, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowShot {
    pub title: String,
    pub app: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Whether this app may see other programs' windows. Without Screen Recording, macOS hides window
/// names and the capture library skips those windows, so a program that opened one looks as if it
/// hadn't. Ask before starting a program: nothing can be pictured, and starting it can have effects.
pub fn check_can_see_windows() -> Result<(), ToolError> {
    #[cfg(target_os = "macos")]
    return permission_result(objc2_core_graphics::CGPreflightScreenCaptureAccess());
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

#[cfg(target_os = "macos")]
fn permission_result(allowed: bool) -> Result<(), ToolError> {
    if allowed {
        return Ok(());
    }
    Err(ToolError::new(ErrorCode::NoPermission, "The program wasn't started: this app can't see windows without Screen Recording permission.")
        .with_remediation(
            "Allow the app running Craft Anything in System Settings → Privacy & Security → Screen Recording, restart that app, then run this again.",
        ))
}

/// Whether a window's owner is the program called `want` (lowercase): the name on its own or as
/// whole words, so `win` never finds "Window Server". The system's own windows are never the
/// program's, whatever it's called.
#[cfg(any(windows, target_os = "macos"))]
fn is_named(owner: &str, want: &str) -> bool {
    let owner = owner.to_ascii_lowercase();
    if want.is_empty() || owner == "window server" {
        return false;
    }
    owner.match_indices(want).any(|(at, _)| {
        let before = owner[..at].chars().next_back();
        let after = owner[at + want.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Where a program's window is on the screen and whether it has the keyboard, read just now.
/// `x`/`y`/`width`/`height` are the system's screen coordinates for the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPlace {
    pub pid: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The window is the one in front, that keyboard and mouse input goes to.
    pub focused: bool,
}

#[cfg(any(windows, target_os = "macos"))]
fn all_windows() -> Result<Vec<xcap::Window>, ToolError> {
    xcap::Window::all().map_err(|e| ToolError::new(ErrorCode::FeatureMissing, format!("Can't list windows on this computer: {e}")))
}

/// The largest visible window that belongs to `pid`, or (when none does, as with a launcher that
/// starts the real program) to a process whose name matches `app_name`.
#[cfg(any(windows, target_os = "macos"))]
fn choose<'a>(windows: &'a [xcap::Window], pid: u32, app_name: Option<&str>) -> Result<&'a xcap::Window, ToolError> {
    let usable = |w: &xcap::Window| !w.is_minimized().unwrap_or(true) && w.width().unwrap_or(0) > 32 && w.height().unwrap_or(0) > 32;
    let area = |w: &xcap::Window| u64::from(w.width().unwrap_or(0)) * u64::from(w.height().unwrap_or(0));
    let by_pid = windows.iter().filter(|w| usable(w) && w.pid().ok() == Some(pid)).max_by_key(|w| area(w));
    let chosen = by_pid.or_else(|| {
        let want = app_name?.to_ascii_lowercase();
        windows.iter().filter(|w| usable(w) && w.app_name().is_ok_and(|n| is_named(&n, &want))).max_by_key(|w| area(w))
    });
    chosen.ok_or_else(|| {
        ToolError::new(ErrorCode::NotFound, "The program didn't show a window to picture.")
            .with_remediation("Give it longer to start (wait_secs), or check that it opens a window.")
    })
}

/// Where `pid`'s largest visible window is and whether it's in front (no name fallback: input is
/// only ever aimed at a window the program itself owns).
#[cfg(any(windows, target_os = "macos"))]
pub fn window_place(pid: u32) -> Result<WindowPlace, ToolError> {
    let windows = all_windows()?;
    let w = choose(&windows, pid, None)?;
    let unreadable = |what: &str, e: xcap::XCapError| ToolError::new(ErrorCode::ProgramFailed, format!("Couldn't read the window's {what}: {e}"));
    Ok(WindowPlace {
        pid,
        title: w.title().unwrap_or_default(),
        x: w.x().map_err(|e| unreadable("position", e))?,
        y: w.y().map_err(|e| unreadable("position", e))?,
        width: w.width().map_err(|e| unreadable("size", e))?,
        height: w.height().map_err(|e| unreadable("size", e))?,
        // Unknown counts as not in front: input then isn't sent.
        focused: w.is_focused().unwrap_or(false),
    })
}

/// Linux and the BSDs: not built in yet (ROADMAP R2.4).
#[cfg(not(any(windows, target_os = "macos")))]
pub fn window_place(_pid: u32) -> Result<WindowPlace, ToolError> {
    Err(unsupported())
}

/// Picture the largest visible window that belongs to `pid`, or (when none does, as with a
/// launcher that starts the real program) to a process whose name matches `app_name`.
#[cfg(any(windows, target_os = "macos"))]
pub fn capture_window(pid: u32, app_name: Option<&str>) -> Result<WindowShot, ToolError> {
    let windows = all_windows()?;
    let w = choose(&windows, pid, app_name)?;
    let img = w.capture_image().map_err(|e| ToolError::new(ErrorCode::ProgramFailed, format!("Couldn't picture the window: {e}")))?;
    Ok(WindowShot {
        title: w.title().unwrap_or_default(),
        app: w.app_name().unwrap_or_default(),
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Linux and the BSDs: not built in yet (ROADMAP R2.4).
#[cfg(not(any(windows, target_os = "macos")))]
pub fn capture_window(_pid: u32, _app_name: Option<&str>) -> Result<WindowShot, ToolError> {
    Err(unsupported())
}

#[cfg(all(test, any(windows, target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn a_name_matches_whole_words_and_never_the_system() {
        assert!(is_named("Game", "game"));
        assert!(is_named("Game Launcher", "game"));
        assert!(is_named("game.exe", "game"));
        assert!(is_named("My Game Client", "my game"));
        assert!(!is_named("Window Server", "win"), "the menu bar was pictured as a program called win");
        assert!(!is_named("Window Server", "window"));
        assert!(!is_named("Trunk", "run"));
        assert!(!is_named("Game", ""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_missing_permission_says_where_to_allow_it() {
        assert!(permission_result(true).is_ok());
        let e = permission_result(false).unwrap_err();
        assert_eq!(e.code, ErrorCode::NoPermission);
        assert!(e.message.contains("wasn't started"));
        assert!(e.remediation.as_deref().is_some_and(|r| r.contains("Screen Recording")), "{e:?}");
    }
}
