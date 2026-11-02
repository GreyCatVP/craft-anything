# Extending Craft Anything

Every part people tend to change sits behind a seam: a trait or a table with built-in entries and one
place to add yours. Follow `AGENTS.md` (never crash, layers, external programs through
`craft_anything_platform::process`) and give each addition tests.

| You want to add | Where | How |
|---|---|---|
| A tool | `crates/engine/src/tools/<area>.rs` | A `ToolSpec` (id per [tool-ids.md](tool-ids.md), title, summary, beginner text, toolset, honest effects, a schemars params type, `run`), and one `v.extend(<area>::specs())` line in `tools/mod.rs`. It's then a CLI command, an MCP tool and callable by the agent and the app. It runs only in Deep mode until it's listed in `crates/engine/src/clean_room.rs`; list it there only if it touches no target or watches from outside, as a visitor or user would. |
| A tool from a higher layer | that crate, e.g. `crates/ai/src/rebuild/tools.rs` | Static `ToolSpec`s passed to `Engine::shared(session, extra)`; reach the engine through `ctx.session.ext.get::<EngineRef>()`. Never block a tool on work that calls back into the engine: start it and return. |
| A kind of thing to make | `crates/ai/src/rebuild/kinds/` | Implement `TargetKind` (detection score, default stack, brief sections, checks, refine message, the app's words) and `KindRegistry::register` it. |
| A way to judge a build | `crates/ai/src/rebuild/kinds/` | Implement `Check` (`run(&mut CheckCx) -> CheckResult`; `applies` to skip it in some runs, or wrap it in `DeepOnly`); return it from your kind's `checks`. Decide on `passed`/`similarity`, never on text. |
| A stack | `crates/ai/src/rebuild/stacks.toml` | One `[[stack]]` entry: name, the builder's steps, the phrases that ask for it, its engine, the package.json dependencies that show a build uses it. A component or styling library is a `[[library]]` entry (name, phrases, the stacks it works with, steps). |
| A coding agent | `crates/ai/src/cli_agents/adapters.rs` | Implement `AgentAdapter` (args, required flags, stream parsing, resume, sign-in check) and register it in `AgentAdapters`. Agents it doesn't know run with the plain adapter. An agent that speaks ACP needs only a row in `cli_agents::acp::ACP_AGENTS` (its id and the arguments that start ACP mode); if its ACP mode takes the backend's model, a row in `ACP_MODEL_FLAGS` too. |
| An asset provider | `crates/engines/src/assets/` | Implement `AssetProvider` (`key_env` names its environment variable); add its models and prices to `assets.toml`. Its key is its own: `assets_key_set` with `provider`, kept as `assets:<id>`. Return a failure after the provider accepted paid work through `assets::charged` so the charge is recorded. Register it in the session's `AssetProviders` (the tools, builder prompts and app all list that registry). |
| An external engine | `crates/engines/src/table.rs` | One `ExternalEngine` entry (exes, known paths, what it's for, install pins with SHA-256 checked against the publisher), plus a `tool_paths!` id in `settings.rs` if people can point at their own install. |
| A front end for builds | anywhere above L5 | Hold a `rebuild::RunSet` (it queues past its limit) and `poll` it; test it with `RunSet::with_starter(StandIns::starter())`, whose runs report only what the test sends. |
| A persisted file | its owner module | A version field and a migration step; see [file-formats.md](file-formats.md). |
| A UI language | `crates/ui-egui/src/i18n/` and `i18n.rs` | Add a TSV catalog and a language registry entry; preserve placeholders and run the catalog tests and `cargo xtask i18n-coverage`. See [localisation](localization.md) for scope and font setup. |

## Built-in generated assets

Settings → AI → Game assets manages each provider's credentials, separately from coding-agent
credentials. The OS keychain is preferred; if unavailable, the app uses its private, owner-only
`ai-keys.toml` fallback file. That fallback is not encrypted. Use **Scenario** for images and
image-to-3D models, **ElevenLabs** for text-to-speech, sound effects and music, or the existing
**fal.ai** models. Scenario has separate, masked **API key** and **API secret** inputs; Save combines
them internally for authentication. Previously saved combined credentials keep working.
ElevenLabs needs its API key. Credentials also come from `SCENARIO_API_KEY` (the combined
`API key:API secret` value), `ELEVENLABS_API_KEY`, or
`FAL_KEY` when no key is stored. Never include real credentials in a project, prompt or committed file.

Scenario's [direct REST API](https://docs.scenario.com/get-started/documentation/quick-start-guide/step-2-authenticate-your-requests)
documents HTTP Basic authentication, which this provider uses over HTTPS. Base64 encodes the
credential pair; TLS provides transport encryption. Authenticated requests go to Scenario's API,
redirects are refused, and downloads use a separate client with no authentication header.
Scenario also offers [OAuth through its official MCP server](https://help.scenario.com/articles/2664328768-getting-started-with-the-scenario-mcp-server);
that is a separate connection path, not an authentication mode of this built-in provider.
Use a dedicated [project-scoped key with a usage limit](https://help.scenario.com/articles/2952168846-api-keys-and-scopes?lang=en)
and grant only the permissions needed for generation and asset access.

Choose the picture and 3D models in Settings → Builds. Headless settings use `[assets] image_model`
and `model_3d`; individual image/model tool calls can override their model. Existing fal.ai defaults
are preserved. Every project's sidebar has **Generated assets** controls, including app projects
whose recorded stack is not a game engine. Turn providers on or off individually there. A key
alone does not enable spending, and a disabled provider is refused by the generation tools even
when a model override selects it. Turning a provider off affects new operations; an already-running
generation operation can finish.

The same sidebar shows the shared asset budget, estimated spending and remaining amount. Edit
the budget and press **Save budget** to change this project's budget, including lowering it. The
spending tally comes from generated asset records and is separate from the coding agent's AI cost.
Provider switches and budget edits are saved with the project; disabling a provider keeps its
previous asset records and spending. Provider switches do not change the selected image/3D models.

Headless clients use `project_assets_allow` with `project`, `provider` and `allow` to control a
single provider; omit `allow` to read its status. Omitting `provider` retains the existing master
asset switch. `project_assets_budget` reads or sets the shared budget and reports spending.
While the master switch is paused, disabling a provider preserves the other remembered choices
without resuming generation. Enabling one provider from the paused state activates only that
provider; it does not silently enable other unchecked providers.

For example, these settings select Scenario for both stages and give a new project enough budget
for a model and its source picture:

```toml
[assets]
image_model = "scenario-gemini-3-1"
model_3d = "scenario-meshy"
budget_usd = 20.0
```

Existing projects keep their own budget; change one through `project_assets_budget` or its project
screen. To request speech, pass a tool call such as the following (the voice id is an example;
choose one available to your ElevenLabs account):

```json
{"text":"Welcome to the game.","voice_id":"JBFqnCBsd6RMkjVDRZzb","save_as":"Audio/welcome.mp3"}
```

The shared tool path saves assets inside the project's `app/` folder and records each asset's
estimated cost against the project budget. `assets_speech_generate` accepts text and a voice id;
`assets_sound_generate` and `assets_music_generate` accept a prompt and duration. These tools save
MP3 audio, work with ElevenLabs alone, and are available in the CLI, MCP and builder for generating
original assets.

The spending ledger is checked before authorizing generation or changing a budget. Only a missing
`assets.json` means no history; an unreadable, invalid, oversized or non-regular ledger blocks
generation and is left untouched. Ledger symlinks are rejected rather than followed or replaced.
The sidebar shows spending as unavailable until that history is repaired.

Once generation has returned a valid reported cost, its spending record is saved before the output
file is published. A failed ledger write leaves the destination untouched. A later file-save
failure keeps the spending record and any previous destination file, so a record's `file` is the
intended output location rather than proof that the new bytes were saved. A cost above the approved
estimate is recorded and reported as an overrun; a precursor overrun stops the 3D stage. Do not
blindly regenerate after these errors: check the recorded history and provider account first.

A failure after the provider accepted the paid work is recorded too. Scenario reports its quoted
charge for any failure once it accepts the job (an unreadable submit answer, a failed or unknown
job status, a polling timeout, a missing asset or address, a failed download); ElevenLabs reports the request's estimate
when the accepted response body can't be read; fal reports it when a finished generation returns
no usable output. The provider marks the error with `assets::charged`, and the tool records that
charge for the intended file before returning the error, so a retry is checked against the budget
it already used. A charge that can't be recorded is reported as such, never dropped.

This is not a reservation journal or a guarantee against process crashes or ambiguous provider
timeouts. A provider may charge a request whose result never reaches the app, or a ledger write
itself can fail. Use provider-side spending limits when a hard account-level cap is needed.

Scenario bills Compute Units, which depend on model settings and account discounts. Its table
amounts are conservative spending ceilings, not published per-generation prices: $1 per image
and $10 per 3D model. Before submitting a paid job, the provider checks Scenario's authenticated
`dryRun=api` quote and refuses missing, invalid or over-ceiling quotes. The dollar tally values a
CU at $0.01, an estimate based on the Starter plan; it is not an invoice. A Scenario 3D call therefore
needs a project budget covering the $10 ceiling (plus its source picture when generated first),
even if the eventual quote is smaller. ElevenLabs estimates use the model rates in `assets.toml`
and the requested character count or duration. Speech rates assume standard-priced voices;
Voice Library credit multipliers can increase the actual charge. The project budget tracks these
estimates, and actual provider billing can depend on your plan and selected voice.
fal FLUX estimates round up to whole decimal megapixels conservatively. Its model pages specify
rounding up, while its publishing guide uses a 1024×1024-pixel unit; the effective invoice unit
still needs a real invoice check. The app therefore uses the larger estimate rather than claiming
a measured per-image charge.

Protocol and budget behavior is tested with local stand-in providers. Scenario and ElevenLabs
generation have not been tested with live account credentials. Current API sources:
[Scenario generation](https://docs.scenario.com/api/resources/generate/methods/run_model),
[Scenario Compute Units](https://help.scenario.com/articles/7934059476-api-usage-and-credits-compute-units),
[Scenario pricing](https://www.scenario.com/pricing),
[ElevenLabs speech](https://elevenlabs.io/docs/api-reference/text-to-speech/convert),
[sound effects](https://elevenlabs.io/docs/api-reference/text-to-sound-effects/convert), and
[music](https://elevenlabs.io/docs/api-reference/music/compose).

## Plugins: external MCP servers

Any MCP server, local (stdio) or remote (Streamable HTTP), can add tools without changing code.
Add it in Settings → Plugins, with `plugins_add` (`craft-anything-cli plugins-add --id notes
--command notes-mcp --args '["--stdio"]'`), or list it in `config.toml`. A plugin added through the tool
or the app always asks before each call; `plugins_remove` takes one away.

```toml
[[plugins.mcp]]
id = "notes"            # its toolset id; its tools become notes_<tool>
name = "Notes"          # shown to people (default: the id)
command = "notes-mcp"
args = ["--stdio"]
consent = true          # ask before each call (the default): it runs someone else's code
# enabled = false       # keep it configured but off
# env = { NOTES_DIR = "C:/notes" }

[[plugins.mcp]]
id = "docs"             # a remote server: url instead of command
url = "https://mcp.example.com/mcp"   # https, or http:// to this computer
headers = { Authorization = "Bearer …" }
```

Then `craft-anything-cli plugins-refresh --allow-run` starts it once and caches its tool list; from
the next start its tools are offered everywhere, in a toolset of their own (MCP agents enable it with
`enable_toolset`). `plugins-list` shows what's configured and cached.

- A local server starts on its first call and stays up for the session; one that exits is started
  again. A remote one is reached on its first call; its session id is sent back with each request.
- Effects come from the server's tool annotations (`readOnlyHint`, `destructiveHint`,
  `openWorldHint`), plus launching a process (local) or the network (remote). With
  `consent = true` every call asks first.
- `env` and `headers` values are never shown by `settings_show` or `plugins_list`.
- Another transport implements `engines::mcp_client::Transport` and is passed to
  `McpClient::with_transport`.
- Results are recorded as evidence from an external service (`engine: plugin:<id>`).
- Tool ids are `<plugin id>_<tool name>`, lower-cased with `_` for anything else; a clash is skipped
  and logged.

## Plugins: WASM components

A build with `--features wasm-plugins` (it compiles wasmtime, so it's off by default) also loads
WASM components. A component exports `craft-anything:plugin/tools` from
[`crates/engines/wit/plugin.wit`](../crates/engines/wit/plugin.wit): `describe` returns its tools
(name, title, description, input schema) as JSON, and `call(name, input)` answers
`{"ok": …}` or `{"error": "…"}`. List it in `config.toml`:

```toml
[[plugins.wasm]]
id = "wordtools"        # its toolset id; its tools become wordtools_<tool>
path = "C:/plugins/wordtools.wasm"
```

- It may also (or instead) export `craft-anything:plugin/checks`: ways of judging a build, run
  after the built-in checks of each round for the kinds of project they name. A check gets the
  build as JSON (every file's path and size, plus the text of the extensions it asks for) and
  answers passed, an optional 0-100 similarity and a summary; a failed one is shown to the builder
  to fix. Asset providers can't be plugins: they need the network.
- It may import nothing: no files, network, environment or clock, so its tools run without asking.
- Each call runs in a fresh instance with a work limit (fuel) and 256 MiB of memory; one that runs
  past them is stopped with an error.
- A plugin that can't be loaded, or a tool whose id is invalid or taken, is skipped and logged.
- Complete plugins in Rust, with build steps: a tool,
  [`examples/wasm-plugin-wordcount`](../examples/wasm-plugin-wordcount), and a check,
  [`examples/wasm-plugin-readme-check`](../examples/wasm-plugin-readme-check).

### Plugin API versions

The plugin API is `package craft-anything:plugin@<version>` in `plugin.wit`, versioned with semver;
a plugin exports the interfaces at the version it was built against
(`craft-anything:plugin/tools@0.1.0`).

- **Compatible means the same major version, and while it's 0.x the same minor too** (0.1.0 and
  0.1.7 work together; 0.2.0 doesn't). Craft Anything loads a compatible plugin whatever its patch
  version, and refuses an incompatible one by name ("exports … for API 0.2.0; this Craft Anything
  speaks 0.1.0") instead of silently skipping it.
- **A patch version only adds optional JSON fields**: in what a plugin returns (Craft Anything
  ignores fields it doesn't know) and in what it's given (plugins must ignore unknown fields too).
- **A minor version (while 0.x) or a major one** is anything else: a new or changed function, a
  field that becomes required, a changed meaning. The host's `API_VERSION`
  (`engines::wasm_plugin`) and the WIT package move together; a test checks they agree.

Code that defines tools at run time itself (a WASM host, a proxy for another protocol) uses
`registry::RuntimeTool` with `Engine::shared_with`: owned names, a JSON schema, and a function that
serves its tools, telling them apart by `ctx.tool`.
