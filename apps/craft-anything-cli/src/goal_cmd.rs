//! Keep the shared goal worker alive while a terminal is attached. No second runner.
use crate::{Cli, INTERRUPTED, make_engine};
use clap::Args;
use craft_anything_engine::goal::{Goal, GoalStatus};
use craft_anything_engine::inbox::Inbox;
use craft_anything_engine::workspace::Workspace;
use craft_anything_engine::{CallOptions, Caller, Consent, Engine};
use craft_anything_evidence::ToolError;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[derive(Debug, Args)]
pub struct GoalArgs {
    /// An existing project with a saved goal (project-goal-set).
    pub folder: PathBuf,
}

pub fn goal(cli: &Cli, args: &GoalArgs) -> u8 {
    match run(cli, args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("goal: {}", e.for_people());
            1
        }
    }
}

fn call(engine: &Engine, id: &str, project: &std::path::Path) -> Result<Value, ToolError> {
    let out = engine.call(id, json!({"project":project}), CallOptions::new(Caller::Cli).with_consent(Consent::Granted))?;
    Ok(out.to_json().get("result").cloned().unwrap_or(Value::Null))
}

fn run(cli: &Cli, args: &GoalArgs) -> Result<u8, ToolError> {
    let root = std::path::absolute(&args.folder).map_err(|e| ToolError::io("project folder", e))?;
    let ws = Workspace::open(&root)?;
    let engine = make_engine(None, cli.deep_for_run())?;
    call(&engine, "project_goal_resume", &ws.root)?;
    let mut last = None;
    let mut asked = HashSet::new();
    let mut stopping = None;
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) && stopping.is_none() {
            call(&engine, "project_goal_pause", &ws.root)?;
            stopping = Some(Instant::now());
        }
        let value = call(&engine, "project_goal_show", &ws.root)?;
        let g: Goal = serde_json::from_value(value.get("goal").cloned().unwrap_or(Value::Null))
            .map_err(|e| ToolError::internal(format!("Couldn't read goal status: {e}")))?;
        let running = value.get("running").and_then(Value::as_bool).unwrap_or(false);
        // Time ticks need not flood the terminal; print only a new verdict or completed cycle.
        let state = (g.status, g.current_milestone, g.cycles, g.note.clone());
        if last.as_ref() != Some(&state) {
            if matches!(cli.format(), crate::output::Format::Json | crate::output::Format::Jsonl) {
                println!("{value}");
            } else {
                println!(
                    "[{}] Milestones {}/{} · cycles {}/{} · {}",
                    g.status.label(),
                    g.current_milestone,
                    g.spec.milestones.len(),
                    g.cycles,
                    g.spec.limits.max_cycles,
                    g.note
                );
            }
            last = Some(state);
        }
        // Never block the controller reading stdin: another client can answer the same inbox.
        for request in Inbox::new(ws.inbox_dir()).pending() {
            if asked.insert(request.id.clone()) {
                eprintln!(
                    "Waiting for an answer: {} ({}) — use project-inbox-answer in another terminal, or open the project in the app.",
                    request.title, request.id
                );
            }
        }
        if !running && g.status != GoalStatus::Active {
            return Ok(if stopping.is_some() {
                130
            } else if g.status == GoalStatus::Complete {
                0
            } else {
                1
            });
        }
        if stopping.is_some_and(|at| at.elapsed() > Duration::from_secs(30)) {
            return Err(ToolError::invalid("The pause was saved, but the worker hasn't released the project yet. Wait before resuming."));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn goal_command_parses_a_project_and_its_mode() {
        let cli = Cli::try_parse_from(["craft-anything-cli", "--clean-room", "goal", "./sample"]).unwrap();
        assert_eq!(cli.deep_for_run(), Some(false));
        assert!(matches!(cli.command, crate::Command::Goal(GoalArgs { folder }) if folder == std::path::Path::new("./sample")));
    }
}
