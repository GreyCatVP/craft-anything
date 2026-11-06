# Grok Build

Craft Anything can use the installed Grok Build CLI for analysis and building through the
Agent Client Protocol (ACP). This is separate from the xAI API. When `XAI_API_KEY` is set,
Settings → AI also offers **xAI (Grok)** as an OpenAI-compatible server for questions.

## Setup

1. Install Grok Build. Its program is `grok`, usually in `~/.grok/bin` (or `%USERPROFILE%\.grok\bin`).
   Craft Anything looks there even when a desktop launch left that folder off `PATH`.
2. In your own terminal, run `grok login` and finish the sign-in. Grok keeps the credentials.
   Craft Anything only checks that `auth.json` exists under `GROK_HOME`, or `~/.grok` when that
   variable is unset. It does not read the file, collect an API key, or sign in for you.
3. In Settings → AI, detect and add **Grok Build**, then choose it for chat or **Build with this**.
   If `grok` is installed somewhere else, use **Choose...** and pick the program.

The equivalent backend entry in `config.toml` is:

```toml
[[ai.backends]]
name = "Grok Build"
kind = "agent_cli"
model = "grok"
enabled = true
```

`model` names the agent. ACP is on for this agent. Do not set `acp = false` or an SSH host:
those are refused rather than opening the interactive TUI, which has no way to ask Craft
Anything for approval. Extra `args` are refused too. They would be placed before `agent` and
could request blanket approval. There is no folder override: the run uses the sign-in Grok
already has on this computer.

## What runs

A build or a question starts:

```sh
grok agent --model <cli_model> --no-leader --reasoning-effort <level> stdio
```

`--model` and `--reasoning-effort` are omitted when you have not chosen them. `--no-leader` is
always passed, so the run does not attach to a Grok session you already have open. `stdio`
comes last. Craft Anything does not pass `--always-approve`.

The CLI `--model` flag is visible while Grok starts, then `session/new` switches back to the
model saved in Grok. Craft Anything sets the chosen model again with `session/set_config_option`
before the session is ready, and sets the reasoning effort after that, because a model change
can reset it. The value on the wire is a string (`"grok-4.6"`, `"low"`), not a nested object.
If the session already has that value, the call is skipped.

## Model and thinking

Use the shared model and **Thinking depth** picker (Settings → AI → **Change...**), or set
`cli_model` and `cli_thinking` on the same backend entry. The list comes from the session's
advertised models. There is no `auto` entry and no context-window control: Grok does not
advertise one, and inventing Standard and Extended tiers would be wrong. Reasoning levels are
whatever that model lists (`low`, `medium`, `high`, `xhigh`, and so on). A level the model
does not list is refused. An older `cli_reasoning_effort` or `effort` value is still used when
`cli_thinking` is unset.

```toml
cli_model = "grok-4.6"
cli_thinking = "low"
```

Discovery sends no prompt. It does open a session long enough to read the model list and apply
`cli_model`, so the thinking choices match that model.

## Tools, MCP and Clean room

Builds attach Craft Anything's MCP server on `session/new` as a stdio server named
`craft-anything`. Questions attach no server and refuse permission requests. Grok also keeps
the MCP servers from your own Grok config. Craft Anything does not edit that config and cannot
turn those servers off for one run.

Clean room depends on Grok asking through `session/request_permission`. Requests it does not
send are not something this integration can refuse.

## What was checked

Against Grok Build 1.0.77: `initialize`, `session/new`, and `session/set_config_option` (a
string model id sticks; the CLI `--model` flag does not survive `session/new`;
`--reasoning-effort` after `agent` does stick and is sent again after a model change). The
argument order, the `type: stdio` server payload, and the config queue are covered by unit
tests with a scripted agent. No live prompt or build was run, and a real Craft Anything MCP
process was not connected during that check.
