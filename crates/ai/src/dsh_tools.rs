//! The DeepSeek Harness's own account as tools: sign in and out of the DeepSeek account it uses,
//! so the app, the CLI and MCP all reach the same flow. The account service is reached and the
//! Harness's own credential file is written; the token is never read out.

use craft_anything_engine::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// How long a sign-in waits for the browser to come back.
const SIGN_IN_WAIT: Duration = Duration::from_secs(15 * 60);

pub fn specs() -> Vec<ToolSpec> {
    let spec = |id, title, summary, beginner, effects, run| ToolSpec {
        id,
        title,
        summary,
        beginner,
        toolset: Toolset::Setup,
        effects,
        requires: Requires::Nothing,
        input_schema: schema_of::<NoParams>,
        run,
    };
    vec![
        spec(
            "ai_dsh_sign_in",
            "Sign in to DeepSeek Harness",
            "Sign in to the DeepSeek account the Harness uses: opens the DeepSeek sign-in page in the browser and keeps the Harness's own grant in its credential file (DSH_HOME, else ~/.dsh). The token is never shown. A browser round trip takes up to 15 minutes.",
            "Signs the Harness in to your DeepSeek account.",
            Effects::ACCESSES_NETWORK.union(Effects::WRITES_FILESYSTEM).union(Effects::LAUNCHES_PROCESS),
            sign_in,
        ),
        spec(
            "ai_dsh_sign_out",
            "Sign out of DeepSeek Harness",
            "Sign out of the DeepSeek account: the Harness's grant record is removed from its credential file, and every other credential line is kept. The account service is told too, when it can be reached.",
            "Signs the Harness out of your DeepSeek account.",
            Effects::ACCESSES_NETWORK.union(Effects::WRITES_FILESYSTEM),
            sign_out,
        ),
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// The Harness's own folder on this computer.
fn home() -> Result<std::path::PathBuf, ToolError> {
    crate::dsh_auth::home()
        .ok_or_else(|| ToolError::new(ErrorCode::BadSettings, "Couldn't find the DeepSeek Harness folder on this computer (DSH_HOME or ~/.dsh)."))
}

fn sign_in(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let _: NoParams = parse_params(params)?;
    let home = home()?;
    if let crate::dsh_auth::SignIn::SignedIn { account } = crate::dsh_auth::status(&home) {
        return Ok(ToolOutput::Value(json!({"signed_in": true, "account": account, "harness": home.display().to_string()})));
    }
    let open = |url: &str| {
        let _ = craft_anything_platform::Launch::new(craft_anything_platform::process::system_opener()).arg(url).start();
    };
    crate::dsh_auth::sign_in(&home, crate::dsh_auth::ORIGIN, &open, SIGN_IN_WAIT)?;
    let account = match crate::dsh_auth::status(&home) {
        crate::dsh_auth::SignIn::SignedIn { account } => account,
        crate::dsh_auth::SignIn::SignedOut => None,
    };
    Ok(ToolOutput::Value(json!({"signed_in": true, "account": account, "harness": home.display().to_string()})))
}

fn sign_out(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let _: NoParams = parse_params(params)?;
    let home = home()?;
    crate::dsh_auth::sign_out(&home, crate::dsh_auth::ORIGIN)?;
    Ok(ToolOutput::Value(json!({"signed_in": false, "harness": home.display().to_string()})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engine::{CallOptions, Caller, Engine};

    /// Both tools are registered under the ids the app, the CLI and MCP call, and refuse anything
    /// they don't take before touching a file.
    #[test]
    fn the_account_tools_are_offered_and_validate_their_arguments() {
        let e = Engine::shared(craft_anything_engine::testing::session(), specs());
        let ids: Vec<&str> = specs().iter().map(|s| s.id).collect();
        assert_eq!(ids, ["ai_dsh_sign_in", "ai_dsh_sign_out"]);
        for id in ids {
            let err = e.call(id, json!({"nope": true}), CallOptions::new(Caller::Cli)).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadInput, "{id}");
        }
    }
}
