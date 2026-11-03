# Using Codex with Craft Anything

Craft Anything reaches Codex in two ways, and both have to get past Codex's own tool approval.

## Why a tool can be refused

`codex exec` is non-interactive: it has no one to ask, so its approval policy is `never`. A tool
that an MCP server doesn't mark read-only (such as `rebuild_start`, which runs a coding agent) is
refused before our server sees the call, with `requires approval, but approval policy is never`.
Codex's own settings decide this; Craft Anything's consent is a second, separate check.

## What we set

- **Connecting Codex to Craft Anything** (`craft-anything-cli setup --client codex --yes`, or the
  Connect screen) writes the entry in `~/.codex/config.toml` (`$CODEX_HOME/config.toml` if set)
  with `default_tools_approval_mode = "approve"`:

  ```toml
  [mcp_servers.craft-anything]
  command = "…/craft-anything-cli"
  args = ["mcp"]
  startup_timeout_sec = 30
  default_tools_approval_mode = "approve"
  ```

  Your other entries and comments stay as they are, and the old file is kept as
  `config.toml.craft-anything.backup`. `craft-anything-cli doctor --client codex` reports an entry
  without the setting as "a different copy or shape of the command"; running setup again fixes it.
- **Building with Codex from the app** starts `codex exec` with the same setting on its `-c`
  flags, so nothing in your config is needed.

## Who still asks

Approving the tools in Codex doesn't remove Craft Anything's own consent: a tool that runs code or
changes a device (`rebuild_start` among them) still asks the person in the app (Allow / Don't
allow), unless the server was started with `--allow-run`. In Clean room mode the tools that touch
the original stay refused.

## If it still fails

A refused call now says what was blocked and how to fix it, instead of showing `null`. To allow
tools by hand, add `default_tools_approval_mode = "approve"` under `[mcp_servers.craft-anything]`
and start the chat again. To approve only some tools, use
`[mcp_servers.craft-anything.tools.rebuild_start]` with `approval_mode = "approve"` instead.

Not yet verified on a real Codex install (this was written from Codex's published configuration
keys, tested with stand-in output).
