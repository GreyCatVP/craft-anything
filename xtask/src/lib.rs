//! The `cargo xtask` toolbox: the checks and helpers that keep the workspace honest.
//!
//! The binary in `main.rs` is only the command line; everything it can do lives here, so the
//! same code can be tested and called from elsewhere.

pub mod fonts;
pub mod i18n_coverage;
pub mod ico;
pub mod locks;
pub mod secrets;

use std::path::{Path, PathBuf};

/// The workspace root: the directory that holds `crates/`, `apps/` and `Cargo.toml`.
pub fn workspace_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent().map(Path::to_path_buf).unwrap_or(here)
}

#[cfg(test)]
mod tests {
    use super::workspace_root;

    #[test]
    fn readme_status_does_not_list_first_public_release_as_next() {
        let readme = std::fs::read_to_string(workspace_root().join("README.md")).unwrap();
        let status = readme.split("\n## Status").nth(1).unwrap();
        let status = status.split("\n**Responsible use.**").next().unwrap().replace("\n> ", " ");
        assert!(!status.contains("**Next:** the first public release"), "{status}");
    }
}
