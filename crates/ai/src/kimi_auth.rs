//! Kimi Code CLI's own Kimi account: `kimi login` signs in (the CLI opens the browser) and
//! `kimi logout` signs out. Craft Anything only reads whether that account exists and runs the
//! CLI's own commands, so the sign-in and the credentials stay Kimi's.

use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::process::Process;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The environment variable that moves Kimi Code CLI's folder (`~/.kimi-code` by default).
pub const SHARE_VAR: &str = "KIMI_CODE_HOME";
/// How long the CLI's own sign-in waits: its device code stays good for 30 minutes.
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(30 * 60);
/// The most of the CLI's output kept.
const MAX_OUTPUT: usize = 256 * 1024;

/// The folder Kimi Code CLI keeps its configuration and credentials in: `KIMI_CODE_HOME` when the
/// person set one, else `~/.kimi-code`, and the archived Kimi CLI's `~/.kimi` only when the
/// current one isn't there at all.
pub fn home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(SHARE_VAR).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let usual = dirs::home_dir()?;
    let current = usual.join(".kimi-code");
    if current.is_dir() {
        return Some(current);
    }
    let legacy = usual.join(".kimi");
    Some(if legacy.is_dir() { legacy } else { current })
}

/// Whether the CLI is signed in: a credential file it wrote (`credentials/*.json`).
pub fn signed_in(home: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(home.join("credentials")) else { return false };
    entries.flatten().take(64).any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
}

/// The `kimi` program to run: the one the person chose for this backend, else the one on PATH.
pub fn find_program(chosen: Option<&str>) -> Result<String, ToolError> {
    find_program_in(chosen, craft_anything_platform::which("kimi"))
}

/// The same choice, with what the PATH search found (so a test never depends on this computer).
pub(crate) fn find_program_in(chosen: Option<&str>, found: Option<std::path::PathBuf>) -> Result<String, ToolError> {
    if let Some(chosen) = chosen.map(str::trim).filter(|c| !c.is_empty()) {
        return Ok(chosen.to_string());
    }
    found.map(|p| p.display().to_string()).ok_or_else(|| {
        ToolError::new(ErrorCode::EngineMissing, "Kimi Code CLI isn't installed, or isn't on your PATH.")
            .with_remediation("Install Kimi Code CLI (npm install -g @moonshot-ai/kimi-code), or set its program in Settings → AI.")
    })
}

/// Sign in by running the CLI's own `kimi login`, which opens the browser. Blocking, with a
/// deadline: call it off the UI thread.
pub fn sign_in(program: &str) -> Result<String, ToolError> {
    run(program, "login", SIGN_IN_WAIT)
}

/// Sign out: the CLI keeps its account in a credential file, and this version has no `kimi logout`
/// command, so removing that file is the sign-out. The CLI's configuration stays, so it asks the
/// person to sign in again the next time it needs the account.
pub fn sign_out(_program: &str) -> Result<String, ToolError> {
    let Some(home) = home() else { return Err(ToolError::new(ErrorCode::BadSettings, "Couldn't find the Kimi Code folder on this computer.")) };
    let file = home.join("credentials").join("kimi-code.json");
    match std::fs::remove_file(&file) {
        Ok(()) => Ok("Signed out of Kimi Code.".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Kimi Code wasn't signed in.".to_string()),
        Err(e) => Err(ToolError::new(ErrorCode::FileError, format!("Couldn't remove {}: {e}", file.display()))),
    }
}

/// Run one of the CLI's own account commands and give back the last of what it said.
fn run(program: &str, command: &str, wait: Duration) -> Result<String, ToolError> {
    let out = Process::new(program)
        .arg(command)
        .timeout(wait)
        .stdout_cap(MAX_OUTPUT)
        .stderr_cap(MAX_OUTPUT)
        .run()
        .map_err(|e| ToolError::new(ErrorCode::EngineMissing, format!("Couldn't run {program} {command}: {e}")))?;
    if !out.success {
        let said = tail(out.stderr_text().trim());
        let said = if said.is_empty() { tail(out.stdout_text().trim()) } else { said };
        return Err(ToolError::new(
            ErrorCode::EngineMissing,
            format!("{program} {command} failed{}.", if said.is_empty() { String::new() } else { format!(": {said}") }),
        )
        .with_remediation("Run it in a terminal to see what it asks for, then try again."));
    }
    Ok(tail(out.stdout_text().trim()))
}

/// The last lines of what a command said, bounded for a message.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(6);
    let joined = lines[start..].join(" ");
    joined.chars().take(600).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_file_is_a_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!signed_in(dir.path()), "no credentials folder");
        std::fs::create_dir_all(dir.path().join("credentials")).unwrap();
        assert!(!signed_in(dir.path()), "an empty folder isn't a sign-in");
        std::fs::write(dir.path().join("credentials/kimi-code.json"), b"{}").unwrap();
        assert!(signed_in(dir.path()));
    }

    #[test]
    fn a_missing_program_is_named_with_a_fix() {
        let err = find_program_in(Some("  "), None).unwrap_err();
        assert!(err.remediation.is_some(), "{err:?}");
        assert_eq!(find_program_in(Some("C:/tools/kimi.exe"), None).unwrap(), "C:/tools/kimi.exe");
        let found = std::path::PathBuf::from("C:/Program Files/nodejs/kimi.cmd");
        assert_eq!(find_program_in(None, Some(found.clone())).unwrap(), found.display().to_string());
    }

    #[test]
    fn the_last_lines_of_an_answer_are_kept() {
        assert_eq!(tail("one\n\ntwo\nthree\n"), "one two three");
        assert!(tail(&"x".repeat(2000)).chars().count() <= 600);
    }
}
