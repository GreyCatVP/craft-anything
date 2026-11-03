//! User settings (`config.toml`). No environment variables are required for anything.

use crate::asset_choice::AssetChoice;
use crate::versioned::{Step, upgrade};
use craft_anything_engines::assets::{
    AssetKind,
    model::{Pricing, Unit},
};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Beginner,
    Experienced,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GhidraSettings {
    /// A Ghidra install to use instead of the managed one.
    pub install_dir: Option<PathBuf>,
    /// A Java runtime to use instead of the managed one.
    pub java_home: Option<PathBuf>,
    /// Seconds to wait for Ghidra to import and analyse a target.
    pub startup_timeout_secs: Option<u64>,
    /// A Ghidra language id to force, e.g. `RISCV:LE:64:RV64GC`, `PowerPC:BE:32:default`,
    /// `6502:LE:16:default`, `MIPS:BE:32:default`, `ARM:LE:32:v7`. Lets Deep mode target
    /// processors our own engine doesn't cover, with no per-architecture code. Only used in Deep
    /// mode with Ghidra installed.
    pub language_id: Option<String>,
    /// A Ghidra compiler-spec id to pair with `language_id` (e.g. `gcc`, `default`, `windows`).
    pub compiler_spec: Option<String>,
    /// Memory for Ghidra in GB (4 to 31). More makes its first look at a big program faster.
    /// `None`: half this computer's memory.
    pub memory_gb: Option<u64>,
}

/// One list for the per-engine path overrides: it declares the `ToolPaths` fields, `override_for`,
/// and the list the Engines settings page shows. The ids and their order are exactly the engines in
/// `craft_anything_engines::ENGINES` that have a path setting (a test checks it).
macro_rules! tool_paths {
    ($($id:ident),* $(,)?) => {
        /// Paths to external tools when the user prefers their own install over the managed one.
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct ToolPaths {
            $(pub $id: Option<PathBuf>,)*
        }

        impl ToolPaths {
            /// Every override as `(engine id, path)`, in settings-page order.
            pub fn entries_mut(&mut self) -> Vec<(&'static str, &mut Option<PathBuf>)> {
                vec![$((stringify!($id), &mut self.$id)),*]
            }
        }

        impl craft_anything_engines::ToolPathLookup for ToolPaths {
            fn override_for(&self, id: &str) -> Option<PathBuf> {
                match id {
                    $(stringify!($id) => self.$id.clone(),)*
                    _ => None,
                }
            }
        }
    };
}

// Ghidra is not here: its folder is `GhidraSettings::install_dir`.
tool_paths!(adb, jadx, apktool, chromium, frida, gdb, lldb, mitmdump, binwalk, unblob, ilspy, java, tshark, objdump, rizin, rtl433, vineflower);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    pub ghidra: GhidraSettings,
    pub tools: ToolPaths,
    /// Folder for managed engine installs; defaults to `<data dir>/engines`.
    pub store_dir: Option<PathBuf>,
    /// URL of a user-run JEB MCP server, e.g. `http://127.0.0.1:8425/mcp` (loopback, plain HTTP only).
    pub jeb_url: Option<String>,
    /// Find engines only where these settings and the managed store say, never on PATH or in the
    /// usual install folders. Never saved: test sessions set it so they don't find what this
    /// computer has installed ([`crate::testing::session`]).
    #[serde(skip)]
    pub only_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The config.toml format version ([`SETTINGS_VERSION`]). Missing means 0: written before versions.
    pub version: u32,
    pub mode: Mode,
    /// Deep mode: tools may look inside the original (its code, files and page source), with our
    /// own engines and with Ghidra when it's installed. Off is Clean room: nothing inside the
    /// original is read, only what can be seen from outside ([`crate::clean_room`]).
    pub deep_mode: bool,
    /// `studio` (dark) or `studio-light`.
    pub theme: String,
    /// UI language (BCP 47), empty = system.
    pub language: String,
    pub first_run_done: bool,
    pub engines: EngineSettings,
    /// AI backends and roles. Owned and parsed by the `ai` crate; kept opaque here so the engine
    /// doesn't depend on it.
    pub ai: toml::Table,
    /// Ask before running tools that execute code or change devices (always true unless a project allows them).
    pub ask_before_running: bool,
    /// Auto mode: a build keeps going without stopping to ask the person. The builder's actions are
    /// allowed except a few that are never done unasked (deleting outside the project, system
    /// changes, publishing), and, unless `questions` says otherwise, its questions get "decide
    /// yourself". On by default.
    pub auto_mode: bool,
    /// What the builder's questions get in projects that don't choose (`Manifest::questions`):
    /// none follows Auto mode (decide when it's on, ask when it's off).
    pub questions: Option<crate::workspace::Questions>,
    /// Study an app or game target by running it once and picturing its window
    /// (`native_window_capture` into capture/window.png). Off by default: it runs the person's
    /// program.
    pub runtime_study: bool,
    /// Recently opened projects and targets.
    pub recent: Vec<PathBuf>,
    /// Where new projects are created; `None` = `Documents/Craft Anything`.
    pub projects_dir: Option<PathBuf>,
    /// More places projects are listed from: folders of projects, or projects started in a folder
    /// the person picked (added when they start one there).
    pub project_folders: Vec<PathBuf>,
    /// What the files already in a folder picked for a new project are: `None` asks each time;
    /// the person's "don't ask again" answer otherwise.
    pub existing_files: Option<crate::workspace::ExistingFiles>,
    /// Generated game assets: what a new project starts with for each asset type.
    pub assets: AssetSettings,
    /// External MCP servers whose tools are offered as Craft Anything tools (`[[plugins.mcp]]`).
    pub plugins: crate::plugins::PluginSettings,
    /// How builds are checked and when refining stops (`[build]`).
    pub build: BuildSettings,
}

/// How builds are checked and when refining stops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSettings {
    /// Likeness (percent) at which a compared build is good enough and refining stops.
    pub good_enough: f64,
    /// Refinement rounds after the first build in the app (the CLI's `--rounds` sets its own).
    pub max_rounds: u32,
    /// Widths (CSS px) a website and its rebuild are compared at.
    pub web_widths: Vec<u32>,
    /// Other pages of a website studied and checked besides the start page.
    pub max_pages: usize,
    /// When set, an app or game must look at least this alike (percent) to the person's pictures
    /// (or the original running) to be good enough; unset, that likeness is only reported.
    pub picture_good_enough: Option<f64>,
    /// Projects that may build at the same time; more wait their turn (at least 1).
    pub parallel: usize,
}

impl Default for BuildSettings {
    fn default() -> Self {
        BuildSettings { good_enough: 90.0, max_rounds: 6, web_widths: vec![1440, 768, 390], max_pages: 4, picture_good_enough: None, parallel: 2 }
    }
}

impl BuildSettings {
    /// The widths to compare at: the configured ones a capture can take (320-3840 px, at most 4),
    /// else the defaults (desktop, tablet and phone).
    pub fn widths(&self) -> Vec<u32> {
        let w: Vec<u32> = self.web_widths.iter().copied().filter(|w| (320..=3840).contains(w)).take(4).collect();
        if w.is_empty() { BuildSettings::default().web_widths } else { w }
    }
}

/// What a new project starts with for generated assets. Each project keeps its own choices and
/// limits once created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetSettings {
    /// The provider and model a new project uses for each asset type. A type that isn't here is Off.
    pub defaults: BTreeMap<AssetKind, AssetChoice>,
    /// What a new project may spend on each asset type billed in US dollars (fal.ai) before the
    /// person is asked.
    pub usd_limit: f64,
    /// What a new project may use of each asset type billed in credits (Scenario, ElevenLabs).
    /// Credits are never converted to dollars.
    pub credit_limit: f64,
    /// Whether a new project starts with "Ignore budget" on: its limits are not enforced.
    pub ignore_budget: bool,
}

impl Default for AssetSettings {
    /// No defaults chosen, and the built-in limits.
    fn default() -> Self {
        AssetSettings {
            defaults: BTreeMap::new(),
            usd_limit: crate::workspace::ASSET_BUDGET_USD,
            credit_limit: crate::workspace::ASSET_CREDIT_LIMIT,
            ignore_budget: false,
        }
    }
}

impl AssetSettings {
    /// The limit a new project starts with for an asset type billed in `unit`; a negative or
    /// non-number value gives the built-in default.
    pub fn default_limit(&self, unit: Unit) -> f64 {
        let (set, built_in) = match unit {
            Unit::Usd => (self.usd_limit, crate::workspace::ASSET_BUDGET_USD),
            Unit::Credits => (self.credit_limit, crate::workspace::ASSET_CREDIT_LIMIT),
        };
        if set.is_finite() && set >= 0.0 { set } else { built_in }
    }

    /// What a new project uses for an asset type; Off when no default is chosen, and when the one
    /// chosen names a provider but no model (a hand-edited file): that can't generate anything.
    pub fn choice(&self, kind: AssetKind) -> AssetChoice {
        self.defaults.get(&kind).filter(|c| c.provider.is_none() || !c.model.is_empty()).cloned().unwrap_or_else(AssetChoice::off)
    }

    /// The dollar limit for a new project.
    ///
    /// Temporary: the callers that still think in one budget (a new project's starting budget in
    /// `ai::rebuild::project`, the price words of the Engines screen) use it until Task 10 and
    /// Task 13 of the per-type assets plan move them to [`Self::default_limit`].
    pub fn budget(&self) -> f64 {
        self.default_limit(Unit::Usd)
    }

    /// The id the asset table (removed in Task 10 of the per-type assets plan) gave the model
    /// chosen as the default for `kind`; `None` for Off, for a model that table never had, and for
    /// any type but pictures and 3D models.
    ///
    /// Temporary: the callers still reading that table (`asset_tools`, the rebuild brief, the
    /// settings screens) use it until Task 10 and Task 13 replace the table with the chosen model.
    pub fn legacy_table_id(&self, kind: AssetKind) -> Option<String> {
        let c = self.defaults.get(&kind)?;
        let provider = c.provider.as_deref()?;
        LEGACY_ASSET_MODELS.iter().find(|m| m.kind == kind && m.provider == provider && m.model == c.model).map(|m| m.id.to_string())
    }

    /// Choose the default for `kind` by its asset table id (see [`Self::legacy_table_id`]); false,
    /// and nothing changes, when the table never had that id for that type.
    ///
    /// Temporary: the Settings pictures and 3D rows use it until Task 13 of the per-type assets plan
    /// replaces them with the provider and model pickers.
    pub fn set_legacy_table_id(&mut self, kind: AssetKind, id: &str) -> bool {
        match LEGACY_ASSET_MODELS.iter().find(|m| m.kind == kind && m.id == id) {
            Some(m) => {
                self.defaults.insert(kind, m.choice());
                true
            }
            None => false,
        }
    }
}

/// A model of the asset table that `config.toml` versions 4 and older named by id
/// (`[assets] image_model` and `model_3d`).
struct LegacyAssetModel {
    id: &'static str,
    kind: AssetKind,
    provider: &'static str,
    /// The model's address at its provider.
    model: &'static str,
    name: &'static str,
    unit: Unit,
}

/// The ids of that table and the model each stands for. `scenario-meshy` pointed at an endpoint
/// Scenario has deprecated, so it stands for its replacement.
const LEGACY_ASSET_MODELS: [LegacyAssetModel; 6] = [
    LegacyAssetModel {
        id: "nano-banana-pro",
        kind: AssetKind::Image,
        provider: "fal",
        model: "fal-ai/nano-banana-pro",
        name: "Nano Banana Pro",
        unit: Unit::Usd,
    },
    LegacyAssetModel { id: "flux-dev", kind: AssetKind::Image, provider: "fal", model: "fal-ai/flux/dev", name: "FLUX.1 [dev]", unit: Unit::Usd },
    LegacyAssetModel { id: "flux-schnell", kind: AssetKind::Image, provider: "fal", model: "fal-ai/flux/schnell", name: "FLUX.1 [schnell]", unit: Unit::Usd },
    LegacyAssetModel {
        id: "scenario-gemini-3-1",
        kind: AssetKind::Image,
        provider: "scenario",
        model: "model_google-gemini-3-1-flash",
        name: "Scenario Gemini 3.1",
        unit: Unit::Credits,
    },
    LegacyAssetModel { id: "tripo-p1", kind: AssetKind::Model3d, provider: "fal", model: "tripo3d/p1/image-to-3d", name: "Tripo P1", unit: Unit::Usd },
    LegacyAssetModel {
        id: "scenario-meshy",
        kind: AssetKind::Model3d,
        provider: "scenario",
        model: "model_meshy-7-1-img23d",
        name: "Scenario Meshy 7.1",
        unit: Unit::Credits,
    },
];

impl LegacyAssetModel {
    /// The choice this model becomes. Its price is learnt at the next refresh of the provider's list.
    fn choice(&self) -> AssetChoice {
        AssetChoice {
            provider: Some(self.provider.into()),
            model: self.model.into(),
            model_name: self.name.into(),
            unit: self.unit,
            pricing: Pricing::After { unit: self.unit },
        }
    }

    /// The same choice as the `[assets.defaults.<type>]` table version 5 of `config.toml` has.
    fn v5_table(&self) -> Result<toml::Table, String> {
        let unit = toml::Value::try_from(self.unit).map_err(|e| e.to_string())?;
        let mut pricing = toml::Table::new();
        pricing.insert("when".into(), "after".into());
        pricing.insert("unit".into(), unit.clone());
        let mut choice = toml::Table::new();
        choice.insert("provider".into(), self.provider.into());
        choice.insert("model".into(), self.model.into());
        choice.insert("model_name".into(), self.name.into());
        choice.insert("unit".into(), unit);
        choice.insert("pricing".into(), toml::Value::Table(pricing));
        Ok(choice)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            mode: Mode::Beginner,
            deep_mode: false,
            theme: "studio".into(),
            language: String::new(),
            first_run_done: false,
            engines: EngineSettings::default(),
            ai: toml::Table::new(),
            ask_before_running: true,
            auto_mode: true,
            questions: None,
            runtime_study: false,
            recent: Vec::new(),
            projects_dir: None,
            project_folders: Vec::new(),
            existing_files: None,
            assets: AssetSettings::default(),
            plugins: crate::plugins::PluginSettings::default(),
            build: BuildSettings::default(),
        }
    }
}

/// The current `config.toml` format version. Bumping it means appending a step to [`SETTINGS_STEPS`].
pub const SETTINGS_VERSION: u32 = 5;

/// The steps that bring an older `config.toml` forward; step `i` goes from version `i` to `i + 1`.
const SETTINGS_STEPS: &[Step<toml::Table>] = &[settings_v0_to_v1, settings_v1_to_v2, settings_v2_to_v3, settings_v3_to_v4, settings_v4_to_v5];
const _: () = assert!(SETTINGS_STEPS.len() == SETTINGS_VERSION as usize, "one step per config.toml version");

/// Version 0 (no `version` key) has the same shape as version 1; only the number is new.
fn settings_v0_to_v1(_: &mut toml::Table) -> Result<(), String> {
    Ok(())
}

/// Version 2: Deep mode is its own switch. `[engines.ghidra] enabled` (the old switch, which
/// already meant Deep mode with or without Ghidra installed) becomes the top-level `deep_mode`.
fn settings_v1_to_v2(table: &mut toml::Table) -> Result<(), String> {
    let old = table
        .get_mut("engines")
        .and_then(toml::Value::as_table_mut)
        .and_then(|e| e.get_mut("ghidra"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|g| g.remove("enabled"));
    if let Some(v) = old {
        let on = v.as_bool().ok_or_else(|| format!("engines.ghidra.enabled ({v}) isn't true or false"))?;
        table.entry("deep_mode").or_insert(toml::Value::Boolean(on));
    }
    Ok(())
}

/// Version 3 adds more project folders and the answer about a picked folder's files; both start
/// empty. The number is new so an older build doesn't save over the list (and lose it).
fn settings_v2_to_v3(_: &mut toml::Table) -> Result<(), String> {
    Ok(())
}

/// Version 4 adds `[assets] model_3d`, the 3D generator's id in the asset table; missing keeps the
/// table's default. The number is new so an older build doesn't save over the choice (and lose it).
fn settings_v3_to_v4(_: &mut toml::Table) -> Result<(), String> {
    Ok(())
}

/// Version 5: generated assets are chosen per type. `[assets] budget_usd` becomes `usd_limit`, and
/// `image_model` and `model_3d` (ids of the old asset table) become `[assets.defaults.image]` and
/// `[assets.defaults.model_3d]`, a provider, a model and the unit it bills in. An id the old table
/// never had is dropped: that type is Off. The price is unknown until the provider's list is fetched.
fn settings_v4_to_v5(table: &mut toml::Table) -> Result<(), String> {
    let Some(assets) = table.get_mut("assets").and_then(toml::Value::as_table_mut) else {
        return Ok(());
    };
    if let Some(old) = assets.remove("budget_usd") {
        let usd = match old {
            toml::Value::Float(f) => f,
            toml::Value::Integer(n) => n as f64,
            other => return Err(format!("assets.budget_usd ({other}) isn't a number")),
        };
        assets.entry("usd_limit").or_insert(toml::Value::Float(usd));
    }
    let mut defaults = toml::Table::new();
    for (old_key, kind, new_key) in [("image_model", AssetKind::Image, "image"), ("model_3d", AssetKind::Model3d, "model_3d")] {
        let Some(old) = assets.remove(old_key) else { continue };
        let id = old.as_str().ok_or_else(|| format!("assets.{old_key} ({old}) isn't a model id"))?;
        if let Some(model) = LEGACY_ASSET_MODELS.iter().find(|m| m.kind == kind && m.id == id) {
            defaults.insert(new_key.into(), toml::Value::Table(model.v5_table()?));
        }
    }
    if !defaults.is_empty() {
        assets.entry("defaults").or_insert(toml::Value::Table(defaults));
    }
    Ok(())
}

/// Read a `config.toml` of any version: older ones are brought forward step by step; a newer one is
/// read as far as its keys are known and keeps its version, so [`Settings::save_to`] won't write it.
pub fn read_settings(text: &str) -> Result<Settings, String> {
    let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let found = match table.get("version") {
        None => 0,
        Some(v) => v.as_integer().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| format!("its version ({v}) isn't a whole number"))?,
    };
    if found <= SETTINGS_VERSION {
        let reached = upgrade(&mut table, found, 0, SETTINGS_STEPS)?;
        table.insert("version".into(), toml::Value::Integer(reached.into()));
    }
    table.try_into::<Settings>().map_err(|e| e.to_string())
}

/// Where settings, logs and the engine store live: `CRAFT_ANYTHING_CONFIG_DIR`, then a portable
/// `portable.txt` marker next to the executable, then the platform config dir.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CRAFT_ANYTHING_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
        && exe_dir.join("portable.txt").exists()
    {
        return exe_dir.join("CraftAnythingData");
    }
    dirs::config_dir().map(|d| d.join("craft-anything")).unwrap_or_else(|| PathBuf::from(".craft-anything"))
}

impl Settings {
    /// The folder new projects go in.
    pub fn projects_dir(&self) -> PathBuf {
        self.projects_dir.clone().unwrap_or_else(crate::workspace::default_projects_dir)
    }

    /// What the builder's questions get in a project that doesn't choose.
    pub fn questions(&self) -> crate::workspace::Questions {
        use crate::workspace::Questions;
        self.questions.unwrap_or(if self.auto_mode { Questions::Decide } else { Questions::Ask })
    }

    /// What the builder's questions get in `project` (its own choice, else the app's default).
    pub fn questions_in(&self, project: Option<&crate::workspace::Manifest>) -> crate::workspace::Questions {
        project.and_then(|m| m.questions).unwrap_or_else(|| self.questions())
    }

    /// Everywhere projects are listed from: the folder new projects go in, then the others, each once.
    pub fn project_locations(&self) -> Vec<PathBuf> {
        let mut v = vec![self.projects_dir()];
        for f in &self.project_folders {
            if !v.contains(f) {
                v.push(f.clone());
            }
        }
        v
    }

    /// Keep a project started in a picked folder in the list. Returns whether the list changed:
    /// false when one of the locations already shows it (it is one, or sits directly in one).
    pub fn remember_project(&mut self, root: &Path) -> bool {
        let shown = self.project_locations().iter().any(|l| l == root || root.parent() == Some(l.as_path()));
        if !shown {
            self.project_folders.push(root.to_path_buf());
        }
        !shown
    }

    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Load settings; a missing file gives defaults, an older one is brought up to date, a corrupt
    /// one is set aside and replaced with defaults (never a crash).
    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => match read_settings(&text) {
                Ok(s) => {
                    if s.version > SETTINGS_VERSION {
                        log::warn!("{} is from a newer Craft Anything (version {}); changes won't be saved over it", path.display(), s.version);
                    }
                    s
                }
                Err(e) => {
                    // A newer file this build can't read is left in place, and never saved over.
                    let newer = toml::from_str::<toml::Table>(&text)
                        .ok()
                        .and_then(|t| t.get("version").and_then(toml::Value::as_integer))
                        .and_then(|n| u32::try_from(n).ok());
                    if let Some(version) = newer.filter(|v| *v > SETTINGS_VERSION) {
                        log::warn!(
                            "{} is from a newer Craft Anything (version {version}) and unreadable here ({e}); using defaults without saving",
                            path.display()
                        );
                        return Settings { version, ..Settings::default() };
                    }
                    log::warn!("{} is unreadable ({e}); using defaults and keeping a copy", path.display());
                    let _ = std::fs::rename(path, path.with_extension("toml.broken"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    /// Load startup settings, first recovering app edits deferred by a busy shutdown.
    pub fn load() -> Settings {
        let path = Settings::path();
        if let Err(e) = Self::recover_pending(&path) {
            log::warn!("Couldn't recover pending settings: {}", e.message);
        }
        Settings::load_from(&path)
    }

    /// Keep pending non-AI edits without touching config.toml or waiting for the engine session.
    /// The sidecar uses the settings schema/version and migrations; startup merges it with the
    /// latest committed AI table, so a writer still finishing at shutdown cannot be overwritten.
    pub fn defer_save_to(&self, path: &Path) -> Result<(), ToolError> {
        let mut pending = self.clone();
        pending.ai.clear();
        pending.save_to(&path.with_extension("pending.toml"))
    }

    /// Remove deferred edits superseded by a successful full save. A missing sidecar is fine.
    pub fn discard_pending(path: &Path) -> Result<(), ToolError> {
        let pending_path = path.with_extension("pending.toml");
        match std::fs::remove_file(&pending_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ToolError::io(pending_path.display(), e)),
        }
    }

    /// Recover a deferred snapshot only when both files are readable and supported. Leave the
    /// sidecar intact on failure, and remove it only after the merged settings are saved.
    fn recover_pending(path: &Path) -> Result<(), ToolError> {
        let pending_path = path.with_extension("pending.toml");
        let text = match std::fs::read_to_string(&pending_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(ToolError::io(pending_path.display(), e)),
        };
        let bad_settings = |e: String| ToolError::new(ErrorCode::BadSettings, e);
        let mut pending = read_settings(&text).map_err(bad_settings)?;
        let current = match std::fs::read_to_string(path) {
            Ok(text) => read_settings(&text).map_err(bad_settings)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(ToolError::io(path.display(), e)),
        };
        if current.version > SETTINGS_VERSION || pending.version > SETTINGS_VERSION {
            return Err(ToolError::new(ErrorCode::BadSettings, "Update Craft Anything to recover settings saved by a newer version."));
        }
        pending.ai = current.ai;
        pending.save_to(path)?;
        Self::discard_pending(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ToolError> {
        if self.version > SETTINGS_VERSION {
            return Err(ToolError::new(
                ErrorCode::BadSettings,
                format!("{} was written by a newer Craft Anything (version {}), so this one doesn't change it.", path.display(), self.version),
            )
            .with_remediation("Update Craft Anything to change settings, or move config.toml aside to start fresh. Nothing was written."));
        }
        let text = toml::to_string_pretty(self).map_err(|e| ToolError::new(ErrorCode::BadSettings, e.to_string()))?;
        crate::project::write_atomic(path, text.as_bytes())
    }

    pub fn save(&self) -> Result<(), ToolError> {
        self.save_to(&Settings::path())
    }

    pub fn engine_store_dir(&self) -> PathBuf {
        self.engines.store_dir.clone().unwrap_or_else(|| config_dir().join("engines"))
    }

    /// Ghidra's headless launcher, when Deep mode can really use Ghidra (`None`: not installed).
    pub fn ghidra_launcher(&self) -> Option<PathBuf> {
        let (dir, store) = (self.engines.ghidra.install_dir.as_deref(), self.engine_store_dir());
        if self.engines.only_configured {
            craft_anything_engines::ghidra::get::headless_configured(dir, &store)
        } else {
            craft_anything_engines::ghidra::get::headless(dir, &store)
        }
    }

    /// The Java runtime Ghidra runs on (`None`: let Ghidra find its own).
    pub fn ghidra_java(&self) -> Option<PathBuf> {
        craft_anything_engines::ghidra::get::java_home(self.engines.ghidra.java_home.as_deref(), &self.engine_store_dir())
    }

    /// Where an external engine is on this computer, honouring every setting that can name one:
    /// the path overrides (`engines.tools`) and, for Ghidra, its own folder setting
    /// (`engines.ghidra.install_dir`). The one lookup behind every listing (`engines_list`,
    /// `doctor`, `capabilities`) and behind `Requires::Engine`, so they all agree with what Deep
    /// mode actually runs.
    pub fn engine_path(&self, id: &str) -> Option<PathBuf> {
        if id == craft_anything_engines::ghidra::get::ID {
            return self.ghidra_launcher();
        }
        if self.engines.only_configured {
            craft_anything_engines::locate_configured(id, &self.engines.tools, &self.engine_store_dir())
        } else {
            craft_anything_engines::locate(id, &self.engines.tools, &self.engine_store_dir())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn build_settings_have_safe_widths_and_old_configs_get_defaults() {
        let b = BuildSettings { web_widths: vec![10, 1280, 99_999, 375, 200], ..BuildSettings::default() };
        assert_eq!(b.widths(), [1280, 375], "widths a capture can't take are dropped");
        assert_eq!(BuildSettings { web_widths: vec![], ..BuildSettings::default() }.widths(), [1440, 768, 390]);
        let old: Settings = toml::from_str(
            "version = 1
auto_mode = true
",
        )
        .unwrap();
        assert_eq!(old.build, BuildSettings::default());
        assert_eq!(old.build.picture_good_enough, None, "picture likeness is only reported unless set");
    }

    use super::*;
    use craft_anything_engines::assets::model::Rate;

    #[test]
    fn only_configured_finds_no_engine_from_this_computer_but_keeps_the_ones_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.engines.store_dir = Some(dir.path().join("engines"));
        s.engines.only_configured = true;
        assert_eq!(s.ghidra_launcher(), None, "PATH and the usual install folders aren't searched");
        assert_eq!(s.engine_path("java"), None);
        // The folder the person picked still counts, and so does the managed store.
        let launcher_name = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        let picked = dir.path().join("picked").join("ghidra_12_PUBLIC");
        std::fs::create_dir_all(picked.join("support")).unwrap();
        std::fs::write(picked.join("support").join(launcher_name), "").unwrap();
        s.engines.ghidra.install_dir = Some(dir.path().join("picked"));
        assert_eq!(s.ghidra_launcher(), Some(picked.join("support").join(launcher_name)));
        assert_eq!(s.engine_path("ghidra"), s.ghidra_launcher());
        s.engines.ghidra.install_dir = None;
        let managed = dir.path().join("engines").join("ghidra").join("ghidra_12_PUBLIC").join("support");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join(launcher_name), "").unwrap();
        assert_eq!(s.ghidra_launcher(), Some(managed.join(launcher_name)));
        // It is never written to config.toml, and a loaded config looks around the computer.
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(!text.contains("only_configured"), "{text}");
        let loaded: Settings = toml::from_str(&text).unwrap();
        assert!(!loaded.engines.only_configured);
    }

    #[test]
    fn the_test_session_looks_only_where_its_settings_say() {
        assert!(crate::testing::session().settings.engines.only_configured);
    }

    #[test]
    fn path_overrides_match_the_engine_table_exactly() {
        use craft_anything_engines::ToolPathLookup;
        let mut paths = ToolPaths::default();
        let ids: Vec<&str> = paths.entries_mut().into_iter().map(|(id, _)| id).collect();
        let table: Vec<&str> = craft_anything_engines::path_setting_ids().collect();
        assert_eq!(ids, table, "tool_paths! must list the engine table's path-setting ids, in table order");
        for (id, v) in paths.entries_mut() {
            *v = Some(PathBuf::from(format!("/opt/{id}")));
        }
        for id in table {
            assert_eq!(paths.override_for(id), Some(PathBuf::from(format!("/opt/{id}"))), "no override for {id}");
        }
        // An older config.toml without the newer fields still loads.
        let old: ToolPaths = toml::from_str("adb = \"/x/adb\"\n").unwrap();
        assert_eq!(old.adb, Some(PathBuf::from("/x/adb")));
        assert_eq!(old.tshark, None);
    }

    /// A config.toml as written before versions existed.
    const CONFIG_V0: &str = r#"mode = "experienced"
theme = "studio-light"
language = ""
first_run_done = true
ask_before_running = true
auto_mode = false
recent = ["/home/me/app.exe"]

[engines]
store_dir = "/data/engines"

[engines.ghidra]
enabled = true

[engines.tools]
adb = "/x/adb"

[ai]
default = "Claude Code"
"#;

    #[test]
    fn configs_from_before_versions_load_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, CONFIG_V0).unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!((s.mode, s.theme.as_str(), s.first_run_done, s.auto_mode), (Mode::Experienced, "studio-light", true, false));
        assert!(s.deep_mode, "the old Ghidra switch becomes Deep mode");
        assert_eq!(s.engines.tools.adb, Some(PathBuf::from("/x/adb")));
        assert_eq!(s.ai.get("default").and_then(|v| v.as_str()), Some("Claude Code"));
        assert!(!path.with_extension("toml.broken").exists());
        s.save_to(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with(&format!("version = {SETTINGS_VERSION}\n")) && saved.contains("deep_mode = true") && !saved.contains("enabled"), "{saved}");
        assert_eq!(Settings::load_from(&path), s);
    }

    #[test]
    fn newer_configs_load_but_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let newer = format!("version = 9\nsomething_new = 3\n{CONFIG_V0}");
        std::fs::write(&path, &newer).unwrap();
        let s = Settings::load_from(&path);
        assert_eq!((s.version, s.mode), (9, Mode::Experienced));
        let err = s.save_to(&path).unwrap_err();
        assert!(err.message.contains("newer") && err.remediation.is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        // Newer and unreadable here: defaults, but the file stays where it is and isn't saved over.
        let unreadable = "version = 9\nmode = { kind = \"pro\" }\n";
        std::fs::write(&path, unreadable).unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s, Settings { version: 9, ..Settings::default() });
        assert!(s.save_to(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), unreadable);
        assert!(!path.with_extension("toml.broken").exists());
        // A version that isn't a number is a broken file: set aside, defaults used.
        std::fs::write(&path, "version = \"two\"\n").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert!(path.with_extension("toml.broken").exists());
        assert!(read_settings("version = -1").is_err());
    }

    #[test]
    fn round_trip_and_corrupt_file_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut s = Settings { mode: Mode::Experienced, ..Default::default() };
        s.deep_mode = true;
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        std::fs::write(&path, "mode = [[[").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert!(path.with_extension("toml.broken").exists());
    }

    /// Deferred saves cannot replace damaged/newer data, and failed recovery keeps both originals.
    #[test]
    fn deferred_settings_keep_ai_and_refuse_unsupported_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let pending_path = path.with_extension("pending.toml");
        let mut pending = Settings { language: "de".into(), ..Settings::default() };
        pending.ai.insert("model".into(), "stale".into());
        pending.defer_save_to(&path).unwrap();
        let pending_text = std::fs::read_to_string(&pending_path).unwrap();
        assert!(read_settings(&pending_text).unwrap().ai.is_empty());
        assert!(!path.exists(), "deferring never writes the shared config");
        for invalid in ["mode = [[[", "version = 999\n", "version = 999\nmode = []"] {
            std::fs::write(&path, invalid).unwrap();
            assert!(Settings::recover_pending(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
            assert_eq!(std::fs::read_to_string(&pending_path).unwrap(), pending_text);
        }
        let mut current = Settings::default();
        current.ai.insert("model".into(), "latest".into());
        current.save_to(&path).unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        for invalid in ["mode = [[[", "version = 999\n"] {
            std::fs::write(&pending_path, invalid).unwrap();
            assert!(Settings::recover_pending(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(std::fs::read_to_string(&pending_path).unwrap(), invalid);
        }
        std::fs::write(&pending_path, &pending_text).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(Settings::recover_pending(&path).is_err(), "I/O failure must not consume pending edits");
        assert_eq!(std::fs::read_to_string(&pending_path).unwrap(), pending_text);
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, original).unwrap();
        // An old sidecar migrates through the existing settings schema before recovery.
        std::fs::write(&pending_path, "language = \"de\"\n[ai]\nmodel = \"stale\"\n").unwrap();
        Settings::recover_pending(&path).unwrap();
        let recovered = Settings::load_from(&path);
        assert_eq!(recovered.version, SETTINGS_VERSION);
        assert_eq!(recovered.language, "de");
        assert_eq!(recovered.ai, current.ai);
        assert!(!pending_path.exists());
    }

    #[test]
    fn version_4_asset_settings_become_per_type_defaults() {
        let s = read_settings("version = 4\n[assets]\nbudget_usd = 9.0\nimage_model = \"flux-dev\"\nmodel_3d = \"scenario-meshy\"\n").unwrap();
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.assets.usd_limit, 9.0);
        assert_eq!(s.assets.credit_limit, 500.0);
        let img = s.assets.choice(AssetKind::Image);
        assert_eq!((img.provider.as_deref(), img.model.as_str(), img.unit), (Some("fal"), "fal-ai/flux/dev", Unit::Usd));
        assert_eq!(img.model_name, "FLUX.1 [dev]");
        assert_eq!(img.pricing, Pricing::After { unit: Unit::Usd }, "the price is learnt from the next refresh");
        let m3d = s.assets.choice(AssetKind::Model3d);
        assert_eq!((m3d.provider.as_deref(), m3d.model.as_str(), m3d.unit), (Some("scenario"), "model_meshy-7-1-img23d", Unit::Credits));
        assert_eq!(m3d.pricing, Pricing::After { unit: Unit::Credits });
        assert_eq!(s.assets.choice(AssetKind::Music).provider, None, "no old choice: Off");
        assert!(!s.assets.ignore_budget);
        let saved = toml::to_string(&s).unwrap();
        assert!(!saved.contains("budget_usd") && !saved.contains("image_model"), "the old keys are gone: {saved}");
    }

    #[test]
    fn every_old_table_id_maps_to_a_model_and_back() {
        let expected = [
            ("nano-banana-pro", AssetKind::Image, "fal", "fal-ai/nano-banana-pro", Unit::Usd),
            ("flux-dev", AssetKind::Image, "fal", "fal-ai/flux/dev", Unit::Usd),
            ("flux-schnell", AssetKind::Image, "fal", "fal-ai/flux/schnell", Unit::Usd),
            ("scenario-gemini-3-1", AssetKind::Image, "scenario", "model_google-gemini-3-1-flash", Unit::Credits),
            ("tripo-p1", AssetKind::Model3d, "fal", "tripo3d/p1/image-to-3d", Unit::Usd),
            ("scenario-meshy", AssetKind::Model3d, "scenario", "model_meshy-7-1-img23d", Unit::Credits),
        ];
        for (id, kind, provider, model, unit) in expected {
            let key = if kind == AssetKind::Image { "image_model" } else { "model_3d" };
            let s = read_settings(&format!("version = 4\n[assets]\n{key} = \"{id}\"\n")).unwrap();
            let c = s.assets.choice(kind);
            assert_eq!((c.provider.as_deref(), c.model.as_str(), c.unit), (Some(provider), model, unit), "{id}");
            assert!(!c.model_name.is_empty(), "{id} keeps a name to show");
            assert_eq!(s.assets.legacy_table_id(kind).as_deref(), Some(id), "{id} maps back");
            let mut other = AssetSettings::default();
            assert!(other.set_legacy_table_id(kind, id), "{id}");
            assert_eq!(other.choice(kind), c, "{id} set the same way");
        }
        let none = read_settings("version = 4\n[assets]\nimage_model = \"made-up\"\nmodel_3d = \"also-made-up\"\n").unwrap();
        assert!(none.assets.defaults.is_empty(), "an id the old table never had is dropped (Off)");
        assert_eq!(none.assets.legacy_table_id(AssetKind::Image), None);
        let mut other = AssetSettings::default();
        assert!(!other.set_legacy_table_id(AssetKind::Speech, "flux-dev"));
        assert!(!other.set_legacy_table_id(AssetKind::Image, "tripo-p1"));
        assert!(other.defaults.is_empty(), "a picture model can't be the speech default, nor a 3D one the picture default");
        let wrong = read_settings("version = 4\n[assets]\nimage_model = \"tripo-p1\"\n").unwrap();
        assert!(wrong.assets.defaults.is_empty(), "a 3D model isn't a picture model");
    }

    #[test]
    fn old_asset_settings_edge_cases_still_load() {
        let old: Settings = toml::from_str("mode = \"experienced\"\nauto_mode = false\n").unwrap();
        assert_eq!(
            (old.assets.default_limit(Unit::Usd), old.assets.default_limit(Unit::Credits)),
            (5.0, 500.0),
            "a config without [assets] gets the built-in limits"
        );
        assert!(old.assets.defaults.is_empty() && !old.assets.ignore_budget);
        // Versions 0 to 3 go through the same step; a whole-number budget is read too.
        let v3 = read_settings("version = 3\n[assets]\nbudget_usd = 9\nimage_model = \"flux-dev\"\n").unwrap();
        assert_eq!((v3.version, v3.assets.usd_limit, v3.assets.legacy_table_id(AssetKind::Model3d)), (SETTINGS_VERSION, 9.0, None));
        assert_eq!(v3.assets.legacy_table_id(AssetKind::Image).as_deref(), Some("flux-dev"));
        let kept = read_settings("version = 3\n[assets]\nmodel_3d = \"tripo-p1\"\n").unwrap();
        assert_eq!((kept.version, kept.assets.legacy_table_id(AssetKind::Model3d).as_deref()), (SETTINGS_VERSION, Some("tripo-p1")));
        // Nothing to move: no [assets] table, and an empty one.
        assert_eq!(read_settings("version = 4\n").unwrap().assets, AssetSettings::default());
        assert_eq!(read_settings("version = 4\n[assets]\n").unwrap().assets, AssetSettings::default());
        // A key of the wrong type is an error the person is told about, as before.
        assert!(read_settings("version = 4\n[assets]\nbudget_usd = \"lots\"\n").is_err());
        assert!(read_settings("version = 4\n[assets]\nimage_model = 3\n").is_err());
        // A version 5 file isn't touched again by the step.
        let v5 = read_settings("version = 5\n[assets]\nusd_limit = 7.5\ncredit_limit = 20.0\nignore_budget = true\n").unwrap();
        assert_eq!((v5.assets.usd_limit, v5.assets.credit_limit, v5.assets.ignore_budget), (7.5, 20.0, true));
        // A build older than version 5 won't save over a version 5 file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "version = 6\n[assets]\nusd_limit = 7.5\n";
        std::fs::write(&path, text).unwrap();
        let newer = Settings::load_from(&path);
        assert!(newer.save_to(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn per_type_defaults_round_trip_and_bad_limits_fall_back() {
        let choice = |provider: &str, model: &str, unit: Unit, pricing: Pricing| AssetChoice {
            provider: Some(provider.into()),
            model: model.into(),
            model_name: format!("Name of {model}"),
            unit,
            pricing,
        };
        let mut s = Settings::default();
        s.assets.credit_limit = 750.0;
        s.assets.usd_limit = 12.5;
        s.assets.ignore_budget = true;
        let a = &mut s.assets.defaults;
        a.insert(AssetKind::Image, choice("fal", "fal-ai/flux/dev", Unit::Usd, Pricing::Before { unit: Unit::Usd, rate: Rate::PerMegapixel(0.025) }));
        a.insert(AssetKind::Model3d, choice("scenario", "model_x", Unit::Credits, Pricing::After { unit: Unit::Credits }));
        a.insert(AssetKind::Speech, choice("elevenlabs", "eleven_v3", Unit::Credits, Pricing::Before { unit: Unit::Credits, rate: Rate::PerCharacter(0.5) }));
        a.insert(AssetKind::SoundEffect, choice("elevenlabs", "eleven_sfx", Unit::Credits, Pricing::Unknown));
        a.insert(
            AssetKind::Music,
            choice("elevenlabs", "music_v1", Unit::Credits, Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: None } }),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        s.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[assets.defaults.music]") && text.contains("[assets.defaults.model_3d]"), "{text}");
        assert_eq!(Settings::load_from(&path), s, "{text}");
        assert_eq!(s.assets.choice(AssetKind::Music).model, "music_v1");
        assert_eq!(s.assets.default_limit(Unit::Credits), 750.0);
        // An explicit Off choice and a missing one both mean Off.
        s.assets.defaults.insert(AssetKind::Music, AssetChoice::off());
        let again = read_settings(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(again.assets.choice(AssetKind::Music), AssetChoice::off());
        assert_eq!(AssetSettings::default().choice(AssetKind::Speech), AssetChoice::off());
        assert_eq!(AssetChoice::off().provider, None);
        // A limit that isn't a usable amount falls back to the built-in one; the other unit is untouched.
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let odd = AssetSettings { usd_limit: bad, credit_limit: bad, ..AssetSettings::default() };
            assert_eq!((odd.default_limit(Unit::Usd), odd.default_limit(Unit::Credits)), (5.0, 500.0), "{bad}");
        }
        let odd: Settings = toml::from_str("[assets]\nusd_limit = -3.0\ncredit_limit = 40.0\n").unwrap();
        assert_eq!((odd.assets.default_limit(Unit::Usd), odd.assets.default_limit(Unit::Credits)), (5.0, 40.0));
        assert_eq!(odd.assets.budget(), 5.0, "the temporary dollar budget follows the dollar limit");
    }

    #[test]
    fn a_default_with_a_provider_but_no_model_is_off() {
        let mut s = AssetSettings::default();
        s.defaults.insert(AssetKind::Image, AssetChoice { provider: Some("fal".into()), ..AssetChoice::off() });
        assert_eq!(s.choice(AssetKind::Image), AssetChoice::off());
        // The same read from a file that names the provider only.
        let from_file = read_settings("version = 5\n[assets.defaults.image]\nprovider = \"fal\"\n").unwrap();
        assert_eq!(from_file.assets.choice(AssetKind::Image), AssetChoice::off());
        // A model makes it a choice.
        s.defaults.insert(AssetKind::Image, AssetChoice { provider: Some("fal".into()), model: "fal-ai/flux/dev".into(), ..AssetChoice::off() });
        assert_eq!(s.choice(AssetKind::Image).provider.as_deref(), Some("fal"));
    }

    #[test]
    fn projects_are_listed_from_more_folders_and_a_picked_one_is_remembered() {
        let mut s = Settings { projects_dir: Some(PathBuf::from("/p")), ..Settings::default() };
        assert_eq!(s.project_locations(), [PathBuf::from("/p")]);
        assert!(!s.remember_project(Path::new("/p/notes")), "already listed: it's in the projects folder");
        assert!(s.remember_project(Path::new("/work/mine")));
        assert!(!s.remember_project(Path::new("/work/mine")), "once");
        s.project_folders.push(PathBuf::from("/p"));
        assert_eq!(s.project_locations(), [PathBuf::from("/p"), PathBuf::from("/work/mine")], "each place once");
        // Saved and read back, with the answer about a picked folder's files.
        s.existing_files = Some(crate::workspace::ExistingFiles::StartFrom);
        let back = read_settings(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.project_folders, s.project_folders);
        assert_eq!(back.existing_files, s.existing_files);
        // A version 2 config has neither, and asks.
        let v2 = read_settings("version = 2\ndeep_mode = true\n").unwrap();
        assert_eq!(v2.version, SETTINGS_VERSION);
        assert!(v2.project_folders.is_empty() && v2.existing_files.is_none() && v2.deep_mode);
    }
}
