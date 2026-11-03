# GitHub Copilot

Craft Anything can use the installed GitHub Copilot CLI for analysis and building through the
Agent Client Protocol (ACP).

## Setup

1. Install the standalone Copilot CLI using GitHub's
   [installation instructions](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli).
2. In your own terminal, run `copilot login` and complete GitHub's sign-in flow. The CLI owns
   authentication, credential storage and refresh. Craft Anything does not read its credential
  store, collect a Copilot key or provide a login flow. Retry after login; if authentication
  still fails, restart Craft Anything.
3. In Settings > AI, detect and add **GitHub Copilot**, then choose it for chat or **Build with this**.
   If it is installed outside the searched locations, use **Choose...** to select the executable.

The equivalent backend entry in `config.toml` is:

```toml
[[ai.backends]]
name = "GitHub Copilot"
kind = "agent_cli"
model = "copilot"
enabled = true
```

ACP is enabled automatically for this provider. Do not set `acp = false` or an SSH host: those
configurations are refused rather than falling back to an interactive CLI. Extra `args` are also
refused because they could override the integration's tool or authentication configuration.

The `model` field identifies the agent. To select a model, use the app's model picker or set
`cli_model` on the same backend entry. With no override, the app shows **Model default**
and respects the CLI's own model configuration. The picker offers `auto` and models advertised
by a fresh ACP session. Model discovery has a deadline and requires the CLI to be ready and
authenticated. Account entitlement, model access, organization policies and billing remain
GitHub's responsibility; a Copilot subscription is not an unlimited API allowance.

## Context and Reasoning

Copilot uses the same model and **Thinking depth** picker as OpenCode. In Settings > AI, choose
**Change...**, select a model and its advertised thinking depth, then **Apply**. Model changes
reload model-specific metadata on a worker; stale results and unsupported choices are refused.
The shared catalogue and validated save use `AgentModel`, `ModelContext`, `cli_thinking`,
`ai_models_list` and `ai_model_set`, rather than a second Copilot model-selection path.

**Context window** remains a separate, always-visible Copilot control. It loads automatically
and saves immediately, without changing thinking depth. Loading and retryable errors keep the
control visible. Changing the model clears the old context override.

- **Default** inherits Copilot CLI's own setting. It is not the same as an explicit `none` effort.
- **Standard** passes `--context default`; **Extended** passes `--context long_context`.
  Token limits depend on the model. The picker preserves reported window labels when available;
  CLI 1.0.105 does not advertise numeric window limits in its initial ACP metadata, so this
  version uses tier labels rather than guessing a token count.
- Reasoning choices come from the selected model's ACP `reasoning_effort` options. For example,
  GPT-6 Luna advertised `none`, `low`, `medium`, `high`, `xhigh` and `max` during verification.
  The CLI accepts a wider overall range, but the picker does not offer levels the model omits.
  These are named effort levels, not a configurable numeric thinking-token budget.

The saved options are typed backend fields, not unrestricted extra arguments:

```toml
cli_model = "gpt-6-luna"
cli_context = "long_context"
cli_thinking = "xhigh"
```

Place these on the same backend entry. Older `cli_reasoning_effort` and `effort` settings remain
fallbacks until the shared model/thinking picker is saved; that save clears the old overrides.
An explicit `cli_thinking = "none"` is different from omitting `cli_thinking`. GUI changes take
effect on the next chat turn, or the next build/continue run, not midway through a running turn.
Model discovery sends no model prompt and runs off the UI thread.

The same capability is available headlessly:

```sh
craft-anything-cli ai-models-list --name "GitHub Copilot"
craft-anything-cli ai-model-set --name "GitHub Copilot" --model gpt-6-luna --thinking xhigh
craft-anything-cli ai-model-options-show --backend "GitHub Copilot"
craft-anything-cli ai-model-options-set --backend "GitHub Copilot" --expected-cli-model gpt-6-luna --context long_context
```

The context show tool returns the current `cli_model` and supported choices. Supply that model as
`expected_cli_model` when saving; a changed model is refused instead of applying stale choices.
For a backend inheriting the CLI's model, the guard is null. The write tool also accepts null
context to restore its default. Model and thinking changes go through the shared `ai_model_set`
validation path; the context-only tool never changes thinking.
Context-option discovery follows the calling tool's cancellation token, including before the
CLI starts. Shared picker warnings name the configured agent rather than assuming OpenCode.

## Tools and Permissions

Chat and builds use an explicit allowlist of Craft Anything's built-in engine and AI MCP tools.
Copilot's native file, shell, web and delegation tools are not exposed. The MCP server enforces
Clean room or Deep mode, and build tools retain Craft Anything's approval policy and inbox.
Copilot's built-in MCP servers and custom-instruction loading are disabled for these runs;
the person's own CLI configuration and credentials are not rewritten.

The adapter passes a session-only `--additional-mcp-config` with the installed
`craft-anything-cli mcp` command, investigation and inbox. CLI 1.0.105 did not expose the test
server when it was supplied only through ACP's `session/new`, so both forms are supplied.

Questions about a build start a fresh session, never load or change the builder's session, and
attach no MCP server. Deep-mode questions can use only `view`, `glob` and `grep`. Clean-room
questions have no tools. The empty `--available-tools=` value restores the CLI's defaults, so
the tool-free case instead allowlists `ask_user` and disables it with `--no-ask-user`.

Processes run through the platform supervisor with bounded output and a wall-clock deadline;
the app's stop action sends ACP cancellation and dropping the conversation terminates its
process group. Builds preflight the companion CLI before starting. Session loading is used
only when the agent advertises that capability; failed ACP startup can be retried after login.

## Targets and Findings

Local agent chats use the same saved investigation as the app. If no `.cany` project is open,
the conversation worker creates one under the app's configuration directory and preserves the
current evidence, unknowns and investigation memory before starting MCP. Open target metadata
is handed to the child, keeping at most the last 128 targets with the active target restored
last, matching the receiver's limit. Deep mode reopens and verifies
the files; Clean room shares metadata without reading their contents.

At a turn boundary, changing the project, open targets, active target or analysis mode refreshes
the MCP connection. A saved agent session is carried across only within the same project.
Tool completion and turn completion reload the shared evidence and findings on a worker; the
UI renders the cached snapshot without reading files while drawing.
Rebuild agents receive the same target handoff at startup and refresh their project's shared
investigation before completed tool and turn events reach the app. A refresh error is logged
without ending the builder's turn.

The workbench's **Evidence** pane now lists recorded findings (`fnd_1`, `fnd_2`, ...), their
statements, confidence and links to supporting evidence. These are separate from evidence
records (`ev_...`). Agents read them with `core_investigation_show`, record a settled answer
with `core_finding_record`, and correct one with `core_finding_edit`. Corrections keep the id,
require valid evidence and can refuse a stale `based_on_revision`. Omitting `confidence` keeps
the existing confidence; supplying it explicitly replaces it.

Every project-backed investigation writer holds the same OS file lock across loading state,
checking its revision, applying the change and writing structured state plus Markdown renders.
The wait is cancellable and times out after ten seconds. Concurrent edits based on the same
revision cannot both succeed; malformed state is refused rather than replaced with an empty
investigation. The lock is released when its handle closes, including after process exit.

An older MCP process that started without a project may still hold its findings only in memory.
Before closing that conversation, ask it to export its investigation or a report to a file.
Those session-only records are not automatically recovered by the new shared-project path.

## Verification and Limits

- Unit tests cover discovery/routing, model settings and catalogue parsing, permissions,
  session loading, cancellation, external sign-in guidance and refusal of unsafe options.
- Copilot CLI 1.0.105 was checked with an isolated home, a synthetic MCP server and a fake
  OpenAI-compatible model on loopback. The model received exactly the allowed tools for
  analysis, both build modes and both question modes. No real account or hosted model was used.
- The opt-in compatibility test needs Node.js for its synthetic MCP fixture, not for the
  application itself. Run it with
  `cargo test -p craft-anything-ai copilot_cli_enforces_mcp_only_and_empty_tool_filters -- --ignored`.
- Authenticated builds, account-specific model discovery and real-account session resume were
  confirmed by the person using the app. GitHub documents ACP as public preview.
- The target handoff has process-level MCP tests in both modes. Finding correction/persistence
  has engine tests, and a headless UI test checks that recorded ids and statements render.
  The live authenticated target-to-finding workflow was confirmed by the person using the app.
- Separate-process regressions verify concurrent appends, stale revision refusal and matching
  rendered files. Lock cancellation/timeouts and preservation of malformed state have tests.
  The shared-picker rework has isolated catalogue, persistence and headless GUI tests; it has
  not received a new live-account run after this rebase.
- ACP usage totals are unavailable here. The activity stream says so, and no dollar cost is
  fabricated. Consult Copilot CLI for usage and billing.
- Questions use current project context rather than a fork of the builder's conversation.
  Follow-up notes wait for a turn boundary; Claude Code's mid-tool notes hook has no equivalent
  in this integration yet.
- The explicit tool allowlist covers built-in engine and AI tools, not runtime plugin tools.
  An expansion needs an end-to-end restriction test before those tools are enabled.

Reference: [GitHub's ACP server documentation](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server).
