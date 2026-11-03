//! What the providers list for each asset type: a model, what it costs and in which unit, and
//! which of our options it ignores. Prices come from the provider; where it gives none, the
//! model's [`Pricing`] is [`Pricing::Unknown`] and its usage is counted without an amount.

use super::inputs::InputField;
use super::{AssetKind, under_a_cent, usd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most credits a limit can be set to (dollar limits stop at `MAX_ASSET_BUDGET_USD`).
pub const MAX_ASSET_CREDITS: f64 = 10_000_000.0;

/// What a provider bills in. Credits are never converted to dollars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// US dollars (fal.ai).
    Usd,
    /// The provider's own credits (Scenario, ElevenLabs).
    Credits,
}

/// How a price is measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rate {
    /// One price for each generation.
    PerRequest(f64),
    /// By the size of the picture.
    PerMegapixel(f64),
    /// By the length of the sound.
    PerSecond(f64),
    /// By the length of the text spoken.
    PerCharacter(f64),
    /// The provider quotes each run before it starts; `typical` is a usual quote, if it gives one.
    Quoted { typical: Option<f64> },
}

/// What a model costs, and when that is known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "when", rename_all = "snake_case")]
pub enum Pricing {
    /// The price is known before a run, so a limit can be checked first.
    Before { unit: Unit, rate: Rate },
    /// Only the finished run reports its cost, so a limit is checked against what is already spent.
    After { unit: Unit },
    /// The provider gives no usable price: runs are counted but have no amount.
    Unknown,
}

impl Pricing {
    /// The unit usage is counted in, or `None` when the price is unknown.
    pub fn unit(&self) -> Option<Unit> {
        match self {
            Pricing::Before { unit, .. } | Pricing::After { unit } => Some(*unit),
            Pricing::Unknown => None,
        }
    }

    /// Whether usage can be counted as an amount.
    pub fn tracked(&self) -> bool {
        self.unit().is_some()
    }

    /// The price as the picker shows it: "$0.15 a picture", "~20 credits a picture",
    /// "cost known after each run" or "price unknown".
    pub fn shown(&self, kind: AssetKind) -> String {
        match self {
            Pricing::Before { unit, rate } => rate.shown(*unit, kind),
            Pricing::After { .. } => "cost known after each run".into(),
            Pricing::Unknown => "price unknown".into(),
        }
    }
}

impl Rate {
    fn shown(&self, unit: Unit, kind: AssetKind) -> String {
        let (amount, per) = match *self {
            Rate::PerRequest(x) => (amount(unit, x), request_noun(kind)),
            Rate::PerMegapixel(x) => (rate_amount(unit, x), "a megapixel"),
            Rate::PerSecond(x) => (rate_amount(unit, x), "a second"),
            Rate::PerCharacter(x) => (rate_amount(unit, x), "a character"),
            Rate::Quoted { typical: Some(x) } => (quoted(unit, x), request_noun(kind)),
            Rate::Quoted { typical: None } => return "quoted before each run".into(),
        };
        format!("{amount} {per}")
    }
}

/// What one generation of this kind is called.
fn request_noun(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Image => "a picture",
        AssetKind::Model3d => "a 3D model",
        AssetKind::Speech => "a clip",
        AssetKind::SoundEffect => "a sound",
        AssetKind::Music => "a track",
    }
}

fn amount(unit: Unit, x: f64) -> String {
    match unit {
        Unit::Usd => usd(x),
        Unit::Credits => credits(x),
    }
}

/// The amount of a rate for one unit (a megapixel, a second, a character). Credits are shown as
/// they always are; a dollar rate is often a fraction of a cent, so it is not rounded to the cent.
fn rate_amount(unit: Unit, x: f64) -> String {
    match unit {
        Unit::Usd => usd_rate(x),
        Unit::Credits => credits(x),
    }
}

/// A dollar rate with up to three significant digits and at least two decimals: "$0.003",
/// "$0.025", "$0.10", "$12.00". Below a millionth of a dollar it reads "under".
fn usd_rate(x: f64) -> String {
    const SMALLEST: f64 = 1e-6;
    if x <= 0.0 || !x.is_finite() {
        return format!("${x:.2}");
    }
    if x < SMALLEST {
        return format!("under {}", usd_rate(SMALLEST));
    }
    // Two places past the first nonzero one keeps three significant digits.
    let decimals = if x >= 1.0 { 2 } else { (2.0 - x.log10().floor()) as usize };
    let fixed = format!("{x:.decimals$}");
    let trimmed = fixed.trim_end_matches('0');
    let (whole, fraction) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    format!("${whole}.{fraction:0<2}")
}

/// A typical quote: "~$0.20", or just the amount when that already reads "under …".
fn quoted(unit: Unit, x: f64) -> String {
    let shown = amount(unit, x);
    if under_a_cent(x) { shown } else { format!("~{shown}") }
}

/// "20 credits", "1 credit", "0.05 credits", or "under 0.01 credits" for a fraction of that.
fn credits(x: f64) -> String {
    if under_a_cent(x) {
        return "under 0.01 credits".into();
    }
    let fixed = format!("{x:.2}");
    let n = fixed.trim_end_matches('0').trim_end_matches('.');
    format!("{n} {}", if n == "1" { "credit" } else { "credits" })
}

/// Something a model may not use of what we pass it. The picker shows these so the person knows
/// the option has no effect with that model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssetOption {
    /// The picture's shape (square, wide, tall).
    Shape,
    /// The number of faces asked of a 3D model.
    Faces,
    /// Whether a 3D model has textures.
    Textured,
    /// How long a sound or a track lasts.
    Duration,
    /// Music without vocals.
    Instrumental,
    /// The voice speech is read in.
    Voice,
}

/// A model a provider lists for one or more asset types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedModel {
    /// The provider's id (`fal`, `scenario`, `elevenlabs`).
    pub provider: String,
    /// The model's id at the provider.
    pub id: String,
    /// Shown to the person.
    pub name: String,
    /// The asset types the model makes.
    pub kinds: Vec<AssetKind>,
    pub pricing: Pricing,
    /// Options we pass that this model does nothing with.
    pub ignores: Vec<AssetOption>,
}

/// One model with what the app needs to run it: the inputs it takes and what it costs.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDetail {
    /// The model's id at the provider.
    pub id: String,
    /// Shown to the person.
    pub name: String,
    /// The asset types the model makes.
    pub kinds: Vec<AssetKind>,
    /// Every input the provider describes for it.
    pub fields: Vec<InputField>,
    pub pricing: Pricing,
}

/// The models fetched for one asset type, with the reason the fetch failed if it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelList {
    pub models: Vec<ListedModel>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_units_keep_their_names() {
        assert_eq!(serde_json::to_string(&AssetKind::SoundEffect).unwrap(), "\"sound_effect\"");
        assert_eq!(serde_json::to_string(&AssetKind::Model3d).unwrap(), "\"model_3d\"", "not `model3d`");
        assert_eq!(serde_json::from_str::<AssetKind>("\"model_3d\"").unwrap(), AssetKind::Model3d);
        assert!(serde_json::from_str::<AssetKind>("\"model3d\"").is_err());
        assert_eq!(serde_json::to_string(&Unit::Credits).unwrap(), "\"credits\"");
        for k in AssetKind::ALL {
            assert_eq!(AssetKind::from_ledger_name(k.ledger_name()), Some(k));
        }
        assert_eq!(AssetKind::from_ledger_name("model"), Some(AssetKind::Model3d));
        assert_eq!(AssetKind::from_ledger_name("model_3d"), None, "the serde name is not a ledger name");
    }

    #[test]
    fn pricing_says_what_it_tracks() {
        let p = Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.15) };
        assert_eq!((p.unit(), p.tracked()), (Some(Unit::Usd), true));
        assert_eq!(p.shown(AssetKind::Image), "$0.15 a picture");
        assert_eq!(Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: Some(20.0) } }.shown(AssetKind::Image), "~20 credits a picture");
        assert_eq!(Pricing::After { unit: Unit::Usd }.shown(AssetKind::Image), "cost known after each run");
        assert!(!Pricing::Unknown.tracked());
        assert_eq!(Pricing::Unknown.shown(AssetKind::Music), "price unknown");
    }

    #[test]
    fn every_rate_names_what_it_is_charged_by() {
        let usd_rate = |rate| Pricing::Before { unit: Unit::Usd, rate };
        let credit_rate = |rate| Pricing::Before { unit: Unit::Credits, rate };
        assert_eq!(usd_rate(Rate::PerMegapixel(0.03)).shown(AssetKind::Image), "$0.03 a megapixel");
        assert_eq!(usd_rate(Rate::PerRequest(0.4)).shown(AssetKind::Model3d), "$0.40 a 3D model");
        assert_eq!(usd_rate(Rate::PerSecond(0.1)).shown(AssetKind::Music), "$0.10 a second");
        assert_eq!(credit_rate(Rate::PerCharacter(0.05)).shown(AssetKind::Speech), "0.05 credits a character");
        assert_eq!(credit_rate(Rate::PerSecond(1.0)).shown(AssetKind::SoundEffect), "1 credit a second");
        assert_eq!(credit_rate(Rate::PerRequest(0.0001)).shown(AssetKind::Speech), "under 0.01 credits a clip");
        assert_eq!(credit_rate(Rate::PerRequest(0.0)).shown(AssetKind::Music), "0 credits a track");
        assert_eq!(credit_rate(Rate::Quoted { typical: None }).shown(AssetKind::Image), "quoted before each run");
        assert_eq!(usd_rate(Rate::Quoted { typical: Some(0.2) }).shown(AssetKind::Image), "~$0.20 a picture");
        assert_eq!(usd_rate(Rate::Quoted { typical: Some(0.001) }).shown(AssetKind::Image), "under $0.01 a picture");
    }

    /// fal bills a megapixel, a second or a character in fractions of a cent, and the picker exists
    /// to compare them: a dollar rate per unit keeps up to three significant digits.
    #[test]
    fn dollar_rates_per_unit_keep_three_significant_digits() {
        let per = |rate| Pricing::Before { unit: Unit::Usd, rate }.shown(AssetKind::Image);
        assert_eq!(per(Rate::PerMegapixel(0.003)), "$0.003 a megapixel");
        assert_eq!(per(Rate::PerMegapixel(0.025)), "$0.025 a megapixel");
        assert_eq!(per(Rate::PerMegapixel(0.0262144)), "$0.0262 a megapixel");
        assert_eq!(per(Rate::PerSecond(0.0015)), "$0.0015 a second");
        assert_eq!(per(Rate::PerCharacter(0.00003)), "$0.00003 a character");
        assert_eq!(per(Rate::PerMegapixel(0.03)), "$0.03 a megapixel", "a round rate keeps two decimals, not more");
        assert_eq!(per(Rate::PerSecond(0.1)), "$0.10 a second");
        assert_eq!(per(Rate::PerSecond(12.0)), "$12.00 a second");
        assert_eq!(per(Rate::PerMegapixel(0.0099999)), "$0.01 a megapixel", "rounding up to the next place is not shown as 0.010");
        assert_eq!(per(Rate::PerCharacter(0.0)), "$0.00 a character");
        assert_eq!(per(Rate::PerCharacter(1e-9)), "under $0.000001 a character");
        // A price for a whole generation is still shown to the cent, and credits are untouched.
        assert_eq!(per(Rate::PerRequest(0.003)), "under $0.01 a picture");
        let credits = |rate| Pricing::Before { unit: Unit::Credits, rate }.shown(AssetKind::Speech);
        assert_eq!(credits(Rate::PerCharacter(0.003)), "under 0.01 credits a character");
    }

    /// The "~" says a quote is only typical. It is left off when the amount reads "under …", and
    /// that is decided on the number, never on the words the amount was written with.
    #[test]
    fn the_approximate_mark_follows_the_number() {
        let quoted = |unit, x| Pricing::Before { unit, rate: Rate::Quoted { typical: Some(x) } }.shown(AssetKind::Image);
        assert_eq!(quoted(Unit::Usd, 0.004), "under $0.01 a picture");
        assert_eq!(quoted(Unit::Credits, 0.004), "under 0.01 credits a picture");
        assert_eq!(quoted(Unit::Usd, 0.005), "~$0.01 a picture", "half a cent rounds to a cent: not under");
        assert_eq!(quoted(Unit::Credits, 20.0), "~20 credits a picture");
        assert_eq!(quoted(Unit::Credits, 0.0), "~0 credits a picture");
    }

    #[test]
    fn pricing_is_stored_with_a_when_tag() {
        let before = Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.15) };
        let json = serde_json::to_string(&before).unwrap();
        assert_eq!(json, r#"{"when":"before","unit":"usd","rate":{"per_request":0.15}}"#);
        assert_eq!(serde_json::from_str::<Pricing>(&json).unwrap(), before);
        assert_eq!(serde_json::to_string(&Pricing::Unknown).unwrap(), r#"{"when":"unknown"}"#);
        assert_eq!(serde_json::from_str::<Pricing>(r#"{"when":"after","unit":"credits"}"#).unwrap(), Pricing::After { unit: Unit::Credits });
    }

    #[test]
    fn listed_models_round_trip() {
        let list = ModelList {
            models: vec![ListedModel {
                provider: "fal".into(),
                id: "fal-ai/flux/dev".into(),
                name: "FLUX dev".into(),
                kinds: vec![AssetKind::Image],
                pricing: Pricing::Before { unit: Unit::Usd, rate: Rate::PerMegapixel(0.025) },
                ignores: vec![AssetOption::Voice],
            }],
            error: Some("rate limited".into()),
        };
        let json = serde_json::to_string(&list).unwrap();
        assert!(json.contains(r#""ignores":["voice"]"#), "{json}");
        assert_eq!(serde_json::from_str::<ModelList>(&json).unwrap(), list);
    }

    #[test]
    fn the_types_tools_take_have_a_schema() {
        let schema = |s: schemars::Schema| serde_json::to_string(&s).unwrap();
        assert!(schema(schemars::schema_for!(AssetKind)).contains("sound_effect"));
        assert!(schema(schemars::schema_for!(Unit)).contains("credits"));
        assert!(schema(schemars::schema_for!(AssetOption)).contains("instrumental"));
        let pricing = schema(schemars::schema_for!(Pricing));
        assert!(pricing.contains("\"when\"") && pricing.contains("per_megapixel"), "{pricing}");
    }
}
