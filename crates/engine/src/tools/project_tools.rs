//! Projects (what the person is making) as tools, so every client — the app, the CLI, an MCP
//! agent — can list them, read one with its conversation, and answer its questions without the
//! app. Ids follow docs/tool-ids.md.

use crate::feed;
use crate::inbox::{Answer, Inbox};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::workspace::Workspace;
use craft_anything_engines::assets::AssetProviders;
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::sync::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

/// Most feed items one call returns.
const MAX_ITEMS: usize = 200;

/// Expose project management tools with their parameter schemas and declared side effects.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: "project_list",
            title: "List projects",
            summary: "List the projects in every project folder from settings (where new projects go, then `project_folders`), or in one folder, newest first: name, folder, app folder, request, target, stack, status, the latest check and how many questions are waiting.",
            beginner: "Shows everything you're making.",
            toolset: Toolset::Projects,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ListParams>,
            run: project_list,
        },
        ToolSpec {
            id: "project_show",
            title: "Show a project",
            summary: "One project: its request, target, stack, status, checks, questions still waiting for an answer, and its conversation (paged with offset/limit, newest last).",
            beginner: "Shows one project and what happened in it.",
            toolset: Toolset::Projects,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ShowParams>,
            run: project_show,
        },
        ToolSpec {
            id: "project_inbox_answer",
            title: "Answer a project's question",
            summary: "Answer a question or approval a project is waiting on (an id from project_show): a choice or text for questions, allow (and always) for approvals. The answer reaches a running build at once and is kept in the project's conversation.",
            beginner: "Answers a question the builder asked you.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM.union(Effects::CHANGES_UI_STATE),
            requires: Requires::Nothing,
            input_schema: schema_of::<AnswerParams>,
            run: project_inbox_answer,
        },
        ToolSpec {
            id: "project_play",
            title: "Play a project's result",
            summary: "Start the finished game or app a project built (its program, a Unity or Unreal package, or a Godot project in Godot), in its own window. If it's already running, says so instead of starting a second copy, unless `another` is true. A start that quits within two seconds is reported with its exit code and last output. It runs the code the builder wrote, so it needs the person's consent. Websites use project_preview instead. Known limit on Windows: when the caller's output is a pipe, the started program inherits it, so a caller reading that output to its end waits until the program exits (#207).",
            beginner: "Opens the game or app that was made.",
            toolset: Toolset::Projects,
            effects: Effects::LAUNCHES_PROCESS.union(Effects::EXECUTES_FOREIGN_CODE),
            requires: Requires::Nothing,
            input_schema: schema_of::<PlayParams>,
            run: project_play,
        },
        ToolSpec {
            id: "project_preview",
            title: "Preview a project's website",
            summary: "Serve a project's built website on this computer only (loopback) and return its address. It keeps serving for this session; with `wait`, the call itself serves until it's cancelled (Ctrl+C in the CLI).",
            beginner: "Shows the website that was made.",
            toolset: Toolset::Projects,
            effects: Effects::MUTATES_SESSION,
            requires: Requires::Nothing,
            input_schema: schema_of::<PreviewParams>,
            run: project_preview,
        },
        ToolSpec {
            id: "project_open_folder",
            title: "Open a project's folder",
            summary: "Open the project's folder (or its app/ folder) in the file manager, or one of its pinned builds' folders.",
            beginner: "Opens the project's files.",
            toolset: Toolset::Projects,
            effects: Effects::LAUNCHES_PROCESS,
            requires: Requires::Nothing,
            input_schema: schema_of::<FolderParams>,
            run: project_open_folder,
        },
        ToolSpec {
            id: "project_rename",
            title: "Rename a project",
            summary: "Give a project a new name (its folder stays where it is).",
            beginner: "Renames something you're making.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<RenameParams>,
            run: project_rename,
        },
        ToolSpec {
            id: "project_duplicate",
            title: "Duplicate a project",
            summary: "Copy a project beside it under a new name, to try something else from where it is: what was built, the pictures of the original and the study come along; the conversation, questions and usage don't, and neither do dependencies or compiler output (the next build makes them again). An app edited in place is copied into the copy's app/ folder.",
            beginner: "Makes a copy to try something different.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<DuplicateParams>,
            run: project_duplicate,
        },
        ToolSpec {
            id: "project_delete",
            title: "Delete a project",
            summary: "Delete a project: everything Craft Anything made in its folder (its records, checks, and what was built in app/). The person's own files stay (an app edited in place, the files a project started from). Can't be undone, so `confirm` must be true; stop a build first.",
            beginner: "Deletes something you made. Your own files are kept.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<DeleteParams>,
            run: project_delete,
        },
        ToolSpec {
            id: "project_questions_set",
            title: "Choose what the builder's questions get",
            summary: "Choose whether the builder's questions (core_person_ask) in a project are shown to the person to answer (`ask`) or answered with \"decide yourself and say what you chose\" (`decide`); `default` follows the app's setting (`questions` in settings, which follows Auto mode until set). With no mode, says what applies now. Takes effect from the builder's next question, also mid-build.",
            beginner: "Chooses whether the builder asks you or decides by itself.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<QuestionsParams>,
            run: project_questions_set,
        },
        ToolSpec {
            id: "project_build_finish",
            title: "Call the build done",
            summary: "The person is happy with the build as it is (#210): its status becomes Ready and the conversation says they called it done, so a paused, stopped or failed build no longer offers to carry on. Everything so far is kept, and a new message carries on from here. It doesn't stop a build that is still running: pause it first.",
            beginner: "Keeps the build as it is and stops offering to carry on.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<FinishParams>,
            run: project_build_finish,
        },
        ToolSpec {
            id: "project_assets_budget",
            title: "Set a project's asset budget",
            summary: "Set the shared budget for a project's generated images, 3D models and audio (US dollars), or read it with no amount. Shows estimated spending and the remaining budget.",
            beginner: "Changes how much a project may spend on generated art.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<BudgetParams>,
            run: project_assets_budget,
        },
        ToolSpec {
            id: "project_assets_allow",
            title: "Let a project generate assets",
            summary: "Turn asset generation on or off for a project, or for one registered provider using provider. Omit allow to read the master switch and each provider's enabled state. Omitting provider preserves individual choices while setting the master switch. Images, 3D models and audio share the project's budget and require their provider's key. Off until the person turns it on.",
            beginner: "Chooses which services may generate pictures, 3D models and audio for your project.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<AllowAssetsParams>,
            run: project_assets_allow,
        },
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Its new name.
    pub name: String,
}

fn project_rename(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: RenameParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    ws.rename(&p.name)?;
    Ok(ToolOutput::Value(json!({"project": ws.root, "name": ws.manifest.name})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The copy's name (default: "<name> copy").
    #[serde(default)]
    pub name: Option<String>,
}

fn project_duplicate(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DuplicateParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let name = p.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("{} copy", ws.manifest.name));
    let copy = ws.duplicate(&name)?;
    Ok(ToolOutput::Value(json!({"project": copy.root, "name": copy.manifest.name})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Must be true: deleting can't be undone.
    #[serde(default)]
    pub confirm: bool,
}

fn project_delete(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DeleteParams = parse_params(params)?;
    if !p.confirm {
        return Err(ToolError::invalid("Deleting a project can't be undone.").with_remediation("Pass confirm: true to delete it."));
    }
    let ws = open(&p.project)?;
    let kept = ws.manifest.app_in_root || !ws.manifest.start_files.is_empty();
    ws.delete()?;
    Ok(ToolOutput::Value(json!({"deleted": p.project, "your_files_kept": kept})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BudgetParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The new budget in US dollars (0 to 10 000); leave out to only read it.
    #[serde(default)]
    pub budget_usd: Option<f64>,
}

/// Read or persist a bounded asset budget and report spending with a nonnegative remaining balance.
fn project_assets_budget(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: BudgetParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    let spent = ws.try_assets_spent()?;
    if let Some(b) = p.budget_usd {
        if !b.is_finite() || !(0.0..=crate::workspace::MAX_ASSET_BUDGET_USD).contains(&b) {
            return Err(ToolError::invalid(format!("budget_usd must be between 0 and {:.0}.", crate::workspace::MAX_ASSET_BUDGET_USD)));
        }
        ws.set_person(|m| m.asset_budget_usd = b)?;
    }
    Ok(ToolOutput::Value(json!({"budget_usd": ws.manifest.asset_budget_usd, "spent_usd": spent,
        "remaining_usd": (ws.manifest.asset_budget_usd - spent).max(0.0)})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowAssetsParams {
    /// The project's folder.
    pub project: PathBuf,
    /// true lets the builder generate assets, false stops it; leave out to only read it.
    #[serde(default)]
    pub allow: Option<bool>,
    /// One registered asset provider id (for example scenario or elevenlabs). Omit to read or
    /// set the master switch, preserving individual provider choices.
    #[serde(default)]
    pub provider: Option<String>,
}

/// Read or update asset permissions using the latest manifest and registered provider ids.
/// Individual changes preserve other enabled providers; the master switch retains stored choices.
fn project_assets_allow(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AllowAssetsParams = parse_params(params)?;
    let providers = ctx.session.ext.get::<AssetProviders>().unwrap_or_default();
    if let Some(id) = &p.provider
        && providers.get(id).is_none()
    {
        return Err(ToolError::invalid(format!("No asset provider called \"{id}\" is available.")));
    }
    let mut ws = open(&p.project)?;
    if let Some(allow) = p.allow {
        ws.set_person(|m| {
            if let Some(id) = &p.provider {
                // Work from the freshly merged manifest, including changes made during a build.
                let was_enabled = m.generate_assets;
                let mut enabled = if was_enabled || !allow {
                    m.asset_providers.clone().unwrap_or_else(|| providers.ids().into_iter().map(str::to_owned).collect())
                } else {
                    Vec::new()
                };
                enabled.retain(|other| other != id);
                if allow {
                    enabled.push(id.clone());
                }
                m.generate_assets = (was_enabled || allow) && !enabled.is_empty();
                m.asset_providers = Some(enabled);
            } else {
                m.generate_assets = allow;
            }
        })?;
    }
    let statuses: Vec<_> = providers.ids().into_iter().map(|id| json!({"id": id, "enabled": ws.manifest.asset_provider_allowed(id)})).collect();
    Ok(ToolOutput::Value(json!({"generate_assets": ws.manifest.generate_assets,
        "asset_providers": ws.manifest.asset_providers, "providers": statuses})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectParams {
    /// The project's folder.
    pub project: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Serve until the call is cancelled instead of returning at once (for the CLI).
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Open the app/ folder (what was built) instead of the project folder.
    #[serde(default)]
    pub app: bool,
    /// Open a pinned build's folder.
    #[serde(default)]
    pub pin: Option<u32>,
}

/// Websites being previewed in this session, by project folder (kept serving until it ends).
#[derive(Default)]
struct Previews(Mutex<HashMap<PathBuf, craft_anything_platform::serve::StaticServer>>);

/// What a project's questions should get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionsChoice {
    Ask,
    Decide,
    /// The app's setting.
    Default,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishParams {
    /// The project's folder.
    pub project: PathBuf,
}

/// What the conversation and the project say when the person calls a build done.
pub const CALLED_DONE: &str = "Ready: you called it done. Everything so far is kept; a new message carries on from here.";

fn project_build_finish(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    use crate::workspace::{Status, Step};
    let p: FinishParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    ws.set_status(Status::Ready, CALLED_DONE)?;
    let mut items = feed::load(&ws);
    items.push(feed::FeedItem::Step { step: Step::Done, text: CALLED_DONE.into() });
    feed::save(&ws.root, &items)?;
    Ok(ToolOutput::Value(json!({"project": ws.root, "status": ws.manifest.status})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionsParams {
    /// The project's folder.
    pub project: PathBuf,
    /// `ask`, `decide` or `default`; none just reads what applies.
    #[serde(default)]
    pub mode: Option<QuestionsChoice>,
}

fn project_questions_set(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    use crate::workspace::Questions;
    let p: QuestionsParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    if let Some(mode) = p.mode {
        let chosen = match mode {
            QuestionsChoice::Ask => Some(Questions::Ask),
            QuestionsChoice::Decide => Some(Questions::Decide),
            QuestionsChoice::Default => None,
        };
        ws.set_person(|m| m.questions = chosen)?;
    }
    let settings = &ctx.session.settings;
    Ok(ToolOutput::Value(json!({
        "project": ws.root,
        "questions": settings.questions_in(Some(&ws.manifest)),
        "chosen": ws.manifest.questions,
        "app_default": settings.questions(),
    })))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Start another copy even if one is running.
    #[serde(default)]
    pub another: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// Only this folder (default: every project folder from settings).
    #[serde(default)]
    pub projects_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowParams {
    /// The project's folder.
    pub project: PathBuf,
    /// Skip this many conversation items from the start.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Return at most this many items (default and maximum 200; the newest when no offset).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The question's or approval's id.
    pub id: String,
    /// The chosen suggestion (questions).
    #[serde(default)]
    pub choice: Option<String>,
    /// A typed answer (questions), or a reason (approvals).
    #[serde(default)]
    pub text: Option<String>,
    /// Allow the action (approvals).
    #[serde(default)]
    pub allow: Option<bool>,
    /// Allow this kind of action for the rest of the project (approvals).
    #[serde(default)]
    pub always: bool,
}

fn open(project: &std::path::Path) -> Result<Workspace, ToolError> {
    Workspace::open(project).map_err(|e| {
        let why = e.message.clone();
        ToolError::new(ErrorCode::NotFound, format!("{} isn't a project folder: {why}", project.display()))
            .with_remediation("Pass a folder from project_list (it holds craft.json).")
    })
}

fn summary(ws: &Workspace) -> Value {
    let m = &ws.manifest;
    json!({
        "name": m.name,
        "folder": ws.root.display().to_string(),
        "app_folder": ws.app_dir().display().to_string(),
        "request": m.request,
        "target": m.target,
        "stack": m.stack,
        "status": m.status,
        "status_label": m.status.label(),
        "note": m.note,
        "updated_ms": m.updated_ms,
        "last_check": m.checks.last(),
        "waiting": Inbox::new(ws.inbox_dir()).pending().len(),
        "usage": ws.usage(),
    })
}

fn project_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ListParams = parse_params(params)?;
    let settings = &ctx.session.settings;
    let locations = p.projects_dir.map_or_else(|| settings.project_locations(), |d| vec![d]);
    let projects: Vec<Value> = Workspace::list_in(&locations).iter().map(summary).collect();
    Ok(ToolOutput::Value(json!({
        "projects_dir": settings.projects_dir().display().to_string(),
        "locations": locations.iter().map(|l| l.display().to_string()).collect::<Vec<_>>(),
        "count": projects.len(),
        "projects": projects,
    })))
}

fn project_show(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ShowParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let items = feed::load(&ws);
    let limit = p.limit.unwrap_or(MAX_ITEMS).clamp(1, MAX_ITEMS);
    let offset = p.offset.unwrap_or_else(|| items.len().saturating_sub(limit)).min(items.len());
    let page: Vec<&feed::FeedItem> = items.iter().skip(offset).take(limit).collect();
    let mut out = summary(&ws);
    if let Some(o) = out.as_object_mut() {
        o.insert("checks".into(), json!(ws.manifest.checks));
        o.insert("last_reply".into(), json!(ws.manifest.last_reply));
        o.insert("questions".into(), json!(Inbox::new(ws.inbox_dir()).pending()));
        o.insert("conversation".into(), json!({"total": items.len(), "offset": offset, "items": page}));
    }
    Ok(ToolOutput::Value(out))
}

fn project_inbox_answer(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AnswerParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let inbox = Inbox::new(ws.inbox_dir());
    // Taken before it's answered: the session that asked may read and remove it right after.
    let Some(question) = inbox.pending().into_iter().find(|r| r.id == p.id) else {
        return Err(ToolError::new(ErrorCode::NotFound, format!("No question {} is waiting in this project.", p.id))
            .with_remediation("Use project_show to see the questions still waiting."));
    };
    let answer = Answer { choice: p.choice, text: p.text, allow: p.allow, always: p.always };
    let shown = match answer.allow {
        Some(true) if answer.always => "Always allowed".to_string(),
        Some(true) => "Allowed".to_string(),
        Some(false) => "Not allowed".to_string(),
        None => answer.as_text(),
    };
    inbox.answer(&p.id, &answer)?;
    // Into the saved conversation; a running build keeps answers it finds there when it saves.
    if let Err(e) = feed::record_answer(&ws, &question, &shown) {
        log::warn!("answer not added to the conversation: {}", e.message);
    }
    Ok(ToolOutput::Value(json!({"answered": p.id, "shown": shown})))
}

fn project_play(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: PlayParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let app = ws.app_dir();
    if crate::outputs::site_dir(&app).is_some() {
        return Err(ToolError::new(ErrorCode::NotSupported, "This project made a website.").with_remediation("Use project_preview to see it."));
    }
    let out = super::pin_tools::start(ctx, &app, &crate::pins::play_log(&ws), p.another, "the finished project")?;
    Ok(ToolOutput::Value(out))
}

fn project_preview(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: PreviewParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let Some(site) = crate::outputs::site_dir(&ws.app_dir()) else {
        return Err(ToolError::new(ErrorCode::NotFound, "There's no built website in this project yet.")
            .with_remediation("Finish a build first (rebuild); a game or app is started with project_play."));
    };
    let previews = match ctx.session.ext.get::<Previews>() {
        Some(p) => p,
        None => {
            let p = Arc::new(Previews::default());
            ctx.session.ext.insert(p.clone());
            p
        }
    };
    let url = {
        let mut map = previews.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(server) = map.get(&ws.root) {
            server.url()
        } else {
            let server = craft_anything_platform::serve::serve_dir(&site).map_err(|e| ToolError::io(site.display(), e))?;
            let url = server.url();
            map.insert(ws.root.clone(), server);
            url
        }
    };
    if p.wait {
        ctx.report(None, format!("Serving {url} (stop to end)"));
        while !ctx.cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        previews.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&ws.root);
    }
    Ok(ToolOutput::Value(json!({"url": url, "site": site.display().to_string()})))
}

fn project_open_folder(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: FolderParams = parse_params(params)?;
    let ws = open(&p.project)?;
    let dir = folder_to_open(&ws, p.app, p.pin)?;
    craft_anything_platform::open_path(&dir)?;
    Ok(ToolOutput::Value(json!({"opened": dir.display().to_string()})))
}

/// The folder `project_open_folder` opens: the project, its app, or a pinned build's copy.
fn folder_to_open(ws: &Workspace, app: bool, pin: Option<u32>) -> Result<PathBuf, ToolError> {
    Ok(match (pin, app) {
        (Some(id), app) => {
            let pin = crate::pins::get(ws, id)?;
            if app { pin.app_dir(ws) } else { pin.dir(ws) }
        }
        (None, true) => ws.app_dir(),
        (None, false) => ws.root.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::RequestKind;
    use crate::workspace::TargetRef;
    use crate::{CallOptions, Caller, Engine};

    /// #210: "enough now". A paused build becomes Ready, says so in its conversation, and keeps
    /// everything it had.
    #[test]
    fn a_build_called_done_is_ready_and_says_so() {
        use crate::workspace::{Status, Step};
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "Site", "A website", TargetRef::None).unwrap();
        ws.set_status(Status::Stopped, "Paused.").unwrap();
        let e = Engine::new(crate::testing::session());
        let r = e.call("project_build_finish", json!({"project": ws.root}), CallOptions::new(Caller::Cli)).unwrap().to_json();
        assert_eq!(r["result"]["status"], "ready", "{r}");
        let ws = Workspace::open(&ws.root).unwrap();
        assert_eq!((ws.manifest.status, ws.manifest.note.as_str()), (Status::Ready, CALLED_DONE));
        let items = feed::load(&ws);
        assert!(
            matches!(items.as_slice(), [feed::FeedItem::User(a), feed::FeedItem::Step { step: Step::Done, text }] if a.text() == "A website" && text == CALLED_DONE),
            "{items:?}"
        );
        let err = e.call("project_build_finish", json!({"project": d.path().join("nope")}), CallOptions::new(Caller::Cli)).unwrap_err();
        assert!(err.message.contains("isn't a project folder"), "{err:?}");
    }

    #[test]
    fn list_show_and_answer_a_project_without_the_app() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
        let q = Inbox::new(ws.inbox_dir()).post(RequestKind::Question, "Blue or red?", "", vec!["Blue".into(), "Red".into()], true, None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |id: &str, p: Value| e.call(id, p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());

        let list = call("project_list", json!({"projects_dir": d.path()})).unwrap();
        assert_eq!(list["count"], 1);
        assert_eq!(list["projects"][0]["name"], "Snake");
        assert_eq!(list["projects"][0]["waiting"], 1);

        let show = call("project_show", json!({"project": ws.root})).unwrap();
        assert_eq!(show["questions"][0]["id"], q.as_str());
        assert_eq!(show["conversation"]["items"][0]["User"], "A snake game");

        let r = call("project_inbox_answer", json!({"project": ws.root, "id": q, "choice": "Blue"})).unwrap();
        assert_eq!(r["shown"], "Blue");
        assert!(Inbox::new(ws.inbox_dir()).pending().is_empty(), "answered");
        let again = call("project_inbox_answer", json!({"project": ws.root, "id": q, "choice": "Red"}));
        assert!(again.is_err(), "a question is answered once");
        assert!(call("project_show", json!({"project": d.path().join("nope")})).is_err());
    }

    /// Budget updates persist within the allowed range, and past spending never produces a negative balance.
    #[test]
    fn a_projects_asset_budget_is_read_and_set() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_budget", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert_eq!(call(json!({"project": ws.root})).unwrap()["budget_usd"], 5.0);
        assert_eq!(call(json!({"project": ws.root, "budget_usd": 12.5})).unwrap()["budget_usd"], 12.5);
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.asset_budget_usd, 12.5);
        assert!(call(json!({"project": ws.root, "budget_usd": -1})).is_err());
        assert!(call(json!({"project": ws.root, "budget_usd": 10001})).unwrap_err().message.contains("between 0 and 10000"));
        ws.record_asset(crate::workspace::AssetRecord { file: "sound.mp3".into(), kind: "sound".into(), prompt: "chime".into(), cost_usd: 2.25, at_ms: 1 })
            .unwrap();
        let out = call(json!({"project": ws.root, "budget_usd": 1.75})).unwrap();
        assert_eq!(out["budget_usd"], 1.75);
        assert_eq!(out["spent_usd"], 2.25);
        assert_eq!(out["remaining_usd"], 0.0);
        assert_eq!(call(json!({"project": ws.root, "budget_usd": 0.0})).unwrap()["budget_usd"], 0.0);
        assert_eq!(call(json!({"project": ws.root, "budget_usd": 10000.0})).unwrap()["remaining_usd"], 9997.75);
    }

    #[test]
    fn a_project_generates_assets_only_once_allowed() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_allow", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert_eq!(call(json!({"project": ws.root})).unwrap()["generate_assets"], false, "off until the person turns it on");
        assert_eq!(call(json!({"project": ws.root, "allow": true})).unwrap()["generate_assets"], true);
        assert!(Workspace::open(&ws.root).unwrap().manifest.generate_assets, "saved with the project");
        assert_eq!(call(json!({"project": ws.root, "allow": false})).unwrap()["generate_assets"], false);
        assert!(call(json!({"project": ws.root, "allow": "yes"})).is_err());
    }

    /// Provider toggles persist independently, while a master off/on cycle restores the selected providers.
    #[test]
    fn provider_switches_are_independent_persisted_and_preserved_by_the_master_switch() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_allow", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert!(call(json!({"project": ws.root, "provider": "missing", "allow": true})).is_err());
        assert!(!Workspace::open(&ws.root).unwrap().manifest.generate_assets);
        let on = call(json!({"project": ws.root, "provider": "scenario", "allow": true})).unwrap();
        assert_eq!(on["asset_providers"], json!(["scenario"]));
        assert_eq!(on["providers"].as_array().unwrap().iter().filter(|p| p["enabled"] == true).count(), 1);
        call(json!({"project": ws.root, "provider": "elevenlabs", "allow": true})).unwrap();
        call(json!({"project": ws.root, "allow": false})).unwrap();
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.asset_providers, Some(vec!["scenario".into(), "elevenlabs".into()]));
        call(json!({"project": ws.root, "allow": true})).unwrap();
        let manifest = Workspace::open(&ws.root).unwrap().manifest;
        assert!(manifest.asset_provider_allowed("scenario") && manifest.asset_provider_allowed("elevenlabs"));
        assert!(!manifest.asset_provider_allowed("fal"));
        call(json!({"project": ws.root, "provider": "scenario", "allow": false})).unwrap();
        assert_eq!(call(json!({"project": ws.root, "provider": "elevenlabs", "allow": false})).unwrap()["generate_assets"], false);
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.asset_providers, Some(Vec::new()));
        assert_eq!(call(json!({"project": ws.root, "provider": "fal"})).unwrap()["generate_assets"], false, "query does not mutate");
    }

    /// Editing a paused provider list never resumes its other providers; an explicit enable starts alone.
    #[test]
    fn paused_provider_changes_preserve_choices_without_resuming_them() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_allow", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        for (disabled, remaining) in [("scenario", vec!["elevenlabs"]), ("fal", vec!["scenario", "elevenlabs"])] {
            ws.set_person(|m| {
                m.generate_assets = false;
                m.asset_providers = Some(vec!["scenario".into(), "elevenlabs".into()]);
            })
            .unwrap();
            let paused = call(json!({"project": ws.root, "provider": disabled, "allow": false})).unwrap();
            assert_eq!(paused["generate_assets"], false);
            assert_eq!(paused["asset_providers"], json!(remaining));
            assert!(paused["providers"].as_array().unwrap().iter().all(|p| p["enabled"] == false));
            let resumed = call(json!({"project": ws.root, "allow": true})).unwrap();
            assert_eq!(resumed["generate_assets"], true);
            assert_eq!(resumed["asset_providers"], json!(remaining));
        }
        call(json!({"project": ws.root, "allow": false})).unwrap();
        let enabled = call(json!({"project": ws.root, "provider": "fal", "allow": true})).unwrap();
        assert_eq!(enabled["generate_assets"], true);
        assert_eq!(enabled["asset_providers"], json!(["fal"]));
    }

    /// A missing ledger means no spending; an unreadable ledger refuses reads and leaves budget writes untouched.
    #[test]
    fn budget_ledger_errors_do_not_partially_change_the_budget() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let call = |p: Value| e.call("project_assets_budget", p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
        assert_eq!(call(json!({"project": ws.root})).unwrap()["spent_usd"], 0.0);
        let ledger = ws.root.join("assets.json");
        for contents in ["not JSON", "{\"version\":999,\"assets\":[]}"] {
            std::fs::write(&ledger, contents).unwrap();
            assert!(call(json!({"project": ws.root})).is_err());
            assert!(call(json!({"project": ws.root, "budget_usd": 1.0})).is_err());
            assert_eq!(Workspace::open(&ws.root).unwrap().manifest.asset_budget_usd, 5.0);
            assert_eq!(std::fs::read_to_string(&ledger).unwrap(), contents);
        }
        std::fs::remove_file(&ledger).unwrap();
        std::fs::create_dir(&ledger).unwrap();
        assert!(call(json!({"project": ws.root, "budget_usd": 1.0})).is_err());
        assert_eq!(Workspace::open(&ws.root).unwrap().manifest.asset_budget_usd, 5.0);
    }

    /// Restricting legacy all-provider permission must retain every other provider, including extensions.
    #[test]
    fn disabling_a_legacy_provider_keeps_other_registered_providers_enabled() {
        use craft_anything_engines::assets::{AssetProvider, AssetRequest, Generated};
        struct Custom;
        impl AssetProvider for Custom {
            /// Register a provider outside the built-in catalogue to exercise permission preservation.
            fn id(&self) -> &'static str {
                "custom"
            }
            /// Supply a display label for the custom permission-test provider.
            fn name(&self) -> &'static str {
                "Custom"
            }
            /// Fail any generation attempt because this fixture only tests permission changes.
            fn generate(&self, _: &str, _: &AssetRequest) -> Result<Generated, ToolError> {
                Err(ToolError::invalid("test provider must not generate"))
            }
        }
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "App", "An app", TargetRef::None).unwrap();
        ws.set_person(|m| m.generate_assets = true).unwrap();
        let mut providers = AssetProviders::built_in();
        providers.register(Arc::new(Custom));
        let mut session = crate::testing::session();
        session.ext.insert(Arc::new(providers));
        let e = Engine::new(session);
        e.call("project_assets_allow", json!({"project": ws.root, "provider": "scenario", "allow": false}), CallOptions::new(Caller::Cli)).unwrap();
        let manifest = Workspace::open(&ws.root).unwrap().manifest;
        assert!(!manifest.asset_provider_allowed("scenario"));
        for id in ["fal", "elevenlabs", "custom"] {
            assert!(manifest.asset_provider_allowed(id));
        }
        e.call("project_assets_allow", json!({"project": ws.root, "provider": "custom", "allow": false}), CallOptions::new(Caller::Cli)).unwrap();
        assert!(!Workspace::open(&ws.root).unwrap().manifest.asset_provider_allowed("custom"));
    }

    #[test]
    fn a_built_site_is_previewed_and_nothing_to_play_says_so() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Site", "A page", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let granted = || CallOptions::new(Caller::Cli).with_consent(crate::Consent::Granted);
        let call = |id: &str, p: Value| e.call(id, p, granted()).map(|o| o.to_json()["result"].clone());
        assert!(call("project_preview", json!({"project": ws.root})).is_err(), "nothing built yet");
        assert!(call("project_play", json!({"project": ws.root})).is_err(), "nothing to start yet");
        std::fs::create_dir_all(ws.app_dir().join("dist")).unwrap();
        std::fs::write(ws.app_dir().join("dist").join("index.html"), b"<h1>hi</h1>").unwrap();
        let first = call("project_preview", json!({"project": ws.root})).unwrap();
        let url = first["url"].as_str().unwrap().to_string();
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert_eq!(call("project_preview", json!({"project": ws.root})).unwrap()["url"], url.as_str(), "one server per project");
        let err = call("project_play", json!({"project": ws.root})).unwrap_err();
        assert!(err.remediation.unwrap_or_default().contains("project_preview"));
        // Playing runs the builder's code: refused without consent.
        assert!(e.call("project_play", json!({"project": ws.root}), CallOptions::new(Caller::Mcp)).is_err());
    }

    #[test]
    fn a_pinned_builds_folder_is_opened_from_its_project() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
        let pin = ws.root.join(crate::pins::PINNED).join("7");
        std::fs::create_dir_all(pin.join("app")).unwrap();
        std::fs::write(pin.join(crate::pins::PIN_FILE), r#"{"version":1,"id":7,"name":"Pinned","mode":"frozen","created_ms":0}"#).unwrap();
        assert_eq!(folder_to_open(&ws, false, Some(7)).unwrap(), pin);
        assert_eq!(folder_to_open(&ws, true, Some(7)).unwrap(), pin.join("app"));
        assert_eq!(folder_to_open(&ws, false, None).unwrap(), ws.root);
        assert_eq!(folder_to_open(&ws, true, None).unwrap(), ws.app_dir());
        assert!(folder_to_open(&ws, false, Some(8)).is_err(), "no such pin");
    }
}
