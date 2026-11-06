//! The person's installed Grok Build CLI, driven over ACP (`grok agent stdio`). Sign-in stays in
//! `grok login`. Craft Anything checks that an auth file exists and does not read it.

use super::*;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Grok Build's ACP-only adapter. `acp = false` or an SSH host fails closed: the plain runner
/// would open the interactive TUI, which has no approval channel back to Craft Anything.
pub struct Grok;

impl AgentAdapter for Grok {
    fn ids(&self) -> &'static [&'static str] {
        &["grok"]
    }

    fn args(&self, _cx: &ArgsCx) -> Vec<String> {
        // The native runner refuses `requires_acp` before it asks for arguments.
        Vec::new()
    }

    fn requires_acp(&self) -> bool {
        true
    }

    fn has_model_catalog(&self) -> bool {
        true
    }

    /// Reasoning levels differ by model, so a model change has to open that model again.
    fn model_catalog_depends_on_selection(&self) -> bool {
        true
    }

    fn model_catalog(
        &self,
        config: &BackendConfig,
        options: &BackendOptions,
        directory: Option<&Path>,
        stop: &Stop,
    ) -> Result<Vec<models::AgentModel>, ToolError> {
        let metadata = super::super::acp_model_metadata(config, options, directory, stop)?;
        Ok(catalogue(&metadata, options))
    }

    fn validate_model(&self, config: &BackendConfig, options: &BackendOptions, directory: Option<&Path>, stop: &Stop) -> Result<(), ToolError> {
        stop.check()?;
        self.validate_options(options)?;
        if options.cli_model.is_none() && options.cli_thinking.is_none() {
            return Ok(());
        }
        models::validate(options, &self.model_catalog(config, options, directory, stop)?)
    }

    fn plain_fallback(&self) -> bool {
        false
    }

    fn answers_questions(&self) -> bool {
        true
    }

    fn model_in_use(&self, options: Option<&BackendOptions>) -> Option<String> {
        Some(
            options
                .and_then(|options| options.cli_model.as_deref().map(str::trim).filter(|model| !model.is_empty()).map(str::to_string))
                .unwrap_or_else(|| "Grok Build default".into()),
        )
    }

    fn model_choices(&self, config: &BackendConfig, options: &BackendOptions) -> Result<Vec<String>, ToolError> {
        Ok(self.model_catalog(config, options, None, &Stop::default())?.into_iter().map(|model| model.id).collect())
    }

    /// Reasoning is the shared thinking picker. Grok advertises no context window, and the
    /// Copilot context control would invent Standard and Extended tiers, so it stays off.
    fn supports_model_tuning(&self) -> bool {
        false
    }

    fn check_tool_server(&self, stop: &CancelToken) -> Result<(), ToolError> {
        cli_can_start(craft_anything_platform::self_cli_path().as_deref(), stop)
    }

    /// Extra arguments would land before `agent` and can request blanket approval. Supported
    /// settings are passed after `agent` by [`acp_mode_options`](Self::acp_mode_options).
    fn acp_options(&self, _options: &BackendOptions, _access: Access) -> Vec<String> {
        Vec::new()
    }

    fn validate_options(&self, options: &BackendOptions) -> Result<(), ToolError> {
        if options.args.is_empty() {
            Ok(())
        } else {
            Err(ToolError::invalid("Extra Grok arguments could bypass Craft Anything's approval checks.")
                .with_remediation("Remove this backend's extra arguments. Set its program, model and thinking through the backend options instead."))
        }
    }

    fn acp_mode_options(&self, options: &BackendOptions, _access: Access) -> Vec<String> {
        // `--no-leader` keeps this run off the person's shared Grok session, including one
        // already open in a terminal. Effort after `agent` is what the CLI actually applies.
        let mut args = vec!["--no-leader".to_string()];
        if let Some(effort) = chosen_effort(options) {
            args.extend(["--reasoning-effort".into(), effort]);
        }
        args
    }

    fn acp_trailing(&self) -> &'static [&'static str] {
        &["stdio"]
    }

    fn acp_session_config(&self, options: &BackendOptions) -> Vec<(String, String)> {
        // Model first. Switching model can reset the effort the CLI flag just set, so the
        // effort override is sent again afterwards.
        let mut config = Vec::new();
        if let Some(model) = options.cli_model.as_deref().map(str::trim).filter(|model| !model.is_empty()) {
            config.push(("model".into(), model.to_string()));
        }
        if let Some(effort) = chosen_effort(options) {
            config.push(("reasoning_effort".into(), effort));
        }
        config
    }

    fn sign_in_fix(&self) -> Option<String> {
        Some("Sign in with `grok login` in a terminal, then try again. Grok keeps the sign-in; Craft Anything never sees it.".into())
    }

    fn signed_in(&self, _program: &str) -> Option<bool> {
        let grok_home = std::env::var_os("GROK_HOME");
        // Existence only. The file holds the person's credentials and is not read.
        auth_path(grok_home.as_deref(), dirs::home_dir().as_deref()).map(|path| path.is_file())
    }
}

/// `GROK_HOME/auth.json` when that variable is set, otherwise `~/.grok/auth.json`.
fn auth_path(grok_home: Option<&OsStr>, user_home: Option<&Path>) -> Option<PathBuf> {
    let dir = match grok_home {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => user_home?.join(".grok"),
    };
    Some(dir.join("auth.json"))
}

/// Thinking depth, in the same order Copilot uses: an explicit picker choice, then a saved
/// named effort, then the older `effort` field.
fn chosen_effort(options: &BackendOptions) -> Option<String> {
    options
        .cli_thinking
        .map(|effort| effort.as_str().to_string())
        .or_else(|| options.cli_reasoning_effort.map(|effort| effort.as_str().to_string()))
        .or_else(|| options.effort.map(|effort| effort.as_str().to_string()))
}

fn catalogue(metadata: &super::super::acp::ModelMetadata, options: &BackendOptions) -> Vec<models::AgentModel> {
    let tuning = crate::model_options::ModelTuning::from_metadata(metadata.current_model.clone(), &metadata.config_options);
    let active = options.cli_model.as_deref().map(str::trim).filter(|model| !model.is_empty()).unwrap_or(metadata.current_model.as_str());
    let thinking: Vec<_> = tuning.reasoning.iter().filter_map(|choice| crate::Effort::parse(choice.value.as_str())).collect();
    let mut ids = metadata.models.clone();
    if !metadata.current_model.is_empty() {
        ids.push(metadata.current_model.clone());
    }
    let mut ordered = Vec::new();
    for id in ids {
        if !id.is_empty() && !ordered.contains(&id) {
            ordered.push(id);
        }
    }
    ordered.into_iter().map(|id| models::AgentModel { name: id.clone(), thinking: if id == active { thinking.clone() } else { Vec::new() }, id }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_is_acp_only_and_registered() {
        let adapter = AgentAdapters::built_in().for_agent("grok");
        assert!(adapter.ids().contains(&"grok"));
        assert!(adapter.requires_acp() && !adapter.plain_fallback() && adapter.answers_questions());
        assert!(adapter.has_model_catalog() && adapter.model_catalog_depends_on_selection());
        assert!(!adapter.supports_model_tuning());
        assert_eq!(adapter.acp_trailing(), &["stdio"]);
        assert_eq!(super::super::acp::acp_args("grok"), Some(&["agent"][..]));
        assert_eq!(super::super::acp::model_args("grok", Some("grok-4.5")), ["--model", "grok-4.5"]);
        assert!(super::super::acp::model_args("grok", Some("  ")).is_empty());
    }

    #[test]
    fn mode_flags_follow_agent_and_never_approve_everything() {
        let defaults = BackendOptions::default();
        assert_eq!(Grok.acp_mode_options(&defaults, Access::Question), ["--no-leader"]);
        assert!(Grok.acp_options(&defaults, Access::Build).is_empty());
        assert!(Grok.acp_session_config(&defaults).is_empty());
        let options = BackendOptions {
            cli_model: Some("  grok-4.6  ".into()),
            cli_thinking: Some(crate::Effort::Low),
            cli_reasoning_effort: Some(crate::config::CliReasoningEffort::High),
            effort: Some(crate::Effort::Medium),
            args: vec!["--always-approve".into()],
            ..Default::default()
        };
        // The saved extra arguments are refused and are not copied in front of `agent`.
        assert!(Grok.validate_options(&options).is_err());
        assert!(Grok.acp_options(&options, Access::Build).is_empty());
        assert_eq!(Grok.acp_mode_options(&options, Access::Build), ["--no-leader", "--reasoning-effort", "low"]);
        assert_eq!(Grok.acp_session_config(&options), [("model".into(), "grok-4.6".into()), ("reasoning_effort".into(), "low".into())]);
        assert!(!Grok.acp_mode_options(&options, Access::CleanRoomBuild).iter().any(|arg| arg.contains("always-approve")));
        let named =
            BackendOptions { cli_reasoning_effort: Some(crate::config::CliReasoningEffort::Xhigh), effort: Some(crate::Effort::Low), ..Default::default() };
        assert_eq!(Grok.acp_session_config(&named), [("reasoning_effort".into(), "xhigh".into())]);
        assert_eq!(Grok.model_in_use(None).as_deref(), Some("Grok Build default"));
        assert_eq!(Grok.model_in_use(Some(&options)).as_deref(), Some("grok-4.6"));
        let fix = Grok.sign_in_fix().unwrap();
        assert!(fix.contains("grok login") && fix.contains("never sees"));
    }

    #[test]
    fn catalogue_offers_only_the_selected_models_advertised_thinking() {
        let metadata = super::super::acp::ModelMetadata {
            models: vec!["grok-4.7".into(), "grok-4.5".into()],
            current_model: "grok-4.7".into(),
            config_options: vec![json!({"id": "reasoning_effort", "options": [
                {"value": "low", "name": "Low"},
                {"value": "medium", "name": "Medium"},
                {"value": "high", "name": "High"},
                {"value": "xhigh", "name": "High"},
                {"value": "unsupported", "name": "Unknown"}
            ]})],
        };
        let mut options = BackendOptions { cli_model: Some("grok-4.5".into()), ..Default::default() };
        let models = catalogue(&metadata, &options);
        assert!(models.iter().all(|model| model.id != "auto"));
        assert_eq!(models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(), ["grok-4.7", "grok-4.5"]);
        assert!(models.iter().find(|model| model.id == "grok-4.7").unwrap().thinking.is_empty());
        options.cli_model = Some("grok-4.7".into());
        let models = catalogue(&metadata, &options);
        assert_eq!(
            models.iter().find(|model| model.id == "grok-4.7").unwrap().thinking,
            [crate::Effort::Low, crate::Effort::Medium, crate::Effort::High, crate::Effort::Xhigh]
        );
        options.cli_thinking = Some(crate::Effort::Xhigh);
        assert!(models::validate(&options, &models).is_ok());
        options.cli_thinking = Some(crate::Effort::Max);
        assert!(models::validate(&options, &models).is_err());
    }

    #[test]
    fn auth_path_uses_grok_home_and_does_not_need_the_real_one() {
        let custom = PathBuf::from("/tmp/grok-home");
        assert_eq!(auth_path(Some(custom.as_os_str()), None), Some(custom.join("auth.json")));
        let user = PathBuf::from("/home/u");
        assert_eq!(auth_path(Some(OsStr::new("")), Some(&user)), Some(user.join(".grok").join("auth.json")));
        assert_eq!(auth_path(None, Some(&user)), Some(user.join(".grok").join("auth.json")));
        assert_eq!(auth_path(None, None), None);
    }
}
