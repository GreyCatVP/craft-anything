//! Generated game assets: pictures (textures, skies, concept art) and game-ready 3D models, made
//! by a paid provider. Each provider is an [`AssetProvider`]; an [`AssetProviders`] registry holds
//! them (built in: [`fal::Fal`]). The models and their prices come from the `assets.toml` table
//! ([`Catalogue`]), so a cost is known before anything is spent, and every generation reports
//! its estimated cost. Scenario uses a live CU quote within a table spending ceiling;
//! ElevenLabs supplies speech, sound effects and music.

pub mod elevenlabs;
pub mod fal;
mod http;
pub mod inputs;
pub mod model;
pub mod scenario;
pub mod tables;

use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;
use std::sync::{Arc, OnceLock};

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
}

/// How an audio model's published rate is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBilling {
    Character,
    Second,
}

/// An audio model and its estimated USD rate.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AudioModel {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub endpoint: String,
    pub kind: AssetKind,
    pub billing: AudioBilling,
    pub usd_per_unit: f64,
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
        match self {
            Shape::Square => 1024.0 * 1024.0 / 1e6,
            Shape::Wide | Shape::Tall => 1024.0 * 576.0 / 1e6,
        }
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

/// How a picture model is told the shape it should make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeBy {
    /// A named size (`square_hd`, `landscape_16_9`), as FLUX takes it.
    #[default]
    ImageSize,
    /// An aspect ratio (`1:1`, `16:9`), as Nano Banana Pro takes it.
    AspectRatio,
}

/// A picture model from the table.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ImageModel {
    /// The table id (`flux-dev`), used in Settings.
    pub id: String,
    /// The provider that runs it (`fal`).
    pub provider: String,
    /// Shown to the person.
    pub name: String,
    /// The model's address at the provider.
    pub endpoint: String,
    /// The price by size; a model billed per picture leaves it at 0.
    #[serde(default)]
    pub usd_per_megapixel: f64,
    /// Round decimal megapixels up for a conservative estimate, not an invoice measurement.
    #[serde(default)]
    pub round_up_megapixels: bool,
    /// The price per picture, whatever its size.
    #[serde(default)]
    pub usd_per_image: f64,
    #[serde(default)]
    pub size_by: SizeBy,
    /// The model weights' licence, for the person choosing one.
    #[serde(default)]
    pub licence: String,
}

impl ImageModel {
    /// Estimate a picture's cost using the table's rate and conservative rounding policy.
    pub fn cost(&self, shape: Shape) -> f64 {
        let megapixels = shape.megapixels();
        let megapixels = if self.round_up_megapixels { megapixels.ceil() } else { megapixels };
        self.usd_per_image + self.usd_per_megapixel * megapixels
    }
}

/// A 3D model generator from the table (a picture in, a GLB out).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Model3d {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub endpoint: String,
    /// Per model, untextured and textured.
    pub usd: f64,
    pub textured_usd: f64,
    /// The face counts it accepts, `[fewest, most]`.
    pub faces: [u32; 2],
    #[serde(default)]
    pub licence: String,
}

impl Model3d {
    pub fn cost(&self, textured: bool) -> f64 {
        if textured { self.textured_usd } else { self.usd }
    }

    pub fn faces(&self) -> RangeInclusive<u32> {
        let [low, high] = self.faces;
        low..=high
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Defaults {
    image: String,
    model_3d: String,
}

/// The models assets are made with and their prices (`assets.toml`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Catalogue {
    default: Defaults,
    #[serde(default)]
    image: Vec<ImageModel>,
    #[serde(default)]
    model_3d: Vec<Model3d>,
    #[serde(default)]
    audio: Vec<AudioModel>,
}

fn price_ok(usd: f64) -> bool {
    usd.is_finite() && usd >= 0.0
}

impl Catalogue {
    /// The built-in table. One that doesn't parse (a broken edit) gives [`Catalogue::fallback`],
    /// so assets keep working at the known prices; a test keeps the table parsing.
    pub fn built_in() -> &'static Catalogue {
        static TABLE: OnceLock<Catalogue> = OnceLock::new();
        TABLE.get_or_init(|| {
            Catalogue::parse(include_str!("assets.toml")).unwrap_or_else(|e| {
                log::warn!("assets.toml is unreadable ({e}); using the built-in asset models");
                Catalogue::fallback()
            })
        })
    }

    /// A table from TOML text. Entries with a price that isn't a positive number, an empty
    /// endpoint or an empty face range are left out (asking for them gives a clear error).
    pub fn parse(text: &str) -> Result<Catalogue, toml::de::Error> {
        let mut c: Catalogue = toml::from_str(text)?;
        c.image.retain(|m| price_ok(m.usd_per_megapixel) && price_ok(m.usd_per_image) && !m.endpoint.trim().is_empty());
        c.model_3d.retain(|m| {
            price_ok(m.usd) && price_ok(m.textured_usd) && !m.endpoint.trim().is_empty() && {
                let [low, high] = m.faces;
                low <= high
            }
        });
        c.audio.retain(|m| {
            m.usd_per_unit.is_finite()
                && m.usd_per_unit > 0.0
                && !m.endpoint.trim().is_empty()
                && matches!(
                    (m.kind, m.billing),
                    (AssetKind::Speech, AudioBilling::Character) | (AssetKind::SoundEffect | AssetKind::Music, AudioBilling::Second)
                )
        });
        Ok(c)
    }

    /// The defaults the table ships with, in code: Nano Banana Pro and Tripo P1 on fal.ai.
    pub fn fallback() -> Catalogue {
        Catalogue {
            default: Defaults { image: "nano-banana-pro".into(), model_3d: "tripo-p1".into() },
            image: vec![ImageModel {
                id: "nano-banana-pro".into(),
                provider: fal::ID.into(),
                name: "Nano Banana Pro".into(),
                endpoint: "fal-ai/nano-banana-pro".into(),
                usd_per_megapixel: 0.0,
                round_up_megapixels: false,
                usd_per_image: 0.15,
                size_by: SizeBy::AspectRatio,
                licence: "Commercial use enabled through fal.ai".into(),
            }],
            model_3d: vec![Model3d {
                id: "tripo-p1".into(),
                provider: fal::ID.into(),
                name: "Tripo P1".into(),
                endpoint: "tripo3d/p1/image-to-3d".into(),
                usd: 0.40,
                textured_usd: 0.50,
                faces: [48, 20_000],
                licence: String::new(),
            }],
            audio: Vec::new(),
        }
    }

    pub fn images(&self) -> &[ImageModel] {
        &self.image
    }

    pub fn models_3d(&self) -> &[Model3d] {
        &self.model_3d
    }

    /// Return the validated audio catalogue entries in their configured order.
    pub fn audio_models(&self) -> &[AudioModel] {
        &self.audio
    }

    /// Select a model of this modality; the first table entry is its default.
    pub fn audio_model(&self, kind: AssetKind, id: Option<&str>) -> Result<&AudioModel, ToolError> {
        let id = id.map(str::trim).filter(|s| !s.is_empty());
        self.audio.iter().find(|m| m.kind == kind && id.is_none_or(|id| m.id == id)).ok_or_else(|| {
            let known: Vec<&str> = self.audio.iter().filter(|m| m.kind == kind).map(|m| m.id.as_str()).collect();
            missing("audio model", id.unwrap_or("default"), &known)
        })
    }

    /// The picture model `id` names, or the default one when `id` is `None` or empty.
    pub fn image_model(&self, id: Option<&str>) -> Result<&ImageModel, ToolError> {
        let id = id.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&self.default.image);
        self.image.iter().find(|m| m.id == id).ok_or_else(|| {
            let known: Vec<&str> = self.image.iter().map(|m| m.id.as_str()).collect();
            missing("picture model", id, &known)
        })
    }

    /// The 3D model generator `id` names, or the default one.
    pub fn model_3d(&self, id: Option<&str>) -> Result<&Model3d, ToolError> {
        let id = id.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&self.default.model_3d);
        self.model_3d.iter().find(|m| m.id == id).ok_or_else(|| {
            let known: Vec<&str> = self.model_3d.iter().map(|m| m.id.as_str()).collect();
            missing("3D model generator", id, &known)
        })
    }
}

fn missing(what: &str, id: &str, known: &[&str]) -> ToolError {
    let remediation = if known.is_empty() {
        "The asset table lists none; generate assets with simple shapes and colours instead.".to_string()
    } else {
        format!("Choose one of: {} (Settings → AI → Game assets, or [assets] in config.toml).", known.join(", "))
    };
    ToolError::new(ErrorCode::BadSettings, format!("There is no {what} called \"{id}\" in the asset table.")).with_remediation(remediation)
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

/// One thing to generate, with the table entry that says where and at what price. The providers
/// still take this until the asset tools use [`AssetRequest`] and the providers' live models.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum LegacyRequest<'a> {
    /// A picture (PNG) from a description.
    Image {
        model: &'a ImageModel,
        prompt: &'a str,
        shape: Shape,
    },
    /// A 3D model (GLB) of at most `faces` faces from a picture: a web address, or a `data:` URI.
    Model3d {
        model: &'a Model3d,
        image_url: &'a str,
        textured: bool,
        faces: u32,
    },
    Speech {
        model: &'a AudioModel,
        text: &'a str,
        voice_id: &'a str,
    },
    SoundEffect {
        model: &'a AudioModel,
        prompt: &'a str,
        duration_seconds: f64,
    },
    Music {
        model: &'a AudioModel,
        prompt: &'a str,
        duration_seconds: f64,
        instrumental: bool,
    },
}

impl LegacyRequest<'_> {
    /// Identify the requested asset modality independently of its selected model.
    pub fn kind(&self) -> AssetKind {
        match self {
            LegacyRequest::Image { .. } => AssetKind::Image,
            LegacyRequest::Model3d { .. } => AssetKind::Model3d,
            LegacyRequest::Speech { .. } => AssetKind::Speech,
            LegacyRequest::SoundEffect { .. } => AssetKind::SoundEffect,
            LegacyRequest::Music { .. } => AssetKind::Music,
        }
    }

    /// The provider the table entry names.
    pub fn provider(&self) -> &str {
        match self {
            LegacyRequest::Image { model, .. } => &model.provider,
            LegacyRequest::Model3d { model, .. } => &model.provider,
            LegacyRequest::Speech { model, .. } | LegacyRequest::SoundEffect { model, .. } | LegacyRequest::Music { model, .. } => &model.provider,
        }
    }

    /// The table's price for it.
    pub fn listed_cost(&self) -> f64 {
        match self {
            LegacyRequest::Image { model, shape, .. } => model.cost(*shape),
            LegacyRequest::Model3d { model, textured, .. } => model.cost(*textured),
            LegacyRequest::Speech { model, text, .. } => text.chars().count() as f64 * model.usd_per_unit,
            LegacyRequest::SoundEffect { model, duration_seconds, .. } => duration_seconds * model.usd_per_unit,
            LegacyRequest::Music { model, duration_seconds, .. } => (duration_seconds * 1000.0).ceil() / 1000.0 * model.usd_per_unit,
        }
    }
}

/// A generated file: its remote address (empty for direct audio), bytes and estimated USD cost.
#[derive(Debug, Clone)]
pub struct Generated {
    pub url: String,
    pub bytes: Vec<u8>,
    pub cost_usd: f64,
}

/// A paid service that generates assets.
pub trait AssetProvider: Send + Sync {
    /// Stable id, named by the table's `provider` (`"fal"`).
    fn id(&self) -> &'static str;
    /// Shown to the person ("fal.ai").
    fn name(&self) -> &'static str;
    /// The environment variable the provider's own tools read its key from (`FAL_KEY`), used
    /// when no key is stored.
    fn key_env(&self) -> Option<&'static str> {
        None
    }
    /// Where the person gets a key (shown as a link in Settings → AI).
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
    /// What `req` will cost, before anything is spent (the budget is checked against it).
    fn estimate(&self, req: &LegacyRequest) -> Result<f64, ToolError> {
        Ok(req.listed_cost())
    }
    /// Make it with the person's `key`; the result carries the provider's estimated USD cost.
    /// A failure after the provider accepted the paid work is returned through [`charged`], so the
    /// caller records the charge even though no output arrived.
    fn generate(&self, key: &str, req: &LegacyRequest) -> Result<Generated, ToolError>;
}

/// The [`ToolError::details`] field that carries a charge a failed generation already incurred.
const CHARGED_USD: &str = "charged_usd";

/// Mark a failure that happened after the provider accepted paid work costing about `cost_usd`.
/// Existing details are kept beside the charge.
pub fn charged(mut error: ToolError, cost_usd: f64) -> ToolError {
    let mut details = match error.details.take() {
        Some(serde_json::Value::Object(map)) => map,
        Some(other) => serde_json::Map::from_iter([("provider".to_string(), other)]),
        None => serde_json::Map::new(),
    };
    details.insert(CHARGED_USD.into(), serde_json::json!(cost_usd));
    error.details = Some(serde_json::Value::Object(details));
    error
}

/// The charge a failed generation reported through [`charged`], if any. A charge that is present but
/// not a number reads as NaN, so the caller's cost check refuses it instead of ignoring it.
pub fn charged_usd(error: &ToolError) -> Option<f64> {
    error.details.as_ref()?.get(CHARGED_USD).map(|v| v.as_f64().unwrap_or(f64::NAN))
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
    /// fal.ai.
    pub fn built_in() -> AssetProviders {
        AssetProviders { providers: vec![Arc::new(fal::Fal::default()), Arc::new(scenario::Scenario::default()), Arc::new(elevenlabs::ElevenLabs::default())] }
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

    /// The provider that makes `req`.
    pub fn for_request(&self, req: &LegacyRequest) -> Result<Arc<dyn AssetProvider>, ToolError> {
        let id = req.provider();
        self.get(id).ok_or_else(|| {
            ToolError::new(ErrorCode::FeatureMissing, format!("No asset provider called \"{id}\" is available."))
                .with_remediation("Check the asset table, or generate assets with simple shapes and colours instead.")
        })
    }
}

/// A price in prompt text: "$0.53", or "under $0.01" for a fraction of a cent.
pub fn usd(x: f64) -> String {
    if x > 0.0 && x < 0.005 { "under $0.01".into() } else { format!("${x:.2}") }
}

/// "about $0.53", or "under $0.01".
pub fn about_usd(x: f64) -> String {
    if x > 0.0 && x < 0.005 { usd(x) } else { format!("about {}", usd(x)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_parses_and_its_defaults_match_the_code() {
        let t = Catalogue::parse(include_str!("assets.toml")).unwrap();
        let f = Catalogue::fallback();
        assert_eq!(t.image_model(None).unwrap(), f.image_model(None).unwrap());
        assert_eq!(t.model_3d(None).unwrap(), f.model_3d(None).unwrap());
        assert_eq!(Catalogue::built_in(), &t);
        let schnell = t.image_model(Some("flux-schnell")).unwrap();
        assert_eq!(schnell.endpoint, "fal-ai/flux/schnell");
        assert!(schnell.cost(Shape::Square) < t.image_model(Some("flux-dev")).unwrap().cost(Shape::Square));
        let nano = t.image_model(None).unwrap();
        assert_eq!((nano.id.as_str(), nano.endpoint.as_str(), nano.size_by), ("nano-banana-pro", "fal-ai/nano-banana-pro", SizeBy::AspectRatio));
        assert_eq!(t.image_model(Some("flux-dev")).unwrap().size_by, SizeBy::ImageSize, "a named size unless the table says otherwise");
        for m in t.images() {
            assert!(AssetProviders::built_in().get(&m.provider).is_some(), "{} names an unknown provider", m.id);
            assert_eq!(m.round_up_megapixels, matches!(m.id.as_str(), "flux-dev" | "flux-schnell"));
        }
    }

    #[test]
    fn prices_and_face_limits() {
        let c = Catalogue::built_in();
        let flux = c.image_model(Some("flux-dev")).unwrap();
        assert!((flux.cost(Shape::Square) - 0.05).abs() < 1e-9, "a conservative two decimal megapixels, not a verified invoice charge");
        assert!(flux.cost(Shape::Wide) < flux.cost(Shape::Square));
        let nano = c.image_model(None).unwrap();
        assert_eq!((nano.cost(Shape::Square), nano.cost(Shape::Wide)), (0.15, 0.15), "per picture, whatever its shape");
        let tripo = c.model_3d(Some(" ")).unwrap();
        assert_eq!((tripo.cost(true), tripo.cost(false)), (0.50, 0.40));
        assert_eq!(tripo.faces(), 48..=20_000);
        assert_eq!(about_usd(tripo.cost(true) + flux.cost(Shape::Square)), "about $0.55");
        assert_eq!(about_usd(tripo.cost(true) + nano.cost(Shape::Square)), "about $0.65");
        assert_eq!(about_usd(0.0031), "under $0.01");
        assert_eq!(usd(0.0262144), "$0.03");
    }

    /// Keep FLUX estimates above fractional decimal megapixels for every supported shape.
    #[test]
    fn flux_estimates_round_up_to_whole_decimal_megapixels() {
        let catalogue = Catalogue::built_in();
        for (id, rate) in [("flux-dev", 0.025), ("flux-schnell", 0.003)] {
            let model = catalogue.image_model(Some(id)).unwrap();
            for (shape, units) in [(Shape::Square, 2.0), (Shape::Wide, 1.0), (Shape::Tall, 1.0)] {
                assert_eq!(model.cost(shape), rate * units);
                let request = LegacyRequest::Image { model, prompt: "a crate", shape };
                assert_eq!(request.listed_cost(), rate * units);
                assert!(model.cost(shape) >= rate * shape.megapixels());
            }
        }
    }

    /// Leave flat image prices unchanged for every supported shape and rounding setting.
    #[test]
    fn flat_image_prices_do_not_depend_on_rounding() {
        for (id, price) in [("nano-banana-pro", 0.15), ("scenario-gemini-3-1", 1.0)] {
            let mut model = Catalogue::built_in().image_model(Some(id)).unwrap().clone();
            for round_up in [false, true] {
                model.round_up_megapixels = round_up;
                for shape in [Shape::Square, Shape::Wide, Shape::Tall] {
                    assert_eq!(model.cost(shape), price);
                }
            }
        }
    }

    /// Preserve fractional pricing for custom tables that omit the opt-in rounding policy.
    #[test]
    fn custom_models_default_to_fractional_megapixel_estimates() {
        let model: ImageModel = toml::from_str(
            r#"
            id = "custom-image"
            provider = "custom"
            name = "Custom image"
            endpoint = "custom/image"
            usd_per_megapixel = 0.01
            usd_per_image = 0.1
            "#,
        )
        .unwrap();
        assert!(!model.round_up_megapixels);
        for shape in [Shape::Square, Shape::Wide, Shape::Tall] {
            assert_eq!(model.cost(shape), 0.1 + 0.01 * shape.megapixels());
        }
    }

    #[test]
    fn a_broken_or_odd_table_never_panics() {
        assert!(Catalogue::parse("not = [toml").is_err());
        assert!(Catalogue::parse("").is_err(), "the defaults are required");
        let odd = Catalogue::parse(
            r#"
            [default]
            image = "free"
            model_3d = "upside-down"
            [[image]]
            id = "free"
            provider = "fal"
            name = "x"
            endpoint = "x"
            usd_per_image = -1.0
            [[model_3d]]
            id = "upside-down"
            provider = "fal"
            name = "x"
            endpoint = "x"
            usd = 0.1
            textured_usd = 0.2
            faces = [500, 10]
            "#,
        )
        .unwrap();
        let e = odd.image_model(None).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadSettings);
        assert!(e.message.contains("\"free\""), "{}", e.message);
        assert!(odd.model_3d(None).is_err());
        let e = Catalogue::fallback().image_model(Some("dall-e")).unwrap_err();
        assert!(e.remediation.unwrap_or_default().contains("nano-banana-pro"));
    }

    /// Register the built-in providers and route a model to a custom provider after registration.
    #[test]
    fn the_registry_finds_the_provider_a_model_names() {
        struct Stub;
        impl AssetProvider for Stub {
            fn id(&self) -> &'static str {
                "stub"
            }
            fn name(&self) -> &'static str {
                "Stub"
            }
            fn generate(&self, _key: &str, req: &LegacyRequest) -> Result<Generated, ToolError> {
                Ok(Generated { url: String::new(), bytes: Vec::new(), cost_usd: req.listed_cost() })
            }
        }
        let mut r = AssetProviders::built_in();
        assert_eq!(r.ids(), vec!["fal", "scenario", "elevenlabs"]);
        let mut m = Catalogue::fallback().image_model(None).unwrap().clone();
        m.provider = "stub".into();
        let req = LegacyRequest::Image { model: &m, prompt: "a crate", shape: Shape::Square };
        assert_eq!(r.for_request(&req).err().map(|e| e.code), Some(ErrorCode::FeatureMissing));
        r.register(Arc::new(Stub));
        let p = r.for_request(&req).unwrap();
        assert_eq!(p.estimate(&req).unwrap(), m.cost(Shape::Square));
        assert_eq!(p.generate("k", &req).unwrap().cost_usd, m.cost(Shape::Square));
        assert_eq!(req.kind(), AssetKind::Image);
    }
}
