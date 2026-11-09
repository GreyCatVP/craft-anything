//! Keyboard and mouse input sent to the system, for short scripted sessions with a program the
//! person asked us to run (ROADMAP R2.4). Uses the system's own input through `enigo` on Windows
//! and macOS; elsewhere it says it isn't available yet.
//!
//! Input goes wherever the system sends it (the window in front), so callers check that the
//! program's window is in front before every event ([`crate::window::window_place`]); this module
//! only sends. [`InputSink`] is the seam: the system's input is one implementation, tests use their
//! own and never move the real pointer or press a real key.

use craft_anything_evidence::{ErrorCode, ToolError};

/// Whether this system can send input: Windows and macOS for now (ROADMAP R2.4).
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

/// Why it can't, where it can't.
pub fn unsupported() -> ToolError {
    ToolError::new(ErrorCode::FeatureMissing, "Sending keys and clicks to a program works on Windows and macOS, not on this system yet.")
        .with_remediation("Play the program yourself and add screenshots of what you see instead.")
}

/// A key a script can press. Letters and digits are typed as their character; the rest are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Shift,
    Control,
    Alt,
    /// The Windows key, or Command on a Mac.
    Meta,
    /// F1 to F12.
    F(u8),
}

/// The names a script may use for keys other than letters and digits (lower case).
pub const KEY_NAMES: &[(&str, Key)] = &[
    ("space", Key::Space),
    ("enter", Key::Enter),
    ("escape", Key::Escape),
    ("tab", Key::Tab),
    ("backspace", Key::Backspace),
    ("delete", Key::Delete),
    ("up", Key::Up),
    ("down", Key::Down),
    ("left", Key::Left),
    ("right", Key::Right),
    ("home", Key::Home),
    ("end", Key::End),
    ("page_up", Key::PageUp),
    ("page_down", Key::PageDown),
    ("shift", Key::Shift),
    ("control", Key::Control),
    ("alt", Key::Alt),
    ("meta", Key::Meta),
];

impl Key {
    /// A key by its name: one letter or digit (`w`, `7`), a name from [`KEY_NAMES`], or `f1`–`f12`.
    /// Case doesn't matter; `ctrl`, `cmd`, `return` and `esc` are accepted too.
    pub fn named(name: &str) -> Option<Key> {
        let lower = name.trim().to_ascii_lowercase();
        let mut chars = lower.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_ascii_alphanumeric()
        {
            return Some(Key::Char(c));
        }
        let lower = match lower.as_str() {
            "ctrl" => "control",
            "cmd" | "command" | "win" | "super" => "meta",
            "return" => "enter",
            "esc" => "escape",
            "pageup" => "page_up",
            "pagedown" => "page_down",
            other => other,
        };
        if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            return (1..=12).contains(&n).then_some(Key::F(n));
        }
        KEY_NAMES.iter().find(|(n, _)| *n == lower).map(|(_, k)| *k)
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Where input goes. Every call sends one event at once; holding and timing are the caller's.
pub trait InputSink {
    /// Press (`down`) or release a key.
    fn key(&mut self, key: Key, down: bool) -> Result<(), ToolError>;
    /// Type text as characters (the layout's own keys).
    fn text(&mut self, text: &str) -> Result<(), ToolError>;
    /// Move the pointer to screen coordinates and say where it ended up (a system may scale
    /// or clamp, and the caller must not click somewhere it didn't aim).
    fn pointer_to(&mut self, x: i32, y: i32) -> Result<(i32, i32), ToolError>;
    /// Press (`down`) or release a mouse button where the pointer is.
    fn button(&mut self, button: Button, down: bool) -> Result<(), ToolError>;
}

/// The system's own keyboard and mouse. On macOS it needs Accessibility permission, which is
/// checked here and never asked for with a dialog.
pub fn system_input() -> Result<Box<dyn InputSink>, ToolError> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let settings = enigo::Settings { open_prompt_to_get_permissions: false, release_keys_when_dropped: true, ..Default::default() };
        match enigo::Enigo::new(&settings) {
            Ok(enigo) => Ok(Box::new(SystemInput { enigo })),
            Err(enigo::NewConError::NoPermission) => Err(ToolError::new(
                ErrorCode::NoPermission,
                "The program wasn't started: this app may not send keys and clicks without Accessibility permission.",
            )
            .with_remediation(
                "Allow the app running Craft Anything in System Settings → Privacy & Security → Accessibility, restart that app, then run this again.",
            )),
            Err(e) => Err(ToolError::new(ErrorCode::FeatureMissing, format!("Can't send input on this computer: {e}"))),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    Err(unsupported())
}

#[cfg(any(windows, target_os = "macos"))]
struct SystemInput {
    enigo: enigo::Enigo,
}

#[cfg(any(windows, target_os = "macos"))]
fn failed(what: &str, e: enigo::InputError) -> ToolError {
    ToolError::new(ErrorCode::ProgramFailed, format!("The system refused the {what}: {e}"))
}

#[cfg(any(windows, target_os = "macos"))]
fn direction(down: bool) -> enigo::Direction {
    if down { enigo::Direction::Press } else { enigo::Direction::Release }
}

#[cfg(any(windows, target_os = "macos"))]
fn system_key(key: Key) -> enigo::Key {
    use enigo::Key as K;
    match key {
        Key::Char(c) => physical_key(c).map_or(K::Unicode(c), K::Other),
        Key::Space => K::Space,
        Key::Enter => K::Return,
        Key::Escape => K::Escape,
        Key::Tab => K::Tab,
        Key::Backspace => K::Backspace,
        Key::Delete => K::Delete,
        Key::Up => K::UpArrow,
        Key::Down => K::DownArrow,
        Key::Left => K::LeftArrow,
        Key::Right => K::RightArrow,
        Key::Home => K::Home,
        Key::End => K::End,
        Key::PageUp => K::PageUp,
        Key::PageDown => K::PageDown,
        Key::Shift => K::Shift,
        Key::Control => K::Control,
        Key::Alt => K::Alt,
        Key::Meta => K::Meta,
        Key::F(1) => K::F1,
        Key::F(2) => K::F2,
        Key::F(3) => K::F3,
        Key::F(4) => K::F4,
        Key::F(5) => K::F5,
        Key::F(6) => K::F6,
        Key::F(7) => K::F7,
        Key::F(8) => K::F8,
        Key::F(9) => K::F9,
        Key::F(10) => K::F10,
        Key::F(11) => K::F11,
        // `Key::named` only makes F1–F12.
        Key::F(_) => K::F12,
    }
}

/// The key in a letter's or digit's place on a US keyboard, whatever layout is in use: a script's
/// `w` is the key a game reads as forward, not whatever key types "w" in the current layout. As a
/// character, enigo maps it through the layout of the window in front, and where that layout has
/// no such key (Arabic, Russian…) it types the character as text on both the press and the
/// release, ignoring Shift: a live Shift+C typed "cc".
#[cfg(windows)]
fn physical_key(c: char) -> Option<u32> {
    // Virtual-key codes: 'A'–'Z' are 0x41–0x5A and '0'–'9' are 0x30–0x39, their ASCII values.
    c.is_ascii_alphanumeric().then(|| u32::from(c.to_ascii_uppercase()))
}

/// The same on a Mac: its ANSI key codes (`kVK_ANSI_*`), which name places on the keyboard.
#[cfg(target_os = "macos")]
fn physical_key(c: char) -> Option<u32> {
    const CODES: &[(char, u32)] = &[
        ('a', 0x00),
        ('s', 0x01),
        ('d', 0x02),
        ('f', 0x03),
        ('h', 0x04),
        ('g', 0x05),
        ('z', 0x06),
        ('x', 0x07),
        ('c', 0x08),
        ('v', 0x09),
        ('b', 0x0B),
        ('q', 0x0C),
        ('w', 0x0D),
        ('e', 0x0E),
        ('r', 0x0F),
        ('y', 0x10),
        ('t', 0x11),
        ('1', 0x12),
        ('2', 0x13),
        ('3', 0x14),
        ('4', 0x15),
        ('6', 0x16),
        ('5', 0x17),
        ('9', 0x19),
        ('7', 0x1A),
        ('8', 0x1C),
        ('0', 0x1D),
        ('o', 0x1F),
        ('u', 0x20),
        ('i', 0x22),
        ('p', 0x23),
        ('l', 0x25),
        ('j', 0x26),
        ('k', 0x28),
        ('n', 0x2D),
        ('m', 0x2E),
    ];
    let c = c.to_ascii_lowercase();
    CODES.iter().find(|(k, _)| *k == c).map(|(_, code)| *code)
}

#[cfg(any(windows, target_os = "macos"))]
impl InputSink for SystemInput {
    fn key(&mut self, key: Key, down: bool) -> Result<(), ToolError> {
        enigo::Keyboard::key(&mut self.enigo, system_key(key), direction(down)).map_err(|e| failed("key", e))
    }

    fn text(&mut self, text: &str) -> Result<(), ToolError> {
        enigo::Keyboard::text(&mut self.enigo, text).map_err(|e| failed("typing", e))
    }

    fn pointer_to(&mut self, x: i32, y: i32) -> Result<(i32, i32), ToolError> {
        enigo::Mouse::move_mouse(&mut self.enigo, x, y, enigo::Coordinate::Abs).map_err(|e| failed("pointer move", e))?;
        enigo::Mouse::location(&self.enigo).map_err(|e| failed("pointer position", e))
    }

    fn button(&mut self, button: Button, down: bool) -> Result<(), ToolError> {
        let b = match button {
            Button::Left => enigo::Button::Left,
            Button::Right => enigo::Button::Right,
            Button::Middle => enigo::Button::Middle,
        };
        enigo::Mouse::button(&mut self.enigo, b, direction(down)).map_err(|e| failed("click", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_found_by_their_names() {
        assert_eq!(Key::named("W"), Some(Key::Char('w')));
        assert_eq!(Key::named("7"), Some(Key::Char('7')));
        assert_eq!(Key::named("Space"), Some(Key::Space));
        assert_eq!(Key::named("ctrl"), Some(Key::Control));
        assert_eq!(Key::named("cmd"), Some(Key::Meta));
        assert_eq!(Key::named("esc"), Some(Key::Escape));
        assert_eq!(Key::named("Return"), Some(Key::Enter));
        assert_eq!(Key::named("page_up"), Some(Key::PageUp));
        assert_eq!(Key::named("f1"), Some(Key::F(1)));
        assert_eq!(Key::named("F12"), Some(Key::F(12)));
        assert_eq!(Key::named("f"), Some(Key::Char('f')), "one letter is the letter, not a function key");
        for bad in ["", "f0", "f13", "fx", "ww", "é", "-", "shift+w", "launch_missiles"] {
            assert_eq!(Key::named(bad), None, "{bad:?}");
        }
        for (name, key) in KEY_NAMES {
            assert_eq!(Key::named(name), Some(*key), "every listed name is found");
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn every_function_key_maps_to_its_own_system_key() {
        let keys: std::collections::BTreeSet<String> = (1..=12).map(|n| format!("{:?}", system_key(Key::F(n)))).collect();
        assert_eq!(keys.len(), 12);
    }

    /// Letters and digits are sent as places on the keyboard, never through the current layout.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn every_letter_and_digit_is_its_own_physical_key() {
        let all: Vec<char> = ('a'..='z').chain('0'..='9').collect();
        let codes: std::collections::BTreeSet<u32> = all.iter().filter_map(|c| physical_key(*c)).collect();
        assert_eq!(codes.len(), all.len(), "each has a code of its own");
        for c in all {
            assert!(matches!(system_key(Key::Char(c)), enigo::Key::Other(_)), "{c}");
            assert_eq!(physical_key(c), physical_key(c.to_ascii_uppercase()));
        }
        #[cfg(windows)]
        {
            assert_eq!(physical_key('w'), Some(0x57));
            assert_eq!(physical_key('0'), Some(0x30));
        }
    }
}
