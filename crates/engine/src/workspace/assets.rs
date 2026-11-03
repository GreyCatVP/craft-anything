//! What a project chose for its generated assets: for each asset type a provider and model, a
//! limit in the provider's unit with its own switch, and the project-wide "Ignore budget" switch.
//! The person owns all of it (see [`super::Workspace::set_person`]); Settings only supplies the
//! starting values for a new project.

use crate::asset_choice::AssetChoice;
use crate::settings::AssetSettings;
use craft_anything_engines::assets::AssetKind;
use craft_anything_engines::assets::model::{MAX_ASSET_CREDITS, Unit};
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
    /// stay when the provider and its unit are the same as before; another provider starts the
    /// count afresh at `now_ms` with Settings' default limit in the new unit.
    pub fn choose(&mut self, kind: AssetKind, choice: AssetChoice, s: &AssetSettings, now_ms: u64) {
        let before = self.effective(kind, s);
        let same = before.choice.provider == choice.provider && before.choice.unit == choice.unit;
        let (limit, since_ms) = if same { (before.limit, before.since_ms) } else { (s.default_limit(choice.unit), now_ms) };
        self.types.insert(kind, TypeAssets { choice, limit, limit_on: before.limit_on, since_ms });
    }

    /// Change the limit of `kind` (in its provider's unit) and/or its switch. A limit outside what
    /// the unit allows is refused, and then nothing changes.
    pub fn set_limit(&mut self, kind: AssetKind, limit: Option<f64>, on: Option<bool>, s: &AssetSettings) -> Result<(), ToolError> {
        if limit.is_none() && on.is_none() {
            return Ok(());
        }
        let mut t = self.effective(kind, s);
        if let Some(l) = limit {
            let (max, unit) = match t.choice.unit {
                Unit::Usd => (super::MAX_ASSET_BUDGET_USD, "dollars"),
                Unit::Credits => (MAX_ASSET_CREDITS, "credits"),
            };
            if !l.is_finite() || !(0.0..=max).contains(&l) {
                return Err(ToolError::invalid(format!("A limit must be a number from 0 to {max:.0} {unit}."))
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
