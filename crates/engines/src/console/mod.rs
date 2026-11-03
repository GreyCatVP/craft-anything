//! Recompiling original console titles: one backend per console, each wrapping an open-source
//! static recompiler. A backend says what it is built on, which computers its output really runs
//! on, and what a person's game files hold and whether that backend can take them.
//!
//! Nothing here decrypts, unlocks or works around copy protection. An encrypted executable is
//! reported as encrypted, with what the backend needs instead; files are only read.
//!
//! Adding a console: implement [`Console`] in a module of its own and add it to [`CONSOLES`].

use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::cancel::CancelToken as Cancel;
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};

pub mod build;
pub mod ps3;
pub mod xbox;
pub mod xbox_audio;
pub mod xbox_map;

/// Every console that can be recompiled, in the order people see them.
pub static CONSOLES: &[&dyn Console] = &[&xbox::Xbox, &ps3::Ps3];

/// The console with this id.
pub fn console(id: &str) -> Option<&'static dyn Console> {
    CONSOLES.iter().copied().find(|c| c.id() == id)
}

/// One console's recompilation backend.
pub trait Console: Sync {
    /// Stable id: `xbox`, `ps3`.
    fn id(&self) -> &'static str;
    /// The name people see.
    fn name(&self) -> &'static str;
    /// The project it's built on.
    fn backend(&self) -> &'static Backend;
    /// Every system people may ask about, with what the backend's output does there.
    fn targets(&self) -> &'static [Target];
    /// What a game image, folder or executable holds, and whether this backend can take it.
    fn inspect(&self, input: &Path, cancel: &Cancel) -> Result<Inspection, ToolError>;
    /// Run one stage of a build (see [`build`]), returning what it did in a sentence.
    fn run_stage(&self, stage: build::Stage, cx: &mut build::StageCx, project: &mut build::Project) -> Result<String, ToolError>;
}

/// The open-source recompiler behind a console.
#[derive(Debug, Serialize)]
pub struct Backend {
    pub name: &'static str,
    pub repository: &'static str,
    pub license: &'static str,
    /// Which version of it a build uses.
    pub version: &'static str,
    /// Programs the backend's own build needs on the computer.
    pub needs: &'static [&'static str],
    /// What it takes as input, in a sentence.
    pub takes: &'static str,
}

/// An operating system a recompiled game might be built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Windows,
    Linux,
    Macos,
    Android,
    Ios,
}

impl Os {
    /// The name people see.
    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
            Os::Macos => "macOS",
            Os::Android => "Android",
            Os::Ios => "iOS",
        }
    }

    /// The system this program runs on.
    pub fn host() -> Option<Os> {
        match std::env::consts::OS {
            "windows" => Some(Os::Windows),
            "linux" => Some(Os::Linux),
            "macos" => Some(Os::Macos),
            _ => None,
        }
    }
}

/// How far a backend's output goes on a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// Builds, and games run there.
    Runs,
    /// The code builds there, but no game is known to run yet.
    BuildsOnly,
    /// Not possible with this backend today.
    Unavailable,
}

/// One row of a backend's platform table.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Target {
    pub os: Os,
    /// `x86_64`, `aarch64`.
    pub arch: &'static str,
    pub support: Support,
    /// Why, in a sentence.
    pub note: &'static str,
}

/// How a person's game was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    DiscImage,
    Folder,
    Executable,
}

/// One executable found in the game.
#[derive(Debug, Clone, Serialize)]
pub struct Executable {
    /// Path inside the image or folder, `/`-separated.
    pub path: String,
    pub size: u64,
    /// The backend-specific kind (`xbe`, `ppu_elf`, `retail_self`, …).
    pub kind: String,
    /// Whether the backend can read its code as it is.
    pub readable: bool,
}

/// The result of one compatibility check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Note,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub status: Status,
    pub message: String,
}

/// Whether the backend can start on this game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every hard requirement is met; warnings may still apply.
    Ready,
    /// Something the person must supply or change first.
    Blocked,
}

/// What [`Console::inspect`] found.
#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub console: &'static str,
    pub input: PathBuf,
    pub input_kind: InputKind,
    pub title: Option<String>,
    pub title_id: Option<String>,
    /// The executable the game starts from, when found.
    pub main_executable: Option<String>,
    pub executables: Vec<Executable>,
    pub checks: Vec<Check>,
    pub verdict: Verdict,
    /// Console-specific facts (an XBE's libraries, a PARAM.SFO's values).
    pub details: serde_json::Value,
    /// A limit stopped the look early.
    pub truncated: bool,
}

impl Inspection {
    pub(crate) fn finish(mut self) -> Self {
        self.checks.sort_by_key(|c| std::cmp::Reverse(c.status));
        self.verdict = if self.checks.iter().any(|c| c.status == Status::Fail) { Verdict::Blocked } else { Verdict::Ready };
        self
    }
}

pub(crate) fn check(id: &'static str, status: Status, message: impl Into<String>) -> Check {
    Check { id, status, message: message.into() }
}

/// The host check every backend shares: can this computer build and run what the backend makes?
pub(crate) fn host_check(targets: &[Target]) -> Check {
    let row = Os::host().and_then(|os| targets.iter().find(|t| t.os == os && t.arch == std::env::consts::ARCH));
    match row.map(|t| t.support) {
        Some(Support::Runs) => check("host", Status::Pass, "This computer can build and run what this backend makes."),
        Some(Support::BuildsOnly) => check(
            "host",
            Status::Warn,
            format!("This computer can build the code, but no game is known to run here yet: {}", row.map(|t| t.note).unwrap_or_default()),
        ),
        _ => check(
            "host",
            Status::Warn,
            "This backend's output doesn't build on this computer; the project can still be made here and built on a supported one.",
        ),
    }
}

/// Folders deep and entries looked at in a game's folder.
pub(crate) const MAX_DEPTH: usize = 6;
pub(crate) const MAX_ENTRIES: usize = 50_000;

/// A file found in a game folder: its path relative to the folder (with `/`), full path and size.
pub(crate) type Found = (String, PathBuf, u64);

/// Files in `root`, up to the limits. Returns (files, truncated).
pub(crate) fn walk(root: &Path, cancel: &Cancel) -> Result<(Vec<Found>, bool), ToolError> {
    let (mut out, mut seen, mut stack) = (Vec::new(), 0usize, vec![(root.to_path_buf(), 0usize)]);
    while let Some((dir, depth)) = stack.pop() {
        cancel.check()?;
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Ok((out, true));
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push((rel, path, size));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((out, false))
}

/// The first `n` bytes of a file (fewer if it's shorter).
pub(crate) fn read_head(path: &Path, n: u64) -> Result<Vec<u8>, ToolError> {
    let f = std::fs::File::open(path).map_err(|e| ToolError::new(ErrorCode::FileError, format!("Couldn't open {}: {e}", path.display())))?;
    let mut out = Vec::new();
    f.take(n).read_to_end(&mut out).map_err(|e| ToolError::new(ErrorCode::FileError, format!("Couldn't read {}: {e}", path.display())))?;
    Ok(out)
}

/// A file read through [`craft_anything_formats::xdvdfs::ReadAt`], for disc images too large to load.
pub(crate) struct FileAt(pub std::fs::File);

impl craft_anything_formats::xdvdfs::ReadAt for FileAt {
    fn read_at(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        use std::io::{Seek, SeekFrom};
        if len > (64 << 20) {
            return None;
        }
        self.0.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0u8; len];
        self.0.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

/// The input as an absolute, existing path.
pub(crate) fn existing(input: &Path) -> Result<InputKind, ToolError> {
    if !input.is_absolute() {
        return Err(ToolError::invalid(format!("{:?} isn't an absolute path", input.display().to_string()))
            .with_remediation("Pass the full path of the game's disc image, folder or executable."));
    }
    if input.is_dir() {
        Ok(InputKind::Folder)
    } else if input.is_file() {
        Ok(InputKind::Executable) // the caller tells a disc image apart by its contents
    } else {
        Err(ToolError::new(ErrorCode::BadInput, format!("Nothing at {}", input.display()))
            .with_remediation("Check the path; pick the game's disc image, folder or executable."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_console_is_listed_once_with_an_honest_table() {
        let mut ids: Vec<_> = CONSOLES.iter().map(|c| c.id()).collect();
        ids.dedup();
        assert_eq!(ids.len(), CONSOLES.len());
        for c in CONSOLES {
            assert!(console(c.id()).is_some());
            // Each table names every system once, and says why for each.
            for os in [Os::Windows, Os::Linux, Os::Macos, Os::Android, Os::Ios] {
                assert!(c.targets().iter().any(|t| t.os == os), "{} doesn't say what happens on {os:?}", c.id());
            }
            assert!(c.targets().iter().all(|t| !t.note.is_empty()));
            assert!(c.backend().repository.starts_with("https://github.com/"));
        }
    }

    #[test]
    fn a_relative_or_missing_path_is_refused() {
        let cancel = Cancel::default();
        for c in CONSOLES {
            assert!(c.inspect(Path::new("game.iso"), &cancel).is_err());
            let dir = tempfile::tempdir().unwrap();
            assert!(c.inspect(&dir.path().join("missing"), &cancel).is_err());
        }
    }
}
