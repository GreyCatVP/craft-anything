//! Project assets the builder generates through registered providers: pictures
//! (`assets_image_generate`: a texture, a sky, concept art), low-poly GLB models
//! (`assets_model_generate`), speech, sound effects and music. Each asset type is made with the
//! provider and model the project chose for it, and has its own limit in that provider's unit
//! (dollars or credits): the builder generates freely under it, and the person is asked before it
//! goes over, unless they switched the type's limit off or the project ignores the budget. Nothing
//! is generated unless the person turned generated assets on for the project and chose a provider
//! for the type. Every generation and what it cost is listed in the project (assets.json).

use crate::inbox::Inbox;
use crate::settings::AssetSettings;
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::tools::interaction_tools::{Verdict, ask_approval, inbox, inside, resolve, workspace_of};
use crate::workspace::{AssetRecord, OWN_NAMES, SpendLock, TypeAssets, TypeUsage, Workspace};
use craft_anything_engines::assets::inputs::map_request;
use craft_anything_engines::assets::model::{ModelDetail, Unit, amount};
use craft_anything_engines::assets::tables::InputRoles;
use craft_anything_engines::assets::{AssetKind, AssetProvider, AssetProviders, AssetRequest, Generated, Shape, charged_amount, fal, was_charged};
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The person's key for each asset provider (by provider id), put in the session by the tool
/// server (read from the keychain; a key never travels on a command line or into a log).
#[derive(Default)]
pub struct AssetKeys(pub std::collections::BTreeMap<String, String>);

impl AssetKeys {
    /// The key for `provider`, if one is set up.
    pub fn get(&self, provider: &str) -> Option<&str> {
        self.0.get(provider).map(String::as_str).filter(|k| !k.trim().is_empty())
    }
}

/// Remembered, per asset type, when the person says "always" to going over its limit.
fn over_limit_approval(kind: AssetKind) -> String {
    format!("assets:over-budget:{}", kind.id())
}

/// How long a generation waits for another process's generation in the same project to finish.
const SPEND_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Amounts this close are equal: sums of decimal amounts are not exact.
const EPSILON: f64 = 1e-9;

/// Expose image, model and audio generation tools with network and filesystem effects declared.
pub fn specs() -> Vec<ToolSpec> {
    let effects = Effects::ACCESSES_NETWORK.union(Effects::WRITES_FILESYSTEM);
    vec![
        ToolSpec {
            id: "assets_image_generate",
            title: "Generate a picture",
            summary: "Make a picture with the provider and model the project chose for pictures, and save it as a PNG inside app/: a texture, a sky or background, an icon, a title screen or concept art. shape: square (1024x1024, default), wide (1024x576) or tall (576x1024); a model that can't make the shape says so in the result's ignored list. Counts against the project's limit for pictures, in its provider's unit (dollars or credits). Describe original designs in the original's style; never ask for recognisable characters, logos or brand artwork. Returns where it was saved, what it cost and what is left of the limit.",
            beginner: "Makes a picture for your game, like a texture or a sky.",
            toolset: Toolset::Core,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<ImageParams>,
            run: assets_image_generate,
        },
        ToolSpec {
            id: "assets_model_generate",
            title: "Generate a 3D model",
            summary: "Make a game-ready, low-poly 3D model (GLB) with the provider and model the project chose for 3D models, and save it inside app/. Give a prompt or an image (a web address, or a picture inside the project such as reference/picture-1.png). From a prompt, a picture is made first with the project's picture model (counted against the limit for pictures) when the 3D model starts from pictures; a 3D model that takes words is given the prompt. faces requests the face count (default 3000): about 500-1500 for small props and pickups, 2000-5000 for weapons, furniture and vehicles, 6000-15000 for main characters; fewer faces run faster, so use the fewest that still look right (the model keeps it within its own range). textured (default true) gives colours and surfaces. Counts against the project's limit for 3D models; it takes a minute or two. Describe one original object per model; never ask for recognisable characters, logos or brand artwork. Returns where it was saved, what it cost and what is left of the limit.",
            beginner: "Makes a 3D object for your game, like a crate, a tree or a car.",
            toolset: Toolset::Core,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<ModelParams>,
            run: assets_model_generate,
        },
        ToolSpec {
            id: "assets_speech_generate",
            title: "Generate speech",
            summary: "Generate speech from text with the provider and model the project chose for speech, and save an MP3 inside app/. Choose a voice_id you are entitled to use. Requires the project's generated-assets opt-in and counts against its limit for speech. Returns the saved path and what it cost.",
            beginner: "Makes spoken dialogue or narration for your project.",
            toolset: Toolset::Core,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<SpeechParams>,
            run: assets_speech_generate,
        },
        ToolSpec {
            id: "assets_sound_generate",
            title: "Generate a sound effect",
            summary: "Generate an original sound effect from a prompt and duration_seconds (0.5 to 30) with the provider and model the project chose for sound effects, saving an MP3 inside app/. Requires the project's generated-assets opt-in and counts against its limit for sound effects.",
            beginner: "Makes a sound effect for your project.",
            toolset: Toolset::Core,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<SoundParams>,
            run: assets_sound_generate,
        },
        ToolSpec {
            id: "assets_music_generate",
            title: "Generate music",
            summary: "Generate original music from a prompt and duration_seconds (3 to 600) with the provider and model the project chose for music, saving an MP3 inside app/. instrumental defaults to true. Requires the project's generated-assets opt-in and counts against its limit for music.",
            beginner: "Makes original background music for your project.",
            toolset: Toolset::Core,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<MusicParams>,
            run: assets_music_generate,
        },
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageParams {
    /// What to draw, in detail (subject, style, colours, lighting).
    pub prompt: String,
    /// Where to save it, relative to app/, ending in .png (e.g. Assets/Textures/crate.png).
    pub save_as: String,
    /// square (default), wide or tall.
    #[serde(default)]
    pub shape: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelParams {
    /// Where to save it, relative to app/, ending in .glb (e.g. Assets/Models/crate.glb).
    pub save_as: String,
    /// What to make: one object, described in detail. Used when no image is given.
    #[serde(default)]
    pub prompt: String,
    /// A picture to turn into the model: a web address, or a picture inside the project.
    #[serde(default)]
    pub image: String,
    /// Colours and surfaces (default true).
    #[serde(default)]
    pub textured: Option<bool>,
    /// Requested face count (default 3000); the model keeps it within its own range.
    #[serde(default)]
    pub faces: Option<u32>,
}

/// A mid-sized game prop: detailed enough up close, cheap to draw many of.
const DEFAULT_FACES: u32 = 3000;
/// The most faces asked of any model; each model holds the count to its own range as well.
const MAX_FACES: u32 = 500_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpeechParams {
    /// The words to speak (1 to 5000 characters).
    pub text: String,
    /// A voice id of the chosen speech model that you are entitled to use.
    pub voice_id: String,
    /// Destination relative to app/, ending in .mp3.
    pub save_as: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoundParams {
    pub prompt: String,
    /// Sound length, 0.5 to 30 seconds.
    pub duration_seconds: f64,
    /// Destination relative to app/, ending in .mp3.
    pub save_as: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MusicParams {
    pub prompt: String,
    /// Track length, 3 to 600 seconds.
    pub duration_seconds: f64,
    /// Defaults to true (no vocals).
    #[serde(default)]
    pub instrumental: Option<bool>,
    /// Destination relative to app/, ending in .mp3.
    pub save_as: String,
}

/// What generations of `kind` are called, many of them and one.
fn type_words(kind: AssetKind) -> (&'static str, &'static str) {
    match kind {
        AssetKind::Image => ("pictures", "picture"),
        AssetKind::Model3d => ("3D models", "3D model"),
        AssetKind::Speech => ("speech", "clip of speech"),
        AssetKind::SoundEffect => ("sound effects", "sound effect"),
        AssetKind::Music => ("music", "music track"),
        // A type a later build of the engines adds.
        _ => ("assets", "asset"),
    }
}

/// Reject negative or nonfinite amounts before they enter limit or spending calculations.
fn valid_cost(cost: f64) -> Result<(), ToolError> {
    if cost.is_finite() && cost >= 0.0 { Ok(()) } else { Err(ToolError::invalid("asset costs and limits must be finite and nonnegative")) }
}

/// An amount as a receipt or a warning states it, to the fraction of a cent or credit.
fn exact(unit: Unit, x: f64) -> String {
    match unit {
        Unit::Usd => format!("${x:.6}"),
        Unit::Credits => format!("{x:.4} credits"),
    }
}

/// Resolve existing ancestors, including symlinks, without creating an output directory.
fn canonical_future(path: &Path) -> Result<PathBuf, ToolError> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let mut resolved = std::fs::canonicalize(ancestor).map_err(|e| ToolError::io(ancestor.display(), e))?;
                for part in missing.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(ancestor.file_name().ok_or_else(|| ToolError::invalid("asset path has no existing root"))?);
                ancestor = ancestor.parent().ok_or_else(|| ToolError::invalid("asset path has no existing root"))?;
            }
            Err(e) => return Err(ToolError::io(ancestor.display(), e)),
        }
    }
}

/// Whether two names may be one entry on a case-insensitive file system (macOS, Windows): equal
/// by lower- or upper-case mapping, so "Reference" and "attachmentſ" meet their records too.
fn same_name_ignoring_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase() || a.to_uppercase() == b.to_uppercase()
}

/// Refuse a destination that is one of Craft Anything's records (in a project whose app is the
/// project folder, they sit beside the person's files), named directly, in another case, or
/// through a link.
fn not_a_record(ws: &Workspace, path: &Path) -> Result<(), ToolError> {
    let refused = || ToolError::invalid("save_as must not be one of Craft Anything's own records");
    if ws.is_own_record(path) {
        return Err(refused());
    }
    let root = canonical_future(&ws.root)?;
    let resolved = canonical_future(path)?;
    // A folder that doesn't exist yet keeps the case it was given, so compare the first part under
    // the project ignoring case: on macOS "Reference/x.png" is written into reference/.
    let first = |p: &Path, base: &Path| p.strip_prefix(base).ok()?.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned());
    for top in [first(path, &ws.root), first(&resolved, &root)].into_iter().flatten() {
        if OWN_NAMES.iter().filter(|n| **n != "checks").any(|n| same_name_ignoring_case(n, &top)) {
            return Err(refused());
        }
    }
    for name in OWN_NAMES.iter().filter(|n| **n != "checks") {
        if inside(&canonical_future(&root.join(name))?, &resolved) {
            return Err(refused());
        }
    }
    Ok(())
}

/// Reject paths whose resolved ancestors escape the root, including through symbolic links.
fn confined(root: &Path, path: &Path) -> Result<(), ToolError> {
    let root = canonical_future(root)?;
    let path = canonical_future(path)?;
    if inside(&root, &path) { Ok(()) } else { Err(ToolError::invalid("asset paths must stay inside the project, including through symbolic links")) }
}

/// Read a nonempty regular asset file with a 20 MiB cap, rechecking size after opening and reading.
fn read_asset(path: &Path) -> Result<Vec<u8>, ToolError> {
    const MAX: u64 = 20 << 20;
    let before = std::fs::metadata(path).map_err(|e| ToolError::io(path.display(), e))?;
    if !before.is_file() || before.len() == 0 || before.len() > MAX {
        return Err(ToolError::invalid("asset input must be a nonempty regular file of at most 20 MiB"));
    }
    let file = std::fs::File::open(path).map_err(|e| ToolError::io(path.display(), e))?;
    let meta = file.metadata().map_err(|e| ToolError::io(path.display(), e))?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX {
        return Err(ToolError::invalid("asset input must be a nonempty regular file of at most 20 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes).map_err(|e| ToolError::io(path.display(), e))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX {
        return Err(ToolError::invalid("asset input must contain 1 byte to 20 MiB"));
    }
    Ok(bytes)
}

/// Require a finite duration within the selected audio generator's inclusive bounds.
fn duration_ok(seconds: f64, minimum: f64, maximum: f64) -> Result<(), ToolError> {
    if seconds.is_finite() && (minimum..=maximum).contains(&seconds) {
        Ok(())
    } else {
        Err(ToolError::invalid(format!("duration_seconds must be {minimum} to {maximum}")))
    }
}

/// Validate speech text and the voice id before generating it with the project's speech model.
fn assets_speech_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SpeechParams = parse_params(params)?;
    let text = p.text.trim();
    if text.is_empty() || text.chars().count() > 5000 {
        return Err(ToolError::invalid("text must contain 1 to 5000 characters"));
    }
    let voice = p.voice_id.trim();
    if voice.is_empty() || voice.len() > 128 || !voice.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        return Err(ToolError::invalid("voice_id must contain 1 to 128 letters, digits, underscores or hyphens"));
    }
    generate_one(ctx, &p.save_as, "mp3", text, AssetRequest::Speech { text, voice_id: voice }, "Generating speech…")
}

/// Validate the prompt and sound duration before generating an MP3 with the project's sound model.
fn assets_sound_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SoundParams = parse_params(params)?;
    let prompt = prompt_ok(&p.prompt)?;
    duration_ok(p.duration_seconds, 0.5, 30.0)?;
    generate_one(ctx, &p.save_as, "mp3", prompt, AssetRequest::SoundEffect { prompt, duration_seconds: p.duration_seconds }, "Generating a sound effect…")
}

/// Validate music length and prompt, defaulting to instrumental audio, before generating it.
fn assets_music_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: MusicParams = parse_params(params)?;
    let prompt = prompt_ok(&p.prompt)?;
    duration_ok(p.duration_seconds, 3.0, 600.0)?;
    let req = AssetRequest::Music { prompt, duration_seconds: p.duration_seconds, instrumental: p.instrumental.unwrap_or(true) };
    generate_one(ctx, &p.save_as, "mp3", prompt, req, "Generating music…")
}

/// Make one asset of the request's type with the project's choice for it: the destination, the
/// type's provider, key and model are checked, then its limit, before anything is spent.
fn generate_one(ctx: &mut Ctx<'_>, save_as: &str, ext: &str, from: &str, req: AssetRequest<'_>, progress: &str) -> Result<ToolOutput, ToolError> {
    let mut j = job(ctx)?;
    let (path, rel) = save_path(&j.ws, save_as, ext)?;
    let ready = j.ready(req.kind())?;
    let estimate = ready.estimate(&req)?;
    within_limit(ctx, &j.inbox, &mut j.ws, ready.kind, &ready.limited(), estimate)?;
    ctx.report(None, progress);
    let made = make(&j.ws, &ready, &req, &path, &rel, from, estimate)?;
    summary(&j.ws, &ready, &rel, &made, json!({}))
}

/// What a generation needs: the keys, the providers, the starting choices from Settings (for a
/// project that has not chosen a type yet), the inbox (for the over-limit question) and the
/// project, with its spending lock held until the generation's receipts are written.
struct Job {
    keys: Arc<AssetKeys>,
    providers: AssetProviders,
    settings: AssetSettings,
    inbox: Inbox,
    ws: Workspace,
    _spending: SpendLock,
}

/// Load generation context only for an opted-in project whose manifest this build can safely update.
fn job(ctx: &Ctx<'_>) -> Result<Job, ToolError> {
    let keys = ctx.session.ext.get::<AssetKeys>().unwrap_or_default();
    // A session can bring its own providers (a fork, a test); otherwise the built-in ones.
    let providers = ctx.session.ext.get::<AssetProviders>().map(|p| p.as_ref().clone()).unwrap_or_default();
    let settings = ctx.session.settings.assets.clone();
    let not_project = || ToolError::new(ErrorCode::FeatureMissing, "Assets can only be generated inside a Craft Anything project.");
    let inbox = inbox(ctx).ok_or_else(not_project)?;
    let mut ws = workspace_of(&inbox).ok_or_else(not_project)?;
    generation_permitted(&ws)?;
    // Held through the limit check, the paid calls and their receipts: another process sharing
    // this project then sees every earlier receipt and never writes the ledger at the same time.
    let spending = ws.lock_spending(SPEND_LOCK_WAIT)?;
    // The person may have changed their choices (generation, providers, limits) while this waited.
    ws.reload()?;
    generation_permitted(&ws)?;
    Ok(Job { keys, providers, settings, inbox, ws, _spending: spending })
}

/// Generation needs a manifest this build can update and the person's opt-in for the project.
fn generation_permitted(ws: &Workspace) -> Result<(), ToolError> {
    if ws.is_read_only() {
        return Err(ToolError::new(ErrorCode::NoPermission, "This project's settings were written by a newer Craft Anything; asset generation is disabled.")
            .with_remediation("Update Craft Anything before generating assets for this project."));
    }
    if !ws.manifest.generate_assets {
        return Err(ToolError::new(ErrorCode::NoPermission, "The person hasn't turned on generated assets for this project.").with_remediation(
            "Make these assets with simple shapes and colours instead, and tell the person they can turn on generated assets for this game in the app.",
        ));
    }
    Ok(())
}

/// Explain which provider lacks credentials and offer a fallback that requires no paid assets.
fn no_key(provider: &str) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, format!("No {provider} key is set up for this asset operation.")).with_remediation(format!(
        "Make these assets with simple shapes and colours instead, and tell the person they can add a {provider} key in Settings → AI for realistic ones."
    ))
}

/// Where the person chooses what each asset type is made with.
const CHOOSE_AGAIN: &str = "in the project's Generated Assets settings";

/// The type is Off for this project.
fn type_off(kind: AssetKind) -> ToolError {
    let (many, _) = type_words(kind);
    ToolError::new(ErrorCode::NoPermission, format!("Generated {many} are off for this project.")).with_remediation(format!(
        "Make these with simple shapes, colours or the assets already made instead, and tell the person they can choose a provider and model for {many} {CHOOSE_AGAIN}."
    ))
}

impl Job {
    /// Everything one asset type needs before its limit is checked: the project's choice (Off is
    /// refused), a registered provider billing in the unit the type is counted in, its key, and the
    /// chosen model as the provider describes it now. The ledger is read (checked) before the
    /// provider is asked anything, so a damaged ledger stops the generation first; a model the
    /// provider no longer serves stops it before anything is spent.
    fn ready(&self, kind: AssetKind) -> Result<Ready, ToolError> {
        let (many, _) = type_words(kind);
        let t = self.ws.manifest.asset_type_on(kind, &self.settings).ok_or_else(|| type_off(kind))?;
        let Some(id) = t.choice.provider.clone() else { return Err(type_off(kind)) };
        let provider = self.providers.get(&id).ok_or_else(|| {
            ToolError::new(ErrorCode::FeatureMissing, format!("No asset provider called \"{id}\" is available."))
                .with_remediation(format!("Ask the person to choose another provider for {many} {CHOOSE_AGAIN}."))
        })?;
        if provider.unit() != t.choice.unit {
            return Err(ToolError::new(
                ErrorCode::BadSettings,
                format!("The project counts {many} in {}, but {} bills in {}.", unit_words(t.choice.unit), provider.name(), unit_words(provider.unit())),
            )
            .with_remediation(format!("Ask the person to choose the provider and model for {many} again {CHOOSE_AGAIN}.")));
        }
        let key = self.keys.get(provider.id()).ok_or_else(|| no_key(provider.name()))?.to_string();
        provider.validate_key(&key)?;
        self.ws.try_type_usage(kind, &t)?;
        let model = provider.describe(&key, &t.choice.model)?;
        if !model.kinds.is_empty() && !model.kinds.contains(&kind) {
            return Err(ToolError::new(ErrorCode::BadSettings, format!("The model \"{}\" at {} doesn't make {many}.", model.name, provider.name()))
                .with_remediation(format!("Ask the person to choose another model for {many} {CHOOSE_AGAIN}.")));
        }
        Ok(Ready { kind, t, provider, key, model })
    }
}

fn unit_words(unit: Unit) -> &'static str {
    match unit {
        Unit::Usd => "dollars",
        Unit::Credits => "credits",
    }
}

/// One asset type, ready to run: the project's choice for it, its provider and key, and the model
/// as the provider describes it now.
struct Ready {
    kind: AssetKind,
    t: TypeAssets,
    provider: Arc<dyn AssetProvider>,
    key: String,
    model: ModelDetail,
}

impl Ready {
    /// The type as its limit is checked. It is tracked when the price the person was shown, or the
    /// price the provider gives now, is one to count with: a model whose price became known is
    /// limited, and one whose price can no longer be read is not quietly let off its limit.
    fn limited(&self) -> TypeAssets {
        let mut t = self.t.clone();
        if !t.choice.pricing.tracked() {
            t.choice.pricing = self.model.pricing.clone();
        }
        t
    }

    /// What `req` will cost before it runs, in the type's unit; `None` when only the run can say.
    fn estimate(&self, req: &AssetRequest) -> Result<Option<f64>, ToolError> {
        let estimate = self.provider.estimate(&self.key, &self.model, req)?;
        if let Some(e) = estimate {
            valid_cost(e)?;
        }
        Ok(estimate)
    }

    /// What this type's receipts say besides where each went and what it cost.
    fn receipt<'a>(&'a self, from: &'a str) -> Receipt<'a> {
        Receipt { kind: self.kind, from, provider: self.provider.id(), model: &self.t.choice.model, unit: self.t.choice.unit }
    }
}

/// What the ledger says of one generation besides where it went and what it cost: the project's
/// choice for the type, so it names the model the project chose and counts in the type's unit.
struct Receipt<'a> {
    kind: AssetKind,
    /// What it was made from: the description, or the picture it was made from.
    from: &'a str,
    provider: &'a str,
    model: &'a str,
    unit: Unit,
}

impl Receipt<'_> {
    /// The ledger row for `rel`; no amount when the provider gave no cost that can be believed.
    fn record(&self, rel: &str, amount: Option<f64>) -> AssetRecord {
        AssetRecord {
            file: rel.to_string(),
            kind: self.kind.ledger_name().into(),
            prompt: self.from.chars().take(1000).collect(),
            provider: self.provider.into(),
            model: self.model.into(),
            amount,
            unit: Some(self.unit),
            cost_missing: amount.is_none(),
            at_ms: crate::inbox::now_ms(),
        }
    }

    /// What the provider may have charged, in words.
    fn charge(&self, amount: Option<f64>) -> String {
        match amount {
            Some(a) => exact(self.unit, a),
            None => "an amount it did not say".into(),
        }
    }
}

/// Go ahead if the type's limit allows this generation, else ask the person (unless they said
/// "always" for this type). Usage is the type's own, under its provider and in its unit, from a
/// checked ledger read. A limit applies when it is switched on, the project doesn't ignore the
/// budget and the type's price is one to count with. With an estimate, usage plus the estimate
/// must stay within the limit; without one (a cost known only after the run) a generation may
/// start while usage is under the limit, and so may pass it once. A generation whose cost could not
/// be read leaves usage unknown, so every later one asks until the person allows it.
fn within_limit(ctx: &mut Ctx<'_>, inbox: &Inbox, ws: &mut Workspace, kind: AssetKind, t: &TypeAssets, estimate: Option<f64>) -> Result<(), ToolError> {
    let usage = ws.try_type_usage(kind, t)?;
    valid_cost(usage.amount)?;
    if let Some(e) = estimate {
        valid_cost(e)?;
        valid_cost(usage.amount + e)?;
    }
    let limited = t.limit_on && !ws.manifest.assets.ignore_budget && t.choice.pricing.tracked();
    if !limited {
        return Ok(());
    }
    valid_cost(t.limit)?;
    let approval = over_limit_approval(kind);
    if ws.manifest.allowed_actions.contains(&approval) {
        return Ok(());
    }
    let within = match estimate {
        Some(e) => usage.amount + e <= t.limit + EPSILON,
        None => usage.amount + EPSILON < t.limit,
    };
    if within && !usage.cost_missing {
        return Ok(());
    }
    let (many, one) = type_words(kind);
    let unit = t.choice.unit;
    let (limit, used) = (amount(unit, t.limit), amount(unit, usage.amount));
    let stop = |message: String| {
        ToolError::new(ErrorCode::OverLimit, message).with_remediation(format!(
            "Use simple shapes, colours and the assets already made for the rest. The person can raise or switch off the limit for {many}, or ignore the budget, {CHOOSE_AGAIN}."
        ))
    };
    // Auto mode doesn't stop to ask, and doesn't spend the person's money past a limit unasked.
    if ctx.session.settings.auto_mode {
        return Err(stop(if usage.cost_missing {
            format!("The cost of an earlier {one} could not be read, so this project's {limit} limit for {many} can't be checked ({used} counted so far).")
        } else {
            format!("This project's {limit} limit for {many} doesn't cover another {one} ({used} used so far).")
        }));
    }
    let title = match estimate {
        Some(e) => format!("Spend about {} more on {many}?", amount(unit, e)),
        None => format!("Make another {one} past this project's limit for {many}?"),
    };
    let mut detail = if usage.cost_missing {
        format!(
            "The cost of an earlier {one} could not be read, so how much of this project's {limit} limit for {many} is used is not known ({used} counted so far)."
        )
    } else {
        format!("This project's {limit} limit for {many} is reached ({used} used so far).")
    };
    if estimate.is_none() {
        detail.push_str(" This model's cost is known only after it runs.");
    }
    detail.push_str(&format!(" Allow once for this {one}, or always to let the builder keep generating {many} for this project."));
    match ask_approval(ctx, inbox, Some(ws), &title, &detail, &approval)? {
        Verdict::Yes => Ok(()),
        Verdict::No(_) | Verdict::NoAnswer => Err(stop(format!("The limit for {many} is reached and the person didn't allow more."))),
    }
}

/// Run one paid stage, record it and save its output. When the provider fails after it accepted
/// the work and reports a charge, that charge is recorded first, so a retry is checked against the
/// limit it already used. An overrun of the estimate is reported only after the receipt and the
/// output are kept.
fn make(ws: &Workspace, ready: &Ready, req: &AssetRequest<'_>, path: &Path, rel: &str, from: &str, estimate: Option<f64>) -> Result<Generated, ToolError> {
    let receipt = ready.receipt(from);
    let g = ready.provider.generate(&ready.key, &ready.model, req).map_err(|e| record_charge(ws, &receipt, ready.kind, rel, e))?;
    keep(ws, path, rel, &receipt, &g)?;
    within_estimate(rel, g.cost, estimate, ready.t.choice.unit)?;
    Ok(g)
}

/// Record the charge a failed generation reported, and say so in its error. An invalid or
/// unrecordable charge is reported instead of being dropped; a charge of unknown cost is recorded
/// as a generation without an amount.
fn record_charge(ws: &Workspace, receipt: &Receipt<'_>, kind: AssetKind, rel: &str, error: ToolError) -> ToolError {
    if !was_charged(&error) {
        return error;
    }
    let amount = charged_amount(&error);
    let (many, _) = type_words(kind);
    let recorded = || {
        if let Some(a) = amount {
            valid_cost(a)?;
        }
        ws.record_asset(receipt.record(rel, amount))
    };
    match recorded() {
        Ok(()) => {
            let note = format!(
                "The charge already counts against the project's limit for {many}. Do not blindly regenerate or retry: check the provider's job and account first."
            );
            let remediation = Some(error.remediation.as_deref().map_or_else(|| note.clone(), |r| format!("{r} {note}")));
            let said = match amount {
                Some(_) => format!("this was recorded as spending for {rel}, which was not saved."),
                None => format!("this was recorded as spending of unknown amount for {rel}, which was not saved."),
            };
            ToolError { message: format!("{} The provider may have charged {}; {said}", error.message, receipt.charge(amount)), remediation, ..error }
        }
        Err(e) => {
            let mut failed = ToolError::new(
                e.code,
                format!("{} The provider may have charged {}, but the charge could not be recorded: {}", error.message, receipt.charge(amount), e.message),
            )
            .with_remediation("Do not blindly regenerate or retry: check the provider charge and repair the asset ledger first.");
            failed.details = error.details;
            failed
        }
    }
}

/// Record a paid generation before preparing or saving its output. A failed save keeps the receipt;
/// a failed receipt leaves the destination untouched. These two files are not a crash-atomic transaction.
fn keep(ws: &Workspace, path: &Path, rel: &str, receipt: &Receipt<'_>, g: &Generated) -> Result<(), ToolError> {
    if let Some(cost) = g.cost {
        valid_cost(cost)?;
    }
    let charged = receipt.charge(g.cost);
    ws.record_asset(receipt.record(rel, g.cost)).map_err(|e| {
        ToolError::new(e.code, format!("Asset spending could not be recorded; the provider may have charged {charged}. Output was not saved: {}", e.message))
            .with_remediation("Do not blindly regenerate or retry: check the provider charge and repair the asset ledger first.")
    })?;
    let save = || {
        confined(&ws.root, &ws.app_dir())?;
        confined(&ws.app_dir(), path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ToolError::io(parent.display(), e))?;
        }
        confined(&ws.app_dir(), path)?;
        not_a_record(ws, path)?;
        crate::project::write_atomic(path, &g.bytes)
    };
    save().map_err(|e| {
        ToolError::new(e.code, format!("Asset spending of {charged} was recorded, but saving {rel} failed: {}", e.message)).with_remediation(
            "Do not blindly regenerate or retry: the generation is already recorded as spending. Check the destination and provider output first.",
        )
    })
}

/// Report an overrun only after preserving both the charge and the generated output.
fn within_estimate(saved: &str, actual: Option<f64>, estimate: Option<f64>, unit: Unit) -> Result<(), ToolError> {
    if let (Some(actual), Some(estimate)) = (actual, estimate)
        && actual > estimate + EPSILON
    {
        return Err(ToolError::new(
            ErrorCode::OverLimit,
            format!("Saved {saved} and recorded {}, more than the {} estimated before it ran.", exact(unit, actual), exact(unit, estimate)),
        )
        .with_remediation("Do not blindly regenerate or retry: reuse the saved asset and review the recorded spending before continuing."));
    }
    Ok(())
}

/// The result of a saved generation: what it cost and what the type has used of its limit, from a
/// checked ledger read (an unreadable ledger is never shown as an empty one), and the options of the
/// request the model ignored.
fn summary(ws: &Workspace, ready: &Ready, saved: &str, made: &Generated, extra: Value) -> Result<ToolOutput, ToolError> {
    let unit = ready.t.choice.unit;
    let usage: TypeUsage = ws.try_type_usage(ready.kind, &ready.t).map_err(|e| {
        let (many, _) = type_words(ready.kind);
        ToolError::new(
            e.code,
            format!("Saved {saved} and recorded {}, but the usage of {many} could not be read: {}", ready.receipt("").charge(made.cost), e.message),
        )
        .with_remediation("Do not blindly regenerate or retry: reuse the saved asset and check the asset ledger before continuing.")
    })?;
    let t = ready.limited();
    let limited = t.limit_on && !ws.manifest.assets.ignore_budget && t.choice.pricing.tracked();
    let mut v = json!({
        "saved": saved,
        "kind": ready.kind.id(),
        "provider": ready.provider.id(),
        "model": t.choice.model,
        "cost": made.cost.map(round),
        "unit": unit,
        "used": round(usage.amount),
        "generations": usage.generations,
        "cost_missing": usage.cost_missing,
        "limit": limited.then(|| round(t.limit)),
        "left": limited.then(|| round((t.limit - usage.amount).max(0.0))),
        "ignored": made.ignored,
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    Ok(ToolOutput::Value(v))
}

/// An amount as a result reports it: to a hundredth of a cent or credit.
fn round(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn prompt_ok(p: &str) -> Result<&str, ToolError> {
    let p = p.trim();
    if p.is_empty() || p.chars().count() > 2000 {
        return Err(ToolError::invalid("prompt must be 1..=2000 characters"));
    }
    Ok(p)
}

/// Validate the destination, the project's picture choice and its limit before saving a PNG.
fn assets_image_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ImageParams = parse_params(params)?;
    let prompt = prompt_ok(&p.prompt)?;
    let shape = match p.shape.as_deref() {
        None => Shape::Square,
        Some(s) => Shape::parse(s).ok_or_else(|| ToolError::invalid("shape must be square, wide or tall"))?,
    };
    generate_one(ctx, &p.save_as, "png", prompt, AssetRequest::Image { prompt, shape }, "Making a picture…")
}

/// A stand-in for the picture a 3D model will be made from, which doesn't exist yet when the
/// model's cost is asked for: one transparent pixel, inline, so a provider that quotes the very
/// request has a real picture to look at.
const PREFLIGHT_PICTURE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/// What a 3D model is made from.
enum Start {
    /// A picture given: a web address, or a picture of the project sent inline.
    Given(String),
    /// A picture made first from the description, with the project's choice for pictures.
    Made(Box<Ready>),
    /// The description itself, for a model that takes words.
    Words,
}

/// Generate a GLB from a picture, from a picture made first, or from the description. Every
/// participating type's choice, key and limit is checked before the first paid call.
fn assets_model_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ModelParams = parse_params(params)?;
    let textured = p.textured.unwrap_or(true);
    let faces = p.faces.unwrap_or(DEFAULT_FACES);
    if faces == 0 || faces > MAX_FACES {
        return Err(ToolError::invalid(format!("faces must be 1 to {MAX_FACES}")));
    }
    let mut j = job(ctx)?;
    let (path, rel) = save_path(&j.ws, &p.save_as, "glb")?;
    let image = p.image.trim();
    let prompt = if image.is_empty() { Some(prompt_ok(&p.prompt)?) } else { None };
    // A picture given is checked (and read, when it is the project's) before the provider is asked anything.
    let given = if image.is_empty() { None } else { Some(image_source(&j.ws, image)?) };
    let solid = j.ready(AssetKind::Model3d)?;
    let request = |image_url: Option<&'static str>, words: Option<&'static str>| AssetRequest::Model3d { image_url, prompt: words, textured, faces };
    let start = match (given, prompt) {
        (Some(url), _) => Start::Given(url),
        (None, None) => return Err(ToolError::invalid("give a prompt or an image")),
        (None, Some(prompt)) => {
            let roles = InputRoles::built_in();
            let takes_picture = map_request(&solid.model.fields, &request(Some(PREFLIGHT_PICTURE), None), roles).is_ok();
            let words = AssetRequest::Model3d { image_url: None, prompt: Some(prompt), textured, faces };
            let takes_words = map_request(&solid.model.fields, &words, roles);
            let pictures_on = j.ws.manifest.asset_type_on(AssetKind::Image, &j.settings).is_some();
            match (takes_picture, pictures_on, takes_words) {
                (true, true, _) => Start::Made(Box::new(j.ready(AssetKind::Image)?)),
                (_, _, Ok(_)) => Start::Words,
                (true, false, Err(_)) => {
                    return Err(ToolError::new(
                        ErrorCode::NoPermission,
                        "The project's 3D model starts from a picture, and generated pictures are off for this project.",
                    )
                    .with_remediation(format!(
                        "Pass a picture as image (a web address, or a picture inside the project), or ask the person to choose a provider and model for pictures {CHOOSE_AGAIN}."
                    )));
                }
                (false, _, Err(e)) => return Err(e),
            }
        }
    };
    let words = prompt.unwrap_or_default();
    // The picture made first is saved beside the model; its place is checked before anything is spent.
    let picture_place = match &start {
        Start::Made(_) => Some(save_path(&j.ws, &format!("{}.png", rel.trim_end_matches(".glb").trim_end_matches(".GLB")), "png")?),
        _ => None,
    };
    // The same expanded description is quoted and sent: a provider may price its length.
    let described =
        format!("{words}. One whole object, centred and fully in view, on a plain light-grey background, soft even lighting, three-quarter view, no text.");
    let picture_req = AssetRequest::Image { prompt: &described, shape: Shape::Square };
    let model_estimate = match &start {
        Start::Given(url) => solid.estimate(&AssetRequest::Model3d { image_url: Some(url), prompt: None, textured, faces })?,
        // The picture isn't made yet; what a model costs doesn't depend on it.
        Start::Made(_) => solid.estimate(&request(Some(PREFLIGHT_PICTURE), None))?,
        Start::Words => solid.estimate(&AssetRequest::Model3d { image_url: None, prompt: Some(words), textured, faces })?,
    };
    let picture_estimate = match &start {
        Start::Made(pictures) => pictures.estimate(&picture_req)?,
        _ => None,
    };
    // Both limits are checked before the first paid call.
    if let Start::Made(pictures) = &start {
        within_limit(ctx, &j.inbox, &mut j.ws, AssetKind::Image, &pictures.limited(), picture_estimate)?;
    }
    within_limit(ctx, &j.inbox, &mut j.ws, AssetKind::Model3d, &solid.limited(), model_estimate)?;
    let mut picture = Value::Null;
    let (url, made_from): (Option<String>, String) = match (start, picture_place) {
        (Start::Given(url), _) => (Some(url), format!("from {image}")),
        (Start::Made(pictures), Some((pic_path, pic_rel))) => {
            ctx.report(None, "Making a picture of it first…");
            let g = make(&j.ws, &pictures, &picture_req, &pic_path, &pic_rel, words, picture_estimate)?;
            picture = json!({"saved": pic_rel, "provider": pictures.provider.id(), "model": pictures.t.choice.model, "cost": g.cost.map(round), "unit": pictures.t.choice.unit});
            // A provider that sends the picture back without an address: it goes inline.
            let url = if g.url.is_empty() { fal::data_uri(&g.bytes, "image/png") } else { g.url };
            (Some(url), words.to_string())
        }
        (Start::Made(_), None) => return Err(ToolError::internal("the picture made first has no place to be saved")),
        (Start::Words, _) => (None, words.to_string()),
    };
    // From a picture, the picture alone, as before; from words, the words.
    let model_req = AssetRequest::Model3d { image_url: url.as_deref(), prompt: url.is_none().then_some(words), textured, faces };
    ctx.report(None, "Turning it into a 3D model (a minute or two)…");
    let made = make(&j.ws, &solid, &model_req, &path, &rel, &made_from, model_estimate)?;
    summary(&j.ws, &solid, &rel, &made, json!({"picture": picture, "textured": textured, "faces": faces}))
}

/// Where to save inside app/: (full path, path relative to app/). `ext` is the required extension.
fn save_path(ws: &Workspace, save_as: &str, ext: &str) -> Result<(PathBuf, String), ToolError> {
    let app = ws.app_dir();
    // Paths given from the workspace ("app/Assets/x.glb") mean the same place.
    let rel = save_as.trim().trim_start_matches("./").trim_start_matches("app/").trim_start_matches("app\\");
    // Windows drops trailing dots and spaces, reads ':' as a data stream and "NAME~1" as a short
    // name, so "reference./x.png" or "REFERE~1/x.png" could reach a record by another spelling.
    // Refuse those spellings everywhere.
    let short_name = |part: &str| part.split('~').skip(1).any(|after| after.starts_with(|c: char| c.is_ascii_digit()));
    if rel.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").any(|part| part.ends_with(['.', ' ']) || part.contains(':') || short_name(part)) {
        return Err(ToolError::invalid("save_as parts must not end in a dot or space, or contain ':' or a short-name '~1'"));
    }
    let path = resolve(&app, rel).filter(|p| inside(&app, p) && *p != app).ok_or_else(|| ToolError::invalid("save_as must be a file path inside app/"))?;
    confined(&ws.root, &app)?;
    confined(&app, &path)?;
    not_a_record(ws, &path)?;
    if path.is_dir() {
        return Err(ToolError::invalid("save_as must name a file, not a directory"));
    }
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
        return Err(ToolError::invalid(format!("save_as must end in .{ext}")));
    }
    let shown = path.strip_prefix(&app).unwrap_or(&path).display().to_string().replace('\\', "/");
    Ok((path, shown))
}

/// The picture a model is made from: a web address as it is, or a picture in the project sent
/// inline.
fn image_source(ws: &Workspace, image: &str) -> Result<String, ToolError> {
    if image.starts_with("https://") || image.starts_with("http://") {
        return Ok(image.to_string());
    }
    let path = resolve(&ws.root, image)
        .filter(|p| inside(&ws.root, p))
        .ok_or_else(|| ToolError::invalid("image must be a web address or a picture inside the project"))?;
    confined(&ws.root, &path)?;
    let media = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => return Err(ToolError::invalid("image must be a PNG, JPEG or WebP picture")),
    };
    let bytes = read_asset(&path)?;
    Ok(fal::data_uri(&bytes, media))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_choice::AssetChoice;
    use crate::inbox::{Answer, InboxDir};
    use crate::workspace::TargetRef;
    use craft_anything_engines::assets::inputs::{FieldKind, InputField};
    use craft_anything_engines::assets::model::{AssetOption, ModelList, Pricing, Rate};
    use craft_anything_engines::assets::{charged_or_unknown, estimate_from};
    use craft_anything_platform::sync::Mutex;
    use std::collections::BTreeMap;
    use std::time::Duration;

    /// A game whose person allows generated assets. Every type is Off until a test chooses it.
    fn ws() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Workspace::create(dir.path(), "game", "A snake game", TargetRef::None).unwrap();
        w.set_person(|m| m.generate_assets = true).unwrap();
        (dir, w)
    }

    /// The bytes of a version 1 `assets.json` holding `records`.
    fn ledger_of(records: &[AssetRecord]) -> Vec<u8> {
        serde_json::to_vec(&json!({"version": 1, "records": records})).unwrap()
    }

    /// A spoken line made at ElevenLabs, as a receipt records it.
    fn speech() -> Receipt<'static> {
        Receipt { kind: AssetKind::Speech, from: "hi", provider: "elevenlabs", model: "eleven_v3", unit: Unit::Credits }
    }

    /// A dollar receipt written by hand, to stand for spending already in the ledger.
    fn receipt(file: &str, kind: &str, cost: f64) -> AssetRecord {
        AssetRecord {
            file: file.into(),
            kind: kind.into(),
            prompt: "x".into(),
            provider: "fal".into(),
            model: "flux-dev".into(),
            amount: Some(cost),
            unit: Some(Unit::Usd),
            cost_missing: false,
            at_ms: 0,
        }
    }

    fn price(unit: Unit, x: f64) -> Pricing {
        Pricing::Before { unit, rate: Rate::PerRequest(x) }
    }

    fn input(name: &str, kind: FieldKind, required: bool) -> InputField {
        InputField { name: name.into(), kind, required, has_default: false }
    }

    fn detail(id: &str, kind: AssetKind, fields: Vec<InputField>, pricing: Pricing) -> ModelDetail {
        ModelDetail { id: id.into(), name: id.into(), kinds: vec![kind], fields, pricing, unit_price: None }
    }

    /// A picture model that makes every shape.
    fn picture_model(id: &str, pricing: Pricing) -> ModelDetail {
        let sizes = FieldKind::Choice(vec!["square_hd".into(), "landscape_16_9".into(), "portrait_16_9".into()]);
        detail(id, AssetKind::Image, vec![input("prompt", FieldKind::Text, true), input("image_size", sizes, false)], pricing)
    }

    /// A 3D model that starts from a picture, from words, or either.
    fn solid_model(id: &str, picture: bool, words: bool, pricing: Pricing) -> ModelDetail {
        let mut fields = vec![input("face_limit", FieldKind::Integer { min: Some(48), max: Some(20_000) }, false), input("texture", FieldKind::Bool, false)];
        if picture {
            fields.push(input("image_url", FieldKind::Text, !words));
        }
        if words {
            fields.push(input("prompt", FieldKind::Text, !picture));
        }
        detail(id, AssetKind::Model3d, fields, pricing)
    }

    fn audio_model(id: &str, kind: AssetKind, pricing: Pricing) -> ModelDetail {
        let fields = match kind {
            AssetKind::Speech => vec![input("text", FieldKind::Text, true), input("voice_id", FieldKind::Text, false)],
            AssetKind::SoundEffect => vec![input("text", FieldKind::Text, true), input("duration_seconds", FieldKind::Number { min: None, max: None }, false)],
            _ => vec![
                input("prompt", FieldKind::Text, true),
                input("music_length_ms", FieldKind::Integer { min: None, max: None }, false),
                input("force_instrumental", FieldKind::Bool, false),
            ],
        };
        detail(id, kind, fields, pricing)
    }

    /// One request a stand-in was asked to run.
    #[derive(Debug, Clone, PartialEq)]
    struct Call {
        kind: AssetKind,
        model: String,
        picture: Option<String>,
        words: Option<String>,
    }

    /// Stands in for a provider: describes the models it holds, records each estimate and run,
    /// and answers with a few bytes. What it estimates, what a run costs and whether a run fails
    /// after it was accepted can be set; otherwise they follow the model's price.
    struct Recording {
        id: &'static str,
        unit: Unit,
        models: BTreeMap<String, ModelDetail>,
        /// What every estimate says, instead of the model's price.
        estimate: Option<Option<f64>>,
        /// What every run costs, instead of the model's price.
        cost: Option<Option<f64>>,
        /// Every run fails after it was accepted, with this charge.
        fails_charging: Option<Option<f64>>,
        /// A project whose spending lock is looked at during each run: whether it was held.
        probe: Option<(PathBuf, Mutex<Option<bool>>)>,
        calls: Mutex<Vec<Call>>,
        estimates: Mutex<Vec<AssetKind>>,
        described: Mutex<Vec<String>>,
    }

    impl Recording {
        /// The provider `id`, billing in `unit`, with one model of each type (`pic`, `solid`
        /// (from a picture), `words-3d`, `both-3d`, `speech`, `sound`, `music`) at `per_run` each.
        fn new(id: &'static str, unit: Unit, per_run: f64) -> Recording {
            let p = price(unit, per_run);
            let models = [
                picture_model("pic", p.clone()),
                solid_model("solid", true, false, p.clone()),
                solid_model("words-3d", false, true, p.clone()),
                solid_model("both-3d", true, true, p.clone()),
                audio_model("speech", AssetKind::Speech, p.clone()),
                audio_model("sound", AssetKind::SoundEffect, p.clone()),
                audio_model("music", AssetKind::Music, p),
            ];
            Recording {
                id,
                unit,
                models: models.into_iter().map(|m| (m.id.clone(), m)).collect(),
                estimate: None,
                cost: None,
                fails_charging: None,
                probe: None,
                calls: Mutex::default(),
                estimates: Mutex::default(),
                described: Mutex::default(),
            }
        }

        /// Hold `model` as well (or instead of the one of its id).
        fn with(mut self, model: ModelDetail) -> Recording {
            self.models.insert(model.id.clone(), model);
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn kinds_run(&self) -> Vec<AssetKind> {
            self.calls().into_iter().map(|c| c.kind).collect()
        }

        fn nothing_asked(&self) -> bool {
            self.calls.lock().unwrap().is_empty() && self.estimates.lock().unwrap().is_empty()
        }
    }

    impl AssetProvider for Recording {
        /// Use the configured registry id so tests can replace any built-in provider.
        fn id(&self) -> &'static str {
            self.id
        }
        /// Use the registry id as the stand-in provider's display name in errors.
        fn name(&self) -> &'static str {
            self.id
        }
        fn unit(&self) -> Unit {
            self.unit
        }
        fn list_models(&self, _key: Option<&str>, _kind: AssetKind) -> Result<ModelList, ToolError> {
            Ok(ModelList { models: Vec::new(), error: None })
        }
        /// The model it holds of that id, else the error a provider gives for a model it no longer serves.
        fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
            assert_eq!(key, "sk-test");
            self.described.lock().unwrap().push(model.to_string());
            self.models.get(model).cloned().ok_or_else(|| {
                ToolError::new(ErrorCode::BadSettings, format!("The model \"{model}\" is no longer available at {}.", self.id))
                    .with_remediation("Choose another model for this type of asset in the project's Generated Assets settings.")
            })
        }
        /// Record quote requests, refuse a request the model can't take, and return the injected amount or the model's price.
        fn estimate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Option<f64>, ToolError> {
            assert_eq!(key, "sk-test");
            self.estimates.lock().unwrap().push(req.kind());
            map_request(&model.fields, req, InputRoles::built_in())?;
            Ok(self.estimate.unwrap_or_else(|| estimate_from(&model.pricing, req)))
        }
        /// Require the test key, record the request and return fixed bytes without network access.
        fn generate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Generated, ToolError> {
            assert_eq!(key, "sk-test");
            let mapped = map_request(&model.fields, req, InputRoles::built_in())?;
            let (picture, words) = match *req {
                AssetRequest::Model3d { image_url, prompt, .. } => (image_url.map(str::to_string), prompt.map(str::to_string)),
                AssetRequest::Image { prompt, .. } => (None, Some(prompt.to_string())),
                _ => (None, None),
            };
            self.calls.lock().unwrap().push(Call { kind: req.kind(), model: model.id.clone(), picture, words });
            if let Some((root, held)) = &self.probe {
                // A second handle stands for another process sharing the project.
                *held.lock().unwrap() = Some(Workspace::open(root).unwrap().lock_spending(Duration::ZERO).is_err());
            }
            let cost = self.cost.unwrap_or_else(|| estimate_from(&model.pricing, req));
            if let Some(charge) = self.fails_charging {
                let failed = ToolError::new(ErrorCode::ProgramFailed, "The job failed.").with_remediation("Check the job.");
                return Err(charged_or_unknown(failed, charge));
            }
            Ok(Generated { url: "https://stand-in/picture.png".into(), bytes: b"generated".to_vec(), cost, ignored: mapped.ignored })
        }
    }

    /// Choose `model` at `provider` for `kind` in the project, with its limit on.
    fn choose(w: &mut Workspace, kind: AssetKind, provider: &str, model: &str, unit: Unit, pricing: Pricing, limit: f64) {
        let choice = AssetChoice { provider: Some(provider.into()), model: model.into(), model_name: model.into(), unit, pricing };
        w.set_person(|m| {
            m.assets.types.insert(kind, TypeAssets { choice, limit, limit_on: true, since_ms: 0 });
        })
        .unwrap();
    }

    /// The model of each type that `Recording::new` holds.
    fn model_for(kind: AssetKind) -> &'static str {
        match kind {
            AssetKind::Image => "pic",
            AssetKind::Model3d => "solid",
            AssetKind::Speech => "speech",
            AssetKind::SoundEffect => "sound",
            AssetKind::Music => "music",
            other => panic!("no stand-in model for {other:?}"),
        }
    }

    /// Choose `stub`'s model of every type in the project, priced as `stub` prices it, with the
    /// unit's default limit.
    fn choose_all(w: &mut Workspace, stub: &Recording) {
        let limit = AssetSettings::default().default_limit(stub.unit);
        for kind in AssetKind::ALL {
            let pricing = stub.models[model_for(kind)].pricing.clone();
            choose(w, kind, stub.id, model_for(kind), stub.unit, pricing, limit);
        }
    }

    /// An auto-mode engine whose keyed providers are the given stand-ins.
    fn engine_with(w: &Workspace, stand_ins: Vec<Arc<dyn AssetProvider>>) -> crate::Engine {
        let mut providers = AssetProviders::built_in();
        let mut keys = BTreeMap::new();
        for p in stand_ins {
            keys.insert(p.id().to_string(), "sk-test".to_string());
            providers.register(p);
        }
        let mut session = crate::testing::session();
        session.settings.auto_mode = true;
        session.ext.insert(Arc::new(InboxDir(w.inbox_dir())));
        session.ext.insert(Arc::new(AssetKeys(keys)));
        session.ext.insert(Arc::new(providers));
        crate::Engine::new(session)
    }

    /// An auto-mode engine with one keyed recording provider, chosen for every type in the project.
    fn recording_engine(w: &mut Workspace, stub: Recording) -> (crate::Engine, Arc<Recording>) {
        choose_all(w, &stub);
        let stub = Arc::new(stub);
        (engine_with(w, vec![stub.clone()]), stub)
    }

    fn call(engine: &crate::Engine, tool: &str, params: Value) -> Result<Value, ToolError> {
        match engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp))? {
            ToolOutput::Value(v) => Ok(v),
            other => panic!("{other:?}"),
        }
    }

    /// Provide valid speech, sound and music requests with distinct destinations and expected request kinds.
    fn audio_calls() -> Vec<(&'static str, Value, AssetKind)> {
        vec![
            ("assets_speech_generate", json!({"text":"A new adventure", "voice_id":"voice-test", "save_as":"audio/speech.mp3"}), AssetKind::Speech),
            ("assets_sound_generate", json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"audio/chime.mp3"}), AssetKind::SoundEffect),
            ("assets_music_generate", json!({"prompt":"A calm piano theme", "duration_seconds":3.0, "save_as":"audio/theme.mp3"}), AssetKind::Music),
        ]
    }

    /// Every tool, one call each, with what it makes.
    fn every_call() -> Vec<(&'static str, Value, AssetKind)> {
        let mut calls = audio_calls();
        calls.push(("assets_image_generate", json!({"prompt":"a crate", "save_as":"crate.png"}), AssetKind::Image));
        calls.push(("assets_model_generate", json!({"image":"https://example.invalid/source.png", "save_as":"crate.glb"}), AssetKind::Model3d));
        calls
    }

    /// An over-limit question answered on another thread, once it is asked.
    fn answer_when_asked(w: &Workspace, allow: bool, always: bool, title_has: &'static str) -> std::thread::JoinHandle<()> {
        let inbox = Inbox::new(w.inbox_dir());
        std::thread::spawn(move || {
            for _ in 0..200 {
                if let Some(r) = inbox.pending().first() {
                    assert!(r.title.contains(title_has), "{}", r.title);
                    inbox.answer(&r.id, &Answer { allow: Some(allow), always, ..Default::default() }).unwrap();
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            panic!("no question came");
        })
    }

    /// A session outside Auto mode, which asks the person.
    fn asking() -> crate::Session {
        let mut session = crate::testing::session();
        session.settings.auto_mode = false;
        session
    }

    fn ctx(session: &mut crate::Session) -> Ctx<'_> {
        Ctx { tool: "test", session, cancel: crate::CancelToken::default(), progress: &|_| {}, caller: crate::Caller::Mcp }
    }

    #[test]
    fn saved_inside_app_with_the_right_ending() {
        let (_d, w) = ws();
        let (p, rel) = save_path(&w, "Assets/Models/crate.glb", "glb").unwrap();
        assert_eq!(rel, "Assets/Models/crate.glb");
        assert!(p.starts_with(w.app_dir()));
        assert_eq!(save_path(&w, "app/Assets/sky.png", "png").unwrap().1, "Assets/sky.png", "paths from the workspace mean the same place");
        assert!(save_path(&w, "Assets/crate.png", "glb").is_err(), "the ending must match");
        assert!(save_path(&w, "../../evil.glb", "glb").is_err(), "nothing outside app/");
        assert!(image_source(&w, "../../../secret.png").is_err(), "pictures come from the project or the web");
        assert_eq!(image_source(&w, "https://v3.fal.media/x.png").unwrap(), "https://v3.fal.media/x.png");
    }

    /// The builder no longer chooses models: the tools take none, and their words name no model and no price.
    #[test]
    fn the_tools_take_no_model_and_name_the_projects_choice() {
        for spec in specs() {
            let schema = serde_json::to_string(&(spec.input_schema)()).unwrap();
            assert!(!schema.contains("\"model\"") && !schema.contains("image_model"), "{}: {schema}", spec.id);
            assert!(spec.summary.contains("the project chose") && spec.summary.contains("limit"), "{}: {}", spec.id, spec.summary);
            assert!(!spec.summary.contains('$') && !spec.summary.contains("override"), "{}: {}", spec.id, spec.summary);
        }
    }

    /// Each type is made with the model the project chose for it, recorded with that model and the
    /// type's unit; a `model` parameter is refused as unknown.
    #[test]
    fn each_type_uses_the_projects_choice() {
        let (_d, mut w) = ws();
        let stub = Recording::new("scenario", Unit::Credits, 20.0).with(picture_model("other-pic", price(Unit::Credits, 20.0)));
        let (engine, stub) = recording_engine(&mut w, stub);
        choose(&mut w, AssetKind::Image, "scenario", "other-pic", Unit::Credits, price(Unit::Credits, 20.0), 500.0);
        let v = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap();
        assert_eq!(stub.calls(), [Call { kind: AssetKind::Image, model: "other-pic".into(), picture: None, words: Some("a crate".into()) }]);
        assert_eq!(
            (v["kind"].as_str(), v["provider"].as_str(), v["model"].as_str(), v["unit"].as_str()),
            (Some("image"), Some("scenario"), Some("other-pic"), Some("credits"))
        );
        assert_eq!((v["cost"].as_f64(), v["used"].as_f64(), v["limit"].as_f64(), v["left"].as_f64()), (Some(20.0), Some(20.0), Some(500.0), Some(480.0)));
        let records = w.try_assets().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].kind.as_str(), records[0].provider.as_str(), records[0].model.as_str()), ("image", "scenario", "other-pic"));
        assert_eq!((records[0].amount, records[0].unit, records[0].cost_missing), (Some(20.0), Some(Unit::Credits), false));
        // The builder can't choose another model.
        for (tool, params) in [
            ("assets_image_generate", json!({"prompt": "a crate", "save_as": "b.png", "model": "pic"})),
            ("assets_model_generate", json!({"prompt": "a crate", "save_as": "b.glb", "image_model": "pic"})),
            ("assets_sound_generate", json!({"prompt": "wind", "duration_seconds": 2.0, "save_as": "b.mp3", "model": "sound"})),
        ] {
            assert_eq!(call(&engine, tool, params).unwrap_err().code, ErrorCode::BadInput, "{tool}");
        }
        assert_eq!(stub.calls().len(), 1);
    }

    /// A type that is Off is refused before anything is asked, saying how to turn it on.
    #[test]
    fn an_off_type_is_refused_with_how_to_turn_it_on() {
        for (tool, params, kind) in every_call() {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
            w.set_person(|m| {
                m.assets.types.insert(kind, TypeAssets { choice: AssetChoice::off(), ..TypeAssets::default() });
            })
            .unwrap();
            let error = call(&engine, tool, params).unwrap_err();
            assert_eq!(error.code, ErrorCode::NoPermission, "{tool}");
            assert!(error.message.contains("off for this project"), "{}", error.message);
            assert!(error.remediation.as_deref().is_some_and(|r| r.contains("simple shapes") && r.contains("Generated Assets settings")), "{error:?}");
            assert!(stub.nothing_asked(), "{tool}");
            assert!(w.assets().is_empty() && Inbox::new(w.inbox_dir()).pending().is_empty());
        }
    }

    /// Each type has its own limit, in its provider's unit: a type near its limit asks, while another
    /// type of the same project with nothing used runs.
    #[test]
    fn limits_are_per_type_and_in_the_types_unit() {
        let (_d, mut w) = ws();
        let pictures = Recording::new("scenario", Unit::Credits, 2.0);
        let sounds = Recording::new("elevenlabs", Unit::Usd, 0.5).with(audio_model("sound", AssetKind::SoundEffect, price(Unit::Usd, 0.5)));
        choose(&mut w, AssetKind::Image, "scenario", "pic", Unit::Credits, price(Unit::Credits, 2.0), 500.0);
        choose(&mut w, AssetKind::SoundEffect, "elevenlabs", "sound", Unit::Usd, price(Unit::Usd, 0.5), 5.0);
        let earlier = AssetRecord { provider: "scenario".into(), unit: Some(Unit::Credits), ..receipt("old.png", "image", 499.0) };
        w.record_asset(earlier).unwrap();
        let (pictures, sounds) = (Arc::new(pictures), Arc::new(sounds));
        let engine = engine_with(&w, vec![pictures.clone(), sounds.clone()]);
        // 499 of 500 credits used, and a picture costs 2: Auto mode stops instead of asking.
        let error = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert_eq!(error.code, ErrorCode::OverLimit);
        assert!(error.message.contains("500 credits") && error.message.contains("pictures") && error.message.contains("499 credits"), "{}", error.message);
        assert!(pictures.calls().is_empty());
        // Sound effects have used nothing of their own five dollars: they run.
        let v = call(&engine, "assets_sound_generate", json!({"prompt": "wind", "duration_seconds": 2.0, "save_as": "wind.mp3"})).unwrap();
        assert_eq!((v["cost"].as_f64(), v["unit"].as_str(), v["left"].as_f64()), (Some(0.5), Some("usd"), Some(4.5)));
        assert_eq!(sounds.kinds_run(), [AssetKind::SoundEffect]);
        // Outside Auto mode, the person is asked in credits, and a "no" spends nothing.
        let mut session = asking();
        let inbox = Inbox::new(w.inbox_dir());
        let t = w.manifest.assets.types[&AssetKind::Image].clone();
        let asked = answer_when_asked(&w, false, false, "2 credits");
        assert_eq!(within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Image, &t, Some(2.0)).unwrap_err().code, ErrorCode::OverLimit);
        asked.join().unwrap();
        within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Image, &t, Some(1.0)).unwrap();
        assert!(inbox.pending().is_empty(), "exactly the limit is within it");
    }

    #[test]
    fn a_limit_asks_before_going_over_and_remembers_always_for_its_type() {
        let (_d, mut w) = ws();
        choose(&mut w, AssetKind::Model3d, "fal", "solid", Unit::Usd, price(Unit::Usd, 0.48), 5.0);
        choose(&mut w, AssetKind::Image, "fal", "pic", Unit::Usd, price(Unit::Usd, 0.48), 5.0);
        w.record_asset(receipt("a.glb", "model", 4.80)).unwrap();
        w.record_asset(receipt("a.png", "image", 4.80)).unwrap();
        let inbox = Inbox::new(w.inbox_dir());
        let models = w.manifest.assets.types[&AssetKind::Model3d].clone();
        let pictures = w.manifest.assets.types[&AssetKind::Image].clone();
        let mut session = crate::testing::session();
        // Auto mode stops at the limit without asking.
        session.settings.auto_mode = true;
        assert_eq!(within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Model3d, &models, Some(0.48)).unwrap_err().code, ErrorCode::OverLimit);
        assert!(inbox.pending().is_empty(), "no question in Auto mode");
        // Asking is what Auto mode switches off.
        session.settings.auto_mode = false;
        // Under the limit: no question.
        within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Model3d, &models, Some(0.03)).unwrap();
        assert!(inbox.pending().is_empty());
        // Over it: asked; "no" stops, "always" lifts the limit of this type for the project.
        let t = answer_when_asked(&w, false, false, "$0.48");
        assert_eq!(within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Model3d, &models, Some(0.48)).unwrap_err().code, ErrorCode::OverLimit);
        t.join().unwrap();
        let t = answer_when_asked(&w, true, true, "3D models");
        within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Model3d, &models, Some(0.48)).unwrap();
        t.join().unwrap();
        assert!(Workspace::open(&w.root).unwrap().manifest.allowed_actions.iter().any(|k| k == "assets:over-budget:model_3d"));
        within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Model3d, &models, Some(10.0)).unwrap();
        assert!(inbox.pending().is_empty(), "after \"always\" it doesn't ask again for 3D models");
        // Pictures have a limit of their own, which "always" for 3D models doesn't lift.
        let t = answer_when_asked(&w, false, false, "pictures");
        assert_eq!(within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Image, &pictures, Some(0.48)).unwrap_err().code, ErrorCode::OverLimit);
        t.join().unwrap();
        assert_eq!(w.try_assets().unwrap().len(), 2, "asking spends nothing");
    }

    /// "Ignore budget", and a type's limit switched off, never ask; what was used is still recorded.
    #[test]
    fn ignore_budget_and_a_limit_switched_off_never_ask() {
        for how in ["ignore budget", "limit off"] {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 2.0));
            w.set_person(|m| match how {
                "ignore budget" => m.assets.ignore_budget = true,
                _ => {
                    if let Some(t) = m.assets.types.get_mut(&AssetKind::Image) {
                        t.limit_on = false;
                    }
                }
            })
            .unwrap();
            for i in 0..4 {
                let v = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": format!("crate-{i}.png")})).unwrap();
                assert_eq!((v["limit"].clone(), v["left"].clone()), (Value::Null, Value::Null), "{how}: no limit applies");
            }
            assert_eq!(stub.calls().len(), 4, "{how}: eight dollars against a five-dollar limit, unasked");
            assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
            let t = w.manifest.assets.types[&AssetKind::Image].clone();
            assert_eq!(w.try_type_usage(AssetKind::Image, &t).unwrap(), TypeUsage { amount: 8.0, generations: 4, cost_missing: false }, "{how}");
        }
    }

    /// A model whose cost is known only after it runs may start while its type is under the limit,
    /// and so may pass it once; the next one is then over and asks.
    #[test]
    fn a_cost_known_after_the_run_may_pass_the_limit_once() {
        let (_d, mut w) = ws();
        let after = Pricing::After { unit: Unit::Usd };
        let stub =
            Recording { cost: Some(Some(0.4)), ..Recording::new("fal", Unit::Usd, 0.0).with(audio_model("sound", AssetKind::SoundEffect, after.clone())) };
        let (engine, stub) = recording_engine(&mut w, stub);
        choose(&mut w, AssetKind::SoundEffect, "fal", "sound", Unit::Usd, after, 5.0);
        w.record_asset(receipt("old.mp3", "sound", 4.9)).unwrap();
        let params = |i: u32| json!({"prompt": "wind", "duration_seconds": 2.0, "save_as": format!("wind-{i}.mp3")});
        let v = call(&engine, "assets_sound_generate", params(1)).unwrap();
        assert_eq!((v["cost"].as_f64(), v["used"].as_f64(), v["left"].as_f64()), (Some(0.4), Some(5.3), Some(0.0)));
        assert_eq!(w.try_assets().unwrap().last().and_then(|r| r.amount), Some(0.4));
        // Now over: Auto mode stops.
        assert_eq!(call(&engine, "assets_sound_generate", params(2)).unwrap_err().code, ErrorCode::OverLimit);
        assert_eq!(stub.calls().len(), 1);
        // Outside Auto mode, the person is asked, saying the cost is known only after the run.
        let mut session = asking();
        let inbox = Inbox::new(w.inbox_dir());
        let t = w.manifest.assets.types[&AssetKind::SoundEffect].clone();
        let asked = answer_when_asked(&w, false, false, "sound effect");
        assert!(within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::SoundEffect, &t, None).is_err());
        asked.join().unwrap();
    }

    /// Review focus: a generation of a tracked type whose cost can't be read is recorded without an
    /// amount, and every later one of that type asks (Auto mode stops) until the person allows it.
    #[test]
    fn an_unreadable_cost_stops_the_type_until_the_person_allows() {
        let (_d, mut w) = ws();
        let stub = Recording { cost: Some(None), ..Recording::new("fal", Unit::Usd, 0.1) };
        let (engine, stub) = recording_engine(&mut w, stub);
        let params = |i: u32| json!({"prompt": "a crate", "save_as": format!("crate-{i}.png")});
        let v = call(&engine, "assets_image_generate", params(1)).unwrap();
        assert_eq!((v["cost"].clone(), v["cost_missing"].as_bool()), (Value::Null, Some(true)));
        let record = w.try_assets().unwrap().pop().unwrap();
        assert_eq!((record.amount, record.unit, record.cost_missing), (None, Some(Unit::Usd), true));
        // Usage is now unknown: Auto mode stops instead of spending unchecked.
        let error = call(&engine, "assets_image_generate", params(2)).unwrap_err();
        assert_eq!(error.code, ErrorCode::OverLimit);
        assert!(error.message.contains("could not be read"), "{}", error.message);
        assert_eq!(stub.calls().len(), 1);
        // Another type is not stopped by it.
        call(&engine, "assets_sound_generate", json!({"prompt": "wind", "duration_seconds": 2.0, "save_as": "wind.mp3"})).unwrap();
        // Outside Auto mode the person is asked; allowing once lets this one through, and the next asks again.
        let mut session = asking();
        let inbox = Inbox::new(w.inbox_dir());
        let t = w.manifest.assets.types[&AssetKind::Image].clone();
        for allow in [true, true] {
            let asked = answer_when_asked(&w, allow, false, "pictures");
            within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Image, &t, Some(0.1)).unwrap();
            asked.join().unwrap();
        }
        // Always: it stops asking for pictures.
        let asked = answer_when_asked(&w, true, true, "pictures");
        within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Image, &t, Some(0.1)).unwrap();
        asked.join().unwrap();
        within_limit(&mut ctx(&mut session), &inbox, &mut w, AssetKind::Image, &t, Some(0.1)).unwrap();
        assert!(inbox.pending().is_empty());
    }

    /// A model the provider has no price for runs without a limit, and each generation is counted
    /// without an amount.
    #[test]
    fn an_untracked_model_runs_and_is_counted_without_an_amount() {
        let (_d, mut w) = ws();
        let stub = Recording::new("fal", Unit::Usd, 0.0).with(picture_model("pic", Pricing::Unknown));
        let (engine, stub) = recording_engine(&mut w, stub);
        choose(&mut w, AssetKind::Image, "fal", "pic", Unit::Usd, Pricing::Unknown, 0.0);
        for i in 0..3 {
            let v = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": format!("crate-{i}.png")})).unwrap();
            assert_eq!((v["cost"].clone(), v["limit"].clone(), v["generations"].as_u64()), (Value::Null, Value::Null, Some(i + 1)));
        }
        assert_eq!(stub.calls().len(), 3, "a zero limit doesn't stop a type whose usage isn't tracked");
        assert!(w.try_assets().unwrap().iter().all(|r| r.amount.is_none() && r.model == "pic"));
        // A model whose price became known is limited again, whatever the project was shown.
        let priced = Recording::new("fal", Unit::Usd, 0.0).with(picture_model("pic", price(Unit::Usd, 0.1)));
        let engine = engine_with(&w, vec![Arc::new(priced)]);
        assert_eq!(call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "late.png"})).unwrap_err().code, ErrorCode::OverLimit);
    }

    /// From a description: the pictures choice makes a picture first, counted as a picture, when the
    /// 3D model starts from pictures; with pictures Off, a model that takes words gets the words,
    /// and one that takes only pictures is refused, naming both ways out.
    #[test]
    fn a_3d_model_from_text_uses_the_pictures_choice_or_refuses() {
        let from_text = || json!({"prompt": "a barrel", "save_as": "barrel.glb"});
        // Pictures on: a picture with the pictures choice, then the model from it.
        let (_d, mut w) = ws();
        let pictures = Recording::new("fal", Unit::Usd, 0.15);
        let solids = Recording::new("scenario", Unit::Credits, 30.0);
        choose_all(&mut w, &pictures);
        choose(&mut w, AssetKind::Model3d, "scenario", "solid", Unit::Credits, price(Unit::Credits, 30.0), 500.0);
        let (pictures, solids) = (Arc::new(pictures), Arc::new(solids));
        let engine = engine_with(&w, vec![pictures.clone(), solids.clone()]);
        let v = call(&engine, "assets_model_generate", from_text()).unwrap();
        assert_eq!(pictures.kinds_run(), [AssetKind::Image]);
        assert!(pictures.calls()[0].words.as_deref().is_some_and(|w| w.starts_with("a barrel. One whole object")));
        assert_eq!(
            solids.calls(),
            [Call { kind: AssetKind::Model3d, model: "solid".into(), picture: Some("https://stand-in/picture.png".into()), words: None }]
        );
        assert_eq!(v["picture"]["saved"], "barrel.png");
        assert_eq!((v["picture"]["cost"].as_f64(), v["picture"]["unit"].as_str()), (Some(0.15), Some("usd")));
        assert_eq!((v["cost"].as_f64(), v["unit"].as_str(), v["used"].as_f64()), (Some(30.0), Some("credits"), Some(30.0)));
        let records: Vec<_> = w.try_assets().unwrap().into_iter().map(|r| (r.file, r.kind, r.provider, r.amount, r.unit)).collect();
        assert_eq!(
            records,
            [
                ("barrel.png".to_string(), "image".to_string(), "fal".to_string(), Some(0.15), Some(Unit::Usd)),
                ("barrel.glb".to_string(), "model".to_string(), "scenario".to_string(), Some(30.0), Some(Unit::Credits)),
            ],
            "the picture against pictures, the model against 3D models, each in its unit"
        );
        // Pictures Off, and a 3D model that takes words: the words go to it directly.
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("scenario", Unit::Credits, 30.0));
        w.set_person(|m| {
            m.assets.types.insert(AssetKind::Image, TypeAssets { choice: AssetChoice::off(), ..TypeAssets::default() });
        })
        .unwrap();
        for model in ["words-3d", "both-3d"] {
            choose(&mut w, AssetKind::Model3d, "scenario", model, Unit::Credits, price(Unit::Credits, 30.0), 500.0);
            let v = call(&engine, "assets_model_generate", json!({"prompt": "a barrel", "save_as": format!("{model}.glb")})).unwrap();
            assert_eq!(v["picture"], Value::Null);
            assert_eq!(stub.calls().last(), Some(&Call { kind: AssetKind::Model3d, model: model.into(), picture: None, words: Some("a barrel".into()) }));
        }
        assert!(!stub.kinds_run().contains(&AssetKind::Image));
        // A 3D model that takes words only is given the words even with pictures on.
        choose(&mut w, AssetKind::Image, "scenario", "pic", Unit::Credits, price(Unit::Credits, 30.0), 500.0);
        choose(&mut w, AssetKind::Model3d, "scenario", "words-3d", Unit::Credits, price(Unit::Credits, 30.0), 500.0);
        call(&engine, "assets_model_generate", json!({"prompt": "a barrel", "save_as": "direct.glb"})).unwrap();
        assert!(!stub.kinds_run().contains(&AssetKind::Image), "no picture a model can't use");
        // Pictures Off, and a 3D model that takes only pictures: refused, naming both ways out.
        w.set_person(|m| {
            m.assets.types.insert(AssetKind::Image, TypeAssets { choice: AssetChoice::off(), ..TypeAssets::default() });
        })
        .unwrap();
        choose(&mut w, AssetKind::Model3d, "scenario", "solid", Unit::Credits, price(Unit::Credits, 30.0), 500.0);
        let before = (stub.calls().len(), w.try_assets().unwrap().len());
        let error = call(&engine, "assets_model_generate", from_text()).unwrap_err();
        assert_eq!(error.code, ErrorCode::NoPermission);
        let remedy = error.remediation.unwrap_or_default();
        assert!(remedy.contains("image") && remedy.contains("provider and model for pictures"), "{remedy}");
        assert_eq!((stub.calls().len(), w.try_assets().unwrap().len()), before);
        // A picture given works for that model all the same.
        call(&engine, "assets_model_generate", json!({"image": "https://example.invalid/a.png", "save_as": "given.glb"})).unwrap();
    }

    /// The result names the options of the request the model has no input for.
    #[test]
    fn the_result_names_ignored_options() {
        let (_d, mut w) = ws();
        let shapeless = detail("pic", AssetKind::Image, vec![input("prompt", FieldKind::Text, true)], price(Unit::Usd, 0.1));
        let (engine, _) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1).with(shapeless));
        let v = call(&engine, "assets_image_generate", json!({"prompt": "a sky", "shape": "wide", "save_as": "sky.png"})).unwrap();
        assert_eq!(v["ignored"], json!(["shape"]));
        let v = call(&engine, "assets_sound_generate", json!({"prompt": "wind", "duration_seconds": 2.0, "save_as": "wind.mp3"})).unwrap();
        assert_eq!(v["ignored"], json!([]));
        assert_eq!(serde_json::to_value(AssetOption::Shape).unwrap(), "shape");
    }

    /// A chosen model the provider no longer serves is refused when it is described: nothing is
    /// estimated, generated or recorded.
    #[test]
    fn a_model_the_provider_no_longer_serves_is_refused_before_anything_is_spent() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
        choose(&mut w, AssetKind::Image, "fal", "gone-model", Unit::Usd, price(Unit::Usd, 0.1), 5.0);
        let error = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadSettings);
        assert!(error.message.contains("gone-model") && error.remediation.unwrap_or_default().contains("Choose another model"));
        assert_eq!(*stub.described.lock().unwrap(), ["gone-model"]);
        assert!(stub.nothing_asked() && w.assets().is_empty());
        // A model that doesn't make the type is refused too.
        choose(&mut w, AssetKind::Image, "fal", "sound", Unit::Usd, price(Unit::Usd, 0.1), 5.0);
        let error = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert!(error.message.contains("doesn't make pictures"), "{}", error.message);
        assert!(stub.nothing_asked());
    }

    /// A project choice whose unit isn't the provider's, or a provider that isn't registered, is refused.
    #[test]
    fn a_choice_the_registry_cannot_honour_is_refused() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
        choose(&mut w, AssetKind::Image, "fal", "pic", Unit::Credits, price(Unit::Credits, 0.1), 500.0);
        let error = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadSettings);
        assert!(error.message.contains("credits") && error.message.contains("dollars"), "{}", error.message);
        choose(&mut w, AssetKind::Image, "acme", "pic", Unit::Usd, price(Unit::Usd, 0.1), 5.0);
        assert_eq!(call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err().code, ErrorCode::FeatureMissing);
        assert!(stub.nothing_asked() && stub.described.lock().unwrap().is_empty());
    }

    #[test]
    fn a_key_for_another_provider_doesnt_pay_this_one() {
        let (_d, mut w) = ws();
        choose(&mut w, AssetKind::Image, "fal", "fal-ai/flux/dev", Unit::Usd, price(Unit::Usd, 0.05), 5.0);
        let e = crate::Engine::new(crate::testing::session());
        e.session().ext.insert(Arc::new(InboxDir(w.inbox_dir())));
        e.session().ext.insert(Arc::new(AssetKeys([("fal".to_string(), " ".to_string())].into())));
        let err = call(&e, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert!(err.code == ErrorCode::BadSettings, "a blank key is no key");
        let mut providers = AssetProviders::built_in();
        providers.register(Arc::new(Recording::new("other", Unit::Usd, 0.1)));
        e.session().ext.insert(Arc::new(providers));
        e.session().ext.insert(Arc::new(AssetKeys([("other".to_string(), "sk-test".to_string())].into())));
        let err = call(&e, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.message.contains("No fal.ai key"), "{}", err.message);
        assert!(w.assets().is_empty() && Inbox::new(w.inbox_dir()).pending().is_empty(), "nothing asked or spent");
    }

    #[test]
    fn without_a_key_it_says_to_use_simple_shapes() {
        let (_d, mut w) = ws();
        choose(&mut w, AssetKind::Image, "fal", "fal-ai/flux/dev", Unit::Usd, price(Unit::Usd, 0.05), 5.0);
        let e = crate::Engine::new(crate::testing::session());
        e.session().ext.insert(Arc::new(InboxDir(w.inbox_dir())));
        let err = call(&e, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.remediation.unwrap_or_default().contains("simple shapes"));
        for faces in [0, MAX_FACES + 1] {
            let err = call(&e, "assets_model_generate", json!({"prompt": "a crate", "save_as": "crate.glb", "faces": faces})).unwrap_err();
            assert!(err.message.contains("faces must be 1 to 500000"), "{}", err.message);
        }
    }

    #[test]
    fn nothing_is_generated_unless_the_project_allows_it() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
        w.set_person(|m| m.generate_assets = false).unwrap();
        let err = call(&engine, "assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::NoPermission, "a key alone isn't the person's choice");
        assert!(err.remediation.unwrap_or_default().contains("simple shapes"));
        assert!(stub.nothing_asked() && w.assets().is_empty() && Inbox::new(w.inbox_dir()).pending().is_empty(), "nothing asked or spent");
    }

    /// A failure after the provider accepted the work reaches the ledger, and later attempts are
    /// checked against it; a charge that can't be recorded is reported, never dropped.
    #[test]
    fn a_charged_failure_is_recorded_and_counts_against_the_limit() {
        let (_d, mut w) = ws();
        let (engine, _) = recording_engine(&mut w, Recording { fails_charging: Some(Some(30.0)), ..Recording::new("elevenlabs", Unit::Credits, 10.0) });
        let sound = |e: &crate::Engine| {
            call(e, "assets_sound_generate", json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"audio/chime.mp3"})).unwrap_err()
        };
        let error = sound(&engine);
        assert_eq!(error.code, ErrorCode::ProgramFailed);
        assert!(error.message.contains("30.0000 credits") && error.message.contains("recorded as spending"), "{}", error.message);
        assert!(error.remediation.as_deref().is_some_and(|r| r.starts_with("Check the job.") && r.contains("limit for sound effects")));
        let records = w.try_assets().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            (records[0].file.as_str(), records[0].kind.as_str(), records[0].amount, records[0].unit),
            ("audio/chime.mp3", "sound", Some(30.0), Some(Unit::Credits))
        );
        assert!(!w.app_dir().join("audio/chime.mp3").exists(), "no output was saved");
        // With the limit at what was already charged, the next attempt is refused before spending.
        w.set_person(|m| {
            if let Some(t) = m.assets.types.get_mut(&AssetKind::SoundEffect) {
                t.limit = 30.0;
            }
        })
        .unwrap();
        assert_eq!(sound(&engine).code, ErrorCode::OverLimit);
        assert_eq!(w.try_assets().unwrap().len(), 1);
        // A charge of unknown cost is recorded without an amount.
        w.set_person(|m| m.assets.ignore_budget = true).unwrap();
        let unknown = engine_with(&w, vec![Arc::new(Recording { fails_charging: Some(None), ..Recording::new("elevenlabs", Unit::Credits, 10.0) })]);
        assert!(sound(&unknown).message.contains("spending of unknown amount"));
        assert_eq!(w.try_assets().unwrap().last().map(|r| (r.amount, r.cost_missing)), Some((None, true)));
        // An invalid charge is reported as unrecorded, and the ledger is left as it was.
        let invalid = engine_with(&w, vec![Arc::new(Recording { fails_charging: Some(Some(-1.0)), ..Recording::new("elevenlabs", Unit::Credits, 10.0) })]);
        assert!(sound(&invalid).message.contains("could not be recorded"));
        assert_eq!(w.try_assets().unwrap().len(), 2);
    }

    /// A model stage that fails after its picture was made records both charges.
    #[test]
    fn a_charged_model_failure_after_its_picture_records_both() {
        let (_d, mut w) = ws();
        let pictures = Arc::new(Recording::new("fal", Unit::Usd, 0.1));
        choose_all(&mut w, &pictures);
        choose(&mut w, AssetKind::Model3d, "scenario", "solid", Unit::Credits, price(Unit::Credits, 40.0), 500.0);
        let failing = Recording { fails_charging: Some(Some(40.0)), ..Recording::new("scenario", Unit::Credits, 40.0) };
        let engine = engine_with(&w, vec![pictures.clone(), Arc::new(failing)]);
        let error = call(&engine, "assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})).unwrap_err();
        assert!(error.message.contains("recorded as spending for crate.glb"), "{}", error.message);
        let records = w.try_assets().unwrap();
        assert_eq!(records.iter().map(|r| (r.file.as_str(), r.kind.as_str())).collect::<Vec<_>>(), [("crate.png", "image"), ("crate.glb", "model")]);
        assert_eq!((records[1].amount, records[1].unit), (Some(40.0), Some(Unit::Credits)));
        assert!(w.app_dir().join("crate.png").exists() && !w.app_dir().join("crate.glb").exists());
        assert_eq!(pictures.kinds_run(), [AssetKind::Image]);
    }

    /// A generation holds the project's spending lock through its paid call and releases it after
    /// its receipt; another process's generation then waits instead of using a stale total.
    #[test]
    fn a_generation_holds_the_spending_lock_until_its_receipt_is_written() {
        let (_d, mut w) = ws();
        let probe = Recording { probe: Some((w.root.clone(), Mutex::default())), ..Recording::new("elevenlabs", Unit::Credits, 10.0) };
        let (engine, probe) = recording_engine(&mut w, probe);
        call(&engine, "assets_sound_generate", json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"audio/chime.mp3"})).unwrap();
        assert_eq!(probe.probe.as_ref().and_then(|(_, held)| *held.lock().unwrap()), Some(true), "held during the paid call");
        assert_eq!(w.try_assets().unwrap().len(), 1);
        w.lock_spending(Duration::ZERO).expect("released once the receipt is written");
    }

    /// A generation that waited for the spending lock re-reads the person's choices first: turned
    /// off while it waited, it stops before any estimate, paid call or receipt.
    #[test]
    fn a_generation_that_waited_for_the_lock_rereads_the_projects_choices() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
        let held = w.lock_spending(Duration::ZERO).unwrap();
        let waiting = std::thread::spawn(move || {
            let params = json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"audio/chime.mp3"});
            call(&engine, "assets_sound_generate", params).map(|_| ()).unwrap_err().code
        });
        // Long enough for the call to pass its first checks and wait on the lock.
        std::thread::sleep(Duration::from_millis(500));
        Workspace::open(&w.root).unwrap().set_person(|m| m.generate_assets = false).unwrap();
        drop(held);
        assert_eq!(waiting.join().unwrap(), ErrorCode::NoPermission);
        assert!(stub.nothing_asked() && stub.described.lock().unwrap().is_empty());
        assert!(w.try_assets().unwrap().is_empty());
    }

    /// Where the app is the project folder, an asset never lands on Craft Anything's records, named
    /// directly or through a link that resolves into one, and nothing is spent trying.
    #[test]
    fn assets_never_overwrite_the_projects_own_records() {
        let (_d, mut w) = ws();
        w.manifest.app_in_root = true;
        w.save().unwrap();
        std::fs::create_dir_all(w.root.join("reference")).unwrap();
        // Other cases too: a case-insensitive file system writes them into the record folders.
        for record in ["reference/picture-1.png", "pinned/old.png", "inbox/x.png", "./capture/shot.png", "Reference/x.png", "PINNED/x.png", "attachmentſ/x.png"]
        {
            assert_eq!(save_path(&w, record, "png").unwrap_err().code, ErrorCode::BadInput, "{record}");
        }
        // Spellings Windows maps onto a record: trailing dots or spaces, data streams, short names.
        for spelling in ["reference./x.png", "reference /x.png", "inbox:hidden/x.png", "ATTACH~1/x.png", "Assets/x.png.", "Assets\\pinned. \\x.png"] {
            assert_eq!(save_path(&w, spelling, "png").unwrap_err().code, ErrorCode::BadInput, "{spelling}");
        }
        assert_eq!(save_path(&w, "Assets/crate.png", "png").unwrap().1, "Assets/crate.png");
        assert_eq!(save_path(&w, "Assets/sky~night.png", "png").unwrap().1, "Assets/sky~night.png", "a tilde alone is fine");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(w.root.join("reference"), w.root.join("art")).unwrap();
            assert_eq!(save_path(&w, "art/sky.png", "png").unwrap_err().code, ErrorCode::BadInput, "a link into a record");
        }
        let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
        let params = json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"reference/chime.mp3"});
        assert_eq!(call(&engine, "assets_sound_generate", params).unwrap_err().code, ErrorCode::BadInput);
        assert!(stub.nothing_asked());
        assert!(w.try_assets().unwrap().is_empty());
    }

    /// A newer manifest version must block every generation tool without altering the manifest or asset ledger.
    #[test]
    fn future_project_settings_refuse_all_paid_generation() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
        let mut future = w.manifest.clone();
        future.version = crate::workspace::MANIFEST_VERSION + 1;
        std::fs::write(w.root.join("craft.json"), serde_json::to_vec(&future).unwrap()).unwrap();
        let original = std::fs::read(w.root.join("craft.json")).unwrap();
        for (tool, params, _) in every_call() {
            assert_eq!(call(&engine, tool, params).unwrap_err().code, ErrorCode::NoPermission);
        }
        assert!(stub.nothing_asked());
        assert!(w.assets().is_empty());
        assert_eq!(std::fs::read(w.root.join("craft.json")).unwrap(), original);
    }

    /// Each audio tool must select its request kind, save the returned bytes and record exactly one paid asset.
    #[test]
    fn audio_tools_route_with_only_an_elevenlabs_key_and_record_each_asset_once() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
        for (tool, params, kind) in audio_calls() {
            let v = call(&engine, tool, params).unwrap();
            assert_eq!(v["provider"], "elevenlabs");
            let saved = v["saved"].as_str().unwrap();
            assert_eq!(std::fs::read(w.app_dir().join(saved)).unwrap(), b"generated");
            assert_eq!(stub.calls().last().map(|c| (c.kind, c.model.clone())), Some((kind, model_for(kind).to_string())));
        }
        let records = w.assets();
        assert_eq!(records.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(), ["speech", "sound", "music"]);
        assert!(records.iter().all(|r| r.amount == Some(10.0) && r.unit == Some(Unit::Credits)));
        assert_eq!(stub.calls().len(), 3);
    }

    /// Auto-mode audio calls must stop before generation when opt-in is missing or the type's limit is used up.
    #[test]
    fn every_audio_tool_requires_opt_in_and_stops_before_spending_over_its_limit() {
        for (tool, params, kind) in audio_calls() {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
            w.set_person(|m| m.generate_assets = false).unwrap();
            assert_eq!(call(&engine, tool, params.clone()).unwrap_err().code, ErrorCode::NoPermission);
            w.set_person(|m| {
                m.generate_assets = true;
                if let Some(t) = m.assets.types.get_mut(&kind) {
                    t.limit = 0.0;
                }
            })
            .unwrap();
            assert_eq!(call(&engine, tool, params).unwrap_err().code, ErrorCode::OverLimit);
            assert!(stub.calls().is_empty());
            assert!(w.assets().is_empty());
            assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
        }
    }

    /// Invalid audio parameters, escaping paths and missing provider credentials must leave generation untouched.
    #[test]
    fn audio_validation_and_provider_keys_are_checked_before_paid_calls() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
        for (tool, params) in [
            ("assets_speech_generate", json!({"text":" ","voice_id":"voice-test","save_as":"a.mp3"})),
            ("assets_speech_generate", json!({"text":"Hi","voice_id":"../voice","save_as":"a.mp3"})),
            ("assets_sound_generate", json!({"prompt":"chime","duration_seconds":0.0,"save_as":"a.mp3"})),
            ("assets_music_generate", json!({"prompt":"piano","duration_seconds":601.0,"save_as":"a.mp3"})),
            ("assets_music_generate", json!({"prompt":"piano","duration_seconds":3.0,"save_as":"../a.mp3"})),
            ("assets_sound_generate", json!({"prompt":"chime","duration_seconds":2.0,"save_as":"a.mp3","model":"music"})),
        ] {
            assert!(call(&engine, tool, params).is_err());
        }
        engine.session().ext.insert(Arc::new(AssetKeys([("fal".into(), "sk-test".into())].into())));
        let (tool, params, _) = audio_calls().remove(0);
        assert_eq!(call(&engine, tool, params).unwrap_err().code, ErrorCode::BadSettings);
        assert!(stub.nothing_asked());
        assert!(w.assets().is_empty());
    }

    /// Invalid provider estimates must fail before generation or any asset ledger entry.
    #[test]
    fn nonfinite_or_negative_provider_estimates_never_authorize_spending() {
        for estimate in [f64::NAN, f64::INFINITY, -0.1] {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&mut w, Recording { estimate: Some(Some(estimate)), ..Recording::new("elevenlabs", Unit::Credits, 10.0) });
            let (tool, params, _) = audio_calls().remove(0);
            assert!(call(&engine, tool, params).is_err());
            assert!(stub.calls().is_empty());
            assert!(w.assets().is_empty());
        }
    }

    /// Invalid actual costs must prevent both writing generated bytes and adding a spending record.
    #[test]
    fn invalid_returned_cost_is_never_saved_or_recorded() {
        let (_d, w) = ws();
        let (path, rel) = save_path(&w, "a.mp3", "mp3").unwrap();
        for cost in [f64::NAN, f64::INFINITY, -1.0] {
            let made = Generated { url: String::new(), bytes: vec![1], cost: Some(cost), ignored: Vec::new() };
            assert!(keep(&w, &path, &rel, &speech(), &made).is_err());
            assert!(!path.exists());
            assert!(w.assets().is_empty());
        }
    }

    /// Unreadable ledgers stop every paid tool before quotes, approval questions or provider calls.
    #[test]
    fn invalid_ledgers_block_all_generations_before_provider_calls() {
        for invalid in ["json", "directory", "negative", "overflow"] {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
            let ledger = w.root.join("assets.json");
            let receipt = |cost| receipt("old.png", "image", cost);
            match invalid {
                "directory" => std::fs::create_dir(&ledger).unwrap(),
                "negative" => std::fs::write(&ledger, ledger_of(&[receipt(-1.0)])).unwrap(),
                "overflow" => std::fs::write(&ledger, ledger_of(&[receipt(f64::MAX), receipt(f64::MAX)])).unwrap(),
                _ => std::fs::write(&ledger, "not JSON").unwrap(),
            }
            let original = std::fs::read(&ledger).ok();
            let mut calls = every_call();
            calls.push(("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"}), AssetKind::Model3d));
            for (tool, params, _) in calls {
                assert!(call(&engine, tool, params).is_err(), "{invalid}: {tool}");
            }
            assert!(stub.nothing_asked() && stub.described.lock().unwrap().is_empty(), "{invalid}");
            assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
            assert_eq!(std::fs::read(&ledger).ok(), original);
        }
    }

    /// A dangling ledger link cannot authorize a quote or generation and must remain untouched.
    #[cfg(unix)]
    #[test]
    fn dangling_asset_ledger_blocks_provider_calls() {
        let (_d, mut w) = ws();
        let (engine, provider) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
        let ledger = w.root.join("assets.json");
        let target = w.root.join("missing-ledger.json");
        std::os::unix::fs::symlink(&target, &ledger).unwrap();
        for (tool, params) in [
            ("assets_image_generate", json!({"prompt":"a crate", "save_as":"crate.png"})),
            ("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})),
        ] {
            assert!(call(&engine, tool, params).is_err());
        }
        assert!(provider.nothing_asked());
        assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
        assert_eq!(std::fs::read_link(ledger).unwrap(), target);
        assert!(!target.exists());
    }

    /// Once a provider has charged, a ledger failure must leave new and existing output destinations untouched.
    #[test]
    fn receipt_failure_preserves_the_destination_and_warns_of_possible_charges() {
        for existing in [false, true] {
            let (_d, w) = ws();
            let (path, rel) = save_path(&w, "audio/a.mp3", "mp3").unwrap();
            if existing {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, b"original").unwrap();
            }
            std::fs::write(w.root.join("assets.json"), "broken").unwrap();
            let made = Generated { url: String::new(), bytes: b"replacement".to_vec(), cost: Some(20.0), ignored: Vec::new() };
            let err = keep(&w, &path, &rel, &speech(), &made).unwrap_err();
            assert!(err.message.contains("may have charged 20.0000 credits"), "{}", err.message);
            assert!(err.remediation.unwrap().contains("Do not blindly"));
            if existing {
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
            } else {
                assert!(!path.parent().unwrap().exists(), "no destination preparation before a receipt");
            }
            assert_eq!(std::fs::read(w.root.join("assets.json")).unwrap(), b"broken");
        }
    }

    /// A final ledger read failure cannot be reported as successful generation with zero historical spending.
    #[test]
    fn saved_asset_summary_reports_ledger_failure_without_erasing_output() {
        let (_d, mut w) = ws();
        let stub = Recording::new("elevenlabs", Unit::Credits, 10.0);
        choose_all(&mut w, &stub);
        let t = w.manifest.assets.types[&AssetKind::Speech].clone();
        let ready = Ready { kind: AssetKind::Speech, model: stub.models["speech"].clone(), t, provider: Arc::new(stub), key: "sk-test".into() };
        let (path, rel) = save_path(&w, "a.mp3", "mp3").unwrap();
        let made = Generated { url: String::new(), bytes: b"generated".to_vec(), cost: Some(0.2), ignored: Vec::new() };
        keep(&w, &path, &rel, &ready.receipt("hi"), &made).unwrap();
        std::fs::write(w.root.join("assets.json"), "broken").unwrap();
        let err = summary(&w, &ready, &rel, &made, json!({})).unwrap_err();
        assert!(err.message.contains("Saved a.mp3 and recorded") && err.message.contains("could not be read"), "{}", err.message);
        assert!(err.remediation.unwrap().contains("Do not blindly"));
        assert_eq!(std::fs::read(path).unwrap(), b"generated");
        assert_eq!(std::fs::read(w.root.join("assets.json")).unwrap(), b"broken");
    }

    /// Filesystem preparation and atomic-save failures happen after accounting and never erase a charge.
    #[test]
    fn saving_failure_keeps_the_receipt_and_existing_files() {
        for blocked_parent in [false, true] {
            let (_d, w) = ws();
            let (path, rel) = save_path(&w, "audio/a.mp3", "mp3").unwrap();
            std::fs::create_dir_all(w.app_dir()).unwrap();
            let preserved = if blocked_parent {
                path.parent().unwrap().to_path_buf()
            } else {
                std::fs::create_dir_all(&path).unwrap();
                path.join("original")
            };
            std::fs::write(&preserved, b"original").unwrap();
            let made = Generated { url: String::new(), bytes: b"replacement".to_vec(), cost: Some(0.2), ignored: Vec::new() };
            let err = keep(&w, &path, &rel, &speech(), &made).unwrap_err();
            assert!(err.message.contains("was recorded, but saving"));
            assert!(err.remediation.unwrap().contains("Do not blindly"));
            assert_eq!(w.try_assets().unwrap()[0].amount, Some(0.2));
            assert_eq!(w.try_assets().unwrap()[0].file, rel);
            assert_eq!(std::fs::read(preserved).unwrap(), b"original");
        }
    }

    /// A confinement check that fails after generation still retains the paid receipt and the old target bytes.
    #[cfg(unix)]
    #[test]
    fn post_generation_confinement_failure_keeps_charge_and_existing_output() {
        let (_d, w) = ws();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("original.mp3");
        std::fs::write(&target, b"original").unwrap();
        let (path, rel) = save_path(&w, "audio/a.mp3", "mp3").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let made = Generated { url: String::new(), bytes: b"replacement".to_vec(), cost: Some(0.2), ignored: Vec::new() };
        assert!(keep(&w, &path, &rel, &speech(), &made).unwrap_err().message.contains("was recorded, but saving"));
        assert_eq!(w.try_assets().unwrap()[0].amount, Some(0.2));
        assert_eq!(std::fs::read(target).unwrap(), b"original");
    }

    /// Overruns for every asset kind preserve output and charge, and return a non-retryable explanation.
    #[test]
    fn stage_overruns_are_reported_only_after_recording_and_saving() {
        for (tool, params, _) in every_call() {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&mut w, Recording { estimate: Some(Some(0.0)), ..Recording::new("fal", Unit::Usd, 0.1) });
            let rel = params["save_as"].as_str().unwrap().to_string();
            let err = call(&engine, tool, params).unwrap_err();
            assert_eq!(err.code, ErrorCode::OverLimit);
            assert!(err.message.contains(&rel) && err.message.contains("recorded"), "{}", err.message);
            assert!(err.remediation.unwrap().contains("Do not blindly"));
            assert_eq!(std::fs::read(w.app_dir().join(rel)).unwrap(), b"generated");
            assert_eq!(w.try_assets().unwrap().len(), 1);
            assert_eq!(w.try_assets().unwrap()[0].amount, Some(0.1));
            assert_eq!(stub.calls().len(), 1);
        }
    }

    /// An over-estimate precursor is kept and recorded, but cannot authorize the subsequent model generation.
    #[test]
    fn precursor_overrun_stops_before_model_generation() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&mut w, Recording { estimate: Some(Some(0.0)), ..Recording::new("fal", Unit::Usd, 0.1) });
        let err = call(&engine, "assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        assert!(err.message.contains("crate.png"));
        assert_eq!(stub.kinds_run(), [AssetKind::Image]);
        assert_eq!(std::fs::read(w.app_dir().join("crate.png")).unwrap(), b"generated");
        assert!(!w.app_dir().join("crate.glb").exists());
        assert_eq!(w.try_assets().unwrap().len(), 1);
    }

    /// A custom provider whose price depends on the exact prompt, exposing quote/generation mismatches.
    struct PromptPriced {
        stand_in: Recording,
        quoted: Mutex<Vec<String>>,
        generated: Mutex<Vec<String>>,
        model_calls: Mutex<usize>,
    }

    impl PromptPriced {
        /// Price every picture character; charge a fixed amount for the downstream model.
        fn price(req: &AssetRequest) -> f64 {
            match req {
                AssetRequest::Image { prompt, .. } => prompt.chars().count() as f64 / 1000.0,
                _ => 0.5,
            }
        }
    }

    impl AssetProvider for PromptPriced {
        /// Replace fal without real credentials or network calls.
        fn id(&self) -> &'static str {
            "fal"
        }
        /// Name the local stand-in in any error.
        fn name(&self) -> &'static str {
            "prompt-priced stand-in"
        }
        fn unit(&self) -> Unit {
            Unit::Usd
        }
        fn list_models(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError> {
            self.stand_in.list_models(key, kind)
        }
        fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
            self.stand_in.describe(key, model)
        }
        /// Capture exactly what was quoted.
        fn estimate(&self, _key: &str, _model: &ModelDetail, req: &AssetRequest) -> Result<Option<f64>, ToolError> {
            if let AssetRequest::Image { prompt, .. } = req {
                self.quoted.lock().unwrap().push(prompt.to_string());
            }
            Ok(Some(Self::price(req)))
        }
        /// Capture exactly what was generated and return the matching charge.
        fn generate(&self, _key: &str, _model: &ModelDetail, req: &AssetRequest) -> Result<Generated, ToolError> {
            match req {
                AssetRequest::Image { prompt, .. } => self.generated.lock().unwrap().push(prompt.to_string()),
                _ => *self.model_calls.lock().unwrap() += 1,
            }
            Ok(Generated { url: "https://example.invalid/source.png".into(), bytes: b"generated".to_vec(), cost: Some(Self::price(req)), ignored: Vec::new() })
        }
    }

    /// Precursor quotes use the expanded trimmed prompt, while ledger provenance retains the person's description.
    #[test]
    fn precursor_quotes_and_generation_use_identical_expanded_prompts() {
        for picture_limit in [0.05, 5.0] {
            let (_d, mut w) = ws();
            let stub = Arc::new(PromptPriced {
                stand_in: Recording::new("fal", Unit::Usd, 0.1),
                quoted: Mutex::default(),
                generated: Mutex::default(),
                model_calls: Mutex::default(),
            });
            choose_all(&mut w, &stub.stand_in);
            choose(&mut w, AssetKind::Image, "fal", "pic", Unit::Usd, price(Unit::Usd, 0.1), picture_limit);
            let engine = engine_with(&w, vec![stub.clone()]);
            let result = call(&engine, "assets_model_generate", json!({"prompt":"  a crate  ", "save_as":"crate.glb"}));
            let quoted = stub.quoted.lock().unwrap().clone();
            assert_eq!(quoted.len(), 1);
            assert!(quoted[0].starts_with("a crate. One whole object"));
            if picture_limit < 1.0 {
                assert_eq!(result.unwrap_err().code, ErrorCode::OverLimit);
                assert!(stub.generated.lock().unwrap().is_empty());
                assert_eq!(*stub.model_calls.lock().unwrap(), 0);
                assert!(w.try_assets().unwrap().is_empty());
            } else {
                result.unwrap();
                assert_eq!(*stub.generated.lock().unwrap(), quoted);
                assert_eq!(*stub.model_calls.lock().unwrap(), 1);
                assert!(w.try_assets().unwrap().iter().all(|record| record.prompt == "a crate"));
            }
        }
    }

    /// A malformed downstream model key must be rejected before paying for its precursor picture.
    #[test]
    fn malformed_model_credentials_stop_before_spending_on_the_source_picture() {
        let (_d, mut w) = ws();
        let (engine, pictures) = recording_engine(&mut w, Recording::new("fal", Unit::Usd, 0.1));
        choose(&mut w, AssetKind::Model3d, "scenario", "model_meshy-7-1-img23d", Unit::Credits, price(Unit::Credits, 30.0), 500.0);
        // Scenario's key needs a secret; this one has none.
        engine.session().ext.insert(Arc::new(AssetKeys([("fal".into(), "sk-test".into()), ("scenario".into(), "sk-test".into())].into())));
        let result = call(&engine, "assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"}));
        assert_eq!(result.unwrap_err().code, ErrorCode::BadSettings);
        assert!(pictures.nothing_asked(), "reject a malformed downstream key before the first paid call");
        assert!(w.assets().is_empty());
    }

    /// Symlink escapes in inputs or outputs, including dangling links, must fail before paid generation.
    #[cfg(unix)]
    #[test]
    fn symlinked_outputs_and_inputs_cannot_escape_the_project() {
        let (_d, mut w) = ws();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(w.app_dir()).unwrap();
        std::os::unix::fs::symlink(outside.path(), w.app_dir().join("escape")).unwrap();
        std::fs::write(outside.path().join("picture.png"), b"png").unwrap();
        assert!(save_path(&w, "escape/new.mp3", "mp3").is_err());
        assert!(image_source(&w, "app/escape/picture.png").is_err());
        std::os::unix::fs::symlink(outside.path().join("new.mp3"), w.app_dir().join("broken.mp3")).unwrap();
        assert!(save_path(&w, "broken.mp3", "mp3").is_err());
        let (engine, stub) = recording_engine(&mut w, Recording::new("elevenlabs", Unit::Credits, 10.0));
        assert!(call(&engine, "assets_sound_generate", json!({"prompt":"chime","duration_seconds":2.0,"save_as":"escape/new.mp3"})).is_err());
        assert!(stub.calls().is_empty());
        assert!(!outside.path().join("new.mp3").exists());
    }
}
