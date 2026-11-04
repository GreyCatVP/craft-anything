//! Game engines in the UI: the "Build with" choice on Make, the step-by-step "Get Unity / Unreal /
//! Godot" dialog, the engines list in Settings, and the download progress shown everywhere. Also a
//! game's generated-assets switch and provider keys.
//!
//! Craft Anything downloads only what it can check (Godot against its official checksums; Unity
//! Hub and the Epic Games Launcher against their vendors' signatures). Sign-in, licences and the
//! engines themselves stay in the vendors' own apps, with the person.

use crate::asset_picker::ModelsView;
use crate::i18n::{tr, trf};
use crate::state::{MakeDraft, Modal, Screen};
use crate::theme::{self, Tokens};
use crate::{CraftApp, Done, icons, widgets};
use craft_anything_ai::rebuild::KindRegistry;
use craft_anything_engine::evidence::{ErrorCode, ToolError};
use craft_anything_engine::{CallOptions, Caller};
use craft_anything_engines::assets::{AssetKind, AssetProvider, AssetProviders};
use craft_anything_engines::game::{self, Builder, Inventory};
use craft_anything_engines::ghidra::{self, get};
use egui::{RichText, Ui, Vec2};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the person is in getting an engine.
#[derive(Debug, Default)]
pub struct Flow {
    /// The inventory refresh is running.
    pub checking: bool,
    pub checked_at: Option<Instant>,
    /// The download running for an engine, and its job.
    pub job: Option<(Builder, u64)>,
    /// Engines whose vendor installer was opened (waiting for the person to finish it).
    pub installer_opened: Vec<Builder>,
    /// Start the waiting project once the engine is ready.
    pub start_after: bool,
}

impl CraftApp {
    /// Look for game engines again (in the background; skipped while a look is running).
    pub fn refresh_game_engines(&mut self) {
        if self.engine_flow.checking || self.headless {
            return;
        }
        self.engine_flow.checking = true;
        let store = self.settings.engine_store_dir();
        self.jobs.spawn("Look for game engines", false, move |_| Ok(Done::GameEngines(game::inventory(&game::Places::here(store)))));
    }

    pub fn engines_found(&mut self, inv: Inventory) {
        self.game_engines = Some(inv);
        self.engine_flow.checking = false;
        self.engine_flow.checked_at = Some(Instant::now());
    }

    /// Look up which engine made the game the person just picked.
    pub fn detect_game_engine(&mut self, path: PathBuf) {
        self.state.make.found = None;
        self.jobs.spawn("Look at the game", false, move |_| {
            let folder = craft_anything_ai::rebuild::game_folder(&path);
            Ok(Done::GameFound { found: folder.as_deref().and_then(game::detect), path })
        });
    }

    /// Start getting `b`: download what we can check, else open the vendor's page.
    pub fn get_engine(&mut self, b: Builder, ctx: &egui::Context) {
        if self.engine_flow.job.is_some() {
            return;
        }
        // Installs go through the engines_install tool, as they do from the CLI or an agent.
        let install = |engine: &Arc<craft_anything_engine::Engine>, id: &'static str, j: &crate::jobs::JobCtx| {
            let progress = j.reporter();
            let opts = CallOptions::new(Caller::Ui).with_cancel(j.cancel.clone()).with_progress(&progress);
            engine.call("engines_install", serde_json::json!({"engine": id}), opts).map(|_| ()).map_err(|e| e.for_people())
        };
        let engine = self.engine.clone();
        let id = match b {
            Builder::Godot => self.jobs.spawn(tr("Getting Godot"), true, move |j| {
                install(&engine, "godot", j)?;
                Ok(Done::EngineGot { builder: b, installer: false })
            }),
            Builder::Unity | Builder::Unreal if game::installer(b).is_some() => {
                let title = if b == Builder::Unity { tr("Getting Unity Hub") } else { tr("Getting the Epic Games Launcher") };
                let tool_id = if b == Builder::Unity { "unity" } else { "unreal" };
                self.jobs.spawn(title, true, move |j| {
                    install(&engine, tool_id, j)?;
                    Ok(Done::EngineGot { builder: b, installer: true })
                })
            }
            _ => {
                ctx.open_url(egui::OpenUrl::new_tab(game::download_page(b)));
                return;
            }
        };
        self.engine_flow.job = Some((b, id));
    }

    pub fn engine_got(&mut self, b: Builder, installer: bool) {
        self.engine_flow.job = None;
        if installer {
            if !self.engine_flow.installer_opened.contains(&b) {
                self.engine_flow.installer_opened.push(b);
            }
            self.toast(tr("The installer is open. Follow its steps, then come back here."), false);
        } else {
            self.toast(trf("{name} is ready.", &[("name", b.name())]), false);
        }
        self.refresh_game_engines();
    }

    /// The download for `b`, while it runs: (fraction, "120 of 150 MB").
    fn engine_download(&self, b: Builder) -> Option<(Option<f32>, String)> {
        let (jb, id) = self.engine_flow.job?;
        let j = self.jobs.running().find(|j| j.id == id).filter(|_| jb == b)?;
        Some(j.progress().map(|p| (p.fraction, p.message)).unwrap_or((None, tr("Starting…"))))
    }
}

/// Getting Ghidra for Deep mode: asked first, then downloaded, checked and set up in the background.
#[derive(Debug, Default)]
pub struct GhidraFlow {
    /// Whether Ghidra was found; `None` until looked for again (after a settings change or install).
    pub ready: Option<bool>,
    /// The download running, as a job.
    pub job: Option<u64>,
    /// The project to start once Ghidra is ready (or the person goes on without it): the draft
    /// as it was when Start was pressed, so editing the next one meanwhile doesn't change it.
    pub start_after: Option<Box<crate::state::MakeDraft>>,
}

impl CraftApp {
    /// Ghidra is here for Deep mode to use (looked up once, then remembered).
    pub fn ghidra_ready(&mut self) -> bool {
        let settings = &self.settings;
        *self.ghidra.ready.get_or_insert_with(|| settings.ghidra_launcher().is_some())
    }

    /// Deep mode is on, but it can't use Ghidra yet.
    pub fn ghidra_missing(&mut self) -> bool {
        self.settings.deep_mode && !self.ghidra_ready()
    }

    /// Download Ghidra and its Java in the background (the person already agreed).
    pub fn get_ghidra(&mut self) {
        if self.ghidra.job.is_some() {
            return;
        }
        let engine = self.engine.clone();
        let id = self.jobs.spawn(tr("Getting Ghidra"), true, move |j| {
            let progress = j.reporter();
            let opts = CallOptions::new(Caller::Ui).with_cancel(j.cancel.clone()).with_progress(&progress);
            engine.call("engines_install", serde_json::json!({"engine": "ghidra"}), opts).map_err(|e| e.for_people())?;
            Ok(Done::GhidraGot)
        });
        self.ghidra.job = Some(id);
    }

    pub fn ghidra_got(&mut self) {
        let waiting = self.ghidra.start_after.take();
        self.ghidra = GhidraFlow::default();
        if self.ghidra_ready() {
            self.toast(tr("Ghidra is ready. Deep mode uses it from now on."), false);
        }
        if let Some(draft) = waiting {
            self.start_draft(*draft);
        }
    }

    /// The Ghidra download while it runs: (fraction, "Ghidra · 120 of 543 MB").
    fn ghidra_download(&self) -> Option<(Option<f32>, String)> {
        let j = self.jobs.running().find(|j| Some(j.id) == self.ghidra.job)?;
        Some(j.progress().map(|p| (p.fraction, p.message)).unwrap_or((None, tr("Starting…"))))
    }
}

/// Where Clean room can't do what was asked (a look inside the original): why, and the switch.
pub fn deep_needed(app: &mut CraftApp, ui: &mut Ui) {
    widgets::empty_state(
        ui,
        "shield-check",
        "Looking inside needs Deep mode",
        "Clean room never reads the original's code, files or page source. Deep mode reads them with every tool Craft Anything has.",
    );
    ui.vertical_centered(|ui| {
        if widgets::primary_icon(ui, Some("microscope"), "Turn on Deep mode", true).clicked() {
            // Closes the dialog this is in, if it is in one; Deep mode may offer Ghidra next.
            app.state.modal = None;
            app.set_deep(true);
        }
    });
}

/// [`deep_needed`] as a dialog, for a look inside asked for anywhere else.
pub fn deep_modal(app: &mut CraftApp, ui: &mut Ui, close: &mut bool) {
    deep_needed(app, ui);
    ui.add_space(4.0);
    ui.vertical_centered(|ui| {
        if widgets::secondary(ui, None, "Not now").clicked() {
            *close = true;
        }
    });
}

/// Asked when Deep mode could use Ghidra and it isn't here: get it (then use it), or go on without.
pub fn ghidra_modal(app: &mut CraftApp, ui: &mut Ui, close: &mut bool) {
    let t = Tokens::get(ui.ctx());
    ui.set_width(520.0);
    let ready = app.ghidra_ready();
    let downloading = app.ghidra_download();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, t.radius, t.accent_soft);
        icons::paint(ui, rect, if ready { "check" } else { "microscope" }, 24.0, t.accent_ink);
        ui.vertical(|ui| {
            let title = if ready {
                "Ghidra is ready"
            } else if downloading.is_some() {
                "Getting Ghidra"
            } else {
                "Get Ghidra for Deep mode"
            };
            ui.label(theme::heading(tr(title), theme::SECTION));
            ui.label(
                RichText::new(tr("Ghidra, a free reverse-engineering tool from the NSA, gives Deep mode richer decompiled code and types.")).color(t.text_dim),
            );
        });
    });
    ui.add_space(12.0);
    let body = |ui: &mut Ui, text: String| ui.add(egui::Label::new(RichText::new(text).size(theme::BODY)).wrap());
    let then = if app.ghidra.start_after.is_some() { tr("Your project starts as soon as it's done.") } else { String::new() };
    if ready {
        body(ui, tr("Deep mode now decompiles with Ghidra. The first look at a large program can take several minutes."));
    } else if let Some((fraction, message)) = &downloading {
        progress_bar(ui, *fraction, message);
        ui.add_space(6.0);
        body(
            ui,
            format!("{} {then}", tr("This keeps going in the background, and each download is checked against its publisher's checksum before it's used.")),
        );
    } else if get::can_install() {
        body(
            ui,
            trf(
                "It isn't on this computer yet. Craft Anything can download Ghidra {version} and the Java runtime it needs (about {mb} MB), check both against their publishers' checksums, and set them up in its own folder. Nothing else on your computer changes.",
                &[("version", get::VERSION), ("mb", &get::DOWNLOAD_MB.to_string())],
            ),
        );
    } else {
        body(ui, trf("It isn't on this computer yet. Install Ghidra {version} from its release page, then choose its folder.", &[("version", get::VERSION)]));
        if widgets::link(ui, "Open Ghidra's release page").clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(get::PAGE));
        }
    }
    if !ready && downloading.is_none() {
        body(ui, tr("Without it, Deep mode uses Craft Anything's own decompiler, which is rougher."));
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        if ready {
            let start = app.ghidra.start_after.is_some();
            if widgets::primary_icon(ui, Some(if start { "sparkles" } else { "check" }), if start { "Start building" } else { "Done" }, true).clicked() {
                *close = true;
                if let Some(draft) = app.ghidra.start_after.take() {
                    app.start_draft(*draft);
                }
            }
        } else if downloading.is_some() {
            if widgets::secondary(ui, None, "Close").on_hover_text(tr("The download keeps going in the background.")).clicked() {
                *close = true;
            }
        } else {
            if get::can_install() && widgets::primary_icon(ui, Some("download"), "Download and use Ghidra", true).clicked() {
                app.get_ghidra();
            }
            if widgets::secondary(ui, Some("folder-open"), "I have Ghidra").on_hover_text(tr("Choose the folder Ghidra is installed in.")).clicked() {
                app.pick(crate::Pick::GhidraDir);
            }
            let without = if app.ghidra.start_after.is_some() { "Build without it" } else { "Not now" };
            if widgets::secondary(ui, None, without).clicked() {
                *close = true;
                if let Some(mut draft) = app.ghidra.start_after.take() {
                    draft.skip_ghidra = true;
                    app.start_draft(*draft);
                }
            }
        }
    });
}

/// The memory the person set for Ghidra (GB), or `None` for the automatic amount.
pub fn ghidra_memory(app: &CraftApp) -> Option<u64> {
    app.settings.engines.ghidra.memory_gb.map(|g| g.clamp(*ghidra::HEAP_GB.start(), *ghidra::HEAP_GB.end()))
}

/// "Ghidra will use …": the automatic amount in words, or the set one.
pub fn ghidra_memory_words(app: &CraftApp) -> String {
    match ghidra_memory(app) {
        Some(gb) => trf("Ghidra will use up to {gb} GB of memory.", &[("gb", &gb.to_string())]),
        None => trf("Ghidra will use half of this computer's memory (up to {gb} GB).", &[("gb", &ghidra::HEAP_GB.end().to_string())]),
    }
}

/// "Rough guide: 4 GB for programs up to 50 MB, 8 GB up to 200 MB, …", from the engine's table.
pub fn ghidra_memory_guide() -> String {
    let size = |mb: u64| if mb >= 1024 { format!("{} GB", mb / 1024) } else { format!("{mb} MB") };
    let mut parts: Vec<String> = ghidra::HEAP_FOR_PROGRAM
        .iter()
        .enumerate()
        .map(|(i, (mb, gb))| {
            let space = size(*mb);
            if i == 0 {
                trf("{gb} GB for programs up to {size}", &[("gb", &gb.to_string()), ("size", &space)])
            } else {
                trf("{gb} GB up to {size}", &[("gb", &gb.to_string()), ("size", &space)])
            }
        })
        .collect();
    parts.push(trf("{gb} GB for bigger ones", &[("gb", &ghidra::BIG_PROGRAM_GB.to_string())]));
    format!("{} {}.", tr("Rough guide:"), parts.join(", "))
}

/// Settings → Engines: Automatic (half this computer's memory) or a set amount for Ghidra.
pub fn ghidra_memory_setting(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let set = ghidra_memory(app);
    ui.vertical(|ui| {
        if let Some(i) = widgets::segmented(ui, &["Automatic", "Custom"], usize::from(set.is_some()), 30.0) {
            // Custom starts from what a program of a few hundred MB wants.
            app.settings.engines.ghidra.memory_gb = (i == 1).then(|| ghidra::suggested_heap_gb(500 << 20));
            app.settings_changed();
        }
        ui.add_space(6.0);
        match set {
            None => {
                let line = trf("Half of this computer's memory, up to {gb} GB.", &[("gb", &ghidra::HEAP_GB.end().to_string())]);
                ui.label(RichText::new(line).color(t.text_faint).size(theme::CAPTION));
            }
            Some(mut gb) => {
                // A visible rail on the card, filled in the app colour up to the knob.
                let r = ui
                    .scope(|ui| {
                        ui.spacing_mut().slider_width = 180.0;
                        ui.visuals_mut().widgets.inactive.bg_fill = t.field_border;
                        ui.visuals_mut().selection.bg_fill = t.accent;
                        ui.add(egui::Slider::new(&mut gb, ghidra::HEAP_GB).suffix(" GB").trailing_fill(true))
                    })
                    .inner;
                let r = widgets::named(r, "Memory for Ghidra");
                if r.changed() {
                    app.settings.engines.ghidra.memory_gb = Some(gb);
                }
                // Saved once the slider is let go, not on every step of a drag.
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    app.settings_changed();
                }
                ui.label(RichText::new(tr("Leave a few GB for the rest of your computer.")).color(t.text_faint).size(theme::CAPTION));
            }
        }
    });
}

/// Settings → Engines: whether Deep mode can use Ghidra, and the way to get it.
pub fn ghidra_status(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    if let Some((fraction, message)) = app.ghidra_download() {
        progress_bar(ui, fraction, &message);
    } else if app.ghidra_ready() {
        let at = app.settings.ghidra_launcher().map(|p| p.display().to_string()).unwrap_or_default();
        widgets::pill(ui, "Installed", t.success, t.raised).on_hover_text(at);
    } else {
        ui.horizontal(|ui| {
            widgets::pill(ui, "Not installed", t.warning, t.raised);
            if widgets::secondary(ui, Some("download"), "Get Ghidra").clicked() {
                app.state.modal = Some(Modal::Ghidra);
            }
        });
    }
}

/// Which of the app's asset providers have a key stored, by id.
fn stored_keys(hub: &craft_anything_ai::AiHub, all: &AssetProviders) -> BTreeMap<String, bool> {
    all.ids().into_iter().map(|id| (id.to_string(), hub.has_asset_key(all, id))).collect()
}

impl CraftApp {
    /// Find out which asset providers have a key set up (in the background: the keychain can be slow).
    pub fn check_asset_keys(&mut self) {
        if self.headless {
            return;
        }
        let (hub, all) = (self.hub.clone(), self.asset_providers.clone());
        self.jobs.spawn("Check the asset keys", false, move |_| Ok(Done::AssetKeys(stored_keys(&hub, &all))));
    }

    /// Whether any asset provider has a key. Each tool checks the key for its chosen model.
    pub fn has_asset_key(&self) -> Option<bool> {
        self.asset_keys.as_ref().map(|keys| keys.values().any(|ready| *ready))
    }

    /// Save an asset provider's key in the system keychain.
    pub fn save_asset_key(&mut self, provider: String, key: String) {
        let (hub, all) = (self.hub.clone(), self.asset_providers.clone());
        self.jobs.spawn(tr("Save the key"), true, move |_| {
            hub.set_asset_key(&provider, &key).map_err(|e| e.message)?;
            Ok(Done::AssetKeys(stored_keys(&hub, &all)))
        });
    }

    pub fn remove_asset_key(&mut self, provider: String) {
        let (hub, all) = (self.hub.clone(), self.asset_providers.clone());
        self.jobs.spawn(tr("Forget the key"), true, move |_| {
            hub.remove_asset_key(&provider).map_err(|e| e.message)?;
            Ok(Done::AssetKeys(stored_keys(&hub, &all)))
        });
    }

    /// Ask the providers, in the background, which models they offer for `kind` and what each costs:
    /// what `assets_models_list` does, through the same function, which keeps the lists on disk and
    /// uses them again while they are young; `force` asks every provider again. It can take minutes,
    /// so it doesn't go through the engine: the providers, keys and config folder are read under one
    /// short hold of the session, and the engine stays free for other tools meanwhile. It is the only
    /// way the pickers get their lists: drawing never asks.
    pub fn refresh_asset_models(&mut self, kind: AssetKind, force: bool) {
        // An engine without the AI layer's tools (the demo's) has nobody to ask.
        if self.engine.registry().get("assets_models_list").is_none() {
            return;
        }
        self.asset_models.entry(kind).or_default().loading = true;
        let engine = self.engine.clone();
        let job = self.jobs.spawn(tr("Ask the providers for their models"), true, move |j| {
            let progress = j.reporter();
            let report = |message: String| progress(craft_anything_engine::Progress { fraction: None, message });
            let sources = craft_anything_ai::asset_models::ListSources::of(&engine.session());
            let result = craft_anything_ai::asset_models::list_models(&sources, kind, force, None, &j.cancel, &report)
                .and_then(|listed| ModelsView::from_result(&listed, craft_anything_engine::inbox::now_ms()).map_err(ToolError::internal));
            Ok(Done::AssetModels { kind, result })
        });
        // A request this one replaces is stopped, not left running for nobody: its answer would be
        // dropped anyway.
        if let Some(old) = self.asset_model_jobs.insert(kind, job) {
            self.jobs.cancel(old);
        }
    }

    /// Ask again for the lists of every type that was never read or was read more than ten minutes
    /// ago (when Settings → Builds opens). A request already running is left to finish.
    pub fn refresh_stale_asset_models(&mut self) {
        for kind in AssetKind::ALL {
            self.refresh_asset_models_if_stale(kind);
        }
    }

    /// Ask again for `kind`'s lists if they were never read or were read more than ten minutes ago
    /// (when its picker opens). A request already running is left to finish.
    pub fn refresh_asset_models_if_stale(&mut self, kind: AssetKind) {
        let now = craft_anything_engine::inbox::now_ms();
        if self.asset_models.get(&kind).is_none_or(|v| !v.loading && v.is_stale(now)) {
            self.refresh_asset_models(kind, false);
        }
    }

    /// A request for `kind`'s lists finished. An answer to a request that a newer one has replaced is
    /// dropped. A request that failed (or was stopped) leaves the lists shown as they were.
    pub(crate) fn asset_models_listed(&mut self, job: u64, kind: AssetKind, result: Result<ModelsView, ToolError>) {
        if self.asset_model_jobs.get(&kind) != Some(&job) {
            return;
        }
        self.asset_model_jobs.remove(&kind);
        match result {
            Ok(view) => {
                self.asset_models.insert(kind, view);
            }
            Err(e) => {
                if let Some(view) = self.asset_models.get_mut(&kind) {
                    view.loading = false;
                }
                if e.code != ErrorCode::Cancelled {
                    self.log(true, e.message.clone());
                    self.toast(e.for_people(), true);
                }
            }
        }
    }

    /// Let the builder generate assets, or not: for the open project on its screen
    /// (through the project_assets_allow tool), else for the game being set up on Make. Turning it
    /// on without a provider key asks for one.
    pub fn set_generate_assets(&mut self, on: bool) {
        let project = self.workspace.as_ref().filter(|_| self.state.screen == Screen::Project).map(|w| w.root.clone());
        match project {
            Some(root) => {
                let title = tr(if on { "Allow generated assets" } else { "Stop generated assets" });
                self.act("project_assets_allow", serde_json::json!({"project": root, "allow": on}), title);
            }
            None => self.state.make.generate_assets = on,
        }
        if on && self.has_asset_key() == Some(false) {
            self.state.modal = Some(Modal::Assets);
        }
    }
}

/// A provider's key field and its Save button; returns the key when Save is pressed.
fn key_field(app: &mut CraftApp, ui: &mut Ui, provider: &dyn AssetProvider, save: &str) -> Option<String> {
    let t = Tokens::get(ui.ctx());
    let secret_id = format!("{}:secret", provider.id());
    let secret_label = provider.key_secret_label();
    let fields = std::iter::once((provider.id(), provider.key_hint(), "{name} key", "Paste your {name} key"))
        .chain(secret_label.map(|label| (secret_id.as_str(), label, "{name} API secret", "Paste your {name} API secret")));
    for (id, label, name, hint) in fields {
        ui.label(RichText::new(tr(label)).size(theme::CAPTION).color(t.text_dim));
        let r = ui.add(
            egui::TextEdit::singleline(app.key_inputs.entry(id.into()).or_default())
                .id_salt(id)
                .password(true)
                .hint_text(trf(hint, &[("name", provider.name())]))
                .desired_width(320.0)
                .frame(widgets::text_field_frame_compact(&t)),
        );
        widgets::named(r, &trf(name, &[("name", provider.name())]));
    }
    let key = app.key_inputs.get(provider.id()).map(|s| s.trim()).unwrap_or_default();
    let secret = app.key_inputs.get(&secret_id).map(|s| s.trim()).unwrap_or_default();
    let ready = !key.is_empty() && (secret_label.is_none() || !secret.is_empty());
    if widgets::primary_icon(ui, None, save, ready).clicked() && ready {
        let credential = if secret_label.is_some() { format!("{key}:{secret}") } else { key.to_string() };
        // Check it as generation will, so a credential it would refuse keeps its fields and says why.
        if let Err(e) = provider.validate_key(&credential) {
            app.toast(e.for_people(), true);
            return None;
        }
        app.key_inputs.remove(provider.id());
        app.key_inputs.remove(&secret_id);
        Some(credential)
    } else {
        None
    }
}

/// The game's "Generate custom pictures and models" switch and what they cost. It works while the
/// project builds: a run keeps the person's choice when it saves, and the asset tools read it each time.
pub fn assets_toggle(app: &mut CraftApp, ui: &mut Ui, mut on: bool) {
    let t = Tokens::get(ui.ctx());
    if widgets::toggle(ui, &mut on, "Generate pictures, models and audio").changed() {
        app.set_generate_assets(on);
    }
    let no_key = on && app.has_asset_key() == Some(false);
    let note = if no_key {
        tr("Needs an asset provider key. Until then the game is built without generated assets.")
    } else {
        tr("Paid from your provider accounts, at the prices each provider gives for the model you choose. Off, the game is built without generated assets.")
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(note).color(if no_key { t.warning } else { t.text_dim }).size(theme::SMALL));
        if no_key && widgets::link(ui, "Add a provider key").clicked() {
            app.state.modal = Some(Modal::Assets);
        }
    });
}

/// Asks for provider credentials when generated assets are turned on without them.
pub fn assets_modal(app: &mut CraftApp, ui: &mut Ui, close: &mut bool) {
    let t = Tokens::get(ui.ctx());
    ui.set_width(520.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, t.radius, t.accent_soft);
        icons::paint(ui, rect, "boxes", 24.0, t.accent_ink);
        ui.vertical(|ui| {
            ui.label(theme::heading(tr("Your asset provider keys"), theme::SECTION));
            ui.label(RichText::new(tr("Use fal.ai, Scenario or ElevenLabs for generated assets.")).color(t.text_dim));
        });
    });
    ui.add_space(12.0);
    ui.add(
        egui::Label::new(
            RichText::new(tr(
                "Generate original pictures and models, speech, sound effects and music with your own provider accounts. Prices come from the providers, and each type of asset has its own limit. Keys use the system keychain when available, otherwise a private local key file. Choose providers and models in Settings → Builds.",
            ))
            .size(theme::BODY),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    let all = app.asset_providers.clone();
    for provider in all.ids().into_iter().filter_map(|id| all.get(id)) {
        ui.push_id(provider.id(), |ui| {
            ui.label(RichText::new(provider.name()).size(theme::SMALL));
            keys_link(ui, provider.as_ref());
            if let Some(key) = key_field(app, ui, provider.as_ref(), "Save") {
                *close = true;
                app.save_asset_key(provider.id().into(), key);
            }
        });
        ui.add_space(8.0);
    }
    ui.add_space(14.0);
    if widgets::secondary(ui, None, "Not now").clicked() {
        *close = true;
        // On a project this dialog manages credentials; closing it must preserve the
        // person's independent provider choices. Make still cancels its initial opt-in.
        if app.state.screen != Screen::Project {
            app.set_generate_assets(false);
        }
    }
}

/// "Get a key at …" for a provider that says where.
fn keys_link(ui: &mut Ui, provider: &dyn AssetProvider) {
    if let Some(page) = provider.keys_page()
        && widgets::link(ui, &trf("Get a key at {name}", &[("name", provider.name())])).clicked()
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(page));
    }
}

/// Settings → Builds: each asset provider's key, and whether one is saved. A provider with a saved
/// key is listed in the pickers; Remove forgets it.
pub fn assets_settings(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    widgets::caption(ui, "Asset provider keys");
    ui.add(
        egui::Label::new(
            RichText::new(tr(
                "Pictures, 3D models, speech, sound effects and music are made by the providers you save a key for. Keys use the system keychain when available, otherwise a private local key file.",
            ))
            .color(t.text_faint)
            .size(theme::CAPTION),
        )
        .wrap(),
    );
    let all = app.asset_providers.clone();
    for provider in all.ids().into_iter().filter_map(|id| all.get(id)) {
        ui.add_space(6.0);
        // Known once the keys were looked up; a provider the lookup doesn't list has none.
        let stored = app.asset_keys.as_ref().map(|k| k.get(provider.id()).copied().unwrap_or(false));
        let open_id = ui.make_persistent_id(("asset-key-open", provider.id()));
        let open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false) && stored == Some(false);
        let mut toggle_open = false;
        ui.horizontal(|ui| {
            // Room for the buttons, so the name is centred beside them.
            ui.set_min_height(34.0);
            ui.label(RichText::new(provider.name()).font(theme::medium(theme::SMALL)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match stored {
                Some(true) => {
                    let remove = widgets::secondary(ui, None, "Remove");
                    if widgets::named(remove, &trf("Remove the {name} key", &[("name", provider.name())])).clicked() {
                        app.remove_asset_key(provider.id().into());
                    }
                    widgets::pill(ui, "Key saved", t.success, t.raised);
                }
                Some(false) => {
                    let add = widgets::secondary(ui, None, "Add a key");
                    toggle_open = widgets::named(add, &trf("Add a key for {name}", &[("name", provider.name())])).clicked();
                    widgets::pill(ui, "Key needed", t.warning, t.raised);
                }
                None => {
                    ui.label(RichText::new(tr("Checking for a saved key…")).color(t.text_faint).size(theme::CAPTION));
                }
            });
        });
        if toggle_open {
            ui.data_mut(|d| d.insert_temp(open_id, !open));
        }
        if open {
            ui.push_id(provider.id(), |ui| {
                if let Some(key) = key_field(app, ui, provider.as_ref(), "Save") {
                    ui.data_mut(|d| d.insert_temp(open_id, false));
                    app.save_asset_key(provider.id().into(), key);
                }
                keys_link(ui, provider.as_ref());
            });
        }
    }
}

/// The engine to build this draft with: the person's pick, else the one their own files use, else
/// the one the request names, else none when the request names another way to build ("…in
/// Electron"), else the original's own or Unity.
pub fn chosen(kinds: &KindRegistry, d: &MakeDraft) -> Option<Builder> {
    if d.builder.is_some() {
        return d.builder;
    }
    if let Some(stack) = crate::make::own_stack(kinds, d) {
        return Builder::from_stack(stack);
    }
    match kinds.named(&d.request, &crate::make::resolve_target(d)) {
        Some(stack) => Builder::from_stack(stack),
        None => Some(Builder::for_game(d.found.as_ref().map(|f| f.engine))),
    }
}

/// "Build with" on Make: what made the original, the engine we'll use, and whether it's here.
pub fn build_with(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let b = chosen(&app.kinds, &app.state.make);
    if let Some(f) = app.state.make.found.clone() {
        ui.horizontal_wrapped(|ui| {
            icons::show(ui, "puzzle", 16.0, t.accent_ink);
            ui.label(RichText::new(trf("Made with {engine}", &[("engine", &f.label())])).font(theme::semibold(theme::BODY)));
        });
        if !matches!(f.engine, game::Made::Unity | game::Made::Unreal | game::Made::Godot) {
            ui.label(
                RichText::new(trf("Craft Anything can't build with {engine}, so Unity is the closest match.", &[("engine", f.engine.name())]))
                    .color(t.text_dim)
                    .size(theme::SMALL),
            );
        }
        ui.add_space(8.0);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("Build with")).size(theme::BODY).color(t.text_dim));
        let names: Vec<&str> = Builder::ALL.iter().map(|b| b.name()).collect();
        // Nothing is selected while the request names another way to build.
        let sel = b.and_then(|b| Builder::ALL.iter().position(|x| *x == b)).unwrap_or(usize::MAX);
        if let Some(i) = widgets::segmented(ui, &names, sel, 32.0) {
            app.state.make.builder = Builder::ALL.get(i).copied();
        }
    });
    ui.add_space(6.0);
    match b {
        Some(b) => status_line(app, ui, b, true),
        None => {
            let named = app.kinds.named(&app.state.make.request, &crate::make::resolve_target(&app.state.make)).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                icons::show(ui, "check", 16.0, t.success);
                ui.label(
                    RichText::new(trf("Built with {stack}, as you asked, so no game engine is needed.", &[("stack", named)]))
                        .color(t.text_dim)
                        .size(theme::SMALL),
                );
            });
        }
    }
}

// `xtask i18n-coverage` only scans direct `tr("…")` literals. These keys are
// translated by `status_line`/`step`, so keep the TSV catalogs in sync with them.
/// What the status line says when Unreal is installed but its C++ tools are missing.
fn unreal_tools_missing(os: &str) -> &'static str {
    match os {
        "linux" => "Unreal is here, but its bundled Clang C++ toolchain wasn't found.",
        "macos" => "Unreal is here, but it also needs Xcode.",
        _ => "Unreal is here, but it also needs Visual Studio's C++ tools.",
    }
}

/// The C++ toolchain step for Unreal: title, detail, and the page to open (if any).
fn cpp_step(os: &str) -> (&'static str, &'static str, Option<&'static str>) {
    match os {
        "linux" => (
            "Add Unreal's Linux C++ toolchain",
            "Run Engine/Build/BatchFiles/Linux/SetupToolchain.sh in the engine folder to install its bundled Clang toolchain. If you built Unreal from source, Setup.sh downloads the native toolchain instead. A system Clang may not match the engine's version.",
            None,
        ),
        "macos" => ("Install Xcode", "Unreal builds games with Apple's Xcode, free in the App Store.", None),
        _ => {
            ("Add Visual Studio's C++ tools", "Install Visual Studio Community (free) and tick \"Game development with C++\".", Some(game::VISUAL_STUDIO_PAGE))
        }
    }
}

/// "Ready — Unity 6000.0.23f1" or "Not on this computer yet" with the way to get it.
fn status_line(app: &mut CraftApp, ui: &mut Ui, b: Builder, offer: bool) {
    let t = Tokens::get(ui.ctx());
    let inv = app.game_engines.clone();
    ui.horizontal_wrapped(|ui| match &inv {
        None => {
            ui.add(crate::widgets::Spinner::new().size(14.0).color(t.accent));
            ui.label(RichText::new(tr("Looking for game engines on this computer…")).color(t.text_dim).size(theme::SMALL));
        }
        Some(inv) if inv.ready(b) => {
            icons::show(ui, "check", 16.0, t.success);
            let v = inv.best(b).map(|i| i.version.clone()).unwrap_or_default();
            let what = if v.is_empty() { b.name().to_string() } else { format!("{} {v}", b.name()) };
            ui.label(RichText::new(trf("Ready: {what} is on this computer.", &[("what", &what)])).color(t.text_dim).size(theme::SMALL));
        }
        Some(inv) => {
            icons::show(ui, "download", 16.0, t.warning);
            let text = if b == Builder::Unreal && inv.best(b).is_some() {
                tr(unreal_tools_missing(std::env::consts::OS))
            } else {
                format!("{} {}", trf("{name} isn't on this computer yet.", &[("name", b.name())]), tr(b.needs()))
            };
            ui.label(RichText::new(text).color(t.text_dim).size(theme::SMALL));
            if offer && widgets::secondary(ui, Some("download"), &trf("Get {name}", &[("name", b.name())])).clicked() {
                // One dialog at a time: the steps replace Settings when opened from there.
                app.state.settings_open = false;
                app.state.modal = Some(Modal::Engine(b));
            }
        }
    });
}

/// One step of getting an engine.
struct Step {
    title: String,
    detail: String,
    done: bool,
    action: Option<Action>,
}

enum Action {
    Download,
    OpenApp(PathBuf),
    OpenPage(&'static str),
}

fn steps(app: &CraftApp, b: Builder, inv: &Inventory) -> Vec<Step> {
    let opened = app.engine_flow.installer_opened.contains(&b);
    let direct = game::installer(b).is_some();
    let step = |title: &str, detail: &str, done: bool, action: Option<Action>| Step { title: tr(title), detail: tr(detail), done, action };
    match b {
        Builder::Godot => vec![step(
            &trf("Download Godot {v}", &[("v", game::GODOT_VERSION.trim_end_matches("-stable"))]),
            "Checked against Godot's official checksums before it's used.",
            inv.ready(b),
            Some(Action::Download),
        )],
        Builder::Rust => {
            vec![step("Install Rust", "Follow the steps on rustup.rs, then come back here.", inv.ready(b), Some(Action::OpenPage(game::download_page(b))))]
        }
        Builder::Unity => {
            let hub = inv.unity_hub.clone();
            let mut v = Vec::new();
            if direct {
                v.push(step(
                    "Download Unity Hub",
                    "Unity's own installer. It only opens if Unity's signature checks out.",
                    hub.is_some() || opened,
                    Some(Action::Download),
                ));
                v.push(step("Install Unity Hub", "Follow the installer that opened. Come back here when it's done.", hub.is_some(), None));
            } else {
                v.push(step("Get Unity Hub", "Download it from unity.com and install it.", hub.is_some(), Some(Action::OpenPage(game::download_page(b)))));
            }
            v.push(step(
                "Sign in and add the Unity Editor",
                "In Unity Hub: sign in (or make a free account) and accept the free Personal licence. Then open Installs, press Install Editor and choose the recommended version. About 10 GB.",
                inv.best(b).is_some(),
                hub.map(Action::OpenApp),
            ));
            v
        }
        Builder::Unreal => {
            let launcher = inv.epic_launcher.clone();
            let mut v = Vec::new();
            if direct {
                v.push(step(
                    "Download the Epic Games Launcher",
                    "Epic's own installer. It only opens if Epic's signature checks out.",
                    launcher.is_some() || opened,
                    Some(Action::Download),
                ));
                v.push(step("Install the launcher", "Follow the installer that opened. Come back here when it's done.", launcher.is_some(), None));
            } else {
                v.push(step(
                    "Get the Epic Games Launcher",
                    "Download it from unrealengine.com and install it.",
                    launcher.is_some(),
                    Some(Action::OpenPage(game::download_page(b))),
                ));
            }
            v.push(step(
                "Sign in and install Unreal Engine",
                "In the launcher: sign in, open Unreal Engine, then Library, and press + next to Engine Versions to install one. About 60 GB.",
                inv.best(b).is_some(),
                launcher.map(Action::OpenApp),
            ));
            let (title, detail, page) = cpp_step(std::env::consts::OS);
            v.push(step(title, detail, inv.cpp_tools, page.map(Action::OpenPage)));
            v
        }
    }
}

/// The body of the "Get <engine>" dialog. Sets `close` when the person is done with it.
pub fn modal(app: &mut CraftApp, ui: &mut Ui, b: Builder, close: &mut bool) {
    let t = Tokens::get(ui.ctx());
    ui.set_width(520.0);
    let Some(inv) = app.game_engines.clone() else {
        ui.horizontal(|ui| {
            ui.add(crate::widgets::Spinner::new().size(16.0).color(t.accent));
            ui.label(tr("Looking for game engines on this computer…"));
        });
        app.refresh_game_engines();
        return;
    };
    let ready = inv.ready(b);
    // Keep looking while the person installs things in other windows.
    if !ready && !app.engine_flow.checking && app.engine_flow.checked_at.is_none_or(|at| at.elapsed() > Duration::from_secs(3)) {
        app.refresh_game_engines();
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, t.radius, t.accent_soft);
        icons::paint(ui, rect, if ready { "check" } else { "download" }, 24.0, t.accent_ink);
        ui.vertical(|ui| {
            let title = if ready { trf("{name} is ready", &[("name", b.name())]) } else { trf("Get {name}", &[("name", b.name())]) };
            ui.label(theme::heading(title, theme::SECTION));
            ui.label(RichText::new(tr(b.needs())).color(t.text_dim));
        });
    });
    ui.add_space(16.0);

    let list = steps(app, b, &inv);
    let current = list.iter().position(|s| !s.done);
    let mut act = None;
    for (i, s) in list.iter().enumerate() {
        let now = Some(i) == current;
        ui.horizontal_top(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
            if s.done {
                ui.painter().rect_filled(r, r.width() / 2.0, t.success);
                icons::paint(ui, r, "check", 16.0, t.canvas);
            } else {
                ui.painter().rect_stroke(r, r.width() / 2.0, egui::Stroke::new(1.0, if now { t.accent } else { t.field_border }), egui::StrokeKind::Inside);
                let g = widgets::label_galley(ui, &(i + 1).to_string(), 12.0, t.text);
                ui.painter().galley_with_override_text_color(r.center() - g.size() / 2.0, g, if now { t.text } else { t.text_faint });
            }
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&s.title).font(theme::semibold(theme::BODY)).color(if s.done || now { t.text } else { t.text_dim }));
                if now || !s.done {
                    ui.add(egui::Label::new(RichText::new(&s.detail).color(t.text_dim).size(theme::SMALL)).wrap());
                }
                if now {
                    ui.add_space(8.0);
                    if let Some(a) = &s.action {
                        let downloading = app.engine_download(b);
                        match (a, downloading) {
                            (Action::Download, Some((fraction, message))) => progress_bar(ui, fraction, &message),
                            (Action::Download, None) => {
                                if widgets::primary_icon(ui, Some("download"), &tr("Download"), app.engine_flow.job.is_none()).clicked() {
                                    act = Some(Action::Download);
                                }
                            }
                            (Action::OpenApp(p), _) => {
                                let label = if b == Builder::Unity { "Open Unity Hub" } else { "Open the Epic Games Launcher" };
                                if widgets::primary_icon(ui, Some("external-link"), label, true).clicked() {
                                    act = Some(Action::OpenApp(p.clone()));
                                }
                            }
                            (Action::OpenPage(url), _) => {
                                if widgets::primary_icon(ui, Some("external-link"), "Open the download page", true).clicked() {
                                    act = Some(Action::OpenPage(url));
                                }
                            }
                        }
                        ui.add_space(4.0);
                    }
                    if !s.done && current.is_some() && i > 0 {
                        ui.label(RichText::new(tr("This updates by itself when it's done.")).color(t.text_faint).size(theme::SMALL));
                    }
                }
            });
        });
        ui.add_space(12.0);
    }

    match act {
        Some(Action::Download) => app.get_engine(b, ui.ctx()),
        Some(Action::OpenApp(_)) => open_app(app, b),
        Some(Action::OpenPage(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
        None => {}
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ready {
            let label = if app.engine_flow.start_after { "Start building" } else { "Done" };
            if widgets::primary_icon(ui, Some(if app.engine_flow.start_after { "sparkles" } else { "check" }), label, true).clicked() {
                *close = true;
                app.state.make.builder = Some(b);
                if std::mem::take(&mut app.engine_flow.start_after) {
                    app.start_make();
                }
            }
        } else {
            // Another engine that's ready now, best first.
            if let Some(alt) = Builder::ALL.into_iter().find(|x| *x != b && inv.ready(*x))
                && widgets::secondary(ui, None, &trf("Build with {name} instead", &[("name", alt.name())])).clicked()
            {
                *close = true;
                app.state.make.builder = Some(alt);
                if std::mem::take(&mut app.engine_flow.start_after) {
                    app.start_make();
                }
            }
            if widgets::secondary(ui, None, "Close").on_hover_text(tr("Downloads keep going in the background.")).clicked() {
                *close = true;
                app.engine_flow.start_after = false;
            }
        }
    });
}

/// A labelled progress bar (indeterminate when the size isn't known).
pub fn progress_bar(ui: &mut Ui, fraction: Option<f32>, message: &str) {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width().min(420.0);
    let (bar, _) = ui.allocate_exact_size(Vec2::new(w, 6.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 3.0, t.separator);
    let (from, to) = match fraction {
        Some(f) => (0.0, f.clamp(0.0, 1.0)),
        None => {
            let x = (ui.input(|i| i.time) * 0.6).fract() as f32;
            ((x - 0.25).max(0.0), x)
        }
    };
    ui.painter().rect_filled(egui::Rect::from_x_y_ranges(bar.left() + bar.width() * from..=bar.left() + bar.width() * to, bar.y_range()), 3.0, t.accent);
    ui.add_space(4.0);
    let pct = fraction.map(|f| format!("{:.0}%  ·  ", f * 100.0)).unwrap_or_default();
    ui.label(RichText::new(format!("{pct}{message}")).font(theme::mono(12.5)).color(t.text_dim));
    widgets::animate(ui, Duration::from_millis(100));
}

/// Settings → Engines: each game engine, whether it's here, and the way to get it.
pub fn settings_section(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    if app.game_engines.is_none() {
        app.refresh_game_engines();
    }
    ui.label(RichText::new(tr("Game engines")).font(theme::medium(theme::SMALL)));
    ui.add(
        egui::Label::new(
            RichText::new(tr(
                "Recreated games are built with the original's own engine when it's here (Unity, Unreal or Godot), else Unity, and Rust as the last resort.",
            ))
            .color(t.text_faint)
            .size(theme::CAPTION),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    for b in Builder::ALL {
        ui.label(RichText::new(b.name()).font(theme::semibold(theme::SMALL)));
        status_line(app, ui, b, true);
        ui.add_space(8.0);
    }
}

/// The download running anywhere in the app, for the title bar: (title, fraction, message).
pub fn global_download(app: &CraftApp) -> Option<(String, Option<f32>, String)> {
    let id = app.engine_flow.job.map(|(_, id)| id).or(app.ghidra.job)?;
    let j = app.jobs.running().find(|j| j.id == id)?;
    let p = j.progress();
    Some((j.title.clone(), p.as_ref().and_then(|p| p.fraction), p.map(|p| p.message).unwrap_or_default()))
}

/// Unity Hub or the Epic Games Launcher, through the engines_app_open tool.
fn open_app(app: &mut CraftApp, b: Builder) {
    let which = if b == Builder::Unity { "unity_hub" } else { "epic_launcher" };
    app.act("engines_app_open", serde_json::json!({"app": which}), tr("Open the installer app"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_unreal_guidance_names_the_bundled_clang() {
        let (title, detail, page) = cpp_step("linux");
        assert!(title.contains("Linux"));
        assert!(detail.contains("Engine/Build/BatchFiles/Linux/SetupToolchain.sh"));
        assert!(detail.contains("bundled Clang"));
        assert!(detail.contains("native toolchain"), "Setup.sh must be described as the source-build path");
        assert!(!detail.contains("Visual Studio"), "Linux must not send people to Visual Studio");
        assert_eq!(page, None, "Linux must not open the Visual Studio download page");
        let status = unreal_tools_missing("linux");
        assert!(status.contains("bundled Clang"));
        assert!(!status.contains("Visual Studio"));
    }

    #[test]
    fn other_platforms_keep_their_unreal_guidance() {
        let (mac_title, _mac_detail, mac_page) = cpp_step("macos");
        assert_eq!(mac_title, "Install Xcode");
        assert_eq!(mac_page, None);
        assert_eq!(unreal_tools_missing("macos"), "Unreal is here, but it also needs Xcode.");
        let (win_title, _win_detail, win_page) = cpp_step("windows");
        assert_eq!(win_title, "Add Visual Studio's C++ tools");
        assert_eq!(win_page, Some(game::VISUAL_STUDIO_PAGE));
        assert!(unreal_tools_missing("windows").contains("Visual Studio"));
    }

    #[test]
    fn the_request_picks_the_engine_or_none() {
        let kinds = KindRegistry::built_in();
        let draft = |r: &str| MakeDraft { request: r.into(), ..Default::default() };
        assert_eq!(chosen(&kinds, &draft("a racing game")), Some(Builder::Unity));
        assert_eq!(chosen(&kinds, &draft("a racing game in Godot")), Some(Builder::Godot));
        assert_eq!(chosen(&kinds, &draft("a racing game in Electron")), None, "built as asked: no engine needed");
        let mut d = draft("a racing game in Electron");
        d.builder = Some(Builder::Unreal);
        assert_eq!(chosen(&kinds, &d), Some(Builder::Unreal), "the person's pick on Make wins");
    }

    /// A view that was read once, so there is a list to keep when a later request fails.
    fn read_view(loading: bool) -> ModelsView {
        ModelsView { fetched_ms: 5, providers: Vec::new(), loading }
    }

    fn app() -> CraftApp {
        crate::demo::app(crate::demo::engine(craft_anything_engine::Mode::Beginner, true))
    }

    #[test]
    fn an_answer_to_a_replaced_request_is_dropped_and_the_newest_is_shown() {
        let mut app = app();
        app.asset_models.insert(AssetKind::Music, ModelsView { loading: true, ..read_view(true) });
        app.asset_model_jobs.insert(AssetKind::Music, 9);
        // The request before the newest one answers late: nothing changes.
        app.asset_models_listed(8, AssetKind::Music, Ok(ModelsView { fetched_ms: 100, ..read_view(false) }));
        assert_eq!((app.asset_models[&AssetKind::Music].fetched_ms, app.asset_models[&AssetKind::Music].loading), (5, true));
        assert_eq!(app.asset_model_jobs.get(&AssetKind::Music), Some(&9));
        app.asset_models_listed(9, AssetKind::Music, Ok(ModelsView { fetched_ms: 200, ..read_view(false) }));
        assert_eq!((app.asset_models[&AssetKind::Music].fetched_ms, app.asset_models[&AssetKind::Music].loading), (200, false));
        assert!(app.asset_model_jobs.is_empty());
    }

    #[test]
    fn a_failed_request_keeps_the_lists_shown_and_says_why_but_a_stopped_one_is_quiet() {
        let mut app = app();
        app.asset_models.insert(AssetKind::Image, read_view(true));
        app.asset_model_jobs.insert(AssetKind::Image, 3);
        app.asset_models_listed(3, AssetKind::Image, Err(ToolError::new(ErrorCode::EngineMissing, "No answer.")));
        let view = &app.asset_models[&AssetKind::Image];
        assert_eq!((view.fetched_ms, view.loading), (5, false), "the lists from before stay, and nothing is loading");
        assert!(app.state.toasts.iter().any(|(text, error)| *error && text.contains("No answer.")));
        app.state.toasts.clear();
        app.asset_models.entry(AssetKind::Image).or_default().loading = true;
        app.asset_model_jobs.insert(AssetKind::Image, 4);
        app.asset_models_listed(4, AssetKind::Image, Err(ToolError::cancelled()));
        assert!(!app.asset_models[&AssetKind::Image].loading && app.state.toasts.is_empty(), "Stop is not an error");
    }

    #[test]
    fn a_request_whose_worker_died_stops_loading_and_leaves_the_others() {
        let mut app = app();
        for kind in [AssetKind::Image, AssetKind::Speech] {
            app.asset_models.insert(kind, read_view(true));
        }
        app.asset_model_jobs.insert(AssetKind::Image, 11);
        app.asset_model_jobs.insert(AssetKind::Speech, 12);
        app.handle_done(crate::jobs::Finished { job: 11, result: Err("Ask the providers for their models hit a bug and stopped safely.".into()) });
        assert!(!app.asset_models[&AssetKind::Image].loading);
        assert!(app.asset_models[&AssetKind::Speech].loading, "another type's request is still running");
        assert_eq!(app.asset_model_jobs.keys().copied().collect::<Vec<_>>(), [AssetKind::Speech]);
    }
}
