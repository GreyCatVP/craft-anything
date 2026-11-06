//! Cursor Agent (`agent acp`).

use super::*;

/// First Cursor Agent date known to start Client-supplied MCP servers on `session/new`.
/// 2026.06.04-5fd875e accepted the list and then ran without those tools.
const ACP_MCP_SINCE: (u16, u8, u8) = (2026, 6, 5);

/// Cursor Agent speaks only ACP; the native prompt runner is not offered.
pub struct CursorAgent;

impl AgentAdapter for CursorAgent {
    fn ids(&self) -> &'static [&'static str] {
        &["agent"]
    }

    fn args(&self, _cx: &ArgsCx) -> Vec<String> {
        Vec::new()
    }

    fn requires_acp(&self) -> bool {
        true
    }

    fn plain_fallback(&self) -> bool {
        false
    }

    fn acp_options(&self, _options: &BackendOptions, _access: Access) -> Vec<String> {
        Vec::new()
    }

    fn acp_mcp_since(&self) -> Option<(u16, u8, u8)> {
        Some(ACP_MCP_SINCE)
    }

    fn validate_options(&self, options: &BackendOptions) -> Result<(), ToolError> {
        if options.args.is_empty() {
            Ok(())
        } else {
            Err(ToolError::invalid("Extra Cursor Agent arguments could skip Craft Anything's approval step.").with_remediation(
                "Remove this backend's extra arguments. Flags such as --force, -f and --yolo let Cursor run shell commands and file writes without asking.",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_not_plain() {
        assert!(AgentAdapters::built_in().for_agent("agent").ids().contains(&"agent"));
        assert!(CursorAgent.requires_acp() && !CursorAgent.plain_fallback());
        assert_eq!(CursorAgent.acp_mcp_since(), Some((2026, 6, 5)));
    }

    #[test]
    fn extra_arguments_that_can_disable_approval_are_refused() {
        assert!(CursorAgent.validate_options(&BackendOptions::default()).is_ok());
        assert!(CursorAgent.acp_options(&BackendOptions::default(), Access::Build).is_empty());
        for argument in ["--force", "-f", "--yolo", "--print", "--sandbox=disabled"] {
            let options = BackendOptions { args: vec![argument.into()], ..Default::default() };
            assert!(CursorAgent.validate_options(&options).is_err(), "{argument}");
            assert!(CursorAgent.acp_options(&options, Access::Build).is_empty(), "{argument} must not reach argv");
        }
    }
}
