//! UI smoke tests: every screen draws, the main controls work, and nothing blocks or panics.

use craft_anything_engine::{Mode, TargetKind};
use craft_anything_ui_egui::state::{ChatItem, FirstRunStep, RailKind, Screen};
use craft_anything_ui_egui::{CraftApp, demo};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::time::{Duration, Instant};

fn harness(make: impl FnOnce() -> CraftApp + 'static) -> Harness<'static, CraftApp> {
    harness_sized(egui::vec2(1280.0, 820.0), make)
}

fn harness_sized(size: egui::Vec2, make: impl FnOnce() -> CraftApp + 'static) -> Harness<'static, CraftApp> {
    Harness::builder().with_size(size).with_max_steps(32).build_eframe(move |cc| {
        CraftApp::setup_context(&cc.egui_ctx);
        make()
    })
}

/// Step until `done` holds (workers run on threads), or give up after a few seconds.
fn wait(h: &mut Harness<'static, CraftApp>, done: impl Fn(&CraftApp) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        h.step();
        if done(h.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn first_run_walks_through_four_screens() {
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, false)));
    h.run_steps(3);
    // The window controls are there from the first screen (the native frame is off).
    h.get_by_label("Close");
    h.get_by_label("I'm new to this").click();
    h.run_steps(3);
    assert_eq!(h.state().state.first_run, FirstRunStep::Brain);
    assert_eq!(h.state().settings.mode, Mode::Beginner);
    h.get_by_label("No AI for now").click();
    h.run_steps(3);
    assert_eq!(h.state().state.first_run, FirstRunStep::Purpose);
    assert!(h.state().state.backend.is_none());
    // Why we ask is read on the page, not only on hover (#71).
    h.get_by_label_contains("Saying why helps you keep to it");
    h.get_by_label("I'm learning how things work").click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(h.state().state.first_run, FirstRunStep::Done);
    h.get_by_label("Let's go").click();
    h.run_steps(4);
    assert!(h.state().settings.first_run_done);
    assert_eq!(h.state().state.screen, Screen::Home);
    h.get_by_label("Just look inside");
}

/// #231: at the smallest window the app allows, Purpose's Continue was below the window with
/// nothing to scroll; it scrolls into view above the footer and works.
#[test]
fn first_run_continue_is_reachable_at_the_smallest_window() {
    let (w, hgt) = (760.0, 520.0);
    let mut h = harness_sized(egui::vec2(w, hgt), || demo::app(demo::engine(Mode::Beginner, false)));
    h.state_mut().state.first_run = FirstRunStep::Purpose;
    h.run_steps(3);
    h.get_by_label("Continue").scroll_to_me();
    h.run_steps(3);
    let r = h.get_by_label("Continue").rect();
    assert!(r.top() >= 0.0 && r.bottom() <= hgt - 48.0, "Continue at {r:?} isn't in view above the footer");
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(h.state().state.first_run, FirstRunStep::Done);
}

#[test]
fn simple_title_bar_keeps_only_the_essentials() {
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(3);
    assert!(h.query_by_label("Advanced").is_none(), "the mode switch lives in Settings in Simple mode");
    h.get_by_label("Settings");
    h.get_by_label("Clean room");
}

#[test]
fn a_game_is_built_with_its_engine_and_a_missing_engine_is_got_first() {
    use craft_anything_engine::workspace::TargetRef;
    use craft_anything_engines::game::{Builder, Found, Installed, Inventory, Made};
    use craft_anything_ui_egui::state::Modal;
    let d = tempfile::tempdir().unwrap();
    let game = d.path().join("game");
    std::fs::create_dir_all(&game).unwrap();
    // Tall enough to show Start below the Clean room / Deep, engine and asset choices.
    let mut h = harness_sized(egui::vec2(1280.0, 1120.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_projects(&mut a);
        a.state.make.target = TargetRef::Local { path: game };
        a.state.make.game = true;
        a.state.make.request = "Recreate this game".into();
        a.state.make.found = Some(Found { engine: Made::Unity, version: Some("2021.3.16f1".into()), clue: "unityplayer.dll".into(), note: None });
        a.game_engines = Some(Inventory {
            engines: vec![Installed { builder: Builder::Godot, version: "4.4.1".into(), program: "godot.exe".into() }],
            ..Default::default()
        });
        a
    });
    h.run_steps(3);
    h.get_by_label("Made with Unity 2021.3.16f1");
    h.get_by_label("Get Unity");
    // Unity isn't here, so Start opens the steps to get it (and starts once it's ready).
    h.get_by_label("Start").click();
    h.run_steps(4);
    assert_eq!(h.state().state.modal, Some(Modal::Engine(Builder::Unity)));
    assert!(h.state().engine_flow.start_after);
    // The first step: Unity's own installer where we can check its signature, else its website.
    h.get_by_label(if cfg!(windows) { "Download Unity Hub" } else { "Get Unity Hub" });
    h.get_by_label("Build with Godot instead");
    assert!(h.state().projects.iter().all(|p| p.manifest.request != "Recreate this game"), "nothing started yet");
}

/// Whether the fal.ai key is set up, as the app keeps it.
fn fal_key(stored: bool) -> std::collections::BTreeMap<String, bool> {
    [(craft_anything_engines::assets::fal::ID.to_string(), stored)].into()
}

/// Asset opt-in offers every provider, accepts Scenario credentials and keeps key setup reachable in Settings.
#[test]
fn turning_on_generated_assets_offers_all_providers_and_accepts_a_scenario_key() {
    use craft_anything_engine::workspace::TargetRef;
    use craft_anything_engines::game::{Builder, Installed, Inventory};
    use craft_anything_ui_egui::state::Modal;
    let d = tempfile::tempdir().unwrap();
    let game = d.path().join("game");
    std::fs::create_dir_all(&game).unwrap();
    let mut h = harness_sized(egui::vec2(1280.0, 1000.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_projects(&mut a);
        a.state.make.target = TargetRef::Local { path: game };
        a.state.make.game = true;
        a.state.make.request = "Recreate this game".into();
        a.state.make.builder = Some(Builder::Unity);
        a.game_engines = Some(Inventory {
            engines: vec![Installed { builder: Builder::Unity, version: "6000.0.23f1".into(), program: "Unity.exe".into() }],
            ..Default::default()
        });
        a.asset_keys = Some(fal_key(false));
        a
    });
    h.run_steps(3);
    assert!(!h.state().state.make.generate_assets, "off until the person turns it on");
    h.get_by_label("Generate pictures, models and audio").click();
    h.run_steps(3);
    assert!(h.state().state.make.generate_assets);
    assert_eq!(h.state().state.modal, Some(Modal::Assets), "no key yet, so it's asked for");
    h.get_by_label("Your asset provider keys");
    h.get_by_label("fal.ai key");
    h.get_by_label("Scenario key");
    h.get_by_label("Scenario API secret");
    assert!(h.query_by_label("fal.ai API secret").is_none());
    assert!(h.query_by_label("ElevenLabs API secret").is_none());
    h.get_by_label("ElevenLabs key");
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, None);
    assert!(!h.state().state.make.generate_assets, "without a key it goes back off");
    // The switch says what it costs and what happens without it, in either mode.
    for deep in [true, false] {
        h.state_mut().settings.deep_mode = deep;
        h.run_steps(2);
        h.get_by_label_contains("Off, the game is built without generated assets.");
        assert!(h.query_by_label_contains("original game").is_none());
    }
    // With a key, turning it on asks nothing.
    h.state_mut().asset_keys = Some([("scenario".to_string(), true)].into());
    h.get_by_label("Generate pictures, models and audio").click();
    h.run_steps(3);
    assert!(h.state().state.make.generate_assets && h.state().state.modal.is_none());
    // Provider credentials remain reachable in Settings → Builds, and are not on the AI page.
    h.state_mut().asset_keys = Some(Default::default());
    h.state_mut().state.settings_open = true;
    h.state_mut().state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
    h.run_steps(3);
    assert!(h.query_by_label("Add a key for Scenario").is_none(), "the AI page no longer has the asset provider keys");
    h.state_mut().state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Builds;
    h.run_steps(3);
    h.get_by_label("Add a key for Scenario").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Add a key for Scenario").click();
    h.run_steps(3);
    h.get_by_label("Scenario key");
    h.get_by_label("Scenario API secret");
}

/// Scenario requires a nonblank key and secret in password fields and clears both inputs after saving.
#[test]
fn scenario_settings_require_both_masked_fields_and_clear_them_after_save() {
    use egui_kittest::kittest::NodeT;
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.asset_keys = Some(Default::default());
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Builds;
        a
    });
    h.run_steps(3);
    h.get_by_label("Add a key for Scenario").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Add a key for Scenario").click();
    h.run_steps(3);
    for label in ["Scenario key", "Scenario API secret"] {
        assert_eq!(h.get_by_label(label).accesskit_node().role(), egui::accesskit::Role::PasswordInput);
    }
    assert!(h.get_by_label("Save").accesskit_node().is_disabled());
    h.get_by_label("Scenario API secret").scroll_to_me();
    h.get_by_label("Scenario API secret").focus();
    h.run_steps(3);
    h.get_by_label("Scenario API secret").type_text("  ");
    h.run_steps(2);
    h.get_by_label("Scenario key").scroll_to_me();
    h.get_by_label("Scenario key").focus();
    h.run_steps(3);
    h.get_by_label("Scenario key").type_text("  sk-test-key  ");
    h.run_steps(2);
    assert!(h.get_by_label("Save").accesskit_node().is_disabled(), "whitespace is not a secret");
    assert_eq!(h.state().key_inputs.get("scenario").unwrap(), "  sk-test-key  ");
    h.get_by_label("Scenario API secret").focus();
    h.run_steps(2);
    h.get_by_label("Scenario API secret").type_text("sk-test-secret  ");
    h.run_steps(2);
    assert!(!h.get_by_label("Save").accesskit_node().is_disabled());
    h.state_mut().key_inputs.insert("scenario".into(), "  ".into());
    h.run_steps(2);
    assert!(h.get_by_label("Save").accesskit_node().is_disabled(), "a secret alone is not enough");
    // A credential generation would refuse (not ASCII) keeps both fields, saves nothing and says why.
    h.state_mut().key_inputs.insert("scenario".into(), "sk-tëst-key".into());
    h.run_steps(2);
    h.get_by_label("Save").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(h.state().key_inputs.get("scenario").map(String::as_str), Some("sk-tëst-key"));
    assert!(h.state().key_inputs.contains_key("scenario:secret"));
    assert!(h.state().asset_keys.as_ref().is_some_and(|keys| keys.get("scenario").is_none()), "nothing saved");
    assert!(h.state().state.toasts.iter().any(|(text, error)| *error && text.contains("valid asset provider credential")));
    h.state_mut().key_inputs.insert("scenario".into(), String::new());
    h.get_by_label("Scenario key").focus();
    h.run_steps(2);
    h.get_by_label("Scenario key").type_text("sk-test-key  ");
    h.run_steps(2);
    h.get_by_label("Save").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Save").click();
    assert!(wait(&mut h, |a| a.asset_keys.as_ref().and_then(|keys| keys.get("scenario")) == Some(&true)));
    assert!(!h.state().key_inputs.contains_key("scenario"));
    assert!(!h.state().key_inputs.contains_key("scenario:secret"));
    h.get_by_label("Key saved");
}

#[test]
fn deep_mode_without_ghidra_asks_to_get_it_before_using_it() {
    use craft_anything_engine::workspace::TargetRef;
    use craft_anything_engines::game::{Builder, Installed, Inventory};
    use craft_anything_ui_egui::state::Modal;
    let d = tempfile::tempdir().unwrap();
    let (store, game) = (d.path().join("engines"), d.path().join("game"));
    std::fs::create_dir_all(&game).unwrap();
    // Tall enough to show Start below the Clean room / Deep, engine and asset choices.
    let mut h = harness_sized(egui::vec2(1280.0, 1120.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_projects(&mut a);
        // An empty engine store: no Ghidra here.
        a.settings.engines.store_dir = Some(store);
        a.state.make.target = TargetRef::Local { path: game };
        a.state.make.game = true;
        a.state.make.request = "Recreate this game".into();
        a.state.make.builder = Some(Builder::Unity);
        a.game_engines = Some(Inventory {
            engines: vec![Installed { builder: Builder::Unity, version: "6000.0.23f1".into(), program: "Unity.exe".into() }],
            ..Default::default()
        });
        a.asset_keys = Some(fal_key(true));
        a
    });
    h.run_steps(3);
    // An app or a game offers Clean room or Deep right on the Make screen; picking Deep offers
    // Ghidra instead of quietly going without it.
    h.get_by_label("How closely may Craft Anything look at it?");
    h.get_by_label("Deep").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, Some(Modal::Ghidra));
    h.get_by_label("Get Ghidra for Deep mode");
    if craft_anything_engines::ghidra::get::can_install() {
        h.get_by_label("Download and use Ghidra");
    }
    h.get_by_label("I have Ghidra");
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, None);
    assert!(h.state().settings.deep_mode && h.state_mut().ghidra_missing(), "Deep stays on, without Ghidra");
    // Starting a build of a program asks again, and the project waits for the answer.
    h.get_by_label("Start").click();
    h.run_steps(4);
    assert_eq!(h.state().state.modal, Some(Modal::Ghidra));
    assert!(h.state().ghidra.start_after.is_some(), "the build starts once Ghidra is ready");
    h.get_by_label("Build without it");
}

#[test]
fn a_long_prompt_wraps_inside_the_window_in_the_project_and_the_chat() {
    use craft_anything_ui_egui::state::{ChatItem, FeedItem, UserItem};
    // Long sentences and one very long unbroken address.
    let long = format!("{}https://example.com/{}", "Make it exactly like the original, with the same menus, weapons and maps. ".repeat(8), "a".repeat(300));
    for in_chat in [false, true] {
        let text = long.clone();
        let mut h = harness(move || {
            let mut a = demo::app(demo::engine(Mode::Beginner, true));
            if in_chat {
                demo::fill_ask(&mut a);
                a.state.chat.push(ChatItem::User(text));
                a.state.chat.push(ChatItem::User("Make it blue".into()));
            } else {
                demo::fill_project(&mut a);
                a.state.feed.push(FeedItem::User(UserItem::Text(text)));
                a.state.feed.push(FeedItem::User(UserItem::Text("Make it blue".into())));
            }
            a
        });
        h.run_steps(4);
        let width = h.ctx.content_rect().width();
        let r = h.get_by_label(&long).rect();
        assert!(r.right() <= width && r.width() <= 560.0, "the bubble ({r:?}) stays inside the {width}px window (chat: {in_chat})");
        assert!(r.height() > 60.0, "it wraps onto several lines (chat: {in_chat})");
        let short = h.get_by_label("Make it blue").rect();
        assert!(short.width() < 160.0 && short.right() <= width, "a short message keeps a small bubble ({short:?}, chat: {in_chat})");
    }
}

// ---- Settings → Builds: generated-assets defaults -------------------------------------------

/// What the stand-in providers were asked, and the gate that holds their answers back.
struct Probe {
    asked: Vec<(&'static str, std::sync::Arc<std::sync::atomic::AtomicUsize>)>,
    /// While closed, a provider that is asked waits (so a test can look at the page meanwhile).
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The keys the engine's tools read (a file of their own): a test saves one as the person would.
    keys: craft_anything_ai::KeyStore,
}

impl Probe {
    /// How many times `provider` was asked for a list.
    fn asked(&self, provider: &str) -> usize {
        self.asked.iter().filter(|(id, _)| *id == provider).map(|(_, n)| n.load(std::sync::atomic::Ordering::SeqCst)).sum()
    }

    fn total(&self) -> usize {
        self.asked.iter().map(|(_, n)| n.load(std::sync::atomic::Ordering::SeqCst)).sum()
    }

    fn hold(&self, hold: bool) {
        self.open.store(!hold, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A stand-in asset provider with canned model lists. Nothing in it reaches the network.
struct Canned {
    id: &'static str,
    name: &'static str,
    unit: craft_anything_engines::assets::model::Unit,
    models: Vec<craft_anything_engines::assets::model::ListedModel>,
    asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl craft_anything_engines::assets::AssetProvider for Canned {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn unit(&self) -> craft_anything_engines::assets::model::Unit {
        self.unit
    }
    fn list_models(
        &self,
        key: Option<&str>,
        kind: craft_anything_engines::assets::AssetKind,
    ) -> Result<craft_anything_engines::assets::model::ModelList, craft_anything_engine::evidence::ToolError> {
        assert_eq!(key, Some("sk-test"), "a provider is only asked with the key the person saved");
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let start = Instant::now();
        while !self.open.load(std::sync::atomic::Ordering::SeqCst) && start.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let models = self.models.iter().filter(|m| m.kinds.contains(&kind)).cloned().collect();
        Ok(craft_anything_engines::assets::model::ModelList { models, error: None })
    }
    fn describe(&self, _key: &str, _model: &str) -> Result<craft_anything_engines::assets::model::ModelDetail, craft_anything_engine::evidence::ToolError> {
        Err(craft_anything_engine::evidence::ToolError::invalid("not used in this test"))
    }
    fn generate(
        &self,
        _key: &str,
        _model: &craft_anything_engines::assets::model::ModelDetail,
        _req: &craft_anything_engines::assets::AssetRequest,
    ) -> Result<craft_anything_engines::assets::Generated, craft_anything_engine::evidence::ToolError> {
        Err(craft_anything_engine::evidence::ToolError::invalid("not used in this test"))
    }
}

fn listed(
    provider: &str,
    id: &str,
    name: &str,
    kinds: &[craft_anything_engines::assets::AssetKind],
    pricing: craft_anything_engines::assets::model::Pricing,
    ignores: &[craft_anything_engines::assets::model::AssetOption],
) -> craft_anything_engines::assets::model::ListedModel {
    craft_anything_engines::assets::model::ListedModel {
        provider: provider.into(),
        id: id.into(),
        name: name.into(),
        kinds: kinds.to_vec(),
        pricing,
        ignores: ignores.to_vec(),
    }
}

/// An engine whose asset providers are stand-ins with canned lists, whose keys (a saved key for each
/// of `keyed`) and model cache live in a folder of their own, and which has the AI layer's tools.
/// Nothing in it reaches the network, the keychain or the person's config.
fn asset_engine(keyed: &[&str]) -> (std::sync::Arc<craft_anything_engine::Engine>, Probe) {
    use craft_anything_engines::assets::AssetKind::{Image, Model3d, Music, SoundEffect, Speech};
    use craft_anything_engines::assets::model::AssetOption::{Faces, Shape};
    use craft_anything_engines::assets::model::{Pricing, Rate, Unit};
    use craft_anything_engines::assets::{AssetProviders, model::ListedModel};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    let usd = |rate| Pricing::Before { unit: Unit::Usd, rate };
    let quote = |credits| Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: Some(credits) } };
    let catalogue: [(&'static str, &'static str, Unit, Vec<ListedModel>); 3] = [
        (
            "fal",
            "fal.ai",
            Unit::Usd,
            vec![
                listed("fal", "fal-ai/nano-banana-pro", "Nano Banana Pro", &[Image], usd(Rate::PerRequest(0.15)), &[]),
                listed("fal", "fal-ai/flux/dev", "FLUX.1 [dev]", &[Image], usd(Rate::PerMegapixel(0.025)), &[]),
                listed("fal", "fal-ai/square-only", "Square Only", &[Image], usd(Rate::PerRequest(0.04)), &[Shape]),
                listed("fal", "fal-ai/some-new-model", "Some new model", &[Image], Pricing::Unknown, &[Shape, Faces]),
                listed("fal", "tripo3d/p1/image-to-3d", "Tripo P1", &[Model3d], usd(Rate::PerRequest(0.4)), &[]),
                listed("fal", "fal-ai/stable-audio", "Stable Audio", &[SoundEffect], Pricing::Unknown, &[]),
                listed("fal", "fal-ai/lyria-3-pro", "Lyria 3 Pro", &[Music], usd(Rate::PerRequest(2.0)), &[]),
            ],
        ),
        (
            "scenario",
            "Scenario",
            Unit::Credits,
            vec![
                listed("scenario", "model_gemini-3-1", "Gemini 3.1", &[Image], quote(20.0), &[]),
                listed("scenario", "model_flux-kontext", "Flux Kontext", &[Image], quote(15.0), &[]),
                listed("scenario", "model_meshy-7-1-img23d", "Meshy 7.1", &[Model3d], quote(100.0), &[]),
            ],
        ),
        (
            "elevenlabs",
            "ElevenLabs",
            Unit::Credits,
            vec![listed("elevenlabs", "eleven_v3", "Eleven v3", &[Speech], Pricing::After { unit: Unit::Credits }, &[])],
        ),
    ];
    let open = Arc::new(AtomicBool::new(true));
    let mut session = craft_anything_engine::testing::session();
    session.settings.mode = Mode::Beginner;
    session.settings.first_run_done = true;
    session.settings.language = "en".into();
    let keys = craft_anything_ai::KeyStore::file_only(session.config_dir().join("ai-keys.toml"));
    for id in keyed {
        keys.set(&craft_anything_ai::keys::asset_key_name(id), "sk-test").unwrap();
    }
    let mut providers = AssetProviders::built_in();
    let mut asked = Vec::new();
    for (id, name, unit, models) in catalogue {
        let counter = Arc::new(AtomicUsize::new(0));
        asked.push((id, counter.clone()));
        providers.register(Arc::new(Canned { id, name, unit, models, asked: counter, open: open.clone() }));
    }
    session.ext.insert(Arc::new(providers));
    session.ext.insert(Arc::new(keys.clone()));
    (craft_anything_engine::Engine::shared(session, craft_anything_ai::tools()), Probe { asked, open, keys })
}

/// The app over `engine`, with Settings open on Builds and `keyed` showing as the providers with a key.
fn asset_app(engine: std::sync::Arc<craft_anything_engine::Engine>, keyed: &[&str]) -> CraftApp {
    let mut a = demo::app(engine);
    a.asset_keys = Some(["fal", "scenario", "elevenlabs"].into_iter().map(|id| (id.to_string(), keyed.contains(&id))).collect());
    a.state.settings_open = true;
    a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Builds;
    a
}

/// A choice of `model` at `provider`, as the picker makes it.
fn picked(
    provider: &str,
    model: &str,
    name: &str,
    unit: craft_anything_engines::assets::model::Unit,
    pricing: craft_anything_engines::assets::model::Pricing,
) -> craft_anything_engine::asset_choice::AssetChoice {
    craft_anything_engine::asset_choice::AssetChoice { provider: Some(provider.into()), model: model.into(), model_name: name.into(), unit, pricing }
}

/// Step until every asset type's lists have been read and no request is running.
fn lists_read(h: &mut Harness<'static, CraftApp>) -> bool {
    wait(h, |a| a.asset_models.len() == 5 && a.asset_models.values().all(|v| v.is_read() && !v.loading))
}

/// The pictures and music defaults that most tests start from: Scenario (credits) and fal.ai (dollars).
fn with_two_defaults(a: &mut CraftApp) {
    use craft_anything_engines::assets::AssetKind;
    use craft_anything_engines::assets::model::{Pricing, Rate, Unit};
    let quoted = Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: Some(20.0) } };
    a.settings.assets.defaults.insert(AssetKind::Image, picked("scenario", "model_gemini-3-1", "Gemini 3.1", Unit::Credits, quoted));
    let track = Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(2.0) };
    a.settings.assets.defaults.insert(AssetKind::Music, picked("fal", "fal-ai/lyria-3-pro", "Lyria 3 Pro", Unit::Usd, track));
}

/// Settings → Builds has the provider keys and a row for each asset type with its provider and model
/// (Off where none is chosen), its default limit in the provider's unit and whether new projects
/// enforce it. The AI page no longer has the keys.
#[test]
fn settings_shows_a_default_row_per_asset_type() {
    use egui_kittest::kittest::NodeT;
    let (engine, _probe) = asset_engine(&["fal", "scenario"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || {
        let mut a = asset_app(engine, &["fal", "scenario"]);
        with_two_defaults(&mut a);
        a
    });
    h.run_steps(3);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    // The keys: a saved key reads as such, a missing one offers to add it.
    h.get_by_label("Asset provider keys");
    assert_eq!(h.query_all_by_label("Key saved").count(), 2);
    h.get_by_label("Key needed");
    h.get_by_label("Add a key for ElevenLabs");
    h.get_by_label("Remove the fal.ai key");
    // A row for each type, with what a new project uses for it.
    h.get_by_label("Generated assets for new projects");
    h.get_by_label("New projects ignore the budget");
    for title in ["Pictures", "3D models", "Speech", "Sound effects", "Music"] {
        h.get_by_label(title);
    }
    let words = |h: &Harness<'static, CraftApp>, title: &str| h.get_by_label(&format!("{title} model")).value();
    assert_eq!(words(&h, "Pictures").as_deref(), Some("Scenario · Gemini 3.1"));
    assert_eq!(words(&h, "Music").as_deref(), Some("fal.ai · Lyria 3 Pro"));
    for off in ["3D models", "Speech", "Sound effects"] {
        assert_eq!(words(&h, off).as_deref(), Some("Off"), "{off} has no default");
    }
    // Limits are in the provider's unit, and a type that is Off has none.
    assert_eq!(h.get_by_label("Pictures default limit").accesskit_node().numeric_value(), Some(500.0));
    assert_eq!(h.get_by_label("Music default limit").accesskit_node().numeric_value(), Some(5.0));
    assert!(h.query_by_label("Speech default limit").is_none());
    assert_eq!(h.query_all_by_label("On").count(), 2, "new projects start both with their limit on");
    h.get_by_label_contains("Model lists fetched from fal.ai and Scenario just now");
    // The AI page is only about AIs now.
    h.state_mut().state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
    h.run_steps(3);
    assert!(h.query_by_label("Asset provider keys").is_none() && h.query_by_label("Key saved").is_none());
}

/// Saving a provider's key lists its models at once, with Settings still open: the pickers offer
/// that provider without closing and opening the page again.
#[test]
fn saving_a_provider_key_lists_its_models_without_reopening_settings() {
    use craft_anything_engines::assets::AssetKind;
    let (engine, probe) = asset_engine(&["fal"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || asset_app(engine, &["fal"]));
    h.run_steps(3);
    assert!(lists_read(&mut h));
    let providers = |a: &CraftApp| a.asset_models[&AssetKind::Image].providers.iter().map(|p| p.provider.clone()).collect::<Vec<_>>();
    assert_eq!(providers(h.state()), ["fal"]);
    assert_eq!(probe.asked("scenario"), 0, "no key, so no one asked");
    // The person saves a Scenario key; the app is told which providers have one now.
    probe.keys.set(&craft_anything_ai::keys::asset_key_name("scenario"), "sk-test").unwrap();
    let keys: std::collections::BTreeMap<String, bool> =
        [("fal", true), ("scenario", true), ("elevenlabs", false)].map(|(id, saved)| (id.to_string(), saved)).into();
    h.state_mut().jobs.spawn("Save the key", false, move |_| Ok(craft_anything_ui_egui::Done::AssetKeys(keys)));
    assert!(wait(&mut h, |a| providers(a) == ["fal", "scenario"] && a.asset_models.values().all(|v| !v.loading)));
    assert_eq!(probe.asked("scenario"), 5, "once for each type");
    h.run_steps(3);
    // The same keys again change nothing and ask no one.
    let again: std::collections::BTreeMap<String, bool> =
        [("fal", true), ("scenario", true), ("elevenlabs", false)].map(|(id, saved)| (id.to_string(), saved)).into();
    h.state_mut().jobs.spawn("Check the asset keys", false, move |_| Ok(craft_anything_ui_egui::Done::AssetKeys(again)));
    h.run_steps(5);
    assert_eq!(probe.asked("scenario"), 5);
}

/// A default whose provider has no key is flagged, a type no saved key can make says so, and a model
/// whose price is unknown has no limit to set.
#[test]
fn settings_flags_a_default_without_a_key_and_a_model_without_a_price() {
    use craft_anything_engines::assets::AssetKind;
    use craft_anything_engines::assets::model::{Pricing, Unit};
    let (engine, _probe) = asset_engine(&["fal"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || {
        let mut a = asset_app(engine, &["fal"]);
        a.settings
            .assets
            .defaults
            .insert(AssetKind::Music, picked("elevenlabs", "music_v1", "Music v1", Unit::Credits, Pricing::After { unit: Unit::Credits }));
        a.settings.assets.defaults.insert(AssetKind::SoundEffect, picked("fal", "fal-ai/stable-audio", "Stable Audio", Unit::Usd, Pricing::Unknown));
        a
    });
    h.run_steps(3);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    h.get_by_label("No ElevenLabs key");
    h.get_by_label("price unknown");
    h.get_by_label("usage not tracked");
    assert!(h.query_by_label("Sound effects default limit").is_none(), "nothing to limit when usage can't be counted");
    // Speech is Off, and no provider with a saved key makes it (fal.ai doesn't).
    h.get_by_label("No provider with a saved key makes this.");
}

/// The picker lists only the providers with a saved key, with each model's price, what it ignores and
/// "Off"; the search field narrows it; a click chooses the model for new projects.
#[test]
fn the_picker_offers_only_providers_with_keys_and_filters_by_search() {
    use craft_anything_engines::assets::AssetKind;
    use craft_anything_engines::assets::model::{Pricing, Rate, Unit};
    let (engine, _probe) = asset_engine(&["fal", "scenario"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || asset_app(engine, &["fal", "scenario"]));
    h.run_steps(3);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    h.get_by_label("Pictures model").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    // Grouped by provider, each model with its price as the provider gave it.
    h.get_by_label_contains("Nano Banana Pro · $0.15 a picture");
    h.get_by_label_contains("FLUX.1 [dev] · $0.025 a megapixel");
    h.get_by_label_contains("Gemini 3.1 · ~20 credits a picture");
    h.get_by_label_contains("Some new model · price unknown");
    // What a model does nothing with is said beside it.
    h.get_by_label_contains("Square Only · $0.04 a picture   ignores: shape");
    h.get_by_label_contains("Some new model · price unknown   ignores: shape, faces");
    h.get_by_label("Off");
    // The three providers are offered a key in the keys block; only two have lists in the picker.
    assert_eq!(h.query_all_by_label("fal.ai").count(), 2, "its key row and its group");
    assert_eq!(h.query_all_by_label("Scenario").count(), 2, "its key row and its group");
    assert_eq!(h.query_all_by_label("ElevenLabs").count(), 1, "no key, so no group");
    assert!(h.query_by_label_contains("Eleven v3").is_none() && h.query_by_label_contains("Tripo P1").is_none(), "models of other types are not offered");
    // Typing narrows the list.
    h.get_by_label("Search models").focus();
    h.run_steps(2);
    h.get_by_label("Search models").type_text("flux");
    h.run_steps(3);
    h.get_by_label_contains("FLUX.1 [dev]");
    h.get_by_label_contains("Flux Kontext");
    assert!(h.query_by_label_contains("Nano Banana Pro").is_none() && h.query_by_label_contains("Gemini 3.1").is_none());
    h.get_by_label("Search models").type_text("zzz");
    h.run_steps(3);
    h.get_by_label("No model matches that.");
    // A click chooses the model: provider, model, its name, the provider's unit and its price.
    h.get_by_label("Search models").focus();
    h.run_steps(2);
    for _ in 0..3 {
        h.key_press(egui::Key::Backspace);
        h.run_steps(1);
    }
    h.run_steps(2);
    h.get_by_label_contains("FLUX.1 [dev]").click();
    h.run_steps(3);
    let chosen = h.state().settings.assets.defaults.get(&AssetKind::Image).cloned();
    let expected = picked("fal", "fal-ai/flux/dev", "FLUX.1 [dev]", Unit::Usd, Pricing::Before { unit: Unit::Usd, rate: Rate::PerMegapixel(0.025) });
    assert_eq!(chosen, Some(expected));
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("fal.ai · FLUX.1 [dev]"));
    assert!(h.query_by_label("Search models").is_none(), "the list closes once a model is picked");
    // Off is a choice too, and it is saved as one.
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label("Off").click();
    h.run_steps(3);
    assert_eq!(h.state().settings.assets.choice(AssetKind::Image), craft_anything_engine::asset_choice::AssetChoice::off());
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("Off"));
}

/// Opening Settings → Builds asks the providers for their lists on a worker: the page keeps drawing and
/// says so while the request runs; lists read lately aren't asked for again; Refresh asks again.
#[test]
fn opening_settings_refreshes_stale_model_lists_in_the_background() {
    use egui_kittest::kittest::NodeT;
    let (engine, probe) = asset_engine(&["fal", "scenario"]);
    probe.hold(true);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || {
        let mut a = asset_app(engine, &["fal", "scenario"]);
        a.state.settings_open = false;
        a
    });
    h.run_steps(3);
    assert_eq!((probe.total(), h.state().asset_models.len()), (0, 0), "nothing is asked until the page opens");
    h.state_mut().state.settings_open = true;
    h.run_steps(3);
    // The providers are held back, yet the window draws and says what it is waiting for.
    assert_eq!(h.state().asset_models.len(), 5);
    assert!(h.state().asset_models.values().all(|v| v.loading && !v.is_read()), "a request for each type is running");
    assert_eq!(h.state().jobs.running().filter(|j| j.title == "Ask the providers for their models").count(), 5);
    h.get_by_label_contains("Asking");
    assert!(h.get_by_label("Refresh").accesskit_node().is_disabled(), "a request is already running");
    h.get_by_label("Pictures model").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label("Asking the providers for their models…");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // Let the providers answer.
    probe.hold(false);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    assert_eq!(
        (probe.asked("fal"), probe.asked("scenario"), probe.asked("elevenlabs")),
        (5, 5, 0),
        "each keyed provider once per type; the one without a key never"
    );
    // Closing and opening it again soon after asks no one: the lists are young.
    let jobs_before = h.state().jobs.list.len();
    h.state_mut().state.settings_open = false;
    h.run_steps(2);
    h.state_mut().state.settings_open = true;
    h.run_steps(3);
    assert_eq!(h.state().jobs.list.len(), jobs_before, "no request for lists read a moment ago");
    // Lists read more than ten minutes ago are asked for again. The tool's own copy of each list is
    // young, so only the four lists a provider said it makes nothing of are asked for again (those
    // are never kept).
    let long_ago = craft_anything_engine::inbox::now_ms() - 11 * 60 * 1000;
    for v in h.state_mut().asset_models.values_mut() {
        v.fetched_ms = long_ago;
    }
    h.state_mut().state.settings_open = false;
    h.run_steps(2);
    h.state_mut().state.settings_open = true;
    h.run_steps(3);
    assert_eq!(h.state().jobs.list.len(), jobs_before + 5, "a request for each type");
    assert!(lists_read(&mut h));
    assert_eq!(probe.total(), 14);
    // Refresh asks every provider again, whatever it kept.
    h.get_by_label("Refresh").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Refresh").click();
    h.run_steps(3);
    assert!(lists_read(&mut h));
    assert_eq!(probe.total(), 24);
}

/// The default limit is edited in the unit of the provider chosen (dollars or credits), one number for
/// each unit, and it is kept in the settings.
#[test]
fn a_default_limit_is_edited_in_its_unit() {
    use craft_anything_engines::assets::AssetKind;
    use craft_anything_engines::assets::model::{MAX_ASSET_CREDITS, Pricing, Rate, Unit};
    use egui_kittest::kittest::NodeT;
    let (engine, _probe) = asset_engine(&["fal", "scenario"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || {
        let mut a = asset_app(engine, &["fal", "scenario"]);
        with_two_defaults(&mut a);
        let meshy = Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: Some(100.0) } };
        a.settings.assets.defaults.insert(AssetKind::Model3d, picked("scenario", "model_meshy-7-1-img23d", "Meshy 7.1", Unit::Credits, meshy));
        a
    });
    h.run_steps(3);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    let value = |h: &Harness<'static, CraftApp>, label: &str| h.get_by_label(label).accesskit_node().numeric_value();
    assert_eq!(
        (value(&h, "Pictures default limit"), value(&h, "3D models default limit"), value(&h, "Music default limit")),
        (Some(500.0), Some(500.0), Some(5.0))
    );
    // Credits: typed into the pictures row.
    h.get_by_label("Pictures default limit").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Pictures default limit").click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text("750".into()));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let settings = h.state().settings.assets.clone();
    assert_eq!((settings.credit_limit, settings.usd_limit), (750.0, 5.0), "credits changed, dollars didn't");
    assert_eq!(value(&h, "3D models default limit"), Some(750.0), "the other type billed in credits has the same default");
    assert_eq!(value(&h, "Music default limit"), Some(5.0));
    // Dollars: typed into the music row.
    h.get_by_label("Music default limit").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Music default limit").click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text("2.5".into()));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let settings = h.state().settings.assets.clone();
    assert_eq!((settings.credit_limit, settings.usd_limit), (750.0, 2.5));
    // More than a limit can be is cut down to the most.
    h.get_by_label("Pictures default limit").click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text("99999999999".into()));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().settings.assets.credit_limit, MAX_ASSET_CREDITS);
    // The change reaches the engine's settings, which is what saves them.
    h.run_steps(3);
    assert_eq!(h.state().engine.session().settings.assets.usd_limit, 2.5);
}

/// The "ignore the budget" default for new projects is a switch here; with it on, the limit column
/// says new projects start with their limits ignored.
#[test]
fn new_projects_can_ignore_the_budget_by_default() {
    let (engine, _probe) = asset_engine(&["fal", "scenario"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || {
        let mut a = asset_app(engine, &["fal", "scenario"]);
        with_two_defaults(&mut a);
        a
    });
    h.run_steps(3);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    assert!(!h.state().settings.assets.ignore_budget, "off unless the person turns it on");
    h.get_by_label("Ignore budget").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Ignore budget").click();
    h.run_steps(3);
    assert!(h.state().settings.assets.ignore_budget);
    assert_eq!(h.query_all_by_label("Ignored").count(), 2);
    assert!(h.query_by_label("On").is_none());
    h.run_steps(3);
    assert!(h.state().engine.session().settings.assets.ignore_budget, "kept in the settings the engine saves");
}

// ---- The project panel: generated assets per type --------------------------------------------

/// The app showing `ws` on the Project screen, with `keyed` as the providers that have a key.
fn project_app(engine: std::sync::Arc<craft_anything_engine::Engine>, keyed: &[&str], ws: craft_anything_engine::workspace::Workspace) -> CraftApp {
    let mut a = demo::app(engine);
    a.asset_keys = Some(["fal", "scenario", "elevenlabs"].into_iter().map(|id| (id.to_string(), keyed.contains(&id))).collect());
    a.open_workspace(ws);
    a
}

/// What a project has stored for `kind`, read from its folder.
fn stored_type(root: &std::path::Path, kind: craft_anything_engines::assets::AssetKind) -> Option<craft_anything_engine::workspace::TypeAssets> {
    craft_anything_engine::workspace::Workspace::open(root).unwrap().manifest.assets.types.get(&kind).cloned()
}

/// A project with scenario pictures (500 credits) and fal 3D models ($5), and the other types Off, stored.
fn project_with_two_types(parent: &std::path::Path) -> craft_anything_engine::workspace::Workspace {
    use craft_anything_engine::workspace::{TargetRef, TypeAssets, Workspace};
    use craft_anything_engines::assets::AssetKind;
    use craft_anything_engines::assets::model::{Pricing, Rate, Unit};
    let mut ws = Workspace::create(parent, "Snake", "A snake game", TargetRef::None).unwrap();
    ws.set_person(|m| {
        m.generate_assets = true;
        // A new project stores every type (the others Off, as Settings has no default for them).
        m.assets = craft_anything_engine::workspace::ProjectAssets::from_defaults(&craft_anything_engine::settings::AssetSettings::default(), 1);
        let quote = Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: Some(20.0) } };
        let tripo = Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.4) };
        m.assets.types.insert(
            AssetKind::Image,
            TypeAssets { choice: picked("scenario", "model_gemini-3-1", "Gemini 3.1", Unit::Credits, quote), limit: 500.0, limit_on: true, since_ms: 1 },
        );
        m.assets.types.insert(
            AssetKind::Model3d,
            TypeAssets { choice: picked("fal", "tripo3d/p1/image-to-3d", "Tripo P1", Unit::Usd, tripo), limit: 5.0, limit_on: true, since_ms: 1 },
        );
    })
    .unwrap();
    ws
}

/// "Ignore budget" in the project's panel is the project_assets_allow tool's `ignore_budget`: it is
/// saved with the project, and what a project has used is still counted while it is on.
#[test]
fn ignore_budget_switch_calls_the_tool() {
    use craft_anything_engine::workspace::Workspace;
    let d = tempfile::tempdir().unwrap();
    let ws = project_with_two_types(d.path());
    let root = ws.root.clone();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    let ignoring = |root: &std::path::Path| Workspace::open(root).unwrap().manifest.assets.ignore_budget;
    assert!(!ignoring(&root), "off unless the person turns it on");
    h.get_by_label("Ignore budget (no limits)").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.ignore_budget)));
    assert!(ignoring(&root), "saved with the project");
    assert_eq!(stored_type(&root, craft_anything_engines::assets::AssetKind::Image).map(|t| t.limit_on), Some(true), "each type's own switch is left alone");
    assert!(wait(&mut h, |a| a.facts.asset_usage.len() == 2));
    h.run_steps(2);
    h.get_by_label("0 /");
    h.get_by_label("$0.00 /");
    // And back to each type's own switch.
    h.get_by_label("Ignore budget (no limits)").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| !w.manifest.assets.ignore_budget)));
    assert!(!ignoring(&root));
}

/// A limit is typed in its row, in the provider's unit, and applied when the field is left; an amount
/// that isn't a limit is refused and the stored one comes back; Esc leaves it as it was; the row's
/// switch turns the limit on and off.
#[test]
fn a_limit_is_edited_in_the_row() {
    use craft_anything_engines::assets::AssetKind::{Image, Model3d};
    use egui_kittest::kittest::NodeT;
    let d = tempfile::tempdir().unwrap();
    let ws = project_with_two_types(d.path());
    let root = ws.root.clone();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    let type_into = |h: &mut Harness<'static, CraftApp>, label: &str, text: &str| {
        h.get_by_label(label).click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.run_steps(1);
        h.get_by_label(label).type_text(text);
        h.run_steps(2);
    };
    let shown = |h: &Harness<'static, CraftApp>, label: &str| h.get_by_label(label).value();
    assert_eq!((shown(&h, "Pictures limit").as_deref(), shown(&h, "3D models limit").as_deref()), (Some("500"), Some("$5.00")));
    // Credits: 750, applied with Enter.
    type_into(&mut h, "Pictures limit", "750");
    h.key_press(egui::Key::Enter);
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].limit == 750.0)));
    assert_eq!(stored_type(&root, Image).map(|t| (t.limit, t.limit_on)), Some((750.0, true)));
    assert_eq!(stored_type(&root, Model3d).map(|t| t.limit), Some(5.0), "the other type's limit is its own");
    h.run_steps(3);
    assert_eq!(shown(&h, "Pictures limit").as_deref(), Some("750"));
    // Dollars: $2.50, applied by leaving the field for another.
    type_into(&mut h, "3D models limit", "$2.5");
    h.get_by_label("Pictures limit").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Model3d].limit == 2.5)));
    assert_eq!(stored_type(&root, Image).map(|t| t.limit), Some(750.0));
    h.run_steps(3);
    assert_eq!(shown(&h, "3D models limit").as_deref(), Some("$2.50"));
    // Something that isn't a limit says so while it is typed, and is refused when the field is left.
    type_into(&mut h, "Pictures limit", "lots");
    h.get_by_label("Enter an amount from 0 to 10000000 credits.");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(shown(&h, "Pictures limit").as_deref(), Some("750"), "the stored limit comes back");
    assert!(h.query_by_label("Enter an amount from 0 to 10000000 credits.").is_none());
    type_into(&mut h, "3D models limit", "10001");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(stored_type(&root, Model3d).map(|t| t.limit), Some(2.5), "more than a limit can be is not applied");
    // Esc leaves a typed amount unapplied.
    type_into(&mut h, "Pictures limit", "900");
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert_eq!((shown(&h, "Pictures limit").as_deref(), stored_type(&root, Image).map(|t| t.limit)), (Some("750"), Some(750.0)));
    assert_eq!(h.state().state.screen, Screen::Project, "Esc was for the field: it doesn't take the person back to Home");
    // The switch.
    let on = |h: &Harness<'static, CraftApp>| h.get_by_label("Pictures limit switch").accesskit_node().toggled();
    assert_eq!(on(&h), Some(egui::accesskit::Toggled::True));
    h.get_by_label("Pictures limit switch").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| !w.manifest.assets.types[&Image].limit_on)));
    assert_eq!(stored_type(&root, Image).map(|t| (t.limit, t.limit_on)), Some((750.0, false)), "off keeps the number");
    h.run_steps(3);
    assert_eq!(on(&h), Some(egui::accesskit::Toggled::False));
    assert_eq!(stored_type(&root, Model3d).map(|t| t.limit_on), Some(true));
}

/// Selects what is in a limit field and types `text` over it.
fn type_into_field(h: &mut Harness<'static, CraftApp>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_label(label).type_text(text);
    h.run_steps(2);
}

/// A limit is changed while the project's ledger is damaged: the new limit is saved with the project
/// and the ledger is left exactly as it was, still reported as unreadable.
#[test]
fn a_limit_is_edited_while_the_ledger_is_damaged() {
    use craft_anything_engines::assets::AssetKind::Image;
    let d = tempfile::tempdir().unwrap();
    let ws = project_with_two_types(d.path());
    let root = ws.root.clone();
    let ledger = root.join("assets.json");
    let damaged = b"{not-json";
    std::fs::write(&ledger, damaged).unwrap();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    assert!(wait(&mut h, |a| a.facts.assets_error.is_some()), "the ledger is read, and found damaged");
    h.run_steps(2);
    h.get_by_label("Asset spending is unavailable. Repair assets.json before generating.");
    type_into_field(&mut h, "Pictures limit", "750");
    h.key_press(egui::Key::Enter);
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].limit == 750.0)));
    assert_eq!(stored_type(&root, Image).map(|t| (t.limit, t.limit_on)), Some((750.0, true)), "craft.json has the new limit");
    assert_eq!(std::fs::read(&ledger).unwrap(), damaged, "the ledger is as it was");
    // Switching the limit off is a change of the project too, and also leaves the ledger alone.
    h.run_steps(3);
    h.get_by_label("Pictures limit switch").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| !w.manifest.assets.types[&Image].limit_on)));
    assert_eq!(stored_type(&root, Image).map(|t| (t.limit, t.limit_on)), Some((750.0, false)));
    assert_eq!(std::fs::read(&ledger).unwrap(), damaged);
    h.run_steps(3);
    h.get_by_label("Asset spending is unavailable. Repair assets.json before generating.");
}

/// A limit field shows the limit to the hundredth; going into it and out again changes nothing, so
/// a limit with more places is not rounded by a visit. A fraction of a credit is shown as a number
/// that can be typed back, and is not called a wrong amount while the field has the focus.
#[test]
fn visiting_a_limit_field_does_not_round_the_limit() {
    use craft_anything_engines::assets::AssetKind::Image;
    let d = tempfile::tempdir().unwrap();
    let mut ws = project_with_two_types(d.path());
    ws.set_person(|m| {
        if let Some(t) = m.assets.types.get_mut(&Image) {
            t.limit = 0.456;
        }
    })
    .unwrap();
    let root = ws.root.clone();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    assert_eq!(h.get_by_label("Pictures limit").value().as_deref(), Some("0.46"));
    // In and out, to another field: nothing is saved.
    h.get_by_label("Pictures limit").click();
    h.run_steps(2);
    h.get_by_label("3D models limit").click();
    h.run_steps(8);
    assert_eq!(stored_type(&root, Image).map(|t| t.limit), Some(0.456), "the limit is as it was");
    assert_eq!(h.get_by_label("Pictures limit").value().as_deref(), Some("0.46"));
    // Typing what is shown is no change either.
    type_into_field(&mut h, "Pictures limit", "0.46");
    h.key_press(egui::Key::Enter);
    h.run_steps(8);
    assert_eq!(stored_type(&root, Image).map(|t| t.limit), Some(0.456));
    // A different amount is a change.
    type_into_field(&mut h, "Pictures limit", "0.7");
    h.key_press(egui::Key::Enter);
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].limit == 0.7)));
    assert_eq!(stored_type(&root, Image).map(|t| t.limit), Some(0.7));
    // A fraction of a credit is shown as a number, which is a limit.
    type_into_field(&mut h, "Pictures limit", "0.004");
    h.key_press(egui::Key::Enter);
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].limit == 0.004)));
    h.run_steps(3);
    assert_eq!(h.get_by_label("Pictures limit").value().as_deref(), Some("0.004"));
    h.get_by_label("Pictures limit").click();
    h.run_steps(3);
    assert!(h.query_by_label("Enter an amount from 0 to 10000000 credits.").is_none(), "what the field shows is an amount");
    h.get_by_label("3D models limit").click();
    h.run_steps(8);
    assert_eq!(stored_type(&root, Image).map(|t| t.limit), Some(0.004), "and visiting it leaves it as it is");
}

/// A type's provider and model are chosen in its row from the lists the providers gave: another
/// provider starts the count afresh in its own unit, another model of the same provider keeps it, a
/// model picked again has its stored price refreshed, and Off stops the type.
#[test]
fn a_type_is_chosen_in_its_row() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_engines::assets::AssetKind::Image;
    use craft_anything_engines::assets::model::{Pricing, Rate, Unit};
    let (engine, _probe) = asset_engine(&["fal", "scenario"]);
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    let root = ws.root.clone();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || project_app(engine, &["fal", "scenario"], ws));
    h.run_steps(3);
    // A project that never chose a type has what it takes from Settings (Off here) stored as it opens.
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| !w.manifest.assets.follows_settings())));
    assert_eq!(stored_type(&root, Image).map(|t| t.choice), Some(craft_anything_engine::asset_choice::AssetChoice::off()), "Off until the person chooses");
    h.run_steps(3);
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("Off"));
    assert!(h.query_by_label("Pictures limit").is_none());
    // The list opens beside the narrow panel and stays on the screen; opening it asks for the lists.
    h.get_by_label("Pictures model").click();
    assert!(wait(&mut h, |a| a.asset_models.get(&Image).is_some_and(|v| v.is_read() && !v.loading)));
    h.run_steps(3);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1100.0));
    for words in ["Nano Banana Pro · $0.15 a picture", "Gemini 3.1 · ~20 credits a picture", "Some new model · price unknown"] {
        let rect = h.get_by_label_contains(words).rect();
        assert!(screen.contains_rect(rect), "{words} is at {rect:?}, off the screen");
    }
    h.get_by_label_contains("Nano Banana Pro").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].choice.provider.is_some())));
    let t = stored_type(&root, Image).unwrap();
    let per_picture = Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.15) };
    assert_eq!(
        (t.choice.provider.as_deref(), t.choice.model.as_str(), t.choice.model_name.as_str()),
        (Some("fal"), "fal-ai/nano-banana-pro", "Nano Banana Pro")
    );
    assert_eq!((t.choice.unit, t.choice.pricing.clone(), t.limit, t.limit_on), (Unit::Usd, per_picture.clone(), 5.0, true));
    let first_count = t.since_ms;
    assert!(first_count > 0, "its usage counts from now");
    h.run_steps(3);
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("fal.ai · Nano Banana Pro"));
    assert_eq!(h.get_by_label("Pictures limit").value().as_deref(), Some("$5.00"));
    assert!(wait(&mut h, |a| a.facts.asset_usage.contains_key(&Image)));
    h.run_steps(2);
    h.get_by_label("$0.00 /");
    // Another model of the same provider: the limit, its switch and the count stay.
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label_contains("FLUX.1 [dev]").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].choice.model == "fal-ai/flux/dev")));
    let t = stored_type(&root, Image).unwrap();
    assert_eq!(
        (t.limit, t.limit_on, t.since_ms, t.choice.pricing),
        (5.0, true, first_count, Pricing::Before { unit: Unit::Usd, rate: Rate::PerMegapixel(0.025) })
    );
    // A model picked again has the price the provider lists now stored in place of an old one.
    let stale = Pricing::Unknown;
    Workspace::open(&root).unwrap().set_person(|m| m.assets.types.get_mut(&Image).unwrap().choice.pricing = Pricing::Unknown).unwrap();
    h.state_mut().workspace.as_mut().unwrap().manifest.assets.types.get_mut(&Image).unwrap().choice.pricing = Pricing::Unknown;
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label_contains("FLUX.1 [dev]").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].choice.pricing != stale)));
    let t = stored_type(&root, Image).unwrap();
    assert_eq!((t.choice.model.as_str(), t.limit, t.since_ms), ("fal-ai/flux/dev", 5.0, first_count), "only the price is refreshed");
    // Another provider: its own unit, the default limit in it, a count that starts afresh.
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label_contains("Gemini 3.1").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].choice.provider.as_deref() == Some("scenario"))));
    let t = stored_type(&root, Image).unwrap();
    assert_eq!((t.choice.unit, t.limit, t.limit_on), (Unit::Credits, 500.0, true));
    assert!(t.since_ms >= first_count);
    h.run_steps(3);
    assert_eq!(h.get_by_label("Pictures limit").value().as_deref(), Some("500"));
    // Off: nothing of this type is made, and there is no limit to show.
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label("Off").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.types[&Image].choice.provider.is_none())));
    h.run_steps(3);
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("Off"));
    assert!(h.query_by_label("Pictures limit").is_none() && h.query_by_label("Pictures limit switch").is_none());
}

/// The table fits the side panel at its width: every row's controls sit inside it.
#[test]
fn the_asset_rows_stay_inside_the_side_panel() {
    let d = tempfile::tempdir().unwrap();
    let ws = project_with_two_types(d.path());
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(4);
    // The panel is 300 points at the right edge, with 20 points of margin inside it.
    let (left, right) = (1280.0 - 300.0 + 20.0 - 1.0, 1280.0 - 20.0 + 1.0);
    for label in [
        "Pictures",
        "Pictures model",
        "Pictures limit",
        "Pictures limit switch",
        "3D models limit switch",
        "Sound effects model",
        "Music model",
        "Ignore budget (no limits)",
    ] {
        let rect = h.get_by_label(label).rect();
        assert!(rect.left() >= left && rect.right() <= right, "{label} is at {rect:?}, outside the panel's {left}..{right}");
    }
}

/// Pausing generated assets keeps every type's choice and limit, on screen and on disk; turning it back
/// on resumes those same choices.
#[test]
fn pausing_generated_assets_keeps_the_choices() {
    let d = tempfile::tempdir().unwrap();
    let ws = project_with_two_types(d.path());
    let root = ws.root.clone();
    let before = craft_anything_engine::workspace::Workspace::open(&root).unwrap().manifest.assets;
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    assert!(h.query_by_label("Off. Turning this on uses the choices below.").is_none());
    h.get_by_label("Generate pictures, models and audio").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| !w.manifest.generate_assets)));
    h.run_steps(3);
    h.get_by_label("Off. Turning this on uses the choices below.");
    assert_eq!(craft_anything_engine::workspace::Workspace::open(&root).unwrap().manifest.assets, before, "pausing keeps the choices and limits");
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("Scenario · Gemini 3.1"));
    assert_eq!(h.get_by_label("3D models limit").value().as_deref(), Some("$5.00"));
    h.get_by_label("Generate pictures, models and audio").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.generate_assets)));
    assert_eq!(craft_anything_engine::workspace::Workspace::open(&root).unwrap().manifest.assets, before, "resuming starts from the same choices");
}

/// A project that has chosen nothing shows every type as Off, with no limit to set.
#[test]
fn a_project_with_nothing_chosen_shows_every_type_off() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    for title in ["Pictures", "3D models", "Speech", "Sound effects", "Music"] {
        assert_eq!(h.get_by_label(&format!("{title} model")).value().as_deref(), Some("Off"), "{title}");
        assert!(h.query_by_label(&format!("{title} limit")).is_none() && h.query_by_label(&format!("{title} limit switch")).is_none());
    }
    h.get_by_label("Generate pictures, models and audio");
    h.get_by_label("Ignore budget (no limits)");
    h.get_by_label("Manage provider keys");
    // The shared budget field and its summary lines are gone.
    assert!(
        h.query_by_label("Save budget").is_none() && h.query_by_label_contains("Budget $").is_none() && h.query_by_label_contains("estimated spent").is_none()
    );
}

/// A model with no price, one priced only after each run, and a provider without a key each have a
/// line under the table that says so.
#[test]
fn the_notes_under_the_table_name_the_type_they_are_about() {
    use craft_anything_engine::workspace::{TargetRef, TypeAssets, Workspace};
    use craft_anything_engines::assets::AssetKind::{Image, Music, SoundEffect};
    use craft_anything_engines::assets::model::{Pricing, Unit};
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    ws.set_person(|m| {
        m.generate_assets = true;
        let mut put = |kind, choice| {
            m.assets.types.insert(kind, TypeAssets { choice, limit: 5.0, limit_on: true, since_ms: 1 });
        };
        put(SoundEffect, picked("fal", "fal-ai/stable-audio", "Stable Audio", Unit::Usd, Pricing::Unknown));
        put(Music, picked("elevenlabs", "music_v1", "Music v1", Unit::Credits, Pricing::After { unit: Unit::Credits }));
        put(Image, picked("fal", "fal-ai/flux/dev", "FLUX.1 [dev]", Unit::Usd, Pricing::After { unit: Unit::Usd }));
    })
    .unwrap();
    let (engine, _probe) = asset_engine(&["fal"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || project_app(engine, &["fal"], ws));
    h.run_steps(3);
    h.get_by_label("Sound effects: price unknown, usage not tracked");
    h.get_by_label("Music: may pass the limit by one generation");
    h.get_by_label("Pictures: may pass the limit by one generation");
    h.get_by_label("Music: no ElevenLabs key");
    assert!(h.query_by_label_contains("Pictures: no").is_none() && h.query_by_label_contains("Sound effects: no").is_none(), "fal has its key");
    assert!(h.query_by_label_contains("Speech:").is_none() && h.query_by_label_contains("3D models:").is_none(), "a type that is Off has no note");
}

/// Opening a project asks no provider for its model lists; opening a type's picker asks for that
/// type's lists on a worker when they are old, without holding the engine: the panel keeps drawing,
/// its picker says what it is waiting for, and the panel's switches save meanwhile.
#[test]
fn a_types_lists_are_asked_for_when_its_picker_opens() {
    use craft_anything_engines::assets::AssetKind::Image;
    let (engine, probe) = asset_engine(&["fal", "scenario"]);
    probe.hold(true);
    let d = tempfile::tempdir().unwrap();
    let ws = project_with_two_types(d.path());
    let root = ws.root.clone();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(engine);
        a.asset_keys = Some(["fal", "scenario", "elevenlabs"].into_iter().map(|id| (id.to_string(), id != "elevenlabs")).collect());
        a
    });
    h.run_steps(3);
    h.state_mut().open_workspace(ws);
    h.run_steps(3);
    let asking = |h: &Harness<'static, CraftApp>| h.state().jobs.running().filter(|j| j.title == "Ask the providers for their models").count();
    assert_eq!((asking(&h), probe.total(), h.state().asset_models.len()), (0, 0, 0), "opening the project asks no provider");
    // The pictures' picker opens: their lists are asked for, on a worker, while the panel draws.
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    assert_eq!(asking(&h), 1, "a request for pictures only");
    assert!(h.state().asset_models[&Image].loading);
    h.get_by_label("Asking the providers for their models…");
    // Closed by clicking its button again (Esc in its search field would leave the project).
    h.get_by_label("Pictures model").click();
    h.run_steps(2);
    assert!(wait(&mut h, |_| probe.total() == 1), "a provider is being asked, and held");
    // Meanwhile the engine is free: the panel's switch is saved through its tool at once.
    h.get_by_label("Ignore budget (no limits)").click();
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.assets.ignore_budget)));
    assert!(craft_anything_engine::workspace::Workspace::open(&root).unwrap().manifest.assets.ignore_budget, "saved with the project");
    assert!(h.state().asset_models[&Image].loading && probe.total() == 1, "while the provider is still held");
    probe.hold(false);
    assert!(wait(&mut h, |a| a.asset_models[&Image].is_read() && !a.asset_models[&Image].loading));
    assert_eq!((probe.asked("fal"), probe.asked("scenario"), probe.asked("elevenlabs")), (1, 1, 0), "each keyed provider once, for pictures");
    assert_eq!(h.state().asset_models.len(), 1, "no other type was asked for");
    // Opened again soon: the list is young, nobody is asked.
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(2);
    assert_eq!((asking(&h), probe.total()), (0, 2));
    // A list read more than ten minutes ago is asked for again when its picker opens.
    let long_ago = craft_anything_engine::inbox::now_ms() - 11 * 60 * 1000;
    h.state_mut().asset_models.get_mut(&Image).unwrap().fetched_ms = long_ago;
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    assert!(wait(&mut h, |a| a.asset_models[&Image].fetched_ms > long_ago && !a.asset_models[&Image].loading), "asked again, and answered");
    // The list kept on disk is young, so it is what comes back: no provider is asked again.
    assert_eq!((h.state().asset_models.len(), probe.total()), (1, 2));
}

/// A project from before each type had its own choice gets what it takes from Settings stored as the
/// app opens it; Settings changed afterwards doesn't change its rows or what is stored.
#[test]
fn an_old_project_keeps_its_asset_choices_when_settings_change() {
    use craft_anything_engines::assets::AssetKind::{Image, Music};
    let d = tempfile::tempdir().unwrap();
    let old = r#"{"version":3,"name":"Old","request":"A game","target":{"kind":"none"},"created_ms":1,"updated_ms":1,"generate_assets":true,"asset_budget_usd":12.0}"#;
    std::fs::write(d.path().join("craft.json"), old).unwrap();
    let ws = craft_anything_engine::workspace::Workspace::open(d.path()).unwrap();
    let root = ws.root.clone();
    let mut h = harness_sized(egui::vec2(1280.0, 1100.0), move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        with_two_defaults(&mut a);
        a.open_workspace(ws);
        a
    });
    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| !w.manifest.assets.follows_settings())), "stored as it opens");
    let (pictures, music) = (stored_type(&root, Image).unwrap(), stored_type(&root, Music).unwrap());
    assert_eq!((music.choice.model.as_str(), music.limit), ("fal-ai/lyria-3-pro", 12.0), "the old budget is the dollar type's limit");
    assert_eq!((pictures.choice.model.as_str(), pictures.limit), ("model_gemini-3-1", 500.0));
    // The person changes Settings afterwards.
    h.state_mut().settings.assets.defaults.clear();
    h.state_mut().settings.assets.usd_limit = 1.0;
    h.state_mut().settings.assets.credit_limit = 9.0;
    h.run_steps(3);
    assert_eq!(h.get_by_label("Music model").value().as_deref(), Some("fal.ai · Lyria 3 Pro"));
    assert_eq!(h.get_by_label("Music limit").value().as_deref(), Some("$12.00"));
    assert_eq!(h.get_by_label("Pictures model").value().as_deref(), Some("Scenario · Gemini 3.1"));
    assert_eq!((stored_type(&root, Image).unwrap(), stored_type(&root, Music).unwrap()), (pictures, music), "nothing stored changed");
}

/// Asking again for a type's lists (a key was saved or removed) stops the request it replaces, and the
/// answer of the one stopped is dropped.
#[test]
fn asking_for_a_types_lists_again_stops_the_request_it_replaces() {
    use craft_anything_engines::assets::AssetKind;
    let (engine, probe) = asset_engine(&["fal"]);
    probe.hold(true);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || asset_app(engine, &["fal"]));
    h.run_steps(3);
    let requests = |h: &Harness<'static, CraftApp>| -> Vec<(u64, bool)> {
        h.state().jobs.list.iter().filter(|j| j.title == "Ask the providers for their models").map(|j| (j.id, j.cancel.is_cancelled())).collect()
    };
    let first = requests(&h);
    assert_eq!(first.len(), 5);
    assert!(first.iter().all(|(_, cancelled)| !cancelled));
    h.state_mut().refresh_asset_models(AssetKind::Image, false);
    let now = requests(&h);
    assert_eq!(now.len(), 6);
    let stopped: Vec<u64> = now.iter().filter(|(_, cancelled)| *cancelled).map(|(id, _)| *id).collect();
    assert_eq!(stopped.len(), 1, "only the pictures request that was replaced: {now:?}");
    assert!(first.iter().any(|(id, _)| *id == stopped[0]));
    probe.hold(false);
    assert!(lists_read(&mut h));
    assert!(h.state().state.toasts.is_empty(), "stopping a request is not an error");
}

/// A provider whose key was just removed is not offered while the lists are asked for again, though
/// the lists on screen still have it; once the request answers it is gone for good.
#[test]
fn a_provider_whose_key_was_just_removed_is_not_offered_while_the_lists_are_asked_again() {
    use craft_anything_engines::assets::AssetKind::Image;
    let (engine, probe) = asset_engine(&["fal", "scenario"]);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || asset_app(engine, &["fal", "scenario"]));
    h.run_steps(3);
    assert!(lists_read(&mut h));
    h.run_steps(3);
    let has_fal = |a: &CraftApp| a.asset_models[&Image].providers.iter().any(|p| p.provider == "fal");
    assert!(has_fal(h.state()));
    // The engine is busy with something else, so the request that the removal sends waits its turn.
    let (held, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
    let engine = h.state().engine.clone();
    let busy = std::thread::spawn(move || {
        let _session = engine.session();
        let _ = held.0.send(());
        let _ = release.1.recv_timeout(Duration::from_secs(20));
    });
    held.1.recv().unwrap();
    // The key is removed, in the store and for the app.
    probe.keys.delete(&craft_anything_ai::keys::asset_key_name("fal")).unwrap();
    let keys: std::collections::BTreeMap<String, bool> =
        [("fal", false), ("scenario", true), ("elevenlabs", false)].map(|(id, saved)| (id.to_string(), saved)).into();
    h.state_mut().jobs.spawn("Forget the key", false, move |_| Ok(craft_anything_ui_egui::Done::AssetKeys(keys)));
    assert!(wait(&mut h, |a| a.asset_models[&Image].loading));
    h.run_steps(3);
    assert!(h.state().asset_models[&Image].loading && has_fal(h.state()), "still asking, and the lists on screen are the ones from before");
    h.get_by_label("Pictures model").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Pictures model").click();
    h.run_steps(3);
    h.get_by_label_contains("Gemini 3.1");
    assert!(h.query_by_label_contains("FLUX.1").is_none() && h.query_by_label_contains("Nano Banana Pro").is_none(), "fal.ai has no key now");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    drop(release.0);
    busy.join().unwrap();
    assert!(lists_read(&mut h));
    assert!(!has_fal(h.state()), "and the answer agrees");
}

/// A request for the model lists doesn't hold the engine: a Settings default changed while the
/// providers are asked is in the app at once, and reaches the engine's settings and the file without
/// waiting for the lists. (In a child process with a config folder of its own: the app saves to the
/// real one.)
#[test]
fn a_default_changed_while_the_providers_are_asked_reaches_the_session_and_the_file() {
    use craft_anything_platform::process::Process;
    const CHILD: &str = "CRAFT_ANYTHING_HELD_SETTINGS_TEST";
    let Some(dir) = std::env::var_os(CHILD) else {
        let dir = tempfile::tempdir().unwrap();
        let out = Process::new(std::env::current_exe().unwrap())
            .args(["--exact", "a_default_changed_while_the_providers_are_asked_reaches_the_session_and_the_file", "--nocapture"])
            .env(CHILD, dir.path())
            .env("CRAFT_ANYTHING_CONFIG_DIR", dir.path())
            .timeout(Duration::from_secs(120))
            .run_checked()
            .unwrap();
        // A name that matches no test also exits cleanly, having run nothing.
        let said = out.stdout_text();
        assert!(said.contains("1 passed"), "the child ran the test: {said}");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    assert_eq!(craft_anything_engine::Settings::path(), dir.join("config.toml"));
    let (engine, probe) = asset_engine(&["fal", "scenario"]);
    probe.hold(true);
    let mut h = harness_sized(egui::vec2(1280.0, 1500.0), move || {
        let mut a = asset_app(engine, &["fal", "scenario"]);
        a.persist = true;
        a
    });
    h.run_steps(3);
    assert_eq!(h.state().asset_models.len(), 5);
    assert!(h.state().asset_models.values().all(|v| v.loading), "the providers are being asked");
    assert!(wait(&mut h, |_| probe.total() > 0), "and held");
    assert!(wait(&mut h, |a| a.engine.try_session().is_some()), "while the engine is free");
    // The person changes a default while that goes on.
    h.get_by_label("Ignore budget").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Ignore budget").click();
    h.run_steps(3);
    assert!(h.state().settings.assets.ignore_budget, "the app has it at once");
    // The engine gets it, and it is written, with the providers still held.
    assert!(wait(&mut h, |a| a.engine.try_session().is_some_and(|s| s.settings.assets.ignore_budget)));
    assert!(wait(&mut h, |_| craft_anything_engine::Settings::load().assets.ignore_budget), "saved without waiting for the lists");
    assert!(h.state().asset_models.values().any(|v| v.loading), "the providers are still being asked");
    probe.hold(false);
    assert!(lists_read(&mut h));
}

#[test]
fn an_engine_download_shows_in_the_title_bar_on_every_screen() {
    use craft_anything_engines::game::Builder;
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(2);
    let id = h.state_mut().jobs.spawn("Getting Godot", true, |j| {
        j.report(craft_anything_engine::Progress { fraction: Some(0.42), message: "63 of 150 MB".into() });
        std::thread::sleep(Duration::from_secs(5));
        Ok(craft_anything_ui_egui::Done::Nothing)
    });
    h.state_mut().engine_flow.job = Some((Builder::Godot, id));
    assert!(wait(&mut h, |a| a.jobs.running().any(|j| j.progress().is_some())));
    h.run_steps(2);
    h.get_by_label("Getting Godot · 42%").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, Some(craft_anything_ui_egui::state::Modal::Engine(Builder::Godot)), "the bar opens the steps");
}

#[test]
fn a_projects_conversation_and_place_come_back_after_a_restart() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_ui_egui::state::{FeedItem, UserItem};
    struct Mem(std::collections::HashMap<String, String>);
    impl eframe::Storage for Mem {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();

    // One session: a conversation on the project, then the app closes.
    let mut first = demo::app(demo::engine(Mode::Beginner, true));
    first.open_workspace(ws);
    first.state.feed.push(FeedItem::User(UserItem::Text("Make it faster".into())));
    first.state.feed.push(FeedItem::Agent { text: "Done: the snake moves faster now.".into(), streaming: true });
    // A run keeps the feed in the project folder (here, as one would have).
    let root = first.workspace.as_ref().unwrap().root.clone();
    craft_anything_ai::rebuild::feed::save(&root, &first.state.feed).unwrap();
    let mut storage = Mem(Default::default());
    eframe::App::save(&mut first, &mut storage);

    // The next start reopens that project with everything in it.
    let mut next = demo::app(demo::engine(Mode::Beginner, true));
    next.load_prefs(Some(&storage));
    assert_eq!(next.state.screen, Screen::Project);
    assert_eq!(next.workspace.as_ref().map(|w| w.manifest.name.as_str()), Some("Snake"));
    let feed = &next.state.feed;
    assert!(feed.iter().any(|f| matches!(f, FeedItem::User(t) if t.text() == "Make it faster")), "{feed:?}");
    assert!(
        feed.iter().any(|f| matches!(f, FeedItem::Agent { text, streaming: false } if text.starts_with("Done:"))),
        "nothing is still streaming after a restart: {feed:?}"
    );
}

#[test]
fn the_builders_checklist_is_pinned_above_the_prompt_box() {
    use craft_anything_ui_egui::state::{FeedItem, PlanStatus, PlanStep};
    let step = |text: &str, status| PlanStep { text: text.into(), doing: text.into(), status };
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a.state.feed.push(FeedItem::Plan(vec![
            step("Lay out the arena", PlanStatus::Done),
            step("Add the snake", PlanStatus::Doing),
            step("Keep score", PlanStatus::Todo),
        ]));
        a
    });
    h.run_steps(3);
    h.get_by_label("1 of 3 done");
    h.get_by_label("Add the snake");
    h.get_by_label("Keep score");
}

#[test]
fn the_sidebar_lists_projects_and_moves_between_them() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_projects(&mut a);
        a
    });
    h.run_steps(3);
    h.get_by_label("New project");
    assert!(h.query_by_label("Look inside an app").is_none(), "looking inside is on the New project screen");
    h.get_by_label("Settings");
    h.get_by_label("Snake, Ready").click();
    h.run_steps(3);
    assert_eq!(h.state().state.screen, Screen::Project);
    assert_eq!(h.state().workspace.as_ref().map(|w| w.manifest.name.as_str()), Some("Snake"));
    h.get_by_label("New project").click();
    h.run_steps(3);
    assert_eq!(h.state().state.screen, Screen::Home);
    h.get_by_label("Settings").click();
    h.run_steps(3);
    assert!(h.state().state.settings_open);
}

#[test]
fn the_prompt_box_sends_on_enter_with_its_pictures() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_ui_egui::state::FeedItem;
    let d = tempfile::tempdir().unwrap();
    let pic = d.path().join("shot.png");
    std::fs::write(&pic, "x").unwrap();
    let ws = Workspace::create(&d.path().join("projects"), "Game", "Recreate this game", TargetRef::None).unwrap();
    let root = ws.root.clone();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.workspace = Some(ws);
        a.state.screen = Screen::Project;
        a
    });
    h.run_steps(3);
    h.state_mut().attach(vec![pic.clone(), d.path().join("notes.txt"), pic]);
    h.run_steps(2);
    assert_eq!(h.state().state.project_attachments.len(), 1, "pictures only, once each");
    h.get_by_label("shot.png");
    h.get_by_label("Ask for a change").focus();
    h.run_steps(2);
    h.get_by_label("Ask for a change").type_text("Make it faster");
    h.run_steps(2);
    // Shift+Enter adds a line; Enter sends.
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    h.run_steps(2);
    h.get_by_label("Ask for a change").type_text("and louder");
    h.run_steps(2);
    assert_eq!(h.state().state.project_input, "Make it faster\nand louder");
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    // The pictures are copied on a worker; the message goes once they're in.
    assert!(wait(&mut h, |a| a.state.feed.len() > 1));
    assert!(root.join("attachments/picture-1.png").is_file(), "the picture is copied into the project");
    assert!(h.state().state.project_input.is_empty() && h.state().state.project_attachments.is_empty());
    assert!(
        h.state()
            .state
            .feed
            .iter()
            .any(|f| matches!(f, FeedItem::User(t) if t.text().starts_with("Make it faster\nand louder") && t.text().contains("1 picture attached"))),
        "{:?}",
        h.state().state.feed
    );
}

#[test]
fn tab_completes_a_slash_command_and_keeps_focus() {
    use craft_anything_ai::rebuild::StandIns;
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let (dir, runs) = (d.path().to_path_buf(), StandIns::new());
    let mut h = harness(move || stand_in_app(dir, &runs));
    h.run_steps(2);
    h.state_mut().open_workspace(a);
    h.run_steps(2);
    h.get_by_label("Ask for a change").focus();
    h.run_steps(2);
    h.get_by_label("Ask for a change").type_text("/b");
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(h.state().state.project_input, "/btw ");
    assert!(h.get_by_label("Ask for a change").is_focused(), "the box keeps focus");
}

#[test]
fn pictures_of_the_original_go_with_the_draft_then_into_the_project() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let pic = d.path().join("original.webp");
    std::fs::write(&pic, "x").unwrap();
    let notes = d.path().join("notes.txt");
    std::fs::write(&notes, "x").unwrap();

    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(3);
    // "Nothing — start fresh" is picked: the request box offers pictures (not for websites).
    h.get_by_label("Add pictures of the original (optional: screenshots help it look right)");
    h.state_mut().add_pictures(vec![pic.clone(), notes, pic.clone()]);
    h.run_steps(3);
    assert_eq!(h.state().state.make.pictures, vec![pic.clone()], "pictures only, once each");
    h.get_by_label("original.webp");
    h.get_by_label("Remove original.webp").click();
    h.run_steps(3);
    assert!(h.state().state.make.pictures.is_empty());

    // On a project's page they go straight into the project.
    let ws = Workspace::create(&d.path().join("projects"), "Game", "Recreate this game", TargetRef::None).unwrap();
    let root = ws.root.clone();
    h.state_mut().workspace = Some(ws);
    h.state_mut().state.screen = Screen::Project;
    h.run_steps(3);
    h.get_by_label("Add pictures of the original");
    h.state_mut().add_pictures(vec![pic]);
    assert!(wait(&mut h, |a| a.facts.references == 1), "copied on a worker, then counted again");
    assert!(root.join("reference/picture-1.webp").is_file());
    h.run_steps(2);
    h.get_by_label("1 picture(s) to match.");
}

#[test]
fn a_finished_game_offers_to_play_it() {
    use craft_anything_engine::workspace::{Status, TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    // What `cargo build` leaves behind (never started here: the test doesn't click).
    let debug = ws.app_dir().join("target/debug");
    std::fs::create_dir_all(&debug).unwrap();
    std::fs::write(ws.app_dir().join("Cargo.toml"), "[package]").unwrap();
    let exe = debug.join(if cfg!(windows) { "snake.exe" } else { "snake" });
    std::fs::write(&exe, "x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ws.manifest.status = Status::Ready;
    ws.save().unwrap();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.workspace = Some(ws);
        a.state.screen = Screen::Project;
        a
    });
    // What was built is looked for on a worker, never while drawing.
    assert!(wait(&mut h, |a| a.facts.program));
    h.run_steps(2);
    h.get_by_label("Your game is ready");
    assert_eq!(h.get_all_by_label("Play the game").count(), 2, "in the sidebar and on the ready card");
}

/// A finished build can be pinned through scroll-accessible project actions and opened from the sidebar.
#[test]
fn a_finished_build_can_be_pinned_and_opened_from_the_sidebar() {
    use craft_anything_engine::workspace::{Status, TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    // What `cargo build` leaves behind (never started here: the pin is never clicked).
    let debug = ws.app_dir().join("target/debug");
    std::fs::create_dir_all(&debug).unwrap();
    std::fs::write(ws.app_dir().join("Cargo.toml"), "[package]").unwrap();
    let exe = debug.join(if cfg!(windows) { "snake.exe" } else { "snake" });
    std::fs::write(&exe, "x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ws.manifest.status = Status::Ready;
    ws.save().unwrap();
    let root = ws.root.clone();
    let projects = d.path().to_path_buf();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        // Pins are listed from the project folders in settings: only this test's.
        a.engine.session().settings.projects_dir = Some(projects);
        a.workspace = Some(ws);
        a.state.screen = Screen::Project;
        a
    });
    assert!(wait(&mut h, |a| a.facts.program));
    h.run_steps(2);
    assert!(h.query_by_label("Pinned builds").is_none(), "no Pinned section before anything is pinned");
    h.get_by_label("Pin this build").scroll_to_me();
    h.run();
    assert!(h.get_by_label("Pin this build").rect().bottom() < 820.0, "the project sidebar scrolls to its actions");
    h.get_by_label("Pin this build").click();
    // The dialog works out what a frozen copy takes; frozen is picked.
    assert!(wait(&mut h, |a| matches!(a.state.pin_draft.size, Some(Ok(_)))));
    h.run_steps(2);
    h.get_by_label("A frozen copy (recommended)");
    h.get_by_label("Pin it").click();
    assert!(wait(&mut h, |a| !a.pins.is_empty()), "the pin shows up in the sidebar");
    h.run_steps(2);
    assert!(root.join("pinned/1/app").is_dir(), "a frozen copy was made");
    h.get_by_label("Snake · pin 1");
    // The section folds away and opens again.
    h.get_by_label("Pinned builds").click();
    h.run_steps(2);
    assert!(h.query_by_label("Snake · pin 1").is_none());
    h.get_by_label("Pinned builds").click();
    h.run_steps(2);
    h.get_by_label("Snake · pin 1");
    // A folded sidebar keeps the pin as an icon, named for screen readers and on hover, and
    // nothing moves up or down.
    let (pin, refresh) = (h.get_by_label("Snake · pin 1").rect(), h.get_by_label("Look for projects again").rect());
    h.state_mut().state.sidebar_folded = true;
    h.run_steps(2);
    assert_eq!(h.get_by_label("Snake · pin 1").rect().top(), pin.top());
    let folded_refresh = h.get_by_label("Look for projects again").rect();
    assert_eq!(folded_refresh.top(), refresh.top(), "the refresh button stays, on the same line");
    // Everything is centred on the row icons.
    let row = h.get_by_label("New project").rect().center().x;
    for name in ["Show the sidebar", "Pinned builds", "Look for projects again"] {
        let x = h.get_by_label(name).rect().center().x;
        assert!((x - row).abs() <= 1.0, "{name} is at {x}, the rows at {row}");
    }
    // The section folds here too, as it does open.
    h.get_by_label("Pinned builds").click();
    h.run_steps(2);
    assert!(h.query_by_label("Snake · pin 1").is_none());
}

#[test]
fn the_builders_questions_can_come_to_the_person_for_one_project() {
    use craft_anything_engine::workspace::{Questions, TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    let root = ws.root.clone();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.workspace = Some(ws);
        a.state.screen = Screen::Project;
        a
    });
    h.run_steps(2);
    // Auto mode is on by default, so the builder decides, and the switch under the box says so.
    assert!(h.state().settings.auto_mode);
    let shown = |a: &craft_anything_ui_egui::CraftApp| a.settings.questions_in(a.workspace.as_ref().map(|w| &w.manifest));
    assert_eq!(shown(h.state()), Questions::Decide);
    h.get_by_label("Decide yourself");
    h.get_by_label("Ask me").click();
    assert!(wait(&mut h, |_| Workspace::open(&root).unwrap().manifest.questions == Some(Questions::Ask)), "saved in the project");
    h.run_steps(2);
    assert_eq!(shown(h.state()), Questions::Ask);
    assert!(h.state().settings.auto_mode, "Auto mode itself is unchanged");
}

#[test]
fn a_question_about_the_build_is_answered_without_building() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    let root = ws.root.clone();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.workspace = Some(ws);
        a.state.screen = Screen::Project;
        a
    });
    h.run_steps(2);
    let feed_before = h.state().state.feed.len();
    // /btw makes it a question, whatever the switch says.
    h.state_mut().state.project_input = "/btw where are the latest files".into();
    h.run_steps(1);
    h.get_by_label("Send (Enter)").click();
    h.run_steps(2);
    let asked = h.state().asked.get(&root).cloned().unwrap_or_default();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].after, feed_before, "it shows where it was asked");
    assert_eq!(asked[0].question, "where are the latest files");
    assert!(h.state().state.project_input.is_empty(), "the box is cleared");
    assert_eq!(h.state().state.feed.len(), feed_before, "nothing joined the build's conversation");
    assert!(!h.state().runs.is_busy(&root), "no build started");
    // This test engine has no AI layer: the card says why there's no answer.
    assert!(wait(&mut h, |a| a.asked.get(&root).is_some_and(|l| l[0].answer.is_some())));
    h.run_steps(2);
    h.get_by_label("where are the latest files");
    // It folds away and opens again.
    assert!(h.query_by_label("Show the answer").is_none());
    h.get_by_label("Hide the answer").click();
    h.run_steps(2);
    h.get_by_label("Show the answer").click();
    h.run_steps(2);
    h.get_by_label("Hide the answer");
    // The switch makes every message a question.
    h.get_by_label("Just ask").click();
    h.run_steps(2);
    assert!(h.state().state.just_ask);
    // A mistyped command stays in the box, whatever the switch says.
    h.state_mut().state.project_input = "/bwt where is the icon".into();
    h.run_steps(1);
    h.get_by_label("Send (Enter)").click();
    h.run_steps(2);
    assert_eq!(h.state().state.project_input, "/bwt where is the icon");
    assert_eq!(h.state().asked[&root].len(), 1);
    assert_eq!(h.state().state.feed.len(), feed_before);
    assert!(!h.state().runs.is_busy(&root));
}

#[test]
fn a_long_answer_scrolls_with_the_conversation_and_comes_back_after_a_restart() {
    use craft_anything_engine::questions::{self, AskedQuestion};
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    let long = (0..120).map(|n| format!("Line {n} of a long answer.")).collect::<Vec<_>>().join("\n\n");
    questions::add(&ws, AskedQuestion::new("how does it all work".into(), long, true, "Claude".into(), None, 0)).unwrap();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.screen = Screen::Home;
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    let bottom = h.ctx.content_rect().bottom();
    assert!(h.get_by_label("Ask for a change").rect().bottom() <= bottom, "the prompt box is on the screen");

    // Kept from before, it starts folded; the question shows and the answer doesn't.
    h.get_by_label("how does it all work");
    assert!(h.query_by_label("Hide the answer").is_none());
    h.get_by_label("Show the answer").click();
    h.run_steps(3);
    h.get_by_label("Hide the answer");
    assert!(h.get_by_label("Ask for a change").rect().bottom() <= bottom, "opening it doesn't push the prompt box off the screen");
}

#[test]
fn website_choice_stays_picked_while_the_address_is_empty() {
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(3);
    h.get_by_label("A website").click();
    // Several frames: the field used to vanish on the frame after the click.
    for _ in 0..3 {
        h.run_steps(2);
        assert!(h.state().state.make.editing_url);
        h.get_by_label("Type the website's address:");
        // A website is looked at as a visitor sees it, or read in Deep mode: asked.
        h.get_by_label("How closely may Craft Anything look at it?");
    }
    h.get_by_label("Nothing — start fresh").click();
    h.run_steps(3);
    assert!(!h.state().state.make.editing_url);
    assert!(h.query_by_label("How closely may Craft Anything look at it?").is_none(), "nothing to look at");
    assert!(h.query_by_label("Type the website's address:").is_none());
}

#[test]
fn mode_switch_and_deep_toggle() {
    let store = tempfile::tempdir().unwrap();
    let empty = store.path().to_path_buf();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.settings.engines.store_dir = Some(empty);
        a
    });
    h.run_steps(3);
    // Simple mode: the switch is in Settings.
    h.state_mut().state.settings_open = true;
    h.run_steps(3);
    h.get_by_label("Advanced").click();
    h.run_steps(4);
    h.state_mut().state.settings_open = false;
    h.run_steps(3);
    assert_eq!(h.state().settings.mode, Mode::Experienced);
    // The setting reaches the engine's session (it's what tools check).
    assert!(wait(&mut h, |a| a.engine.try_session().is_some_and(|s| s.settings.mode == Mode::Experienced)));
    h.state_mut().set_deep(true);
    assert!(wait(&mut h, |a| a.engine.try_session().is_some_and(|s| s.settings.deep_mode && !s.clean_room())));
    // No Ghidra in the store: Deep mode offers it first. (Hover first, as a mouse does: the
    // dialog's button only takes a click once the pointer has been over it for a frame.)
    h.get_by_label("Not now").hover();
    h.run_steps(2);
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, None, "Not now closes it");
    h.get_by_label("Simple").click();
    h.run_steps(3);
    assert_eq!(h.state().settings.mode, Mode::Beginner);
}

#[test]
fn a_failure_with_a_log_offers_to_open_it_and_one_without_does_not() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_ask(&mut a);
        a.state.chat = vec![ChatItem::Error { message: "It failed.".into(), remediation: Some("ssh printed: no".into()), log: None }];
        a
    });
    h.run_steps(4);
    h.get_by_label_contains("ssh printed: no");
    assert!(h.query_by_label("Open log").is_none());
    h.state_mut().state.chat = vec![ChatItem::Error { message: "It failed.".into(), remediation: None, log: Some("/tmp/agent.log".into()) }];
    h.run_steps(4);
    h.get_by_label("Open log");
}

#[test]
fn ask_screen_renders_every_chat_item_and_consent_answers() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_ask(&mut a);
        a
    });
    h.run_steps(4);
    h.get_by_label("Show me the proof");
    h.get_by_label("Don't allow").click();
    h.run_steps(3);
    let answered = h.state().state.chat.iter().any(|i| matches!(i, ChatItem::Consent { answer: Some(_), .. }));
    assert!(answered);
}

#[test]
fn asking_without_ai_runs_the_built_in_engines() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::deep(&mut a);
        a.state.backend = None;
        a.state.screen = Screen::Ask;
        a
    });
    h.run_steps(3);
    h.state_mut().send("What does this program do?");
    // A first-look tool always exists (core_session_show at worst); its answer and the call to
    // action arrive without blocking the UI.
    assert!(wait(&mut h, |a| a.state.chat.iter().any(|i| matches!(i, ChatItem::AddBrain))));
    h.run_steps(2);
    h.get_by_label("Add a brain").click();
    h.run_steps(2);
    assert!(h.state().state.settings_open);
}

#[test]
fn opening_a_file_lands_on_ask_with_a_first_look() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny-program");
    std::fs::write(&path, demo::sample_program()).unwrap();
    let mut h = harness(|| {
        let engine = std::sync::Arc::new(craft_anything_engine::Engine::new(craft_anything_engine::Session::new(craft_anything_engine::Settings {
            first_run_done: true,
            deep_mode: true,
            language: "en".into(),
            ..Default::default()
        })));
        demo::app(engine)
    });
    h.run_steps(2);
    h.state_mut().open_path(path.clone());
    assert!(wait(&mut h, |a| a.state.screen == Screen::Ask && a.view.active_target().is_some()));
    assert_eq!(h.state().settings.recent.first(), Some(&path));
    assert!(wait(&mut h, |a| a.state.chat.len() >= 2), "the first look arrives");
}

#[test]
fn a_picked_game_shows_what_its_made_of_and_can_be_asked_about() {
    let d = tempfile::tempdir().unwrap();
    let game = d.path().join("Game");
    let names = br#"{"names": ["Assembly-CSharp.dll", "FishNet.Runtime.dll", "UnityEngine.CoreModule.dll"]}"#;
    for (rel, bytes) in [
        ("Game.exe", &b"MZ"[..]),
        ("GameAssembly.dll", b"MZ"),
        ("Game_Data/globalgamemanagers", b"\0\0\0\0\x16\0\x002022.3.62f2\0\0junk"),
        ("Game_Data/ScriptingAssemblies.json", names),
    ] {
        let p = game.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::deep(&mut a);
        a.state.screen = Screen::GameFiles;
        a
    });
    h.run_steps(2);
    craft_anything_ui_egui::game_files::folder_picked(h.state_mut(), game.clone());
    assert!(wait(&mut h, |a| a.state.game.profile.is_some()), "the game is looked at");
    h.run_steps(2);
    h.get_by_label("Unity 2022.3.62f2");
    h.get_by_label("FishNet");
    // Asking opens the game in the chat, whose first look says what it's made of.
    h.get_by_label("Ask about this game").click();
    assert!(wait(&mut h, |a| a.state.screen == Screen::Ask && a.view.active_target().is_some()));
    assert!(wait(&mut h, |a| format!("{:?}", a.state.chat).contains("FishNet (multiplayer)")), "{:?}", h.state().state.chat);
}

#[test]
fn opening_a_missing_file_is_a_calm_error() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::deep(&mut a);
        a
    });
    h.run_steps(2);
    h.state_mut().open_path(std::path::PathBuf::from("/definitely/not/here.exe"));
    assert!(wait(&mut h, |a| a.state.toasts.iter().any(|(_, err)| *err)));
}

#[test]
fn clean_room_asks_for_deep_mode_instead_of_opening_a_file() {
    use craft_anything_ui_egui::state::Modal;
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(2);
    h.state_mut().open_path(std::path::PathBuf::from("/samples/tool.exe"));
    assert!(wait(&mut h, |a| a.state.modal == Some(Modal::DeepMode)), "Clean room never opens the original");
    assert!(!h.state().state.toasts.iter().any(|(_, err)| *err), "no error, a question");
    h.get_by_label("Looking inside needs Deep mode");
    // Hover first, as a mouse does: a dialog's button takes a click once the pointer was over it.
    h.get_by_label("Not now").hover();
    h.run_steps(2);
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, None);
    assert!(!h.state().settings.deep_mode);
    // The Game files screen asks the same before anything is picked.
    h.state_mut().state.screen = Screen::GameFiles;
    h.run_steps(3);
    assert!(h.query_by_label("Pick a game's folder").is_none());
    h.get_by_label("Turn on Deep mode").click();
    h.run_steps(3);
    assert!(h.state().settings.deep_mode);
    h.state_mut().state.modal = None;
    h.run_steps(2);
    h.get_by_label("Pick a game's folder");
}

#[test]
fn workbench_lists_fall_back_gracefully() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Experienced, true));
        demo::fill_workbench(&mut a);
        a
    });
    h.run_steps(4);
    h.get_by_label("percent_key_pressed");
    for kind in RailKind::ALL {
        if kind == RailKind::Targets {
            continue;
        }
        // Every rail is backed by a registered tool for every kind of target.
        for k in [None, Some(TargetKind::Managed), Some(TargetKind::Jar), Some(TargetKind::Apk), Some(TargetKind::MachO)] {
            assert!(h.state().first_tool(kind.tools(k)).is_some(), "{kind:?} for {k:?} names no registered tool");
        }
        h.state_mut().state.lists.remove(&kind);
        h.state_mut().state.rail = kind;
        h.run_steps(2);
    }
    // Every centre tab draws (built-in hex view included) and names a real tool.
    for tab in craft_anything_ui_egui::state::CenterTab::ALL {
        assert!(tab == craft_anything_ui_egui::state::CenterTab::Result || h.state().first_tool(tab.tools()).is_some(), "{tab:?}");
        h.state_mut().state.center = tab;
        h.run_steps(2);
    }
    // The Jobs tab reads "Jobs (1)" while a list is still loading, so let them finish first.
    assert!(wait(&mut h, |a| a.jobs.running().all(|j| !j.visible)), "the lists finished loading");
    for tab in ["Inspector", "Evidence", "Unknowns", "Jobs", "Log", "Agent"] {
        h.get_by_label(tab).click();
        h.run_steps(2);
    }
}

#[test]
fn the_agent_console_keeps_its_size() {
    // The console's history and prompt box came out 17 points taller than the panel, so the panel
    // grew by that every frame until its maximum, and then the top of its tabs was cut off.
    let mut h = harness_sized(egui::vec2(1890.0, 1025.0), || {
        let mut a = demo::app(demo::engine(Mode::Experienced, true));
        demo::fill_workbench(&mut a);
        a
    });
    h.run_steps(4);
    assert!(wait(&mut h, |a| a.jobs.running().all(|j| !j.visible)), "the lists finished loading");
    h.get_by_label("Agent").click();
    h.run_steps(2);
    let top = h.get_by_label("Agent").rect().top();
    h.run_steps(10);
    assert_eq!(h.get_by_label("Agent").rect().top(), top, "the agent console grew while nothing changed");
}

#[test]
fn command_palette_opens_and_lists_tools() {
    let mut h = harness(|| demo::app(demo::engine(Mode::Experienced, true)));
    h.run_steps(3);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    h.run_steps(3);
    assert!(h.state().state.palette.open);
    h.get_by_label("Session status");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().state.palette.open);
}

/// #584: the palette's list jumped back to the top after every scroll, because the selected row
/// asked to be scrolled into view on every frame.
#[test]
fn the_command_palette_stays_where_it_was_scrolled() {
    let mut h = harness(|| demo::app(demo::engine(Mode::Experienced, true)));
    h.run_steps(3);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    h.run_steps(3);
    let before = h.get_by_label("Session status").rect();
    // Over the list's first rows (this one may be below what the list shows).
    h.event(egui::Event::PointerMoved(egui::pos2(before.center().x, 250.0)));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -300.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(20);
    let after = h.get_by_label("Session status").rect();
    assert!(after.top() < before.top() - 100.0, "it scrolled and stayed scrolled: {before:?} → {after:?}");
    // The arrow keys still bring the selection back into view.
    h.key_press(egui::Key::ArrowDown);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(20);
    assert_eq!(h.get_by_label("Session status").rect().top(), before.top());
}

/// A stand-in model server whose every answer lists an embedding model, then `qwen3:8b`. Its
/// OpenAI-style address.
fn model_server() -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for mut s in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let body = r#"{"object":"list","data":[{"id":"nomic-embed-text"},{"id":"qwen3:8b"}]}"#;
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    url
}

/// #123, #127: the model an AI uses is changed in Settings → AI from the ones it lists.
#[test]
fn an_ais_model_is_changed_in_settings() {
    let url = model_server();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Experienced, true));
        a.hub.add_server("Box", &url, Some("nomic-embed-text"), None).unwrap();
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
        a
    });
    h.state_mut().refresh_backends();
    assert!(wait(&mut h, |a| a.backends.iter().any(|b| b.name == "Box")));
    h.run_steps(2);
    h.get_by_label("Model: nomic-embed-text");
    h.get_by_label("Change…").click();
    assert!(wait(&mut h, |a| a.model_picker.as_ref().is_some_and(|(_, m)| m.as_deref() == Some(&["nomic-embed-text".to_string(), "qwen3:8b".into()][..]))));
    h.run_steps(2);
    h.state_mut().set_model("Box", "qwen3:8b");
    assert!(wait(&mut h, |a| a.backends.iter().any(|b| b.name == "Box" && b.model == "qwen3:8b")));
    h.run_steps(2);
    h.get_by_label("Model: qwen3:8b");
    assert!(h.state().model_picker.is_none(), "the picker closes once a model is chosen");
}

/// #54, #98: an AI server on another computer is added by its address in Settings → AI, with a
/// model that can chat, and the form empties once it's added.
#[test]
fn an_ai_server_is_added_by_its_address() {
    let url = model_server();
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Experienced, true));
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
        a
    });
    h.run_steps(2);
    h.get_by_label("Add an AI by its address");
    h.state_mut().server_form = craft_anything_ui_egui::ServerForm { name: "Studio box".into(), address: url.clone(), ..Default::default() };
    h.run_steps(1);
    h.get_by_label("Add").click();
    assert!(wait(&mut h, |a| a.backends.iter().any(|b| b.name == "Studio box" && b.model == "qwen3:8b")), "added with a model that can chat");
    assert!(h.state().server_form.name.is_empty(), "the form empties once it's added");
}

/// #53: an agent app installed where it isn't looked for gets its program chosen in Settings → AI,
/// and goes back to being found by its usual name.
#[test]
fn an_agent_apps_program_is_set_in_settings() {
    use craft_anything_ai::{BackendConfig, BackendKind};
    let mut h = harness(|| {
        let a = demo::app(demo::engine(Mode::Experienced, true));
        let cfg =
            BackendConfig { name: "OpenCode".into(), kind: BackendKind::AgentCli, model: "opencode".into(), base_url: None, ssh_host: None, enabled: true };
        a.hub.add_backend(cfg, None).unwrap();
        a
    });
    h.state_mut().refresh_backends();
    assert!(wait(&mut h, |a| a.backends.iter().any(|b| b.name == "OpenCode")));
    h.state_mut().state.settings_open = true;
    h.state_mut().state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
    h.run_steps(2);
    h.get_by_label("Program: found automatically");
    h.state_mut().set_agent_program("OpenCode", Some("/opt/oc/opencode".into()));
    let program = |a: &CraftApp| a.backend_options.get("OpenCode").and_then(|o| o.command.clone());
    assert!(wait(&mut h, |a| program(a).as_deref() == Some("/opt/oc/opencode")));
    h.run_steps(2);
    h.get_by_label("Program: /opt/oc/opencode");
    h.get_by_label("Find it automatically").click();
    assert!(wait(&mut h, |a| program(a).is_none()));
}

/// Claude Code's effort is chosen in Settings → AI, from every level Claude offers.
#[test]
fn a_claude_ais_effort_is_set_in_settings() {
    use craft_anything_ai::{BackendConfig, BackendKind, Effort};
    let mut h = harness(|| {
        let a = demo::app(demo::engine(Mode::Experienced, true));
        let cfg = BackendConfig {
            name: "Claude Code".into(),
            kind: BackendKind::AgentCli,
            model: "claude-code".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        };
        a.hub.add_backend(cfg, None).unwrap();
        a
    });
    h.state_mut().refresh_backends();
    assert!(wait(&mut h, |a| a.backends.iter().any(|b| b.name == "Claude Code")));
    h.state_mut().state.settings_open = true;
    h.state_mut().state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
    h.run_steps(2);
    h.get_by_label("Effort:");
    assert!(h.state().backend_options.get("Claude Code").and_then(|o| o.effort).is_none(), "the usual level until one is chosen");
    // Chosen through the dropdown itself, which shows the level in use.
    h.get_by_value("high").click();
    h.run();
    h.get_by_label("max").click();
    assert!(wait(&mut h, |a| a.backend_options.get("Claude Code").and_then(|o| o.effort) == Some(Effort::Max)));
    h.run_steps(2);
    h.get_by_value("max");
}

/// The picker exposes only the chosen model's depths and clears an old depth when changing models.
#[test]
fn opencode_picker_offers_only_the_selected_models_thinking_depths() {
    use craft_anything_ai::{AgentModel, BackendConfig, BackendKind, Effort};
    use craft_anything_ui_egui::builders::AgentModelPicker;
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Experienced, true));
        a.hub
            .add_backend(
                BackendConfig { name: "OpenCode".into(), kind: BackendKind::AgentCli, model: "opencode".into(), base_url: None, ssh_host: None, enabled: true },
                None,
            )
            .unwrap();
        a.backends = a.hub.backends();
        a.backend_options = a.hub.options();
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
        a.agent_model_picker = Some(AgentModelPicker {
            request: 1,
            context: a.hub.model_context("OpenCode").unwrap(),
            model: Some("test/deep".into()),
            thinking: None,
            result: Some(Ok(vec![
                AgentModel { id: "test/deep".into(), name: "Deep".into(), thinking: Effort::ALL.into() },
                AgentModel { id: "test/small".into(), name: "Small".into(), thinking: vec![Effort::Low] },
            ])),
        });
        a
    });
    h.run_steps(3);
    h.get_all_by_label("Thinking depth").last().unwrap().click();
    h.run_steps(2);
    for label in ["Low", "Medium", "High", "Xhigh", "Max"] {
        h.get_by_label(label);
    }
    h.get_by_label("Max").click();
    h.run_steps(2);
    assert_eq!(h.state().agent_model_picker.as_ref().unwrap().thinking, Some(Effort::Max));
    h.get_by_label("OpenCode model").click();
    h.run_steps(2);
    h.get_by_label("Small · test/small").click();
    h.run_steps(2);
    assert!(h.state().agent_model_picker.as_ref().unwrap().thinking.is_none(), "a model change resets the draft depth");
    h.get_all_by_label("Thinking depth").last().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Low");
    assert!(h.query_by_label("Max").is_none());
    assert!(h.query_by_label("Xhigh").is_none());
}

#[test]
fn settings_window_tabs_draw() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.settings_open = true;
        a
    });
    h.run_steps(3);
    // The window keeps its width on every tab: its close button (left of the title bar's) doesn't
    // move (#68).
    let edge = |h: &Harness<'static, CraftApp>| h.get_all_by_label("Close").map(|n| n.rect().right()).fold(f32::INFINITY, f32::min);
    let right = edge(&h);
    for tab in ["Engines", "AI", "Builds", "Plugins", "Safety", "General"] {
        h.get_by_label(tab).click();
        h.run_steps(2);
        assert_eq!(edge(&h), right, "the Settings window changed width on {tab}");
    }
    h.get_by_label("Studio Light").click();
    h.run_steps(3);
    assert_eq!(h.state().settings.theme, "studio-light");

    // At the same window size, the same tab keeps its content bounds with and without scrolling.
    let content_bounds = |overflow: bool| {
        let mut h = harness_sized(egui::vec2(1000.0, 700.0), move || {
            let mut a = demo::app(demo::engine(Mode::Beginner, true));
            a.state.settings_open = true;
            a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Safety;
            a
        });
        h.run_steps(3);
        if overflow {
            h.state_mut().view.project = Some("Test project".into());
            h.state_mut().view.project_allows = (0..20).map(|n| format!("extra tool {n}")).collect();
            h.run_steps(2);
        }
        let content = h.get_by_label("Run to study").rect();
        let close_right = edge(&h);
        let scrollbars: Vec<_> = h.query_all_by_role(egui::accesskit::Role::ScrollBar).map(|node| node.rect()).collect();
        (content, close_right, scrollbars)
    };
    let (short_content, short_window_right, short_scrollbars) = content_bounds(false);
    let (long_content, long_window_right, long_scrollbars) = content_bounds(true);
    let scrollbar = long_scrollbars
        .iter()
        .copied()
        .filter(|r| r.left() < long_window_right)
        .min_by(|a, b| a.left().total_cmp(&b.left()))
        .expect("the overflowing Settings tab should show a scrollbar");
    assert!((short_window_right - long_window_right).abs() < 0.1, "the windows have different widths");
    assert!(short_scrollbars.iter().all(|r| r.left() > scrollbar.left()), "the short tab unexpectedly scrolls");
    assert_eq!(short_content, long_content, "the Settings content bounds changed when scrolling");
    assert!(
        long_content.right() <= scrollbar.left() - 14.0,
        "the content should stay at least 14 points before the scrollbar: content={long_content:?}, scrollbar={scrollbar:?}"
    );
}

#[test]
fn settings_controls_change_builds_art_and_studying() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.settings_open = true;
        a
    });
    h.run_steps(3);
    h.get_by_label("Builds").click();
    h.run_steps(2);
    h.get_by_label("Use as a goal").click();
    h.run_steps(2);
    assert_eq!(h.state().settings.build.picture_good_enough, Some(80.0));
    h.get_by_label("New projects ignore the budget");
    h.get_by_label("Plugins").click();
    h.run_steps(2);
    h.get_by_label("No plugins yet.");
    h.get_by_label("Safety").click();
    h.run_steps(2);
    assert!(!h.state().settings.runtime_study);
    h.get_by_label("Run to study").click();
    h.run_steps(2);
    assert!(h.state().settings.runtime_study);
}

#[test]
fn a_website_check_lists_each_pages_likeness() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a
    });
    h.run_steps(3);
    h.get_by_label("start page  96%");
    h.get_by_label("/about  91%").click();
    h.run_steps(2);
}

#[test]
fn a_check_shows_the_claims_that_dont_hold_and_the_rest_on_request() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a
    });
    h.run_steps(3);
    h.get_by_label("2 of 3 requirements met");
    h.get_by_label("/about doesn't scroll sideways at 390 px — it's 412 px wide");
    assert!(h.query_by_label("The start page loads every file it asks for").is_none(), "the ones that hold wait behind a click");
    h.get_by_label("Show the ones that are met").click();
    h.run_steps(3);
    h.get_by_label("The start page loads every file it asks for");
}

/// An app whose builds are stand-ins (one at a time), with an agent app as its Builder, the one
/// the queue checks for up front.
fn stand_in_app(projects: std::path::PathBuf, runs: &craft_anything_ai::rebuild::StandIns) -> CraftApp {
    use craft_anything_ai::{AiConfig, BackendConfig, BackendEntry, BackendKind, BackendOptions};
    let engine = demo::engine(Mode::Beginner, true);
    let mut cfg = AiConfig::default();
    let agent =
        BackendConfig { name: "Claude Code".into(), kind: BackendKind::AgentCli, model: "claude-code".into(), base_url: None, ssh_host: None, enabled: true };
    cfg.upsert(BackendEntry { config: agent, options: BackendOptions::default() });
    engine.session().settings.ai = cfg.to_table().unwrap();
    let mut app = demo::app(engine);
    demo::fill_projects(&mut app);
    app.settings.projects_dir = Some(projects);
    app.settings.build.parallel = 1;
    app.runs = craft_anything_ai::rebuild::RunSet::new(1).with_starter(runs.starter());
    app
}

#[test]
fn a_note_waiting_for_the_builder_can_be_taken_back_into_the_prompt_box() {
    use craft_anything_ai::rebuild::{RunEvent, StandIns};
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_ui_egui::state::FeedItem;
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let runs = StandIns::new();
    let (dir, r) = (d.path().to_path_buf(), runs.clone());
    let mut h = harness(move || stand_in_app(dir, &r));
    h.run_steps(2);
    h.state_mut().open_workspace(a.clone());
    h.state_mut().follow_up(Some("Add tags".into()));
    h.run_steps(2);
    // Notes sent while it works wait under the conversation, not in it.
    for note in ["Make them blue", "And sortable"] {
        h.state_mut().state.project_input = note.into();
        h.state_mut().send_message();
    }
    h.run_steps(3);
    let in_feed = |app: &CraftApp, text: &str| app.state.feed.iter().any(|f| matches!(f, FeedItem::User(t) if t.text() == text));
    assert!(!in_feed(h.state(), "Make them blue"));
    h.get_by_label("Make them blue");
    h.get_by_label("And sortable");
    // Taking the first back puts it in the box, before what's been typed since.
    h.state_mut().state.project_input = "draft".into();
    h.get_all_by_label("Take back: puts it in the box again").next().unwrap().click();
    h.run_steps(3);
    assert_eq!(h.state().state.project_input, "Make them blue\ndraft");
    assert!(h.query_by_label("Make them blue").is_none());
    // The builder reads the other: it joins the conversation and can't be taken back.
    assert!(runs.send(&a.root, RunEvent::Note { shown: "And sortable".into(), pictures: Vec::new() }));
    h.run_steps(2);
    assert!(in_feed(h.state(), "And sortable"));
    h.state_mut().pause();
    assert!(runs.finish(&a.root));
    h.run_steps(2);
}

#[test]
fn up_in_the_prompt_box_takes_back_the_last_waiting_note() {
    use craft_anything_ai::rebuild::StandIns;
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let runs = StandIns::new();
    let (dir, r) = (d.path().to_path_buf(), runs.clone());
    let mut h = harness(move || stand_in_app(dir, &r));
    h.run_steps(2);
    h.state_mut().open_workspace(a.clone());
    h.state_mut().follow_up(Some("Add tags".into()));
    h.run_steps(2);
    for note in ["Make them blue", "And sortable"] {
        h.state_mut().state.project_input = note.into();
        h.state_mut().send_message();
    }
    h.run_steps(3);
    h.get_by_label("Ask for a change").focus();
    h.run_steps(2);
    // In an empty box, Up brings back the last note, as Claude Code does with a queued message.
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(h.state().state.project_input, "And sortable");
    assert!(h.query_by_label("And sortable").is_none(), "it no longer waits under the conversation");
    // The cursor went to the end of the text, so Up now moves it instead of taking back another.
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(h.state().state.project_input, "And sortable", "Up away from the start edits the text");
    h.state_mut().state.project_input.clear();
    h.run_steps(2);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(h.state().state.project_input, "Make them blue", "the one before comes next");
    h.state_mut().pause();
    assert!(runs.finish(&a.root));
    h.run_steps(2);
}

#[test]
fn up_and_down_in_the_prompt_box_go_through_earlier_prompts() {
    use craft_anything_ai::rebuild::StandIns;
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_ui_egui::state::{FeedItem, UserItem};
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let (dir, runs) = (d.path().to_path_buf(), StandIns::new());
    let mut h = harness(move || stand_in_app(dir, &runs));
    h.run_steps(2);
    h.state_mut().open_workspace(a);
    h.run_steps(2);
    let user = |t: &str| FeedItem::User(UserItem::Text(t.into()));
    h.state_mut().state.feed = vec![user("A notes app"), user("Make it blue\n(1 picture attached)"), user("Make it blue"), user("First line\nSecond line")];
    h.state_mut().state.project_input = String::new();
    h.get_by_label("Ask for a change").focus();
    h.run_steps(2);
    let input = |h: &Harness<'static, CraftApp>| h.state().state.project_input.clone();
    let press = |h: &mut Harness<'static, CraftApp>, key| {
        h.key_press(key);
        h.run_steps(3);
    };
    let (up, down) = (egui::Key::ArrowUp, egui::Key::ArrowDown);
    let many_lines = "First line\nSecond line";

    // Up leaves the cursor at the start, so it keeps going. The same prompt twice in a row counts
    // once, and pictures aren't part of what was typed.
    press(&mut h, up);
    assert_eq!(input(&h), many_lines);
    press(&mut h, up);
    assert_eq!(input(&h), "Make it blue");
    press(&mut h, up);
    assert_eq!(input(&h), "A notes app");
    press(&mut h, up);
    assert_eq!(input(&h), "A notes app", "the oldest stays");

    // The cursor is at the start, so Down first goes to the end; then it leaves the cursor there.
    press(&mut h, down);
    assert_eq!(input(&h), "A notes app", "Down moved the cursor to the end");
    press(&mut h, down);
    assert_eq!(input(&h), "Make it blue");
    press(&mut h, down);
    assert_eq!(input(&h), many_lines);
    press(&mut h, down);
    assert_eq!(input(&h), "", "past the newest is what was being typed");

    // A half-typed message: Up first goes to its start, and it comes back after looking around.
    h.event(egui::Event::Text("half a th".into()));
    h.run_steps(3);
    press(&mut h, up);
    assert_eq!(input(&h), "half a th", "Up moved the cursor to the start");
    press(&mut h, up);
    assert_eq!(input(&h), many_lines);
    press(&mut h, down);
    assert_eq!(input(&h), many_lines, "Down moved the cursor to the last line");
    press(&mut h, down);
    assert_eq!(input(&h), many_lines, "and to the end of it");
    press(&mut h, down);
    assert_eq!(input(&h), "half a th", "only then is it left");

    // In several lines Up goes line by line, and cycles only from the very start.
    h.event(egui::Event::Text("\nb\nc".into()));
    h.run_steps(3);
    let typed = input(&h);
    assert_eq!(typed, "half a th\nb\nc");
    press(&mut h, up);
    assert_eq!(input(&h), typed, "to the middle line");
    press(&mut h, up);
    assert_eq!(input(&h), typed, "to the first line");
    press(&mut h, up);
    assert_eq!(input(&h), typed, "to the very start");
    press(&mut h, up);
    assert_eq!(input(&h), many_lines);
    press(&mut h, down);
    press(&mut h, down);
    press(&mut h, down);
    assert_eq!(input(&h), typed, "it was kept, lines and all");

    // Editing what came back ends it: Down at the end no longer changes the prompt.
    press(&mut h, up);
    press(&mut h, up);
    press(&mut h, up);
    press(&mut h, up);
    assert_eq!(input(&h), many_lines);
    h.state_mut().state.project_input.push_str(" and bold");
    h.run_steps(2);
    press(&mut h, down);
    press(&mut h, down);
    press(&mut h, down);
    assert_eq!(input(&h), "First line\nSecond line and bold");
}

#[test]
fn the_prompt_box_is_laid_out_with_its_new_height_in_the_frame_it_grows() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(2);
    h.state_mut().open_workspace(a);
    h.run_steps(3);
    let bottom = |h: &Harness<'static, CraftApp>| h.get_by_label("Ask for a change").rect().max.y;
    let before = bottom(&h);
    // Five more lines at once: the very next frame shown already has the taller box in place,
    // not one laid out with last frame's height (which pushed it off the bottom, then back).
    h.state_mut().state.project_input = "one
two
three
four
five
six"
    .into();
    h.step();
    let after = bottom(&h);
    h.run_steps(3);
    assert!((after - bottom(&h)).abs() < 0.5, "the box didn't move after the frame it grew in: {after} then {}", bottom(&h));
    assert!((after - before).abs() < 0.5, "it grows upwards, keeping its bottom edge: {before} then {after}");
}

#[test]
fn nothing_blinks_over_the_text_while_typing() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    // Everything painted over a spot of the typed text, after the text itself.
    fn over_text(shapes: &[egui::epaint::ClippedShape], spot: egui::Rect) -> Vec<String> {
        fn flatten(s: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match s {
                egui::Shape::Vec(v) => v.iter().for_each(|s| flatten(s, out)),
                s => out.push(s.clone()),
            }
        }
        let mut seen_text = false;
        let mut over = Vec::new();
        for cs in shapes {
            let mut flat = Vec::new();
            flatten(&cs.shape, &mut flat);
            for s in flat {
                if !s.visual_bounding_rect().intersects(spot) || !cs.clip_rect.intersects(spot) {
                    continue;
                }
                match &s {
                    egui::Shape::Text(t) if t.galley.text().starts_with("make") => seen_text = true,
                    // The window's own outline goes round everything.
                    egui::Shape::Rect(r) if r.fill.a() == 0 && r.rect.contains_rect(cs.clip_rect) => {}
                    _ if seen_text => over.push(format!("{s:?}").chars().take(160).collect()),
                    _ => {}
                }
            }
        }
        over
    }
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(2);
    h.state_mut().open_workspace(a);
    h.run_steps(3);
    h.get_by_label("Ask for a change").focus();
    h.run_steps(2);
    // The scroll area's "more below" fade showed for one frame after each keystroke, dimming
    // the whole line.
    for c in "make it blue".chars() {
        h.event(egui::Event::Text(c.to_string()));
        for _ in 0..3 {
            h.step();
            let field = h.get_by_label("Ask for a change").rect();
            let spot = egui::Rect::from_min_size(field.min + egui::vec2(2.0, 4.0), egui::vec2(16.0, 8.0));
            let over = over_text(&h.output().shapes, spot);
            assert!(over.is_empty(), "painted over the text after typing {c:?}: {over:?}");
        }
    }
    assert_eq!(h.state().state.project_input, "make it blue");
}

#[test]
fn builds_past_the_limit_wait_their_turn_on_screen() {
    use craft_anything_ai::rebuild::{RunEvent, StandIns, Step};
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_ui_egui::state::FeedItem;
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let b = Workspace::create(d.path(), "Beta", "A timer app", TargetRef::None).unwrap();
    let runs = StandIns::new();
    let (dir, r) = (d.path().to_path_buf(), runs.clone());
    let mut h = harness(move || stand_in_app(dir, &r));
    h.run_steps(2);

    // Alpha builds; Beta waits behind it.
    h.state_mut().open_workspace(a.clone());
    h.state_mut().follow_up(Some("Add tags".into()));
    assert_eq!(runs.started(), std::slice::from_ref(&a.root));
    h.state_mut().open_workspace(b.clone());
    h.state_mut().follow_up(Some("Add a lap button".into()));
    assert_eq!(h.state().queue_position(), Some(1));
    // A message for a project waiting its turn stays in the box instead of being lost.
    h.state_mut().state.project_input = "Make it red".into();
    h.state_mut().send_message();
    assert_eq!(h.state().state.project_input, "Make it red");
    h.state_mut().state.project_input.clear();
    assert!(wait(&mut h, |app| app.projects.len() == 2), "both projects listed");
    h.run_steps(3);
    h.get_by_label("Waiting for another build to finish: this one starts next.");
    h.get_by_label("Beta, Waiting");

    // Alpha's progress stays in Alpha's conversation, not on Beta's screen.
    assert!(runs.send(&a.root, RunEvent::Step { step: Step::Build, text: "Writing the tag list".into() }));
    h.run_steps(2);
    let shows = |app: &CraftApp, text: &str| app.state.feed.iter().any(|f| matches!(f, FeedItem::Step { text: t, .. } if t == text));
    assert!(!shows(h.state(), "Writing the tag list"));
    h.state_mut().open_workspace(a.clone());
    assert!(shows(h.state(), "Writing the tag list"), "opening a building project shows its own feed");

    // Alpha finishes: Beta starts in its place.
    assert!(runs.finish(&a.root));
    h.run_steps(3);
    assert_eq!(runs.started(), [a.root.clone(), b.root.clone()]);
    h.state_mut().open_workspace(b.clone());
    assert_eq!(h.state().queue_position(), None);
    h.run_steps(2);
    assert!(h.query_by_label("Waiting for another build to finish: this one starts next.").is_none());
    h.state_mut().pause();
    assert!(runs.finish(&b.root));
    h.run_steps(2);
    assert!(!h.state().busy());
}

#[test]
fn a_projects_saved_time_and_ai_use_show_beside_it_and_the_session_total_in_the_title_bar() {
    use craft_anything_engine::workspace::ProjectUsage;
    // What earlier builds saved in the project folder: shown after a restart too.
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    ProjectUsage::update(&root, |u| {
        u.work_ms = 725_000;
        u.add_turn(1_180_000, 42_000, Some(3.21), true);
    })
    .unwrap();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        if let Some(ws) = a.workspace.as_mut() {
            ws.root = root.clone();
        }
        a.session_usage.add_turn(1_180_000, 42_000, Some(3.21), true);
        a
    });
    h.run_steps(3);
    h.get_by_label("Time spent");
    h.get_by_label("12m 05s");
    // A coding agent's figure is called an estimate, in the open and not only on hover.
    h.get_by_label("AI cost (estimate)");
    h.get_by_label("The coding agent's estimate at API prices, not your bill. A subscription isn't charged this.");
    h.get_by_label("1.2M");
    // The project's row and the title bar's session total.
    h.get_by_label("about $3.21");
    h.get_by_label("about $3.21 (estimate)");
}

#[test]
fn ghidras_memory_is_automatic_until_the_person_sets_it() {
    let mut h = harness_sized(egui::vec2(1280.0, 1000.0), || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Engines;
        a
    });
    h.run_steps(3);
    assert_eq!(h.state().settings.engines.ghidra.memory_gb, None);
    h.get_by_label_contains("Half of this computer's memory");
    h.get_by_label("Custom").click();
    h.run_steps(3);
    assert_eq!(h.state().settings.engines.ghidra.memory_gb, Some(16), "starts at what a 500 MB program wants");
    assert_eq!(h.get_all_by_label("Memory for Ghidra").count(), 2, "the row and its slider");
    h.get_by_label("Automatic").click();
    h.run_steps(3);
    assert_eq!(h.state().settings.engines.ghidra.memory_gb, None);
}

#[test]
fn the_front_door_makes_something_or_just_looks_inside() {
    use craft_anything_ui_egui::state::Modal;
    let mut h = harness(|| demo::app(demo::engine(Mode::Beginner, true)));
    h.run_steps(3);
    h.get_by_label("What do you want to make?");
    h.get_by_label("Just look inside").click();
    h.run_steps(3);
    assert!(h.state().state.make.look);
    h.get_by_label("What do you want to look inside?");
    // Each way in reads the original, so Clean room asks for Deep mode first.
    assert!(h.query_by_label("An app or a file").is_none());
    h.get_by_label("Looking inside needs Deep mode");
    h.get_by_label("Turn on Deep mode").click();
    h.run_steps(3);
    assert!(h.state().settings.deep_mode);
    // Without Ghidra here, Deep mode offers it; not now.
    h.state_mut().state.modal = None;
    h.run_steps(2);
    h.get_by_label("An app or a file");
    h.get_by_label("A website").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, Some(Modal::Website));
    h.state_mut().state.modal = None;
    h.run_steps(2);
    h.get_by_label("A game's files").click();
    h.run_steps(3);
    assert_eq!(h.state().state.screen, Screen::GameFiles);
    // Back on New project, it's still on "Just look inside"; switching back shows the Make steps.
    h.get_by_label("New project").click();
    h.run_steps(3);
    h.get_by_label("Make my own version").click();
    h.run_steps(3);
    h.get_by_label("What do you want to recreate?");
}

#[test]
fn a_running_build_says_what_it_is_on_and_opens_up_into_its_activity() {
    use craft_anything_ai::rebuild::{RunEvent, RunSet, StandIns};
    use craft_anything_ai::{AgentEvent, AiConfig, BackendConfig, BackendEntry, BackendKind, BackendOptions};
    use craft_anything_engine::workspace::{Step, TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let runs = StandIns::new();
    let (dir, starter) = (d.path().to_path_buf(), runs.starter());
    let mut h = harness(move || {
        let engine = demo::engine(Mode::Beginner, true);
        let mut cfg = AiConfig::default();
        let agent = BackendConfig {
            name: "Claude Code".into(),
            kind: BackendKind::AgentCli,
            model: "claude-code".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        };
        cfg.upsert(BackendEntry { config: agent, options: BackendOptions::default() });
        engine.session().settings.ai = cfg.to_table().unwrap();
        let mut app = demo::app(engine);
        app.settings.projects_dir = Some(dir);
        app.runs = RunSet::new(1).with_starter(starter);
        app
    });
    h.run_steps(2);
    h.state_mut().open_workspace(ws.clone());
    h.state_mut().continue_build();
    h.run_steps(2);
    let tool = |id: &str, name: &str, input: serde_json::Value| RunEvent::Agent(AgentEvent::ToolStarted { call_id: id.into(), tool: name.into(), input });
    assert!(runs.send(&ws.root, RunEvent::Step { step: Step::Build, text: "Building the first version…".into() }));
    assert!(runs.send(&ws.root, tool("a", "Read", serde_json::json!({"file_path": "src/main.rs"}))));
    assert!(runs.send(
        &ws.root,
        RunEvent::Agent(AgentEvent::ToolFinished { call_id: "a".into(), tool: "Read".into(), ok: true, summary: "fn main".into(), evidence_id: None })
    ));
    h.run_steps(2);
    // Between actions it says what it did last.
    assert!(h.query_all_by_label_contains("Last: Reading src/main.rs").count() >= 1);
    assert!(runs.send(&ws.root, tool("b", "Bash", serde_json::json!({"command": "cargo test --quiet"}))));
    h.run_steps(2);
    assert!(h.query_all_by_label_contains("Now: Running cargo test --quiet").count() >= 1);
    // Folded until it's wanted, then the lines and the log.
    assert!(h.query_by_label("Open log").is_none());
    h.get_by_label("More detail").click();
    h.run_steps(3);
    h.get_by_label("Open log");
    h.get_by_label("Reading src/main.rs: fn main");
    h.get_by_label("Running cargo test --quiet");
    assert!(ws.root.join(craft_anything_engine::workspace::ACTIVITY_LOG).is_file());
    // Finished: the record stays, named for what it is now.
    assert!(runs.finish(&ws.root));
    h.run_steps(3);
    h.get_by_label("What happened in this build");
}

#[test]
fn a_build_that_stopped_part_way_offers_to_continue() {
    use craft_anything_ai::rebuild::{RunSet, StandIns};
    use craft_anything_ai::{AiConfig, BackendConfig, BackendEntry, BackendKind, BackendOptions};
    use craft_anything_engine::workspace::{Status, TargetRef, Workspace};
    let d = tempfile::tempdir().unwrap();
    // Paused by the person, and one the app closed on mid-build (its status still says Building).
    let mut paused = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    paused.set_status(Status::Stopped, "Paused. Send a message to continue.").unwrap();
    let mut cut = Workspace::create(d.path(), "Beta", "A timer app", TargetRef::None).unwrap();
    cut.set_status(Status::Building, "Building the first version…").unwrap();
    let runs = StandIns::new();
    let (dir, starter) = (d.path().to_path_buf(), runs.starter());
    let mut h = harness(move || {
        let engine = demo::engine(Mode::Beginner, true);
        let mut cfg = AiConfig::default();
        let agent = BackendConfig {
            name: "Claude Code".into(),
            kind: BackendKind::AgentCli,
            model: "claude-code".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        };
        cfg.upsert(BackendEntry { config: agent, options: BackendOptions::default() });
        engine.session().settings.ai = cfg.to_table().unwrap();
        let mut app = demo::app(engine);
        app.settings.projects_dir = Some(dir);
        app.runs = RunSet::new(1).with_starter(starter);
        app
    });
    h.run_steps(2);
    h.state_mut().refresh_projects();
    assert!(wait(&mut h, |app| app.projects.len() == 2), "both projects listed");
    h.state_mut().open_workspace(paused.clone());
    h.run_steps(3);
    // In the feed, and on the project's status pill.
    assert!(h.query_all_by_label("Paused").count() >= 1);
    // #210: "enough now". It's kept as it is and stops offering to carry on.
    h.get_by_label("It's done").click();
    assert!(wait(&mut h, |app| app.workspace.as_ref().is_some_and(|w| w.manifest.status == Status::Ready)), "called done");
    h.run_steps(3);
    assert_eq!(Workspace::open(&paused.root).unwrap().manifest.status, Status::Ready);
    assert!(h.query_by_label("Continue").is_none(), "nothing left to carry on");
    h.get_by_label(craft_anything_engine::tools::project_tools::CALLED_DONE);
    h.state_mut().open_workspace(cut.clone());
    h.run_steps(3);
    h.get_by_label("Beta, Interrupted");
    h.get_by_label("This build stopped part-way");
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(runs.continued(), std::slice::from_ref(&cut.root), "it carries on instead of starting over");
    assert!(h.query_by_label("This build stopped part-way").is_none(), "no card while it works");
}

#[test]
fn the_builders_open_questions_show_beside_the_project() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a
    });
    h.run_steps(3);
    h.get_by_label("Open questions");
    h.get_by_label("May the original's logo and text be reused, or must they be placeholders?");
    h.get_by_label("Answer any of them in a message below.");
}

#[test]
fn saves_a_picture_from_a_unity_bundle() {
    use craft_anything_testkit::games::{unity_bundle_with, unity_serialized_file, unity_texture2d};
    use craft_anything_ui_egui::{game_files, state::Modal};
    // A game whose only pictures are in a bundle: one DXT1 texture, its pixels in the bundle's .resS.
    let dir = tempfile::tempdir().unwrap();
    let (game, out) = (dir.path().join("Game"), dir.path().join("wall.png"));
    let aa = game.join("Game_Data/StreamingAssets/aa");
    std::fs::create_dir_all(&aa).unwrap();
    let wall = unity_texture2d("2022.3.62f1", "Wall", 4, 4, 10, None, Some(("archive:/CAB-0123456789abcdef/CAB-0123456789abcdef.resS", 0, 8)));
    let red = [0x00, 0xf8, 0x1f, 0x00, 0, 0, 0, 0];
    std::fs::write(aa.join("walls.bundle"), unity_bundle_with(&unity_serialized_file("2022.3.62f1", &[(28, wall)]), &red)).unwrap();
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::deep(&mut a);
        a
    });
    h.state_mut().state.screen = Screen::GameFiles;
    game_files::folder_picked(h.state_mut(), game);
    assert!(wait(&mut h, |a| !a.state.game.looking && !a.state.game.archives.is_empty()), "the bundle is found");
    let name = h.state().state.game.archives[0]["name"].as_str().unwrap().to_string();
    h.get_by_label(&name).click();
    assert!(wait(&mut h, |a| !a.state.game.loading && !a.state.game.rows.is_empty()), "its pictures are listed");
    h.run_steps(3);
    h.get_by_label("Wall");
    h.get_by_label("Save picture").click();
    h.run_steps(3);
    assert_eq!(h.state().state.modal, Some(Modal::GamePermission), "asked before anything is taken out");
    h.get_by_label("I have permission").click();
    h.run_steps(3);
    // Headless, the save dialog doesn't open; answer it as the person would.
    game_files::save_to(h.state_mut(), out.clone());
    assert!(wait(&mut h, |_| out.exists()), "the picture is saved");
    assert!(std::fs::read(&out).unwrap().starts_with(b"\x89PNG"));
}

#[test]
fn a_new_project_can_start_in_a_folder_that_already_has_the_persons_files() {
    use craft_anything_ai::rebuild::StandIns;
    use craft_anything_engine::workspace::{ExistingFiles, Workspace};
    use craft_anything_ui_egui::state::Modal;
    let (d, mine) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(mine.path().join("index.html"), "<p>hi</p>").unwrap();
    let runs = StandIns::new();
    let (dir, r) = (d.path().to_path_buf(), runs.clone());
    let mut h = harness(move || stand_in_app(dir, &r));
    h.run_steps(2);
    h.state_mut().state.make.request = "Add a contact form".into();
    craft_anything_ui_egui::folders::picked(h.state_mut(), mine.path().to_path_buf());
    h.run_steps(3);
    assert!(matches!(h.state().state.modal, Some(Modal::ExistingFiles)), "it asks what the files are");
    h.get_by_label("Don't ask again (you can change this in Settings)").click();
    h.run_steps(2);
    h.get_by_label("They're my app: edit them in place").click();
    h.run_steps(3);
    assert!(h.state().state.modal.is_none());
    assert_eq!(h.state().state.make.existing, Some(ExistingFiles::Edit));
    assert_eq!(h.state().settings.existing_files, Some(ExistingFiles::Edit), "not asked again");
    h.get_by_label("Pick another folder…");

    h.state_mut().start_make();
    assert!(wait(&mut h, |a| a.creating.is_none() && a.workspace.is_some()), "made on a worker");
    let ws = Workspace::open(mine.path()).expect("the project is in the picked folder");
    assert!(ws.manifest.app_in_root && ws.app_dir() == mine.path());
    assert_eq!(std::fs::read_to_string(mine.path().join("index.html")).unwrap(), "<p>hi</p>");
    assert_eq!(runs.started(), [mine.path().to_path_buf()]);
    assert!(h.state().settings.project_folders.contains(&mine.path().to_path_buf()), "it stays in the list");
    assert!(wait(&mut h, |app| app.projects.iter().any(|w| w.root == mine.path())), "and is listed");
    assert_eq!(std::fs::read_dir(d.path()).map(|r| r.count()).unwrap_or(0), 0, "nothing in the projects folder");

    // The next folder with files uses the remembered answer.
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("notes.md"), "ideas").unwrap();
    craft_anything_ui_egui::folders::picked(h.state_mut(), other.path().to_path_buf());
    h.run_steps(2);
    assert!(h.state().state.modal.is_none() && h.state().state.make.existing == Some(ExistingFiles::Edit));
    h.state_mut().pause();
    assert!(runs.finish(mine.path()));
    h.run_steps(2);
}

#[test]
fn settings_list_projects_from_more_folders_and_remove_them_from_the_list_only() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let (d, more) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let elsewhere = Workspace::create(more.path(), "Elsewhere", "A timer", TargetRef::None).unwrap();
    let dir = d.path().to_path_buf();
    let mut h = harness(move || {
        let mut app = demo::app(demo::engine(Mode::Beginner, true));
        app.settings.projects_dir = Some(dir);
        app
    });
    h.run_steps(2);
    h.state_mut().add_project_folder(more.path().to_path_buf());
    assert!(wait(&mut h, |app| app.projects.iter().any(|w| w.root == elsewhere.root)), "its projects are listed");
    h.state_mut().state.settings_open = true;
    h.run_steps(3);
    h.get_by_label("Add a folder…");
    let label = format!("Stop listing {}", more.path().display());
    h.get_by_label(&label).click();
    assert!(wait(&mut h, |app| app.projects.iter().all(|w| w.root != elsewhere.root)), "off the list");
    assert!(h.state().settings.project_folders.is_empty());
    assert!(elsewhere.root.join("craft.json").is_file(), "nothing is deleted");
}

#[test]
fn a_typed_space_stays_in_a_settings_path() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Engines;
        a
    });
    h.run_steps(3);
    h.get_all_by_label("Ghidra folder").last().unwrap().focus();
    h.run_steps(2);
    h.get_all_by_label("Ghidra folder").last().unwrap().type_text("/opt/my ");
    h.run_steps(2);
    h.get_all_by_label("Ghidra folder").last().unwrap().type_text("tools");
    h.run_steps(2);
    assert_eq!(h.state().settings.engines.ghidra.install_dir, Some(std::path::PathBuf::from("/opt/my tools")));
}

#[test]
fn a_paragraph_is_drawn_as_one_piece_of_text() {
    use craft_anything_ui_egui::state::FeedItem;
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a.state.feed.push(FeedItem::Agent { text: "The snake moves faster now and the score is bigger.".into(), streaming: false });
        a
    });
    h.run_steps(3);
    // Not one label per word: a screen reader reads it as one sentence, and a long reply is a
    // handful of widgets instead of hundreds.
    h.get_by_label("The snake moves faster now and the score is bigger.");
}

#[test]
fn make_says_who_builds_and_a_picked_builder_stays_with_the_project() {
    use craft_anything_ai::{BackendConfig, BackendKind, rebuild::StandIns};
    use craft_anything_engine::workspace::Workspace;
    let d = tempfile::tempdir().unwrap();
    let runs = StandIns::new();
    let (dir, r) = (d.path().to_path_buf(), runs.clone());
    let mut h = harness(move || {
        let mut a = stand_in_app(dir, &r);
        // Claude Code is set up, and so is an API key.
        let cfg = |name: &str, kind, model: &str| BackendConfig { name: name.into(), kind, model: model.into(), base_url: None, ssh_host: None, enabled: true };
        a.backends = vec![cfg("Claude", BackendKind::Anthropic, "claude-opus-5-5"), cfg("Claude Code", BackendKind::AgentCli, "claude-code")];
        a
    });
    h.run_steps(3);
    // The agent app builds by default, and every AI that can build is offered by name.
    let combo = h.get_all_by_label("Built by").last().unwrap();
    assert_eq!(combo.value().as_deref(), Some("Automatic (Claude Code)"));
    let names: Vec<String> = h.state().builder_choices().into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["Claude", "Claude Code"]);
    // Picked explicitly, though it's also the default: it stays with the project.
    h.state_mut().state.make.ai = Some("Claude Code".into());
    h.run_steps(2);
    assert_eq!(h.get_all_by_label("Built by").last().unwrap().value().as_deref(), Some("Claude Code"));
    h.state_mut().state.make.request = "A notes app".into();
    h.state_mut().start_make();
    assert!(wait(&mut h, |a| a.creating.is_none() && a.workspace.is_some()));
    let root = h.state().workspace.as_ref().unwrap().root.clone();
    assert_eq!(Workspace::open(&root).unwrap().manifest.builder.as_deref(), Some("Claude Code"), "the project remembers it");
    h.state_mut().pause();
    assert!(runs.finish(&root));
    h.run_steps(2);
}

#[test]
fn with_nothing_to_build_with_start_offers_what_was_found_instead_of_a_dead_end() {
    use craft_anything_ui_egui::state::Modal;
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.backends.clear();
        a.detected = Some(Vec::new());
        a
    });
    h.run_steps(3);
    h.get_by_label("Building needs a coding agent such as Claude Code, or an AI with an API key.");
    h.state_mut().state.make.request = "A notes app".into();
    h.state_mut().start_make();
    h.run_steps(2);
    assert_eq!(h.state().state.modal, Some(Modal::NeedBuilder { start: true }));
    // Found on this computer meanwhile (detection runs when the dialog opens).
    h.state_mut().detecting = false;
    h.state_mut().detected = Some(demo::detected());
    h.run_steps(2);
    h.get_by_label("Look again");
    h.get_by_label("Use an API key…");
    assert!(h.state().can_build(), "an agent app that was found can build: one rule for the warning and Start");
    h.get_by_label("Build with Claude Code");
}

#[test]
fn an_ai_can_be_turned_off_and_removed_in_settings() {
    use craft_anything_ai::rebuild::StandIns;
    let d = tempfile::tempdir().unwrap();
    let runs = StandIns::new();
    let (dir, r) = (d.path().to_path_buf(), runs.clone());
    let mut h = harness(move || {
        let mut a = stand_in_app(dir, &r);
        a.backends = a.hub.backends();
        a.state.settings_open = true;
        a.state.settings_tab = craft_anything_ui_egui::state::SettingsTab::Ai;
        a
    });
    h.run_steps(3);
    h.get_by_label("Turn off").click();
    assert!(wait(&mut h, |a| a.backends.first().is_some_and(|b| !b.enabled)), "kept, but off");
    assert!(!h.state().can_build(), "an AI that's off doesn't build");
    h.run_steps(2);
    h.get_by_label("Remove Claude Code").click();
    h.run_steps(2);
    h.get_by_label("Remove it and its key").click();
    assert!(wait(&mut h, |a| a.backends.is_empty()), "removed");
}

#[test]
fn start_is_in_view_at_a_common_window_height() {
    use craft_anything_engine::workspace::TargetRef;
    for app_target in [false, true] {
        let mut h = harness_sized(egui::vec2(1280.0, 900.0), move || {
            let mut a = demo::app(demo::engine(Mode::Beginner, true));
            if app_target {
                // An app to recreate adds the Clean room / Deep choice.
                a.state.make.target = TargetRef::Local { path: std::path::PathBuf::from("/samples/calculator") };
            }
            a
        });
        h.run_steps(3);
        let bottom = h.get_by_label("Start").rect().max.y;
        assert!(bottom <= 900.0 - 16.0, "Start ends at {bottom} px with an app picked: {app_target}");
    }
}

#[test]
fn typed_drafts_come_back_after_a_restart_and_stay_with_their_project() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    struct Mem(std::collections::HashMap<String, String>);
    impl eframe::Storage for Mem {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }
    let d = tempfile::tempdir().unwrap();
    let a = Workspace::create(d.path(), "Alpha", "A notes app", TargetRef::None).unwrap();
    let b = Workspace::create(d.path(), "Beta", "A snake game", TargetRef::None).unwrap();
    let mut first = demo::app(demo::engine(Mode::Beginner, true));
    first.state.make.request = "A website for my bakery".into();
    first.open_workspace(a.clone());
    first.state.project_input = "Make the notes bold".into();
    // Another project: its own box, empty; Alpha's text waits for Alpha.
    first.open_workspace(b.clone());
    assert!(first.state.project_input.is_empty(), "one project's text doesn't follow you to the next");
    first.state.project_input = "Make it faster".into();
    first.open_workspace(a.clone());
    assert_eq!(first.state.project_input, "Make the notes bold");
    let mut storage = Mem(Default::default());
    eframe::App::save(&mut first, &mut storage);

    let mut next = demo::app(demo::engine(Mode::Beginner, true));
    next.load_prefs(Some(&storage));
    assert_eq!(next.state.make.request, "A website for my bakery", "Make's draft is kept");
    assert_eq!(next.state.project_input, "Make the notes bold", "the open project's draft is kept");
    next.open_workspace(b);
    assert_eq!(next.state.project_input, "Make it faster", "and every other project's");
}

#[test]
fn the_ask_screen_waits_only_for_its_own_answer() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.screen = Screen::Ask;
        a
    });
    h.run_steps(2);
    // A long job that isn't the conversation (a download, a build's helper).
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let other = h.state_mut().jobs.spawn("Download an engine", true, move |j| {
        while matches!(rx.recv_timeout(Duration::from_millis(20)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) && !j.cancel.is_cancelled() {}
        Ok(craft_anything_ui_egui::Done::Nothing)
    });
    h.run_steps(2);
    assert!(!h.state().answering());
    assert!(h.query_by_label("Working on your answer").is_none(), "a download isn't an answer");
    h.state_mut().stop();
    h.run_steps(2);
    assert!(h.state().jobs.running().any(|j| j.id == other), "Stop on the Ask screen leaves other jobs alone");
    drop(tx);
    assert!(wait(&mut h, |a| a.jobs.running().next().is_none()));
}

#[test]
fn closing_the_ghidra_question_means_the_project_doesnt_start_later() {
    use craft_anything_ui_egui::state::Modal;
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.ghidra.start_after = Some(Box::new(a.state.make.clone()));
        a.state.modal = Some(Modal::Ghidra);
        a
    });
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().state.modal.is_none());
    assert!(h.state().ghidra.start_after.is_none(), "dismissed with nothing downloading: nothing waits for Ghidra");
}

#[test]
fn a_project_that_needs_you_says_so_in_the_sidebar() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_projects(&mut a);
        let root = a.projects.iter().find(|w| w.manifest.name == "Snake").unwrap().root.clone();
        a.asking.insert(root);
        a
    });
    h.run_steps(3);
    h.get_by_label("Snake, Needs you");
}

#[test]
fn a_paused_build_shows_where_it_stopped() {
    use craft_anything_ai::rebuild::Step;
    use craft_anything_engine::workspace::{Status, TargetRef, Workspace};
    use craft_anything_ui_egui::state::FeedItem;
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
    ws.manifest.status = Status::Stopped;
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.workspace = Some(ws);
        a.state.screen = Screen::Project;
        a.state.feed = vec![
            FeedItem::Step { step: Step::Study, text: "Studying".into() },
            FeedItem::Step { step: Step::Build, text: "Building".into() },
            FeedItem::Paused,
        ];
        a
    });
    h.run_steps(3);
    h.get_by_label("Study: Done");
    h.get_by_label("Build: Stopped here");
    assert!(h.query_by_label("Pause").is_none(), "nothing to pause");
}

#[test]
fn opening_another_target_starts_a_new_conversation() {
    use craft_anything_ui_egui::state::ChatItem;
    let d = tempfile::tempdir().unwrap();
    let other = d.path().join("notes.txt");
    std::fs::write(&other, "hello").unwrap();
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::deep(&mut a);
        a.state.chat = vec![ChatItem::User("What does the calculator's % key do?".into())];
        a
    });
    h.run_steps(3);
    h.state_mut().open_path(other);
    assert!(wait(&mut h, |a| !a.state.chat.iter().any(|c| matches!(c, ChatItem::User(t) if t.contains("calculator")))), "answers about two things don't mix");
}

#[test]
fn advanced_mode_keeps_projects_and_their_questions_in_sight() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Experienced, true));
        demo::fill_project(&mut a);
        a
    });
    h.run_steps(3);
    // The project's approval card is on screen in Advanced mode too, with the sidebar beside it.
    h.get_by_label("Allow once");
    h.get_by_label("Workbench").click();
    h.run_steps(3);
    assert_eq!(h.state().state.screen, Screen::Workbench);
    h.get_by_label("Projects and builds").click();
    h.run_steps(3);
    assert_eq!(h.state().state.screen, Screen::Project, "back to the open project");
    h.get_by_label("Allow once");
}

#[test]
fn the_website_dialog_is_ready_to_type_into() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.state.modal = Some(craft_anything_ui_egui::state::Modal::Website);
        a
    });
    h.run_steps(3);
    assert!(h.get_by_label("Website address").is_focused());
}

/// Scroll-accessible project actions rename, duplicate and delete the saved workspace through engine tools.
#[test]
fn a_project_is_renamed_duplicated_and_deleted_from_its_page() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_ui_egui::state::Modal;
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
    let root = ws.root.clone();
    let dir = d.path().to_path_buf();
    let mut h = harness(move || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        a.settings.projects_dir = Some(dir);
        a.open_workspace(ws);
        a
    });
    h.run_steps(3);
    h.get_by_label("Rename").scroll_to_me();
    h.run();
    assert!(h.get_by_label("Rename").rect().bottom() < 820.0, "the project sidebar scrolls to its actions");
    h.get_by_label("Rename").click();
    h.run_steps(2);
    assert_eq!(h.state().state.modal, Some(Modal::Rename(root.clone())));
    h.state_mut().state.rename_input = "Snake II".into();
    h.run_steps(1);
    assert!(h.get_by_label("Project name").is_focused(), "ready to type");
    h.key_press(egui::Key::Enter);

    assert!(wait(&mut h, |a| a.workspace.as_ref().is_some_and(|w| w.manifest.name == "Snake II")), "renamed through project_rename");

    h.get_by_label("Duplicate").scroll_to_me();
    h.run();
    h.get_by_label("Duplicate").click();
    assert!(wait(&mut h, |a| a.projects.iter().any(|w| w.manifest.name == "Snake II copy")), "the copy is listed");

    h.get_by_label("Delete the project").scroll_to_me();
    h.run();
    h.get_by_label("Delete the project").click();
    h.run_steps(2);
    h.get_by_label("Delete Snake II?");
    h.get_by_label("Delete it").click();
    assert!(wait(&mut h, |a| a.workspace.is_none() && a.state.screen == Screen::Home));
    assert!(!root.exists());
    assert!(wait(&mut h, |a| a.projects.len() == 1), "only the copy is left");
}

#[test]
fn messages_float_over_the_screen_and_an_error_isnt_replaced() {
    let mut h = harness(|| {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a
    });
    h.run_steps(3);
    let before = h.get_by_label("Ask for a change").rect();
    h.state_mut().toast("Couldn't save the settings.", true);
    h.state_mut().toast("Opened example.com", false);
    h.run_steps(3);
    h.get_by_label("Couldn't save the settings.");
    h.get_by_label("Opened example.com");
    assert_eq!(h.get_by_label("Ask for a change").rect(), before, "the screen under the messages doesn't move");
}

#[test]
fn a_narrow_window_folds_the_sidebar_and_puts_the_project_details_above_the_feed() {
    let mut h = harness_sized(egui::vec2(760.0, 820.0), || {
        let mut a = demo::app(demo::engine(Mode::Beginner, true));
        demo::fill_project(&mut a);
        a
    });
    h.run_steps(3);
    // Folded to icons: the names are still there for screen readers and on hover.
    let new = h.get_by_label("New project").rect();
    assert!(new.width() <= 60.0, "the sidebar is folded: {new:?}");
    h.get_by_label("Project details");
    assert!(h.query_by_label("Open folder").is_none(), "folded away until opened");
    let send = h.get_by_label("Send (Enter)").rect();
    assert!(send.max.x <= 760.0 && send.max.y <= 820.0, "the prompt box fits: {send:?}");
}

/// #74: a graphics driver can crash while the first frames are presented, so the start's mark is
/// cleared only after frames were shown; the callback runs once, keeps frames coming until then,
/// and also runs when the window is closed before that.
#[test]
fn the_first_frames_callback_runs_once_after_frames_were_shown() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut h = harness(move || {
        let mut app = demo::app(demo::engine(Mode::Beginner, false));
        app.after_first_frames(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        app
    });
    h.run_steps(8);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "once the frames were shown, and only once");
    h.run_steps(8);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
