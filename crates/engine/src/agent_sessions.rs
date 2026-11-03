//! Where a coding agent keeps the conversations it ran in a project's folder. The app reads this to
//! show and choose one (`project_conversations_list`, `project_conversation_use`), so a session
//! begun in plain Claude Code is seen here and the other way round.

use std::path::{Path, PathBuf};

/// The id Claude Code's backend saves in a project's `agent_session_by`.
pub const CLAUDE: &str = "claude-code";

/// Folders under Claude Code's `projects/` looked through for a session.
const SESSION_FOLDERS: usize = 10_000;

/// A conversation an agent saved for a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    /// The session's id: the name of its file.
    pub id: String,
    /// When it last changed, in milliseconds since the Unix epoch (0 when unknown).
    pub modified_ms: u64,
    /// Its size in bytes.
    pub bytes: u64,
}

/// Claude Code's own folder: `CLAUDE_CONFIG_DIR` when it's set (the person's choice, which the
/// agent we start keeps too), else our own under a Flatpak (see [`flatpak_claude_dir`]), else
/// `~/.claude`.
pub fn claude_config_dir() -> Option<PathBuf> {
    let set = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
    set.or_else(flatpak_claude_dir).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
}

/// Whether this app runs in a Flatpak sandbox (it sets `FLATPAK_ID` and makes `/.flatpak-info`).
pub fn in_flatpak() -> bool {
    static IN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IN.get_or_init(|| std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists())
}

/// Where Claude Code keeps its conversations when we start it under a Flatpak and the person
/// hasn't chosen a folder: its own is `~/.claude`, which the sandbox only lets us read, so a
/// conversation was never saved and `--resume` found nothing (#506). A folder of ours in the
/// app's data folder is writable, and kept with the app. `None` outside a Flatpak, or when
/// `CLAUDE_CONFIG_DIR` is set.
pub fn flatpak_claude_dir() -> Option<PathBuf> {
    let chosen = std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|v| !v.is_empty());
    flatpak_claude_dir_in(in_flatpak(), chosen, dirs::data_dir())
}

pub fn flatpak_claude_dir_in(flatpak: bool, chosen: bool, data: Option<PathBuf>) -> Option<PathBuf> {
    (flatpak && !chosen).then_some(data?.join("craft-anything").join("claude"))
}

/// Claude Code's name for the folder a session ran in: the path with everything but letters and
/// digits made `-`. It finds a session by its id in any of these folders (2.1), so a long path
/// cut short still works.
pub fn claude_project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).take(200).collect()
}

/// Whether `id` can name a session: a UUID, or what an agent wrote there.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Claude Code keeps each session as `projects/<the folder it ran in>/<id>.jsonl` under its
/// config folder, whichever folder it ran in: find it by its id.
pub fn claude_session_file(config: &Path, id: &str) -> Option<PathBuf> {
    if !valid_session_id(id) {
        return None;
    }
    let name = format!("{id}.jsonl");
    std::fs::read_dir(config.join("projects")).ok()?.flatten().take(SESSION_FOLDERS).map(|e| e.path().join(&name)).find(|p| p.is_file())
}

/// The conversations Claude Code saved for `cwd`, newest first.
pub fn claude_sessions(cwd: &Path) -> Vec<SessionFile> {
    claude_config_dir().map_or_else(Vec::new, |config| claude_sessions_in(&config, cwd))
}

/// The conversations Claude Code saved for `cwd` under `config`, newest first. A folder it never
/// ran in has none.
pub fn claude_sessions_in(config: &Path, cwd: &Path) -> Vec<SessionFile> {
    let folder = config.join("projects").join(claude_project_folder(cwd));
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<SessionFile> = entries
        .flatten()
        .take(SESSION_FOLDERS)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_string();
            // A subagent's own transcript sits beside the conversation as agent-*.jsonl.
            if !valid_session_id(&id) || id.starts_with("agent-") {
                return None;
            }
            let meta = std::fs::metadata(&path).ok()?;
            Some(SessionFile { id, modified_ms: modified_ms(&meta), bytes: meta.len() })
        })
        .collect();
    found.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.id.cmp(&b.id)));
    found
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    let since = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    since.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folders_sessions_are_listed_newest_first_and_only_its_own() {
        let d = tempfile::tempdir().unwrap();
        let config = d.path().join("claude");
        let cwd = d.path().join("app");
        let folder = config.join("projects").join(claude_project_folder(&cwd));
        std::fs::create_dir_all(&folder).unwrap();
        for id in ["11111111-1111-1111-1111-111111111111", "22222222-2222-2222-2222-222222222222", "agent-3333"] {
            std::fs::write(folder.join(format!("{id}.jsonl")), "{}").unwrap();
        }
        std::fs::write(folder.join("notes.txt"), "not a session").unwrap();
        // Another folder's session is not this folder's.
        let other = config.join("projects").join(claude_project_folder(&d.path().join("elsewhere")));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("44444444-4444-4444-4444-444444444444.jsonl"), "{}").unwrap();

        let found = claude_sessions_in(&config, &cwd);
        let ids: Vec<&str> = found.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(!ids.contains(&"agent-3333"));
        assert!(!ids.contains(&"44444444-4444-4444-4444-444444444444"));
        // Written in the same instant or not, the list is ordered, and every entry is a real file.
        assert!(found.windows(2).all(|w| w[0].modified_ms >= w[1].modified_ms));
        for s in &found {
            assert!(claude_session_file(&config, &s.id).is_some());
        }
    }

    #[test]
    fn a_folder_claude_never_ran_in_has_no_sessions() {
        let d = tempfile::tempdir().unwrap();
        assert!(claude_sessions_in(&d.path().join("nowhere"), &d.path().join("app")).is_empty());
    }

    #[test]
    fn only_session_shaped_ids_name_a_file() {
        let long = "x".repeat(65);
        for bad in ["", "../escape", "a/b", long.as_str()] {
            assert!(!valid_session_id(bad), "{bad:?}");
            assert_eq!(claude_session_file(Path::new("."), bad), None);
        }
        assert!(valid_session_id("0e4a6b1c-1111-4222-8333-944455556666"));
    }
}
