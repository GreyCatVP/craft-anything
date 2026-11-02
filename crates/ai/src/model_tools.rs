//! The same agent model catalogue and validated selection for CLI/MCP clients.

use crate::{AiConfig, ModelContext, ThinkingDepth, cli_agents};
use craft_anything_engine::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::ToolError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn specs() -> Vec<ToolSpec> {
    let effects = Effects::LAUNCHES_PROCESS.union(Effects::ACCESSES_NETWORK).union(Effects::WRITES_FILESYSTEM);
    vec![
        ToolSpec {
            id: "ai_models_list",
            title: "List an agent's models",
            summary: "List the visible, enabled chat models and supported thinking depths of a configured OpenCode V2 backend, locally or on its SSH host. Uses that installation's configuration and credentials. Returns a total and a page (offset, limit: default 200, maximum 1000). OpenCode may start its background service and update its cache.",
            beginner: "Shows which models this OpenCode can use.",
            toolset: Toolset::Setup,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<ListParams>,
            run: ai_models_list,
        },
        ToolSpec {
            id: "ai_model_set",
            title: "Choose an agent's model and thinking",
            summary: "Save a configured OpenCode backend's model (provider/id) and optional thinking depth (low, medium, high, xhigh, max), validated against that instance's current model catalogue. Omit model and thinking to use OpenCode's default; omit thinking to use the selected model's default. Unsupported selections are refused.",
            beginner: "Chooses the model and how deeply it thinks.",
            toolset: Toolset::Setup,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<SetParams>,
            run: ai_model_set,
        },
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListParams {
    /// Configured backend name.
    name: String,
    #[serde(default)]
    offset: usize,
    /// Page size (default 200, at most 1000).
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParams {
    name: String,
    model: Option<String>,
    thinking: Option<ThinkingDepth>,
}

fn context(ctx: &Ctx<'_>, name: &str) -> Result<ModelContext, ToolError> {
    let config = AiConfig::from_table(&ctx.session.settings.ai)?;
    let backend = config.get(name).cloned().ok_or_else(|| ToolError::invalid(format!("No AI backend named {name:?}.")))?;
    if backend.config.kind != crate::BackendKind::AgentCli || !cli_agents::adapter_for(&backend.config.model).has_model_catalog() {
        return Err(ToolError::invalid("Choose an OpenCode backend with a live model catalogue."));
    }
    Ok(ModelContext { backend, directory: ctx.session.project.as_ref().map(|p| p.root.clone()) })
}

fn ai_models_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ListParams = parse_params(params)?;
    let c = context(ctx, &p.name)?;
    let stop = crate::model::Stop::for_tool(ctx.cancel.clone());
    let models = cli_agents::adapter_for(&c.backend.config.model).model_catalog(&c.backend.config, &c.backend.options, c.directory.as_deref(), &stop)?;
    let total = models.len();
    let page: Vec<_> = models.into_iter().skip(p.offset).take(p.limit.unwrap_or(200).min(1000)).collect();
    Ok(ToolOutput::Value(json!({"name": p.name, "total": total, "offset": p.offset, "models": page})))
}

fn ai_model_set(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SetParams = parse_params(params)?;
    let c = context(ctx, &p.name)?;
    let options = cli_agents::models::select_with_stop(&c, p.model.as_deref(), p.thinking, &crate::model::Stop::for_tool(ctx.cancel.clone()))?;
    let result = json!({"name": p.name, "model": options.cli_model, "thinking": options.cli_thinking});
    let mut config = AiConfig::from_table(&ctx.session.settings.ai)?;
    config.upsert(crate::BackendEntry { config: c.backend.config, options });
    let mut next = ctx.session.settings.clone();
    next.ai = config.to_table()?;
    next.save_to(&ctx.session.config_dir().join("config.toml"))?;
    ctx.session.settings = next;
    Ok(ToolOutput::Value(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engine::{CallOptions, Caller, Engine};

    #[test]
    fn model_tools_are_headless_and_default_selection_persists_in_the_session_store() {
        let mut session = craft_anything_engine::testing::session();
        let path = session.config_dir().join("config.toml");
        let mut cfg = AiConfig::default();
        cfg.upsert(crate::BackendEntry {
            config: crate::BackendConfig {
                name: "OpenCode".into(),
                kind: crate::BackendKind::AgentCli,
                model: "opencode".into(),
                base_url: None,
                ssh_host: None,
                enabled: true,
            },
            options: crate::BackendOptions {
                command: Some("missing-test-agent".into()),
                cli_model: Some("test/old".into()),
                cli_thinking: Some(ThinkingDepth::High),
                ..Default::default()
            },
        });
        session.settings.ai = cfg.to_table().unwrap();
        let engine = Engine::shared(session, specs());
        let call = |id, params| engine.call(id, params, CallOptions::new(Caller::Cli));
        assert!(call("ai_models_list", json!({"name":"missing"})).is_err());
        assert!(call("ai_model_set", json!({"name":"OpenCode","thinking":"unsupported"})).is_err());
        call("ai_model_set", json!({"name":"OpenCode"})).unwrap();
        let saved = AiConfig::from_settings(&craft_anything_engine::Settings::load_from(&path));
        assert!(saved.get("OpenCode").unwrap().options.cli_model.is_none());
        assert!(saved.get("OpenCode").unwrap().options.cli_thinking.is_none());
    }
}
