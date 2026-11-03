//! What a project chose for its generated assets: for each asset type a provider and model, a
//! limit in the provider's unit with its own switch, and the project-wide "Ignore budget" switch.
//! The person owns all of it (see [`super::Workspace::set_person`]); Settings only supplies the
//! starting values for a new project.

use crate::asset_choice::AssetChoice;
use crate::settings::AssetSettings;
use craft_anything_engines::assets::AssetKind;
use craft_anything_engines::assets::model::Unit;
use craft_anything_evidence::ToolError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One asset type in a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TypeAssets {
    /// The provider and model this type is generated with (Off: nothing is generated).
    pub choice: AssetChoice,
    /// What the project may use of this type before the person is asked, in the choice's unit.
    pub limit: f64,
    /// Whether the limit is enforced. Off: usage is still counted, but nothing stops it.
    pub limit_on: bool,
    /// When this provider was chosen for the type (milliseconds since 1970): usage is counted
    /// from then, in this provider's unit. 0: from the start of the project.
    pub since_ms: u64,
}

impl Default for TypeAssets {
    /// Off, with the built-in dollar limit switched on: what a file with the fields missing means.
    fn default() -> Self {
        TypeAssets { choice: AssetChoice::off(), limit: super::ASSET_BUDGET_USD, limit_on: true, since_ms: 0 }
    }
}

/// A project's choices for generated assets (the `assets` object of `craft.json`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ProjectAssets {
    /// Every limit in the project is off at once; usage is still counted.
    pub ignore_budget: bool,
    /// What the person chose for each type. A type that isn't here has not been chosen in this
    /// project yet (a project from before version 4): it follows Settings' default for the type.
    pub types: BTreeMap<AssetKind, TypeAssets>,
    /// The one dollar budget a project of `craft.json` version 3 had. It stands in as the limit of
    /// the dollar-billed types that have not been chosen since.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_budget_usd: Option<f64>,
    /// The provider ids a project of `craft.json` version 3 allowed; none: all of them. A type
    /// that has not been chosen since is Off when its default provider isn't listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_providers: Option<Vec<String>>,
}

impl ProjectAssets {
    /// A new project: every type starts with Settings' default and limit, counted from `now_ms`,
    /// and "Ignore budget" as Settings has it. Changing Settings later does not change the project.
    pub fn from_defaults(s: &AssetSettings, now_ms: u64) -> ProjectAssets {
        let types = AssetKind::ALL
            .into_iter()
            .map(|kind| {
                let choice = s.choice(kind);
                let limit = s.default_limit(choice.unit);
                (kind, TypeAssets { choice, limit, limit_on: true, since_ms: now_ms })
            })
            .collect();
        ProjectAssets { ignore_budget: s.ignore_budget, types, legacy_budget_usd: None, legacy_providers: None }
    }

    /// What the project has for `kind`: what the person chose, else (a project from before
    /// version 4) Settings' default for the type, Off when the project's old provider list leaves
    /// that provider out, with the project's old budget as the limit of a dollar-billed type.
    pub fn effective(&self, kind: AssetKind, s: &AssetSettings) -> TypeAssets {
        if let Some(t) = self.types.get(&kind) {
            return t.clone();
        }
        let mut choice = s.choice(kind);
        let allowed = match (&choice.provider, &self.legacy_providers) {
            (Some(p), Some(ids)) => ids.iter().any(|id| id == p),
            _ => true,
        };
        if !allowed {
            choice = AssetChoice::off();
        }
        let limit = match (choice.unit, self.legacy_budget_usd) {
            (Unit::Usd, Some(budget)) => budget,
            (unit, _) => s.default_limit(unit),
        };
        TypeAssets { choice, limit, limit_on: true, since_ms: 0 }
    }

    /// Choose the provider and model for `kind`. The limit, its switch and the start of the count
    /// stay when the provider and its unit are the same as before. Another provider (Off counts as
    /// one) or unit starts the count afresh at `now_ms` with Settings' default limit in the new
    /// unit, switched on: a limit the person turned off for the old provider does not leave the new
    /// one unlimited.
    pub fn choose(&mut self, kind: AssetKind, choice: AssetChoice, s: &AssetSettings, now_ms: u64) {
        let before = self.effective(kind, s);
        let same = before.choice.provider == choice.provider && before.choice.unit == choice.unit;
        let t = if same {
            TypeAssets { choice, limit: before.limit, limit_on: before.limit_on, since_ms: before.since_ms }
        } else {
            TypeAssets { limit: s.default_limit(choice.unit), limit_on: true, since_ms: now_ms, choice }
        };
        self.types.insert(kind, t);
    }

    /// Change the limit of `kind` (in its provider's unit) and/or its switch. A limit outside what
    /// the unit allows is refused, and then nothing changes.
    pub fn set_limit(&mut self, kind: AssetKind, limit: Option<f64>, on: Option<bool>, s: &AssetSettings) -> Result<(), ToolError> {
        if limit.is_none() && on.is_none() {
            return Ok(());
        }
        let mut t = self.effective(kind, s);
        if let Some(l) = limit {
            let words = match t.choice.unit {
                Unit::Usd => "dollars",
                Unit::Credits => "credits",
            };
            if !t.choice.unit.accepts(l) {
                return Err(ToolError::invalid(format!("A limit must be a number from 0 to {:.0} {words}.", t.choice.unit.max_amount()))
                    .with_remediation("Pick a limit in that range, or turn the limit off."));
            }
            t.limit = l;
        }
        if let Some(on) = on {
            t.limit_on = on;
        }
        self.types.insert(kind, t);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AssetSettings;
    use crate::workspace::{AssetRecord, TypeUsage};
    use crate::workspace::{MANIFEST, Manifest, TargetRef, Workspace, read_manifest};
    use craft_anything_engines::assets::AssetKind;
    use craft_anything_engines::assets::model::{MAX_ASSET_CREDITS, Pricing, Unit};
    use serde_json::{Value, json};

    fn pick(provider: &str, model: &str, unit: Unit) -> AssetChoice {
        AssetChoice { provider: Some(provider.into()), model: model.into(), model_name: model.into(), unit, pricing: Pricing::After { unit } }
    }

    /// Settings defaults: pictures at fal (dollars), 3D models at Scenario and music at ElevenLabs (credits).
    fn settings() -> AssetSettings {
        let mut s = AssetSettings::default();
        s.defaults.insert(AssetKind::Image, pick("fal", "fal-ai/flux/dev", Unit::Usd));
        s.defaults.insert(AssetKind::Model3d, pick("scenario", "model_meshy-7-1-img23d", Unit::Credits));
        s.defaults.insert(AssetKind::Music, pick("elevenlabs", "music_v1", Unit::Credits));
        s
    }

    fn v3(extra: &str) -> Vec<u8> {
        format!(r#"{{"version":3,"name":"Old","request":"x","target":{{"kind":"none"}},"created_ms":1,"updated_ms":1,"generate_assets":true{extra}}}"#)
            .into_bytes()
    }

    /// A project's choice for a type, as `try_type_usage` is asked about it.
    fn typed(provider: &str, unit: Unit, since_ms: u64) -> TypeAssets {
        TypeAssets { choice: pick(provider, "m", unit), limit: 5.0, limit_on: true, since_ms }
    }

    /// A generation as the ledger holds it; no amount means the provider reported no cost.
    fn made(kind: &str, provider: &str, unit: Option<Unit>, amount: Option<f64>, at_ms: u64) -> AssetRecord {
        AssetRecord {
            file: format!("{kind}-{at_ms}"),
            kind: kind.into(),
            prompt: "p".into(),
            provider: provider.into(),
            model: "m".into(),
            amount,
            unit,
            cost_missing: amount.is_none(),
            at_ms,
        }
    }

    fn new_workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::create(dir.path(), "game", "A snake game", TargetRef::None).unwrap();
        (dir, ws)
    }

    const OLD_LEDGER: &str = r#"[{"file":"a.png","kind":"image","prompt":"a crate","cost_usd":2.25,"at_ms":7}]"#;

    #[test]
    fn an_old_ledger_reads_as_dollars_of_unknown_provider() {
        let (_d, ws) = new_workspace();
        let path = ws.root.join("assets.json");
        std::fs::write(&path, OLD_LEDGER).unwrap();
        let expected = AssetRecord {
            file: "a.png".into(),
            kind: "image".into(),
            prompt: "a crate".into(),
            provider: String::new(),
            model: String::new(),
            amount: Some(2.25),
            unit: Some(Unit::Usd),
            cost_missing: false,
            at_ms: 7,
        };
        assert_eq!(ws.try_assets().unwrap(), std::slice::from_ref(&expected));
        assert_eq!(ws.try_assets_spent().unwrap(), 2.25);
        let usage = ws.try_type_usage(AssetKind::Image, &typed("fal", Unit::Usd, 0)).unwrap();
        assert_eq!(usage, TypeUsage { amount: 2.25, generations: 1, cost_missing: false });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), OLD_LEDGER, "reading changes nothing");
        // The next receipt writes the new shape and keeps the old one as it was read.
        let next = made("sound", "elevenlabs", Some(Unit::Credits), Some(30.0), 9);
        ws.record_asset(next.clone()).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(saved["records"][0]["amount"], 2.25);
        assert!(saved["records"][0].get("cost_usd").is_none());
        assert_eq!(ws.try_assets().unwrap(), [expected, next]);
    }

    #[test]
    fn a_record_from_before_providers_counts_only_for_a_dollar_type_counted_from_the_start() {
        let (_d, ws) = new_workspace();
        ws.record_asset(made("image", "", Some(Unit::Usd), Some(2.0), 7)).unwrap();
        ws.record_asset(made("sound", "", Some(Unit::Usd), Some(9.0), 8)).unwrap();
        let usage = |kind, t: &TypeAssets| ws.try_type_usage(kind, t).unwrap();
        let one = TypeUsage { amount: 2.0, generations: 1, cost_missing: false };
        assert_eq!(usage(AssetKind::Image, &typed("fal", Unit::Usd, 0)), one, "the dollar type counted from the start of the project");
        assert_eq!(usage(AssetKind::Image, &typed("fal", Unit::Usd, 5)), TypeUsage::default(), "a count that began later leaves the old spending out");
        assert_eq!(usage(AssetKind::Image, &typed("scenario", Unit::Credits, 0)), TypeUsage::default(), "dollars are not credits");
        assert_eq!(
            usage(AssetKind::Image, &TypeAssets { choice: AssetChoice::off(), ..typed("fal", Unit::Usd, 0) }),
            TypeUsage::default(),
            "Off counts nothing"
        );
        assert_eq!(usage(AssetKind::SoundEffect, &typed("elevenlabs", Unit::Usd, 0)).amount, 9.0, "ledger names, not tool names");
        assert_eq!(usage(AssetKind::Music, &typed("fal", Unit::Usd, 0)), TypeUsage::default(), "another type");
    }

    #[test]
    fn usage_counts_only_the_current_provider_unit_and_period() {
        let (_d, ws) = new_workspace();
        for r in [
            made("image", "fal", Some(Unit::Usd), Some(1.0), 10),
            made("image", "scenario", Some(Unit::Credits), Some(30.0), 20),
            made("image", "scenario", Some(Unit::Credits), Some(40.0), 30),
            made("model", "scenario", Some(Unit::Credits), Some(99.0), 30),
        ] {
            ws.record_asset(r).unwrap();
        }
        let usage = |kind, t: &TypeAssets| ws.try_type_usage(kind, t).unwrap();
        let scenario = typed("scenario", Unit::Credits, 25);
        assert_eq!(usage(AssetKind::Image, &scenario), TypeUsage { amount: 40.0, generations: 1, cost_missing: false });
        // The start of the period is included; everything earlier is not.
        assert_eq!(usage(AssetKind::Image, &typed("scenario", Unit::Credits, 30)).generations, 1);
        assert_eq!(usage(AssetKind::Image, &typed("scenario", Unit::Credits, 31)).generations, 0);
        assert_eq!(usage(AssetKind::Image, &typed("scenario", Unit::Credits, 0)), TypeUsage { amount: 70.0, generations: 2, cost_missing: false });
        // Another provider's, and another kind's, spending is not this type's.
        assert_eq!(usage(AssetKind::Image, &typed("fal", Unit::Usd, 0)), TypeUsage { amount: 1.0, generations: 1, cost_missing: false });
        assert_eq!(usage(AssetKind::Model3d, &scenario).amount, 99.0, "the ledger calls 3D models `model`");
        // A record in another unit than the type's does not count, even at the same provider.
        assert_eq!(usage(AssetKind::Image, &typed("scenario", Unit::Usd, 0)), TypeUsage::default());
    }

    #[test]
    fn a_generation_without_a_cost_is_flagged() {
        let (_d, ws) = new_workspace();
        ws.record_asset(made("image", "scenario", Some(Unit::Credits), Some(10.0), 1)).unwrap();
        let none = made("image", "scenario", None, None, 2);
        assert!(none.cost_missing);
        ws.record_asset(none).unwrap();
        // A cost the provider gave in part is still flagged when the record says so.
        let mut partly = made("music", "elevenlabs", Some(Unit::Credits), Some(4.0), 2);
        partly.cost_missing = true;
        ws.record_asset(partly).unwrap();
        ws.record_asset(made("image", "fal", Some(Unit::Usd), None, 3)).unwrap();
        // No amount and not flagged: a model with no price, whose usage is counted without amounts.
        // It is a generation, not a missing cost: once the model has a price, nothing earlier is unknown.
        let unpriced = AssetRecord { cost_missing: false, ..made("speech", "elevenlabs", Some(Unit::Credits), None, 4) };
        ws.record_asset(unpriced).unwrap();
        assert_eq!(
            ws.try_type_usage(AssetKind::Speech, &typed("elevenlabs", Unit::Credits, 0)).unwrap(),
            TypeUsage { amount: 0.0, generations: 1, cost_missing: false }
        );
        let scenario = typed("scenario", Unit::Credits, 0);
        assert_eq!(ws.try_type_usage(AssetKind::Image, &scenario).unwrap(), TypeUsage { amount: 10.0, generations: 2, cost_missing: true });
        assert_eq!(
            ws.try_type_usage(AssetKind::Music, &typed("elevenlabs", Unit::Credits, 0)).unwrap(),
            TypeUsage { amount: 4.0, generations: 1, cost_missing: true }
        );
        // Another provider's missing cost, or one before the period, does not flag this type.
        let only_priced = typed("scenario", Unit::Credits, 0);
        assert!(!ws.try_type_usage(AssetKind::Speech, &only_priced).unwrap().cost_missing);
        assert!(!ws.try_type_usage(AssetKind::Image, &typed("scenario", Unit::Credits, 3)).unwrap().cost_missing);
        assert_eq!(ws.try_type_usage(AssetKind::Image, &typed("fal", Unit::Usd, 0)).unwrap(), TypeUsage { amount: 0.0, generations: 1, cost_missing: true });
    }

    #[test]
    fn the_new_ledger_is_versioned_and_damage_fails_closed() {
        let (_d, ws) = new_workspace();
        let path = ws.root.join("assets.json");
        assert!(ws.try_assets().unwrap().is_empty(), "no file: no spending");
        ws.record_asset(made("image", "fal", Some(Unit::Usd), Some(1.5), 1)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.split_whitespace().collect::<String>().starts_with(r#"{"version":1,"records":[{"#), "{text}");
        assert_eq!(ws.try_assets().unwrap().len(), 1);

        let usd = |amount: Value| json!({"file":"f","kind":"image","prompt":"p","provider":"fal","model":"m","amount":amount,"unit":"usd","cost_missing":false,"at_ms":1});
        let v1 = |records: Value| json!({"version": 1, "records": records}).to_string();
        let cases = [
            ("garbage", "not json".to_string()),
            ("empty", String::new()),
            ("a number", "7".to_string()),
            ("truncated", text[..text.len() / 2].to_string()),
            ("no version", json!({"records": []}).to_string()),
            ("version 0 object", json!({"version": 0, "records": []}).to_string()),
            ("version as text", json!({"version": "1", "records": []}).to_string()),
            ("no records", json!({"version": 1}).to_string()),
            ("negative", v1(json!([usd(json!(-1.0))]))),
            ("overflow", v1(json!([usd(json!(1.7e308)), usd(json!(1.7e308))]))),
            ("out of range", v1(json!([usd(json!(0))])).replace("\"amount\":0", "\"amount\":1e999")),
            ("a text amount", v1(json!([usd(json!("1.0"))]))),
            (
                "amount without a unit",
                v1(json!([{"file":"f","kind":"image","prompt":"p","provider":"fal","model":"m","amount":1.0,"unit":null,"cost_missing":false,"at_ms":1}])),
            ),
            ("a record without its provider", v1(json!([{"file":"f","kind":"image","prompt":"p","amount":1.0,"unit":"usd","cost_missing":false,"at_ms":1}]))),
            ("a bare list of new records", json!([usd(json!(1.0))]).to_string()),
            ("an old record without a cost", r#"[{"file":"a","kind":"image","prompt":"p","at_ms":1}]"#.to_string()),
            ("an old record with a null cost", r#"[{"file":"a","kind":"image","prompt":"p","cost_usd":null,"at_ms":1}]"#.to_string()),
            ("an old record that isn't one", "[7]".to_string()),
        ];
        for (name, bytes) in cases {
            std::fs::write(&path, &bytes).unwrap();
            assert!(ws.try_assets().is_err(), "{name}: read");
            assert!(ws.try_assets_spent().is_err(), "{name}: total");
            assert!(ws.try_type_usage(AssetKind::Image, &typed("fal", Unit::Usd, 0)).is_err(), "{name}: usage");
            assert!(ws.record_asset(made("image", "fal", Some(Unit::Usd), Some(0.2), 2)).is_err(), "{name}: record");
            assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes(), "{name}: the damaged file is left as it was");
        }

        // A charge that can't be added is refused before anything is written.
        std::fs::write(&path, &text).unwrap();
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(ws.record_asset(made("image", "fal", Some(Unit::Usd), Some(bad), 2)).is_err(), "{bad}");
        }
        let no_unit = made("image", "fal", None, Some(1.0), 2);
        assert!(ws.record_asset(no_unit).is_err(), "an amount needs its unit");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn a_ledger_from_a_newer_build_is_never_read_or_written_over() {
        let (_d, ws) = new_workspace();
        let path = ws.root.join("assets.json");
        let newer = json!({"version": 2, "records": [], "extra": true}).to_string();
        std::fs::write(&path, &newer).unwrap();
        let e = ws.try_assets().unwrap_err();
        assert!(e.message.contains("written by a newer Craft Anything"), "{}", e.message);
        assert_eq!(e.code, craft_anything_evidence::ErrorCode::NotSupported);
        assert!(e.remediation.as_deref().unwrap_or_default().contains("Update Craft Anything"));
        assert!(ws.try_type_usage(AssetKind::Image, &typed("fal", Unit::Usd, 0)).is_err());
        assert!(ws.record_asset(made("image", "fal", Some(Unit::Usd), Some(0.2), 2)).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        // A version beyond what a number can hold is newer too, not damage.
        std::fs::write(&path, r#"{"version":99999999999999,"records":[]}"#).unwrap();
        assert!(ws.try_assets().unwrap_err().message.contains("newer Craft Anything"));
    }

    #[test]
    fn a_version_3_project_keeps_its_budget_and_provider_choices() {
        let s = settings();
        let m = read_manifest(&v3(r#","asset_budget_usd":12.0,"asset_providers":["scenario"]"#)).unwrap();
        assert_eq!(m.version, 4);
        assert_eq!((m.assets.legacy_budget_usd, m.assets.legacy_providers.as_deref()), (Some(12.0), Some(&["scenario".to_string()][..])));
        let seen = serde_json::to_value(&m).unwrap();
        assert!(seen.get("asset_budget_usd").is_none() && seen.get("asset_providers").is_none(), "moved into assets: {seen}");

        let image = m.assets.effective(AssetKind::Image, &s);
        assert_eq!(image.choice, AssetChoice::off(), "fal isn't one of the providers the project allowed");
        let model = m.assets.effective(AssetKind::Model3d, &s);
        assert_eq!((model.choice.provider.as_deref(), model.choice.unit, model.limit), (Some("scenario"), Unit::Credits, 500.0));
        assert!(model.limit_on);
        assert_eq!(model.since_ms, 0, "the old ledger counts from the start");
        assert_eq!(m.assets.effective(AssetKind::Music, &s).choice, AssetChoice::off(), "elevenlabs isn't allowed either");
        assert_eq!(m.assets.effective(AssetKind::Speech, &s).choice, AssetChoice::off(), "no default: Off");
        assert!(!m.assets.ignore_budget);
    }

    #[test]
    fn legacy_all_providers_gives_dollar_types_the_old_budget() {
        let s = settings();
        let m = read_manifest(&v3(r#","asset_budget_usd":12.0"#)).unwrap();
        assert_eq!(m.assets.legacy_providers, None);
        let image = m.assets.effective(AssetKind::Image, &s);
        assert_eq!((image.choice.provider.as_deref(), image.choice.unit, image.limit, image.limit_on), (Some("fal"), Unit::Usd, 12.0, true));
        let model = m.assets.effective(AssetKind::Model3d, &s);
        assert_eq!((model.choice.provider.as_deref(), model.limit), (Some("scenario"), 500.0), "credits don't take the dollar budget");
        // A project that never had a budget of its own gets Settings' limit for the unit.
        let plain = read_manifest(&v3("")).unwrap();
        assert_eq!(plain.assets.effective(AssetKind::Image, &s).limit, 5.0);
        let mut roomy = settings();
        roomy.usd_limit = 9.0;
        roomy.credit_limit = 40.0;
        assert_eq!((plain.assets.effective(AssetKind::Image, &roomy).limit, plain.assets.effective(AssetKind::Music, &roomy).limit), (9.0, 40.0));
    }

    #[test]
    fn a_stored_type_wins_over_settings_and_the_old_fields() {
        let s = settings();
        let mut m = read_manifest(&v3(r#","asset_budget_usd":12.0,"asset_providers":["scenario"]"#)).unwrap();
        m.assets.choose(AssetKind::Image, pick("fal", "fal-ai/flux/schnell", Unit::Usd), &s, 7);
        let image = m.assets.effective(AssetKind::Image, &s);
        assert_eq!(
            (image.choice.model.as_str(), image.limit, image.since_ms),
            ("fal-ai/flux/schnell", 5.0, 7),
            "the person's own choice, not filtered by the old list"
        );
        // The other types still follow the old fields.
        assert_eq!(m.assets.effective(AssetKind::Music, &s).choice, AssetChoice::off());
    }

    #[test]
    fn a_new_project_starts_with_every_type_from_settings() {
        let mut s = settings();
        s.ignore_budget = true;
        s.usd_limit = 8.0;
        let a = ProjectAssets::from_defaults(&s, 1234);
        assert!(a.ignore_budget && a.legacy_budget_usd.is_none() && a.legacy_providers.is_none());
        assert_eq!(a.types.keys().copied().collect::<Vec<_>>(), AssetKind::ALL, "all five are stored");
        let image = &a.types[&AssetKind::Image];
        assert_eq!((image.choice.provider.as_deref(), image.limit, image.limit_on, image.since_ms), (Some("fal"), 8.0, true, 1234));
        let music = &a.types[&AssetKind::Music];
        assert_eq!((music.choice.provider.as_deref(), music.limit, music.since_ms), (Some("elevenlabs"), 500.0, 1234));
        let speech = &a.types[&AssetKind::Speech];
        assert_eq!((speech.choice.provider.as_deref(), speech.limit), (None, 8.0), "Off: stored with the dollar limit");
        // Changing Settings afterwards does not change the project.
        s.defaults.remove(&AssetKind::Image);
        assert_eq!(a.effective(AssetKind::Image, &s).choice.provider.as_deref(), Some("fal"));
        // The file names the type the way the tools do, and leaves out the old fields.
        let text = serde_json::to_string(&a).unwrap();
        assert!(text.contains(r#""model_3d":{"choice""#) && !text.contains("legacy"), "{text}");
        assert_eq!(serde_json::from_str::<ProjectAssets>(&text).unwrap(), a);
    }

    #[test]
    fn switching_provider_resets_the_limit_to_the_new_units_default() {
        let s = settings();
        let mut a = ProjectAssets::from_defaults(&s, 1);
        a.set_limit(AssetKind::Image, Some(40.0), None, &s).unwrap();
        assert_eq!(a.effective(AssetKind::Image, &s).limit, 40.0);
        let scenario = pick("scenario", "model_a", Unit::Credits);
        a.choose(AssetKind::Image, scenario.clone(), &s, 99);
        let t = a.effective(AssetKind::Image, &s);
        assert_eq!((t.choice.unit, t.limit, t.since_ms), (Unit::Credits, 500.0, 99));
        a.set_limit(AssetKind::Image, Some(60.0), Some(false), &s).unwrap();
        // The same provider with another model keeps the limit, the switch and the start of the count.
        a.choose(AssetKind::Image, AssetChoice { model: "model_b".into(), ..scenario }, &s, 150);
        let t = a.effective(AssetKind::Image, &s);
        assert_eq!((t.choice.model.as_str(), t.limit, t.limit_on, t.since_ms), ("model_b", 60.0, false, 99));
        // Back to dollars: the dollar default again. The other types are untouched.
        a.choose(AssetKind::Image, pick("fal", "fal-ai/flux/dev", Unit::Usd), &s, 200);
        let t = a.effective(AssetKind::Image, &s);
        assert_eq!((t.limit, t.since_ms), (5.0, 200));
        assert_eq!(a.effective(AssetKind::Model3d, &s).since_ms, 1);
        // Another provider in the same unit still starts afresh.
        a.choose(AssetKind::Model3d, pick("elevenlabs", "m", Unit::Credits), &s, 300);
        assert_eq!(a.effective(AssetKind::Model3d, &s).since_ms, 300);
        // Off is a change of provider too.
        a.choose(AssetKind::Image, AssetChoice::off(), &s, 400);
        assert_eq!(a.effective(AssetKind::Image, &s).since_ms, 400);
    }

    #[test]
    fn a_new_provider_or_unit_puts_the_limit_back_on() {
        let s = settings();
        let mut a = ProjectAssets::from_defaults(&s, 1);
        let on = |a: &ProjectAssets| a.effective(AssetKind::Image, &s).limit_on;
        let off = |a: &mut ProjectAssets| a.set_limit(AssetKind::Image, None, Some(false), &s).unwrap();
        off(&mut a);
        assert!(!on(&a));
        // Another model at the same provider keeps the person's choice to have no limit.
        a.choose(AssetKind::Image, pick("fal", "fal-ai/flux/schnell", Unit::Usd), &s, 5);
        assert!(!on(&a));
        // Another provider in the same unit starts a fresh count, and a fresh count starts limited.
        a.choose(AssetKind::Image, pick("other", "m", Unit::Usd), &s, 6);
        assert!(on(&a));
        off(&mut a);
        // The same provider in another unit does too.
        a.choose(AssetKind::Image, pick("other", "m", Unit::Credits), &s, 7);
        assert!(on(&a));
        off(&mut a);
        a.choose(AssetKind::Image, pick("elevenlabs", "m", Unit::Credits), &s, 8);
        assert!(on(&a), "another provider billed in the same unit");
        off(&mut a);
        // A switched-off limit on a dollar type does not leave the credit type that follows unlimited.
        a.choose(AssetKind::Image, pick("fal", "fal-ai/flux/dev", Unit::Usd), &s, 9);
        assert!(on(&a));
        // Only the type that changed is touched.
        off(&mut a);
        a.choose(AssetKind::Music, pick("fal", "m", Unit::Usd), &s, 10);
        assert!(!on(&a));
    }

    #[test]
    fn off_and_back_to_the_same_provider_starts_a_new_count_and_limit() {
        let mut s = settings();
        s.usd_limit = 9.0;
        let mut a = ProjectAssets::from_defaults(&s, 1);
        a.set_limit(AssetKind::Image, Some(40.0), Some(false), &s).unwrap();
        a.choose(AssetKind::Image, AssetChoice::off(), &s, 400);
        let t = a.effective(AssetKind::Image, &s);
        assert_eq!((t.choice.provider, t.limit, t.limit_on, t.since_ms), (None, 9.0, true, 400), "Off is a change of provider");
        a.choose(AssetKind::Image, pick("fal", "fal-ai/flux/dev", Unit::Usd), &s, 500);
        let t = a.effective(AssetKind::Image, &s);
        assert_eq!(
            (t.choice.provider.as_deref(), t.limit, t.limit_on, t.since_ms),
            (Some("fal"), 9.0, true, 500),
            "neither the 40 dollars nor the count from 1 came back"
        );
        // Choosing Off while Off changes nothing.
        a.choose(AssetKind::Speech, AssetChoice::off(), &s, 600);
        assert_eq!(a.effective(AssetKind::Speech, &s).since_ms, 1);
    }

    #[test]
    fn choosing_in_a_migrated_project_compares_with_what_it_had() {
        let s = settings();
        let mut m = read_manifest(&v3(r#","asset_budget_usd":12.0"#)).unwrap();
        // Pictures were at fal with the project's $12 budget: another fal model keeps both.
        m.assets.choose(AssetKind::Image, pick("fal", "fal-ai/flux/schnell", Unit::Usd), &s, 50);
        let t = m.assets.effective(AssetKind::Image, &s);
        assert_eq!((t.limit, t.since_ms), (12.0, 0));
        // 3D models were at Scenario: moving to fal starts afresh at the dollar default.
        m.assets.choose(AssetKind::Model3d, pick("fal", "tripo3d/p1/image-to-3d", Unit::Usd), &s, 50);
        let t = m.assets.effective(AssetKind::Model3d, &s);
        assert_eq!((t.limit, t.since_ms), (5.0, 50));
    }

    #[test]
    fn limits_are_checked() {
        let s = settings();
        let mut a = ProjectAssets::from_defaults(&s, 1);
        let before = a.clone();
        for bad in [f64::NAN, f64::INFINITY, -1.0, crate::workspace::MAX_ASSET_BUDGET_USD + 1.0] {
            let e = a.set_limit(AssetKind::Image, Some(bad), Some(false), &s).unwrap_err();
            assert!(e.message.contains("10000"), "{bad}: {}", e.message);
        }
        for bad in [f64::NAN, -0.5, MAX_ASSET_CREDITS + 1.0] {
            let e = a.set_limit(AssetKind::Music, Some(bad), None, &s).unwrap_err();
            assert!(e.message.contains("10000000") && e.message.contains("credits"), "{bad}: {}", e.message);
        }
        assert_eq!(a, before, "a refused limit changes nothing, not even the switch");
        // The edges are fine, and a dollar maximum is too high for credits only the other way round.
        a.set_limit(AssetKind::Image, Some(crate::workspace::MAX_ASSET_BUDGET_USD), None, &s).unwrap();
        a.set_limit(AssetKind::Image, Some(0.0), None, &s).unwrap();
        a.set_limit(AssetKind::Music, Some(MAX_ASSET_CREDITS), None, &s).unwrap();
        a.set_limit(AssetKind::Music, Some(crate::workspace::MAX_ASSET_BUDGET_USD + 1.0), None, &s).unwrap();
        // Only the switch, or nothing at all.
        a.set_limit(AssetKind::Music, None, Some(false), &s).unwrap();
        assert!(!a.effective(AssetKind::Music, &s).limit_on);
        let same = a.clone();
        a.set_limit(AssetKind::Speech, None, None, &s).unwrap();
        assert_eq!(a, same);
    }

    #[test]
    fn a_limit_on_a_type_not_stored_yet_starts_from_what_it_is_now() {
        let s = settings();
        let mut m = read_manifest(&v3(r#","asset_budget_usd":12.0"#)).unwrap();
        assert!(m.assets.types.is_empty());
        m.assets.set_limit(AssetKind::Image, Some(3.0), None, &s).unwrap();
        let t = &m.assets.types[&AssetKind::Image];
        assert_eq!((t.choice.provider.as_deref(), t.limit, t.limit_on), (Some("fal"), 3.0, true));
        assert_eq!(m.assets.types.len(), 1, "only the type that was changed");
    }

    #[test]
    fn a_type_is_on_with_the_master_switch_and_a_provider() {
        let s = settings();
        let mut m = Manifest::new("x", "y", TargetRef::None);
        m.assets = ProjectAssets::from_defaults(&s, 5);
        assert!(m.asset_type_on(AssetKind::Image, &s).is_none(), "generated assets are off until the person turns them on");
        m.generate_assets = true;
        assert_eq!(m.asset_type_on(AssetKind::Image, &s).map(|t| t.choice.model), Some("fal-ai/flux/dev".into()));
        assert!(m.asset_type_on(AssetKind::Speech, &s).is_none(), "Off");
        m.generate_assets = false;
        assert!(m.asset_type_on(AssetKind::Model3d, &s).is_none());
    }

    #[test]
    fn a_projects_choices_are_saved_in_assets_and_read_back() {
        let s = settings();
        let dir = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(dir.path(), "game", "A snake game", TargetRef::None).unwrap();
        ws.set_person(|m| m.assets = ProjectAssets::from_defaults(&s, 5)).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(ws.root.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(saved["version"], 4);
        assert_eq!(saved["assets"]["types"]["image"]["choice"]["provider"], json!("fal"));
        assert!(saved["assets"].get("legacy_budget_usd").is_none() && saved.get("asset_budget_usd").is_none());
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.assets, ws.manifest.assets);
    }

    #[test]
    fn an_old_provider_list_that_is_empty_or_null_means_none_or_all() {
        let s = settings();
        let none = read_manifest(&v3(r#","asset_providers":[]"#)).unwrap();
        assert_eq!(none.assets.legacy_providers, Some(Vec::new()));
        assert!(AssetKind::ALL.into_iter().all(|k| none.assets.effective(k, &s).choice.provider.is_none()), "an empty list allowed no provider");
        let all = read_manifest(&v3(r#","asset_providers":null"#)).unwrap();
        assert_eq!(all.assets, ProjectAssets::default(), "nothing to move");
        assert_eq!(all.assets.effective(AssetKind::Image, &s).choice.provider.as_deref(), Some("fal"));
        // A manifest from before any of this reaches version 4 the same way.
        let v0 = br#"{"name":"Old","request":"x","target":{"kind":"none"},"created_ms":1,"updated_ms":1,"asset_budget_usd":3.5}"#;
        let m = read_manifest(v0).unwrap();
        assert_eq!((m.version, m.assets.legacy_budget_usd, m.legacy_budget_usd()), (4, Some(3.5), 3.5));
        // A project that never had either gets the built-in budget.
        assert_eq!(read_manifest(&v3("")).unwrap().legacy_budget_usd(), 5.0);
        // Something that isn't an object, or whose assets isn't one, is refused rather than guessed at.
        assert!(read_manifest(br#"[1]"#).is_err());
        assert!(read_manifest(&v3(r#","assets":3,"asset_budget_usd":1.0"#)).is_err());
    }

    #[test]
    fn person_fields_survive_a_builder_save() {
        let s = settings();
        let dir = tempfile::tempdir().unwrap();
        let mut run = Workspace::create(dir.path(), "game", "A snake game", TargetRef::None).unwrap();
        // While the run works, the person picks models and limits.
        let mut person = Workspace::open(&run.root).unwrap();
        person
            .set_person(|m| {
                m.assets = ProjectAssets::from_defaults(&s, 5);
                m.assets.ignore_budget = true;
                m.assets.set_limit(AssetKind::Image, Some(20.0), None, &s).unwrap();
            })
            .unwrap();
        // The run saves its own progress from the copy it started with.
        run.manifest.note = "Building".into();
        run.save().unwrap();
        let disk = Workspace::open(&run.root).unwrap().manifest;
        assert!(disk.assets.ignore_budget);
        assert_eq!(disk.assets.effective(AssetKind::Image, &s).limit, 20.0);
        assert_eq!(disk.note, "Building", "the run's own fields are saved");
        // A setter on the old copy starts from what's on disk too.
        run.set_person(|m| m.assets.choose(AssetKind::Music, AssetChoice::off(), &s, 9)).unwrap();
        let disk = Workspace::open(&run.root).unwrap().manifest;
        assert!(disk.assets.ignore_budget && disk.assets.effective(AssetKind::Image, &s).limit == 20.0);
        assert_eq!(disk.assets.effective(AssetKind::Music, &s).choice, AssetChoice::off());
    }

    #[test]
    fn a_copy_of_a_project_has_the_same_choices_and_limits() {
        let s = settings();
        let dir = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(dir.path(), "Original", "x", TargetRef::None).unwrap();
        ws.set_person(|m| {
            m.generate_assets = true;
            m.assets = ProjectAssets::from_defaults(&s, 5);
            m.assets.ignore_budget = true;
            m.assets.legacy_providers = Some(vec!["scenario".into()]);
            m.assets.set_limit(AssetKind::Model3d, Some(75.0), Some(false), &s).unwrap();
        })
        .unwrap();
        let copy = ws.duplicate("Copy").unwrap();
        let saved = Workspace::open(&copy.root).unwrap().manifest;
        assert_eq!(saved.assets, ws.manifest.assets);
        assert!(saved.assets.ignore_budget && !saved.assets.types[&AssetKind::Model3d].limit_on);
    }
}
