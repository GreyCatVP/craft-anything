//! Generated game assets: pictures (textures, skies, concept art), game-ready 3D models, speech,
//! sound effects and music, made by a paid provider. Each provider is an [`AssetProvider`]; an
//! [`AssetProviders`] registry holds them (built in: [`fal::Fal`], [`scenario::Scenario`] and
//! [`elevenlabs::ElevenLabs`]). A provider lists its models live ([`AssetProvider::list_models`]),
//! describes the one a project chose ([`AssetProvider::describe`]: its inputs and its price), says
//! what a request will cost before anything is spent when it can ([`AssetProvider::estimate`]),
//! and reports what the finished run cost in its own unit ([`Generated::cost`]).

pub mod elevenlabs;
pub mod fal;
mod http;
pub mod inputs;
pub mod model;
pub mod openapi;
pub mod scenario;
pub mod tables;

use craft_anything_evidence::ToolError;
use craft_anything_platform::sync::Mutex;
use model::{AssetOption, ModelDetail, ModelList, Pricing, Rate, Unit};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// What kind of asset a request makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssetKind {
    Image,
    /// Named `model_3d` in files and tool parameters (`snake_case` alone would give `model3d`).
    #[serde(rename = "model_3d")]
    Model3d,
    Speech,
    SoundEffect,
    Music,
}

impl AssetKind {
    /// Every kind, in the order the screens list them.
    pub const ALL: [AssetKind; 5] = [AssetKind::Image, AssetKind::Model3d, AssetKind::Speech, AssetKind::SoundEffect, AssetKind::Music];

    /// The name the spending ledger has always used for this kind. It differs from the serde name
    /// for 3D models and sound effects, and old ledgers must keep reading.
    pub fn ledger_name(self) -> &'static str {
        match self {
            AssetKind::Image => "image",
            AssetKind::Model3d => "model",
            AssetKind::Speech => "speech",
            AssetKind::SoundEffect => "sound",
            AssetKind::Music => "music",
        }
    }

    /// The kind a ledger name stands for, or `None` for a name this build doesn't know.
    pub fn from_ledger_name(s: &str) -> Option<AssetKind> {
        AssetKind::ALL.into_iter().find(|k| k.ledger_name() == s)
    }

    /// The kind's id in files and tool parameters (`image`, `model_3d`, `sound_effect`...): the
    /// serde name, as a plain string for keys built from it.
    pub fn id(self) -> &'static str {
        match self {
            AssetKind::Image => "image",
            AssetKind::Model3d => "model_3d",
            AssetKind::Speech => "speech",
            AssetKind::SoundEffect => "sound_effect",
            AssetKind::Music => "music",
        }
    }
}

/// Picture shapes, with their pixel size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 1024×1024: objects, textures, icons.
    Square,
    /// 1024×576: skies, backgrounds, title screens.
    Wide,
    /// 576×1024: posters, tall props.
    Tall,
}

impl Shape {
    pub fn parse(s: &str) -> Option<Shape> {
        match s {
            "square" => Some(Shape::Square),
            "wide" => Some(Shape::Wide),
            "tall" => Some(Shape::Tall),
            _ => None,
        }
    }

    pub fn megapixels(self) -> f64 {
        let (width, height) = self.size();
        f64::from(width) * f64::from(height) / 1e6
    }

    /// The picture's width and height in pixels.
    pub fn size(self) -> (u32, u32) {
        match self {
            Shape::Square => (1024, 1024),
            Shape::Wide => (1024, 576),
            Shape::Tall => (576, 1024),
        }
    }
}

/// One thing to generate, with the model it asks for. It names no model: the project's chosen
/// model's own input fields say where each value goes ([`inputs::map_request`]).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum AssetRequest<'a> {
    /// A picture (PNG) from a description.
    Image {
        prompt: &'a str,
        shape: Shape,
    },
    /// A 3D model (GLB) of at most `faces` faces, from a picture (a web address, or a `data:` URI),
    /// a description, or both, as far as the model takes them.
    Model3d {
        image_url: Option<&'a str>,
        prompt: Option<&'a str>,
        textured: bool,
        faces: u32,
    },
    Speech {
        text: &'a str,
        voice_id: &'a str,
    },
    SoundEffect {
        prompt: &'a str,
        duration_seconds: f64,
    },
    Music {
        prompt: &'a str,
        duration_seconds: f64,
        instrumental: bool,
    },
}

impl AssetRequest<'_> {
    /// The kind of asset requested.
    pub fn kind(&self) -> AssetKind {
        match self {
            AssetRequest::Image { .. } => AssetKind::Image,
            AssetRequest::Model3d { .. } => AssetKind::Model3d,
            AssetRequest::Speech { .. } => AssetKind::Speech,
            AssetRequest::SoundEffect { .. } => AssetKind::SoundEffect,
            AssetRequest::Music { .. } => AssetKind::Music,
        }
    }
}

/// What one request comes to at a price measured `rate`, in the rate's unit; `None` when the rate
/// needs a measure the request doesn't have (a picture's size for a sound) or is only quoted.
/// A size is counted in whole megapixels, rounded up: an estimate that errs above the bill.
fn rated(rate: &Rate, req: &AssetRequest) -> Option<f64> {
    let amount = match (rate, *req) {
        (Rate::PerRequest(x), _) => *x,
        (Rate::PerMegapixel(x), AssetRequest::Image { shape, .. }) => x * shape.megapixels().ceil(),
        (Rate::PerSecond(x), AssetRequest::SoundEffect { duration_seconds, .. } | AssetRequest::Music { duration_seconds, .. }) => x * duration_seconds,
        (Rate::PerCharacter(x), AssetRequest::Speech { text, .. }) => x * text.chars().count() as f64,
        _ => return None,
    };
    amount.is_finite().then_some(amount)
}

/// What `req` will cost by the model's published price, before it runs: `None` when only the
/// finished run can say (a cost known after the run, a price quoted per run, or no price).
pub fn estimate_from(pricing: &Pricing, req: &AssetRequest) -> Option<f64> {
    match pricing {
        Pricing::Before { rate, .. } => rated(rate, req),
        Pricing::After { .. } | Pricing::Unknown => None,
    }
}

/// A generated file: its remote address (empty for audio sent back directly), its bytes, what the
/// provider says it cost in its own unit, and which options of the request the model ignored.
#[derive(Debug, Clone)]
pub struct Generated {
    pub url: String,
    pub bytes: Vec<u8>,
    /// What the run cost in the provider's [`AssetProvider::unit`]; `None` when the provider gave
    /// no cost that can be believed. The run is still counted, without an amount.
    pub cost: Option<f64>,
    /// The options of the request no input of the model took (see [`inputs::Mapped::ignored`]).
    pub ignored: Vec<AssetOption>,
}

/// A paid service that generates assets.
pub trait AssetProvider: Send + Sync {
    /// Stable id, named by a project's choice (`"fal"`).
    fn id(&self) -> &'static str;
    /// Shown to the person ("fal.ai").
    fn name(&self) -> &'static str;
    /// The environment variable the provider's own tools read its key from (`FAL_KEY`), used
    /// when no key is stored.
    fn key_env(&self) -> Option<&'static str> {
        None
    }
    /// Where the person gets a key (shown as a link in Settings → Builds → Asset provider keys).
    fn keys_page(&self) -> Option<&'static str> {
        None
    }
    /// Supply the default hint for the provider's primary credential field.
    fn key_hint(&self) -> &'static str {
        "API key"
    }
    /// An additional masked field, stored with the key as `key:secret` when present.
    fn key_secret_label(&self) -> Option<&'static str> {
        None
    }
    /// Validate the credential locally before a workflow spends on any precursor assets.
    fn validate_key(&self, key: &str) -> Result<(), ToolError> {
        http::key(key).map(|_| ())
    }
    /// What the provider bills in; every amount it reports is in this unit.
    fn unit(&self) -> Unit;
    /// The models this provider offers for `kind` that this app can run, with their prices. A key
    /// is used where the provider needs one to list or price its models.
    fn list_models(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError>;
    /// One model with its inputs and its price, as `key` gets them. A model the provider no longer
    /// serves, or is retiring, is a [`craft_anything_evidence::ErrorCode::BadSettings`] error naming it, so a generation
    /// that describes its model first is never charged for one that cannot run.
    fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError>;
    /// What `req` will cost with `model`, in [`AssetProvider::unit`], before anything is spent:
    /// `None` when only the finished run can say. A request the model can't take is an error
    /// here, before the limit is checked. By default, the model's published price.
    fn estimate(&self, _key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Option<f64>, ToolError> {
        Ok(estimate_from(&model.pricing, req))
    }
    /// Make `req` with `model` and the person's `key`. A failure after the provider accepted the
    /// paid work is returned through [`charged`] (or [`charged_unknown`]), so the caller records
    /// the charge even though no output arrived.
    fn generate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Generated, ToolError>;
    /// [`AssetProvider::generate`] for a run whose limit was checked against `approved`, the estimate
    /// [`AssetProvider::estimate`] gave. A provider that prices each run afresh just before it starts
    /// it (a quote) must not start one that now costs more: it refuses, with nothing spent. One whose
    /// estimate is the model's published price has nothing newer to compare with, which is the default.
    fn generate_within(&self, key: &str, model: &ModelDetail, req: &AssetRequest, approved: Option<f64>) -> Result<Generated, ToolError> {
        let _ = approved;
        self.generate(key, model, req)
    }
}

/// The [`ToolError::details`] field that carries a charge a failed generation already incurred:
/// a number, or `null` when the provider accepted the work but gave no cost for it.
const CHARGED_AMOUNT: &str = "charged_amount";

/// Mark a failure that happened after the provider accepted paid work costing about `amount`, in
/// the provider's unit. Existing details are kept beside the charge. An amount that is not a number
/// (JSON has none) is kept as an unknown one, as [`charged_unknown`] does: still a charge.
pub fn charged(error: ToolError, amount: f64) -> ToolError {
    with_charge(error, serde_json::json!(amount))
}

/// Mark a failure that happened after the provider accepted paid work whose cost it did not say.
/// The caller counts it as a generation without an amount.
pub fn charged_unknown(error: ToolError) -> ToolError {
    with_charge(error, serde_json::Value::Null)
}

/// [`charged`] with `amount`, else [`charged_unknown`].
pub fn charged_or_unknown(error: ToolError, amount: Option<f64>) -> ToolError {
    match amount {
        Some(amount) => charged(error, amount),
        None => charged_unknown(error),
    }
}

fn with_charge(mut error: ToolError, charge: serde_json::Value) -> ToolError {
    let mut details = match error.details.take() {
        Some(serde_json::Value::Object(map)) => map,
        Some(other) => serde_json::Map::from_iter([("provider".to_string(), other)]),
        None => serde_json::Map::new(),
    };
    details.insert(CHARGED_AMOUNT.into(), charge);
    error.details = Some(serde_json::Value::Object(details));
    error
}

/// Whether a failed generation reported a charge through [`charged`] or [`charged_unknown`].
pub fn was_charged(error: &ToolError) -> bool {
    error.details.as_ref().is_some_and(|d| d.get(CHARGED_AMOUNT).is_some())
}

/// The amount a failed generation reported through [`charged`]: `None` when it reported no charge
/// or a charge of unknown cost (tell them apart with [`was_charged`]). A charge that is present
/// but neither a number nor `null` reads as NaN, which no cost check passes: the caller records it
/// as a charge whose amount can't be read, never as no charge.
pub fn charged_amount(error: &ToolError) -> Option<f64> {
    match error.details.as_ref()?.get(CHARGED_AMOUNT)? {
        serde_json::Value::Null => None,
        v => Some(v.as_f64().unwrap_or(f64::NAN)),
    }
}

/// How long a model's description is used before it is asked for again.
const DESCRIBED_FOR: Duration = Duration::from_secs(10 * 60);
/// The most descriptions one provider keeps at a time.
const MAX_DESCRIBED: usize = 256;

/// Which description: a fingerprint of the key it was asked with (prices and models can be an
/// account's own), and the model.
type DescribedKey = (u64, String);

/// The models one provider described in the last ten minutes. Each provider keeps its own; the
/// built-in ones live as long as the process ([`AssetProviders::built_in`]).
#[derive(Default)]
pub(crate) struct Described {
    kept: Mutex<HashMap<DescribedKey, (Instant, ModelDetail)>>,
}

impl Described {
    /// What `describe` gives for `model`, or what it gave in the last ten minutes for the same key
    /// and model. Errors are not kept: a model that could not be described is asked about again
    /// next time. The lock is never held while `describe` runs.
    pub(crate) fn get_or(&self, key: &str, model: &str, describe: impl FnOnce() -> Result<ModelDetail, ToolError>) -> Result<ModelDetail, ToolError> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let id: DescribedKey = (hasher.finish(), model.to_string());
        if let Ok(kept) = self.kept.lock()
            && let Some((at, detail)) = kept.get(&id)
            && at.elapsed() < DESCRIBED_FOR
        {
            return Ok(detail.clone());
        }
        let detail = describe()?;
        if let Ok(mut kept) = self.kept.lock() {
            kept.retain(|_, (at, _)| at.elapsed() < DESCRIBED_FOR);
            if kept.len() >= MAX_DESCRIBED {
                kept.clear();
            }
            kept.insert(id, (Instant::now(), detail.clone()));
        }
        Ok(detail)
    }
}

/// The asset providers a project can use.
#[derive(Clone)]
pub struct AssetProviders {
    providers: Vec<Arc<dyn AssetProvider>>,
}

impl std::fmt::Debug for AssetProviders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.providers.iter().map(|p| p.id())).finish()
    }
}

impl Default for AssetProviders {
    fn default() -> Self {
        AssetProviders::built_in()
    }
}

impl AssetProviders {
    /// fal.ai, Scenario and ElevenLabs: the same ones for the whole process, so what they keep (the
    /// models they described lately) outlasts one tool call.
    pub fn built_in() -> AssetProviders {
        static BUILT_IN: OnceLock<AssetProviders> = OnceLock::new();
        BUILT_IN
            .get_or_init(|| AssetProviders {
                providers: vec![Arc::new(fal::Fal::default()), Arc::new(scenario::Scenario::default()), Arc::new(elevenlabs::ElevenLabs::default())],
            })
            .clone()
    }

    /// Add a provider, or replace the one with the same id.
    pub fn register(&mut self, provider: Arc<dyn AssetProvider>) {
        self.providers.retain(|p| p.id() != provider.id());
        self.providers.push(provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn AssetProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.id()).collect()
    }
}

/// Whether an amount of dollars or credits is a real one too small for two decimals to show, so
/// it is written "under 0.01…". The one place that decides this, so no amount reads two ways.
pub(crate) fn under_a_cent(x: f64) -> bool {
    x > 0.0 && x < 0.005
}

/// A price in prompt text: "$0.53", or "under $0.01" for a fraction of a cent.
pub fn usd(x: f64) -> String {
    if under_a_cent(x) { "under $0.01".into() } else { format!("${x:.2}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_evidence::ErrorCode;

    /// `AssetKind::ALL` is what the screens and the model lists walk. `next` names every variant
    /// with no catch-all, so a new variant fails to compile here until it has a place in the chain,
    /// and the chain then differs from `ALL` until the variant is added there too.
    #[test]
    fn all_lists_every_kind_in_order() {
        fn next(kind: AssetKind) -> Option<AssetKind> {
            match kind {
                AssetKind::Image => Some(AssetKind::Model3d),
                AssetKind::Model3d => Some(AssetKind::Speech),
                AssetKind::Speech => Some(AssetKind::SoundEffect),
                AssetKind::SoundEffect => Some(AssetKind::Music),
                AssetKind::Music => None,
            }
        }
        let mut chain = vec![AssetKind::Image];
        while let Some(kind) = chain.last().copied().and_then(next) {
            assert!(chain.len() <= AssetKind::ALL.len(), "the chain never ends: {chain:?}");
            chain.push(kind);
        }
        assert_eq!(chain, AssetKind::ALL);
    }

    #[test]
    fn amounts_under_half_a_cent_read_as_under_a_cent() {
        assert!(under_a_cent(0.0049) && !under_a_cent(0.005) && !under_a_cent(0.0));
        assert_eq!((usd(0.0049), usd(0.005)), ("under $0.01".to_string(), "$0.01".to_string()));
    }

    /// The size and the megapixels come from one place: a price per megapixel follows the pixels.
    #[test]
    fn shape_sizes_and_megapixels_agree() {
        assert_eq!([Shape::Square, Shape::Wide, Shape::Tall].map(Shape::size), [(1024, 1024), (1024, 576), (576, 1024)]);
        assert_eq!([Shape::Square, Shape::Wide, Shape::Tall].map(Shape::megapixels), [1.048576, 0.589824, 0.589824]);
    }

    /// A provider a fork registers: one model, priced per picture.
    struct Stub;

    impl AssetProvider for Stub {
        fn id(&self) -> &'static str {
            "stub"
        }
        fn name(&self) -> &'static str {
            "Stub"
        }
        fn unit(&self) -> Unit {
            Unit::Credits
        }
        fn list_models(&self, _key: Option<&str>, _kind: AssetKind) -> Result<ModelList, ToolError> {
            Ok(ModelList { models: Vec::new(), error: None })
        }
        fn describe(&self, _key: &str, model: &str) -> Result<ModelDetail, ToolError> {
            Ok(ModelDetail {
                id: model.into(),
                name: model.into(),
                kinds: vec![AssetKind::Image],
                fields: Vec::new(),
                pricing: Pricing::Before { unit: Unit::Credits, rate: Rate::PerRequest(3.0) },
                unit_price: None,
            })
        }
        fn generate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Generated, ToolError> {
            Ok(Generated { url: String::new(), bytes: Vec::new(), cost: self.estimate(key, model, req)?, ignored: Vec::new() })
        }
    }

    /// The built-in providers are registered, and one a fork adds is found by its id, with the
    /// default estimate taken from its model's published price.
    #[test]
    fn the_registry_finds_a_provider_by_its_id() {
        let mut r = AssetProviders::built_in();
        assert_eq!(r.ids(), vec!["fal", "scenario", "elevenlabs"]);
        assert_eq!(
            ["fal", "scenario", "elevenlabs"].map(|id| r.get(id).map(|p| p.unit())),
            [Some(Unit::Usd), Some(Unit::Credits), Some(Unit::Credits)],
            "dollars for fal.ai, credits for the others"
        );
        assert!(r.get("stub").is_none());
        r.register(Arc::new(Stub));
        let p = r.get("stub").unwrap();
        let model = p.describe("k", "stub-model").unwrap();
        let req = AssetRequest::Image { prompt: "a crate", shape: Shape::Square };
        assert_eq!(p.estimate("k", &model, &req).unwrap(), Some(3.0));
        assert_eq!(p.generate("k", &model, &req).unwrap().cost, Some(3.0));
        assert_eq!(req.kind(), AssetKind::Image);
    }

    #[test]
    fn kind_ids_are_the_names_files_and_tools_use() {
        for kind in AssetKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(kind.id()), "{kind:?}");
        }
    }

    /// A published rate gives an estimate for the requests it measures; a quote, a cost known after
    /// the run and no price give none.
    #[test]
    fn an_estimate_follows_the_published_rate() {
        let before = |rate| Pricing::Before { unit: Unit::Usd, rate };
        let picture = |shape| AssetRequest::Image { prompt: "a crate", shape };
        let speech = AssetRequest::Speech { text: "héllo", voice_id: "v" };
        let sound = AssetRequest::SoundEffect { prompt: "wind", duration_seconds: 2.5 };
        let music = AssetRequest::Music { prompt: "calm", duration_seconds: 10.0, instrumental: true };
        assert_eq!(estimate_from(&before(Rate::PerRequest(0.15)), &picture(Shape::Wide)), Some(0.15));
        assert_eq!(estimate_from(&before(Rate::PerRequest(0.4)), &sound), Some(0.4), "a price per run whatever the run");
        // Megapixels round up to whole ones: 1.05 is 2, 0.59 is 1.
        assert_eq!(estimate_from(&before(Rate::PerMegapixel(0.025)), &picture(Shape::Square)), Some(0.05));
        assert_eq!(estimate_from(&before(Rate::PerMegapixel(0.025)), &picture(Shape::Tall)), Some(0.025));
        assert_eq!(estimate_from(&before(Rate::PerCharacter(0.5)), &speech), Some(2.5), "characters, not bytes");
        assert_eq!(estimate_from(&before(Rate::PerSecond(40.0)), &sound), Some(100.0));
        assert_eq!(estimate_from(&before(Rate::PerSecond(15.0)), &music), Some(150.0));
        // A rate that doesn't measure this request says nothing about it.
        assert_eq!(estimate_from(&before(Rate::PerSecond(1.0)), &speech), None);
        assert_eq!(estimate_from(&before(Rate::PerMegapixel(1.0)), &music), None);
        assert_eq!(estimate_from(&before(Rate::PerCharacter(1.0)), &picture(Shape::Square)), None);
        for unpriced in [before(Rate::Quoted { typical: Some(20.0) }), Pricing::After { unit: Unit::Usd }, Pricing::Unknown] {
            assert_eq!(estimate_from(&unpriced, &picture(Shape::Square)), None, "{unpriced:?}");
        }
        // A rate times a length that overflows is no estimate.
        let endless = AssetRequest::SoundEffect { prompt: "wind", duration_seconds: f64::MAX };
        assert_eq!(estimate_from(&before(Rate::PerSecond(10.0)), &endless), None);
    }

    /// A failure after the provider accepted the work carries its charge: an amount, or none known.
    #[test]
    fn a_charge_is_an_amount_or_unknown() {
        let failed = || ToolError::new(ErrorCode::ProgramFailed, "The job failed.").with_details(serde_json::json!({"answer": "kept"}));
        let amount = charged(failed(), 0.5);
        assert!(was_charged(&amount));
        assert_eq!(charged_amount(&amount), Some(0.5));
        assert_eq!(amount.details.as_ref().and_then(|d| d.get("answer")), Some(&serde_json::json!("kept")), "other details stay");
        let unknown = charged_unknown(failed());
        assert!(was_charged(&unknown));
        assert_eq!(charged_amount(&unknown), None);
        assert!(!was_charged(&failed()) && charged_amount(&failed()).is_none(), "no charge at all");
        assert_eq!(charged_amount(&charged_or_unknown(failed(), Some(2.0))), Some(2.0));
        assert!(was_charged(&charged_or_unknown(failed(), None)));
        // An amount that is not a number is still a charge, of unknown amount.
        let nan = charged(failed(), f64::NAN);
        assert!(was_charged(&nan) && charged_amount(&nan).is_none());
        // A charge that is not a number is not dropped: it reads as NaN, which no cost check passes.
        let odd = failed().with_details(serde_json::json!({"charged_amount": "a lot"}));
        assert!(was_charged(&odd) && charged_amount(&odd).is_some_and(f64::is_nan));
        // Details that were not an object are kept under a name of their own.
        let bare = charged(ToolError::new(ErrorCode::ProgramFailed, "x").with_details(serde_json::json!("raw")), 1.0);
        assert_eq!(bare.details.unwrap()["provider"], "raw");
    }

    /// A model is described once for ten minutes per provider address, key and model; a failure is
    /// asked about again.
    #[test]
    fn a_description_is_kept_per_provider_key_and_model() {
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let describe = || {
            asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Stub.describe("k", "m")
        };
        let count = || asked.load(std::sync::atomic::Ordering::SeqCst);
        let (kept, elsewhere) = (Described::default(), Described::default());
        assert_eq!(kept.get_or("k", "m", describe).unwrap().id, "m");
        assert_eq!(kept.get_or("k", "m", describe).unwrap().id, "m");
        assert_eq!(count(), 1, "the second time is kept");
        kept.get_or("other key", "m", describe).unwrap();
        elsewhere.get_or("k", "m", describe).unwrap();
        kept.get_or("k", "another model", describe).unwrap();
        assert_eq!(count(), 4, "another key, provider or model is asked about");
        let gone = || Err(ToolError::new(ErrorCode::BadSettings, "gone"));
        assert!(kept.get_or("k", "gone", gone).is_err());
        assert!(kept.get_or("k", "gone", describe).is_ok(), "a failure is not kept");
    }

    /// The built-in providers are the same ones on every call, so what they keep lasts; a registry
    /// a caller changes is its own copy.
    #[test]
    fn the_built_in_providers_are_shared() {
        let (a, mut b) = (AssetProviders::built_in(), AssetProviders::built_in());
        for id in ["fal", "scenario", "elevenlabs"] {
            assert!(Arc::ptr_eq(&a.get(id).unwrap(), &b.get(id).unwrap()), "{id}");
        }
        b.register(Arc::new(Stub));
        assert!(a.get("stub").is_none() && AssetProviders::built_in().get("stub").is_none());
    }

    /// By default a run doesn't compare its price with the one approved: there is nothing newer.
    #[test]
    fn generating_within_an_approved_price_is_generating_by_default() {
        let model = Stub.describe("k", "m").unwrap();
        let req = AssetRequest::Image { prompt: "a crate", shape: Shape::Square };
        assert_eq!(Stub.generate_within("k", &model, &req, Some(0.0)).unwrap().cost, Some(3.0));
    }
}
