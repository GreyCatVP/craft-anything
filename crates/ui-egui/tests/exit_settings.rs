//! Exercise the real exit-save path in a child whose global config directory is isolated.

use craft_anything_ai::{AiConfig, AiHub, BackendConfig, BackendEntry, BackendKind, BackendOptions, Effort, KeyStore};
use craft_anything_engine::{Engine, Mode, Session, Settings};
use craft_anything_platform::process::Process;
use craft_anything_ui_egui::CraftApp;
use eframe::App;
use std::{sync::Arc, time::Duration};

/// Closing with pending app edits must preserve an AI selection committed since the app snapshot.
#[test]
fn exit_save_preserves_latest_ai_and_pending_app_settings() {
    const CHILD: &str = "CRAFT_ANYTHING_EXIT_SAVE_TEST";
    let Some(dir) = std::env::var_os(CHILD) else {
        let dir = tempfile::tempdir().unwrap();
        Process::new(std::env::current_exe().unwrap())
            .args(["--exact", "exit_save_preserves_latest_ai_and_pending_app_settings", "--nocapture"])
            .env(CHILD, dir.path())
            .env("CRAFT_ANYTHING_CONFIG_DIR", dir.path())
            .timeout(Duration::from_secs(30))
            .run_checked()
            .unwrap();
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    assert_eq!(Settings::path(), dir.join("config.toml"));
    let engine = Arc::new(Engine::new(Session::isolated(&dir)));
    let hub = Arc::new(AiHub::with_stores(engine.clone(), Settings::path(), KeyStore::file_only(dir.join("keys.toml"))));
    let mut entry = BackendEntry {
        config: BackendConfig { name: "OpenCode".into(), kind: BackendKind::AgentCli, model: "opencode".into(), base_url: None, ssh_host: None, enabled: true },
        options: BackendOptions { cli_model: Some("test/old".into()), cli_thinking: Some(Effort::Low), ..Default::default() },
    };
    hub.add_backend_with(entry.clone(), None).unwrap();
    let mut app = CraftApp::new(engine.clone(), hub.clone());
    app.settings.mode = Mode::Experienced;
    app.settings.language = "de".into();
    app.settings_changed();

    // Commit the newer selection through the hub, leaving the app's AI snapshot stale.
    entry.options.cli_model = Some("test/new".into());
    entry.options.cli_thinking = Some(Effort::Max);
    hub.add_backend_with(entry.clone(), None).unwrap();
    let latest_ai = engine.session().settings.ai.clone();
    assert_ne!(app.settings.ai, latest_ai);
    app.on_exit();

    let saved = Settings::load();
    assert_eq!(AiConfig::from_table(&saved.ai).unwrap().get("OpenCode"), Some(&entry));
    assert_eq!(saved.mode, Mode::Experienced);
    assert_eq!(saved.language, "de");
    assert_eq!(app.settings.ai, latest_ai);
    let session = engine.session();
    assert_eq!(session.settings.ai, latest_ai);
    assert_eq!(session.settings.mode, saved.mode);
    assert_eq!(session.settings.language, saved.language);
}
