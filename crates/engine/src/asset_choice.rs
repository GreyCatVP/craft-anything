//! What a person picked for one asset type: a provider and one of its models, with the unit the
//! provider bills in and what the model costs. Settings keeps one per type as the default for new
//! projects, and each project keeps its own.

use craft_anything_engines::assets::model::{Pricing, Unit};
use serde::{Deserialize, Serialize};

/// The provider and model chosen for one asset type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetChoice {
    /// The provider's id (`fal`, `scenario`, `elevenlabs`); `None` is Off: nothing is generated.
    pub provider: Option<String>,
    /// The model's id at the provider.
    pub model: String,
    /// The model's name as shown to the person.
    pub model_name: String,
    /// What the provider bills in; limits and usage for this type are counted in it.
    pub unit: Unit,
    /// What the model cost when it was chosen or last listed.
    pub pricing: Pricing,
}

impl AssetChoice {
    /// No provider: this asset type is not generated.
    pub fn off() -> AssetChoice {
        AssetChoice { provider: None, model: String::new(), model_name: String::new(), unit: Unit::Usd, pricing: Pricing::Unknown }
    }
}

impl Default for AssetChoice {
    fn default() -> Self {
        AssetChoice::off()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_has_no_provider_and_a_partial_table_reads_as_off_for_what_is_missing() {
        assert_eq!(AssetChoice::off().provider, None);
        let c: AssetChoice = toml::from_str("provider = \"fal\"\nmodel = \"fal-ai/flux/dev\"\n").unwrap();
        assert_eq!(c.provider.as_deref(), Some("fal"));
        assert_eq!((c.model.as_str(), c.model_name.as_str(), c.unit, c.pricing), ("fal-ai/flux/dev", "", Unit::Usd, Pricing::Unknown));
        let empty: AssetChoice = toml::from_str("").unwrap();
        assert_eq!(empty, AssetChoice::off());
    }
}
