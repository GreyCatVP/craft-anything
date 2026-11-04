# File formats

Every file Craft Anything saves carries a version number. Reading a file runs the same three steps:
parse it into a raw document (a JSON value or a TOML table), run the migration steps from the file's
version up to the current one, then turn the document into Rust types. The step runner is
`crates/engine/src/versioned.rs`.

| File | Owner | Current version | Older files | Newer files |
|---|---|---|---|---|
| `<project>/craft.json` | `engine::workspace` (`Manifest`) | `MANIFEST_VERSION` = 4 (`version`; missing = 0; 3 added the optional `asset_providers` list; 4 moved the asset choices into `assets`, one provider, model and limit per asset type) | brought forward, saved as current | opened read-only: saving is refused |
| `<project>/assets.json` | `engine::workspace` (`AssetRecord`; written by the asset tools under the project's spending lock) | `LEDGER_VERSION` = 1 (`{version, records}`; a bare list = 0, where each record had only a dollar `cost_usd`) | read as before (its records count as dollars of an unknown provider), saved as current at the next receipt | refused: no spending is counted or recorded, and the file is never written over |
| `<project>/inbox/build-progress.json` | `engine::workspace::progress` (`BuildProgress`; written only by the rebuild run) | `BUILD_PROGRESS_VERSION` = 1 (`version`; missing = 0) | brought forward; an absent file starts with no counted turns | the build stops and the record is left unchanged |
| `config.toml` (`settings::config_dir()`) | `engine::settings` (`Settings`) | `SETTINGS_VERSION` = 5 (`version`; missing = 0; 2 moved `[engines.ghidra] enabled` to `deep_mode`; 3 added more project folders and the answer about a picked folder's files; 4 added `[assets] model_3d`; 5 chose generated assets per type: `[assets] budget_usd` became `usd_limit`, next to `credit_limit` and `ignore_budget`, and `image_model` and `model_3d` became `[assets.defaults.image]` and `[assets.defaults.model_3d]`, each a provider, a model, its unit and its price; an old id the asset table never had is dropped, which leaves that type Off) | brought forward | loaded, never saved over |
| `asset-models.json` (`settings::config_dir()`) | `ai::asset_models` (the model lists `assets_models_list` fetched; written only by its `list_models`, which the app's pickers call too, each listing saving its own lists into what is on disk) | `CACHE_VERSION` = 1 (`{version, lists}`; each list is kept under `<provider>/<kind>` as `{fetched_ms, models, error}` and, after a failed refresh, `failed_ms`) | brought forward (no earlier version exists); a file that isn't JSON or has no `version` holds nothing and is replaced | read as far as its fields are known, never written over |
| `<name>.cany/project.toml` | `engine::project` (`ProjectMeta`) | `FORMAT_VERSION` = 1 (`format_version`, required) | brought forward | refused, with "update Craft Anything" |
| `<project>/transcript.json` | `ai::context` (the API builder's conversation) | `TRANSCRIPT_VERSION` = 1 (`{version, history}`) | read as before | not resumed; a fresh build replaces it |
| `<project>/app/mechanics.json` | `engine::mechanics` (written by the builder, read by `MechanicsCheck`) | `MECHANICS_VERSION` = 1 (`version`; missing = 1) | — | the check fails and says so |
| `<project>/checks/round-N/self-test.json` | `engine::self_test` (written by the built game's `--self-test`, read by `SelfTestCheck`) | 1 (`version`; missing = 1) | — | the check fails and says so |
| `<project>/usage.json` | `engine::workspace` (`ProjectUsage`; written only by the rebuild run) | `USAGE_VERSION` = 1 (`version`; missing = 0) | read as before, saved as current | read as far as its fields are known, never written over |
| `<project>/questions.json` | `engine::questions` (`AskedQuestion`, written by `rebuild_question_ask`) | `QUESTIONS_VERSION` = 1 (`{version, items}`) | read as before | read as far as its fields are known |
| `<project>/pinned/<n>/pin.json` | `engine::pins` (`Pin`) | `PIN_VERSION` = 2 (`version`; missing = 1; 2 records the selected program for a copied direct launch) | brought forward | read as far as its fields are known |
| `<folder>/standin.json` | `engine::tools::standin_tools` (`analysis::standin::StandIn`; written only by `web_standin_build`) | `STANDIN_VERSION` = 1 (`version`, required) | brought forward | refused, with "update Craft Anything", and never written over |
| `<project>/feed.json` | `ai::rebuild::feed` (written by the run) | `FEED_VERSION` = 3 (`{version, items}`; a bare list = 0) | read as before, saved as current | never written over; the conversation is rebuilt from the project for display |

A saved `User` message with pictures is `{text, pictures: [paths]}`, where the pictures are the
copies in the project's `attachments/`; one without pictures stays the plain string earlier
versions wrote.

Other JSON in a project (`inbox/` except `inbox/build-progress.json`, `checks/`, evidence records)
has no version yet; add one the first time its shape changes. `assets.lock` is an
empty file that only carries the operating system's lock on the project's spending; it holds no data.

`craft.json` version 4 keeps the generated-asset choices in an `assets` object. `types` holds, for
each of `image`, `model_3d`, `speech`, `sound_effect` and `music` that the person (or Settings, for
a new project) has chosen: the `choice` (provider, model, the unit the provider bills in and the
model's price), the `limit` in that unit, `limit_on`, and `since_ms`, when the provider was chosen
(usage counts from then; 0 is from the start of the project). `ignore_budget` turns every limit off
at once. A new project gets all five types from Settings' defaults and keeps them when Settings
change later. Choosing another provider for a type (Off counts as one) or another unit starts its
count afresh and puts its limit back to Settings' default for the unit, switched on; another model
at the same provider changes none of that.

Version 3's `asset_budget_usd` and `asset_providers` moved into `assets` as `legacy_budget_usd` and
`legacy_providers`. They are written only for a project that had them in version 3: a new project
stores all five types instead, and no tool changes them. A type the project has not chosen yet takes
Settings' default for it until this version first opens the project to work on it, which stores that
default for each such type and drops the two old fields; it is Off when `legacy_providers` is a list
that leaves that provider out (missing or null: every provider, as in version 3), with
`legacy_budget_usd` as the limit of a type billed in dollars. The master switch `generate_assets`
still needs to be on. Choices, limits and "Ignore budget" are person-owned fields: builder saves
preserve the values on disk, and duplicating a project retains them. An older build opens a version 4
project read-only.

`assets.json` version 1 is `{"version": 1, "records": [...]}`. Each record is one paid generation:
`file`, `kind` (`image`, `model`, `speech`, `sound` or `music`), `prompt`, the `provider` and `model`
that made it, the `amount` the provider reported in `unit` (`usd` or `credits`), `cost_missing` and
`at_ms`. A generation whose provider gave no cost has no `amount` and `cost_missing` set, and the
type's usage is then a lower bound. Credits are never converted to dollars. A type's usage
adds up the records of its kind at its current provider and unit with `at_ms` not before the type's
`since_ms`. Version 0, a bare list with a dollar `cost_usd` and no provider, is read as an `amount`
in dollars with an empty provider and model; such records count only for a type billed in dollars
and counted from the start of the project (`since_ms` 0). A ledger that can't be read, has a charge
that isn't a non-negative number with its unit, or is of a newer version is never treated as empty:
spending stops until it is repaired or Craft Anything is updated.

`asset-models.json` is only a cache of what each provider whose key is saved last listed for an asset
type, written with the one atomic writer (`platform::fs::write_atomic`). A list is used again while it
is younger than `max_age_minutes` (default 10) and has no `error`. A provider whose list is empty
with no `error` does not make that type: it is left out of the result and nothing is kept of it. When
asking a provider fails, the list from before is kept as it was, with its own `fetched_ms`, and
`error` says why; `failed_ms` (omitted from a good list) is when that attempt was made, and for a
minute after it a look that is not a refresh shows that list and error without asking again. With
nothing kept, the list is whatever the provider gave, with the `error`. A cancelled call keeps what
it had fetched. A newer file's lists are used and the file is left alone, as are files too large to
read (32 MiB).

## Stand-in cached responses

Version 1 stand-in files retain a recorded 304 when no usable cached body was saved. Those samples
remain in the file for diagnostics, but replay skips them within the best matching path, query and
operation group. If that group has no usable answer, it reports `cached_body_unavailable` instead
of sending a bare 304 to a fresh client. A usable cached body is still stored and replayed as a 200
with `was_not_modified: true`. This interpretation also applies to existing v1 files; the persisted
shape and version are unchanged.

## Newer files

A file from a newer Craft Anything is never downgraded or overwritten, because the fields we don't
know would be lost. `craft.json` and `config.toml` are still read as far as their fields are known,
so the person can see their project or keep their settings; every save returns a `ToolError` that
says to update Craft Anything. A newer `craft.json` whose fields can't be read at all is refused the
same way, and a newer `config.toml` that can't be read gives defaults without touching the file.
A newer `.cany` is refused outright: its evidence layout may have changed in ways we can't
see.

A file that can't be parsed is not "newer": `config.toml` is set aside as `config.toml.broken` and
defaults are used; an unreadable `craft.json` gives a plain-English error when opened and is left
out of the project list.

## Adding a migration step

1. Bump the version constant (`MANIFEST_VERSION`, `LEDGER_VERSION`, `SETTINGS_VERSION` or `FORMAT_VERSION`).
2. Append one function to the matching steps list (`MANIFEST_STEPS`, `LEDGER_STEPS`, `SETTINGS_STEPS`,
   `FORMAT_STEPS`). It takes the raw document from version n to n + 1: rename, move or fill keys.
   Return `Err` with a plain-English reason when it can't. A compile-time assertion fails if the
   list and the constant disagree.
3. Add a test that reads a literal sample of the old file (keep the existing samples; old files
   must keep loading forever).
4. For `craft.json`, `inbox/build-progress.json`, `mechanics.json` and `self-test.json`, run `cargo xtask schemas` and commit `docs/schemas/`.
   `cargo xtask schemas --check` (and an engine test) fail when the published schema is stale.
