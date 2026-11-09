//! Optional craft-fonts (fallbacks for CJK, Arabic, Thai, Hebrew, Devanagari…), following
//! craft-fonts' `docs/integration.md`. Releases and CI set it.
//!
//! Unset `CRAFT_FONTS_DIR` (the default) embeds nothing and the app uses its own DM Sans/JetBrains
//! Mono. Set it to an absolute path of a craft-fonts checkout to embed every font its manifest
//! lists. A bad path is a warning unless `CRAFT_FONTS_REQUIRED=1`.
//!
//! Without these fallbacks the interface shows boxes wherever a script needs one (Chinese,
//! Japanese, Korean, Thai, Arabic, Hebrew, Devanagari, symbols). A build that ships them says so
//! loudly, because the boxes only show up once someone runs the app. `cargo xtask fonts` fetches
//! a checkout and prints the `CRAFT_FONTS_DIR` to set.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let mut src = String::from("pub static CRAFT_FONTS: &[CraftFont] = &[\n");
    if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) {
        match craft_fonts(&dir) {
            Ok(entries) => src.push_str(&entries),
            Err(e) if std::env::var_os("CRAFT_FONTS_REQUIRED").is_some() => {
                println!("cargo::error=CRAFT_FONTS_DIR={}: {e}", dir.display());
            }
            Err(e) => println!("cargo::warning=building without craft-fonts: CRAFT_FONTS_DIR={}: {e}", dir.display()),
        }
    } else if std::env::var_os("CRAFT_FONTS_REQUIRED").is_none() {
        // The wasm build ships one font (BIZ UDPGothic) and the browser viewer uses egui's own
        // defaults, so only a desktop build without fallbacks needs the warning.
        if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
            println!(
                "cargo::warning=CRAFT_FONTS_DIR is not set: the app will show boxes for Chinese, Japanese, Korean, Thai, Arabic, Hebrew, Devanagari and symbol characters. Run `cargo xtask fonts` to fetch a craft-fonts checkout, then build with CRAFT_FONTS_DIR pointing at it."
            );
        }
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("craft_fonts.rs");
    if let Err(e) = std::fs::write(&out, src) {
        println!("cargo::error=writing {}: {e}", out.display());
    }
}

/// One `CraftFont { .. }` initialiser per manifest line.
fn craft_fonts(dir: &std::path::Path) -> Result<String, String> {
    let manifest = dir.join("fonts/manifest.txt");
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    const WEB_FONTS: &[(&str, &str)] = &[("BIZ UDPGothic", "Regular")];
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "wasm32");
    let mut out = String::new();
    let mut entries = 0usize;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split(" | ").map(str::trim).collect();
        let [family, style, file, scripts, ..] = f.as_slice() else {
            return Err(format!("malformed manifest line: {line}"));
        };
        entries += 1;
        if wasm && !WEB_FONTS.contains(&(*family, *style)) {
            continue;
        }
        let path = dir.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
        println!("cargo::rerun-if-changed={}", path.display());
        let scripts: Vec<String> = scripts.split(',').map(|s| format!("{:?}", s.trim())).collect();
        let _ = writeln!(
            out,
            "    CraftFont {{ family: {family:?}, style: {style:?}, scripts: &[{}], bytes: include_bytes!({:?}) }},",
            scripts.join(", "),
            path.display().to_string(),
        );
    }
    // A manifest with no fonts at all means the checkout is not what it should be. Saying so beats
    // an app that quietly shows boxes for every language but Latin.
    if entries == 0 {
        return Err(format!("{} has no font entries", manifest.display()));
    }
    Ok(out)
}
