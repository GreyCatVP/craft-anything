//! Original addresses to the C++ Xbox XBE Recomp generated, and back.
//!
//! The recompiler names each function after its original entry (`sub_003AB12A`), writes its body
//! as `X86_FUNC(__imp__sub_003AB12A) { … }` in one of `generated/game/x86_recomp.<n>.cpp`, and puts
//! each original instruction above its C++ as `// 003AB12A push esi`. A host function with the
//! plain name (`X86_FUNC(sub_003AB12A)` in the build's `src/`) replaces the body at link time.
//!
//! [`CodeMap`] indexes the bodies once into `<build>/code_map.tsv` (rebuilt when the recompiler's
//! output is newer), so finding a function never rereads the whole generated tree (180 MB for one
//! title).

use craft_anything_evidence::ToolError;
use std::path::{Path, PathBuf};

/// The map file's first line; a different one is rebuilt.
const HEADER: &str = "# craft-anything code map v1: address, file, first line, last line";
/// Generated bodies indexed at most (a large title has under 100 000).
const MAX_FUNCTIONS: usize = 1_000_000;
/// Generated files read at most.
const MAX_FILES: usize = 10_000;

/// One generated function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub address: u32,
    /// File name inside `generated/game`.
    pub file: String,
    /// 1-based lines of `X86_FUNC(__imp__sub_…) {` and of its closing `}`.
    pub first: usize,
    pub last: usize,
}

/// Every generated function body of a build, sorted by address.
#[derive(Debug, Default)]
pub struct CodeMap {
    pub bodies: Vec<Body>,
}

/// The function name the recompiler gives an original address.
pub fn function_name(address: u32) -> String {
    format!("sub_{address:08X}")
}

fn generated(build: &Path) -> PathBuf {
    build.join("game").join("generated").join("game")
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `X86_FUNC(__imp__sub_XXXXXXXX) {` → the address.
fn body_start(line: &str) -> Option<u32> {
    let hex = line.strip_prefix("X86_FUNC(__imp__sub_")?.strip_suffix(") {")?;
    u32::from_str_radix(hex, 16).ok()
}

/// `\t// XXXXXXXX mnemonic …` → the instruction's address.
pub fn instruction_at(line: &str) -> Option<u32> {
    let rest = line.trim_start().strip_prefix("// ")?;
    let hex = rest.get(..8)?;
    (rest.as_bytes().get(8) == Some(&b' ')).then_some(())?;
    u32::from_str_radix(hex, 16).ok()
}

/// Index one generated file's bodies.
pub fn index_file(name: &str, text: &str, out: &mut Vec<Body>) {
    let mut open: Option<(u32, usize)> = None;
    for (i, line) in text.lines().enumerate() {
        if let Some(a) = body_start(line) {
            open = Some((a, i + 1));
        } else if line == "}"
            && let Some((address, first)) = open.take()
            && out.len() < MAX_FUNCTIONS
        {
            out.push(Body { address, file: name.to_string(), first, last: i + 1 });
        }
    }
}

impl CodeMap {
    /// The build's map: read from `code_map.tsv` when it's newer than the generated code, else
    /// built from `generated/game/x86_recomp.*.cpp` and saved.
    pub fn load(build: &Path) -> Result<CodeMap, ToolError> {
        let dir = generated(build);
        let stamp = modified(&dir.join("codegen.stamp")).or_else(|| modified(&dir));
        let Some(stamp) = stamp else {
            return Err(ToolError::invalid(format!("{} has no generated code yet.", build.display()))
                .with_remediation("Run the build through its recompile stage first."));
        };
        let saved = build.join("code_map.tsv");
        if modified(&saved).is_some_and(|m| m >= stamp)
            && let Some(map) = std::fs::read_to_string(&saved).ok().and_then(|t| CodeMap::parse(&t))
        {
            return Ok(map);
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| ToolError::io(dir.display(), e))?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("x86_recomp.") && n.ends_with(".cpp"))
            .take(MAX_FILES)
            .collect();
        names.sort();
        let mut bodies = Vec::new();
        for n in &names {
            let p = dir.join(n);
            let text = std::fs::read_to_string(&p).map_err(|e| ToolError::io(p.display(), e))?;
            index_file(n, &text, &mut bodies);
        }
        bodies.sort_by_key(|b| b.address);
        let map = CodeMap { bodies };
        // A cache: losing it only costs the next lookup a rebuild.
        let _ = craft_anything_platform::fs::write_atomic(&saved, map.render().as_bytes());
        Ok(map)
    }

    fn render(&self) -> String {
        let mut s = String::from(HEADER);
        s.push('\n');
        for b in &self.bodies {
            s.push_str(&format!("{:08X}\t{}\t{}\t{}\n", b.address, b.file, b.first, b.last));
        }
        s
    }

    fn parse(text: &str) -> Option<CodeMap> {
        let mut lines = text.lines();
        (lines.next()? == HEADER).then_some(())?;
        let mut bodies = Vec::new();
        for l in lines.take(MAX_FUNCTIONS) {
            let mut f = l.split('\t');
            let address = u32::from_str_radix(f.next()?, 16).ok()?;
            let file = f.next()?.to_string();
            bodies.push(Body { address, file, first: f.next()?.parse().ok()?, last: f.next()?.parse().ok()? });
        }
        Some(CodeMap { bodies })
    }

    /// The body that starts at `address`, or else the closest one starting below it (the
    /// function that most likely holds it; the recompiler only knows entries).
    pub fn containing(&self, address: u32) -> Option<&Body> {
        let i = self.bodies.partition_point(|b| b.address <= address);
        self.bodies.get(i.checked_sub(1)?)
    }

    /// The body a generated file's line is in.
    pub fn at_line(&self, file: &str, line: usize) -> Option<&Body> {
        self.bodies.iter().find(|b| b.file == file && (b.first..=b.last).contains(&line))
    }
}

/// Names the build's config gives original addresses: `[symbols]` and `[data]` (`NAME = 0xADDR`)
/// and `[functions]` (`0xADDR = "name"`, leaving out the recompiler's own `sub_…` names).
pub fn config_names(config: &str) -> Vec<(u32, String)> {
    let hex = |s: &str| {
        let t = s.trim().trim_matches('"');
        u32::from_str_radix(t.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
    };
    let mut section = "";
    let mut out = Vec::new();
    for line in config.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = match name {
                "symbols" => "symbols",
                "data" => "data",
                "functions" => "functions",
                _ => "",
            };
            continue;
        }
        let Some((k, v)) = line.split('#').next().and_then(|l| l.split_once('=')) else { continue };
        let (k, v) = (k.trim(), v.trim());
        let entry = match section {
            "symbols" | "data" => hex(v).filter(|a| *a != 0).map(|a| (a, k.to_string())),
            "functions" => {
                hex(k).map(|a| (a, v.trim_matches('"').to_string())).filter(|(a, n)| !n.is_empty() && *n != format!("sub_{a:X}") && *n != function_name(*a))
            }
            _ => None,
        };
        out.extend(entry);
    }
    out
}

/// Host functions in the build's own sources (`src/**/*.cpp`) that replace the generated body of
/// `address`: (file relative to the build's game folder, 1-based line).
pub fn overrides(build: &Path, address: u32) -> Vec<(String, usize)> {
    let game = build.join("game");
    let needle = format!("X86_FUNC({})", function_name(address));
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(game.join("src")) else { return out };
    for d in dirs.filter_map(Result::ok).filter(|d| d.path().is_dir()).take(64) {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.filter_map(Result::ok).take(1024) {
            let p = f.path();
            if p.extension().is_none_or(|e| e != "cpp") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for (i, l) in text.lines().enumerate() {
                if l.starts_with(&needle) {
                    let rel = p.strip_prefix(&game).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    out.push((rel, i + 1));
                }
            }
        }
    }
    out
}

/// The lines `first..=last` of a generated file (1-based).
pub fn lines(build: &Path, file: &str, first: usize, last: usize) -> Result<Vec<String>, ToolError> {
    if file.contains(['/', '\\']) || file.contains("..") {
        return Err(ToolError::invalid(format!("{file:?} isn't a generated file name.")));
    }
    let p = generated(build).join(file);
    let text = std::fs::read_to_string(&p).map_err(|e| ToolError::io(p.display(), e))?;
    Ok(text.lines().skip(first.saturating_sub(1)).take(last.saturating_sub(first) + 1).map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "#include \"x86_recomp_shared.h\"\n\nX86_FUNC(__imp__sub_00011000) {\n\t// 00011000 push esi\n\tX86_PUSH32(ctx.esi);\n\t// 00011001 ret\n\treturn;\n}\nX86_WEAK_FUNC(sub_00011000, __imp__sub_00011000)\n\nX86_FUNC(__imp__sub_00011010) {\n\t// 00011010 ret\n}\n";

    #[test]
    fn indexes_bodies_and_reads_instruction_comments() {
        let mut bodies = Vec::new();
        index_file("x86_recomp.0.cpp", FILE, &mut bodies);
        assert_eq!(
            bodies,
            vec![
                Body { address: 0x11000, file: "x86_recomp.0.cpp".into(), first: 3, last: 8 },
                Body { address: 0x11010, file: "x86_recomp.0.cpp".into(), first: 11, last: 13 },
            ]
        );
        let map = CodeMap { bodies };
        assert_eq!(map.containing(0x11001).map(|b| b.address), Some(0x11000));
        assert_eq!(map.containing(0x11010).map(|b| b.address), Some(0x11010));
        assert!(map.containing(0x10FFF).is_none());
        assert_eq!(map.at_line("x86_recomp.0.cpp", 5).map(|b| b.address), Some(0x11000));
        assert!(map.at_line("x86_recomp.0.cpp", 9).is_none());
        assert_eq!(instruction_at("\t// 00011001 ret"), Some(0x11001));
        assert_eq!(instruction_at("\t// not an address"), None);
        let back = CodeMap::parse(&map.render()).unwrap();
        assert_eq!(back.bodies, map.bodies);
        assert!(CodeMap::parse("# some other file\n").is_none());
    }

    #[test]
    fn reads_config_names_without_the_recompilers_own() {
        let n = config_names(
            "[functions]\n0x35637D = \"sub_35637D\"\n0x11000 = \"Boot\"\n[symbols]\nDirectSoundCreate = 0x3AD4EB\nBinkWait = 0\n[data]\nXPP_GamepadType = 0x40954C # found by hand\n[abi]\nX = 1\n",
        );
        assert_eq!(n, vec![(0x11000, "Boot".into()), (0x3AD4EB, "DirectSoundCreate".into()), (0x40954C, "XPP_GamepadType".into())]);
    }

    #[test]
    fn finds_overrides_and_refuses_paths_as_file_names() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("game").join("src").join("game");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hooks.cpp"), "#include \"xdk/hle.h\"\nX86_FUNC(sub_003A9DF8) {  // SetPitch\n}\n").unwrap();
        assert_eq!(overrides(dir.path(), 0x3A9DF8), vec![("src/game/hooks.cpp".to_string(), 2)]);
        assert!(overrides(dir.path(), 0x11000).is_empty());
        assert!(lines(dir.path(), "../x.cpp", 1, 2).is_err());
        assert!(CodeMap::load(dir.path()).is_err());
    }
}
