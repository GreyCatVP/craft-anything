//! Projects (what the person is making) as tools, so every client — the app, the CLI, an MCP
//! agent — can list them, read one with its conversation, and answer its questions without the
//! app. Ids follow docs/tool-ids.md.

use crate::asset_choice::AssetChoice;
use crate::feed;
use crate::inbox::{Answer, Inbox};
use crate::settings::AssetSettings;
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::tools::asset_tools::{limit_applies, round, unit_words};
use crate::workspace::{AssetRecord, MAX_ASSET_BUDGET_USD, Workspace, type_usage};
use craft_anything_engines::assets::model::{MAX_ASSET_CREDITS, Pricing, Rate, Unit};
use craft_anything_engines::assets::{AssetKind, AssetProviders};
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::sync::Mutex;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

/// Most feed items one call returns.
const MAX_ITEMS: usize = 200;

/// Expose project management tools with their parameter schemas and declared side effects.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: "project_list",
            title: "List projects",
            summary: "List the projects in every project folder from settings (where new projects go, then `project_folders`), or in one folder, newest first: name, folder, app folder, request, target, stack, status, the latest check and how many questions are waiting.",
            beginner: "Shows everything you're making.",
            toolset: Toolset::Projects,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ListParams>,
            run: project_list,
        },
        ToolSpec {
            id: "project_show",
            title: "Show a project",
            summary: "One project: its request, target, stack, status, checks, questions still waiting for an answer, and its conversation (paged with offset/limit, newest last).",
            beginner: "Shows one project and what happened in it.",
            toolset: Toolset::Projects,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ShowParams>,
            run: project_show,
        },
        ToolSpec {
            id: "project_inbox_answer",
            title: "Answer a project's question",
            summary: "Answer a question or approval a project is waiting on (an id from project_show): a choice or text for questions, allow (and always) for approvals. The answer reaches a running build at once and is kept in the project's conversation.",
            beginner: "Answers a question the builder asked you.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM.union(Effects::CHANGES_UI_STATE),
            requires: Requires::Nothing,
            input_schema: schema_of::<AnswerParams>,
            run: project_inbox_answer,
        },
        ToolSpec {
            id: "project_play",
            title: "Play a project's result",
            summary: "Start the finished game or app a project built (its program, a Unity or Unreal package, or a Godot project in Godot), in its own window. If it's already running, says so instead of starting a second copy, unless `another` is true. A start that quits within two seconds is reported with its exit code and last output. It runs the code the builder wrote, so it needs the person's consent. Websites use project_preview instead. Known limit on Windows: when the caller's output is a pipe, the started program inherits it, so a caller reading that output to its end waits until the program exits (#207).",
            beginner: "Opens the game or app that was made.",
            toolset: Toolset::Projects,
            effects: Effects::LAUNCHES_PROCESS.union(Effects::EXECUTES_FOREIGN_CODE),
            requires: Requires::Nothing,
            input_schema: schema_of::<PlayParams>,
            run: project_play,
        },
        ToolSpec {
            id: "project_preview",
            title: "Preview a project's website",
            summary: "Serve a project's built website on this computer only (loopback) and return its address. It keeps serving for this session; with `wait`, the call itself serves until it's cancelled (Ctrl+C in the CLI).",
            beginner: "Shows the website that was made.",
            toolset: Toolset::Projects,
            effects: Effects::MUTATES_SESSION,
            requires: Requires::Nothing,
            input_schema: schema_of::<PreviewParams>,
            run: project_preview,
        },
        ToolSpec {
            id: "project_open_folder",
            title: "Open a project's folder",
            summary: "Open the project's folder (or its app/ folder) in the file manager, or one of its pinned builds' folders.",
            beginner: "Opens the project's files.",
            toolset: Toolset::Projects,
            effects: Effects::LAUNCHES_PROCESS,
            requires: Requires::Nothing,
            input_schema: schema_of::<FolderParams>,
            run: project_open_folder,
        },
        ToolSpec {
            id: "project_rename",
            title: "Rename a project",
            summary: "Give a project a new name (its folder stays where it is).",
            beginner: "Renames something you're making.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<RenameParams>,
            run: project_rename,
        },
        ToolSpec {
            id: "project_duplicate",
            title: "Duplicate a project",
            summary: "Copy a project beside it under a new name, to try something else from where it is: what was built, the pictures of the original and the study come along; the conversation, questions and usage don't, and neither do dependencies or compiler output (the next build makes them again). An app edited in place is copied into the copy's app/ folder.",
            beginner: "Makes a copy to try something different.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<DuplicateParams>,
            run: project_duplicate,
        },
        ToolSpec {
            id: "project_delete",
            title: "Delete a project",
            summary: "Delete a project: everything Craft Anything made in its folder (its records, checks, and what was built in app/). The person's own files stay (an app edited in place, the files a project started from). Can't be undone, so `confirm` must be true; stop a build first.",
            beginner: "Deletes something you made. Your own files are kept.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<DeleteParams>,
            run: project_delete,
        },
        ToolSpec {
            id: "project_questions_set",
            title: "Choose what the builder's questions get",
            summary: "Choose whether the builder's questions (core_person_ask) in a project are shown to the person to answer (`ask`) or answered with \"decide yourself and say what you chose\" (`decide`); `default` follows the app's setting (`questions` in settings, which follows Auto mode until set). With no mode, says what applies now. Takes effect from the builder's next question, also mid-build.",
            beginner: "Chooses whether the builder asks you or decides by itself.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<QuestionsParams>,
            run: project_questions_set,
        },
        ToolSpec {
            id: "project_build_finish",
            title: "Call the build done",
            summary: "The person is happy with the build as it is (#210): its status becomes Ready and the conversation says they called it done, so a paused, stopped or failed build no longer offers to carry on. Everything so far is kept, and a new message carries on from here. It doesn't stop a build that is still running: pause it first.",
            beginner: "Keeps the build as it is and stops offering to carry on.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<FinishParams>,
            run: project_build_finish,
        },
        ToolSpec {
            id: "project_assets_budget",
            title: "Set a project's asset budget",
            summary: "Set the shared budget for a project's generated images, 3D models and audio (US dollars), or read it with no amount. Shows estimated spending and the remaining budget.",
            beginner: "Changes how much a project may spend on generated art.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<BudgetParams>,
            run: project_assets_budget,
        },
        ToolSpec {
            id: "project_assets_allow",
            title: "Let a project generate assets",
            summary: "Turn asset generation on or off for a project (allow), and the project-wide Ignore budget switch (ignore_budget), which switches every limit off at once while usage is still counted. Omit both to only read them. Pictures, 3D models and audio need their provider's key and a provider chosen for the type (project_assets_choose). Off until the person turns it on. The result's `types` lists every asset type (image, model_3d, speech, sound_effect, music) with its provider, model, model_name, unit (usd or credits), limit and limit_on (the number set and the type's own switch), enforced (whether the limit is applied: its switch on, the budget not ignored and a price to count with), tracked (whether the model's price lets usage be counted as an amount), and used, generations and cost_missing (what the type has used since its provider was chosen; used is a lower bound while cost_missing). project_assets_choose and project_assets_limit return the same `types`.",
            beginner: "Chooses whether your project may generate pictures, 3D models and audio, and whether its limits apply.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<AllowAssetsParams>,
            run: project_assets_allow,
        },
        ToolSpec {
            id: "project_assets_choose",
            title: "Choose how a project makes one kind of asset",
            summary: "Choose the provider and model a project uses for one asset type (kind: image, model_3d, speech, sound_effect or music), or provider null for Off: nothing of that type is generated. `provider` is required (null is Off, leaving it out is refused) and must be a registered provider id; `unit` (default: the provider's own) must be the unit it bills in, usd for fal.ai and credits for Scenario and ElevenLabs. Give `model` as its id at the provider, `model_name` as it is shown to the person (default: the id) and `pricing` as the model list gave it (left out: price unknown, so usage is counted without an amount and no limit applies). The same provider and unit keep the type's limit, its switch and its count; another provider or unit starts the count afresh with the default limit in the new unit, switched on. Returns the same `types` as project_assets_allow.",
            beginner: "Chooses which service and model make your project's pictures, 3D models, voices, sounds or music.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<ChooseAssetParams>,
            run: project_assets_choose,
        },
        ToolSpec {
            id: "project_assets_limit",
            title: "Set a project's limit for one kind of asset",
            summary: "Set what a project may use of one asset type before the person is asked (limit, in the unit of the type's provider: 0 to 10 000 dollars, or 0 to 10 000 000 credits) and/or switch that limit on or off (enabled). Usage is counted either way. Omit both to only read. A limit outside its range is refused and nothing changes. The project's Ignore budget switch (project_assets_allow) overrides every limit. Returns the same `types` as project_assets_allow.",
            beginner: "Changes how much your project may use for one kind of generated asset.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<LimitAssetParams>,
            run: project_assets_limit,
        },
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Its new name.
    pub name: String,
}

fn project_rename(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: RenameParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    ws.rename(&p.name)?;
    Ok(ToolOutput::Value(json!({"project": ws.root, "name": ws.manifest.name})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The copy's name (default: "<name> copy").
    #[serde(default)]
    pub name: Option<String>,
}

fn project_duplicate(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DuplicateParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let name = p.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("{} copy", ws.manifest.name));
    let copy = ws.duplicate(&name)?;
    Ok(ToolOutput::Value(json!({"project": copy.root, "name": copy.manifest.name})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Must be true: deleting can't be undone.
    #[serde(default)]
    pub confirm: bool,
}

fn project_delete(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DeleteParams = parse_params(params)?;
    if !p.confirm {
        return Err(ToolError::invalid("Deleting a project can't be undone.").with_remediation("Pass confirm: true to delete it."));
    }
    let ws = open(&p.project)?;
    let kept = ws.manifest.app_in_root || !ws.manifest.start_files.is_empty();
    ws.delete()?;
    Ok(ToolOutput::Value(json!({"deleted": p.project, "your_files_kept": kept})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BudgetParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The new budget in US dollars (0 to 10 000); leave out to only read it.
    #[serde(default)]
    pub budget_usd: Option<f64>,
}

/// Read or persist a bounded asset budget and report spending with a nonnegative remaining balance.
///
/// Temporary: it keeps the one budget a project had (`assets.legacy_budget_usd`) until Task 14 of the
/// per-type assets plan replaces it with `project_assets_limit` and the panel's limit fields.
fn project_assets_budget(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: BudgetParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    let spent = ws.try_assets_spent()?;
    if let Some(b) = p.budget_usd {
        if !b.is_finite() || !(0.0..=crate::workspace::MAX_ASSET_BUDGET_USD).contains(&b) {
            return Err(ToolError::invalid(format!("budget_usd must be between 0 and {:.0}.", crate::workspace::MAX_ASSET_BUDGET_USD)));
        }
        ws.set_person(|m| m.assets.legacy_budget_usd = Some(b))?;
    }
    Ok(ToolOutput::Value(json!({"budget_usd": ws.manifest.legacy_budget_usd(), "spent_usd": spent,
        "remaining_usd": (ws.manifest.legacy_budget_usd() - spent).max(0.0)})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowAssetsParams {
    /// The project's folder.
    pub project: PathBuf,
    /// true lets the builder generate assets, false stops it; leave out to leave it as it is.
    #[serde(default)]
    pub allow: Option<bool>,
    /// true switches every limit in the project off at once (usage is still counted), false puts
    /// them back to each type's own switch; leave out to leave it as it is.
    #[serde(default)]
    pub ignore_budget: Option<bool>,
    /// Older per-provider switch: with `allow`, enables or disables one registered provider among
    /// those the project allowed before each asset type had its own provider (project_assets_choose
    /// sets that now). Omit it to use the master switch, which keeps those choices.
    #[serde(default)]
    pub provider: Option<String>,
}

/// Read or update the master switch and "Ignore budget" in one save from the latest manifest, and
/// report every asset type. A damaged ledger is an error before anything changes: spending is never
/// shown as none.
///
/// Temporary: `provider` keeps editing the project's old provider list (`assets.legacy_providers`)
/// until the project panel chooses providers per type (Task 14 of the per-type assets plan), which
/// removes it with the `asset_providers` and `providers` results.
fn project_assets_allow(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AllowAssetsParams = parse_params(params)?;
    let providers = ctx.session.ext.get::<AssetProviders>().unwrap_or_default();
    if let Some(id) = &p.provider
        && providers.get(id).is_none()
    {
        return Err(ToolError::invalid(format!("No asset provider called \"{id}\" is available.")));
    }
    let settings = ctx.session.settings.assets.clone();
    let mut ws = open(&p.project)?;
    let records = ws.try_assets()?;
    if p.allow.is_some() || p.ignore_budget.is_some() {
        ws.set_person(|m| {
            if let Some(allow) = p.allow {
                if let Some(id) = &p.provider {
                    // Work from the freshly merged manifest, including changes made during a build.
                    let was_enabled = m.generate_assets;
                    let mut enabled = if was_enabled || !allow {
                        m.assets.legacy_providers.clone().unwrap_or_else(|| providers.ids().into_iter().map(str::to_owned).collect())
                    } else {
                        Vec::new()
                    };
                    enabled.retain(|other| other != id);
                    if allow {
                        enabled.push(id.clone());
                    }
                    m.generate_assets = (was_enabled || allow) && !enabled.is_empty();
                    m.assets.legacy_providers = Some(enabled);
                } else {
                    m.generate_assets = allow;
                }
            }
            if let Some(ignore) = p.ignore_budget {
                m.assets.ignore_budget = ignore;
            }
        })?;
    }
    let statuses: Vec<_> = providers.ids().into_iter().map(|id| json!({"id": id, "enabled": ws.manifest.asset_provider_allowed(id)})).collect();
    let mut report = assets_report(&ws, &settings, &records);
    if let Some(o) = report.as_object_mut() {
        o.insert("asset_providers".into(), json!(ws.manifest.assets.legacy_providers));
        o.insert("providers".into(), json!(statuses));
    }
    Ok(ToolOutput::Value(report))
}

/// A field that must be present but may be null (`provider`: null is Off). Serde reads a missing
/// `Option` as `None`, which would turn a forgotten provider into a choice of Off.
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// The schema of [`nullable`]'s field: a string or null, and (with `required`) always present.
fn nullable_string(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": ["string", "null"]})
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChooseAssetParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The asset type: image, model_3d, speech, sound_effect or music.
    pub kind: AssetKind,
    /// The provider's id (fal, scenario, elevenlabs, or another registered one); null: Off, nothing
    /// of this type is generated. Required: leaving it out is refused.
    #[serde(deserialize_with = "nullable")]
    #[schemars(required, schema_with = "nullable_string")]
    pub provider: Option<String>,
    /// The model's id at the provider. Required with a provider.
    #[serde(default)]
    pub model: Option<String>,
    /// The model's name as shown to the person (default: its id).
    #[serde(default)]
    pub model_name: Option<String>,
    /// What the provider bills in: usd or credits. Default: the provider's own; another is refused.
    #[serde(default)]
    pub unit: Option<Unit>,
    /// What the model costs, as the model list gave it (default: price unknown).
    #[serde(default)]
    pub pricing: Option<Pricing>,
}

/// The longest model id or name a project keeps.
const MAX_MODEL_CHARS: usize = 200;

/// The most a price per unit can be before it isn't believed: the same ceiling as a limit.
fn ceiling(unit: Unit) -> f64 {
    match unit {
        Unit::Usd => MAX_ASSET_BUDGET_USD,
        Unit::Credits => MAX_ASSET_CREDITS,
    }
}

/// A price a client gives is in the unit the type is counted in and a number worth counting with.
fn check_pricing(pricing: &Pricing, unit: Unit, provider: &str) -> Result<(), ToolError> {
    let (priced_in, typical) = match pricing {
        Pricing::Unknown => return Ok(()),
        Pricing::After { unit } => (*unit, None),
        Pricing::Before { unit, rate } => match rate {
            Rate::PerRequest(x) | Rate::PerMegapixel(x) | Rate::PerSecond(x) | Rate::PerCharacter(x) => (*unit, Some(*x)),
            Rate::Quoted { typical } => (*unit, *typical),
        },
    };
    if priced_in != unit {
        return Err(ToolError::invalid(format!("The price is in {}, but {provider} bills in {}.", unit_words(priced_in), unit_words(unit)))
            .with_remediation("Give the price the model list showed, or leave pricing out."));
    }
    let max = ceiling(unit);
    if typical.is_some_and(|x| !x.is_finite() || !(0.0..=max).contains(&x)) {
        return Err(ToolError::invalid(format!("A price must be a number from 0 to {max:.0} {}.", unit_words(unit)))
            .with_remediation("Give the price the model list showed, or leave pricing out."));
    }
    Ok(())
}

/// What `p` asks the project to use for its type, or why it can't: Off has nothing else to say, and
/// a provider must be registered, bill in the unit given (default: its own), and come with a model
/// and a price that fit that unit.
fn asset_choice(providers: &AssetProviders, p: &ChooseAssetParams) -> Result<AssetChoice, ToolError> {
    let Some(id) = &p.provider else {
        if p.model.is_some() || p.model_name.is_some() || p.unit.is_some() || p.pricing.is_some() {
            return Err(ToolError::invalid("An Off choice has no model, name, unit or price.")
                .with_remediation("Leave model, model_name, unit and pricing out when provider is null."));
        }
        return Ok(AssetChoice::off());
    };
    let provider = providers.get(id).ok_or_else(|| {
        ToolError::invalid(format!("No asset provider called \"{id}\" is available."))
            .with_remediation(format!("Choose one of {}, or null for Off.", providers.ids().join(", ")))
    })?;
    let too_long = |s: &str| s.chars().count() > MAX_MODEL_CHARS;
    let model = p
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty() && !too_long(m))
        .ok_or_else(|| ToolError::invalid(format!("model must be the model's id at {}, up to {MAX_MODEL_CHARS} characters.", provider.name())))?;
    let model_name = match p.model_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) if too_long(name) => return Err(ToolError::invalid(format!("model_name must be up to {MAX_MODEL_CHARS} characters."))),
        Some(name) => name,
        None => model,
    };
    let unit = p.unit.unwrap_or_else(|| provider.unit());
    if unit != provider.unit() {
        return Err(ToolError::invalid(format!("{} bills in {}, not {}.", provider.name(), unit_words(provider.unit()), unit_words(unit)))
            .with_remediation("Leave unit out to use the provider's own."));
    }
    let pricing = p.pricing.clone().unwrap_or(Pricing::Unknown);
    check_pricing(&pricing, unit, provider.name())?;
    Ok(AssetChoice { provider: Some(id.clone()), model: model.to_owned(), model_name: model_name.to_owned(), unit, pricing })
}

/// Change the provider and model a project makes one asset type with, and report every type. A
/// refused choice or a damaged ledger is an error before anything changes.
fn project_assets_choose(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ChooseAssetParams = parse_params(params)?;
    let providers = ctx.session.ext.get::<AssetProviders>().unwrap_or_default();
    let choice = asset_choice(&providers, &p)?;
    let settings = ctx.session.settings.assets.clone();
    let mut ws = open(&p.project)?;
    let records = ws.try_assets()?;
    ws.set_person(|m| m.assets.choose(p.kind, choice, &settings, crate::inbox::now_ms()))?;
    Ok(ToolOutput::Value(assets_report(&ws, &settings, &records)))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LimitAssetParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The asset type: image, model_3d, speech, sound_effect or music.
    pub kind: AssetKind,
    /// The new limit, in the unit of the type's provider: 0 to 10 000 dollars, or 0 to 10 000 000
    /// credits. Leave out to keep it.
    #[serde(default)]
    pub limit: Option<f64>,
    /// true switches the type's limit on, false off (usage is still counted); leave out to keep it.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Change a type's limit and/or its switch, and report every type. A limit out of range or a damaged
/// ledger is an error before anything changes.
fn project_assets_limit(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: LimitAssetParams = parse_params(params)?;
    let settings = ctx.session.settings.assets.clone();
    let mut ws = open(&p.project)?;
    let records = ws.try_assets()?;
    if p.limit.is_some() || p.enabled.is_some() {
        // Refused here, so a refusal writes nothing; the save below starts from what is on disk now.
        ws.manifest.assets.clone().set_limit(p.kind, p.limit, p.enabled, &settings)?;
        let mut applied = Ok(());
        ws.set_person(|m| applied = m.assets.set_limit(p.kind, p.limit, p.enabled, &settings))?;
        applied?;
    }
    Ok(ToolOutput::Value(assets_report(&ws, &settings, &records)))
}

/// The master switch, "Ignore budget" and every asset type as the project has it, with what each
/// has used: what all three project asset tools return. `limit` is the number set and `limit_on` the
/// type's switch; `enforced` is the rule generation applies (see [`limit_applies`]), judged by the
/// price stored with the choice, so a generation that finds the provider now gives a price can
/// enforce a limit this reports as not enforced.
fn assets_report(ws: &Workspace, settings: &AssetSettings, records: &[AssetRecord]) -> Value {
    let mut types = serde_json::Map::new();
    for kind in AssetKind::ALL {
        let t = ws.manifest.assets.effective(kind, settings);
        let usage = type_usage(records, kind, &t);
        types.insert(
            kind.id().to_owned(),
            json!({
                "provider": t.choice.provider,
                "model": t.choice.model,
                "model_name": t.choice.model_name,
                "unit": t.choice.unit,
                "limit": round(t.limit),
                "limit_on": t.limit_on,
                "enforced": limit_applies(ws, &t),
                "used": round(usage.amount),
                "generations": usage.generations,
                "cost_missing": usage.cost_missing,
                "tracked": t.choice.pricing.tracked(),
            }),
        );
    }
    json!({"generate_assets": ws.manifest.generate_assets, "ignore_budget": ws.manifest.assets.ignore_budget, "types": types})
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectParams {
    /// The project's folder.
    pub project: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Serve until the call is cancelled instead of returning at once (for the CLI).
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Open the app/ folder (what was built) instead of the project folder.
    #[serde(default)]
    pub app: bool,
    /// Open a pinned build's folder.
    #[serde(default)]
    pub pin: Option<u32>,
}

/// Websites being previewed in this session, by project folder (kept serving until it ends).
#[derive(Default)]
struct Previews(Mutex<HashMap<PathBuf, craft_anything_platform::serve::StaticServer>>);

/// What a project's questions should get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionsChoice {
    Ask,
    Decide,
    /// The app's setting.
    Default,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishParams {
    /// The project's folder.
    pub project: PathBuf,
}

/// What the conversation and the project say when the person calls a build done.
pub const CALLED_DONE: &str = "Ready: you called it done. Everything so far is kept; a new message carries on from here.";

fn project_build_finish(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    use crate::workspace::{Status, Step};
    let p: FinishParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    ws.set_status(Status::Ready, CALLED_DONE)?;
    let mut items = feed::load(&ws);
    items.push(feed::FeedItem::Step { step: Step::Done, text: CALLED_DONE.into() });
    feed::save(&ws.root, &items)?;
    Ok(ToolOutput::Value(json!({"project": ws.root, "status": ws.manifest.status})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionsParams {
    /// The project's folder.
    pub project: PathBuf,
    /// `ask`, `decide` or `default`; none just reads what applies.
    #[serde(default)]
    pub mode: Option<QuestionsChoice>,
}

fn project_questions_set(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    use crate::workspace::Questions;
    let p: QuestionsParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    if let Some(mode) = p.mode {
        let chosen = match mode {
            QuestionsChoice::Ask => Some(Questions::Ask),
            QuestionsChoice::Decide => Some(Questions::Decide),
            QuestionsChoice::Default => None,
        };
        ws.set_person(|m| m.questions = chosen)?;
    }
    let settings = &ctx.session.settings;
    Ok(ToolOutput::Value(json!({
        "project": ws.root,
        "questions": settings.questions_in(Some(&ws.manifest)),
        "chosen": ws.manifest.questions,
        "app_default": settings.questions(),
    })))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Start another copy even if one is running.
    #[serde(default)]
    pub another: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// Only this folder (default: every project folder from settings).
    #[serde(default)]
    pub projects_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Skip this many conversation items from the start.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Return at most this many items (default and maximum 200; the newest when no offset).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The question's or approval's id.
    pub id: String,
    /// The chosen suggestion (questions).
    #[serde(default)]
    pub choice: Option<String>,
    /// A typed answer (questions), or a reason (approvals).
    #[serde(default)]
    pub text: Option<String>,
    /// Allow the action (approvals).
    #[serde(default)]
    pub allow: Option<bool>,
    /// Allow this kind of action for the rest of the project (approvals).
    #[serde(default)]
    pub always: bool,
}

fn open(project: &std::path::Path) -> Result<Workspace, ToolError> {
    Workspace::open(project).map_err(|e| {
        let why = e.message.clone();
        ToolError::new(ErrorCode::NotFound, format!("{} isn't a project folder: {why}", project.display()))
            .with_remediation("Pass a folder from project_list (it holds craft.json).")
    })
}

fn summary(ws: &Workspace) -> Value {
    let m = &ws.manifest;
    json!({
        "name": m.name,
        "folder": ws.root.display().to_string(),
        "app_folder": ws.app_dir().display().to_string(),
        "request": m.request,
        "target": m.target,
        "stack": m.stack,
        "status": m.status,
        "status_label": m.status.label(),
        "note": m.note,
        "updated_ms": m.updated_ms,
        "last_check": m.checks.last(),
        "waiting": Inbox::new(ws.inbox_dir()).pending().len(),
        "usage": ws.usage(),
    })
}

fn project_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ListParams = parse_params(params)?;
    let settings = &ctx.session.settings;
    let locations = p.projects_dir.map_or_else(|| settings.project_locations(), |d| vec![d]);
    let projects: Vec<Value> = Workspace::list_in(&locations).iter().map(summary).collect();
    Ok(ToolOutput::Value(json!({
        "projects_dir": settings.projects_dir().display().to_string(),
        "locations": locations.iter().map(|l| l.display().to_string()).collect::<Vec<_>>(),
        "count": projects.len(),
        "projects": projects,
    })))
}

fn project_show(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ShowParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let items = feed::load(&ws);
    let limit = p.limit.unwrap_or(MAX_ITEMS).clamp(1, MAX_ITEMS);
    let offset = p.offset.unwrap_or_else(|| items.len().saturating_sub(limit)).min(items.len());
    let page: Vec<&feed::FeedItem> = items.iter().skip(offset).take(limit).collect();
    let mut out = summary(&ws);
    if let Some(o) = out.as_object_mut() {
        o.insert("checks".into(), json!(ws.manifest.checks));
        o.insert("last_reply".into(), json!(ws.manifest.last_reply));
        o.insert("questions".into(), json!(Inbox::new(ws.inbox_dir()).pending()));
        o.insert("conversation".into(), json!({"total": items.len(), "offset": offset, "items": page}));
    }
    Ok(ToolOutput::Value(out))
}

fn project_inbox_answer(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AnswerParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let inbox = Inbox::new(ws.inbox_dir());
    // Taken before it's answered: the session that asked may read and remove it right after.
    let Some(question) = inbox.pending().into_iter().find(|r| r.id == p.id) else {
        return Err(ToolError::new(ErrorCode::NotFound, format!("No question {} is waiting in this project.", p.id))
            .with_remediation("Use project_show to see the questions still waiting."));
    };
    let answer = Answer { choice: p.choice, text: p.text, allow: p.allow, always: p.always };
    let shown = match answer.allow {
        Some(true) if answer.always => "Always allowed".to_string(),
        Some(true) => "Allowed".to_string(),
        Some(false) => "Not allowed".to_string(),
        None => answer.as_text(),
    };
    inbox.answer(&p.id, &answer)?;
    // Into the saved conversation; a running build keeps answers it finds there when it saves.
    if let Err(e) = feed::record_answer(&ws, &question, &shown) {
        log::warn!("answer not added to the conversation: {}", e.message);
    }
    Ok(ToolOutput::Value(json!({"answered": p.id, "shown": shown})))
}

fn project_play(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: PlayParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let app = ws.app_dir();
    if crate::outputs::site_dir(&app).is_some() {
        return Err(ToolError::new(ErrorCode::NotSupported, "This project made a website.").with_remediation("Use project_preview to see it."));
    }
    let out = super::pin_tools::start(ctx, &app, &crate::pins::play_log(&ws), p.another, "the finished project")?;
    Ok(ToolOutput::Value(out))
}

fn project_preview(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: PreviewParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let Some(site) = crate::outputs::site_dir(&ws.app_dir()) else {
        return Err(ToolError::new(ErrorCode::NotFound, "There's no built website in this project yet.")
            .with_remediation("Finish a build first (rebuild); a game or app is started with project_play."));
    };
    let previews = match ctx.session.ext.get::<Previews>() {
        Some(p) => p,
        None => {
            let p = Arc::new(Previews::default());
            ctx.session.ext.insert(p.clone());
            p
        }
    };
    let url = {
        let mut map = previews.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(server) = map.get(&ws.root) {
            server.url()
        } else {
            let server = craft_anything_platform::serve::serve_dir(&site).map_err(|e| ToolError::io(site.display(), e))?;
            let url = server.url();
            map.insert(ws.root.clone(), server);
            url
        }
    };
    if p.wait {
        ctx.report(None, format!("Serving {url} (stop to end)"));
        while !ctx.cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        previews.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&ws.root);
    }
    Ok(ToolOutput::Value(json!({"url": url, "site": site.display().to_string()})))
}

fn project_open_folder(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: FolderParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let dir = folder_to_open(&ws, p.app, p.pin)?;
    craft_anything_platform::open_path(&dir)?;
    Ok(ToolOutput::Value(json!({"opened": dir.display().to_string()})))
}

/// The folder `project_open_folder` opens: the project, its app, or a pinned build's copy.
fn folder_to_open(ws: &Workspace, app: bool, pin: Option<u32>) -> Result<PathBuf, ToolError> {
    Ok(match (pin, app) {
        (Some(id), app) => {
            let pin = crate::pins::get(ws, id)?;
            if app { pin.app_dir(ws) } else { pin.dir(ws) }
        }
        (None, true) => ws.app_dir(),
        (None, false) => ws.root.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::RequestKind;
    use crate::workspace::TargetRef;
    use crate::{CallOptions, Caller, Engine};

    /// #210: "enough now". A paused build becomes Ready, says so in its conversation, and keeps
    /// everything it had.
    #[test]
    fn a_build_called_done_is_ready_and_says_so() {
        use crate::workspace::{Status, Step};
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "Site", "A website", TargetRef::None).unwrap();
        ws.set_status(Status::Stopped, "Paused.").unwrap();
        let e = Engine::new(crate::testing::session());
        let r = e.call("project_build_finish", json!({"project": ws.root}), CallOptions::new(Caller::Cli)).unwrap().to_json();
        assert_eq!(r["result"]["status"], "ready", "{r}");
        let ws = Workspace::open(&ws.root).unwrap();
        assert_eq!((ws.manifest.status, ws.manifest.note.as_str()), (Status::Ready, CALLED_DONE));
        let items = feed::load(&ws);
        assert!(
            matches!(items.as_slice(), [feed::FeedItem::User(a), feed::FeedItem::Step { step: Step::Done, text }] if a.text() == "A website" && text == CALLED_DONE),
            "{items:?}"
        );
        let err = e.call("project_build_finish", json!({"project": d.path().join("nope")}), CallOptions::new(Caller::Cli)).unwrap_err();
        assert!(err.message.contains("isn't a project folder"), "{err:?}");
    }

    #[test]
    fn list_show_and_answer_a_project_without_the_app() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
        let q = Inbox::new(ws.inbox_dir()).post(RequestKind::Question, "Blue or red?", "", vec!["Blue".into(), "Red".into()], true, None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |id: &str, p: Value| e.call(id, p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());

        let list = call("project_list", json!({"projects_dir": d.path()})).unwrap();
        assert_eq!(list["count"], 1);
        assert_eq!(list["projects"][0]["name"], "Snake");
        assert_eq!(list["projects"][0]["waiting"], 1);

        let show = call("project_show", json!({"project": ws.root})).unwrap();
        assert_eq!(show["questions"][0]["id"], q.as_str());
        assert_eq!(show["conversation"]["items"][0]["User"], "A snake game");

        let r = call("project_inbox_answer", json!({"project": ws.root, "id": q, "choice": "Blue"})).unwrap();
        assert_eq!(r["shown"], "Blue");
        assert!(Inbox::new(ws.inbox_dir()).pending().is_empty(), "answered");
        let again = call("project_inbox_answer", json!({"project": ws.root, "id": q, "choice": "Red"}));
        assert!(again.is_err(), "a question is answered once");
        assert!(call("project_show", json!({"project": d.path().join("nope")})).is_err());
    }

    /// Budget updates persist within the allowed range, and past spending never produces a negative balance.
    #[test]
    fn a_projects_asset_budget_is_read_and_set() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_budget", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert_eq!(call(json!({"project": ws.root})).unwrap()["budget_usd"], 5.0);
        assert_eq!(call(json!({"project": ws.root, "budget_usd": 12.5})).unwrap()["budget_usd"], 12.5);
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.legacy_budget_usd(), 12.5);
        assert!(call(json!({"project": ws.root, "budget_usd": -1})).is_err());
        assert!(call(json!({"project": ws.root, "budget_usd": 10001})).unwrap_err().message.contains("between 0 and 10000"));
        ws.record_asset(crate::workspace::AssetRecord {
            file: "sound.mp3".into(),
            kind: "sound".into(),
            prompt: "chime".into(),
            provider: "elevenlabs".into(),
            model: "eleven_v3".into(),
            amount: Some(2.25),
            unit: Some(craft_anything_engines::assets::model::Unit::Usd),
            cost_missing: false,
            at_ms: 1,
        })
        .unwrap();
        let out = call(json!({"project": ws.root, "budget_usd": 1.75})).unwrap();
        assert_eq!(out["budget_usd"], 1.75);
        assert_eq!(out["spent_usd"], 2.25);
        assert_eq!(out["remaining_usd"], 0.0);
        assert_eq!(call(json!({"project": ws.root, "budget_usd": 0.0})).unwrap()["budget_usd"], 0.0);
        assert_eq!(call(json!({"project": ws.root, "budget_usd": 10000.0})).unwrap()["remaining_usd"], 9997.75);
    }

    #[test]
    fn a_project_generates_assets_only_once_allowed() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_allow", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert_eq!(call(json!({"project": ws.root})).unwrap()["generate_assets"], false, "off until the person turns it on");
        assert_eq!(call(json!({"project": ws.root, "allow": true})).unwrap()["generate_assets"], true);
        assert!(Workspace::open(&ws.root).unwrap().manifest.generate_assets, "saved with the project");
        assert_eq!(call(json!({"project": ws.root, "allow": false})).unwrap()["generate_assets"], false);
        assert!(call(json!({"project": ws.root, "allow": "yes"})).is_err());
    }

    /// Provider toggles persist independently, while a master off/on cycle restores the selected providers.
    #[test]
    fn provider_switches_are_independent_persisted_and_preserved_by_the_master_switch() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_allow", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert!(call(json!({"project": ws.root, "provider": "missing", "allow": true})).is_err());
        assert!(!Workspace::open(&ws.root).unwrap().manifest.generate_assets);
        let on = call(json!({"project": ws.root, "provider": "scenario", "allow": true})).unwrap();
        assert_eq!(on["asset_providers"], json!(["scenario"]));
        assert_eq!(on["providers"].as_array().unwrap().iter().filter(|p| p["enabled"] == true).count(), 1);
        call(json!({"project": ws.root, "provider": "elevenlabs", "allow": true})).unwrap();
        call(json!({"project": ws.root, "allow": false})).unwrap();
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.assets.legacy_providers, Some(vec!["scenario".into(), "elevenlabs".into()]));
        call(json!({"project": ws.root, "allow": true})).unwrap();
        let manifest = Workspace::open(&ws.root).unwrap().manifest;
        assert!(manifest.asset_provider_allowed("scenario") && manifest.asset_provider_allowed("elevenlabs"));
        assert!(!manifest.asset_provider_allowed("fal"));
        call(json!({"project": ws.root, "provider": "scenario", "allow": false})).unwrap();
        assert_eq!(call(json!({"project": ws.root, "provider": "elevenlabs", "allow": false})).unwrap()["generate_assets"], false);
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.assets.legacy_providers, Some(Vec::new()));
        assert_eq!(call(json!({"project": ws.root, "provider": "fal"})).unwrap()["generate_assets"], false, "query does not mutate");
    }

    /// Editing a paused provider list never resumes its other providers; an explicit enable starts alone.
    #[test]
    fn paused_provider_changes_preserve_choices_without_resuming_them() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_allow", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        for (disabled, remaining) in [("scenario", vec!["elevenlabs"]), ("fal", vec!["scenario", "elevenlabs"])] {
            ws.set_person(|m| {
                m.generate_assets = false;
                m.assets.legacy_providers = Some(vec!["scenario".into(), "elevenlabs".into()]);
            })
            .unwrap();
            let paused = call(json!({"project": ws.root, "provider": disabled, "allow": false})).unwrap();
            assert_eq!(paused["generate_assets"], false);
            assert_eq!(paused["asset_providers"], json!(remaining));
            assert!(paused["providers"].as_array().unwrap().iter().all(|p| p["enabled"] == false));
            let resumed = call(json!({"project": ws.root, "allow": true})).unwrap();
            assert_eq!(resumed["generate_assets"], true);
            assert_eq!(resumed["asset_providers"], json!(remaining));
        }
        call(json!({"project": ws.root, "allow": false})).unwrap();
        let enabled = call(json!({"project": ws.root, "provider": "fal", "allow": true})).unwrap();
        assert_eq!(enabled["generate_assets"], true);
        assert_eq!(enabled["asset_providers"], json!(["fal"]));
    }

    /// A missing ledger means no spending; an unreadable ledger refuses reads and leaves budget writes untouched.
    #[test]
    fn budget_ledger_errors_do_not_partially_change_the_budget() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_budget", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert_eq!(call(json!({"project": ws.root})).unwrap()["spent_usd"], 0.0);
        let ledger = ws.root.join("assets.json");
        for contents in ["not JSON", "{\"version\":999,\"assets\":[]}"] {
            std::fs::write(&ledger, contents).unwrap();
            assert!(call(json!({"project": ws.root})).is_err());
            assert!(call(json!({"project": ws.root, "budget_usd": 1.0})).is_err());
            assert_eq!(Workspace::open(&ws.root).unwrap().manifest.legacy_budget_usd(), 5.0);
            assert_eq!(std::fs::read_to_string(&ledger).unwrap(), contents);
        }
        std::fs::remove_file(&ledger).unwrap();
        std::fs::create_dir(&ledger).unwrap();
        assert!(call(json!({"project": ws.root, "budget_usd": 1.0})).is_err());
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.legacy_budget_usd(), 5.0);
    }

    /// Restricting legacy all-provider permission must retain every other provider, including extensions.
    #[test]
    fn disabling_a_legacy_provider_keeps_other_registered_providers_enabled() {
        use craft_anything_engines::assets::model::{ModelDetail, ModelList, Unit};
        use craft_anything_engines::assets::{AssetKind, AssetProvider, AssetRequest, Generated};
        struct Custom;
        impl AssetProvider for Custom {
            /// Register a provider outside the built-in catalogue to exercise permission preservation.
            fn id(&self) -> &'static str {
                "custom"
            }
            /// Supply a display label for the custom permission-test provider.
            fn name(&self) -> &'static str {
                "Custom"
            }
            fn unit(&self) -> Unit {
                Unit::Usd
            }
            fn list_models(&self, _: Option<&str>, _: AssetKind) -> Result<ModelList, ToolError> {
                Ok(ModelList { models: Vec::new(), error: None })
            }
            fn describe(&self, _: &str, _: &str) -> Result<ModelDetail, ToolError> {
                Err(ToolError::invalid("test provider has no models"))
            }
            /// Fail any generation attempt because this fixture only tests permission changes.
            fn generate(&self, _: &str, _: &ModelDetail, _: &AssetRequest) -> Result<Generated, ToolError> {
                Err(ToolError::invalid("test provider must not generate"))
            }
        }
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        ws.set_person(|m| m.generate_assets = true).unwrap();
        let mut providers = AssetProviders::built_in();
        providers.register(Arc::new(Custom));
        let mut session = crate::testing::session();
        session.ext.insert(Arc::new(providers));
        let e = Engine::new(session);
        e.call("project_assets_allow", json!({"project": ws.root, "provider": "scenario", "allow": false}), CallOptions::new(Caller::Cli)).unwrap();
        let manifest = Workspace::open(&ws.root).unwrap().manifest;
        assert!(!manifest.asset_provider_allowed("scenario"));
        for id in ["fal", "elevenlabs", "custom"] {
            assert!(manifest.asset_provider_allowed(id));
        }
        e.call("project_assets_allow", json!({"project": ws.root, "provider": "custom", "allow": false}), CallOptions::new(Caller::Cli)).unwrap();
        assert!(!Workspace::open(&ws.root).unwrap().manifest.asset_provider_allowed("custom"));
    }

    #[test]
    fn a_built_site_is_previewed_and_nothing_to_play_says_so() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Site", "A page", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let granted = || CallOptions::new(Caller::Cli).with_consent(crate::Consent::Granted);
        let call = |id: &str, p: Value| e.call(id, p, granted()).map(|o| o.to_json()["result"].clone());
        assert!(call("project_preview", json!({"project": ws.root})).is_err(), "nothing built yet");
        assert!(call("project_play", json!({"project": ws.root})).is_err(), "nothing to start yet");
        std::fs::create_dir_all(ws.app_dir().join("dist")).unwrap();
        std::fs::write(ws.app_dir().join("dist").join("index.html"), b"<h1>hi</h1>").unwrap();
        let first = call("project_preview", json!({"project": ws.root})).unwrap();
        let url = first["url"].as_str().unwrap().to_string();
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert_eq!(call("project_preview", json!({"project": ws.root})).unwrap()["url"], url.as_str(), "one server per project");
        let err = call("project_play", json!({"project": ws.root})).unwrap_err();
        assert!(err.remediation.unwrap_or_default().contains("project_preview"));
        // Playing runs the builder's code: refused without consent.
        assert!(e.call("project_play", json!({"project": ws.root}), CallOptions::new(Caller::Mcp)).is_err());
    }

    #[test]
    fn a_pinned_builds_folder_is_opened_from_its_project() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
        let pin = ws.root.join(crate::pins::PINNED).join("7");
        std::fs::create_dir_all(pin.join("app")).unwrap();
        std::fs::write(pin.join(crate::pins::PIN_FILE), r#"{"version":1,"id":7,"name":"Pinned","mode":"frozen","created_ms":0}"#).unwrap();
        assert_eq!(folder_to_open(&ws, false, Some(7)).unwrap(), pin);
        assert_eq!(folder_to_open(&ws, true, Some(7)).unwrap(), pin.join("app"));
        assert_eq!(folder_to_open(&ws, false, None).unwrap(), ws.root);
        assert_eq!(folder_to_open(&ws, true, None).unwrap(), ws.app_dir());
        assert!(folder_to_open(&ws, false, Some(8)).is_err(), "no such pin");
    }

    /// The tools that choose and limit each asset type, and report what each has used.
    mod per_type {
        use super::*;
        use crate::workspace::{AssetRecord, MANIFEST, Status};
        use craft_anything_engines::assets::model::{MAX_ASSET_CREDITS, ModelDetail, ModelList, Pricing, Rate, Unit};
        use craft_anything_engines::assets::{AssetKind, AssetProvider, AssetRequest, Generated};

        /// A registered provider that bills in `unit` and makes nothing.
        struct Stub {
            id: &'static str,
            unit: Unit,
        }
        impl AssetProvider for Stub {
            fn id(&self) -> &'static str {
                self.id
            }
            fn name(&self) -> &'static str {
                "Stub"
            }
            fn unit(&self) -> Unit {
                self.unit
            }
            fn list_models(&self, _: Option<&str>, _: AssetKind) -> Result<ModelList, ToolError> {
                Ok(ModelList { models: Vec::new(), error: None })
            }
            fn describe(&self, _: &str, _: &str) -> Result<ModelDetail, ToolError> {
                Err(ToolError::invalid("the stub has no models"))
            }
            fn generate(&self, _: &str, _: &ModelDetail, _: &AssetRequest) -> Result<Generated, ToolError> {
                Err(ToolError::invalid("the stub must not generate"))
            }
        }

        fn project() -> (tempfile::TempDir, Workspace, Engine) {
            let d = tempfile::tempdir().unwrap();
            let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
            (d, ws, Engine::new(crate::testing::session()))
        }

        fn call(e: &Engine, id: &str, p: Value) -> Result<Value, ToolError> {
            e.call(id, p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone())
        }

        /// What `craft.json` holds, to prove a refused call wrote nothing.
        fn saved(ws: &Workspace) -> Vec<u8> {
            std::fs::read(ws.root.join(MANIFEST)).unwrap()
        }

        fn priced(rate: Rate) -> Value {
            serde_json::to_value(Pricing::Before { unit: Unit::Usd, rate }).unwrap()
        }

        fn after(unit: Unit) -> Value {
            serde_json::to_value(Pricing::After { unit }).unwrap()
        }

        /// A generation as the ledger holds it.
        fn made(kind: &str, provider: &str, unit: Unit, amount: Option<f64>, cost_missing: bool, at_ms: u64) -> AssetRecord {
            AssetRecord {
                file: format!("{kind}-{at_ms}"),
                kind: kind.into(),
                prompt: "p".into(),
                provider: provider.into(),
                model: "m".into(),
                amount,
                unit: Some(unit),
                cost_missing,
                at_ms,
            }
        }

        fn choose_image(e: &Engine, ws: &Workspace, provider: &str, model: &str) -> Value {
            let pricing = match provider {
                "fal" => priced(Rate::PerMegapixel(0.025)),
                _ => after(Unit::Credits),
            };
            call(
                e,
                "project_assets_choose",
                json!({"project": ws.root, "kind": "image", "provider": provider, "model": model, "model_name": model, "pricing": pricing}),
            )
            .unwrap()
        }

        #[test]
        fn a_project_reports_usage_per_type() {
            let (_d, ws, e) = project();
            let off = call(&e, "project_assets_allow", json!({"project": ws.root})).unwrap();
            assert_eq!((&off["generate_assets"], &off["ignore_budget"]), (&json!(false), &json!(false)));
            let mut kinds: Vec<_> = off["types"].as_object().unwrap().keys().cloned().collect();
            kinds.sort();
            assert_eq!(kinds, ["image", "model_3d", "music", "sound_effect", "speech"]);
            let nothing = json!({"provider": null, "model": "", "model_name": "", "unit": "usd", "limit": 5.0, "limit_on": true, "enforced": false,
                "used": 0.0, "generations": 0, "cost_missing": false, "tracked": false});
            for kind in kinds {
                assert_eq!(off["types"][&kind], nothing, "a type nobody chose is Off with the built-in dollar limit: {kind}");
            }

            let image = choose_image(&e, &ws, "fal", "fal-ai/flux/dev");
            assert_eq!(
                image["types"]["image"],
                json!({"provider": "fal", "model": "fal-ai/flux/dev", "model_name": "fal-ai/flux/dev", "unit": "usd", "limit": 5.0, "limit_on": true,
                    "enforced": true, "used": 0.0, "generations": 0, "cost_missing": false, "tracked": true})
            );
            let model = call(
                &e,
                "project_assets_choose",
                json!({"project": ws.root, "kind": "model_3d", "provider": "scenario", "model": "model_x", "model_name": "Model X", "pricing": after(Unit::Credits)}),
            )
            .unwrap();
            assert_eq!(
                (&model["types"]["model_3d"]["unit"], &model["types"]["model_3d"]["limit"]),
                (&json!("credits"), &json!(500.0)),
                "the unit is the provider's"
            );
            let music = call(&e, "project_assets_choose", json!({"project": ws.root, "kind": "music", "provider": "elevenlabs", "model": "music_v1"})).unwrap();
            assert_eq!(
                (&music["types"]["music"]["tracked"], &music["types"]["music"]["enforced"], &music["types"]["music"]["model_name"]),
                (&json!(false), &json!(false), &json!("music_v1")),
                "no price: usage is counted without an amount and no limit applies; the name defaults to the id"
            );

            // The ledger: two pictures (one cost unread), a 3D model, and a track of a model with no price.
            let at = crate::inbox::now_ms() + 3_600_000;
            for r in [
                made("image", "fal", Unit::Usd, Some(1.25), false, at),
                made("image", "fal", Unit::Usd, None, true, at),
                made("model", "scenario", Unit::Credits, Some(30.0), false, at),
                made("music", "elevenlabs", Unit::Credits, None, false, at),
                made("sound", "elevenlabs", Unit::Credits, Some(9.0), false, at),
            ] {
                ws.record_asset(r).unwrap();
            }
            let now = call(&e, "project_assets_allow", json!({"project": ws.root})).unwrap();
            let t = &now["types"];
            assert_eq!((&t["image"]["used"], &t["image"]["generations"], &t["image"]["cost_missing"]), (&json!(1.25), &json!(2), &json!(true)));
            assert_eq!((&t["model_3d"]["used"], &t["model_3d"]["generations"], &t["model_3d"]["cost_missing"]), (&json!(30.0), &json!(1), &json!(false)));
            assert_eq!((&t["music"]["used"], &t["music"]["generations"], &t["music"]["cost_missing"]), (&json!(0.0), &json!(1), &json!(false)));
            assert_eq!(t["sound_effect"]["generations"], 0, "a type that is Off counts nothing");
            // The other two tools say the same thing about the same state.
            let limit = call(&e, "project_assets_limit", json!({"project": ws.root, "kind": "image"})).unwrap();
            assert_eq!((&limit["types"], &limit["generate_assets"]), (&now["types"], &now["generate_assets"]));

            // `enforced` is the rule generation applies: the type's switch on, the project not ignoring
            // its budget, and a price to count with.
            for ignore in [false, true] {
                call(&e, "project_assets_allow", json!({"project": ws.root, "ignore_budget": ignore})).unwrap();
                for on in [true, false] {
                    let r = call(&e, "project_assets_limit", json!({"project": ws.root, "kind": "image", "enabled": on})).unwrap();
                    assert_eq!(
                        (&r["ignore_budget"], &r["types"]["image"]["limit_on"], &r["types"]["image"]["enforced"]),
                        (&json!(ignore), &json!(on), &json!(on && !ignore))
                    );
                    assert_eq!(
                        (&r["types"]["image"]["used"], &r["types"]["image"]["limit"]),
                        (&json!(1.25), &json!(5.0)),
                        "usage is still counted and the limit still shown"
                    );
                    assert_eq!(r["types"]["music"]["enforced"], false, "no price, no limit");
                }
            }
        }

        #[test]
        fn choosing_a_provider_of_another_unit_resets_the_limit() {
            let (_d, mut ws, e) = project();
            choose_image(&e, &ws, "fal", "fal-ai/flux/dev");
            // An earlier period, so the later choices are not counted from the same moment.
            ws.set_person(|m| m.assets.types.get_mut(&AssetKind::Image).unwrap().since_ms = 1).unwrap();
            let r = call(&e, "project_assets_limit", json!({"project": ws.root, "kind": "image", "limit": 20, "enabled": false})).unwrap();
            assert_eq!((&r["types"]["image"]["limit"], &r["types"]["image"]["limit_on"]), (&json!(20.0), &json!(false)));
            ws.record_asset(made("image", "fal", Unit::Usd, Some(3.0), false, 100)).unwrap();

            let same = choose_image(&e, &ws, "fal", "fal-ai/flux/schnell");
            let image = &same["types"]["image"];
            assert_eq!(
                (&image["model"], &image["limit"], &image["limit_on"], &image["used"]),
                (&json!("fal-ai/flux/schnell"), &json!(20.0), &json!(false), &json!(3.0)),
                "another model of the same provider keeps the limit, its switch and the count"
            );

            let credits = choose_image(&e, &ws, "scenario", "model_y");
            let image = &credits["types"]["image"];
            assert_eq!(
                (&image["unit"], &image["limit"], &image["limit_on"], &image["used"], &image["generations"]),
                (&json!("credits"), &json!(500.0), &json!(true), &json!(0.0), &json!(0)),
                "another provider in another unit starts with the credit limit, switched on, counted afresh"
            );
            assert_eq!(ws.try_assets().unwrap().len(), 1, "the earlier generation stays in the history");

            let back = choose_image(&e, &ws, "fal", "fal-ai/flux/dev");
            let image = &back["types"]["image"];
            assert_eq!((&image["unit"], &image["limit"], &image["used"]), (&json!("usd"), &json!(5.0), &json!(0.0)), "going back counts afresh too");

            // Off is a provider of its own: choosing again afterwards starts afresh, with the limit back on.
            call(&e, "project_assets_limit", json!({"project": ws.root, "kind": "image", "limit": 8, "enabled": false})).unwrap();
            let off = call(&e, "project_assets_choose", json!({"project": ws.root, "kind": "image", "provider": null})).unwrap();
            assert_eq!(
                (&off["types"]["image"]["provider"], &off["types"]["image"]["model"], &off["types"]["image"]["enforced"]),
                (&json!(null), &json!(""), &json!(false))
            );
            let again = choose_image(&e, &ws, "fal", "fal-ai/flux/dev");
            assert_eq!((&again["types"]["image"]["limit"], &again["types"]["image"]["limit_on"]), (&json!(5.0), &json!(true)));
        }

        #[test]
        fn an_unregistered_provider_or_wrong_unit_is_refused() {
            let (_d, ws, _) = project();
            let mut providers = AssetProviders::built_in();
            providers.register(Arc::new(Stub { id: "custom", unit: Unit::Usd }));
            let mut session = crate::testing::session();
            session.ext.insert(Arc::new(providers));
            let e = Engine::new(session);
            let before = saved(&ws);
            let choose = |extra: Value| {
                let mut p = json!({"project": ws.root, "kind": "image", "provider": "fal", "model": "m"});
                p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
                call(&e, "project_assets_choose", p)
            };
            let refused = |r: Result<Value, ToolError>, words: &str| {
                let err = r.expect_err(words);
                assert!(err.message.contains(words), "{words}: {err:?}");
                assert_eq!(saved(&ws), before, "{words}: a refused choice writes nothing");
            };
            refused(choose(json!({"provider": "nope"})), "No asset provider called \"nope\"");
            refused(choose(json!({"provider": "scenario", "unit": "usd"})), "credits");
            refused(choose(json!({"provider": "fal", "unit": "credits"})), "dollars");
            refused(choose(json!({"provider": "custom", "unit": "credits"})), "dollars");
            refused(choose(json!({"model": " "})), "model");
            refused(choose(json!({"model": "x".repeat(1000)})), "model");
            refused(choose(json!({"model_name": "x".repeat(1000)})), "model_name");
            refused(choose(json!({"pricing": after(Unit::Credits)})), "price");
            refused(choose(json!({"pricing": priced(Rate::PerRequest(-0.5))})), "price");
            refused(choose(json!({"pricing": priced(Rate::PerRequest(1e300))})), "price");
            refused(choose(json!({"pricing": priced(Rate::Quoted { typical: Some(10_001.0) })})), "price");
            refused(choose(json!({"kind": "video"})), "kind");
            refused(choose(json!({"surprise": 1})), "surprise");
            // Leaving the provider out is not the same as choosing Off, and the schema says so.
            let schema = crate::tool::schema_of::<ChooseAssetParams>();
            let required: Vec<_> = schema["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
            assert!(required.contains(&"provider") && !required.contains(&"model"), "{schema}");
            assert_eq!(schema["properties"]["provider"]["type"], json!(["string", "null"]), "{schema}");
            assert!(schema["properties"]["provider"]["description"].as_str().is_some_and(|d| d.contains("Off")), "the field keeps its description: {schema}");
            let mut no_provider = json!({"project": ws.root, "kind": "image"});
            refused(call(&e, "project_assets_choose", no_provider.clone()), "provider");
            // An Off choice has nothing else to say.
            no_provider["provider"] = json!(null);
            no_provider["model"] = json!("m");
            refused(call(&e, "project_assets_choose", no_provider), "Off");

            // A registered provider of another kind of unit works, in its own unit.
            let ok = choose(json!({"provider": "custom", "unit": "usd", "model": "c1"})).unwrap();
            assert_eq!((&ok["types"]["image"]["provider"], &ok["types"]["image"]["unit"]), (&json!("custom"), &json!("usd")));
            let ok = choose(json!({"provider": "scenario", "model": "s1"})).unwrap();
            assert_eq!(ok["types"]["image"]["unit"], "credits", "the unit defaults to the provider's");
            assert!(choose(json!({"provider": "scenario", "model": "s1", "unit": "credits"})).is_ok());
        }

        #[test]
        fn limits_outside_their_range_are_refused() {
            let (_d, ws, e) = project();
            choose_image(&e, &ws, "fal", "fal-ai/flux/dev");
            call(
                &e,
                "project_assets_choose",
                json!({"project": ws.root, "kind": "model_3d", "provider": "scenario", "model": "m3", "pricing": after(Unit::Credits)}),
            )
            .unwrap();
            let before = saved(&ws);
            let limit = |kind: &str, limit: f64| call(&e, "project_assets_limit", json!({"project": ws.root, "kind": kind, "limit": limit}));
            for (kind, bad) in [("image", 10_001.0), ("image", -1.0), ("model_3d", MAX_ASSET_CREDITS + 1.0), ("model_3d", -0.5), ("sound_effect", 10_001.0)] {
                let err = limit(kind, bad).expect_err("out of range");
                assert!(err.message.contains("A limit must be a number from 0 to"), "{kind} {bad}: {err:?}");
                assert_eq!(saved(&ws), before, "{kind} {bad}: nothing changed");
            }
            assert!(call(&e, "project_assets_limit", json!({"project": ws.root, "kind": "image", "limit": "ten"})).is_err());
            assert!(call(&e, "project_assets_limit", json!({"project": ws.root, "kind": "image", "limit": null, "extra": 1})).is_err());
            // The most and nothing at all are limits (credits go far above dollars).
            let r = limit("image", 10_000.0).unwrap();
            assert_eq!(r["types"]["image"]["limit"], 10_000.0);
            assert_eq!(limit("image", 0.0).unwrap()["types"]["image"]["limit"], 0.0);
            assert_eq!(limit("model_3d", MAX_ASSET_CREDITS).unwrap()["types"]["model_3d"]["limit"], MAX_ASSET_CREDITS);
            assert_eq!(limit("model_3d", 10_001.0).unwrap()["types"]["model_3d"]["limit"], 10_001.0, "credits are not capped at the dollar maximum");
        }

        #[test]
        fn ignore_budget_is_a_person_field() {
            let (_d, mut builder, e) = project();
            // `builder` is the copy a running build holds, from before the person's changes.
            call(&e, "project_assets_allow", json!({"project": builder.root, "ignore_budget": true})).unwrap();
            assert!(Workspace::open(&builder.root).unwrap().manifest.assets.ignore_budget);
            assert!(!Workspace::open(&builder.root).unwrap().manifest.generate_assets, "the switches are separate");
            choose_image(&e, &builder, "fal", "fal-ai/flux/dev");
            call(&e, "project_assets_limit", json!({"project": builder.root, "kind": "image", "limit": 7.5, "enabled": false})).unwrap();
            call(&e, "project_assets_allow", json!({"project": builder.root, "allow": true})).unwrap();
            assert!(Workspace::open(&builder.root).unwrap().manifest.assets.ignore_budget, "allowing generation leaves Ignore budget as it was");

            assert!(!builder.manifest.assets.ignore_budget && builder.manifest.assets.types.is_empty(), "the build's copy is stale");
            builder.set_status(Status::Stopped, "Paused.").unwrap();
            builder.manifest.note = "saved from the old copy".into();
            builder.save().unwrap();
            let disk = Workspace::open(&builder.root).unwrap().manifest;
            assert_eq!(disk.note, "saved from the old copy", "the save happened");
            assert!(disk.assets.ignore_budget, "a stale save keeps Ignore budget");
            assert!(disk.generate_assets);
            let t = &disk.assets.types[&AssetKind::Image];
            assert_eq!((t.choice.provider.as_deref(), t.limit, t.limit_on), (Some("fal"), 7.5, false), "and the choice, the limit and its switch");

            let off = call(&e, "project_assets_allow", json!({"project": builder.root, "ignore_budget": false})).unwrap();
            assert_eq!((&off["ignore_budget"], &off["generate_assets"]), (&json!(false), &json!(true)));
            assert!(!Workspace::open(&builder.root).unwrap().manifest.assets.ignore_budget);
        }

        #[test]
        fn a_damaged_ledger_stops_the_asset_tools_before_they_change_anything() {
            let (_d, ws, e) = project();
            choose_image(&e, &ws, "fal", "fal-ai/flux/dev");
            let before = saved(&ws);
            std::fs::write(ws.root.join("assets.json"), "not JSON").unwrap();
            for (id, p) in [
                ("project_assets_allow", json!({"project": ws.root})),
                ("project_assets_allow", json!({"project": ws.root, "allow": true, "ignore_budget": true})),
                ("project_assets_choose", json!({"project": ws.root, "kind": "image", "provider": null})),
                ("project_assets_limit", json!({"project": ws.root, "kind": "image", "limit": 9})),
            ] {
                assert!(call(&e, id, p.clone()).is_err(), "{id} {p}: damaged spending is never shown as none");
                assert_eq!(saved(&ws), before, "{id} {p}: and nothing was changed");
            }
        }

        #[test]
        fn the_new_tools_run_in_clean_room_beside_the_old_ones() {
            for id in ["project_assets_allow", "project_assets_choose", "project_assets_limit"] {
                assert!(crate::clean_room::allows(id), "{id}");
            }
        }
    }
}
