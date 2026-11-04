//! The person's installed Copilot CLI, driven over ACP with our MCP tools rather than its
//! native file and shell tools. Sign-in, credential storage and refresh stay in Copilot CLI.

use super::*;

/// Copilot's ACP-only adapter. Unsupported connection modes fail instead of falling back to
/// a CLI invocation that has no way to ask Craft Anything for approval.
pub struct Copilot;

impl AgentAdapter for Copilot {
    fn ids(&self) -> &'static [&'static str] {
        &["copilot"]
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
        // No override means the CLI's saved model, not an implicit request for `auto`.
        Some(options.and_then(|options| options.cli_model.clone()).unwrap_or_else(|| "Copilot CLI default".into()))
    }

    fn model_choices(&self, config: &BackendConfig, options: &BackendOptions) -> Result<Vec<String>, ToolError> {
        super::super::acp_model_choices(config, options)
    }

    fn supports_model_tuning(&self) -> bool {
        true
    }

    fn check_tool_server(&self, stop: &CancelToken) -> Result<(), ToolError> {
        cli_can_start(craft_anything_platform::self_cli_path().as_deref(), stop)
    }

    fn acp_options(&self, options: &BackendOptions, access: Access) -> Vec<String> {
        // Keep questions in our inbox, inherited instructions out of the run, and the installed
        // CLI version stable. Copilot already protects the other GitHub token variable names.
        let mut args: Vec<String> =
            ["--disable-builtin-mcps", "--no-ask-user", "--no-auto-update", "--no-custom-instructions", "--secret-env-vars=GH_TOKEN"].map(String::from).into();
        // An empty allowlist restores native tools in CLI 1.0.105. A nonempty list containing
        // only the disabled `ask_user` tool really leaves a Clean room question with no tools.
        let tools = match access {
            Access::Question => "view,glob,grep".to_string(),
            Access::CleanRoomQuestion => "ask_user".to_string(),
            _ => mcp_tools(),
        };
        args.push(format!("--available-tools={tools}"));
        if !matches!(access, Access::Question | Access::CleanRoomQuestion) {
            // Only our server is pre-allowed; its tools still enforce mode, effects and consent.
            args.push(format!("--allow-tool={MCP_NAME}"));
        }
        if let Some(model) = options.cli_model.as_deref().filter(|model| !model.trim().is_empty()) {
            args.extend(["--model".into(), model.into()]);
        }
        if let Some(context) = options.cli_context {
            args.extend(["--context".into(), context.as_str().into()]);
        }
        if let Some(effort) = options
            .cli_thinking
            .map(|effort| effort.as_str())
            .or_else(|| options.cli_reasoning_effort.map(|effort| effort.as_str()))
            .or_else(|| options.effort.map(|effort| effort.as_str()))
        {
            args.extend(["--reasoning-effort".into(), effort.into()]);
        }
        args
    }

    fn validate_options(&self, options: &BackendOptions) -> Result<(), ToolError> {
        // Extra arguments could replace the allowlist, attach another server or choose another
        // credential store. Expose supported settings explicitly instead of merging those flags.
        if options.args.is_empty() {
            Ok(())
        } else {
            Err(ToolError::invalid("Extra Copilot CLI arguments could bypass Craft Anything's tool restrictions.")
                .with_remediation("Remove this backend's extra arguments. Set its program, cli_model and effort through the backend options instead."))
        }
    }

    fn process(&self, program: &str) -> Process {
        // Inherited blanket approval must not bypass the app's policy. Authentication and the
        // person's CLI home stay untouched; the CLI itself resolves its credentials.
        Process::new(program).env("COPILOT_ALLOW_ALL", "false").env("COPILOT_ASSISTED_APPROVAL", "false")
    }

    fn acp_mcp_options(&self, mcp: &McpCommand) -> Vec<String> {
        // CLI 1.0.105 exposed no tools with ACP's server list alone. Its documented startup
        // config works without rewriting settings; the tool deadline matches a long build.
        let config = json!({"mcpServers": {MCP_NAME: {"type": "stdio", "command": mcp.command, "args": mcp.args, "tools": ["*"], "timeout": 86_400_000}}});
        vec![format!("--additional-mcp-config={config}")]
    }

    fn sign_in_fix(&self) -> Option<String> {
        Some("Sign in separately in a terminal with `copilot login`, then try again. If authentication still fails, restart Craft Anything. Copilot CLI manages your credentials.".into())
    }
}

fn catalogue(metadata: &super::super::acp::ModelMetadata, options: &BackendOptions) -> Vec<models::AgentModel> {
    let tuning = crate::model_options::ModelTuning::from_metadata(metadata.current_model.clone(), &metadata.config_options);
    let active = options.cli_model.as_deref().unwrap_or(&metadata.current_model);
    let thinking: Vec<_> = tuning.reasoning.iter().filter_map(|choice| crate::Effort::parse(choice.value.as_str())).collect();
    let mut ids = metadata.models.clone();
    ids.insert(0, "auto".into());
    if !metadata.current_model.is_empty() {
        ids.push(metadata.current_model.clone());
    }
    ids.sort();
    ids.dedup();
    ids.into_iter().map(|id| models::AgentModel { name: id.clone(), thinking: if id == active { thinking.clone() } else { Vec::new() }, id }).collect()
}

/// CLI-level names for our built-in tools and the progressive-toolset helpers. Read them from
/// the same catalogues as the MCP server so new built-ins aren't lost to a handwritten list.
/// Runtime plugin tools stay out until their exposure has its own restriction test.
fn mcp_tools() -> String {
    let specs = craft_anything_engine::tools::all().into_iter().chain(crate::tools());
    let mut names: Vec<String> = specs.map(|spec| format!("{MCP_NAME}-{}", spec.id)).collect();
    names.extend(["list_toolsets", "enable_toolset"].map(|name| format!("{MCP_NAME}-{name}")));
    names.sort();
    names.dedup();
    names.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_catalogue_exposes_only_the_selected_models_advertised_thinking() {
        let metadata = super::super::super::acp::ModelMetadata {
            models: vec!["first".into(), "second".into()],
            current_model: "first".into(),
            config_options: vec![json!({"id":"reasoning_effort", "options":[{"value":"none","name":"None"},{"value":"high","name":"High"}]})],
        };
        let mut options = BackendOptions { cli_model: Some("first".into()), cli_thinking: Some(crate::Effort::None), ..Default::default() };
        let models = catalogue(&metadata, &options);
        assert_eq!(models.iter().find(|model| model.id == "first").unwrap().thinking, [crate::Effort::None, crate::Effort::High]);
        assert!(models.iter().find(|model| model.id == "second").unwrap().thinking.is_empty());
        assert!(models::validate(&options, &models).is_ok());
        options.cli_thinking = Some(crate::Effort::Minimal);
        assert!(models::validate(&options, &models).is_err());
        for effort in crate::Effort::CLI_ALL {
            options.cli_thinking = Some(effort);
            options.cli_reasoning_effort = Some(crate::config::CliReasoningEffort::High);
            assert!(Copilot.acp_options(&options, Access::Build).windows(2).any(|pair| pair == ["--reasoning-effort", effort.as_str()]));
        }
    }

    #[test]
    fn copilot_context_and_full_reasoning_range_round_trip_and_reach_the_cli() {
        use crate::config::{CliContext, CliReasoningEffort};
        for context in [CliContext::Standard, CliContext::Long] {
            for effort in CliReasoningEffort::ALL {
                let options = BackendOptions {
                    cli_context: Some(context),
                    cli_reasoning_effort: Some(*effort),
                    effort: Some(crate::model::Effort::Low),
                    ..Default::default()
                };
                let saved = toml::to_string(&options).unwrap();
                assert_eq!(toml::from_str::<BackendOptions>(&saved).unwrap(), options);
                let args = Copilot.acp_options(&options, Access::Build);
                assert!(args.windows(2).any(|pair| pair == ["--context", context.as_str()]));
                assert!(args.windows(2).any(|pair| pair == ["--reasoning-effort", effort.as_str()]));
                assert_eq!(args.iter().filter(|argument| argument.as_str() == "--reasoning-effort").count(), 1);
            }
        }
        let defaults: BackendOptions = toml::from_str("").unwrap();
        assert_eq!(defaults.cli_context, None);
        assert_eq!(defaults.cli_reasoning_effort, None);
        assert!(!Copilot.acp_options(&defaults, Access::Build).iter().any(|argument| argument == "--context" || argument == "--reasoning-effort"));
    }

    #[test]
    fn copilot_uses_acp_and_the_persons_model_without_a_key() {
        let options = BackendOptions { cli_model: Some("auto".into()), effort: Some(crate::model::Effort::High), ..Default::default() };
        let args = Copilot.acp_options(&options, Access::Build);
        assert!(Copilot.requires_acp() && !Copilot.plain_fallback());
        assert!(args.windows(2).any(|pair| pair == ["--model", "auto"]));
        assert!(args.windows(2).any(|pair| pair == ["--reasoning-effort", "high"]));
        assert!(args.contains(&"--secret-env-vars=GH_TOKEN".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("--allow-all")));
        let fix = Copilot.sign_in_fix().unwrap();
        assert!(fix.contains("copilot login") && fix.contains("restart Craft Anything"));
    }

    #[test]
    fn copilot_exposes_only_our_mcp_tools_for_analysis_and_builds() {
        for access in [Access::Analyse, Access::Build, Access::CleanRoomBuild] {
            let args = Copilot.acp_options(&BackendOptions::default(), access);
            let tools = args.iter().find_map(|arg| arg.strip_prefix("--available-tools=")).unwrap();
            assert!(tools.split(',').all(|tool| tool.starts_with("craft-anything-")));
            assert!(tools.split(',').any(|tool| tool == "craft-anything-build_file_write"));
            assert!(tools.split(',').any(|tool| tool == "craft-anything-enable_toolset"));
            assert!(args.contains(&"--allow-tool=craft-anything".to_string()));
            assert!(args.contains(&"--no-custom-instructions".to_string()));
        }
    }

    #[test]
    fn copilot_questions_have_no_mcp_or_writing_tools() {
        for (access, expected) in [(Access::Question, "--available-tools=view,glob,grep"), (Access::CleanRoomQuestion, "--available-tools=ask_user")] {
            let args = Copilot.acp_options(&BackendOptions::default(), access);
            assert!(args.iter().any(|arg| arg == expected));
            assert!(args.contains(&"--no-ask-user".to_string()));
            assert!(!args.iter().any(|arg| arg.starts_with("--allow-tool")));
        }
    }

    #[test]
    fn copilot_rejects_arguments_that_can_override_safety_or_authentication() {
        assert!(Copilot.validate_options(&BackendOptions::default()).is_ok());
        for argument in ["--allow-all", "--allow-tool=shell", "--available-tools=bash", "--config-dir=other", "--additional-mcp-config={}"] {
            let options = BackendOptions { args: vec![argument.into()], ..Default::default() };
            assert!(Copilot.validate_options(&options).is_err());
        }
    }

    #[test]
    fn copilot_uses_its_own_model_default_not_an_anthropic_model() {
        assert_eq!(Copilot.model_in_use(None).as_deref(), Some("Copilot CLI default"));
        assert!(!Copilot.acp_options(&BackendOptions::default(), Access::Build).iter().any(|argument| argument == "--model"));
        let options = BackendOptions { cli_model: Some("chosen-copilot-model".into()), ..Default::default() };
        assert_eq!(Copilot.model_in_use(Some(&options)).as_deref(), Some("chosen-copilot-model"));
    }
}
