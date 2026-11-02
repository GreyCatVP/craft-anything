//! Maki (`maki -p`).
//!
//! `-p/--print` is a switch: the prompt is Maki's positional argument, so it goes last. Plain
//! `maki <prompt>` opens its terminal interface instead of answering.

use super::*;

pub struct Maki;

impl AgentAdapter for Maki {
    fn ids(&self) -> &'static [&'static str] {
        &["maki"]
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(m) = &cx.options.cli_model {
            v.extend(["-m".into(), m.clone()]);
        }
        v.extend(cx.options.args.iter().cloned());
        v.extend(["-p".into(), format!("{}{}", cx.preamble, cx.prompt)]);
        v
    }

    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--print"])
    }

    /// Plain `maki <prompt>` opens the terminal interface; a failed flag check keeps this adapter.
    fn plain_fallback(&self) -> bool {
        false
    }

    fn streams_json(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(builder: bool, options: &BackendOptions) -> Vec<String> {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        Maki.args(&ArgsCx { options, prompt: "make a game", preamble: "PRE ", mcp: &mcp, resume: None, builder, reads_checked: false, question: false })
    }

    #[test]
    fn the_prompt_goes_last_after_print_and_the_flags() {
        assert_eq!(args(false, &BackendOptions::default()), ["-p", "PRE make a game"]);
        let options =
            BackendOptions { cli_model: Some("anthropic/claude-sonnet-4-6".into()), args: vec!["--max-turns".into(), "40".into()], ..Default::default() };
        assert_eq!(args(true, &options), ["-m", "anthropic/claude-sonnet-4-6", "--max-turns", "40", "-p", "PRE make a game"]);
    }

    #[test]
    fn maki_is_registered_and_never_run_plain() {
        assert!(AgentAdapters::built_in().for_agent("maki").ids().contains(&"maki"), "registered, not the plain adapter");
        assert_eq!(Maki.required_flags(), (&["--help"][..], &["--print"][..]));
        assert!(!Maki.plain_fallback() && !Maki.streams_json());
    }
}
