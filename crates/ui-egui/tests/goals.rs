//! Goal controls dispatch the same tools as headless clients; no live builder is needed.
use craft_anything_engine::{
    Mode,
    goal::{self, Goal, GoalSpec, GoalStatus, Limits},
    workspace::{TargetRef, Workspace},
};
use craft_anything_ui_egui::{CraftApp, demo};
use egui_kittest::{Harness, kittest::Queryable};
use std::time::{Duration, Instant};

fn harness(ws: Workspace) -> Harness<'static, CraftApp> {
    Harness::builder().with_size(egui::vec2(1440.0, 1100.0)).with_max_steps(32).build_eframe(move |cc| {
        CraftApp::setup_context(&cc.egui_ctx);
        let mut app = demo::app(demo::engine(Mode::Beginner, true));
        app.open_workspace(ws);
        app
    })
}

fn wait(h: &mut Harness<'static, CraftApp>, done: impl Fn(&CraftApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.step();
        if done(h.state()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "goal UI didn't update; toasts={:?}, draft={:?}, facts={:?}",
            h.state().state.toasts,
            h.state().state.goal_draft,
            h.state().facts
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_goal_can_be_saved_from_the_project_and_restored_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::create(dir.path(), "Sample", "Rebuild a notes app", TargetRef::None).unwrap();
    let mut h = harness(ws.clone());
    h.run_steps(3);
    h.get_by_label("Set a goal").scroll_to_me();
    h.run_steps(2);
    h.get_by_label("Set a goal").click();
    h.run_steps(2);
    assert!(h.state().state.goal_draft.is_some());
    h.get_by_label("Save goal").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Save goal").hover();
    h.run_steps(2);
    h.get_by_label("Save goal").click();
    wait(&mut h, |a| a.facts.goal.is_some());
    assert!(h.state().state.goal_draft.is_none());
    assert_eq!(goal::load(&ws.root).unwrap().unwrap().status, GoalStatus::Paused);
    drop(h);
    let mut reopened = harness(Workspace::open(&ws.root).unwrap());
    wait(&mut reopened, |a| a.facts.goal.is_some());
    reopened.get_by_label("Resume goal");
    assert!(!reopened.state().facts.goal_running, "opening never resumes automatically");
}

#[test]
fn an_interrupted_goal_exposes_resume_and_pause_without_starting_an_agent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::create(dir.path(), "Sample", "Rebuild a notes app", TargetRef::None).unwrap();
    goal::set(
        &ws.root,
        GoalSpec { objective: "Match the original interactions".into(), milestones: vec![], constraints: vec![], limits: Limits::default() },
        None,
    )
    .unwrap();
    goal::update(&ws.root, Goal::activate).unwrap();
    let mut h = harness(ws.clone());
    wait(&mut h, |a| a.facts.goal.is_some());
    h.get_by_label("Resume goal");
    // Pause appears beside the goal and in the composer; either dispatches the shared tool.
    h.query_all_by_label("Pause goal").next().unwrap().click();
    wait(&mut h, |a| a.facts.goal.as_ref().is_some_and(|g| g.status == GoalStatus::Paused));
    assert_eq!(goal::load(&ws.root).unwrap().unwrap().status, GoalStatus::Paused);
}
