//! Recompiling original console titles: which consoles have a backend and what its output runs on
//! (`game_consoles_list`), and whether a person's game image, folder or executable can go through
//! it (`game_original_inspect`). The backends live in `craft_anything_engines::console`.
//!
//! Nothing here decrypts or unlocks anything: an encrypted executable is reported as such.

use super::native_common::{params_json, spec};
use super::static_input::builder;
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_engines::console::{self, CONSOLES, Status, Verdict, build};
use craft_anything_evidence::{Confidence, Coverage, Limitation, Location, Subject, ToolError};
use craft_anything_platform::process::Launch;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

pub const CONSOLES_LIST_TOOL: &str = "game_consoles_list";
pub const INSPECT_TOOL: &str = "game_original_inspect";

pub fn specs() -> Vec<ToolSpec> {
    vec![
        spec(
            CONSOLES_LIST_TOOL,
            "List consoles that can be recompiled",
            "List the consoles whose original games can be statically recompiled into native programs: each one's backend (project, licence, which version a build uses (each new build fetches the latest upstream commit), the programs its build needs, what it takes), and for Windows, Linux, macOS, Android and iOS whether the backend's output runs there, only builds, or isn't available, with why. Reads nothing of any game.",
            "See which consoles' games can be turned into PC programs, and where they'd run.",
            Toolset::Formats,
            Effects::EVIDENCE,
            Requires::Nothing,
            schema_of::<ListParams>,
            game_consoles_list,
        ),
        spec(
            INSPECT_TOOL,
            "Check an original console game",
            "Look at a game the person owns for a console from game_consoles_list (console id, absolute path of a disc image, an extracted game folder or an executable): its title, id and executables, and the compatibility checks for that console's recompiler, each pass/note/warn/fail, with a verdict (ready or blocked). Original Xbox: reads disc images in place and default.xbe's header (XDK library builds against the backend's signatures). PlayStation 3: reads PARAM.SFO and tells plain ELFs from encrypted executables; nothing is ever decrypted, so an encrypted EBOOT.BIN blocks until the person supplies the decrypted ELF they made.",
            "Check whether a game you own can be recompiled, and what's missing if not.",
            Toolset::Formats,
            Effects::EVIDENCE.union(Effects::CARRIES_TARGET_CONTENT),
            Requires::Nothing,
            schema_of::<InspectParams>,
            game_original_inspect,
        ),
        spec(
            "game_recomp_start",
            "Recompile an original console game",
            "Start (or resume) recompiling a game the person owns into a native program, in the background, and return at once. Takes the console id, the game's absolute path (as for game_original_inspect) and an absolute output folder (new or empty, or one holding this game's earlier build, which resumes from its first unfinished stage). Stages: validation, dependency installation (the backend's latest upstream source is cloned into the folder, plus its toolchain checks), file extraction, native project generation, executable analysis, recompilation, compilation, packaging. Progress is in <output>/recomp.json and recomp.log: read them with game_recomp_status (or pass wait to answer only when it ends). After changing the build's config (game_recomp_config_set), pass from = \"recompile\" to run recompilation, compilation and packaging again. Runs the backend's own build programs and downloads its source and libraries. Nothing is decrypted.",
            "Turn a game you own into a program for this computer. It takes a while; you can watch each step.",
            Toolset::Formats,
            Effects::WRITES_FILESYSTEM.union(Effects::LAUNCHES_PROCESS).union(Effects::ACCESSES_NETWORK).union(Effects::EXECUTES_FOREIGN_CODE),
            Requires::Nothing,
            schema_of::<StartParams>,
            game_recomp_start,
        ),
        spec(
            "game_recomp_status",
            "Show a recompile build",
            "Show a recompile build's state from its output folder (absolute path): each stage's status (pending, running, done, failed, interrupted) and message, the backend commit used, whether it's running now, what Play starts, and the last lines of its log (lines, default 200).",
            "See how far a recompile has got.",
            Toolset::Formats,
            Effects::READ_ONLY,
            Requires::Nothing,
            schema_of::<StatusParams>,
            game_recomp_status,
        ),
        spec(
            "game_recomp_stop",
            "Stop a recompile build",
            "Stop a running recompile build (output folder, absolute path): the program it's running is ended and the stage marked interrupted; game_recomp_start resumes it.",
            "Pause a recompile; you can carry on later.",
            Toolset::Formats,
            Effects::LAUNCHES_PROCESS,
            Requires::Nothing,
            schema_of::<OutParams>,
            game_recomp_stop,
        ),
        spec(
            "game_recomp_play",
            "Play a recompiled game",
            "Start the program a finished recompile build made (output folder, absolute path), with the arguments and environment its backend needs; its output goes to <output>/play.log. Waits up to 8 seconds: if the game quits in that time, answers with its exit code, the error lines of its log, and what the log asks for (Original Xbox: functions the recompiler missed, by address, and XDK data it couldn't place, by name), so it can be fixed with game_recomp_config_set and rebuilt.",
            "Play the recompiled game.",
            Toolset::Formats,
            Effects::LAUNCHES_PROCESS.union(Effects::EXECUTES_FOREIGN_CODE),
            Requires::Nothing,
            schema_of::<OutParams>,
            game_recomp_play,
        ),
        spec(
            "game_recomp_config_set",
            "Fix a recompile build's config",
            "Set one entry of an Original Xbox build's config/game.toml (output folder, absolute path): section functions (key: an address the recompiler missed, as play's log names it; value: an optional name) or symbols/data (key: an XDK name; value: its address in the XBE, which the native tools on the build's game/assets/default.xbe can find). Replaces the entry or adds it. Then rebuild with game_recomp_start from = \"recompile\".",
            "Teach the recompiler something it missed about the game.",
            Toolset::Formats,
            Effects::WRITES_FILESYSTEM,
            Requires::Nothing,
            schema_of::<ConfigParams>,
            game_recomp_config_set,
        ),
    ]
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartParams {
    /// The console's id: `xbox` or `ps3`.
    pub console: String,
    /// Absolute path of the game's disc image, extracted folder, or executable.
    pub path: String,
    /// Absolute path of the build's folder.
    pub output: String,
    /// Wait for the build to finish (or fail) before answering, for callers that exit when the call returns (the CLI).
    #[serde(default)]
    pub wait: bool,
    /// Run this stage and every later one again, even if done: `recompile` after a config change.
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigParams {
    /// Absolute path of the build's folder.
    pub output: String,
    /// `functions`, `symbols` or `data`.
    pub section: String,
    /// functions: the address (hex). symbols/data: the XDK name.
    pub key: String,
    /// functions: an optional name. symbols/data: the address (hex).
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusParams {
    /// Absolute path of the build's folder.
    pub output: String,
    /// Log lines to include (default 200, at most 2000).
    #[serde(default)]
    pub lines: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutParams {
    /// Absolute path of the build's folder.
    pub output: String,
}

fn game_recomp_start(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: StartParams = parse_params(params)?;
    let out = PathBuf::from(&p.output);
    let from = p
        .from
        .as_deref()
        .map(|s| {
            serde_json::from_value::<build::Stage>(json!(s)).map_err(|_| {
                ToolError::invalid(format!("No stage {s:?}."))
                    .with_remediation("Use validate, dependencies, extract, generate, analyse, recompile, compile or package.")
            })
        })
        .transpose()?;
    let mut project = build::start(&p.console, &PathBuf::from(&p.path), &out, from)?;
    if p.wait {
        while build::is_running(&out) {
            if ctx.cancel.is_cancelled() {
                build::stop(&out);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        project = build::load(&out)?;
    }
    Ok(ToolOutput::Value(json!({"started": true, "project": project})))
}

fn game_recomp_status(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: StatusParams = parse_params(params)?;
    let out = PathBuf::from(&p.output);
    let project = build::load(&out)?;
    let log = build::log_tail(&out, p.lines.unwrap_or(200).min(2000));
    Ok(ToolOutput::Value(json!({"running": build::is_running(&out), "project": project, "log": log})))
}

fn game_recomp_stop(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: OutParams = parse_params(params)?;
    Ok(ToolOutput::Value(json!({"stopping": build::stop(&PathBuf::from(&p.output))})))
}

fn game_recomp_play(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: OutParams = parse_params(params)?;
    let out = PathBuf::from(&p.output);
    let play = build::load(&out)?
        .play
        .ok_or_else(|| ToolError::invalid("This build hasn't finished yet.").with_remediation("Start or resume it with game_recomp_start."))?;
    let mut launch = Launch::new(&play.program).args(&play.args).cwd(&play.cwd).log_to(out.join("play.log"));
    for (k, v) in &play.env {
        launch = launch.env(k, v);
    }
    let launched = launch.start_watched()?;
    let Some(ended) = launched.wait_timeout(std::time::Duration::from_secs(8)) else {
        return Ok(ToolOutput::Value(json!({"running": true, "pid": launched.pid(), "program": play.program})));
    };
    let log = std::fs::read(out.join("play.log")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    // The errors, without the floods of one repeated line (the first few of each kind).
    let mut errors: Vec<&str> = Vec::new();
    for l in log.lines().filter(|l| l.split("error:").nth(1).is_some_and(|r| !r.starts_with("  "))) {
        let kind = l.split("error:").nth(1).unwrap_or_default().split_whitespace().take(4).collect::<Vec<_>>().join(" ");
        if errors.iter().filter(|e| e.contains(&kind)).count() < 3 && errors.len() < 60 {
            errors.push(l);
        }
    }
    Ok(ToolOutput::Value(json!({
        "running": false,
        "exit_code": ended.code,
        "seconds": ended.after.as_secs_f32(),
        "errors": errors,
        "asks": craft_anything_engines::console::xbox::asks(&log),
        "log": out.join("play.log"),
    })))
}

fn game_recomp_config_set(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ConfigParams = parse_params(params)?;
    let out = PathBuf::from(&p.output);
    if build::is_running(&out) {
        return Err(ToolError::invalid("The build is running; change its config when it has stopped."));
    }
    let line = craft_anything_engines::console::xbox::config_set(&out, &p.section, &p.key, &p.value)?;
    Ok(ToolOutput::Value(json!({"written": line, "next": "game_recomp_start with from = \"recompile\""})))
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectParams {
    /// The console's id from game_consoles_list: `xbox` or `ps3`.
    pub console: String,
    /// Absolute path of the game's disc image, extracted folder, or executable.
    pub path: String,
}

fn game_consoles_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ListParams = parse_params(params)?;
    let consoles: Vec<Value> = CONSOLES.iter().map(|c| json!({"id": c.id(), "name": c.name(), "backend": c.backend(), "targets": c.targets()})).collect();
    let host = console::Os::host();
    let summary = CONSOLES
        .iter()
        .map(|c| {
            let runs: Vec<String> = c.targets().iter().filter(|t| t.support == console::Support::Runs).map(|t| format!("{} {}", t.os.name(), t.arch)).collect();
            format!("{} via {} (runs on {})", c.name(), c.backend().name, if runs.is_empty() { "nothing yet".into() } else { runs.join(", ") })
        })
        .collect::<Vec<_>>()
        .join("; ");
    let b = builder(ctx, CONSOLES_LIST_TOOL, Subject { kind: "catalogue".into(), sha256: None, name: Some("consoles".into()) }, params_json(&p))
        .result(json!({"consoles": consoles, "host": host, "host_arch": std::env::consts::ARCH}))
        .summary(format!("{} console(s): {summary}.", CONSOLES.len()))
        .confidence(Confidence::Observed)
        .limitations(vec![Limitation::new(
            "upstream_versions",
            "Platform support is what each backend documented when this table was written; its latest upstream may do more.",
        )])
        .coverage(Coverage::complete(CONSOLES.len() as u64));
    Ok(ToolOutput::evidence(b.build()?))
}

fn game_original_inspect(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: InspectParams = parse_params(params)?;
    let Some(c) = console::console(&p.console) else {
        let ids: Vec<&str> = CONSOLES.iter().map(|c| c.id()).collect();
        return Err(ToolError::invalid(format!("No console {:?}", p.console)).with_remediation(format!("Use one of: {}.", ids.join(", "))));
    };
    let path = PathBuf::from(&p.path);
    ctx.report(None, format!("Looking at {} for {}", path.display(), c.name()));
    let ins = c.inspect(&path, &ctx.cancel)?;
    let failed: Vec<&str> = ins.checks.iter().filter(|k| k.status == Status::Fail).map(|k| k.message.as_str()).collect();
    let warned = ins.checks.iter().filter(|k| k.status == Status::Warn).count();
    let name = ins.title.clone().or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let summary = match ins.verdict {
        Verdict::Ready => format!("{name} ({}): ready for {}, {warned} warning(s).", c.name(), c.backend().name),
        Verdict::Blocked => format!("{name} ({}): blocked. {}", c.name(), failed.join(" ")),
    };
    let mut lims = Vec::new();
    if ins.truncated {
        lims.push(Limitation::new("entries_capped", "Only part of the game's files were looked at."));
    }
    lims.push(Limitation::new(
        "static_only",
        "These checks read file headers only. Passing them doesn't mean the game will recompile or run; that's known only after the backend's own analysis and a build.",
    ));
    let examined = ins.executables.len() as u64;
    let b = builder(ctx, INSPECT_TOOL, Subject { kind: format!("{}_game", c.id()), sha256: None, name: Some(name) }, params_json(&p))
        .result(serde_json::to_value(&ins).unwrap_or(Value::Null))
        .summary(summary)
        .confidence(Confidence::Observed)
        .limitations(lims)
        .coverage(if ins.truncated { super::static_input::partial(examined) } else { Coverage::complete(examined) })
        .location(Location::new("path", p.path.clone()));
    Ok(ToolOutput::evidence(b.build()?))
}

#[cfg(test)]
mod tests {
    use crate::{CallOptions, Caller, Engine};
    use serde_json::json;

    fn engine(deep: bool) -> Engine {
        let mut s = crate::testing::session();
        s.settings.deep_mode = deep;
        Engine::new(s)
    }

    #[test]
    fn consoles_list_runs_in_clean_room() {
        let e = engine(false);
        let out = e.call("game_consoles_list", json!({}), CallOptions::new(Caller::Cli)).unwrap();
        let v = serde_json::to_value(&out).unwrap();
        let text = v.to_string();
        assert!(text.contains("\"xbox\"") && text.contains("\"ps3\""), "{text}");
    }

    #[test]
    fn inspecting_a_game_needs_deep_mode_and_a_known_console() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().display().to_string();
        assert!(engine(false).call("game_original_inspect", json!({"console": "xbox", "path": path}), CallOptions::new(Caller::Cli)).is_err());
        let e = engine(true);
        let err = e.call("game_original_inspect", json!({"console": "n64", "path": path}), CallOptions::new(Caller::Cli)).unwrap_err();
        assert!(err.remediation.unwrap_or_default().contains("xbox"));
        let out = e.call("game_original_inspect", json!({"console": "ps3", "path": path}), CallOptions::new(Caller::Cli)).unwrap();
        assert!(serde_json::to_value(&out).unwrap().to_string().contains("blocked"));
    }
}
