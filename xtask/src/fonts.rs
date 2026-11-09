//! `cargo xtask fonts`: get the craft-fonts fallbacks a local build needs.
//!
//! The desktop app embeds DM Sans and JetBrains Mono only; the CJK, Arabic, Thai, Hebrew,
//! Devanagari and symbol fallbacks live in the sibling craft-fonts repository. A build without
//! them shows boxes for those characters, so a local build needs them.
//!
//! This finds a craft-fonts checkout (or clones one) and prints the `CRAFT_FONTS_DIR` to build
//! with. Releases and CI set the variable themselves; this is for people building from source.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The craft-fonts commit the release and CI workflows pin. Changing it means changing those
/// workflows too, so the three builds ship the same glyphs.
const PINNED_REF: &str = "b076417b5880bd4154bd22f00695b1d28c6bea83";

const REPO: &str = "storytold/craft-fonts";

/// Where a checkout is looked for, in order: the variable that is already set, then the
/// directories next to this repository (the usual place for a sibling checkout).
pub fn candidates(root: &Path) -> Vec<PathBuf> {
    candidates_from(root, std::env::var_os("CRAFT_FONTS_DIR"))
}

/// The candidate list for a given `CRAFT_FONTS_DIR` value. The value is a parameter so a test
/// can name one without changing the process environment (which the test harness runs in
/// parallel with other tests).
fn candidates_from(root: &Path, set: Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(set) = set {
        out.push(PathBuf::from(set));
    }
    if let Some(parent) = root.parent() {
        out.push(parent.join("craft-fonts"));
    }
    out.push(root.join("craft-fonts"));
    out
}

/// A directory is a craft-fonts checkout when its manifest is there.
pub fn is_checkout(dir: &Path) -> bool {
    dir.join("fonts").join("manifest.txt").is_file()
}

/// `cargo xtask fonts [--check] [--print]`: find or fetch the fallbacks and print the variable
/// to set. `--check` only reports whether a checkout is there; `--print` prints the path alone,
/// so a shell can put it in the variable.
pub fn run(root: &Path, check: bool, print_path: bool) -> bool {
    for dir in candidates(root) {
        if is_checkout(&dir) {
            if print_path {
                println!("{}", dir.display());
            } else {
                println!("craft-fonts: {}", dir.display());
                print_env(&dir);
            }
            return true;
        }
    }
    if check {
        eprintln!("fonts: no craft-fonts checkout found; run `cargo xtask fonts` to fetch one");
        return false;
    }
    let target = root.parent().map(|p| p.join("craft-fonts")).unwrap_or_else(|| root.join("craft-fonts"));
    match fetch(&target) {
        Ok(()) => {
            if print_path {
                println!("{}", target.display());
            } else {
                println!("craft-fonts: {}", target.display());
                print_env(&target);
            }
            true
        }
        Err(e) => {
            eprintln!("fonts: {e}");
            eprintln!("fonts: clone {REPO} yourself and set CRAFT_FONTS_DIR to the checkout");
            false
        }
    }
}

/// The line a developer pastes into their shell (and the build reads). Windows gets the
/// PowerShell form and the others the shell form, so pasting it works on every platform.
fn print_env(dir: &Path) {
    println!();
    println!("Build with the fallbacks in:");
    if cfg!(windows) {
        println!("  $env:CRAFT_FONTS_DIR = \"{}\"", dir.display());
    } else {
        println!("  export CRAFT_FONTS_DIR=\"{}\"", dir.display());
    }
}

/// Fetch the pinned craft-fonts commit into `target` with `git`.
fn fetch(target: &Path) -> Result<(), String> {
    if target.exists() {
        return Err(format!("{} exists but has no fonts/manifest.txt", target.display()));
    }
    let git = |args: &[&str]| -> Result<(), String> {
        let out = std::process::Command::new("git").args(args).output().map_err(|e| format!("git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    };
    println!("fetching {REPO} at {PINNED_REF}…");
    git(&["clone", "--quiet", "https://github.com/storytold/craft-fonts.git", &target.to_string_lossy()])?;
    // A clone that can't reach the pinned commit is not a checkout: leave it behind and the next
    // run would take it for one, so take it away again.
    if let Err(e) = git(&["-C", &target.to_string_lossy(), "checkout", "--quiet", PINNED_REF]) {
        let _ = std::fs::remove_dir_all(target);
        return Err(e);
    }
    if !is_checkout(target) {
        let _ = std::fs::remove_dir_all(target);
        return Err(format!("{} has no fonts/manifest.txt after checkout", target.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory only counts when the manifest is there: a folder with a `fonts` directory but
    /// no manifest would make the build fail later, with a much less obvious message.
    #[test]
    fn a_checkout_needs_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_checkout(dir.path()));
        std::fs::create_dir_all(dir.path().join("fonts")).unwrap();
        assert!(!is_checkout(dir.path()));
        std::fs::write(dir.path().join("fonts/manifest.txt"), "").unwrap();
        assert!(is_checkout(dir.path()));
    }

    /// The variable that is already set comes first, so a person who set it gets their own
    /// directory rather than a sibling checkout.
    #[test]
    fn the_set_variable_is_the_first_candidate() {
        let root = Path::new("/repo");
        let set = PathBuf::from("/somewhere/else");
        let found = candidates_from(root, Some(set.clone().into_os_string()));
        assert_eq!(found.first().map(PathBuf::as_path), Some(set.as_path()));
    }

    /// Without the variable, the folders beside the repository are the candidates.
    #[test]
    fn a_sibling_checkout_is_the_next_candidate() {
        let root = Path::new("/repo");
        let found = candidates_from(root, None);
        assert_eq!(found, vec![PathBuf::from("/craft-fonts"), PathBuf::from("/repo/craft-fonts")]);
    }
}
