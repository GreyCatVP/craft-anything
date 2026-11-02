//! The attached controller, a second client, and a real worker with synthetic local programs.
#![cfg(unix)]
use craft_anything_ai::{AiConfig, BackendConfig, BackendEntry, BackendKind, BackendOptions};
use craft_anything_engine::{
    Settings,
    goal::{self, BuildLease, GoalStatus},
    workspace::{TargetRef, Workspace},
};
use craft_anything_platform::process::{Output, Process};
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    time::{Duration, Instant},
};

fn cli(config: &Path, args: &[&str], path: &std::ffi::OsStr) -> Output {
    Process::new(env!("CARGO_BIN_EXE_craft-anything-cli"))
        .args(args)
        .env("CRAFT_ANYTHING_CONFIG_DIR", config)
        .env(craft_anything_ai::keys::NO_KEYCHAIN_ENV, "1")
        .env("PATH", path)
        .timeout(Duration::from_secs(30))
        .idle_timeout(Duration::from_secs(20))
        .run()
        .unwrap()
}

#[test]
fn attached_goal_can_be_paused_from_another_client_and_resumed_to_completion() {
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let bin = d.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    // Stand-ins for the coding agent and compiler; no model service, package install or browser.
    for (name, text) in [("agent", "#!/bin/sh\nsleep 1\nprintf 'Done.\\n'\n"), ("cargo", "#!/bin/sh\nexit 0\n")] {
        let file = bin.join(name);
        std::fs::write(&file, text).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    let mut ai = AiConfig::default();
    ai.upsert(BackendEntry {
        config: BackendConfig {
            name: "Synthetic builder".into(),
            kind: BackendKind::AgentCli,
            model: "stand-in".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        },
        options: BackendOptions { command: Some(bin.join("agent").display().to_string()), ..Default::default() },
    });
    Settings { ai: ai.to_table().unwrap(), ..Default::default() }.save_to(&config.join("config.toml")).unwrap();
    let mut ws = Workspace::create(d.path(), "Sample", "A website", TargetRef::None).unwrap();
    ws.manifest.stack = "HTML".into();
    ws.manifest.agent_session = Some("synthetic-session".into());
    ws.save().unwrap();
    std::fs::create_dir_all(ws.capture_dir()).unwrap();
    std::fs::create_dir_all(ws.app_dir()).unwrap();
    std::fs::write(ws.app_dir().join("Cargo.toml"), "[package]\nname = 'sample'\nversion = '0.1.0'\n").unwrap();
    let request =
        json!({"project":ws.root,"goal":{"objective":"Rebuild the visible interactions","milestones":[{"title":"Core flow"},{"title":"Interaction polish"}]}})
            .to_string();
    let saved = cli(&config, &["project-goal-set", "--json", &request], &path);
    assert!(saved.success, "{}", saved.stdout_text());
    let (root, config2, path2) = (ws.root.clone(), config.clone(), path.clone());
    let controller = std::thread::spawn(move || cli(&config2, &["goal", root.to_str().unwrap()], &path2));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !BuildLease::busy(&ws.root).unwrap() {
        assert!(Instant::now() < deadline, "the attached command never started its worker");
        std::thread::sleep(Duration::from_millis(20));
    }
    let duplicate = cli(&config, &["project-goal-resume", "--allow-run", "--project", ws.root.to_str().unwrap()], &path);
    assert!(!duplicate.success, "a second client started another writer");
    let paused = cli(&config, &["project-goal-pause", "--project", ws.root.to_str().unwrap()], &path);
    assert!(paused.success, "{}", paused.stdout_text());
    let first = controller.join().unwrap();
    assert_eq!(first.code, Some(1), "{}", first.stdout_text());
    assert_eq!(goal::load(&ws.root).unwrap().unwrap().status, GoalStatus::Paused);
    assert!(!BuildLease::busy(&ws.root).unwrap());
    let completed = cli(&config, &["goal", ws.root.to_str().unwrap()], &path);
    assert!(completed.success, "{} {}", completed.stdout_text(), completed.stderr_text());
    let g = goal::load(&ws.root).unwrap().unwrap();
    assert_eq!(g.status, GoalStatus::Complete);
    assert_eq!(g.current_milestone, 2);
    assert_eq!(g.cycles, 2);
    assert!(g.work_ms > 0);
    assert!(!BuildLease::busy(&ws.root).unwrap());
}
