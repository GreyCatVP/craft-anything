//! Kimi Code CLI (`kimi acp`): its own Kimi account (`kimi login`, a device-code flow, and a
//! credential file for signing out) and its own models, sessions and workspace settings, so
//! Settings shows only that account.

use super::*;

/// Kimi Code CLI's ACP-only adapter.
pub struct KimiCode;

impl AgentAdapter for KimiCode {
    fn ids(&self) -> &'static [&'static str] {
        &["kimi", "kimi-code"]
    }

    fn args(&self, _cx: &ArgsCx) -> Vec<String> {
        // ACP is the only non-interactive way in; the native runner refuses `requires_acp`.
        Vec::new()
    }

    fn requires_acp(&self) -> bool {
        true
    }

    fn plain_fallback(&self) -> bool {
        false
    }

    /// The CLI keeps its own model, sessions and workspace settings.
    fn shows_options(&self) -> bool {
        false
    }

    fn account_service(&self) -> Option<&'static str> {
        Some("Kimi")
    }

    fn account_tools(&self) -> Option<(&'static str, &'static str)> {
        Some(("ai_kimi_sign_in", "ai_kimi_sign_out"))
    }

    fn account_state(&self) -> Option<(bool, Option<String>)> {
        let home = crate::kimi_auth::home()?;
        Some((crate::kimi_auth::signed_in(&home), None))
    }

    /// The CLI's own account: a credential file under its folder (`KIMI_CODE_HOME`, else
    /// `~/.kimi-code`).
    fn signed_in(&self, _program: &str) -> Option<bool> {
        crate::kimi_auth::home().map(|home| crate::kimi_auth::signed_in(&home))
    }

    fn signed_in_before(&self) -> bool {
        crate::kimi_auth::home().is_some_and(|home| crate::kimi_auth::signed_in(&home))
    }

    fn sign_in_fix(&self) -> Option<String> {
        Some("Sign in to Kimi Code in Settings → AI, or run `kimi login` in a terminal, then try again. Kimi Code keeps your credentials itself.".into())
    }

    fn folder_var(&self) -> Option<&'static str> {
        Some(crate::kimi_auth::SHARE_VAR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kimi_cli_is_acp_only_and_shows_only_its_account() {
        let adapter = KimiCode;
        assert_eq!(adapter.ids(), ["kimi", "kimi-code"]);
        assert!(adapter.requires_acp() && !adapter.plain_fallback());
        assert!(!adapter.shows_options());
        assert_eq!(adapter.account_service(), Some("Kimi"));
        assert_eq!(adapter.account_tools(), Some(("ai_kimi_sign_in", "ai_kimi_sign_out")));
        assert_eq!(adapter.folder_var(), Some(crate::kimi_auth::SHARE_VAR));
        let options = BackendOptions::default();
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        let cx = ArgsCx { options: &options, prompt: "hi", preamble: "", mcp: &mcp, resume: None, builder: false, reads_checked: false, question: false };
        assert!(adapter.args(&cx).is_empty());
        assert!(adapter.sign_in_fix().is_some_and(|fix| fix.contains("kimi login")));
    }
}
