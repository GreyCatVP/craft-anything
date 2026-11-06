use super::*;
use craft_anything_engine::tools::web_common::{Image, encode_png};

fn picture(path: &Path, blank: bool) {
    let rgba = (0..64 * 64).flat_map(|i| if blank || i % 64 < 32 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
    std::fs::write(path, encode_png(&Image { width: 64, height: 64, rgba }).unwrap()).unwrap();
}

#[test]
fn old_empty_studies_and_unreadable_pictures_cannot_start_a_recreation() {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Editor", "Recreate this editor", TargetRef::Local { path: d.path().join("missing.exe") }).unwrap();
    std::fs::create_dir_all(ws.capture_dir()).unwrap();
    std::fs::write(ws.capture_dir().join("README.md"), "An earlier run skipped observation.").unwrap();
    let mut w = check_worker(&ws, false);
    assert!(w.ensure_native_observation(&mut ws).unwrap_err().message.contains("not been observed"));
    let shot = ws.capture_dir().join("window.png");
    std::fs::write(&shot, "invalid image").unwrap();
    assert!(w.ensure_native_observation(&mut ws).is_err());
    picture(&shot, true);
    assert!(w.ensure_native_observation(&mut ws).is_err(), "a blank window establishes no observation");
    picture(&shot, false);
    w.ensure_native_observation(&mut ws).unwrap();
    std::fs::write(ws.capture_dir().join("ui-tree.json"), r#"{"nodes":[{"role":"Window","modal":false}]}"#).unwrap();
    w.engine.session().settings.runtime_study = true;
    w.ensure_native_observation(&mut ws).unwrap();
    assert_eq!(ws.manifest.status, Status::New, "usable saved evidence avoids launching the missing program");
}

#[test]
fn descriptions_and_supplied_pictures_have_their_own_observation_paths() {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Editor", "Make an editor", TargetRef::None).unwrap();
    let mut w = check_worker(&ws, false);
    w.ensure_native_observation(&mut ws).unwrap();
    ws.manifest.target = TargetRef::Local { path: d.path().join("missing.exe") };
    std::fs::create_dir_all(ws.root.join("reference")).unwrap();
    picture(&ws.root.join("reference/picture-1.png"), false);
    w.ensure_native_observation(&mut ws).unwrap();
}

#[cfg(windows)]
#[test]
fn enabling_runtime_study_retries_a_legacy_capture_and_reports_the_real_failure() {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Editor", "Recreate this editor", TargetRef::Local { path: d.path().join("missing.exe") }).unwrap();
    std::fs::create_dir_all(ws.capture_dir()).unwrap();
    std::fs::write(ws.capture_dir().join("README.md"), "Earlier study without runtime observation.").unwrap();
    let mut w = check_worker(&ws, false);
    w.engine.session().settings.runtime_study = true;
    let e = w.ensure_native_observation(&mut ws).unwrap_err();
    assert!(e.message.contains("Could not observe the original") && e.message.contains("isn't a program file"), "{e:?}");
    assert_eq!(ws.manifest.status, Status::Inspecting, "the native study ran before the builder");
    assert!(!ws.capture_dir().join("window.png").exists());
    picture(&ws.capture_dir().join("window.png"), false);
    std::fs::write(ws.capture_dir().join("ui-tree.json"), r#"{"nodes":[{"role":"Window"}]}"#).unwrap();
    assert!(
        w.ensure_native_observation(&mut ws).unwrap_err().message.contains("Could not observe the original"),
        "a legacy screenshot without modal-state evidence is refreshed too"
    );
    w.stop.cancel();
    assert_eq!(w.ensure_native_observation(&mut ws).unwrap_err().code, ErrorCode::Cancelled);
}

#[test]
fn readiness_requires_every_check_and_essential_question_to_be_settled() {
    use craft_anything_engine::workspace::Claim;
    use craft_anything_evidence::{Severity, Unknown};
    let pass = CheckResult::new("app-screenshot", 1, true, "The window draws.");
    assert!(self_check::require_ready(&[], 90.0, &[]).is_err());
    self_check::require_ready(std::slice::from_ref(&pass), 90.0, &[]).unwrap();
    let failures = [
        CheckResult::new("build", 1, false, "Build failed."),
        CheckResult { similarity: Some(30.0), ..pass.clone() },
        CheckResult { unknowns: vec!["Original was not observed".into()], ..pass.clone() },
        CheckResult { claims: vec![Claim::new("A required control exists", false, "Missing")], ..pass.clone() },
    ];
    for fail in failures {
        let checks = [fail, pass.clone()];
        assert!(self_check::require_ready(&checks, 90.0, &[]).is_err(), "a passing last check cannot hide earlier failure");
        assert!(!self_check::report(&checks, 90.0, &[]).contains("This would pass"));
    }
    let questions = [Unknown::new("The editing workflow has not been observed", "rebuild", Severity::High)];
    assert!(self_check::require_ready(std::slice::from_ref(&pass), 90.0, &questions).is_err());
    assert!(!self_check::report(&[pass], 90.0, &questions).contains("This would pass"));
}

#[test]
fn modal_capture_is_an_observation_gap_until_the_working_interface_is_available() {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Editor", "Recreate this editor", TargetRef::Local { path: d.path().join("missing.exe") }).unwrap();
    std::fs::create_dir_all(ws.capture_dir()).unwrap();
    picture(&ws.capture_dir().join("window.png"), false);
    let tree = ws.capture_dir().join("ui-tree.json");
    std::fs::write(&tree, r#"{"nodes":[{"role":"Window","modal":true}]}"#).unwrap();
    let mut w = check_worker(&ws, false);
    let error = w.ensure_native_observation(&mut ws).unwrap_err();
    assert!(error.message.contains("modal dialog"));
    assert!(error.remediation.unwrap().contains("leave the editor open"));
    std::fs::write(&tree, r#"{"nodes":[{"role":"Window","modal":false}]}"#).unwrap();
    w.ensure_native_observation(&mut ws).unwrap();
}
