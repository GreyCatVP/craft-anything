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
`cli_model` on the same backend entry. With no override, the app shows **Copilot CLI default**
and respects the CLI's own model configuration. The picker offers `auto` and models advertised
by a fresh ACP session. Model discovery has a deadline and requires the CLI to be ready and
authenticated. Account entitlement, model access, organization policies and billing remain
GitHub's responsibility; a Copilot subscription is not an unlimited API allowance.

## Context and Reasoning

In Settings > AI, **Context window** and **Reasoning** are always shown beneath each Copilot
entry's model selector. Their choices load automatically for that model. Changing a selection
saves it immediately; there is no separate options menu or apply/cancel step. The rows stay
visible while loading or saving. A failed discovery shows its error with a retry control.
Changing the model reloads its choices and clears the old overrides so an unsupported effort
is not carried into the new model.

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
cli_reasoning_effort = "xhigh"
```

Place these on the same backend entry. An older `effort = "high"` remains a fallback until the
new controls are changed; selecting Default removes that old override too. GUI changes take
effect on the next chat turn, or the next build/continue run, not midway through a running turn.
Model discovery sends no model prompt and runs off the UI thread.

The same capability is available headlessly:

```sh
craft-anything-cli ai-model-options-show --backend "GitHub Copilot"
craft-anything-cli ai-model-options-set --backend "GitHub Copilot" --expected-cli-model gpt-6-luna --context long_context --reasoning xhigh
```

The show tool returns the current `cli_model` and supported choices. Supply that model as
`expected_cli_model` when saving; a changed model is refused instead of applying stale choices.
For a backend inheriting the CLI's model, the guard is null. The write tool also accepts null
context/reasoning values to restore defaults.

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
is handed to the child, with the active target restored last. Deep mode reopens and verifies
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
- ACP usage totals are unavailable here. The activity stream says so, and no dollar cost is
  fabricated. Consult Copilot CLI for usage and billing.
- Questions use current project context rather than a fork of the builder's conversation.
  Follow-up notes wait for a turn boundary; Claude Code's mid-tool notes hook has no equivalent
  in this integration yet.
- The explicit tool allowlist covers built-in engine and AI tools, not runtime plugin tools.
  An expansion needs an end-to-end restriction test before those tools are enabled.

Reference: [GitHub's ACP server documentation](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server).
