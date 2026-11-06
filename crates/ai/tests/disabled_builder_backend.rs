use craft_anything_ai::rebuild::builder_backend;
use craft_anything_ai::{AiConfig, BackendConfig, BackendEntry, BackendKind};
use craft_anything_evidence::ErrorCode;

/// Build a named selection beside an enabled fallback, so rejection cannot pass
/// merely because the configuration has no usable backend.
fn configuration(kind: BackendKind, enabled: bool) -> AiConfig {
    let mut cfg = AiConfig::default();
    for (name, kind, enabled) in [("Available", BackendKind::AgentCli, true), ("Selected", kind, enabled)] {
        cfg.upsert(BackendEntry {
            config: BackendConfig { name: name.into(), kind, model: String::new(), base_url: None, ssh_host: None, enabled },
            options: Default::default(),
        });
    }
    cfg
}

fn refuses_disabled(kind: BackendKind) {
    let cfg = configuration(kind, false);
    let error = builder_backend(&cfg, Some("Selected")).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadSettings);
    assert!(error.message.contains("Selected") && error.message.contains("turned off"), "{}", error.message);
    assert!(error.remediation.as_deref().is_some_and(|r| r.contains("Settings") && r.contains("--backend")));
}

#[test]
fn an_explicit_disabled_api_backend_is_rejected() {
    refuses_disabled(BackendKind::Anthropic);
}

#[test]
fn an_explicit_disabled_cli_backend_is_rejected() {
    refuses_disabled(BackendKind::AgentCli);
}

#[test]
fn an_explicit_enabled_backend_is_preserved() {
    let cfg = configuration(BackendKind::Anthropic, true);
    assert_eq!(builder_backend(&cfg, Some("Selected")).unwrap().config.name, "Selected");
}

#[test]
fn automatic_selection_skips_disabled_backends() {
    let cfg = configuration(BackendKind::Anthropic, false);
    assert_eq!(builder_backend(&cfg, None).unwrap().config.name, "Available");
}

#[test]
fn an_unknown_explicit_backend_does_not_fall_back() {
    let cfg = configuration(BackendKind::Anthropic, true);
    assert_eq!(builder_backend(&cfg, Some("Missing")).unwrap_err().code, ErrorCode::BadSettings);
}
