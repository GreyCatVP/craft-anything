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
| An asset provider | `crates/engines/src/assets/` | Implement `AssetProvider`: `id`, `name`, `unit` (what it bills in), `list_models` (the models it offers for an asset type, with their prices), `describe` (one model's inputs and price), `estimate` (what a request costs before anything is spent, if it can say), `generate`, and `key_env` for its environment variable. A provider whose `estimate` is a live quote must also override `generate_within`, so it never submits a run above the price its limit check approved. `known_models.toml` says which asset types a model makes where the provider's list doesn't, and `input_roles.toml` what a model's input names mean; describe a model's inputs as `InputField`s and let `inputs::map_request` fill them from the request. Its key is its own: `assets_key_set` with `provider`, kept as `assets:<id>`. Return a failure after the provider accepted paid work through `assets::charged` so the charge is recorded. Register it in the session's `AssetProviders` (the tools, builder prompts and app all list that registry). |
| An external engine | `crates/engines/src/table.rs` | One `ExternalEngine` entry (exes, known paths, what it's for, install pins with SHA-256 checked against the publisher), plus a `tool_paths!` id in `settings.rs` if people can point at their own install. |
| A front end for builds | anywhere above L5 | Hold a `rebuild::RunSet` (it queues past its limit) and `poll` it; test it with `RunSet::with_starter(StandIns::starter())`, whose runs report only what the test sends. |
| A persisted file | its owner module | A version field and a migration step; see [file-formats.md](file-formats.md). |
| A UI language | `crates/ui-egui/src/i18n/` and `i18n.rs` | Add a TSV catalog and a language registry entry; preserve placeholders and run the catalog tests and `cargo xtask i18n-coverage`. See [localisation](localization.md) for scope and font setup. |

## Built-in generated assets

A project makes five kinds of asset: pictures, 3D models, speech, sound effects and music. For each
kind it uses one provider and one of that provider's current models, or none (Off), and each kind
has its own limit. The built-in providers are **fal.ai**, **Scenario** and **ElevenLabs**; a fork
can register more (see the table above). Nothing is generated unless generated assets are turned on
for the project and a provider is chosen for the kind.

### Keys

Settings → Builds manages each provider's credentials, separately from coding-agent credentials.
The OS keychain is preferred; if unavailable, the app uses its private, owner-only `ai-keys.toml`
fallback file. That fallback is not encrypted. A provider is offered for a kind only when its key is
saved and it makes that kind. Scenario has separate, masked **API key** and **API secret** inputs;
Save combines them internally for authentication. Previously saved combined credentials keep
working. ElevenLabs needs its API key. Credentials also come from `SCENARIO_API_KEY` (the combined
`API key:API secret` value), `ELEVENLABS_API_KEY`, or `FAL_KEY` when no key is stored. Never include
real credentials in a project, prompt or committed file.

Scenario's [direct REST API](https://docs.scenario.com/get-started/documentation/quick-start-guide/step-2-authenticate-your-requests)
documents HTTP Basic authentication, which this provider uses over HTTPS. Base64 encodes the
credential pair; TLS provides transport encryption. Authenticated requests go to Scenario's API,
redirects are refused, and downloads use a separate client with no authentication header.
Scenario also offers [OAuth through its official MCP server](https://help.scenario.com/articles/2664328768-getting-started-with-the-scenario-mcp-server);
that is a separate connection path, not an authentication mode of this built-in provider.
Use a dedicated [project-scoped key with a usage limit](https://help.scenario.com/articles/2952168846-api-keys-and-scopes?lang=en)
and grant only the permissions needed for generation and asset access.

### Models and prices come from the providers

`assets_models_list` asks each provider whose key is saved for the models it offers for one kind, with
the price it gives for each. The lists are kept in `asset-models.json` in the config folder and used
again for ten minutes (`max_age_minutes`); a provider that can't be asked keeps its earlier list, with
the reason beside it. The pickers in Settings → Builds and in each project's panel only draw what was
fetched: they never wait for a provider. Settings → Builds asks again when it opens with lists older
than ten minutes, and its **Refresh** button asks every provider at once.

A model is offered only when the app can fill every input it requires from its request (a prompt, a
picture, a shape, a number of faces, a duration, a voice…). `input_roles.toml` says which input names
mean which of those, for fal.ai's per-model OpenAPI schemas and Scenario's per-model `inputs`. When a
model has no input for an option the tools take (a wide or tall shape, a face limit, a length, an
instrumental switch, a voice), the picker says so beside the model ("ignores: shape") and the tool's
result lists the option as ignored.

The providers' own lists tell the kind of most models. Where they don't (audio models, which fal.ai
and Scenario list together, and the two fixed ElevenLabs endpoints), `known_models.toml` says which
kinds a model makes. It is also where the app keeps the one price it has to: the credits a second of
ElevenLabs sound effects or music costs, because ElevenLabs publishes no rate for those two
endpoints in its API. Every other price is the provider's.

Each project stores its choice for a kind: the provider, the model, its unit and the price the list
gave. Settings → Builds holds the defaults a new project copies; changing them later does not change
an existing project. Choosing another provider for a kind (Off counts as one) starts that kind's
count afresh in the new provider's unit; another model of the same provider keeps the count. The
builder does not choose models: the project's choice for each kind is the only model used, and the
generation tools take no `model` parameter. A 3D model made from a description first makes a picture
with the project's picture model, which counts against the limit for pictures; with pictures Off, it
is made only by a 3D model that takes words directly.

### Units, limits and Ignore budget

Each provider bills in its own unit: **US dollars** for fal.ai, **credits** for Scenario and
ElevenLabs. Credits are never converted to dollars, and amounts of different units are never added.
Usage is counted for each kind, in its provider's unit, from the day that provider was chosen.

Each kind has its own limit in that unit and its own switch, even when two kinds use the same
provider. A new project starts with $5 for kinds billed in dollars and 500 credits for kinds billed
in credits; Settings → Builds changes these defaults (one default per unit), and a kind that
switches to a provider with another unit takes that unit's default, never the old number. The most a
limit can be is $10,000 or 10,000,000 credits. The project's **Ignore budget** switch turns every
limit off at once; usage is still counted and shown, and Settings has a default for new projects.
Every project's sidebar has a **Generated assets** table with a row for each kind, including app
projects whose recorded stack is not a game engine. This spending is separate from the coding
agent's AI cost.

What a limit can do depends on when the price is known:

- **Known before the run** (a price per picture, megapixel, second or character, or a quote the
  provider gives before each run): the limit is checked against the estimate first. A generation that
  would pass it asks the person instead.
- **Known only after the run** (a model whose price per run isn't published): generations start
  while usage is under the limit, and the last one may go over it. The project's panel says so.
- **Unknown** (the provider gives no usable price): cost tracking is off for that kind while the model
  is chosen, and the screen says "price unknown, usage not tracked". Generations are still counted and
  listed, without an amount.

At a limit the builder stops to ask: the person can allow one more generation, or choose **Always**,
which switches that kind's limit off where they can see it and switch it back on. In Auto mode the
builder stops instead of asking. A generation whose cost was not read leaves usage unknown, so later
ones ask until the person allows them.

### Using it without the app

`project_assets_choose` sets a kind's provider and model (provider null for Off), `project_assets_limit`
sets its limit and switch, `project_assets_allow` turns generation on or off and sets `ignore_budget`
(and reads every kind's provider, model, unit, limit, usage and whether the limit is enforced), and
`assets_models_list` lists what a provider offers. Their results use the same field names. The
asset tools (`assets_image_generate`, `assets_model_generate`, `assets_speech_generate`,
`assets_sound_generate`, `assets_music_generate`) use the project's choice for their kind.

Headless defaults are in `config.toml` (the models are written by Settings → Builds, which has the
lists to pick from):

```toml
[assets]
usd_limit = 5.0        # default limit of a new project's kinds billed in dollars
credit_limit = 500.0   # ... and of those billed in credits
ignore_budget = false  # whether a new project starts with every limit off
# [assets.defaults.image], [assets.defaults.model_3d], ... one table per kind: provider, model, unit, price
```

When an AI calls (the builder, or an MCP client), anything that loosens spending needs the person's
answer on an approval card in the project's inbox, and nothing is changed without it: turning
generation on, **Ignore budget** on, a limit raised or switched off, or a model chosen for a kind.
Changes that only tighten (a lower limit, a limit switched on, Off) apply directly. Approval cards can be answered only
from the app or the CLI, never by an AI. `settings_set` refuses an AI for the `assets.` settings. The
CLI treats itself as an AI when the `CRAFT_ANYTHING_AGENT` variable is set, which the builder's
shell commands inherit, and then ignores `--allow-run`. That protects against a builder acting by
accident or by mistake; a determined one could unset the variable or edit the project's files
directly.

To request speech, pass a tool call such as the following (the voice id is an example; choose one
available to your ElevenLabs account):

```json
{"text":"Welcome to the game.","voice_id":"JBFqnCBsd6RMkjVDRZzb","save_as":"Audio/welcome.mp3"}
```

The shared tool path saves assets inside the project's `app/` folder and records each generation in
the project's spending ledger with the amount in the provider's unit. `assets_speech_generate`
accepts text and a voice id; `assets_sound_generate` and `assets_music_generate` accept a prompt and
duration. These tools save MP3 audio and are available in the CLI, MCP and builder for generating
original assets.

### The spending ledger

The ledger is checked before authorizing generation. Only a missing `assets.json` means no history;
an unreadable, invalid, oversized, non-regular or newer-version ledger blocks generation and is left
untouched. Ledger symlinks are rejected rather than followed or replaced. The project's panel shows
usage as unreadable until that history is repaired, and the limits, switches and choices can still be
changed meanwhile.

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
charge for the intended file before returning the error, so a retry is checked against the limit
it already used. A charge that can't be recorded is reported as such, never dropped.

One project's paid generations run one at a time, across processes too: each holds the project's
`assets.lock` (an operating-system file lock, released when its process exits) from the limit
check until its receipts are written, so a second process never admits spending against a stale
total or writes the ledger at the same time. A generation that can't take the lock within 30
seconds stops with "Another asset generation for this project is still running". Generated files
never land on Craft Anything's own records, named directly or through a link, which matters where
the app is the project folder itself.

This is not a reservation journal or a guarantee against process crashes or ambiguous provider
timeouts. A provider may charge a request whose result never reaches the app, or a ledger write
itself can fail. Use provider-side spending limits when a hard account-level cap is needed.

### How each provider is priced

- **fal.ai** lists its models without a key (a key avoids its rate limit) and prices them with a key.
  A price per image, video or request, or per megapixel, is known before the run. For any other unit
  (a GPU second, say) how much a request uses is the model's own, so its cost is known only after
  the run, from the billable units fal.ai reports with the result times the unit price. A model priced
  before the run records its estimate when fal.ai reports no billable units. Megapixel estimates
  round up to whole decimal megapixels conservatively. Its model pages specify rounding up, while its
  publishing guide uses a 1024×1024-pixel unit; the effective invoice unit still needs a real invoice
  check.
- **Scenario** bills Compute Units (credits), which depend on model settings and account discounts.
  Every run is quoted by its authenticated `dryRun=api` call before a paid job is submitted, and the
  limit is checked against that quote. The listing shows a typical price from the model's published
  performance statistics, or "quoted before each run". A provider whose estimate is a live quote must
  override `generate_within` so it never submits above the price the limit approved: Scenario asks
  for a fresh quote just before submitting and refuses, with nothing spent, one that has risen.
  It refuses missing or invalid quotes. The credits the job reports it used are what is recorded.
- **ElevenLabs** lists speech models; their credit cost per character is the model's own multiplier.
  Speech estimates assume standard-priced voices; Voice Library credit multipliers can increase the
  actual charge. Sound effects and music use the credit rates in `known_models.toml`: 40 credits a
  second for sound effects and 15 credits a second for music. The music rate is ElevenLabs'
  approximate figure from its pricing FAQ, and its pricing page gives the sound-effect rate for
  sounds up to 22 seconds while the API allows 30, so the estimate for a longer sound is unverified.
  ElevenLabs is retiring both endpoints (`eleven_text_to_sound_v2` on 2027-03-01 and `music_v1` on
  2027-01-31) and documents no credit rate for their replacements yet, so those are added to the table
  when it does.

Protocol and limit behavior is tested with local stand-in providers. No live account of any provider
was used, so no price, quote or invoice here has been checked against a real charge. Current API
sources:
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
