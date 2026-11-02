//! Project assets the builder generates through registered providers: `assets_image_generate` (a picture: a texture, a sky,
//! concept art) and `assets_model_generate` (a low-poly GLB model, from a description or a picture). Each one
//! is paid for, so a project has a budget: the builder spends freely under it, and the person is
//! asked before it goes over. Nothing is generated unless the person turned it on for the project
//! (`project_assets_allow`). Every asset and its cost is listed in the project (assets.json).
//! The provider comes from the `AssetProviders` registry and the models and prices from the
//! asset table, so the cost is known before anything is spent.

use crate::inbox::Inbox;
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::tools::interaction_tools::{Verdict, ask_approval, inbox, inside, resolve, workspace_of};
use crate::workspace::{AssetRecord, Workspace};
use craft_anything_engines::assets::{AssetKind, AssetProvider, AssetProviders, AssetRequest, Catalogue, Generated, Shape, about_usd, charged_usd, fal, usd};
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

/// Remembered when the person says "always" to going over the budget.
const OVER_BUDGET: &str = "assets:over-budget";

/// Expose image, model and audio generation tools with network and filesystem effects declared.
pub fn specs() -> Vec<ToolSpec> {
    let effects = Effects::ACCESSES_NETWORK.union(Effects::WRITES_FILESYSTEM);
    vec![
        ToolSpec {
            id: "assets_image_generate",
            title: "Generate a picture",
            summary: &summaries().0,
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
            summary: &summaries().1,
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
            summary: "Generate speech from text with an ElevenLabs voice and save an MP3 inside app/. Requires the project's generated-assets opt-in, an ElevenLabs key and its asset budget. Choose a voice_id you are entitled to use. Returns saved path and cost.",
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
            summary: "Generate an original sound effect with ElevenLabs from a prompt and duration_seconds (0.5 to 30), saving an MP3 inside app/. Requires generated-assets opt-in, a provider key and the project's asset budget.",
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
            summary: "Generate original music with ElevenLabs from a prompt and duration_seconds (3 to 600), saving an MP3 inside app/. instrumental defaults to true. Requires generated-assets opt-in, a provider key and the project's asset budget.",
            beginner: "Makes original background music for your project.",
            toolset: Toolset::Core,
            effects,
            requires: Requires::Nothing,
            input_schema: schema_of::<MusicParams>,
            run: assets_music_generate,
        },
    ]
}

/// The two tools' summaries, with the prices and face limits of the asset table's default models.
fn summaries() -> &'static (String, String) {
    static TEXT: OnceLock<(String, String)> = OnceLock::new();
    TEXT.get_or_init(|| {
        let c = Catalogue::built_in();
        let (picture, picture_model) = c.image_model(None).map(|m| (m.cost(Shape::Square), m.name.as_str())).unwrap_or((0.0, "the default model"));
        let (textured, faces) = c.model_3d(None).map(|m| (m.cost(true), m.faces())).unwrap_or((0.0, 48..=20_000));
        let image = format!(
            "Make a picture with the selected provider (default: fal.ai {picture_model}) and save it as a PNG inside app/: a texture, a sky or background, an icon, a title screen or concept art. shape: square (1024x1024, default), wide (1024x576) or tall (576x1024). Costs {}, counted against the project's asset budget. Describe original designs in the original's style; never ask for recognisable characters, logos or brand artwork. Use model to override Settings; prices and limits depend on the chosen model. Returns where it was saved, the cost and the budget left.",
            about_usd(picture)
        );
        let model = format!(
            "Make a game-ready, low-poly 3D model (GLB) with the selected provider (default: fal.ai Tripo P1) and save it inside app/. Give a prompt (a picture is made first, then turned into the model) or an image (a web address, or a picture inside the project such as reference/picture-1.png). faces requests the face count (default generator range: {} to {}, default {DEFAULT_FACES}): about 500-1500 for small props and pickups, 2000-5000 for weapons, furniture and vehicles, 6000-15000 for main characters; fewer faces run faster, so use the fewest that still look right. textured (default true) gives colours and surfaces. A textured model costs {} plus {} for the picture, counted against the project's asset budget; it takes a minute or two. Describe one original object per model; never ask for recognisable characters, logos or brand artwork. Use model to override Settings; prices and limits depend on the chosen model. Returns where it was saved, the cost and the budget left.",
            faces.start(),
            faces.end(),
            usd(textured),
            usd(picture)
        );
        (image, model)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageParams {
    /// Asset table model id, overriding Settings (for example scenario-gemini-3-1).
    #[serde(default)]
    pub model: Option<String>,
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
    /// 3D asset table model id, overriding Settings (for example scenario-meshy).
    #[serde(default)]
    pub model: Option<String>,
    /// Picture model used when starting from a prompt, overriding Settings.
    #[serde(default)]
    pub image_model: Option<String>,
    /// Where to save it, relative to app/, ending in .glb (e.g. Assets/Models/crate.glb).
    pub save_as: String,
    /// What to make: one object, described in detail. Used when no image is given.
    #[serde(default)]
    pub prompt: String,
    /// A picture to turn into the model: a web address, or a picture inside the project.
    #[serde(default)]
    pub image: String,
    /// Colours and surfaces (default true); prices depend on the selected generator.
    #[serde(default)]
    pub textured: Option<bool>,
    /// Requested face count (default 3000). fal caps it at 48 to 20000; Scenario targets 100 to 300000.
    #[serde(default)]
    pub faces: Option<u32>,
}

/// A mid-sized game prop: detailed enough up close, cheap to draw many of.
const DEFAULT_FACES: u32 = 3000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpeechParams {
    /// The words to speak (1 to 5000 characters).
    pub text: String,
    /// An ElevenLabs voice id you are entitled to use.
    pub voice_id: String,
    /// Destination relative to app/, ending in .mp3.
    pub save_as: String,
    /// Audio model id from the asset table; absent uses its speech default.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoundParams {
    pub prompt: String,
    /// Sound length, 0.5 to 30 seconds.
    pub duration_seconds: f64,
    /// Destination relative to app/, ending in .mp3.
    pub save_as: String,
    #[serde(default)]
    pub model: Option<String>,
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
    #[serde(default)]
    pub model: Option<String>,
}

/// Prefer a nonblank explicit model id, trimming it before falling back to the saved setting.
fn selected<'a>(explicit: Option<&'a str>, setting: Option<&'a str>) -> Option<&'a str> {
    explicit.map(str::trim).filter(|id| !id.is_empty()).or(setting)
}

/// Reject negative or nonfinite amounts before they enter budget or spending calculations.
fn valid_cost(cost: f64) -> Result<(), ToolError> {
    if cost.is_finite() && cost >= 0.0 { Ok(()) } else { Err(ToolError::invalid("asset cost and budget must be finite and nonnegative")) }
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

/// Validate speech text and the voice id before routing the selected speech model through paid generation.
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
    let model = Catalogue::built_in().audio_model(AssetKind::Speech, p.model.as_deref())?;
    generate_audio(ctx, &p.save_as, "speech", text, AssetRequest::Speech { model, text, voice_id: voice })
}

/// Validate the prompt and sound duration before generating an MP3 with the chosen sound model.
fn assets_sound_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SoundParams = parse_params(params)?;
    let prompt = prompt_ok(&p.prompt)?;
    duration_ok(p.duration_seconds, 0.5, 30.0)?;
    let model = Catalogue::built_in().audio_model(AssetKind::SoundEffect, p.model.as_deref())?;
    generate_audio(ctx, &p.save_as, "sound", prompt, AssetRequest::SoundEffect { model, prompt, duration_seconds: p.duration_seconds })
}

/// Validate music length and prompt, defaulting to instrumental audio before paid generation.
fn assets_music_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: MusicParams = parse_params(params)?;
    let prompt = prompt_ok(&p.prompt)?;
    duration_ok(p.duration_seconds, 3.0, 600.0)?;
    let model = Catalogue::built_in().audio_model(AssetKind::Music, p.model.as_deref())?;
    generate_audio(
        ctx,
        &p.save_as,
        "music",
        prompt,
        AssetRequest::Music { model, prompt, duration_seconds: p.duration_seconds, instrumental: p.instrumental.unwrap_or(true) },
    )
}

/// Check destination, provider permission, credentials and budget before generating and recording an MP3.
fn generate_audio(ctx: &mut Ctx<'_>, save_as: &str, kind: &str, prompt: &str, req: AssetRequest<'_>) -> Result<ToolOutput, ToolError> {
    let mut j = job(ctx)?;
    let (path, rel) = save_path(&j.ws, save_as, "mp3")?;
    let provider = j.providers.for_request(&req)?;
    let key = j.key(provider.as_ref())?;
    j.ws.try_assets_spent()?;
    let estimate = provider.estimate(&req)?;
    within_budget(ctx, &j.inbox, &mut j.ws, estimate)?;
    ctx.report(None, "Generating audio…");
    let g = generate_paid(&j.ws, provider.as_ref(), &key, &req, &rel, kind, prompt)?;
    keep(&j.ws, &path, &rel, kind, prompt, &g)?;
    within_estimate(&rel, g.cost_usd, estimate)?;
    summary(&j.ws, &rel, g.cost_usd, json!({"provider": provider.id()}))
}

/// What a generation needs: the keys, the providers, the inbox (for the over-budget question) and
/// the project.
struct Job {
    keys: std::sync::Arc<AssetKeys>,
    providers: AssetProviders,
    inbox: Inbox,
    ws: Workspace,
}

/// Load generation context only for an opted-in project whose manifest this build can safely update.
fn job(ctx: &Ctx<'_>) -> Result<Job, ToolError> {
    let keys = ctx.session.ext.get::<AssetKeys>().unwrap_or_default();
    // A session can bring its own providers (a fork, a test); otherwise the built-in ones.
    let providers = ctx.session.ext.get::<AssetProviders>().map(|p| p.as_ref().clone()).unwrap_or_default();
    let not_project = || ToolError::new(ErrorCode::FeatureMissing, "Assets can only be generated inside a Craft Anything project.");
    let inbox = inbox(ctx).ok_or_else(not_project)?;
    let ws = workspace_of(&inbox).ok_or_else(not_project)?;
    if ws.is_read_only() {
        return Err(ToolError::new(ErrorCode::NoPermission, "This project's settings were written by a newer Craft Anything; asset generation is disabled.")
            .with_remediation("Update Craft Anything before generating assets for this project."));
    }
    if !ws.manifest.generate_assets {
        return Err(ToolError::new(ErrorCode::NoPermission, "The person hasn't turned on generated assets for this project.").with_remediation(
            "Make these assets with simple shapes and colours instead, and tell the person they can turn on generated assets for this game in the app.",
        ));
    }
    Ok(Job { keys, providers, inbox, ws })
}

/// Explain which provider lacks credentials and offer a fallback that requires no paid assets.
fn no_key(provider: &str) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, format!("No {provider} key is set up for this asset operation.")).with_remediation(format!(
        "Make these assets with simple shapes and colours instead, and tell the person they can add a {provider} key in Settings → AI for realistic ones."
    ))
}

impl Job {
    /// The key for `provider`, checked before anything is asked or spent.
    fn key(&self, provider: &dyn AssetProvider) -> Result<String, ToolError> {
        if !self.ws.manifest.asset_provider_allowed(provider.id()) {
            return Err(ToolError::new(ErrorCode::NoPermission, format!("{} is disabled for this project.", provider.name()))
                .with_remediation("Use an enabled provider, or ask the person to enable this provider in the project's asset settings."));
        }
        let key = self.keys.get(provider.id()).ok_or_else(|| no_key(provider.name()))?;
        provider.validate_key(key)?;
        Ok(key.to_string())
    }
}

/// Where to save inside app/: (full path, path relative to app/). `ext` is the required extension.
fn save_path(ws: &Workspace, save_as: &str, ext: &str) -> Result<(PathBuf, String), ToolError> {
    let app = ws.app_dir();
    // Paths given from the workspace ("app/Assets/x.glb") mean the same place.
    let rel = save_as.trim().trim_start_matches("./").trim_start_matches("app/").trim_start_matches("app\\");
    let path = resolve(&app, rel).filter(|p| inside(&app, p) && *p != app).ok_or_else(|| ToolError::invalid("save_as must be a file path inside app/"))?;
    confined(&ws.root, &app)?;
    confined(&app, &path)?;
    if path.is_dir() {
        return Err(ToolError::invalid("save_as must name a file, not a directory"));
    }
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
        return Err(ToolError::invalid(format!("save_as must end in .{ext}")));
    }
    let shown = path.strip_prefix(&app).unwrap_or(&path).display().to_string().replace('\\', "/");
    Ok((path, shown))
}

/// Go ahead if this stays within the budget, else ask the person (unless they said "always").
fn within_budget(ctx: &mut Ctx<'_>, inbox: &Inbox, ws: &mut Workspace, cost: f64) -> Result<(), ToolError> {
    valid_cost(cost)?;
    let spent = ws.try_assets_spent()?;
    let budget = ws.manifest.asset_budget_usd;
    valid_cost(spent)?;
    valid_cost(budget)?;
    valid_cost(spent + cost)?;
    if spent + cost <= budget + 1e-9 || ws.manifest.allowed_actions.iter().any(|k| k == OVER_BUDGET) {
        return Ok(());
    }
    // Auto mode doesn't stop to ask, and doesn't spend the person's money past the budget unasked.
    if ctx.session.settings.auto_mode {
        return Err(ToolError::new(ErrorCode::OverLimit, format!("This project's ${budget:.2} asset budget is used up (${spent:.2} so far)."))
            .with_remediation("Use simple shapes, colours and the assets already made for the rest. The person can raise the budget in the app."));
    }
    let title = format!("Spend about ${cost:.2} more on generated assets?");
    let detail = format!(
        "This project's ${budget:.2} budget for generated assets is used up (${spent:.2} so far). Allow once for this asset, or always to let the builder keep generating for this project."
    );
    match ask_approval(ctx, inbox, Some(ws), &title, &detail, OVER_BUDGET)? {
        Verdict::Yes => Ok(()),
        Verdict::No(_) | Verdict::NoAnswer => Err(ToolError::new(ErrorCode::OverLimit, "The asset budget is used up and the person didn't raise it.")
            .with_remediation("Use simple shapes, colours and the assets already made for the rest.")),
    }
}

/// Run one paid stage. When it fails after the provider accepted the work and reports a charge, that
/// charge is recorded as spending first, so a retry is checked against the budget it already used.
fn generate_paid(
    ws: &Workspace,
    provider: &dyn AssetProvider,
    key: &str,
    req: &AssetRequest<'_>,
    rel: &str,
    kind: &str,
    made_from: &str,
) -> Result<Generated, ToolError> {
    provider.generate(key, req).map_err(|e| record_charge(ws, rel, kind, made_from, e))
}

/// Record the charge a failed generation reported, and say so in its error. An invalid or
/// unrecordable charge is reported instead of being dropped.
fn record_charge(ws: &Workspace, rel: &str, kind: &str, made_from: &str, error: ToolError) -> ToolError {
    let Some(cost) = charged_usd(&error) else { return error };
    let receipt = || {
        valid_cost(cost)?;
        ws.record_asset(AssetRecord {
            file: rel.to_string(),
            kind: kind.into(),
            prompt: made_from.chars().take(1000).collect(),
            cost_usd: cost,
            at_ms: crate::inbox::now_ms(),
        })
    };
    match receipt() {
        Ok(()) => {
            let note =
                "The charge already counts against the project's asset budget. Do not blindly regenerate or retry: check the provider's job and account first.";
            let remediation = Some(error.remediation.as_deref().map_or_else(|| note.to_string(), |r| format!("{r} {note}")));
            ToolError {
                message: format!("{} The provider may have charged ${cost:.6}; this was recorded as spending for {rel}, which was not saved.", error.message),
                remediation,
                ..error
            }
        }
        Err(e) => {
            let mut failed = ToolError::new(
                e.code,
                format!("{} The provider may have charged ${cost:.6}, but the charge could not be recorded: {}", error.message, e.message),
            )
            .with_remediation("Do not blindly regenerate or retry: check the provider charge and repair the asset ledger first.");
            failed.details = error.details;
            failed
        }
    }
}

/// Record a paid generation before preparing or saving its output. A failed save keeps the receipt;
/// a failed receipt leaves the destination untouched. These two files are not a crash-atomic transaction.
fn keep(ws: &Workspace, path: &Path, rel: &str, kind: &str, made_from: &str, g: &Generated) -> Result<(), ToolError> {
    valid_cost(g.cost_usd)?;
    ws.record_asset(AssetRecord {
        file: rel.to_string(),
        kind: kind.into(),
        prompt: made_from.chars().take(1000).collect(),
        cost_usd: g.cost_usd,
        at_ms: crate::inbox::now_ms(),
    })
    .map_err(|e| {
        ToolError::new(
            e.code,
            format!("Asset spending could not be recorded; the provider may have charged ${:.6}. Output was not saved: {}", g.cost_usd, e.message),
        )
        .with_remediation("Do not blindly regenerate or retry: check the provider charge and repair the asset ledger first.")
    })?;
    let save = || {
        confined(&ws.root, &ws.app_dir())?;
        confined(&ws.app_dir(), path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ToolError::io(parent.display(), e))?;
        }
        confined(&ws.app_dir(), path)?;
        crate::project::write_atomic(path, &g.bytes)
    };
    save().map_err(|e| {
        ToolError::new(e.code, format!("Asset spending of ${:.6} was recorded, but saving {rel} failed: {}", g.cost_usd, e.message)).with_remediation(
            "Do not blindly regenerate or retry: the generation is already recorded as spending. Check the destination and provider output first.",
        )
    })
}

/// Report an overrun only after preserving both the charge and the generated output.
fn within_estimate(saved: &str, actual: f64, estimate: f64) -> Result<(), ToolError> {
    if actual > estimate + 1e-9 {
        return Err(ToolError::new(
            ErrorCode::OverLimit,
            format!("Saved {saved} and recorded ${actual:.6}, exceeding the approved stage estimate of ${estimate:.6}."),
        )
        .with_remediation("Do not blindly regenerate or retry: reuse the saved asset and review the recorded spending before continuing."));
    }
    Ok(())
}

/// Report checked spending after saving, without representing an unreadable ledger as an empty one.
fn summary(ws: &Workspace, saved: &str, cost: f64, extra: Value) -> Result<ToolOutput, ToolError> {
    let spent = ws.try_assets_spent().map_err(|e| {
        ToolError::new(e.code, format!("Saved {saved} and recorded ${cost:.6}, but the spending total could not be read: {}", e.message))
            .with_remediation("Do not blindly regenerate or retry: reuse the saved asset and check the asset ledger before continuing.")
    })?;
    let mut v =
        json!({"saved": saved, "cost_usd": round(cost), "spent_usd": round(spent), "budget_left_usd": round((ws.manifest.asset_budget_usd - spent).max(0.0))});
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    Ok(ToolOutput::Value(v))
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn prompt_ok(p: &str) -> Result<&str, ToolError> {
    let p = p.trim();
    if p.is_empty() || p.chars().count() > 2000 {
        return Err(ToolError::invalid("prompt must be 1..=2000 characters"));
    }
    Ok(p)
}

/// Resolve the model override and validate the destination, permission, key and estimate before saving a PNG.
fn assets_image_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ImageParams = parse_params(params)?;
    let prompt = prompt_ok(&p.prompt)?;
    let shape = match p.shape.as_deref() {
        None => Shape::Square,
        Some(s) => Shape::parse(s).ok_or_else(|| ToolError::invalid("shape must be square, wide or tall"))?,
    };
    let mut j = job(ctx)?;
    let (path, rel) = save_path(&j.ws, &p.save_as, "png")?;
    let model = Catalogue::built_in().image_model(selected(p.model.as_deref(), ctx.session.settings.assets.image_model.as_deref()))?;
    let req = AssetRequest::Image { model, prompt, shape };
    let provider = j.providers.for_request(&req)?;
    let key = j.key(provider.as_ref())?;
    j.ws.try_assets_spent()?;
    let estimate = provider.estimate(&req)?;
    within_budget(ctx, &j.inbox, &mut j.ws, estimate)?;
    ctx.report(None, "Making a picture…");
    let g = generate_paid(&j.ws, provider.as_ref(), &key, &req, &rel, "image", prompt)?;
    keep(&j.ws, &path, &rel, "image", prompt, &g)?;
    within_estimate(&rel, g.cost_usd, estimate)?;
    summary(&j.ws, &rel, g.cost_usd, json!({"provider": provider.id(), "model": model.id}))
}

/// Generate a GLB from an existing image or a newly generated precursor picture.
/// Preflight all participating providers and their combined estimate before either paid generation step.
fn assets_model_generate(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ModelParams = parse_params(params)?;
    let textured = p.textured.unwrap_or(true);
    let faces = p.faces.unwrap_or(DEFAULT_FACES);
    let catalogue = Catalogue::built_in();
    let model = catalogue.model_3d(selected(p.model.as_deref(), ctx.session.settings.assets.model_3d.as_deref()))?;
    let range = model.faces();
    if !range.contains(&faces) {
        return Err(ToolError::invalid(format!("faces must be {} to {}", range.start(), range.end())));
    }
    let mut j = job(ctx)?;
    let (path, rel) = save_path(&j.ws, &p.save_as, "glb")?;
    let image = p.image.trim();
    let from_prompt = image.is_empty();
    let prompt = if from_prompt { prompt_ok(&p.prompt)? } else { "" };
    // Quote the same trimmed, expanded prompt sent to generation: custom providers may price its length.
    let described =
        format!("{prompt}. One whole object, centred and fully in view, on a plain light-grey background, soft even lighting, three-quarter view, no text.");
    // The picture's address isn't known yet; the estimate doesn't depend on it.
    let model_req = AssetRequest::Model3d { model, image_url: "https://example.invalid/preflight.png", textured, faces };
    let provider = j.providers.for_request(&model_req)?;
    let key = j.key(provider.as_ref())?;
    // From a prompt, a picture is made first: with the picture model chosen in Settings.
    let mut picture_maker = None;
    if from_prompt {
        let image_model = catalogue.image_model(selected(p.image_model.as_deref(), ctx.session.settings.assets.image_model.as_deref()))?;
        let picture_req = AssetRequest::Image { model: image_model, prompt: &described, shape: Shape::Square };
        let picture_provider = j.providers.for_request(&picture_req)?;
        let picture_key = j.key(picture_provider.as_ref())?;
        let (pic_path, pic_rel) = save_path(&j.ws, &format!("{}.png", rel.trim_end_matches(".glb").trim_end_matches(".GLB")), "png")?;
        picture_maker = Some((image_model, picture_provider, picture_key, pic_path, pic_rel));
    }
    // All participating providers are allowed and keyed before asking any provider for a quote.
    j.ws.try_assets_spent()?;
    let model_estimate = provider.estimate(&model_req)?;
    valid_cost(model_estimate)?;
    let picture_estimate = if let Some((image_model, picture_provider, ..)) = &picture_maker {
        let req = AssetRequest::Image { model: image_model, prompt: &described, shape: Shape::Square };
        let picture_cost = picture_provider.estimate(&req)?;
        valid_cost(picture_cost)?;
        picture_cost
    } else {
        0.0
    };
    let source = if from_prompt { None } else { Some(image_source(&j.ws, image)?) };
    within_budget(ctx, &j.inbox, &mut j.ws, model_estimate + picture_estimate)?;
    let mut cost = 0.0;
    let mut picture = Value::Null;
    let (url, made_from) = match source {
        Some(src) => (src, format!("from {image}")),
        None => {
            let (image_model, picture_provider, picture_key, pic_path, pic_rel) =
                picture_maker.ok_or_else(|| ToolError::internal("no picture model was chosen"))?;
            ctx.report(None, "Making a picture of it first…");
            let picture_req = AssetRequest::Image { model: image_model, prompt: &described, shape: Shape::Square };
            let g = generate_paid(&j.ws, picture_provider.as_ref(), &picture_key, &picture_req, &pic_rel, "image", prompt)?;
            keep(&j.ws, &pic_path, &pic_rel, "image", prompt, &g)?;
            within_estimate(&pic_rel, g.cost_usd, picture_estimate)?;
            cost += g.cost_usd;
            picture = Value::String(pic_rel);
            (g.url, prompt.to_string())
        }
    };
    ctx.report(None, "Turning it into a 3D model (a minute or two)…");
    let model_job = AssetRequest::Model3d { model, image_url: &url, textured, faces };
    let g = generate_paid(&j.ws, provider.as_ref(), &key, &model_job, &rel, "model", &made_from)?;
    keep(&j.ws, &path, &rel, "model", &made_from, &g)?;
    within_estimate(&rel, g.cost_usd, model_estimate)?;
    cost += g.cost_usd;
    summary(&j.ws, &rel, cost, json!({"picture": picture, "textured": textured, "faces": faces, "provider": provider.id(), "model": model.id}))
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
    use crate::inbox::{Answer, InboxDir};
    use crate::workspace::TargetRef;
    use std::time::Duration;

    /// A game whose person allows generated assets.
    fn ws() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Workspace::create(dir.path(), "game", "A snake game", TargetRef::None).unwrap();
        w.set_person(|m| m.generate_assets = true).unwrap();
        (dir, w)
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

    /// Tool descriptions must retain the catalogue defaults, prices and guidance about model overrides.
    #[test]
    fn the_summaries_read_the_asset_table() {
        // The default picture model's name and price, and the default 3D model's.
        assert_eq!(
            summaries().0,
            "Make a picture with the selected provider (default: fal.ai Nano Banana Pro) and save it as a PNG inside app/: a texture, a sky or background, an icon, a title screen or concept art. shape: square (1024x1024, default), wide (1024x576) or tall (576x1024). Costs about $0.15, counted against the project's asset budget. Describe original designs in the original's style; never ask for recognisable characters, logos or brand artwork. Use model to override Settings; prices and limits depend on the chosen model. Returns where it was saved, the cost and the budget left."
        );
        assert_eq!(
            summaries().1,
            "Make a game-ready, low-poly 3D model (GLB) with the selected provider (default: fal.ai Tripo P1) and save it inside app/. Give a prompt (a picture is made first, then turned into the model) or an image (a web address, or a picture inside the project such as reference/picture-1.png). faces requests the face count (default generator range: 48 to 20000, default 3000): about 500-1500 for small props and pickups, 2000-5000 for weapons, furniture and vehicles, 6000-15000 for main characters; fewer faces run faster, so use the fewest that still look right. textured (default true) gives colours and surfaces. A textured model costs $0.50 plus $0.15 for the picture, counted against the project's asset budget; it takes a minute or two. Describe one original object per model; never ask for recognisable characters, logos or brand artwork. Use model to override Settings; prices and limits depend on the chosen model. Returns where it was saved, the cost and the budget left."
        );
    }

    /// Stands in for fal.ai: records each request's endpoint and answers with a few bytes.
    #[derive(Default)]
    struct StandIn(craft_anything_platform::sync::Mutex<Vec<String>>);

    impl craft_anything_engines::assets::AssetProvider for StandIn {
        fn id(&self) -> &'static str {
            "fal"
        }
        fn name(&self) -> &'static str {
            "stand-in"
        }
        fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
            assert_eq!(key, "k-1");
            let (endpoint, bytes) = match req {
                AssetRequest::Image { model, .. } => (model.endpoint.clone(), b"png".to_vec()),
                AssetRequest::Model3d { model, image_url, .. } => {
                    assert_eq!(*image_url, "https://stand-in/picture.png", "the model is made from the picture");
                    (model.endpoint.clone(), b"glb".to_vec())
                }
                _ => return Err(ToolError::invalid("the stand-in makes pictures and 3D models only")),
            };
            self.0.lock().unwrap().push(endpoint);
            Ok(Generated { url: "https://stand-in/picture.png".into(), bytes, cost_usd: req.listed_cost() })
        }
    }

    #[test]
    fn a_model_from_a_prompt_through_the_provider_registry() {
        let (_d, w) = ws();
        let stand_in = std::sync::Arc::new(StandIn::default());
        let mut providers = AssetProviders::built_in();
        providers.register(stand_in.clone());
        let mut session = crate::testing::session();
        session.settings.assets.image_model = Some("flux-schnell".into());
        let e = crate::Engine::new(session);
        e.session().ext.insert(std::sync::Arc::new(InboxDir(w.inbox_dir())));
        e.session().ext.insert(std::sync::Arc::new(AssetKeys([("fal".to_string(), "k-1".to_string())].into())));
        e.session().ext.insert(std::sync::Arc::new(providers));
        let out =
            e.call("assets_model_generate", json!({"prompt": "a crate", "save_as": "Assets/crate.glb"}), crate::CallOptions::new(crate::Caller::Mcp)).unwrap();
        let ToolOutput::Value(v) = out else { panic!("{out:?}") };
        assert_eq!(v["saved"], "Assets/crate.glb");
        assert_eq!(v["picture"], "Assets/crate.png");
        assert_eq!(*stand_in.0.lock().unwrap(), vec!["fal-ai/flux/schnell".to_string(), "tripo3d/p1/image-to-3d".to_string()], "the configured picture model");
        assert_eq!(std::fs::read(w.app_dir().join("Assets/crate.glb")).unwrap(), b"glb");
        let spent = w.assets_spent();
        let catalogue = Catalogue::built_in();
        let image = catalogue.image_model(Some("flux-schnell")).unwrap();
        let expected = 0.50 + AssetRequest::Image { model: image, prompt: "a crate", shape: Shape::Square }.listed_cost();
        assert!((spent - expected).abs() < 1e-9, "{spent}");
        assert_eq!(w.assets().len(), 2);
    }

    #[test]
    fn a_key_for_another_provider_doesnt_pay_this_one() {
        let (_d, w) = ws();
        let e = crate::Engine::new(crate::testing::session());
        e.session().ext.insert(std::sync::Arc::new(InboxDir(w.inbox_dir())));
        e.session().ext.insert(std::sync::Arc::new(AssetKeys([("fal".to_string(), " ".to_string())].into())));
        let err =
            e.call("assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"}), crate::CallOptions::new(crate::Caller::Mcp)).err().unwrap();
        assert!(err.code == ErrorCode::BadSettings, "a blank key is no key");
        let mut providers = AssetProviders::built_in();
        providers.register(std::sync::Arc::new(Named("other")));
        e.session().ext.insert(std::sync::Arc::new(providers));
        e.session().ext.insert(std::sync::Arc::new(AssetKeys([("other".to_string(), "o-1".to_string())].into())));
        let err =
            e.call("assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"}), crate::CallOptions::new(crate::Caller::Mcp)).err().unwrap();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.message.contains("No fal.ai key"), "{}", err.message);
        assert!(w.assets().is_empty() && Inbox::new(w.inbox_dir()).pending().is_empty(), "nothing asked or spent");
    }

    /// A provider that serves nothing in the catalogue.
    struct Named(&'static str);

    impl craft_anything_engines::assets::AssetProvider for Named {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn generate(&self, _key: &str, _req: &AssetRequest) -> Result<Generated, ToolError> {
            Err(ToolError::invalid("not here"))
        }
    }

    #[test]
    fn without_a_key_it_says_to_use_simple_shapes() {
        let (_d, w) = ws();
        let e = crate::Engine::new(crate::testing::session());
        e.session().ext.insert(std::sync::Arc::new(InboxDir(w.inbox_dir())));
        let err =
            e.call("assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"}), crate::CallOptions::new(crate::Caller::Mcp)).err().unwrap();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.remediation.unwrap_or_default().contains("simple shapes"));
        let err =
            e.call("assets_model_generate", json!({"prompt": "a crate", "save_as": "crate.glb", "faces": 40_000}), crate::CallOptions::new(crate::Caller::Mcp));
        assert!(err.err().unwrap().message.contains("faces must be 48 to 20000"), "Tripo P1's face limit");
    }

    #[test]
    fn nothing_is_generated_unless_the_project_allows_it() {
        let (_d, mut w) = ws();
        w.set_person(|m| m.generate_assets = false).unwrap();
        let e = crate::Engine::new(crate::testing::session());
        e.session().ext.insert(std::sync::Arc::new(InboxDir(w.inbox_dir())));
        e.session().ext.insert(std::sync::Arc::new(AssetKeys([("fal".to_string(), "k-1".to_string())].into())));
        let err =
            e.call("assets_image_generate", json!({"prompt": "a crate", "save_as": "crate.png"}), crate::CallOptions::new(crate::Caller::Mcp)).err().unwrap();
        assert_eq!(err.code, ErrorCode::NoPermission, "a key alone isn't the person's choice");
        assert!(err.remediation.unwrap_or_default().contains("simple shapes"));
        assert!(w.assets().is_empty() && Inbox::new(w.inbox_dir()).pending().is_empty(), "nothing asked or spent");
    }

    #[test]
    fn the_budget_asks_before_going_over_and_remembers_always() {
        let (_d, mut w) = ws();
        w.record_asset(AssetRecord { file: "a.glb".into(), kind: "model".into(), prompt: "x".into(), cost_usd: 4.80, at_ms: 0 }).unwrap();
        let inbox = Inbox::new(w.inbox_dir());
        let mut session = crate::testing::session();
        // Auto mode stops at the budget without asking.
        session.settings.auto_mode = true;
        {
            let mut ctx = Ctx { tool: "test", session: &mut session, cancel: crate::CancelToken::default(), progress: &|_| {}, caller: crate::Caller::Mcp };
            assert_eq!(within_budget(&mut ctx, &inbox, &mut w, 0.48).unwrap_err().code, ErrorCode::OverLimit);
            assert!(inbox.pending().is_empty(), "no question in Auto mode");
        }
        // Asking is what Auto mode switches off.
        session.settings.auto_mode = false;
        let mut ctx = Ctx { tool: "test", session: &mut session, cancel: crate::CancelToken::default(), progress: &|_| {}, caller: crate::Caller::Mcp };
        // Under the budget: no question.
        within_budget(&mut ctx, &inbox, &mut w, 0.03).unwrap();
        assert!(inbox.pending().is_empty());
        // Over it: asked; "no" stops, "always" lifts the cap for the project.
        let dir = w.inbox_dir();
        let answer = |allow: bool, always: bool| {
            let inbox = Inbox::new(dir.clone());
            std::thread::spawn(move || {
                for _ in 0..200 {
                    if let Some(r) = inbox.pending().first() {
                        assert!(r.title.contains("$0.48"), "{}", r.title);
                        inbox.answer(&r.id, &Answer { allow: Some(allow), always, ..Default::default() }).unwrap();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                panic!("no question came");
            })
        };
        let t = answer(false, false);
        assert_eq!(within_budget(&mut ctx, &inbox, &mut w, 0.48).unwrap_err().code, ErrorCode::OverLimit);
        t.join().unwrap();
        let t = answer(true, true);
        within_budget(&mut ctx, &inbox, &mut w, 0.48).unwrap();
        t.join().unwrap();
        assert!(Workspace::open(&w.root).unwrap().manifest.allowed_actions.iter().any(|k| k == OVER_BUDGET));
        within_budget(&mut ctx, &inbox, &mut w, 10.0).unwrap();
        assert!(inbox.pending().is_empty(), "after \"always\" it doesn't ask again");
        assert!((w.assets_spent() - 4.80).abs() < 1e-9);
    }

    struct Recording {
        id: &'static str,
        calls: craft_anything_platform::sync::Mutex<Vec<AssetKind>>,
        estimates: craft_anything_platform::sync::Mutex<Vec<AssetKind>>,
        estimate: Option<f64>,
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
        /// Record quote requests and return the injected amount or the request's catalogue price.
        fn estimate(&self, req: &AssetRequest) -> Result<f64, ToolError> {
            self.estimates.lock().unwrap().push(req.kind());
            Ok(self.estimate.unwrap_or_else(|| req.listed_cost()))
        }
        /// Require the test key, record the request kind and return fixed bytes without network access.
        fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
            assert_eq!(key, "sk-test");
            self.calls.lock().unwrap().push(req.kind());
            Ok(Generated { url: "https://stand-in/picture.png".into(), bytes: b"generated".to_vec(), cost_usd: req.listed_cost() })
        }
    }

    /// Fails after accepting the work and reports a charge, like a provider whose download broke.
    struct ChargedFailure(&'static str, f64);

    impl AssetProvider for ChargedFailure {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn generate(&self, key: &str, _req: &AssetRequest) -> Result<Generated, ToolError> {
            assert_eq!(key, "sk-test");
            let failed = ToolError::new(ErrorCode::ProgramFailed, "The job failed.").with_remediation("Check the job.");
            Err(craft_anything_engines::assets::charged(failed, self.1))
        }
    }

    /// An auto-mode engine whose keyed providers are the given stand-ins.
    fn engine_with(w: &Workspace, stand_ins: Vec<std::sync::Arc<dyn AssetProvider>>) -> crate::Engine {
        let mut providers = AssetProviders::built_in();
        let mut keys = std::collections::BTreeMap::new();
        for p in stand_ins {
            keys.insert(p.id().to_string(), "sk-test".to_string());
            providers.register(p);
        }
        let mut session = crate::testing::session();
        session.settings.auto_mode = true;
        session.ext.insert(std::sync::Arc::new(InboxDir(w.inbox_dir())));
        session.ext.insert(std::sync::Arc::new(AssetKeys(keys)));
        session.ext.insert(std::sync::Arc::new(providers));
        crate::Engine::new(session)
    }

    /// A failure after the provider accepted the work reaches the ledger, and later attempts are
    /// checked against it; a charge that can't be recorded is reported, never dropped.
    #[test]
    fn a_charged_failure_is_recorded_and_counts_against_the_budget() {
        let (_d, mut w) = ws();
        let engine = engine_with(&w, vec![std::sync::Arc::new(ChargedFailure("elevenlabs", 0.03))]);
        let call = |e: &crate::Engine| {
            let params = json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"audio/chime.mp3"});
            e.call("assets_sound_generate", params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err()
        };
        let error = call(&engine);
        assert_eq!(error.code, ErrorCode::ProgramFailed);
        assert!(error.message.contains("recorded as spending"), "{}", error.message);
        assert!(error.remediation.as_deref().is_some_and(|r| r.starts_with("Check the job.") && r.contains("asset budget")));
        let records = w.try_assets().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].file.as_str(), records[0].kind.as_str(), records[0].cost_usd), ("audio/chime.mp3", "sound", 0.03));
        assert!(!w.app_dir().join("audio/chime.mp3").exists(), "no output was saved");
        // With the budget at what was already charged, the next attempt is refused before spending.
        w.set_person(|m| m.asset_budget_usd = 0.03).unwrap();
        assert_eq!(call(&engine).code, ErrorCode::OverLimit);
        assert_eq!(w.try_assets().unwrap().len(), 1);
        // An invalid charge is reported as unrecorded, and the ledger is left as it was.
        w.set_person(|m| m.asset_budget_usd = 5.0).unwrap();
        let invalid = call(&engine_with(&w, vec![std::sync::Arc::new(ChargedFailure("elevenlabs", f64::NAN))]));
        assert!(invalid.message.contains("could not be recorded"), "{}", invalid.message);
        assert_eq!(w.try_assets().unwrap().len(), 1);
    }

    /// A model stage that fails after its picture was made records both charges.
    #[test]
    fn a_charged_model_failure_after_its_picture_records_both() {
        let (_d, mut w) = ws();
        // Scenario's mesh ceiling is above the default budget.
        w.set_person(|m| m.asset_budget_usd = 20.0).unwrap();
        let picture = std::sync::Arc::new(Recording { id: "fal", calls: Default::default(), estimates: Default::default(), estimate: None });
        let engine = engine_with(&w, vec![picture.clone(), std::sync::Arc::new(ChargedFailure("scenario", 0.4))]);
        let params = json!({"prompt":"a crate", "model":"scenario-meshy", "image_model":"flux-schnell", "save_as":"crate.glb"});
        let error = engine.call("assets_model_generate", params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err();
        assert!(error.message.contains("recorded as spending for crate.glb"), "{}", error.message);
        let records = w.try_assets().unwrap();
        assert_eq!(records.iter().map(|r| (r.file.as_str(), r.kind.as_str())).collect::<Vec<_>>(), [("crate.png", "image"), ("crate.glb", "model")]);
        assert!((records[1].cost_usd - 0.4).abs() < 1e-9);
        assert!(w.app_dir().join("crate.png").exists() && !w.app_dir().join("crate.glb").exists());
        assert_eq!(*picture.calls.lock().unwrap(), [AssetKind::Image]);
    }

    /// Build an isolated auto-mode engine with one keyed recording provider and an optional estimate override.
    fn recording_engine(w: &Workspace, provider: &'static str, estimate: Option<f64>) -> (crate::Engine, std::sync::Arc<Recording>) {
        let stub = std::sync::Arc::new(Recording { id: provider, calls: Default::default(), estimates: Default::default(), estimate });
        let mut providers = AssetProviders::built_in();
        providers.register(stub.clone());
        let mut session = crate::testing::session();
        session.settings.auto_mode = true;
        session.ext.insert(std::sync::Arc::new(InboxDir(w.inbox_dir())));
        session.ext.insert(std::sync::Arc::new(AssetKeys([(provider.to_string(), "sk-test".to_string())].into())));
        session.ext.insert(std::sync::Arc::new(providers));
        (crate::Engine::new(session), stub)
    }

    /// Provide valid speech, sound and music requests with distinct destinations and expected request kinds.
    fn audio_calls() -> Vec<(&'static str, Value, AssetKind)> {
        vec![
            ("assets_speech_generate", json!({"text":"A new adventure", "voice_id":"voice-test", "save_as":"audio/speech.mp3"}), AssetKind::Speech),
            ("assets_sound_generate", json!({"prompt":"A gentle chime", "duration_seconds":2.0, "save_as":"audio/chime.mp3"}), AssetKind::SoundEffect),
            ("assets_music_generate", json!({"prompt":"A calm piano theme", "duration_seconds":3.0, "save_as":"audio/theme.mp3"}), AssetKind::Music),
        ]
    }

    /// Disabled providers must be rejected before quotes, approval prompts, files or spending, even as precursors.
    #[test]
    fn disabled_providers_cannot_spend_through_audio_or_model_overrides_or_precursors() {
        let (_d, mut w) = ws();
        let (engine, audio) = recording_engine(&w, "elevenlabs", None);
        w.set_person(|m| m.asset_providers = Some(vec!["scenario".into()])).unwrap();
        for (tool, params, _) in audio_calls() {
            assert_eq!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::NoPermission);
        }
        assert!(audio.calls.lock().unwrap().is_empty());
        assert!(audio.estimates.lock().unwrap().is_empty());
        // No fal key is configured: the provider's permission must still win over missing keys.
        for (tool, params) in [
            ("assets_image_generate", json!({"prompt":"a crate", "model":"flux-schnell", "save_as":"crate.png"})),
            ("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})),
        ] {
            assert_eq!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::NoPermission);
        }
        let (engine, scenario) = recording_engine(&w, "scenario", None);
        // Scenario may make the model, but its default fal precursor is disabled.
        let params = json!({"prompt":"a crate", "model":"scenario-meshy", "image_model":"flux-schnell", "save_as":"crate.glb"});
        assert_eq!(engine.call("assets_model_generate", params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::NoPermission);
        assert!(scenario.calls.lock().unwrap().is_empty());
        assert!(scenario.estimates.lock().unwrap().is_empty());
        assert!(w.assets().is_empty());
        assert_eq!(w.assets_spent(), 0.0);
        assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
        assert!(!w.app_dir().join("crate.png").exists());
        assert!(!w.app_dir().join("crate.glb").exists());
    }

    /// A newer manifest version must block every generation tool without altering the manifest or asset ledger.
    #[test]
    fn future_project_settings_refuse_all_paid_generation() {
        let (_d, w) = ws();
        let (engine, stub) = recording_engine(&w, "elevenlabs", None);
        let mut future = w.manifest.clone();
        future.version = crate::workspace::MANIFEST_VERSION + 1;
        std::fs::write(w.root.join("craft.json"), serde_json::to_vec(&future).unwrap()).unwrap();
        let original = std::fs::read(w.root.join("craft.json")).unwrap();
        let mut calls: Vec<_> = audio_calls().into_iter().map(|(tool, params, _)| (tool, params)).collect();
        calls.push(("assets_image_generate", json!({"prompt":"a crate", "save_as":"crate.png"})));
        calls.push(("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})));
        for (tool, params) in calls {
            assert_eq!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::NoPermission);
        }
        assert!(stub.calls.lock().unwrap().is_empty());
        assert!(w.assets().is_empty());
        assert_eq!(std::fs::read(w.root.join("craft.json")).unwrap(), original);
    }

    /// Each audio tool must select its request kind, save the returned bytes and record exactly one paid asset.
    #[test]
    fn audio_tools_route_with_only_an_elevenlabs_key_and_record_each_asset_once() {
        let (_d, w) = ws();
        let (engine, stub) = recording_engine(&w, "elevenlabs", None);
        for (tool, params, kind) in audio_calls() {
            let out = engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap();
            let ToolOutput::Value(v) = out else { panic!("expected saved audio") };
            assert_eq!(v["provider"], "elevenlabs");
            let saved = v["saved"].as_str().unwrap();
            assert_eq!(std::fs::read(w.app_dir().join(saved)).unwrap(), b"generated");
            assert_eq!(stub.calls.lock().unwrap().last(), Some(&kind));
        }
        assert_eq!(w.assets().len(), 3);
        assert_eq!(stub.calls.lock().unwrap().len(), 3);
        assert!(w.assets_spent() > 0.0);
    }

    /// Auto-mode audio calls must stop before generation when opt-in is missing or the shared budget is exhausted.
    #[test]
    fn every_audio_tool_requires_opt_in_and_stops_before_spending_over_budget() {
        for (tool, params, _) in audio_calls() {
            let (_d, mut w) = ws();
            let (engine, stub) = recording_engine(&w, "elevenlabs", None);
            w.set_person(|m| m.generate_assets = false).unwrap();
            assert_eq!(engine.call(tool, params.clone(), crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::NoPermission);
            w.set_person(|m| {
                m.generate_assets = true;
                m.asset_budget_usd = 0.0;
            })
            .unwrap();
            assert_eq!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::OverLimit);
            assert!(stub.calls.lock().unwrap().is_empty());
            assert!(w.assets().is_empty());
            assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
        }
    }

    /// Invalid audio parameters, escaping paths and missing provider credentials must leave generation untouched.
    #[test]
    fn audio_validation_and_provider_keys_are_checked_before_paid_calls() {
        let (_d, w) = ws();
        let (engine, stub) = recording_engine(&w, "elevenlabs", None);
        for (tool, params) in [
            ("assets_speech_generate", json!({"text":" ","voice_id":"voice-test","save_as":"a.mp3"})),
            ("assets_speech_generate", json!({"text":"Hi","voice_id":"../voice","save_as":"a.mp3"})),
            ("assets_sound_generate", json!({"prompt":"chime","duration_seconds":0.0,"save_as":"a.mp3"})),
            ("assets_music_generate", json!({"prompt":"piano","duration_seconds":601.0,"save_as":"a.mp3"})),
            ("assets_music_generate", json!({"prompt":"piano","duration_seconds":3.0,"save_as":"../a.mp3"})),
            ("assets_sound_generate", json!({"prompt":"chime","duration_seconds":2.0,"save_as":"a.mp3","model":"eleven-music-v1"})),
        ] {
            assert!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).is_err());
        }
        engine.session().ext.insert(std::sync::Arc::new(AssetKeys([("fal".into(), "sk-test".into())].into())));
        let (tool, params, _) = audio_calls().remove(0);
        assert_eq!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err().code, ErrorCode::BadSettings);
        assert!(stub.calls.lock().unwrap().is_empty());
        assert!(w.assets().is_empty());
    }

    /// Model overrides take precedence, and a two-stage model request needs both keys and sufficient total budget.
    #[test]
    fn scenario_selection_overrides_settings_and_both_keys_and_total_budget_are_preflighted() {
        let (_d, mut w) = ws();
        let (engine, stub) = recording_engine(&w, "scenario", None);
        let options = || crate::CallOptions::new(crate::Caller::Mcp);
        engine.session().settings.assets.model_3d = Some("scenario-meshy".into());
        // The default image is fal: a Scenario key cannot pay for that first stage.
        let params = json!({"prompt":"a crate","save_as":"crate.glb"});
        assert_eq!(engine.call("assets_model_generate", params.clone(), options()).unwrap_err().code, ErrorCode::BadSettings);
        assert!(stub.calls.lock().unwrap().is_empty());
        engine.session().settings.assets.image_model = Some("scenario-gemini-3-1".into());
        w.set_person(|m| m.asset_budget_usd = 0.0).unwrap();
        assert_eq!(engine.call("assets_model_generate", params.clone(), options()).unwrap_err().code, ErrorCode::OverLimit);
        assert!(stub.calls.lock().unwrap().is_empty());
        w.set_person(|m| m.asset_budget_usd = 20.0).unwrap();
        engine.call("assets_model_generate", params, options()).unwrap();
        assert_eq!(*stub.calls.lock().unwrap(), vec![AssetKind::Image, AssetKind::Model3d]);
        assert_eq!(w.assets().len(), 2);
        engine.session().settings.assets.image_model = Some("not-a-model".into());
        engine.call("assets_image_generate", json!({"prompt":"a sky","save_as":"sky.png","model":"scenario-gemini-3-1"}), options()).unwrap();
        assert_eq!(w.assets().len(), 3);
        // Explicit fal 3D selection requires its key before generating a Scenario picture.
        let before = stub.calls.lock().unwrap().len();
        assert_eq!(
            engine
                .call(
                    "assets_model_generate",
                    json!({"prompt":"a barrel","save_as":"barrel.glb","model":"tripo-p1","image_model":"scenario-gemini-3-1"}),
                    options()
                )
                .unwrap_err()
                .code,
            ErrorCode::BadSettings
        );
        assert_eq!(stub.calls.lock().unwrap().len(), before);
    }

    /// Invalid provider quotes must fail before generation or any asset ledger entry.
    #[test]
    fn nonfinite_or_negative_provider_estimates_never_authorize_spending() {
        for estimate in [f64::NAN, f64::INFINITY, -0.1] {
            let (_d, w) = ws();
            let (engine, stub) = recording_engine(&w, "elevenlabs", Some(estimate));
            let (tool, params, _) = audio_calls().remove(0);
            assert!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).is_err());
            assert!(stub.calls.lock().unwrap().is_empty());
            assert!(w.assets().is_empty());
        }
    }

    /// Invalid actual costs must prevent both writing generated bytes and adding a spending record.
    #[test]
    fn invalid_returned_cost_is_never_saved_or_recorded() {
        let (_d, w) = ws();
        let (path, rel) = save_path(&w, "a.mp3", "mp3").unwrap();
        for cost_usd in [f64::NAN, f64::INFINITY, -1.0] {
            let made = Generated { url: String::new(), bytes: vec![1], cost_usd };
            assert!(keep(&w, &path, &rel, "speech", "hi", &made).is_err());
            assert!(!path.exists());
            assert!(w.assets().is_empty());
        }
    }

    /// Unreadable ledgers stop every paid tool before quotes, approval questions or provider calls.
    #[test]
    fn invalid_ledgers_block_all_generations_before_provider_calls() {
        for invalid in ["json", "directory", "negative", "overflow"] {
            let (_d, w) = ws();
            let ledger = w.root.join("assets.json");
            let receipt = |cost_usd| AssetRecord { file: "old.png".into(), kind: "image".into(), prompt: "x".into(), cost_usd, at_ms: 0 };
            match invalid {
                "directory" => std::fs::create_dir(&ledger).unwrap(),
                "negative" => std::fs::write(&ledger, serde_json::to_vec(&[receipt(-1.0)]).unwrap()).unwrap(),
                "overflow" => std::fs::write(&ledger, serde_json::to_vec(&[receipt(f64::MAX), receipt(f64::MAX)]).unwrap()).unwrap(),
                _ => std::fs::write(&ledger, "not JSON").unwrap(),
            }
            let original = std::fs::read(&ledger).ok();
            let (engine, audio) = recording_engine(&w, "elevenlabs", None);
            for (tool, params, _) in audio_calls() {
                assert!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).is_err());
            }
            let (engine, pictures) = recording_engine(&w, "fal", None);
            for (tool, params) in [
                ("assets_image_generate", json!({"prompt":"a crate", "save_as":"crate.png"})),
                ("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})),
            ] {
                assert!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).is_err());
            }
            assert!(audio.calls.lock().unwrap().is_empty() && audio.estimates.lock().unwrap().is_empty());
            assert!(pictures.calls.lock().unwrap().is_empty() && pictures.estimates.lock().unwrap().is_empty());
            assert!(Inbox::new(w.inbox_dir()).pending().is_empty());
            assert_eq!(std::fs::read(&ledger).ok(), original);
        }
    }

    /// A dangling ledger link cannot authorize a quote or generation and must remain untouched.
    #[cfg(unix)]
    #[test]
    fn dangling_asset_ledger_blocks_provider_calls() {
        let (_d, w) = ws();
        let ledger = w.root.join("assets.json");
        let target = w.root.join("missing-ledger.json");
        std::os::unix::fs::symlink(&target, &ledger).unwrap();
        let (engine, provider) = recording_engine(&w, "fal", None);
        for (tool, params) in [
            ("assets_image_generate", json!({"prompt":"a crate", "save_as":"crate.png"})),
            ("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"})),
        ] {
            assert!(engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).is_err());
        }
        assert!(provider.calls.lock().unwrap().is_empty() && provider.estimates.lock().unwrap().is_empty());
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
            let made = Generated { url: String::new(), bytes: b"replacement".to_vec(), cost_usd: 0.2 };
            let err = keep(&w, &path, &rel, "speech", "hi", &made).unwrap_err();
            assert!(err.message.contains("may have charged"));
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
        let (_d, w) = ws();
        let (path, rel) = save_path(&w, "a.mp3", "mp3").unwrap();
        let made = Generated { url: String::new(), bytes: b"generated".to_vec(), cost_usd: 0.2 };
        keep(&w, &path, &rel, "speech", "hi", &made).unwrap();
        std::fs::write(w.root.join("assets.json"), "broken").unwrap();
        let err = summary(&w, &rel, made.cost_usd, json!({})).unwrap_err();
        assert!(err.message.contains("Saved a.mp3 and recorded") && err.message.contains("total could not be read"));
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
            let made = Generated { url: String::new(), bytes: b"replacement".to_vec(), cost_usd: 0.2 };
            let err = keep(&w, &path, &rel, "speech", "hi", &made).unwrap_err();
            assert!(err.message.contains("was recorded, but saving"));
            assert!(err.remediation.unwrap().contains("Do not blindly"));
            assert_eq!(w.try_assets_spent().unwrap(), 0.2);
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
        let made = Generated { url: String::new(), bytes: b"replacement".to_vec(), cost_usd: 0.2 };
        assert!(keep(&w, &path, &rel, "speech", "hi", &made).unwrap_err().message.contains("was recorded, but saving"));
        assert_eq!(w.try_assets_spent().unwrap(), 0.2);
        assert_eq!(std::fs::read(target).unwrap(), b"original");
    }

    /// Overruns for every asset kind preserve output and charge, and return a non-retryable explanation.
    #[test]
    fn stage_overruns_are_reported_only_after_recording_and_saving() {
        let mut calls: Vec<_> = audio_calls().into_iter().map(|(tool, params, _)| ("elevenlabs", tool, params)).collect();
        calls.push(("fal", "assets_image_generate", json!({"prompt":"a crate", "save_as":"crate.png"})));
        calls.push(("fal", "assets_model_generate", json!({"image":"https://example.invalid/source.png", "save_as":"crate.glb"})));
        for (provider, tool, params) in calls {
            let (_d, w) = ws();
            let (engine, stub) = recording_engine(&w, provider, Some(0.0));
            let rel = params["save_as"].as_str().unwrap().to_string();
            let err = engine.call(tool, params, crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err();
            assert_eq!(err.code, ErrorCode::OverLimit);
            assert!(err.message.contains(&rel) && err.message.contains("recorded"));
            assert!(err.remediation.unwrap().contains("Do not blindly"));
            assert_eq!(std::fs::read(w.app_dir().join(rel)).unwrap(), b"generated");
            assert_eq!(w.try_assets().unwrap().len(), 1);
            assert!(w.try_assets_spent().unwrap() > 0.0);
            assert_eq!(stub.calls.lock().unwrap().len(), 1);
        }
    }

    /// An over-budget precursor is kept and recorded, but cannot authorize the subsequent model generation.
    #[test]
    fn precursor_overrun_stops_before_model_generation() {
        let (_d, w) = ws();
        let (engine, stub) = recording_engine(&w, "fal", Some(0.0));
        let err =
            engine.call("assets_model_generate", json!({"prompt":"a crate", "save_as":"crate.glb"}), crate::CallOptions::new(crate::Caller::Mcp)).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        assert!(err.message.contains("crate.png"));
        assert_eq!(*stub.calls.lock().unwrap(), [AssetKind::Image]);
        assert_eq!(std::fs::read(w.app_dir().join("crate.png")).unwrap(), b"generated");
        assert!(!w.app_dir().join("crate.glb").exists());
        assert_eq!(w.try_assets().unwrap().len(), 1);
    }

    /// A custom provider whose price depends on the exact prompt, exposing quote/generation mismatches.
    #[derive(Default)]
    struct PromptPriced {
        quoted: craft_anything_platform::sync::Mutex<Vec<String>>,
        generated: craft_anything_platform::sync::Mutex<Vec<String>>,
        model_calls: craft_anything_platform::sync::Mutex<usize>,
    }

    impl PromptPriced {
        /// Price every image character; charge a fixed amount for the downstream model.
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
        /// Capture exactly what was quoted.
        fn estimate(&self, req: &AssetRequest) -> Result<f64, ToolError> {
            if let AssetRequest::Image { prompt, .. } = req {
                self.quoted.lock().unwrap().push(prompt.to_string());
            }
            Ok(Self::price(req))
        }
        /// Capture exactly what was generated and return the matching charge.
        fn generate(&self, _key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
            match req {
                AssetRequest::Image { prompt, .. } => self.generated.lock().unwrap().push(prompt.to_string()),
                _ => *self.model_calls.lock().unwrap() += 1,
            }
            Ok(Generated { url: "https://example.invalid/source.png".into(), bytes: b"generated".to_vec(), cost_usd: Self::price(req) })
        }
    }

    /// Precursor quotes use the expanded trimmed prompt, while ledger provenance retains the person's description.
    #[test]
    fn precursor_quotes_and_generation_use_identical_expanded_prompts() {
        for budget in [0.507, 5.0] {
            let (_d, mut w) = ws();
            w.set_person(|m| m.asset_budget_usd = budget).unwrap();
            let (engine, _) = recording_engine(&w, "fal", None);
            let stub = std::sync::Arc::new(PromptPriced::default());
            let mut providers = AssetProviders::built_in();
            providers.register(stub.clone());
            engine.session().ext.insert(std::sync::Arc::new(providers));
            let result =
                engine.call("assets_model_generate", json!({"prompt":"  a crate  ", "save_as":"crate.glb"}), crate::CallOptions::new(crate::Caller::Mcp));
            let quoted = stub.quoted.lock().unwrap().clone();
            assert_eq!(quoted.len(), 1);
            assert!(quoted[0].starts_with("a crate. One whole object"));
            if budget < 1.0 {
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
        w.set_person(|m| m.asset_budget_usd = 20.0).unwrap();
        let (engine, pictures) = recording_engine(&w, "fal", None);
        engine.session().ext.insert(std::sync::Arc::new(AssetKeys([("fal".into(), "sk-test".into()), ("scenario".into(), "sk-test".into())].into())));
        let result = engine.call(
            "assets_model_generate",
            json!({"model":"scenario-meshy", "prompt":"a crate", "save_as":"crate.glb"}),
            crate::CallOptions::new(crate::Caller::Mcp),
        );
        assert!(result.is_err());
        assert!(pictures.calls.lock().unwrap().is_empty(), "reject a malformed downstream key before the first paid call");
        assert!(w.assets().is_empty());
    }

    /// Symlink escapes in inputs or outputs, including dangling links, must fail before paid generation.
    #[cfg(unix)]
    #[test]
    fn symlinked_outputs_and_inputs_cannot_escape_the_project() {
        let (_d, w) = ws();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(w.app_dir()).unwrap();
        std::os::unix::fs::symlink(outside.path(), w.app_dir().join("escape")).unwrap();
        std::fs::write(outside.path().join("picture.png"), b"png").unwrap();
        assert!(save_path(&w, "escape/new.mp3", "mp3").is_err());
        assert!(image_source(&w, "app/escape/picture.png").is_err());
        std::os::unix::fs::symlink(outside.path().join("new.mp3"), w.app_dir().join("broken.mp3")).unwrap();
        assert!(save_path(&w, "broken.mp3", "mp3").is_err());
        let (engine, stub) = recording_engine(&w, "elevenlabs", None);
        assert!(
            engine
                .call(
                    "assets_sound_generate",
                    json!({"prompt":"chime","duration_seconds":2.0,"save_as":"escape/new.mp3"}),
                    crate::CallOptions::new(crate::Caller::Mcp)
                )
                .is_err()
        );
        assert!(stub.calls.lock().unwrap().is_empty());
        assert!(!outside.path().join("new.mp3").exists());
    }
}
