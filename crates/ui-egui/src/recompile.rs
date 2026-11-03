//! Simple-mode Recompile: pick a console, see where its recompiler's output runs, pick a game the
//! person owns (disc image, extracted folder or executable) and see whether it can be recompiled.
//! The checks come from the engine's `game_original_inspect`; this screen only shows and asks.

use crate::i18n::tr;
use crate::theme::{self, Tokens};
use crate::{CraftApp, Pick, icons, widgets};
use craft_anything_engine::evidence::ToolError;
use craft_anything_engines::console::{CONSOLES, Support};
use egui::{RichText, Ui, Vec2};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn show(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    // Once there's a build, the conversation sits beside it: the AI fixes what Play reports.
    if app.settings.deep_mode && app.state.recompile.status.is_some() {
        prime_chat(app);
        let side_w = 420.0_f32.min(ui.available_width() * 0.4);
        egui::Panel::right("recompile-chat")
            .exact_size(side_w)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.canvas).inner_margin(crate::chat::PANE_MARGIN))
            .show(ui, |ui| crate::chat::conversation(app, ui, true));
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let full_w = ui.available_width();
        let w = full_w.min(1000.0) - 48.0;
        let pad = ((full_w - w) / 2.0).max(24.0);
        ui.add_space(40.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(w);
                ui.label(theme::heading(tr("Recompile a console game"), theme::TITLE));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(tr("Turn an original console game into a program for this computer. Only for games you own; nothing is decrypted."))
                        .size(theme::BODY)
                        .color(t.text_dim),
                );
                ui.add_space(18.0);
                if !app.settings.deep_mode {
                    crate::engine_ui::deep_needed(app, ui);
                    return;
                }
                consoles(app, ui, w);
                ui.add_space(18.0);
                pick(app, ui);
                ui.add_space(18.0);
                result(app, ui, w);
                progress(app, ui, w);
                ui.add_space(36.0);
            });
        });
    });
}

/// Start a fresh conversation about this build (once per build), told what the build is and how
/// to fix it; the context goes ahead of the person's first message.
fn prime_chat(app: &mut CraftApp) {
    let r = &app.state.recompile;
    let (Some(out), Some(input)) = (r.out.clone(), r.input.clone()) else { return };
    if app.chat_for.as_ref() == Some(&out) {
        return;
    }
    let title = r.result.as_ref().and_then(|v| v["title"].as_str()).unwrap_or("the game").to_string();
    let console = r.console.clone();
    app.state.chat.clear();
    if let Some(mut c) = app.conversation.take() {
        c.stop();
    }
    app.chat_for = Some(out.clone());
    app.chat_context = Some(format!(
        "Context: I'm recompiling my own copy of {title} ({console}) with Craft Anything. The build's folder is {out}; the game is {input}. \
         game_recomp_status (output = that folder) shows each stage and the build log; the last run of the game logged to {play}. \
         To fix what the game stops on: find the missing piece (open {xbe} with the native tools to read its code), set it with game_recomp_config_set, \
         then tell me to press Rebuild and play (or run game_recomp_start with from = \"recompile\" and wait = true, then game_recomp_play, if you're allowed to). \
         Never decrypt or bypass anything.",
        out = out.display(),
        input = input.display(),
        play = out.join("play.log").display(),
        xbe = out.join("game").join("assets").join("default.xbe").display(),
    ));
}

/// One tile per console, then the chosen backend and where its output runs.
fn consoles(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    let gap = 10.0;
    let size = Vec2::new(((w - gap) / 2.0).floor(), 60.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(gap);
        for c in CONSOLES {
            if widgets::choice_tile(ui, size, "puzzle", c.name(), "", app.state.recompile.console == c.id()).clicked() && app.state.recompile.console != c.id()
            {
                app.state.recompile = crate::state::Recompile { console: c.id().into(), ..Default::default() };
            }
        }
    });
    let Some(c) = craft_anything_engines::console::console(&app.state.recompile.console) else { return };
    let b = c.backend();
    ui.add_space(12.0);
    widgets::card(&t).show(ui, |ui| {
        ui.set_width(w - 34.0);
        ui.label(theme::heading(b.name, theme::CARD));
        ui.label(RichText::new(format!("{} · {} · {}", b.license, b.version, b.repository)).color(t.text_faint).size(theme::CAPTION));
        ui.add_space(4.0);
        ui.label(RichText::new(b.takes).color(t.text_dim).size(theme::BODY));
        ui.add_space(10.0);
        widgets::caption(ui, "Where it runs");
        for target in c.targets() {
            let (word, colour) = match target.support {
                Support::Runs => (tr("Runs"), t.success),
                Support::BuildsOnly => (tr("Builds only"), t.warning),
                Support::Unavailable => (tr("Not available"), t.text_faint),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} {}", target.os.name(), target.arch)).strong().size(theme::SMALL));
                widgets::pill(ui, &word, colour, t.raised);
                ui.label(RichText::new(target.note).color(t.text_dim).size(theme::SMALL));
            });
        }
        ui.add_space(6.0);
        widgets::caption(ui, "Its build needs");
        ui.label(RichText::new(b.needs.join(" · ")).color(t.text_dim).size(theme::SMALL));
    });
}

fn pick(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    if app.state.recompile.console.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        if widgets::primary_icon(ui, Some("hard-drive"), "Pick a disc image or executable", true).clicked() {
            app.pick(Pick::RecompileFile);
        }
        if widgets::secondary(ui, Some("folder-open"), "Pick an extracted folder").clicked() {
            app.pick(Pick::RecompileFolder);
        }
    });
    if let Some(input) = &app.state.recompile.input {
        ui.add_space(4.0);
        ui.label(RichText::new(input.display().to_string()).color(t.text_faint).size(theme::CAPTION));
    }
}

/// The verdict, the title, and every check, worst first.
fn result(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    let r = &app.state.recompile;
    if r.looking {
        widgets::working(ui, "Looking at the game…", None);
        return;
    }
    let Some(v) = &r.result else { return };
    let ready = v["verdict"] == "ready";
    widgets::card(&t).show(ui, |ui| {
        ui.set_width(w - 34.0);
        ui.horizontal(|ui| {
            icons::show(ui, if ready { "check" } else { "x" }, 20.0, if ready { t.success } else { t.danger });
            let title = v["title"].as_str().unwrap_or("Untitled game");
            ui.label(theme::heading(title, theme::CARD));
            if let Some(id) = v["title_id"].as_str() {
                ui.label(RichText::new(id).color(t.text_faint).size(theme::CAPTION));
            }
        });
        ui.label(
            RichText::new(tr(if ready { "Ready to recompile." } else { "Can't be recompiled yet: see what's missing below." }))
                .color(t.text_dim)
                .size(theme::BODY),
        );
        if let Some(main) = v["main_executable"].as_str() {
            ui.label(RichText::new(main).color(t.text_faint).size(theme::CAPTION));
        }
        ui.add_space(10.0);
        for c in v["checks"].as_array().into_iter().flatten() {
            let (icon, colour) = match c["status"].as_str().unwrap_or_default() {
                "pass" => ("check", t.success),
                "warn" => ("triangle-alert", t.warning),
                "fail" => ("x", t.danger),
                _ => ("info", t.text_dim),
            };
            ui.horizontal_top(|ui| {
                icons::show(ui, icon, 14.0, colour);
                ui.add(egui::Label::new(RichText::new(c["message"].as_str().unwrap_or_default()).size(theme::SMALL)).wrap());
            });
        }
    });
    if ready && r.status.is_none() {
        ui.add_space(14.0);
        start(app, ui);
    }
}

/// Where the build goes, and the button that starts it.
fn start(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let out = output(app);
    ui.horizontal(|ui| {
        icons::show(ui, "folder", 14.0, t.text_faint);
        ui.label(RichText::new(format!("{} {}", tr("Saved in"), out.display())).color(t.text_dim).size(theme::SMALL));
        if ui.link(tr("Choose a folder…")).clicked() {
            app.pick(Pick::RecompileOut);
        }
    });
    ui.add_space(8.0);
    if widgets::primary_icon(ui, Some("hammer"), "Recompile", !app.state.recompile.starting).clicked() {
        begin(app, out, None);
    }
    ui.label(
        RichText::new(tr("Downloads the recompiler's latest version and runs its build. The first build can take an hour."))
            .color(t.text_faint)
            .size(theme::CAPTION),
    );
}

/// The folder picked, or a new one beside the person's projects named after the game.
fn output(app: &CraftApp) -> PathBuf {
    let r = &app.state.recompile;
    r.out.clone().or_else(|| r.chosen.clone()).unwrap_or_else(|| {
        let title = r.result.as_ref().and_then(|v| v["title"].as_str()).unwrap_or("Game");
        let name: String = title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let console = craft_anything_engines::console::console(&r.console).map_or("", |c| c.name());
        app.settings.projects_dir().join(format!("{} ({console})", name.trim()))
    })
}

/// Start, resume, or (with `from`) run the build again from a stage.
fn begin(app: &mut CraftApp, out: PathBuf, from: Option<&str>) {
    let r = &mut app.state.recompile;
    let Some(input) = r.input.clone() else { return };
    r.out = Some(out.clone());
    r.starting = true;
    let mut params = json!({"console": r.console, "path": input.display().to_string(), "output": out.display().to_string()});
    if let Some(from) = from {
        params["from"] = json!(from);
    }
    app.act("game_recomp_start", params, tr("Recompile a console game"));
}

/// The build's stages, its log and what can be done with it. Polls the build while it runs.
fn progress(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    let r = &mut app.state.recompile;
    let Some(out) = r.out.clone() else { return };
    let running = r.status.as_ref().is_none_or(|s| s["running"] == true);
    if (running || r.status.is_none()) && !r.polling && r.last_poll.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
        r.polling = true;
        r.last_poll = Some(Instant::now());
        app.act("game_recomp_status", json!({"output": out.display().to_string(), "lines": 300}), tr("Show a recompile build"));
    }
    if running {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    let Some(s) = app.state.recompile.status.clone() else { return };
    let p = &s["project"];
    let stages = p["stages"].as_array().cloned().unwrap_or_default();
    let all_done = !stages.is_empty() && stages.iter().all(|x| x["status"] == "done");
    if !running && !app.state.recompile.starting && app.state.recompile.play_after {
        app.state.recompile.play_after = false;
        if all_done {
            play(app, &out);
        }
    }
    ui.add_space(14.0);
    widgets::card(&t).show(ui, |ui| {
        ui.set_width(w - 34.0);
        ui.horizontal(|ui| {
            ui.label(theme::heading(
                tr(if all_done {
                    "Recompiled"
                } else if running {
                    "Recompiling…"
                } else {
                    "Recompile stopped"
                }),
                theme::CARD,
            ));
            if let Some(c) = p["backend_commit"].as_str() {
                ui.label(RichText::new(format!("{} {}", tr("upstream"), c.get(..12).unwrap_or(c))).color(t.text_faint).size(theme::CAPTION));
            }
        });
        ui.add_space(8.0);
        for x in &stages {
            let (icon, colour) = match x["status"].as_str().unwrap_or_default() {
                "done" => ("check", t.success),
                "running" => ("loader", t.accent),
                "failed" => ("x", t.danger),
                "interrupted" => ("circle-stop", t.warning),
                _ => ("minus", t.text_faint),
            };
            ui.horizontal_top(|ui| {
                icons::show(ui, icon, 14.0, colour);
                ui.vertical(|ui| {
                    ui.label(RichText::new(stage_title(x["stage"].as_str().unwrap_or_default())).size(theme::SMALL).strong());
                    if let Some(m) = x["message"].as_str() {
                        ui.add(egui::Label::new(RichText::new(m).color(t.text_dim).size(theme::SMALL)).wrap());
                    }
                });
            });
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if running {
                if widgets::secondary(ui, Some("circle-stop"), "Stop").clicked() {
                    app.act("game_recomp_stop", json!({"output": out.display().to_string()}), tr("Stop a recompile build"));
                }
            } else if !all_done && widgets::primary_icon(ui, Some("refresh-cw"), "Resume", !app.state.recompile.starting).clicked() {
                begin(app, out.clone(), None);
            }
            if !running && p["play"].is_object() {
                if widgets::primary_icon(ui, Some("play"), "Play", true).clicked() {
                    play(app, &out);
                }
                if widgets::secondary(ui, Some("hammer"), "Rebuild and play")
                    .on_hover_text(tr("After the config changed: recompile, build and play again."))
                    .clicked()
                {
                    app.state.recompile.play_after = true;
                    begin(app, out.clone(), Some("recompile"));
                }
            }
            if widgets::secondary(ui, Some("folder-open"), "Open folder").clicked() {
                app.open_log(&out);
            }
            if widgets::secondary(ui, Some("file"), "Open log").clicked() {
                app.open_log(&out.join("recomp.log"));
            }
        });
        let log = s["log"].as_array().cloned().unwrap_or_default();
        if !log.is_empty() {
            ui.add_space(10.0);
            widgets::caption(ui, "Build log");
            egui::Frame::new().fill(t.code_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                    for l in log.iter().filter_map(Value::as_str) {
                        ui.label(RichText::new(l).font(theme::mono(12.0)).color(t.text_dim));
                    }
                });
            });
        }
    });
    played(app, ui, w, &out);
}

fn play(app: &mut CraftApp, out: &std::path::Path) {
    app.state.recompile.play = None;
    app.act("game_recomp_play", json!({"output": out.display().to_string()}), tr("Play a recompiled game"));
}

/// What the last Play said: running, or why the game stopped, with the fixes on offer.
fn played(app: &mut CraftApp, ui: &mut Ui, w: f32, out: &std::path::Path) {
    let t = Tokens::get(ui.ctx());
    let Some(v) = app.state.recompile.play.clone() else { return };
    ui.add_space(14.0);
    widgets::card(&t).show(ui, |ui| {
        ui.set_width(w - 34.0);
        if v["running"] == true {
            ui.horizontal(|ui| {
                icons::show(ui, "play", 16.0, t.success);
                ui.label(theme::heading(tr("The game is running."), theme::CARD));
            });
            return;
        }
        let secs = v["seconds"].as_f64().unwrap_or(0.0);
        ui.horizontal(|ui| {
            icons::show(ui, "triangle-alert", 16.0, t.danger);
            ui.label(theme::heading(crate::i18n::trf("The game stopped after {secs} s.", &[("secs", &format!("{secs:.1}"))]), theme::CARD));
        });
        let errors: Vec<&str> = v["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !errors.is_empty() {
            ui.add_space(8.0);
            egui::Frame::new().fill(t.code_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                for l in errors.iter().take(8) {
                    ui.add(egui::Label::new(RichText::new(*l).font(theme::mono(12.0)).color(t.text_dim)).wrap());
                }
            });
        }
        let functions: Vec<String> = v["asks"]["functions"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        let data: Vec<&str> = v["asks"]["data"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        ui.add_space(8.0);
        if !functions.is_empty() {
            ui.label(
                RichText::new(crate::i18n::trf(
                    "The recompiler missed {n} function(s) the game reached; they can be added for you.",
                    &[("n", &functions.len().to_string())],
                ))
                .size(theme::SMALL),
            );
        }
        if !data.is_empty() {
            ui.label(
                RichText::new(crate::i18n::trf(
                    "It also needs where these are in the game: {names}. The AI can look for them.",
                    &[("names", &data.join(", "))],
                ))
                .size(theme::SMALL),
            );
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if !functions.is_empty() && widgets::primary_icon(ui, Some("wrench"), "Add them and rebuild", true).clicked() {
                let r = &mut app.state.recompile;
                r.fixing = functions.len();
                r.play = None;
                for f in &functions {
                    app.act(
                        "game_recomp_config_set",
                        json!({"output": out.display().to_string(), "section": "functions", "key": f}),
                        tr("Fix a recompile build's config"),
                    );
                }
            }
            if widgets::secondary(ui, Some("message-square"), "Ask the AI to fix it").clicked() {
                let message = format!(
                    "The game stopped after {secs:.1} s. Its log says:\n{}\nIt asks for functions: {}; XDK data: {}. Please find and fix what's missing.",
                    errors.iter().take(20).copied().collect::<Vec<_>>().join("\n"),
                    if functions.is_empty() { "none".into() } else { functions.join(", ") },
                    if data.is_empty() { "none".into() } else { data.join(", ") },
                );
                app.send(&message);
            }
            if widgets::secondary(ui, Some("file"), "Open the game's log").clicked() {
                app.open_log(&out.join("play.log"));
            }
        });
    });
}

fn stage_title(id: &str) -> String {
    tr(match id {
        "validate" => "Game input and validation",
        "dependencies" => "Dependency installation",
        "extract" => "File extraction",
        "generate" => "Native project generation",
        "analyse" => "Executable analysis",
        "recompile" => "Recompilation",
        "compile" => "Compilation",
        "package" => "Packaging",
        _ => "Stage",
    })
}

/// A disc image, folder or executable was picked: look at it.
pub fn picked(app: &mut CraftApp, input: PathBuf) {
    let r = &mut app.state.recompile;
    r.input = Some(input.clone());
    r.result = None;
    r.status = None;
    r.out = None;
    r.looking = true;
    let params = json!({"console": r.console, "path": input.display().to_string()});
    app.act("game_original_inspect", params, tr("Check a console game"));
}

/// The build's folder was picked (used when the build starts).
pub fn out_picked(app: &mut CraftApp, out: PathBuf) {
    app.state.recompile.chosen = Some(out);
}

pub fn tool_done(app: &mut CraftApp, tool: &str, result: Result<Value, ToolError>) {
    let r = &mut app.state.recompile;
    match (tool, result) {
        ("game_original_inspect", Ok(v)) => {
            r.looking = false;
            r.result = Some(v);
        }
        ("game_recomp_start", Ok(_)) => {
            r.starting = false;
            r.status = None;
            r.last_poll = None;
        }
        ("game_recomp_status", Ok(v)) => {
            r.polling = false;
            r.status = Some(v);
        }
        ("game_recomp_status", Err(_)) => r.polling = false,
        ("game_recomp_stop", Ok(_)) => r.last_poll = None,
        ("game_recomp_play", Ok(v)) => r.play = Some(v),
        ("game_recomp_config_set", Ok(_)) => {
            r.fixing = r.fixing.saturating_sub(1);
            if r.fixing == 0
                && let Some(out) = r.out.clone()
            {
                r.play_after = true;
                begin(app, out, Some("recompile"));
            }
        }
        (_, Err(e)) => {
            r.looking = false;
            r.starting = false;
            r.fixing = 0;
            app.toast(e.for_people(), true);
        }
        _ => {}
    }
}
