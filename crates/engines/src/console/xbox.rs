//! Original Xbox, through Xbox XBE Recomp: the title's x86 XBE is translated to C++ ahead of time
//! and linked with a runtime that implements the Xbox kernel and XDK libraries on Windows
//! (Direct3D 12, XAudio2, SDL3 for controllers).
//!
//! Its signature scanner (`xbe_symbols`) only works for XDK builds it has signatures for, so that
//! is the hard compatibility check. It picks the build of D3D8, D3D8LTCG or XAPILIB first, then any
//! library's; [`signature_build`] follows the same order.

use super::build::{self, Play, Project, Stage, StageCx};
use super::xbox_audio;
use super::{
    Backend, Check, Console, Executable, FileAt, InputKind, Inspection, Os, Status, Support, Target, Verdict, check, existing, host_check, read_head, walk,
};
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_formats::xdvdfs::ReadAt;
use craft_anything_formats::{xbe, xdvdfs};
use craft_anything_platform::cancel::CancelToken as Cancel;
use craft_anything_platform::process::Process;
use serde_json::json;
use std::path::Path;

pub struct Xbox;

static BACKEND: Backend = Backend {
    name: "Xbox XBE Recomp",
    repository: "https://github.com/TGP482/XboxXBERecomp",
    license: "MIT",
    version: "latest upstream commit, fetched for each new build",
    needs: &["Visual Studio 2022 with the C++ workload", "Clang (clang++, lld)", "CMake 3.25+", "Ninja", "git"],
    takes: "The game's default.xbe with its data folders beside it, or the disc image.",
};

static TARGETS: &[Target] = &[
    Target { os: Os::Windows, arch: "x86_64", support: Support::Runs, note: "Native 64-bit Windows with Direct3D 12 and XAudio2." },
    Target {
        os: Os::Linux,
        arch: "x86_64",
        support: Support::BuildsOnly,
        note: "Only the backend's headless test harness builds here: no window, GPU, audio or controller.",
    },
    Target {
        os: Os::Macos,
        arch: "aarch64",
        support: Support::Unavailable,
        note: "The runtime draws with Direct3D 12 and plays sound through XAudio2, which macOS doesn't have.",
    },
    Target { os: Os::Android, arch: "aarch64", support: Support::Unavailable, note: "The runtime is written for Windows on x86-64 only." },
    Target { os: Os::Ios, arch: "aarch64", support: Support::Unavailable, note: "The runtime is written for Windows on x86-64 only." },
];

/// XDK builds the backend has signatures for (`xdk/<build>/signatures.toml`).
// shortcut: copied from upstream on 2026-11-03; a build newer upstream supports is blocked at validation until this list grows (or reads the cloned xdk/ folder).
pub const SIGNED_BUILDS: &[u16] = &[4831, 5849];

/// The XDK build whose signatures the backend will use, given the builds it has.
pub fn signature_build(x: &xbe::Xbe, signed: &[u16]) -> Option<u16> {
    let preferred = ["D3D8", "D3D8LTCG", "XAPILIB"].iter().flat_map(|n| x.libraries.iter().filter(move |l| l.name == *n));
    preferred.chain(x.libraries.iter()).map(|l| l.build).find(|b| signed.contains(b))
}

impl Console for Xbox {
    fn id(&self) -> &'static str {
        "xbox"
    }
    fn name(&self) -> &'static str {
        "Original Xbox"
    }
    fn backend(&self) -> &'static Backend {
        &BACKEND
    }
    fn targets(&self) -> &'static [Target] {
        TARGETS
    }

    fn run_stage(&self, stage: Stage, cx: &mut StageCx, project: &mut Project) -> Result<String, ToolError> {
        let game = cx.out.join("game");
        let sdk = game.join("sdk");
        let release = game.join("out").join("build").join("release");
        let vs = "In the Visual Studio Installer choose Modify and add \"C++ Clang tools for Windows\" and \"C++ CMake tools for Windows\".";
        match stage {
            Stage::Validate => {
                let ins = build::validate(self, cx, project)?;
                Ok(format!("{} ({}) can be recompiled.", ins.title.unwrap_or_else(|| "The game".into()), ins.title_id.unwrap_or_default()))
            }
            Stage::Dependencies => {
                if !cfg!(windows) {
                    return Err(ToolError::new(
                        ErrorCode::FeatureMissing,
                        "Original Xbox builds need Windows: the backend's runtime uses Direct3D 12 and XAudio2.",
                    ));
                }
                cx.toolchain_env()?;
                for names in [&["clang++"][..], &["cmake"], &["ninja"]] {
                    cx.need(names, vs)?;
                }
                let commit = cx.upstream(BACKEND.repository, &sdk)?;
                let short = commit.get(..12).unwrap_or(&commit).to_string();
                project.backend_commit = Some(commit);
                Ok(format!("Visual Studio's C++ tools are here; {} is at upstream commit {short}.", BACKEND.name))
            }
            Stage::Extract => extract(cx, &game.join("assets")),
            Stage::Generate => {
                let template = sdk.join("templates").join("game");
                // Never overwrite: config/game.toml is rewritten by the symbol scan later.
                let skip = |p: &Path| p.strip_prefix(&template).is_ok_and(|rel| game.join(rel).is_file());
                build::copy_tree(&template, &game, &cx.cancel, &skip)?;
                let info = game.join("src").join("common").join("game_info.cpp");
                let title: String =
                    project.title.clone().unwrap_or_default().chars().filter(|c| (c.is_ascii_graphic() && *c != '"' && *c != '\\') || *c == ' ').collect();
                if !title.trim().is_empty()
                    && let Ok(text) = std::fs::read_to_string(&info)
                {
                    std::fs::write(&info, text.replace("L\"Game Title\"", &format!("L\"{}\"", title.trim()))).map_err(|e| ToolError::io(info.display(), e))?;
                }
                Ok(format!("Game project made from the backend's template in {}.", game.display()))
            }
            Stage::Analyse => {
                cx.toolchain_env()?;
                let cmake = cx.need(&["cmake"], vs)?;
                cx.run(Process::new(&cmake).args(["--preset", "release"]).cwd(&game))?;
                let config = game.join("config").join("game.toml");
                let found = if std::fs::read_to_string(&config).is_ok_and(|t| t.lines().any(|l| l.trim() == "[symbols]")) {
                    "The XDK symbols were already found.".to_string()
                } else {
                    cx.run(Process::new(&cmake).arg("--build").arg(&release).args(["--target", "xbe_symbols"]).cwd(&game))?;
                    let text = cx.run(
                        Process::new(release.join("xbe_symbols.exe"))
                            .arg(game.join("assets").join("default.xbe"))
                            .arg(sdk.join("xdk"))
                            .args(["--library", "Game.dll", "--out"])
                            .arg(&config)
                            .cwd(&game),
                    )?;
                    symbols_summary(&text)
                };
                Ok(format!("{found} {}", sound(&cx.out, &game)?))
            }
            Stage::Recompile => {
                cx.toolchain_env()?;
                let cmake = cx.need(&["cmake"], vs)?;
                cx.run(Process::new(&cmake).arg("--build").arg(&release).args(["--target", "recompile_Game"]).cwd(&game))?;
                Ok("default.xbe was translated to C++ (game/generated).".into())
            }
            Stage::Compile => {
                cx.toolchain_env()?;
                let cmake = cx.need(&["cmake"], vs)?;
                cx.run(Process::new(&cmake).arg("--build").arg(&release).cwd(&game))?;
                Ok(format!("Built {}.", release.join("Game.exe").display()))
            }
            Stage::Package => {
                let package = cx.out.join("package");
                std::fs::create_dir_all(package.join("assets")).map_err(|e| ToolError::io(package.display(), e))?;
                for name in ["Game.exe", "Game.dll"] {
                    let from = release.join(name);
                    std::fs::copy(&from, package.join(name)).map_err(|e| ToolError::io(from.display(), e))?;
                }
                // shortcut: no Visual C++ runtime DLLs beside it (the backend's release.bat adds them), so the package
                // needs a PC with that runtime installed; add them when packages are shared.
                let readme = package.join("README.txt");
                std::fs::write(&readme, "Put the game's files (default.xbe with its folders, or its .iso) in the assets folder, then start Game.exe.\r\nNo game files are included.\r\n")
                    .map_err(|e| ToolError::io(readme.display(), e))?;
                // Played from the build folder: the runtime finds game/assets in a folder above it.
                project.play = Some(Play { program: release.join("Game.exe"), args: Vec::new(), cwd: release.clone(), env: Vec::new() });
                project.package = Some(package.clone());
                Ok(format!("Packaged in {} (without the game's files).", package.display()))
            }
        }
    }

    fn inspect(&self, input: &Path, cancel: &Cancel) -> Result<Inspection, ToolError> {
        let mut kind = existing(input)?;
        let mut ins = Inspection {
            console: "xbox",
            input: input.to_path_buf(),
            input_kind: kind,
            title: None,
            title_id: None,
            main_executable: None,
            executables: Vec::new(),
            checks: Vec::new(),
            verdict: Verdict::Blocked,
            details: json!({}),
            truncated: false,
        };
        // (path, header bytes) of each XBE found.
        let mut xbes: Vec<(String, u64, Vec<u8>)> = Vec::new();
        if kind == InputKind::Folder {
            let (files, truncated) = walk(input, cancel)?;
            ins.truncated = truncated;
            for (rel, path, size) in files.iter().filter(|f| f.0.to_ascii_lowercase().ends_with(".xbe")) {
                cancel.check()?;
                xbes.push((rel.clone(), *size, read_xbe_head(&mut FileAt(open(path)?), 0)?));
            }
            if xbes.is_empty() && files.iter().any(|f| is_image_name(&f.0)) {
                ins.checks.push(check(
                    "disc_image_in_folder",
                    Status::Note,
                    "This folder holds a disc image: pick the image itself, or the folder its files were extracted to.",
                ));
            }
        } else {
            let head = read_head(input, 4)?;
            if xbe::is_xbe(&head) {
                let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
                let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                xbes.push((name, size, read_xbe_head(&mut FileAt(open(input)?), 0)?));
            } else {
                let mut src = FileAt(open(input)?);
                let vol = xdvdfs::open(&mut src).map_err(|e| {
                    ToolError::unsupported(format!("This isn't an Xbox executable or disc image: {e}"))
                        .with_remediation("Pick the game's default.xbe, its folder, or an image of the disc.")
                })?;
                kind = InputKind::DiscImage;
                ins.input_kind = kind;
                let listing = xdvdfs::list(&mut src, &vol);
                ins.truncated = listing.truncated;
                for e in listing.entries.iter().filter(|e| !e.dir && e.path.to_ascii_lowercase().ends_with(".xbe")) {
                    cancel.check()?;
                    xbes.push((e.path.clone(), u64::from(e.size), read_xbe_head(&mut src, e.offset)?));
                }
                let files = listing.entries.iter().filter(|e| !e.dir).count();
                ins.details["disc"] = json!({"partition_offset": vol.partition, "files": files, "whole_disc": vol.partition != 0});
            }
        }

        // The title starts from default.xbe; any other XBE is a separate image to recompile.
        let main = xbes.iter().position(|x| x.0.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("default.xbe"))).or(if xbes.len() == 1 {
            Some(0)
        } else {
            None
        });
        let mut parsed = None;
        for (i, (path, size, head)) in xbes.iter().enumerate() {
            let p = xbe::parse(head);
            ins.executables.push(Executable { path: path.clone(), size: *size, kind: "xbe".into(), readable: p.is_ok() });
            if Some(i) == main {
                ins.main_executable = Some(path.clone());
                parsed = Some(p);
            }
        }
        match parsed {
            None if xbes.is_empty() => {
                ins.checks.push(check("xbe_found", Status::Fail, "No Xbox executable (.xbe) found. The game starts from default.xbe at the top of its disc."))
            }
            None => ins.checks.push(check("xbe_found", Status::Fail, "Several XBEs but no default.xbe: point at the game's top folder.")),
            Some(Err(e)) => ins.checks.push(check("xbe_readable", Status::Fail, format!("default.xbe couldn't be read: {e}"))),
            Some(Ok(x)) => {
                ins.checks.push(check("xbe_found", Status::Pass, "Found default.xbe."));
                xbe_checks(&x, &mut ins.checks);
                ins.title = Some(x.title_name.clone()).filter(|t| !t.is_empty());
                ins.title_id = Some(x.title_code.clone());
                ins.details["xbe"] = serde_json::to_value(&x).unwrap_or_default();
                ins.details["signature_build"] = json!(signature_build(&x, SIGNED_BUILDS));
            }
        }
        if xbes.len() > 1 {
            ins.checks.push(check("several_images", Status::Note, format!("{} XBEs: each one the game loads is recompiled as its own image.", xbes.len())));
        }
        if kind == InputKind::Executable {
            ins.checks.push(check("data_files", Status::Note, "Only the executable was given: the game's data folders must sit beside it to play."));
        }
        if ins.truncated {
            ins.checks.push(check("truncated", Status::Warn, "The game holds more files than were looked at; an XBE may have been missed."));
        }
        ins.checks.push(host_check(TARGETS));
        Ok(ins.finish())
    }
}

/// The checks that come from the main XBE's header.
fn xbe_checks(x: &xbe::Xbe, checks: &mut Vec<Check>) {
    let builds: Vec<String> = x.libraries.iter().map(|l| format!("{} {}", l.name, l.build)).collect();
    match signature_build(x, SIGNED_BUILDS) {
        Some(b) => checks.push(check("xdk_signatures", Status::Pass, format!("Built with XDK {b}, which the backend has signatures for."))),
        None => checks.push(check(
            "xdk_signatures",
            Status::Fail,
            format!(
                "Built with XDK libraries ({}) the backend has no signatures for (it has {}). Its symbol scanner stops here; signatures for a new build are made from titles of that build that already work.",
                if builds.is_empty() { "none listed".into() } else { builds.join(", ") },
                SIGNED_BUILDS.iter().map(u16::to_string).collect::<Vec<_>>().join(" and ")
            ),
        )),
    }
    if x.libraries.iter().any(|l| l.name == "D3D8LTCG" || l.name.ends_with("LTCG")) {
        checks.push(check(
            "ltcg",
            Status::Note,
            "Some libraries were built with link-time code generation; the backend supports this but may need [abi] entries in the game's config.",
        ));
    }
    if x.libraries.iter().any(|l| l.name == "XACTENG") {
        checks.push(check(
            "xact",
            Status::Warn,
            "The title uses the XACT audio engine, which the backend doesn't implement. The build adds an audio clock and places the DirectSound functions XACT calls (checked on one title so far), so it may still stop, or play without some sounds, where XACT needs more.",
        ));
    }
    if x.libraries.iter().any(|l| l.name.starts_with("XONLINE")) {
        checks.push(check(
            "online",
            Status::Warn,
            "The title uses Xbox Live, whose service for the original Xbox ended in 2010: online play won't work. System Link (LAN) is supported.",
        ));
    }
    if x.libraries.iter().any(|l| l.debug) {
        checks.push(check("debug_libraries", Status::Note, "Linked against debug XDK libraries (a development build); signatures may not match."));
    }
    if !x.sections.iter().any(|s| s.executable) {
        checks.push(check("code_sections", Status::Fail, "The XBE declares no code sections."));
    }
}

/// What a run of the game asked for in its log: functions the recompiler missed (it names their
/// addresses) and XDK data it couldn't place (by name).
#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct Asks {
    pub functions: Vec<String>,
    pub data: Vec<String>,
}

pub fn asks(log: &str) -> Asks {
    let mut a = Asks::default();
    for l in log.lines() {
        if let Some(rest) = l.split("add 0x").nth(1)
            && l.contains("to [functions]")
        {
            let hex: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
            let addr = format!("0x{}", hex.to_ascii_uppercase());
            if !hex.is_empty() && !a.functions.contains(&addr) {
                a.functions.push(addr);
            }
        } else if let Some(name) = l.split(": ").last().and_then(|t| t.strip_suffix(" is unknown for this image, add it to the config"))
            && !a.data.iter().any(|d| d == name)
        {
            a.data.push(name.to_string());
        }
    }
    a
}

fn hex_u32(s: &str) -> Result<u32, ToolError> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u32::from_str_radix(t, 16).map_err(|_| ToolError::invalid(format!("{s:?} isn't a hex address.")))
}

fn ident(s: &str) -> Result<&str, ToolError> {
    let ok =
        !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && !s.bytes().next().is_some_and(|b| b.is_ascii_digit());
    if ok { Ok(s) } else { Err(ToolError::invalid(format!("{s:?} isn't a name (letters, digits and _)."))) }
}

/// Set one entry of the build's `config/game.toml`: `functions` (key: an address the recompiler
/// missed, value: an optional name) or `symbols`/`data` (key: an XDK name, value: its address).
/// Replaces the entry if it's there, else adds it to its section. Returns the line written.
pub fn config_set(out: &Path, section: &str, key: &str, value: &str) -> Result<String, ToolError> {
    let path = out.join("game").join("config").join("game.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| ToolError::invalid(format!("No Xbox build config at {}: {e}", path.display())))?;
    // The line to write, and the key an existing line for the same entry has: an address in
    // [functions] (however it's spelled), a name elsewhere.
    let (line, addr, name) = match section {
        "functions" => {
            let at = hex_u32(key)?;
            let name = if value.trim().is_empty() { format!("sub_{at:X}") } else { ident(value.trim())?.to_string() };
            (format!("0x{at:X} = \"{name}\""), Some(at), String::new())
        }
        "symbols" | "data" => {
            let name = ident(key.trim())?.to_string();
            let at = hex_u32(value)?;
            (format!("{name} = 0x{at:X}"), None, name)
        }
        _ => return Err(ToolError::invalid(format!("No section {section:?}.")).with_remediation("Use functions, symbols or data.")),
    };
    let same = |k: &str| match addr {
        Some(at) => hex_u32(k).is_ok_and(|x| x == at),
        None => k == name,
    };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let header = format!("[{section}]");
    match lines.iter().position(|l| l.trim() == header) {
        None => {
            lines.push(String::new());
            lines.push(header);
            lines.push(line.clone());
        }
        Some(h) => {
            let end = lines.iter().skip(h + 1).position(|l| l.trim_start().starts_with('[')).map_or(lines.len(), |i| h + 1 + i);
            let found = (h + 1..end).find(|&i| lines.get(i).and_then(|l| l.split_once('=')).is_some_and(|(k, _)| same(k.trim())));
            match found {
                Some(i) => {
                    if let Some(l) = lines.get_mut(i) {
                        *l = line.clone();
                    }
                }
                None => {
                    // After the section's last entry, before the blank lines and the next section's comments.
                    let mut at = end;
                    while at > h + 1 && lines.get(at - 1).is_some_and(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) {
                        at -= 1;
                    }
                    lines.insert(at, line.clone());
                }
            }
        }
    }
    let mut out_text = lines.join("\n");
    out_text.push('\n');
    craft_anything_platform::fs::write_atomic(&path, out_text.as_bytes())?;
    Ok(line)
}

/// Writes the sound fixes ([`xbox_audio`]) into the build: `[symbols]` entries, the host
/// stand-ins (`src/game/craft_dsound.cpp`), and the audio clock started from the `loaded` hook
/// (`src/common/craft_audio.cpp`). Rewritten on each analysis; says what it did in a sentence.
fn sound(out: &Path, game: &Path) -> Result<String, ToolError> {
    let xbe_path = game.join("assets").join("default.xbe");
    let file = std::fs::read(&xbe_path).map_err(|e| ToolError::io(xbe_path.display(), e))?;
    let x = xbe::parse(&file).map_err(|e| ToolError::invalid(format!("{} can't be read: {e}", xbe_path.display())))?;
    let config = game.join("config").join("game.toml");
    let text = std::fs::read_to_string(&config).map_err(|e| ToolError::io(config.display(), e))?;
    let plan = xbox_audio::plan(&file, &x, &xbox_audio::symbols(&text));
    for (name, at) in &plan.symbols {
        config_set(out, "symbols", name, &format!("0x{at:X}"))?;
    }
    let write = |p: std::path::PathBuf, s: String| craft_anything_platform::fs::write_atomic(&p, s.as_bytes());
    let src = game.join("src");
    write(src.join("game").join("craft_dsound.cpp"), xbox_audio::hooks_source(&plan.hooks))?;
    let info = src.join("common").join("game_info.cpp");
    let clock = match std::fs::read_to_string(&info).ok().and_then(|t| xbox_audio::wire_loaded(&t)) {
        Some(wired) => {
            write(src.join("common").join("craft_audio.cpp"), xbox_audio::audio_source(&plan.pokes))?;
            write(info, wired)?;
            "the audio clock runs"
        }
        None => "the audio clock isn't wired (game_info.cpp's loaded hook is taken)",
    };
    let hrtf = if plan.pokes.is_empty() { "" } else { ", the HRTF table is filled" };
    Ok(format!("Sound: {clock}{hrtf}; {} more DirectSound function(s) named and {} given host stand-ins.", plan.symbols.len(), plan.hooks.len()))
}

/// The symbol scan's result in a sentence: how many XDK functions it found, and how many it
/// couldn't, which trap if the game calls them.
fn symbols_summary(text: &str) -> String {
    let found = text.lines().find_map(|l| l.trim().strip_prefix("found ")).unwrap_or("the XDK functions");
    let missing = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("missing").and_then(|r| r.split_once(':')).map(|(_, names)| names.split_whitespace().count()))
        .unwrap_or(0);
    let mut s = format!("Found {found}; wrote config/game.toml.");
    if missing > 0 {
        s.push_str(&format!(" {missing} weren't found: the game stops if it calls one (they're listed in the log; config/game.toml can name them by hand)."));
    }
    s
}

/// Put the game's files in `assets`: copied from a folder, or streamed out of a disc image.
fn extract(cx: &StageCx, assets: &Path) -> Result<String, ToolError> {
    std::fs::create_dir_all(assets).map_err(|e| ToolError::io(assets.display(), e))?;
    let input = &cx.input;
    let folder = if input.is_dir() {
        Some(input.as_path())
    } else if xbe::is_xbe(&read_head(input, 4)?) {
        input.parent()
    } else {
        None
    };
    if let Some(folder) = folder {
        let n = build::copy_tree(folder, assets, &cx.cancel, &|_| false)?;
        return Ok(format!("Copied {n} file(s) from {}.", folder.display()));
    }
    let mut src = FileAt(open(input)?);
    let vol = xdvdfs::open(&mut src).map_err(|e| ToolError::unsupported(e.to_string()))?;
    let listing = xdvdfs::list(&mut src, &vol);
    if listing.truncated {
        cx.log.line("Warning: the disc's file table was only partly read; some files may be missing.");
    }
    let (mut files, mut bytes) = (0usize, 0u64);
    for e in &listing.entries {
        cx.cancel.check()?;
        let dest = assets.join(e.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if e.dir {
            std::fs::create_dir_all(&dest).map_err(|err| ToolError::io(dest.display(), err))?;
            continue;
        }
        if dest.metadata().is_ok_and(|m| m.len() == u64::from(e.size)) {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| ToolError::io(parent.display(), err))?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|err| ToolError::io(dest.display(), err))?;
        let (mut at, end) = (e.offset, e.offset.saturating_add(u64::from(e.size)));
        while at < end {
            cx.cancel.check()?;
            let len = (end - at).min(8 << 20) as usize;
            let chunk = src.read_at(at, len).ok_or_else(|| ToolError::new(ErrorCode::FileError, format!("The disc image ends inside {}.", e.path)))?;
            std::io::Write::write_all(&mut out, &chunk).map_err(|err| ToolError::io(dest.display(), err))?;
            at += len as u64;
        }
        files += 1;
        bytes += u64::from(e.size);
    }
    Ok(format!("Took {files} file(s) ({} MB) out of the disc image.", bytes >> 20))
}

fn open(path: &Path) -> Result<std::fs::File, ToolError> {
    std::fs::File::open(path).map_err(|e| ToolError::new(ErrorCode::FileError, format!("Couldn't open {}: {e}", path.display())))
}

fn is_image_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".iso") || n.ends_with(".xiso")
}

/// An XBE's header block, read from `offset` in `src` (0 for a plain file).
fn read_xbe_head(src: &mut dyn xdvdfs::ReadAt, offset: u64) -> Result<Vec<u8>, ToolError> {
    let first = src.read_at(offset, 0x170).unwrap_or_default();
    let len = xbe::header_len(&first).unwrap_or(0x170) as usize;
    Ok(src.read_at(offset, len).unwrap_or(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::Verdict;

    fn xbe_bytes(builds: &[(&str, u16)]) -> Vec<u8> {
        // The formats crate's test helper isn't visible here; build the same minimal header.
        let base = 0x10000u32;
        let mut d = vec![0u8; 0x1000];
        d[..4].copy_from_slice(b"XBEH");
        let put = |d: &mut Vec<u8>, off: usize, v: u32| d[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0x104, base);
        put(&mut d, 0x108, 0x1000);
        put(&mut d, 0x118, base + 0x200);
        put(&mut d, 0x11C, 1);
        put(&mut d, 0x120, base + 0x400);
        put(&mut d, 0x160, builds.len() as u32);
        put(&mut d, 0x164, base + 0x600);
        put(&mut d, 0x208, 0x4D53_0004);
        for (i, c) in "Test Title".encode_utf16().enumerate() {
            d[0x20C + i * 2..0x20E + i * 2].copy_from_slice(&c.to_le_bytes());
        }
        put(&mut d, 0x400, 4);
        put(&mut d, 0x414, base + 0x800);
        d[0x800..0x805].copy_from_slice(b".text");
        for (i, (name, build)) in builds.iter().enumerate() {
            let l = 0x600 + i * 16;
            d[l..l + name.len()].copy_from_slice(name.as_bytes());
            d[l + 12..l + 14].copy_from_slice(&build.to_le_bytes());
        }
        d
    }

    #[test]
    fn a_folder_with_a_signed_build_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("default.xbe"), xbe_bytes(&[("XAPILIB", 5849), ("D3D8", 5849), ("XONLINES", 5849), ("XACTENG", 5849)])).unwrap();
        std::fs::create_dir(dir.path().join("media")).unwrap();
        let ins = Xbox.inspect(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(ins.verdict, Verdict::Ready, "{:?}", ins.checks);
        assert_eq!(ins.title.as_deref(), Some("Test Title"));
        assert_eq!(ins.title_id.as_deref(), Some("MS-004"));
        assert_eq!(ins.main_executable.as_deref(), Some("default.xbe"));
        assert!(ins.checks.iter().any(|c| c.id == "online" && c.status == Status::Warn));
        assert!(ins.checks.iter().any(|c| c.id == "xact" && c.status == Status::Warn));
    }

    #[test]
    fn an_unsigned_build_is_blocked_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let xbe = dir.path().join("default.xbe");
        std::fs::write(&xbe, xbe_bytes(&[("XAPILIB", 4627), ("D3D8", 4627)])).unwrap();
        let ins = Xbox.inspect(&xbe, &Cancel::default()).unwrap();
        assert_eq!(ins.input_kind, InputKind::Executable);
        assert_eq!(ins.verdict, Verdict::Blocked);
        let c = ins.checks.iter().find(|c| c.id == "xdk_signatures").unwrap();
        assert_eq!(c.status, Status::Fail);
        assert!(c.message.contains("4627"));
    }

    #[test]
    fn a_disc_image_is_read_in_place() {
        // A partition-only image: volume descriptor at sector 32, root table at 40, the XBE at 44.
        let xbe = xbe_bytes(&[("D3D8", 4831)]);
        let mut d = vec![0u8; 44 * 2048 + xbe.len()];
        let vd = 32 * 2048;
        d[vd..vd + 20].copy_from_slice(xdvdfs::MAGIC);
        d[vd + 20..vd + 24].copy_from_slice(&40u32.to_le_bytes());
        d[vd + 24..vd + 28].copy_from_slice(&2048u32.to_le_bytes());
        d[vd + 0x7EC..vd + 0x800].copy_from_slice(xdvdfs::MAGIC);
        let root = 40 * 2048;
        d[root + 4..root + 8].copy_from_slice(&44u32.to_le_bytes());
        d[root + 8..root + 12].copy_from_slice(&(xbe.len() as u32).to_le_bytes());
        d[root + 12] = 0x20;
        d[root + 13] = 11;
        d[root + 14..root + 25].copy_from_slice(b"default.xbe");
        d[44 * 2048..].copy_from_slice(&xbe);
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("game.iso");
        std::fs::write(&iso, d).unwrap();
        let ins = Xbox.inspect(&iso, &Cancel::default()).unwrap();
        assert_eq!(ins.input_kind, InputKind::DiscImage);
        assert_eq!(ins.verdict, Verdict::Ready, "{:?}", ins.checks);
        assert_eq!(ins.details["signature_build"], 4831);
    }

    #[test]
    fn a_run_log_says_what_to_add() {
        let log = "[1.391] error: indirect call to 0035637D has no recompiled function\n[1.391] error: add 0x35637D to [functions] in the image's config/*.toml and rebuild\n[1.392] error: add 0x35637d to [functions] in the image's config/*.toml and rebuild\n[1.414] error: xapi: XPP_GamepadType is unknown for this image, add it to the config\n";
        assert_eq!(asks(log), Asks { functions: vec!["0x35637D".into()], data: vec!["XPP_GamepadType".into()] });
    }

    #[test]
    fn config_entries_are_added_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("game").join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        let file = cfg.join("game.toml");
        std::fs::write(
            &file,
            "[xbe]\npath = \"x\"\n\n# extra entry points\n[functions]\n\n# names\n[symbols]\nA = 0x1\n\n[data]\nXPP_GamepadType             = 0\n",
        )
        .unwrap();
        assert_eq!(config_set(dir.path(), "functions", "0x35637d", "").unwrap(), "0x35637D = \"sub_35637D\"");
        config_set(dir.path(), "functions", "35637D", "Named").unwrap();
        config_set(dir.path(), "data", "XPP_GamepadType", "0x4B04E0").unwrap();
        config_set(dir.path(), "symbols", "B", "2").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.matches("0x35637D").count(), 1, "{text}");
        assert!(text.contains("[functions]\n0x35637D = \"Named\"\n"), "{text}");
        assert!(text.contains("XPP_GamepadType = 0x4B04E0") && !text.contains("= 0\n"), "{text}");
        assert!(text.contains("A = 0x1\nB = 0x2\n"), "{text}");
        assert!(config_set(dir.path(), "data", "bad name", "1").is_err());
        assert!(config_set(dir.path(), "functions", "zz", "").is_err());
        assert!(config_set(dir.path(), "abi", "x", "1").is_err());
    }

    #[test]
    fn the_symbol_scan_is_summed_up() {
        let text = "default.xbe: title 5454009B\n  found 179 of 243 functions, 10 of 12 data\n  ambiguous X: 0x1 0x2\n  missing (0, the host traps if the game calls one): BinkWait WSARecv socket\n";
        let s = symbols_summary(text);
        assert!(s.starts_with("Found 179 of 243 functions, 10 of 12 data;"), "{s}");
        assert!(s.contains("3 weren't found"), "{s}");
        assert_eq!(symbols_summary(""), "Found the XDK functions; wrote config/game.toml.");
    }

    #[test]
    fn signature_build_prefers_d3d8_then_any() {
        let x = xbe::parse(&xbe_bytes(&[("XAPILIB", 4831), ("D3D8", 5849)])).unwrap();
        assert_eq!(signature_build(&x, &[4831, 5849]), Some(5849));
        assert_eq!(signature_build(&x, &[4831]), Some(4831));
        assert_eq!(signature_build(&x, &[1]), None);
    }

    #[test]
    fn an_empty_folder_and_a_stray_file_are_handled() {
        let dir = tempfile::tempdir().unwrap();
        let ins = Xbox.inspect(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(ins.verdict, Verdict::Blocked);
        let junk = dir.path().join("x.bin");
        std::fs::write(&junk, b"not a game").unwrap();
        assert!(Xbox.inspect(&junk, &Cancel::default()).is_err());
        // A damaged XBE in a folder is reported, not a crash.
        std::fs::write(dir.path().join("default.xbe"), b"XBEH\0\0").unwrap();
        let ins = Xbox.inspect(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(ins.verdict, Verdict::Blocked);
    }
}
