# Using Antigravity (agy) as the builder

Craft Anything can build with the person's own local Antigravity (`agy`) CLI. This page records what the
integration does, how it operates, and how it mirrors Claude Code support.

## How it works

- Craft Anything finds the `agy` program on the computer (PATH, `~/.gemini/bin` or `$ANTIGRAVITY_HOME/bin`)
  and runs it as a child process in headless mode: `agy --output-format stream-json -p <prompt>`, with
  `--conversation <id>` to resume an ongoing project.
- Model selection: Antigravity defaults to `gemini-3.8-flash-high`, and users can choose other models
  in Settings → AI or `--model` (`gemini-3.8-flash-medium`, `gemini-3.8-flash-low`, `claude-sonnet-4-6`,
  `claude-opus-4-6-thinking`, `gpt-oss-120b-medium`).
- Build mode: In builder mode, `--dangerously-skip-permissions` is passed so non-interactive builds proceed
  without terminal prompts.
- Questions: Questions about a build run in plan mode (`--mode plan`) without skipping permissions, ensuring
  the builder's session can be queried safely without modifying files or triggering side effects.
- MCP tools and skills: Craft Anything integrates with Antigravity via `craft-anything-cli setup antigravity`
  (or `setup agy`), registering the MCP server in Antigravity's `mcp_config.json` (`~/.gemini/config/mcp_config.json`)
  and installing the workspace skill into `~/.gemini/config/skills/craft-anything/SKILL.md`.
- Preflight sign-in check: Before starting a build or run, Antigravity's auth status is verified using `agy models`.
  If authentication is needed, the run stops cleanly with: "Sign in to Antigravity: open a terminal and run `agy`
  (or sign in via Antigravity IDE), then try again."

## Principles

Craft Anything:

- runs the unmodified `agy` binary that the person installed;
- never reads, stores or forwards credentials or tokens, and offers no Antigravity login of its own
  (sign-in happens through Antigravity CLI or the Antigravity IDE);
- doesn't pay for, resell or intermediate usage — usage is billed to the person under their own account;
- uses documented flags (`--output-format stream-json`, `-p`, `--conversation`, `--model`, `--mode plan`,
  `--dangerously-skip-permissions`).
