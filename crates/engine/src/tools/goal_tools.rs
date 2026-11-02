//! Project-owned long-running goals, shared by all clients.
use crate::goal::{self, BuildLease, GoalSpec, GoalStatus};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::ToolError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: "project_goal_set",
            title: "Save a project goal",
            summary: "Save a paused, versioned goal: objective, constraints, ordered milestones with machine-check acceptance requirements, and cycle, active-time and reported-token limits. Read project_goal_show first and pass expected_revision (omit only for a new goal). Changing only limits retains progress and usage; changing scope starts fresh. Pause a running build before editing. No agent can mark a goal complete: fresh project checks decide.",
            beginner: "Sets a long-term goal and how to check it.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<SetParams>,
            run: set,
        },
        ToolSpec {
            id: "project_goal_show",
            title: "Show a project goal",
            summary: "Read the saved goal, milestone results, budgets and checkpoint history (newest 5 by default; offset counts back from newest, limit is at most 32). running reports the cross-process build lease; active with running=false may be starting, or interrupted and resumable after restart. Reading never starts a build.",
            beginner: "Shows the goal and its progress.",
            toolset: Toolset::Projects,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ShowParams>,
            run: show,
        },
        ToolSpec {
            id: "project_goal_pause",
            title: "Pause a project goal",
            summary: "Save a pause request. A running goal cancels its agent and checks at the next cancellation boundary, normally within one second; wait for running=false before editing or resuming. Files, conversation, checkpoints and budget usage are retained. This also pauses an interrupted goal.",
            beginner: "Pauses the goal and keeps its work.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<ProjectParams>,
            run: pause,
        },
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectParams {
    project: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShowParams {
    project: PathBuf,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    5
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParams {
    project: PathBuf,
    goal: GoalSpec,
    #[serde(default)]
    expected_revision: Option<u64>,
}

fn set(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SetParams = parse_params(params)?;
    let ws = goal::project(&p.project)?;
    let goal = goal::set(&ws.root, p.goal, p.expected_revision)?;
    Ok(ToolOutput::Value(json!({ "project":ws.root,"goal": goal, "running": false })))
}

fn show(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ShowParams = parse_params(params)?;
    let ws = goal::project(&p.project)?;
    let mut goal = goal::load(&ws.root)?;
    let total = goal.as_ref().map_or(0, |g| g.checkpoints.len());
    let offset = p.offset.min(total);
    let limit = p.limit.min(32);
    if let Some(g) = &mut goal {
        let mut page: Vec<_> = g.checkpoints.iter().rev().skip(offset).take(limit).cloned().collect();
        page.reverse();
        g.checkpoints = page;
    }
    Ok(ToolOutput::Value(json!({ "project":ws.root,"goal":goal,"checkpoint_total":total,"offset":offset,"limit":limit,"running":BuildLease::busy(&ws.root)? })))
}

fn pause(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ProjectParams = parse_params(params)?;
    let ws = goal::project(&p.project)?;
    let goal = goal::update(&ws.root, |g| {
        if g.status == GoalStatus::Active {
            g.status = GoalStatus::Paused;
            g.note = "Paused by the person. Resume to verify the current build and carry on.".into();
        }
        Ok(())
    })?;
    Ok(ToolOutput::Value(json!({ "project":ws.root,"goal": goal, "running": BuildLease::busy(&ws.root)? })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallOptions, Caller, Engine};
    #[test]
    fn lifecycle_tools_are_headless_and_revision_checked() {
        let dir = crate::testing::temp_dir();
        let ws = crate::workspace::Workspace::create(&dir, "Sample", "A game", crate::workspace::TargetRef::None).unwrap();
        let engine = Engine::new(crate::testing::session());
        let call = |id, params| engine.call(id, params, CallOptions::new(Caller::Cli));
        let params = json!({"project":ws.root,"goal":{"objective":"Rebuild sample"}});
        assert!(call("project_goal_set", params.clone()).is_ok());
        assert!(call("project_goal_set", params).is_err());
        assert!(call("project_goal_show", json!({"project":ws.root})).is_ok());
        assert!(call("project_goal_pause", json!({"project":ws.root})).is_ok());
        assert_eq!(goal::load(&ws.root).unwrap().unwrap().status, GoalStatus::Paused);
        let _writer = BuildLease::acquire(&ws.root).unwrap();
        assert!(call("project_delete", json!({"project":ws.root,"confirm":true})).is_err());
        assert!(call("project_build_finish", json!({"project":ws.root})).is_err());
        assert!(ws.duplicate("Copy").is_err());
        assert!(ws.root.join(crate::workspace::MANIFEST).is_file());
    }
}
