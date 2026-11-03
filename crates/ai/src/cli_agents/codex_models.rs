//! Codex's own list of models and reasoning levels, as an agent model catalogue ([`AgentModel`]):
//! Settings → AI, `ai model`, `ai_models_list`.
//!
//! Codex keeps the models it offers in `$CODEX_HOME/models_cache.json` (it refreshes the file
//! itself). Its model names change often, so none are written here: they're read from that file.
//! Every reader takes a path; only [`home`] looks at the environment.

use super::models::AgentModel;
use crate::model::Effort;
use crate::{BackendConfig, BackendOptions};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Codex's list of models, in its folder.
pub const MODELS_FILE: &str = "models_cache.json";
/// The biggest models list read (Codex's own is a few hundred KB).
pub const MAX_MODELS_BYTES: u64 = 16 * 1024 * 1024;
/// The most models kept from the list.
pub const MAX_MODELS: usize = 500;
/// The levels offered when Codex's list doesn't give a model's own: ones every Codex reasoning
/// model takes.
const USUAL_LEVELS: [Effort; 3] = [Effort::Low, Effort::Medium, Effort::High];
/// The longest model id taken.
const MAX_ID_LEN: usize = 200;

/// Codex's folder: `CODEX_HOME` when it's set (the person's choice, which the Codex we start keeps
/// too), else `~/.codex`.
pub fn home() -> Option<PathBuf> {
    home_from(std::env::var_os("CODEX_HOME"), dirs::home_dir())
}

/// [`home`] from the variable's value and the home folder. A relative `CODEX_HOME` names no folder
/// we can read: Codex runs in each project's folder, so it would mean a different folder there
/// than here. Then none is read (not `~/.codex` either, which that Codex doesn't use).
pub fn home_from(codex_home: Option<std::ffi::OsString>, home_dir: Option<PathBuf>) -> Option<PathBuf> {
    match codex_home.filter(|v| !v.is_empty()).map(PathBuf::from) {
        Some(set) => set.is_absolute().then_some(set),
        None => home_dir.map(|h| h.join(".codex")),
    }
}

#[derive(Deserialize)]
struct Cache {
    #[serde(default)]
    models: Vec<Value>,
}

#[derive(Deserialize)]
struct Entry {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Vec<Value>,
    #[serde(default)]
    visibility: Visibility,
    #[serde(default)]
    priority: Option<i64>,
}

/// Whether Codex shows a model in its own picker.
#[derive(Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Visibility {
    List,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Level {
    effort: String,
}

/// A model id Codex can be asked for (`-m`): one word with no control characters.
pub fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID_LEN && !s.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The models Codex lists for people (hidden ones left out), in its order, each with the levels
/// it names that are thinking depths ([`Effort`]; its `minimal` and `none` aren't, and stay
/// reachable through the backend's own arguments), or the usual ones when it names no levels. A model whose entry can't be read is skipped;
/// a file that isn't a models list is an error.
pub fn parse_models(bytes: &[u8]) -> Result<Vec<AgentModel>, ToolError> {
    let cache: Cache = serde_json::from_slice(bytes).map_err(|_| {
        ToolError::new(ErrorCode::GarbledOutput, "Codex's list of models couldn't be read.")
            .with_remediation("Run Codex once to refresh its list, then reload.")
    })?;
    let mut listed: Vec<(i64, AgentModel)> = Vec::new();
    for v in cache.models {
        let Ok(e) = serde_json::from_value::<Entry>(v) else { continue };
        let id = e.slug.trim().to_string();
        if e.visibility != Visibility::List || !valid_id(&id) || listed.iter().any(|(_, m)| m.id == id) {
            continue;
        }
        let levels: Vec<String> = e.supported_reasoning_levels.into_iter().filter_map(|l| serde_json::from_value::<Level>(l).ok()).map(|l| l.effort).collect();
        // The usual levels only when the list names none; a model it gives only `minimal` or `none`
        // for offers no depth, just its default.
        let thinking = if levels.is_empty() {
            USUAL_LEVELS.to_vec()
        } else {
            Effort::ALL.into_iter().filter(|d| levels.iter().any(|l| Effort::parse(l) == Some(*d))).collect()
        };
        let name = e.display_name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| id.clone());
        listed.push((e.priority.unwrap_or(i64::MAX), AgentModel { id, name, thinking }));
        if listed.len() >= MAX_MODELS {
            break;
        }
    }
    // Stable: models of the same priority keep the file's order.
    listed.sort_by_key(|(p, _)| *p);
    Ok(listed.into_iter().map(|(_, m)| m).collect())
}

/// Codex's list of models in `path` (`MODELS_FILE` in its folder); empty when there's no such
/// file: Codex hasn't run here yet.
pub fn read_models(path: &Path) -> Result<Vec<AgentModel>, ToolError> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ToolError::io(format!("reading {}", path.display()), e)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_MODELS_BYTES.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| ToolError::io(format!("reading {}", path.display()), e))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MODELS_BYTES {
        return Err(ToolError::new(ErrorCode::OverLimit, format!("Codex's list of models is bigger than {MAX_MODELS_BYTES} bytes.")));
    }
    parse_models(&bytes)
}

/// The models a Codex backend offers, from Codex's folder `home` on this computer. A Codex over
/// SSH keeps its list on that computer, which isn't read.
pub fn catalog(config: &BackendConfig, home: Option<&Path>) -> Result<Vec<AgentModel>, ToolError> {
    if config.ssh_host.as_deref().is_some_and(|h| !h.trim().is_empty()) {
        return Err(ToolError::invalid("Codex's models are listed only for a Codex on this computer.")
            .with_remediation(format!("Set its model by name: craft-anything-cli ai model {:?} <model> [--thinking <depth>].", config.name)));
    }
    let Some(home) = home else {
        return Err(ToolError::invalid("Codex's folder isn't known here (CODEX_HOME is a relative path).")
            .with_remediation("Set CODEX_HOME to a full path, or set the model by name with `craft-anything-cli ai model`."));
    };
    read_models(&home.join(MODELS_FILE))
}

/// Check a Codex choice before it's saved or run. Its list is a cache Codex refreshes itself, so a
/// model it doesn't name yet is taken (Codex says if it's wrong); a depth is refused only for a
/// model whose levels the list gives. A list that can't be read checks nothing more.
pub fn validate(config: &BackendConfig, options: &BackendOptions, home: Option<&Path>) -> Result<(), ToolError> {
    if options.cli_model.is_none() && options.cli_thinking.is_none() {
        return Ok(());
    }
    // Extra flags must not silently override the chosen model or depth.
    let overrides = options.args.iter().any(|a| a == "--model" || a.starts_with("--model=") || a.starts_with("-m") || a.contains("model_reasoning_effort"));
    if overrides {
        return Err(ToolError::invalid("Choose Codex's model and thinking depth in its model setting, rather than in extra arguments."));
    }
    let Some(id) = options.cli_model.as_deref() else { return Ok(()) };
    if !valid_id(id) {
        return Err(ToolError::invalid(format!("{id:?} isn't a model id Codex can be asked for."))
            .with_remediation("Use one word, such as the ids `ai model <name>` lists."));
    }
    let models = catalog(config, home).unwrap_or_default();
    match (options.cli_thinking, models.iter().find(|m| m.id == id)) {
        (Some(depth), Some(model)) if !model.thinking.contains(&depth) => Err(ToolError::invalid(format!("{id} does not offer {} thinking.", depth.title()))
            .with_remediation("Choose a thinking depth listed for this model, or use its default.")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_agents::{Access, McpCommand, adapter_for, build_args};
    use crate::{BackendKind, BackendOptions};

    /// A made-up list in Codex's shape: one hidden model, priorities out of file order, levels
    /// that aren't thinking depths, one entry that isn't a model, and fields we don't read.
    const CACHE: &str = r#"{
        "fetched_at": "2026-01-01T00:00:00Z",
        "etag": "x",
        "models": [
            {"slug": "fake-mid", "display_name": "Fake Mid", "description": "Middle one", "default_reasoning_level": "medium",
             "supported_reasoning_levels": [{"effort": "high", "description": "a"}, {"effort": "minimal"}, {"effort": "Low"}],
             "visibility": "list", "priority": 2, "context_window": 1000},
            {"slug": "fake-hidden", "visibility": "hide", "priority": 0},
            {"slug": "fake-top", "supported_reasoning_levels": [{"effort": "xhigh"}, {"effort": "ultra"}, {"effort": "bad level"}],
             "visibility": "list", "priority": 1},
            {"display_name": "no slug", "visibility": "list"},
            {"slug": "fake-last", "visibility": "list"},
            {"slug": "fake-mid", "visibility": "list", "priority": 0}
        ]
    }"#;

    fn codex(ssh: Option<&str>) -> BackendConfig {
        BackendConfig {
            name: "Codex".into(),
            kind: BackendKind::AgentCli,
            model: "codex".into(),
            base_url: None,
            ssh_host: ssh.map(str::to_string),
            enabled: true,
        }
    }

    #[test]
    fn listed_models_in_codex_order_without_hidden_ones() {
        let models = parse_models(CACHE.as_bytes()).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["fake-top", "fake-mid", "fake-last"], "by priority, hidden and broken entries skipped, a repeat kept once");
        assert_eq!(models[1].name, "Fake Mid");
        assert_eq!(models[1].thinking, [Effort::Low, Effort::High], "least to most; `minimal` isn't a depth");
        assert_eq!(models[0].thinking, [Effort::Xhigh], "levels that aren't depths are dropped");
        assert_eq!((models[2].name.as_str(), models[2].thinking.as_slice()), ("fake-last", USUAL_LEVELS.as_slice()), "no levels listed: the usual ones");
    }

    /// A model the list gives only levels that aren't depths offers none: the usual ones are for a
    /// model whose levels the list doesn't give at all.
    #[test]
    fn listed_levels_are_never_padded_with_the_usual_ones() {
        let list = br#"{"models": [
            {"slug": "fake-quick", "supported_reasoning_levels": [{"effort": "minimal"}, {"effort": "none"}], "visibility": "list"},
            {"slug": "fake-odd", "supported_reasoning_levels": [{"no_effort": "x"}], "visibility": "list"}
        ]}"#;
        let models = parse_models(list).unwrap();
        assert!(models[0].thinking.is_empty(), "{:?}", models[0]);
        assert_eq!(models[1].thinking, USUAL_LEVELS, "entries that can't be read name no levels");
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(MODELS_FILE), list).unwrap();
        let options = BackendOptions { cli_model: Some("fake-quick".into()), cli_thinking: Some(Effort::Low), ..Default::default() };
        assert_eq!(validate(&codex(None), &options, Some(d.path())).unwrap_err().code, ErrorCode::BadInput, "a depth it doesn't list is refused");
    }

    #[test]
    fn a_list_with_nothing_or_nonsense_never_crashes() {
        assert!(parse_models(br#"{"models": []}"#).unwrap().is_empty());
        assert!(parse_models(b"{}").unwrap().is_empty());
        assert!(parse_models(br#"{"models": [1, "x", null, {"slug": 5}]}"#).unwrap().is_empty());
        assert_eq!(parse_models(b"not json").unwrap_err().code, ErrorCode::GarbledOutput);
        assert!(parse_models(br#"{"models": "x"}"#).is_err());
        assert!(parse_models(&[0xff, 0xfe, 0x00]).is_err());
    }

    #[test]
    fn the_file_is_read_bounded_and_a_missing_one_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(MODELS_FILE);
        assert!(read_models(&p).unwrap().is_empty(), "Codex hasn't run here");
        std::fs::write(&p, CACHE).unwrap();
        assert_eq!(catalog(&codex(None), Some(d.path())).unwrap().len(), 3);
        // Over the limit: refused without reading it all.
        std::fs::write(&p, vec![b' '; usize::try_from(MAX_MODELS_BYTES).unwrap() + 1]).unwrap();
        assert_eq!(read_models(&p).unwrap_err().code, ErrorCode::OverLimit);
        // A folder where the file should be is an error, not a crash.
        let dir = d.path().join("sub");
        std::fs::create_dir(&dir).unwrap();
        assert!(read_models(&dir).is_err());
    }

    /// Over SSH, or with no folder to read, the list is refused with a way to name the model.
    #[test]
    fn only_a_codex_on_this_computer_is_listed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(MODELS_FILE), CACHE).unwrap();
        let remote = catalog(&codex(Some("build-box")), Some(d.path())).unwrap_err();
        assert!(remote.remediation.as_deref().is_some_and(|r| r.contains("ai model \"Codex\"")), "{remote:?}");
        assert!(catalog(&codex(None), None).is_err());
        assert!(catalog(&codex(Some("  ")), Some(d.path())).is_ok(), "a blank host is this computer");
    }

    #[test]
    fn a_relative_codex_home_is_ignored() {
        let home = Some(PathBuf::from(if cfg!(windows) { r"C:\Users\me" } else { "/home/me" }));
        let abs = if cfg!(windows) { r"D:\codex" } else { "/opt/codex" };
        assert_eq!(home_from(Some(abs.into()), home.clone()), Some(PathBuf::from(abs)));
        assert_eq!(home_from(Some("rel/codex".into()), home.clone()), None, "not this process's folder, nor ~/.codex");
        assert_eq!(home_from(Some("".into()), home.clone()), home.clone().map(|h| h.join(".codex")), "empty: as if unset");
        assert_eq!(home_from(None, home.clone()), home.map(|h| h.join(".codex")));
        assert_eq!(home_from(None, None), None);
    }

    #[test]
    fn model_ids_are_checked() {
        assert!(valid_id("fake-1.5-mini") && valid_id("org/model:tag"));
        for bad in ["", "two words", "a\nb", &"m".repeat(201)] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    /// A model the list doesn't name yet is taken, a depth the listed model lacks isn't, and extra
    /// arguments can't override the choice.
    #[test]
    fn a_choice_is_checked_against_what_the_list_knows() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(MODELS_FILE), CACHE).unwrap();
        let check = |model: Option<&str>, thinking: Option<Effort>, args: &[&str]| {
            let options = BackendOptions {
                cli_model: model.map(str::to_string),
                cli_thinking: thinking,
                args: args.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            };
            validate(&codex(None), &options, Some(d.path()))
        };
        assert!(check(None, None, &["-m", "legacy"]).is_ok(), "no choice: extra arguments are the person's own");
        assert!(check(Some("fake-mid"), Some(Effort::High), &[]).is_ok());
        assert_eq!(check(Some("fake-mid"), Some(Effort::Max), &[]).unwrap_err().code, ErrorCode::BadInput);
        assert!(check(Some("typed-model-9"), Some(Effort::Max), &[]).is_ok(), "not in the list yet: Codex decides");
        assert!(check(None, Some(Effort::Low), &[]).is_ok(), "a depth for Codex's own default model");
        assert!(check(Some("two words"), None, &[]).is_err());
        for args in [&["-m", "x"][..], &["--model=x"], &["-c", "model_reasoning_effort=\"low\""]] {
            assert!(check(Some("fake-mid"), None, args).is_err(), "{args:?}");
        }
        // Over SSH the list isn't read: only the id is checked.
        let options = BackendOptions { cli_model: Some("fake-mid".into()), cli_thinking: Some(Effort::Max), ..Default::default() };
        assert!(validate(&codex(Some("build-box")), &options, Some(d.path())).is_ok());
    }

    /// Codex is asked for a model and a depth only when they're set; otherwise its own settings
    /// apply.
    #[test]
    fn codex_gets_model_and_effort_flags_exactly_when_set() {
        let mcp = McpCommand { command: "/opt/ca/craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        let args = |o: &BackendOptions| build_args(adapter_for("codex").as_ref(), o, "q", "", &mcp, None, Access::Analyse);
        let effort = |a: &[String]| a.iter().filter(|x| x.starts_with("model_reasoning_effort")).count();
        let plain = args(&BackendOptions::default());
        assert!(!plain.contains(&"-m".to_string()) && effort(&plain) == 0);
        let o = BackendOptions { cli_model: Some("fake-top".into()), cli_thinking: Some(Effort::Xhigh), ..Default::default() };
        let a = args(&o);
        assert!(a.windows(2).any(|w| w[0] == "-m" && w[1] == "fake-top"), "{a:?}");
        assert!(a.windows(2).any(|w| w[0] == "-c" && w[1] == "model_reasoning_effort=\"xhigh\""), "{a:?}");
        assert_eq!(effort(&a), 1);
        // Before the backend's own arguments.
        let mine = BackendOptions { args: vec!["--own-flag".into()], ..o.clone() };
        let a = args(&mine);
        let pos = |x: &str| a.iter().position(|y| y == x).unwrap();
        assert!(pos("model_reasoning_effort=\"xhigh\"") < pos("--own-flag"), "{a:?}");
        let only_effort = args(&BackendOptions { cli_thinking: Some(Effort::Low), ..Default::default() });
        assert!(!only_effort.contains(&"-m".to_string()) && effort(&only_effort) == 1);
        // Claude Code never gets Codex's flag.
        let claude = build_args(adapter_for("claude-code").as_ref(), &o, "q", "", &mcp, None, Access::Analyse);
        assert_eq!(effort(&claude), 0);
    }
}
