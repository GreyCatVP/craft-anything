//! Antigravity (`agy`). Flags are checked against `agy --help`.
//!
//! Antigravity runs prompts non-interactively with `--output-format stream-json -p <prompt>`.
//! In builder mode, `--dangerously-skip-permissions` is passed so tool requests proceed without prompting.
//! In question mode, `--mode plan` is passed without permission bypass so it can only read and plan.

use super::*;

pub const DEFAULT_MODEL: &str = "gemini-3.8-flash-high";
pub const MODELS: &[&str] =
    &["gemini-3.8-flash-high", "gemini-3.8-flash-medium", "gemini-3.8-flash-low", "claude-sonnet-4-6", "claude-opus-4-6-thinking", "gpt-oss-120b-medium"];

/// Antigravity's failure in plain words and its fix when the person can make one.
pub fn antigravity_failure(why: &str) -> (String, Option<String>) {
    let w = why.to_ascii_lowercase();
    let remediation = if ["authenticat", "oauth", "log in", "login", "not logged", "unauthorized", "api key"].iter().any(|token| w.contains(token)) {
        Some("Sign in to Antigravity: open a terminal and run `agy` (or sign in via Antigravity IDE), then try again.".into())
    } else {
        None
    };
    (format!("Antigravity stopped: {why}"), remediation)
}

/// Antigravity answering a question about a build: runs with `--mode plan` without auto-skipping
/// permissions so files cannot be modified.
fn question_args(mut a: Vec<String>, cx: &ArgsCx) -> Vec<String> {
    if let Some(m) = &cx.options.cli_model {
        a.extend(["--model".into(), m.clone()]);
    } else {
        a.extend(["--model".into(), DEFAULT_MODEL.into()]);
    }
    if let Some(r) = cx.resume {
        a.extend(["--conversation".into(), r.to_string()]);
    }
    a.extend(["--mode".into(), "plan".into()]);
    a.extend(["--output-format".into(), "stream-json".into(), "-p".into(), format!("{}{}", cx.preamble, cx.prompt)]);
    a
}

pub struct Antigravity;

impl AgentAdapter for Antigravity {
    fn ids(&self) -> &'static [&'static str] {
        &["antigravity", "agy", "gemini-cli", "gemini"]
    }

    fn answers_questions(&self) -> bool {
        true
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut a = cx.options.args.clone();
        if cx.question {
            return question_args(a, cx);
        }
        if cx.builder {
            a.push("--dangerously-skip-permissions".into());
        }
        if let Some(m) = &cx.options.cli_model {
            a.extend(["--model".into(), m.clone()]);
        }
        if let Some(r) = cx.resume {
            a.extend(["--conversation".into(), r.to_string()]);
        }
        a.extend(["--output-format".into(), "stream-json".into(), "-p".into(), format!("{}{}", cx.preamble, cx.prompt)]);
        a
    }

    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--output-format", "stream-json"])
    }

    fn streams_json(&self) -> bool {
        true
    }

    fn resumes(&self) -> bool {
        true
    }

    fn signed_in(&self, program: &str) -> Option<bool> {
        let out = Process::new(program).args(["models"]).timeout(Duration::from_secs(10)).stdout_cap(64 * 1024).run().ok()?;
        Some(out.status.success() && !out.stdout_text().trim().is_empty())
    }

    fn sign_in_fix(&self) -> Option<String> {
        Some("Sign in to Antigravity: open a terminal and run `agy` (or sign in via Antigravity IDE), then try again.".into())
    }

    fn parse(&self, p: &mut StreamParser, v: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let event = v.get("event").and_then(Value::as_str).unwrap_or_default();
        match event {
            "init" => {
                if let Some(id) = v.get("conversation_id").and_then(Value::as_str) {
                    p.session_id = Some(id.to_string());
                }
            }
            "step_update" => {
                let Some(su) = v.get("step_update") else { return out };
                if let Some(id) = su.get("conversation_id").and_then(Value::as_str) {
                    p.session_id.get_or_insert_with(|| id.to_string());
                }
                let step_type = su.get("step_type").and_then(Value::as_str).unwrap_or_default();
                let state = su.get("state").and_then(Value::as_str).unwrap_or_default();
                match step_type {
                    "agent_response" => {
                        if let Some(text) = su.get("text_delta").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(AgentEvent::TextDelta { text: text.to_string() });
                        }
                    }
                    "tool" => {
                        let id = su.get("step_index").map(|i| i.to_string()).unwrap_or_default();
                        let name = su.get("tool_name").or_else(|| su.pointer("/tool_info/name")).and_then(Value::as_str).unwrap_or_default();
                        if state == "ACTIVE" {
                            let input = su.pointer("/tool_info/parameters").cloned().unwrap_or(Value::Null);
                            out.push(p.started(&id, name, input));
                        } else if state == "DONE" || state == "ERROR" {
                            if !p.tool_names.contains_key(&id) {
                                let input = su.pointer("/tool_info/parameters").cloned().unwrap_or(Value::Null);
                                out.push(p.started(&id, name, input));
                            }
                            let ok = state != "ERROR" && su.pointer("/tool_info/error").is_none();
                            let text = su.pointer("/tool_info/output").or_else(|| su.pointer("/tool_info/error")).map(content_text).unwrap_or_default();
                            out.push(p.finished(&id, ok, &text));
                        }
                    }
                    _ => {}
                }
            }
            "result" => {
                p.saw_result = true;
                if let Some(res) = v.get("result") {
                    p.cost_usd = res.get("cost_usd").or_else(|| res.get("total_cost_usd")).and_then(Value::as_f64);
                    if let Some(u) = res.get("usage") {
                        let in_tok = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                        let cache_tok = u.get("cache_read_tokens").and_then(Value::as_u64).unwrap_or(0);
                        p.input_tokens = in_tok.saturating_add(cache_tok);
                        let out_tok = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                        let think_tok = u.get("thinking_tokens").and_then(Value::as_u64).unwrap_or(0);
                        p.output_tokens = out_tok.saturating_add(think_tok);
                    }
                    if res.get("status").and_then(Value::as_str) == Some("ERROR") {
                        let why = res.get("error").or_else(|| res.get("response")).and_then(Value::as_str).unwrap_or("run failed");
                        let (message, remediation) = antigravity_failure(why);
                        out.push(AgentEvent::Error { message, remediation, log: None });
                    }
                }
            }
            "error" => {
                let msg = v.pointer("/error/message").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or("unknown error");
                let (message, remediation) = antigravity_failure(msg);
                out.push(AgentEvent::Error { message, remediation, log: None });
            }
            _ => {
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                match s("type").as_str() {
                    "init" => p.session_id = v.get("session_id").and_then(Value::as_str).map(str::to_string),
                    "message" if s("role") == "assistant" => out.push(AgentEvent::TextDelta { text: s("content") }),
                    "tool_use" => {
                        let id = s("tool_id");
                        let name = s("tool_name");
                        out.push(p.started(&id, &name, v.get("parameters").cloned().unwrap_or(Value::Null)));
                    }
                    "tool_result" => {
                        let id = s("tool_id");
                        let ok = s("status") != "error";
                        let text = v.get("output").map(content_text).unwrap_or_default();
                        out.push(p.finished(&id, ok, &text));
                    }
                    "error" => {
                        let (message, remediation) = antigravity_failure(&s("message"));
                        out.push(AgentEvent::Error { message, remediation, log: None });
                    }
                    "result" => {
                        p.saw_result = true;
                        if let Some(st) = v.get("stats") {
                            p.input_tokens = st.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                            p.output_tokens = st.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(builder: bool, resume: Option<&str>, options: &BackendOptions) -> Vec<String> {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        Antigravity.args(&ArgsCx { options, prompt: "analyse repo", preamble: "PRE ", mcp: &mcp, resume, builder, reads_checked: false, question: false })
    }

    #[test]
    fn antigravity_args_construction() {
        let default_opts = BackendOptions::default();
        assert_eq!(args(false, None, &default_opts), ["--output-format", "stream-json", "-p", "PRE analyse repo"]);

        let builder_opts =
            BackendOptions { cli_model: Some("Gemini 3.8 Flash (Medium)".into()), args: vec!["--add-dir".into(), "/repo".into()], ..Default::default() };
        assert_eq!(
            args(true, Some("conv-123"), &builder_opts),
            [
                "--add-dir",
                "/repo",
                "--dangerously-skip-permissions",
                "--model",
                "Gemini 3.8 Flash (Medium)",
                "--conversation",
                "conv-123",
                "--output-format",
                "stream-json",
                "-p",
                "PRE analyse repo"
            ]
        );
        assert!(Antigravity.resumes());
        assert_eq!(Antigravity.required_flags(), (&["--help"][..], &["--output-format", "stream-json"][..]));
        assert!(AgentAdapters::built_in().for_agent("antigravity").ids().contains(&"antigravity"));
        assert!(AgentAdapters::built_in().for_agent("agy").ids().contains(&"agy"));
        assert!(AgentAdapters::built_in().for_agent("gemini-cli").ids().contains(&"gemini-cli"));
        assert!(AgentAdapters::built_in().for_agent("gemini").ids().contains(&"gemini"));
    }

    #[test]
    fn antigravity_stream_parsing() {
        let adapter: Arc<dyn AgentAdapter> = Arc::new(Antigravity);
        let mut p = StreamParser::new(adapter);

        let init = json!({
            "event": "init",
            "conversation_id": "conv-456",
            "init": {"cwd": "/tmp", "permission_mode": "always-proceed"}
        });
        assert!(p.line(&init.to_string()).is_empty());
        assert_eq!(p.session_id.as_deref(), Some("conv-456"));

        let text_step = json!({
            "event": "step_update",
            "step_update": {
                "conversation_id": "conv-456",
                "step_index": 1,
                "state": "ACTIVE",
                "step_type": "agent_response",
                "text_delta": "Hello from Antigravity!"
            }
        });
        let evs = p.line(&text_step.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::TextDelta { text } => assert_eq!(text, "Hello from Antigravity!"),
            other => panic!("expected TextDelta, got {other:?}"),
        }

        let tool_active = json!({
            "event": "step_update",
            "step_update": {
                "step_index": 2,
                "state": "ACTIVE",
                "step_type": "tool",
                "tool_name": "run_command",
                "tool_info": {
                    "name": "run_command",
                    "parameters": {"CommandLine": "cargo check"}
                }
            }
        });
        let evs = p.line(&tool_active.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::ToolStarted { call_id, tool, input } => {
                assert_eq!(call_id, "2");
                assert_eq!(tool, "run_command");
                assert_eq!(input["CommandLine"], "cargo check");
            }
            other => panic!("expected ToolStarted, got {other:?}"),
        }

        let tool_done = json!({
            "event": "step_update",
            "step_update": {
                "step_index": 2,
                "state": "DONE",
                "step_type": "tool",
                "tool_name": "run_command",
                "tool_info": {
                    "name": "run_command",
                    "parameters": {"CommandLine": "cargo check"},
                    "output": "Finished dev profile"
                }
            }
        });
        let evs = p.line(&tool_done.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::ToolFinished { call_id, tool, ok, summary, .. } => {
                assert_eq!(call_id, "2");
                assert_eq!(tool, "run_command");
                assert!(ok);
                assert_eq!(summary, "Finished dev profile");
            }
            other => panic!("expected ToolFinished, got {other:?}"),
        }

        let result = json!({
            "event": "result",
            "result": {
                "conversation_id": "conv-456",
                "status": "SUCCESS",
                "response": "Done!",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "thinking_tokens": 10,
                    "cache_read_tokens": 20,
                    "total_tokens": 180
                }
            }
        });
        let evs = p.line(&result.to_string());
        assert!(evs.is_empty());
        assert!(p.saw_result);
        assert_eq!(p.input_tokens, 120);
        assert_eq!(p.output_tokens, 60);

        let err = json!({
            "event": "error",
            "error": "authentication expired"
        });
        let evs = p.line(&err.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Error { message, remediation, .. } => {
                assert!(message.contains("authentication expired"));
                assert!(remediation.is_some() && remediation.as_ref().unwrap().contains("Sign in to Antigravity"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn antigravity_answers_questions_and_args() {
        assert!(Antigravity.answers_questions());
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        let opts = BackendOptions::default();
        let q_args = Antigravity.args(&ArgsCx {
            options: &opts,
            prompt: "what stack was used?",
            preamble: "PRE ",
            mcp: &mcp,
            resume: Some("conv-99"),
            builder: false,
            reads_checked: false,
            question: true,
        });
        assert_eq!(
            q_args,
            ["--model", DEFAULT_MODEL, "--conversation", "conv-99", "--mode", "plan", "--output-format", "stream-json", "-p", "PRE what stack was used?"]
        );
        assert!(!q_args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn antigravity_sign_in_hint_and_remediation() {
        assert!(Antigravity.sign_in_fix().is_some());
        let (msg, fix) = antigravity_failure("login required to proceed");
        assert!(msg.contains("Antigravity stopped"));
        assert!(fix.is_some() && fix.unwrap().contains("Sign in to Antigravity"));
    }
}
