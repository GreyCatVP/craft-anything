//! PlayStation 3, through ps3recomp: the title's PowerPC (PPU) code, and the SPU programs it
//! carries, are lifted to C ahead of time and linked with runtime libraries that implement the LV2
//! kernel and the system libraries.
//!
//! The backend reads a plain ELF. Retail and store executables (`EBOOT.BIN`, `.sprx`) are
//! encrypted SELF files; Craft Anything never decrypts them, so such a game is blocked until the
//! person supplies the decrypted ELF they made from their own console. A development SELF stores
//! its ELF unencrypted, and the backend's own `unfself.py` takes it out without any key.

use super::build::{self, Play, Project, Stage, StageCx};
use super::{Backend, Console, Executable, InputKind, Inspection, Os, Status, Support, Target, Verdict, check, existing, host_check, read_head, walk};
use craft_anything_evidence::ErrorCode;
use craft_anything_evidence::ToolError;
use craft_anything_formats::ps3::{self, ExecutableKind};
use craft_anything_platform::cancel::CancelToken as Cancel;
use craft_anything_platform::process::Process;
use serde_json::json;
use std::path::Path;

pub struct Ps3;

static BACKEND: Backend = Backend {
    name: "ps3recomp",
    repository: "https://github.com/sp00nznet/ps3recomp",
    license: "MIT",
    version: "latest upstream commit, fetched for each new build",
    needs: &["CMake 3.20+", "Ninja", "Python 3.10+", "clang-cl (Windows), GCC or Clang (Linux), or Xcode (macOS)"],
    takes: "A decrypted PPU ELF (EBOOT.ELF) and the game's files (PS3_GAME/USRDIR).",
};

static TARGETS: &[Target] = &[
    Target { os: Os::Windows, arch: "x86_64", support: Support::Runs, note: "Recompiled titles run here, drawn with Direct3D 12." },
    Target {
        os: Os::Linux,
        arch: "x86_64",
        support: Support::BuildsOnly,
        note: "The runtime builds and passes its tests (Vulkan or headless), but no title has booted to gameplay from the backend itself yet.",
    },
    Target {
        os: Os::Macos,
        arch: "aarch64",
        support: Support::BuildsOnly,
        note: "The runtime builds with a Metal renderer, but no title has booted to gameplay from the backend itself yet.",
    },
    Target { os: Os::Android, arch: "aarch64", support: Support::Unavailable, note: "The backend has no Android build or renderer." },
    Target { os: Os::Ios, arch: "aarch64", support: Support::Unavailable, note: "The backend has no iOS build." },
];

/// An executable's kind as the inspection names it.
fn kind_name(k: ExecutableKind) -> &'static str {
    match k {
        ExecutableKind::PpuElf => "ppu_elf",
        ExecutableKind::SpuElf => "spu_elf",
        ExecutableKind::DevelopmentSelf => "development_self",
        ExecutableKind::RetailSelf => "retail_self",
        ExecutableKind::StoreSelf => "store_self",
        ExecutableKind::Other => "other",
    }
}

/// Files that may hold executable code.
fn maybe_executable(rel: &str) -> bool {
    let n = rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase();
    n == "eboot.bin" || n.ends_with(".elf") || n.ends_with(".self") || n.ends_with(".sprx") || n.ends_with(".prx")
}

/// Whether a file is licence-protected data (`.edat`/`.sdat`), which nothing here unlocks.
fn licensed_data(rel: &str) -> bool {
    let n = rel.to_ascii_lowercase();
    n.ends_with(".edat") || n.ends_with(".sdat")
}

impl Console for Ps3 {
    fn id(&self) -> &'static str {
        "ps3"
    }
    fn name(&self) -> &'static str {
        "PlayStation 3"
    }
    fn backend(&self) -> &'static Backend {
        &BACKEND
    }
    fn targets(&self) -> &'static [Target] {
        TARGETS
    }

    fn run_stage(&self, stage: Stage, cx: &mut StageCx, project: &mut Project) -> Result<String, ToolError> {
        let backend = cx.out.join("backend");
        let work = cx.out.join("work");
        let elf = work.join("EBOOT.ELF");
        let game = cx.out.join("game");
        let recompiled = game.join("recompiled");
        let venv = cx.out.join("venv");
        let python = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
        let tool = |name: &str| backend.join("tools").join(name);
        let compilers = if cfg!(windows) {
            "In the Visual Studio Installer choose Modify and add \"C++ Clang tools for Windows\" and \"C++ CMake tools for Windows\"."
        } else if cfg!(target_os = "macos") {
            "Install Xcode's command line tools (xcode-select --install), then CMake and Ninja (brew install cmake ninja)."
        } else {
            "Install CMake, Ninja and GCC or Clang with your package manager."
        };
        match stage {
            Stage::Validate => {
                let ins = build::validate(self, cx, project)?;
                Ok(format!(
                    "{} ({}) can be recompiled from {}.",
                    ins.title.unwrap_or_else(|| "The game".into()),
                    ins.title_id.unwrap_or_default(),
                    ins.main_executable.unwrap_or_default()
                ))
            }
            Stage::Dependencies => {
                cx.toolchain_env()?;
                if cfg!(windows) {
                    cx.need(&["clang-cl"], compilers)?;
                } else {
                    cx.need(&["cc", "clang", "gcc"], compilers)?;
                }
                cx.need(&["cmake"], compilers)?;
                cx.need(&["ninja"], compilers)?;
                let system_python = cx.need(&["python", "python3"], "Install Python 3.10 or later (https://www.python.org) and try again.")?;
                let commit = cx.upstream(BACKEND.repository, &backend)?;
                let short = commit.get(..12).unwrap_or(&commit).to_string();
                project.backend_commit = Some(commit);
                if !python.is_file() {
                    cx.run(Process::new(&system_python).args(["-m", "venv"]).arg(&venv))?;
                }
                cx.run(Process::new(&python).args(["-m", "pip", "install", "--disable-pip-version-check", "-r"]).arg(backend.join("requirements.txt")))?;
                Ok(format!("The C/C++ tools are here; {} is at upstream commit {short}, with its Python tools installed.", BACKEND.name))
            }
            Stage::Extract => {
                let ins = self.inspect(&cx.input, &cx.cancel)?;
                let main = ins
                    .executables
                    .iter()
                    .find(|e| Some(&e.path) == ins.main_executable.as_ref())
                    .ok_or_else(|| ToolError::invalid("The game's executable is gone."))?;
                let source = if cx.input.is_dir() { cx.input.join(&main.path) } else { cx.input.clone() };
                std::fs::create_dir_all(&work).map_err(|e| ToolError::io(work.display(), e))?;
                match main.kind.as_str() {
                    "ppu_elf" => {
                        std::fs::copy(&source, &elf).map_err(|e| ToolError::io(source.display(), e))?;
                        Ok(format!("Copied {} to work/EBOOT.ELF.", main.path))
                    }
                    "development_self" => {
                        // The backend's own converter: a development SELF isn't encrypted, so no key is involved.
                        cx.run(Process::new(&python).arg(tool("unfself.py")).arg(&source).arg("--output").arg(&elf))?;
                        Ok(format!("Took the ELF out of the development SELF {}.", main.path))
                    }
                    other => Err(ToolError::invalid(format!("{} is a {other} executable, which Craft Anything doesn't decrypt.", main.path))),
                }
            }
            Stage::Generate => {
                let template = backend.join("templates").join("project");
                let skip = |p: &Path| p.strip_prefix(&template).is_ok_and(|rel| game.join(rel).is_file());
                build::copy_tree(&template, &game, &cx.cancel, &skip)?;
                Ok(format!("Game project made from the backend's template in {}.", game.display()))
            }
            Stage::Analyse => {
                cx.run(Process::new(&python).arg(tool("ppu_loader.py")).arg(&elf).arg("-o").arg(work.join("out")))?;
                Ok("Found the executable's functions, firmware imports and TOC (work/out).".into())
            }
            Stage::Recompile => {
                let out = work.join("out");
                cx.run(
                    Process::new(&python)
                        .arg(tool("ppu_lifter.py"))
                        .arg(&elf)
                        .arg("--functions")
                        .arg(out.join("EBOOT.functions.json"))
                        .arg("--hle-stubs")
                        .arg(out.join("EBOOT.imports.json"))
                        .arg("--output")
                        .arg(&recompiled),
                )?;
                cx.run(Process::new(&python).arg(tool("gen_hle_nids.py")).arg("--all").arg("--out").arg(recompiled.join("ppu_hle_nids.cpp")))?;
                Ok("The PPU code was lifted to C++ (game/recompiled), with the firmware call table.".into())
            }
            Stage::Compile => {
                cx.toolchain_env()?;
                let cmake = cx.need(&["cmake"], compilers)?;
                let define = |k: &str, v: &Path| format!("-D{k}={}", v.display());
                let mut configure = Process::new(&cmake)
                    .arg("-S")
                    .arg(&game)
                    .arg("-B")
                    .arg(game.join("build"))
                    .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"])
                    .arg(define("PS3RECOMP_DIR", &backend))
                    .arg(define("RECOMP_DIR", &recompiled))
                    .arg(define("PORT_SPU_INPUTS", &elf))
                    .arg(define("Python3_EXECUTABLE", &python));
                if cfg!(windows) {
                    configure = configure.args(["-DCMAKE_C_COMPILER=clang-cl", "-DCMAKE_CXX_COMPILER=clang-cl"]);
                }
                cx.run(configure)?;
                cx.run(Process::new(&cmake).arg("--build").arg(game.join("build")))?;
                Ok("Built the recompiled game (game/build).".into())
            }
            Stage::Package => {
                let built = game.join("build");
                let exe = built.join(if cfg!(windows) { "MyGameRecomp.exe" } else { "MyGameRecomp" });
                if !exe.is_file() {
                    return Err(ToolError::new(ErrorCode::FileError, format!("The build didn't make {}.", exe.display())));
                }
                let package = cx.out.join("package");
                std::fs::create_dir_all(&package).map_err(|e| ToolError::io(package.display(), e))?;
                for entry in std::fs::read_dir(&built).map_err(|e| ToolError::io(built.display(), e))?.flatten() {
                    let p = entry.path();
                    let keep = p == exe || p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll") || x == "dylib" || x == "so");
                    if keep && p.is_file() {
                        std::fs::copy(&p, package.join(entry.file_name())).map_err(|e| ToolError::io(p.display(), e))?;
                    }
                }
                let vfs = if cx.input.is_dir() { cx.input.clone() } else { cx.input.parent().map(Path::to_path_buf).unwrap_or_default() };
                project.play = Some(Play {
                    program: exe,
                    args: vec![elf.display().to_string()],
                    cwd: built,
                    env: vec![("PS3_VFS_ROOT".into(), vfs.display().to_string())],
                });
                project.package = Some(package.clone());
                Ok(format!("Packaged in {} (without the game's files; it starts with the decrypted ELF's path).", package.display()))
            }
        }
    }

    fn inspect(&self, input: &Path, cancel: &Cancel) -> Result<Inspection, ToolError> {
        let kind = existing(input)?;
        let mut ins = Inspection {
            console: "ps3",
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
        let mut files = Vec::new();
        if kind == InputKind::Folder {
            let (found, truncated) = walk(input, cancel)?;
            ins.truncated = truncated;
            files = found;
        } else {
            let head = read_head(input, 0x8006)?;
            if head.get(0x8001..0x8006) == Some(b"CD001".as_slice()) {
                ins.input_kind = InputKind::DiscImage;
                ins.checks.push(check(
                    "disc_image",
                    Status::Fail,
                    "Reading PS3 disc images isn't supported yet. Extract the disc's files (PS3_GAME) to a folder and pick that folder.",
                ));
                ins.checks.push(host_check(TARGETS));
                return Ok(ins.finish());
            }
            if ps3::classify(&head) == ExecutableKind::Other {
                return Err(ToolError::unsupported("This isn't a PS3 executable, game folder or disc image.")
                    .with_remediation("Pick the game's folder (with PS3_GAME), its EBOOT.BIN, or a decrypted EBOOT.ELF."));
            }
            let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            files.push((name, input.to_path_buf(), size));
        }

        // PARAM.SFO: the disc layout (PS3_GAME/PARAM.SFO) or a store title's (PARAM.SFO at the top).
        let sfo = files.iter().filter(|f| f.0.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("PARAM.SFO"))).min_by_key(|f| f.0.matches('/').count());
        if let Some((rel, path, _)) = sfo {
            match read_head(path, 1 << 20).and_then(|b| ps3::parse_sfo(&b).map_err(|e| ToolError::unsupported(e.to_string()))) {
                Ok(s) => {
                    ins.title = s.title().map(str::to_owned);
                    ins.title_id = s.title_id().map(str::to_owned);
                    if s.category() == Some("GD") {
                        ins.checks.push(check("category", Status::Warn, "This PARAM.SFO describes game data or an update, not a game."));
                    }
                    ins.details["param_sfo"] = json!({"path": rel, "values": s.values});
                }
                Err(e) => ins.checks.push(check("param_sfo", Status::Warn, format!("{rel} couldn't be read: {}", e.message))),
            }
        } else if kind == InputKind::Folder {
            ins.checks.push(check("param_sfo", Status::Note, "No PARAM.SFO found, so the title's name and id are unknown."));
        }

        for (rel, path, size) in files.iter().filter(|f| maybe_executable(&f.0)) {
            cancel.check()?;
            let k = ps3::classify(&read_head(path, ps3::CLASSIFY_LEN as u64)?);
            if k != ExecutableKind::Other {
                ins.executables.push(Executable { path: rel.clone(), size: *size, kind: kind_name(k).into(), readable: k.readable() });
            }
        }
        // The main program: a plain PPU ELF the person made beats EBOOT.BIN; then EBOOT.BIN.
        let is_eboot = |e: &&Executable| e.path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("EBOOT.BIN"));
        let main = ins
            .executables
            .iter()
            .filter(|e| e.kind == "ppu_elf")
            .min_by_key(|e| (!e.path.to_ascii_lowercase().contains("eboot"), e.path.matches('/').count()))
            .or_else(|| ins.executables.iter().filter(is_eboot).min_by_key(|e| e.path.matches('/').count()))
            .cloned();
        match &main {
            None => ins.checks.push(check("executable", Status::Fail, "No PS3 executable found (EBOOT.BIN in USRDIR, or a decrypted EBOOT.ELF).")),
            Some(e) if e.kind == "ppu_elf" => ins.checks.push(check("executable", Status::Pass, format!("{} is a plain PPU ELF the backend can read.", e.path))),
            Some(e) if e.kind == "development_self" => ins.checks.push(check(
                "executable",
                Status::Pass,
                format!("{} is a development SELF: its ELF isn't encrypted, and the backend's unfself.py takes it out without any key.", e.path),
            )),
            Some(e) => ins.checks.push(check(
                "executable",
                Status::Fail,
                format!(
                    "{} is an encrypted {} executable. Craft Anything doesn't decrypt games. Put the decrypted ELF you made from your own console beside it (EBOOT.ELF) and look again.",
                    e.path,
                    if e.kind == "store_self" { "store (NPDRM)" } else { "retail" }
                ),
            )),
        }
        ins.main_executable = main.map(|e| e.path);
        let locked_modules = ins.executables.iter().filter(|e| !e.readable && !e.path.to_ascii_lowercase().ends_with("eboot.bin")).count();
        if locked_modules > 0 {
            ins.checks.push(check(
                "modules",
                Status::Warn,
                format!("{locked_modules} other executable module(s) (.sprx/.self) are encrypted; if the game loads them, they need decrypted copies too."),
            ));
        }
        let licensed = files.iter().filter(|f| licensed_data(&f.0)).count();
        if licensed > 0 {
            ins.checks.push(check(
                "licensed_data",
                Status::Warn,
                format!("{licensed} licence-protected data file(s) (.edat/.sdat). Craft Anything won't unlock these; the game may need them."),
            ));
        }
        if kind == InputKind::Executable {
            ins.checks.push(check("data_files", Status::Note, "Only the executable was given: the game's files (USRDIR) are needed to play."));
        }
        if ins.truncated {
            ins.checks.push(check("truncated", Status::Warn, "The game holds more files than were looked at; an executable may have been missed."));
        }
        ins.checks.push(host_check(TARGETS));
        Ok(ins.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_elf() -> Vec<u8> {
        let mut e = vec![0u8; 64];
        e[..4].copy_from_slice(b"\x7fELF");
        e[4] = 2;
        e[5] = 2;
        e[0x12..0x14].copy_from_slice(&21u16.to_be_bytes());
        e
    }

    fn retail_self() -> Vec<u8> {
        let mut s = vec![0u8; 0x100];
        s[..4].copy_from_slice(b"SCE\0");
        s[10..12].copy_from_slice(&1u16.to_be_bytes());
        s[0x28..0x30].copy_from_slice(&0x70u64.to_be_bytes());
        s[0x70 + 12..0x70 + 16].copy_from_slice(&4u32.to_be_bytes());
        s
    }

    fn sfo() -> Vec<u8> {
        // TITLE = "Test", one entry.
        let mut d = b"\0PSF".to_vec();
        d.extend(0x0101u32.to_le_bytes());
        d.extend(36u32.to_le_bytes());
        d.extend(44u32.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend(0u16.to_le_bytes());
        d.extend(0x0204u16.to_le_bytes());
        d.extend(5u32.to_le_bytes());
        d.extend(8u32.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(b"TITLE\0\0\0");
        d.extend(b"Test\0\0\0\0");
        d
    }

    fn game(dir: &Path) -> std::path::PathBuf {
        let usr = dir.join("PS3_GAME").join("USRDIR");
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(dir.join("PS3_GAME").join("PARAM.SFO"), sfo()).unwrap();
        std::fs::write(usr.join("EBOOT.BIN"), retail_self()).unwrap();
        usr
    }

    #[test]
    fn an_encrypted_eboot_is_blocked_and_never_decrypted() {
        let dir = tempfile::tempdir().unwrap();
        let usr = game(dir.path());
        std::fs::write(usr.join("data.edat"), b"NPD\0").unwrap();
        let ins = Ps3.inspect(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(ins.title.as_deref(), Some("Test"));
        assert_eq!(ins.verdict, Verdict::Blocked);
        let c = ins.checks.iter().find(|c| c.id == "executable").unwrap();
        assert!(c.message.contains("doesn't decrypt"), "{}", c.message);
        assert!(ins.checks.iter().any(|c| c.id == "licensed_data"));
    }

    #[test]
    fn a_decrypted_elf_beside_it_makes_it_ready() {
        let dir = tempfile::tempdir().unwrap();
        let usr = game(dir.path());
        std::fs::write(usr.join("EBOOT.ELF"), ppu_elf()).unwrap();
        let ins = Ps3.inspect(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(ins.verdict, Verdict::Ready, "{:?}", ins.checks);
        assert_eq!(ins.main_executable.as_deref(), Some("PS3_GAME/USRDIR/EBOOT.ELF"));
    }

    #[test]
    fn single_files_and_disc_images() {
        let dir = tempfile::tempdir().unwrap();
        let elf = dir.path().join("EBOOT.ELF");
        std::fs::write(&elf, ppu_elf()).unwrap();
        assert_eq!(Ps3.inspect(&elf, &Cancel::default()).unwrap().verdict, Verdict::Ready);
        let mut iso = vec![0u8; 0x8006];
        iso[0x8001..0x8006].copy_from_slice(b"CD001");
        let iso_path = dir.path().join("game.iso");
        std::fs::write(&iso_path, iso).unwrap();
        let ins = Ps3.inspect(&iso_path, &Cancel::default()).unwrap();
        assert_eq!(ins.input_kind, InputKind::DiscImage);
        assert_eq!(ins.verdict, Verdict::Blocked);
        let junk = dir.path().join("junk.bin");
        std::fs::write(&junk, b"hello").unwrap();
        assert!(Ps3.inspect(&junk, &Cancel::default()).is_err());
    }
}
