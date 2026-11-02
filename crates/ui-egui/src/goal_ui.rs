//! A thin project-goal client. Drawing never reads files or starts a worker directly.
use crate::{
    CraftApp,
    i18n::{tr, trf},
    state::GoalDraft,
    theme::{self, Tokens},
    widgets,
};
use craft_anything_engine::{
    goal::{GoalSpec, GoalStatus, Limits, Milestone, Requirement, default_requirements},
    workspace::Workspace,
};
use egui::{RichText, Ui};
use serde_json::json;

pub fn configured(app: &CraftApp, ws: &Workspace) -> bool {
    (app.facts.root == ws.root && (app.facts.goal.is_some() || app.facts.goal_error.is_some()))
        || app.state.goal_draft.as_ref().is_some_and(|d| d.root == ws.root)
}

pub fn show(app: &mut CraftApp, ui: &mut Ui, ws: &Workspace) {
    let t = Tokens::get(ui.ctx());
    let goal = (app.facts.root == ws.root).then(|| app.facts.goal.clone()).flatten();
    let busy = app.runs.is_busy(&ws.root) || (app.facts.root == ws.root && app.facts.goal_running);
    widgets::card(&t).show(ui, |ui| {
        widgets::caption(ui, "Long-term goal");
        if let Some(error) = &app.facts.goal_error {
            ui.label(RichText::new(error).color(t.warning));
        }
        if let Some(g) = &goal {
            ui.label(theme::heading(&g.spec.objective, theme::BODY));
            ui.label(RichText::new(tr(g.status.label())).color(t.text_dim));
            let done = g.current_milestone.min(g.spec.milestones.len());
            ui.label(trf("{done} of {total} milestones", &[("done", &done.to_string()), ("total", &g.spec.milestones.len().to_string())]));
            if let Some(m) = g.spec.milestones.get(done) {
                ui.label(&m.title);
            }
            ui.label(trf(
                "Cycles {used}/{limit} · active minutes {minutes}/{max}",
                &[
                    ("used", &g.cycles.to_string()),
                    ("limit", &g.spec.limits.max_cycles.to_string()),
                    ("minutes", &(g.work_ms / 60_000).to_string()),
                    ("max", &g.spec.limits.max_minutes.to_string()),
                ],
            ));
            let cap = g.spec.limits.max_tokens.map(|n| n.to_string()).unwrap_or_else(|| tr("No token limit"));
            ui.label(trf("Reported tokens: {used} · {limit}", &[("used", &g.tokens.to_string()), ("limit", &cap)]));
            if let Some(c) = g.checkpoints.last() {
                ui.collapsing(tr("Latest acceptance results"), |ui| {
                    for (m, gates) in g.spec.milestones.iter().zip(&c.results) {
                        ui.label(&m.title);
                        for gate in gates {
                            ui.label(format!("{} {}", if gate.passed { "✓" } else { "○" }, gate.detail));
                        }
                    }
                });
            }
            if !g.note.is_empty() {
                ui.label(RichText::new(&g.note).size(theme::SMALL).color(t.text_dim));
            }
            if g.status == GoalStatus::Active && !busy {
                ui.label(RichText::new(tr("Waiting to start, or interrupted. Resume after reopening.")).size(theme::SMALL).color(t.text_dim));
            }
            ui.horizontal_wrapped(|ui| {
                if (g.status == GoalStatus::Active || busy) && widgets::secondary(ui, Some("pause"), "Pause goal").clicked() {
                    app.act("project_goal_pause", json!({"project":ws.root}), tr("Pause goal"));
                }
                if !busy && g.status != GoalStatus::Complete && widgets::secondary(ui, Some("play"), "Resume goal").clicked() {
                    app.act("project_goal_resume", json!({"project":ws.root}), tr("Resume goal"));
                }
            });
        } else {
            ui.label(RichText::new(tr("Work through milestones, verify each result, and keep progress across restarts.")).color(t.text_dim));
        }
        if !busy && goal.as_ref().is_none_or(|g| g.status != GoalStatus::Active) && app.state.goal_draft.as_ref().is_none_or(|d| d.root != ws.root) {
            let label = if goal.is_some() { "Edit goal" } else { "Set a goal" };
            if widgets::secondary(ui, None, label).clicked() {
                app.state.goal_draft = Some(GoalDraft {
                    root: ws.root.clone(),
                    expected_revision: goal.as_ref().map(|g| g.revision),
                    spec: goal.as_ref().map(|g| g.spec.clone()).unwrap_or_else(|| GoalSpec {
                        objective: ws.manifest.request.clone(),
                        constraints: vec![],
                        milestones: vec![milestone()],
                        limits: Limits::default(),
                    }),
                });
            }
        }
        if !busy {
            editor(app, ui, ws);
        }
    });
    ui.add_space(12.0);
}

fn milestone() -> Milestone {
    Milestone { title: tr("Complete the goal"), instructions: String::new(), acceptance: default_requirements() }
}

fn editor(app: &mut CraftApp, ui: &mut Ui, ws: &Workspace) {
    let Some(d) = app.state.goal_draft.as_mut().filter(|d| d.root == ws.root) else { return };
    ui.separator();
    widgets::caption(ui, "Objective");
    widgets::named(ui.add(egui::TextEdit::multiline(&mut d.spec.objective).desired_width(f32::INFINITY).desired_rows(3).char_limit(4000)), "Objective");
    ui.collapsing(tr("Constraints"), |ui| {
        for (i, c) in d.spec.constraints.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                widgets::named(ui.text_edit_singleline(c), "Constraint");
            });
        }
        if d.spec.constraints.len() < 64 && widgets::link(ui, "Add constraint").clicked() {
            d.spec.constraints.push(String::new());
        }
    });
    widgets::caption(ui, "Milestones");
    let mut remove = None;
    for (i, m) in d.spec.milestones.iter_mut().enumerate() {
        ui.push_id(i, |ui| {
            widgets::named(ui.text_edit_singleline(&mut m.title), "Milestone title");
            widgets::named(
                ui.add(
                    egui::TextEdit::multiline(&mut m.instructions)
                        .hint_text(tr("What should this milestone achieve?"))
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .char_limit(4000),
                ),
                "What should this milestone achieve?",
            );
            ui.collapsing(tr("Acceptance requirements"), |ui| {
                let mut remove_requirement = None;
                let removable = m.acceptance.len() > 1;
                for (j, r) in m.acceptance.iter_mut().enumerate() {
                    ui.push_id(j, |ui| {
                        match r {
                            Requirement::ProjectChecks { minimum_similarity } => {
                                ui.label(tr("All project checks must pass"));
                                number(ui, "Minimum similarity (%)", minimum_similarity, 0.0..=100.0);
                            }
                            Requirement::Check { id, minimum_similarity } => {
                                ui.label(tr("Required check id"));
                                widgets::named(ui.text_edit_singleline(id), "Required check id");
                                let mut limited = minimum_similarity.is_some();
                                if ui.checkbox(&mut limited, tr("Require a similarity score")).changed() {
                                    *minimum_similarity = limited.then_some(90.0);
                                }
                                if let Some(score) = minimum_similarity {
                                    number(ui, "Minimum similarity (%)", score, 0.0..=100.0);
                                }
                            }
                            Requirement::ResolvedUnknown { id } => {
                                ui.label(tr("Evidence-backed unknown id"));
                                widgets::named(ui.text_edit_singleline(id), "Evidence-backed unknown id");
                            }
                        }
                        if removable && widgets::link(ui, "Remove requirement").clicked() {
                            remove_requirement = Some(j);
                        }
                    });
                }
                if let Some(j) = remove_requirement {
                    m.acceptance.remove(j);
                }
                if m.acceptance.len() < 64 {
                    if widgets::link(ui, "Add required check").clicked() {
                        m.acceptance.push(Requirement::Check { id: String::new(), minimum_similarity: None });
                    }
                    if widgets::link(ui, "Add evidence requirement").clicked() {
                        m.acceptance.push(Requirement::ResolvedUnknown { id: String::new() });
                    }
                }
            });
            if widgets::link(ui, "Remove milestone").clicked() {
                remove = Some(i);
            }
            ui.add_space(6.0);
        });
    }
    if let Some(i) = remove {
        d.spec.milestones.remove(i);
    }
    if d.spec.milestones.len() < 64 && widgets::link(ui, "Add milestone").clicked() {
        d.spec.milestones.push(milestone());
    }
    ui.label(RichText::new(tr("Checks measure what they test. A successful build alone does not prove a faithful reconstruction.")).size(theme::SMALL));
    number(ui, "Maximum cycles", &mut d.spec.limits.max_cycles, 1..=1000);
    number(ui, "Active time (minutes)", &mut d.spec.limits.max_minutes, 1..=10_080);
    number(ui, "Cycles without progress", &mut d.spec.limits.stalled_cycles, 2..=100);
    let mut limit = d.spec.limits.max_tokens.is_some();
    if ui.checkbox(&mut limit, tr("Limit reported tokens")).changed() {
        d.spec.limits.max_tokens = limit.then_some(100_000);
    }
    if let Some(max) = &mut d.spec.limits.max_tokens {
        number(ui, "Reported token limit", max, 1..=u64::MAX);
    }
    let mut save = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        save = widgets::secondary(ui, None, "Save goal").clicked();
        cancel = widgets::link(ui, "Cancel").clicked();
    });
    if save {
        let params = json!({"project":d.root,"expected_revision":d.expected_revision,"goal":d.spec});
        app.act("project_goal_set", params, tr("Save goal"));
    } else if cancel {
        app.state.goal_draft = None;
    }
}

fn number<N: egui::emath::Numeric>(ui: &mut Ui, label: &str, value: &mut N, range: std::ops::RangeInclusive<N>) {
    ui.horizontal_wrapped(|ui| {
        widgets::named(ui.add(egui::DragValue::new(value).range(range)), label);
        ui.label(tr(label));
    });
}
