//! Recompiling original console titles: which consoles have a backend and what its output runs on
//! (`game_consoles_list`), and whether a person's game image, folder or executable can go through
//! it (`game_original_inspect`). The backends live in `craft_anything_engines::console`.
//!
//! Nothing here decrypts or unlocks anything: an encrypted executable is reported as such.

use super::native_common::{params_json, spec};
use super::static_input::builder;
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_engines::console::{self, CONSOLES, Status, Verdict, build, xbox_map};
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
            Toolset::Recompile,
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
            Toolset::Recompile,
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
            Toolset::Recompile,
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
            Toolset::Recompile,
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
            Toolset::Recompile,
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
            Toolset::Recompile,
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
            Toolset::Recompile,
            Effects::WRITES_FILESYSTEM,
            Requires::Nothing,
            schema_of::<ConfigParams>,
            game_recomp_config_set,
        ),
        spec(
            "game_recomp_open",
            "Open a build's original executable",
            "Open an Original Xbox build's executable (output folder, absolute path: its game/assets/default.xbe) as the active target, so every native tool (functions, disassembly, callers, callees, references, pseudocode) reads the original game code. The names the build's config gives addresses (XDK functions and data the symbol scan placed, functions named by hand) become target names; names the person or AI already gave are kept.",
            "Look at the original game code behind a recompiled game.",
            Toolset::Recompile,
            Effects::MUTATES_SESSION,
            Requires::Nothing,
            schema_of::<OutParams>,
            game_recomp_open,
        ),
        spec(
            "game_recomp_function_source",
            "Original code to generated C++",
            "Map between an Original Xbox build's original executable and the C++ the recompiler generated (output folder, absolute path). Given an address: the function holding it (its generated name, file and lines), the generated line of that very instruction, its XDK or hand-given name, any host function in the build's src/ that replaces it, and the generated code (up to `lines` lines around the instruction). Given a generated file and line instead (a compiler error, a crash symbol): the original function and instruction there. The map is kept in <output>/code_map.tsv and rebuilt when the generated code changes.",
            "Find the C++ for a piece of the original game, or the other way round.",
            Toolset::Recompile,
            Effects::WRITES_FILESYSTEM,
            Requires::Nothing,
            schema_of::<SourceParams>,
            game_recomp_function_source,
        ),
    ]
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceParams {
    /// Absolute path of the build's folder.
    pub output: String,
    /// An original address (hex), e.g. `0x3AB12A`: a function's entry or any instruction in it.
    #[serde(default)]
    pub address: Option<String>,
    /// A generated file name instead (`x86_recomp.115.cpp`), with `line`.
    #[serde(default)]
    pub file: Option<String>,
    /// The 1-based line in `file`.
    #[serde(default)]
    pub line: Option<usize>,
    /// Generated lines to include (default 60, at most 400; 0 for none).
    #[serde(default)]
    pub lines: Option<usize>,
}

fn game_recomp_open(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: OutParams = parse_params(params)?;
    let out = PathBuf::from(&p.output);
    let xbe = out.join("game").join("assets").join("default.xbe");
    if !xbe.is_file() {
        return Err(ToolError::invalid(format!("{} has no game/assets/default.xbe.", out.display()))
            .with_remediation("Give an Original Xbox build's folder whose extract stage has run."));
    }
    let names = std::fs::read_to_string(out.join("game").join("config").join("game.toml")).map(|t| xbox_map::config_names(&t)).unwrap_or_default();
    let target = crate::target::Target::load(&xbe)?;
    let (id, name) = (target.id.clone(), target.name.clone());
    ctx.session.add_target(target);
    let t = ctx.session.active_mut()?;
    // Every function the recompiler generated is a known entry: the analysis also finds the ones
    // only reached through pointers (vtables, callbacks), as the generated code has them.
    let map = xbox_map::CodeMap::load(&out).ok();
    let starts = map.as_ref().map(|m| m.bodies.len());
    if let Some(map) = map {
        let mut img = super::native_common::image_for(&t.target)?;
        img.bin.function_starts.extend(map.bodies.iter().map(|b| u64::from(b.address)));
        img.bin.function_starts.sort_unstable();
        img.bin.function_starts.dedup();
        t.ext.insert(std::sync::Arc::new(super::native_common::NativeImage(std::sync::Arc::new(img))));
    }
    let (mut added, mut kept) = (0, 0);
    for (a, n) in names {
        match t.annotations.names.entry(u64::from(a)) {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(n);
                added += 1;
            }
            std::collections::btree_map::Entry::Occupied(_) => kept += 1,
        }
    }
    ctx.session.save_annotations()?;
    Ok(ToolOutput::Value(json!({
        "target": id,
        "name": name,
        "path": xbe.display().to_string(),
        "names_added": added,
        "names_kept": kept,
        "known_functions": starts,
        "next": "native_functions_list, native_function_disassemble, native_function_callers_list; game_recomp_function_source for the generated C++",
    })))
}

fn game_recomp_function_source(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SourceParams = parse_params(params)?;
    let out = PathBuf::from(&p.output);
    let map = xbox_map::CodeMap::load(&out)?;
    let want = p.lines.unwrap_or(60).min(400);
    let (body, instruction) = match (&p.address, &p.file, p.line) {
        (Some(a), None, None) => {
            let t = a.trim();
            let at = u32::from_str_radix(t.trim_start_matches("0x").trim_start_matches("0X"), 16)
                .map_err(|_| ToolError::invalid(format!("{a:?} isn't a hex address.")))?;
            let body = map.containing(at).ok_or_else(|| ToolError::invalid(format!("No generated function at or below {at:#X}.")))?;
            (body.clone(), Some(at))
        }
        (None, Some(f), Some(l)) => {
            let body = map.at_line(f, l).ok_or_else(|| {
                ToolError::invalid(format!("Line {l} of {f} isn't inside a generated function."))
                    .with_remediation("Give a line between a function's X86_FUNC and its closing brace.")
            })?;
            let before = xbox_map::lines(&out, &body.file, body.first, l)?;
            (body.clone(), before.iter().rev().find_map(|x| xbox_map::instruction_at(x)))
        }
        _ => return Err(ToolError::invalid("Give either address, or file and line.")),
    };
    let all = xbox_map::lines(&out, &body.file, body.first, body.last)?;
    // The generated line of the instruction itself (its `// ADDR …` comment).
    let ins_line = instruction.and_then(|a| all.iter().position(|l| xbox_map::instruction_at(l) == Some(a)).map(|i| body.first + i));
    let centre = ins_line.unwrap_or(body.first);
    let from = centre.saturating_sub(want / 3).max(body.first);
    let excerpt: Vec<&String> = all.iter().skip(from - body.first).take(want).collect();
    let names = std::fs::read_to_string(out.join("game").join("config").join("game.toml")).map(|t| xbox_map::config_names(&t)).unwrap_or_default();
    let name = names.iter().find(|(a, _)| *a == body.address).map(|(_, n)| n.clone());
    let overrides: Vec<Value> = xbox_map::overrides(&out, body.address).into_iter().map(|(f, l)| json!({"file": f, "line": l})).collect();
    let note = match (instruction, ins_line) {
        (Some(a), None) if a != body.address => {
            Some(format!("{a:#X} has no instruction comment in this body: it may be data, or code the recompiler reached from another entry."))
        }
        _ => None,
    };
    Ok(ToolOutput::Value(json!({
        "function": {"address": format!("{:#X}", body.address), "generated_name": xbox_map::function_name(body.address), "name": name},
        "generated": {"file": format!("game/generated/game/{}", body.file), "first_line": body.first, "last_line": body.last},
        "instruction": instruction.map(|a| json!({"address": format!("{a:#X}"), "line": ins_line})),
        "replaced_by": overrides,
        "excerpt_from_line": from,
        "excerpt": excerpt,
        "note": note,
    })))
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

    /// A build folder with a tiny XBE (one code section: `push esi; call +0; pop esi; ret` at
    /// 0x11000), its config and one generated file, as Xbox XBE Recomp lays them out.
    fn build() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        for d in ["assets", "config", "generated/game", "src/game"] {
            std::fs::create_dir_all(game.join(d)).unwrap();
        }
        let mut x = vec![0u8; 0x1010];
        let put = |x: &mut Vec<u8>, off: usize, v: u32| x[off..off + 4].copy_from_slice(&v.to_le_bytes());
        x[..4].copy_from_slice(b"XBEH");
        put(&mut x, 0x104, 0x10000);
        put(&mut x, 0x108, 0x1000);
        put(&mut x, 0x10C, 0x2000);
        put(&mut x, 0x118, 0x10200);
        put(&mut x, 0x11C, 1);
        put(&mut x, 0x120, 0x10400);
        put(&mut x, 0x128, 0x11000 ^ 0xA8FC_57AB);
        put(&mut x, 0x158, 0x10900 ^ 0x5B6D_40B6);
        put(&mut x, 0x900, 0x8000_0001);
        put(&mut x, 0x400, 4 | 2);
        put(&mut x, 0x404, 0x11000);
        put(&mut x, 0x408, 0x10);
        put(&mut x, 0x40C, 0x1000);
        put(&mut x, 0x410, 0x10);
        put(&mut x, 0x414, 0x10800);
        x[0x800..0x805].copy_from_slice(b".text");
        x[0x1000..0x1008].copy_from_slice(&[0x56, 0xE8, 0, 0, 0, 0, 0x5E, 0xC3]);
        std::fs::write(game.join("assets/default.xbe"), x).unwrap();
        std::fs::write(game.join("config/game.toml"), "[functions]\n0x11000 = \"Boot\"\n[symbols]\nDirectSoundCreate = 0\n").unwrap();
        let body = "X86_FUNC(__imp__sub_00011000) {\n\t// 00011000 push esi\n\tX86_PUSH32(ctx.esi);\n\t// 00011006 pop esi\n\tX86_POP32(ctx.esi);\n\t// 00011007 ret\n}\n";
        std::fs::write(game.join("generated/game/x86_recomp.0.cpp"), format!("#include \"x86_recomp_shared.h\"\n\n{body}")).unwrap();
        std::fs::write(game.join("generated/game/codegen.stamp"), "").unwrap();
        std::fs::write(game.join("src/game/hooks.cpp"), "X86_FUNC(sub_00011000) {\n}\n").unwrap();
        dir
    }

    #[test]
    fn a_builds_executable_opens_with_its_names_and_the_native_tools_read_it() {
        let dir = build();
        let out = dir.path().display().to_string();
        let e = engine(true);
        let o = |id: &str, p: serde_json::Value| serde_json::to_value(e.call(id, p, CallOptions::new(Caller::Cli)).unwrap()).unwrap().to_string();
        let opened = o("game_recomp_open", json!({"output": out}));
        assert!(opened.contains("\"names_added\":1"), "{opened}");
        let dis = o("native_function_disassemble", json!({"function": "Boot"}));
        assert!(dis.contains("Boot:") && dis.contains("push esi") && dis.contains("\"kind\":\"xbe\""), "{dis}");
        // Clean room never reads the original
        assert!(engine(false).call("game_recomp_open", json!({"output": out}), CallOptions::new(Caller::Cli)).is_err());
    }

    #[test]
    fn maps_original_addresses_to_generated_lines_and_back() {
        let dir = build();
        let out = dir.path().display().to_string();
        let e = engine(true);
        let o = |p: serde_json::Value| serde_json::to_value(e.call("game_recomp_function_source", p, CallOptions::new(Caller::Cli)).unwrap()).unwrap();
        let v = o(json!({"output": out, "address": "0x11006"}));
        let text = v.to_string();
        assert!(text.contains("sub_00011000") && text.contains("\"name\":\"Boot\"") && text.contains("x86_recomp.0.cpp"), "{text}");
        assert!(text.contains("\"line\":6") && text.contains("src/game/hooks.cpp"), "{text}");
        assert!(dir.path().join("code_map.tsv").is_file());
        let back = o(json!({"output": out, "file": "x86_recomp.0.cpp", "line": 7})).to_string();
        assert!(back.contains("\"address\":\"0x11006\""), "{back}");
        assert!(e.call("game_recomp_function_source", json!({"output": out, "address": "0x10000"}), CallOptions::new(Caller::Cli)).is_err());
        assert!(e.call("game_recomp_function_source", json!({"output": out}), CallOptions::new(Caller::Cli)).is_err());
    }
}
