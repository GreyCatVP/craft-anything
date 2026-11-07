//! Kimi Code CLI's own account as tools: sign in and out by running the CLI's own kimi login
//! and kimi logout, so the app, the CLI and MCP share one path and the login stays Kimi's.

use craft_anything_engine::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::ToolError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn specs() -> Vec<ToolSpec> {
    let spec = |id, title, summary, beginner, effects, run| ToolSpec {
        id,
        title,
        summary,
        beginner,
        toolset: Toolset::Setup,
        effects,
        requires: Requires::Nothing,
        input_schema: schema_of::<ProgramParams>,
        run,
    };
    vec![
        spec(
            "ai_kimi_sign_in",
            "Sign in to Kimi Code",
            "Sign in by running Kimi Code CLI's own login command, which opens the browser; Craft Anything never keeps the login itself. A browser round trip waits up to 15 minutes. program names the CLI when it isn't the kimi on your PATH.",
            "Signs Kimi Code in to your Kimi account.",
            Effects::ACCESSES_NETWORK.union(Effects::LAUNCHES_PROCESS).union(Effects::WRITES_FILESYSTEM),
            sign_in,
        ),
        spec(
            "ai_kimi_sign_out",
            "Sign out of Kimi Code",
            "Sign out by removing the account Kimi Code stored in its credential file (this version has no logout command); the CLI's configuration stays, so it asks you to sign in again when it next needs the account.",
            "Signs Kimi Code out of your Kimi account.",
            Effects::ACCESSES_NETWORK.union(Effects::LAUNCHES_PROCESS).union(Effects::WRITES_FILESYSTEM),
            sign_out,
        ),
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProgramParams {
    /// The CLI's program, when it isn't the kimi on your PATH.
    #[serde(default)]
    pub program: Option<String>,
}

fn sign_in(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ProgramParams = parse_params(params)?;
    let program = crate::kimi_auth::find_program(p.program.as_deref())?;
    let home = crate::kimi_auth::home();
    if home.as_deref().is_some_and(crate::kimi_auth::signed_in) {
        return Ok(ToolOutput::Value(json!({"signed_in": true, "program": program, "already_signed_in": true})));
    }
    let said = crate::kimi_auth::sign_in(&program)?;
    let home = crate::kimi_auth::home();
    let signed_in = home.as_deref().is_some_and(crate::kimi_auth::signed_in);
    Ok(ToolOutput::Value(json!({"signed_in": signed_in, "program": program, "output": said})))
}

fn sign_out(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ProgramParams = parse_params(params)?;
    let program = crate::kimi_auth::find_program(p.program.as_deref())?;
    let said = crate::kimi_auth::sign_out(&program)?;
    Ok(ToolOutput::Value(json!({"signed_in": false, "program": program, "output": said})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engine::{CallOptions, Caller, Engine};

    #[test]
    fn the_kimi_account_tools_are_offered_and_validate_their_arguments() {
        let e = Engine::shared(craft_anything_engine::testing::session(), specs());
        let ids: Vec<&str> = specs().iter().map(|s| s.id).collect();
        assert_eq!(ids, ["ai_kimi_sign_in", "ai_kimi_sign_out"]);
        for id in ids {
            let err = e.call(id, json!({"nope": true}), CallOptions::new(Caller::Cli)).unwrap_err();
            assert_eq!(err.code, craft_anything_evidence::ErrorCode::BadInput, "{id}");
        }
    }
}
