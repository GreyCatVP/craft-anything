//! A short scripted session with a program: start it, press keys and click in its window as a
//! script says, picture the window along the way, and close it (ROADMAP R2.4). For games and apps
//! that have no accessibility tree to drive (`runtime_ui_walkthrough_record` covers those that
//! do), so a rebuild can be compared with how the original looks after the same input.
//!
//! Input goes to whichever window is in front, so nothing is sent unless the program's own window
//! is in front, checked again before every key, click and pointer move and while a key is held.
//! When it isn't (the person clicked elsewhere, a dialog opened, the program closed), the session
//! stops there, releases what it holds and says which step it stopped at.

use super::{check_paths, process_tree, start_and_wait};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::tools::web_common::{Image, encode_png, png_artifact};
use craft_anything_evidence::{Confidence, Coverage, CoverageStatus, ErrorCode, EvidenceBuilder, Limitation, Location, Subject, ToolError};
use craft_anything_platform::input::{Button, InputSink, Key};
use craft_anything_platform::window::{WindowPlace, WindowShot};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The tool's id (docs/tool-ids.md).
pub(crate) const ID: &str = "native_input_record";
/// Most steps in one script.
const MAX_STEPS: usize = 100;
/// Most pictures a script may ask for (the start and the end are taken as well).
const MAX_PICTURES: usize = 20;
/// Longest single wait or key hold.
const MAX_STEP_MS: u64 = 10_000;
/// Longest the script's waits and holds may add up to.
const MAX_SCRIPT_MS: u64 = 120_000;
/// Longest text one step types, in characters.
const MAX_TEXT: usize = 200;
/// Most keys held together.
const MAX_CHORD: usize = 4;
/// How long a key is held when the step doesn't say.
const DEFAULT_HOLD_MS: u64 = 80;
/// How long a mouse button is held for a click.
const CLICK_MS: u64 = 40;
/// How often a wait or a hold looks again at the window, the program and cancel.
const SLICE: Duration = Duration::from_millis(50);
/// How long to wait for the program's window to come to the front before the first input.
const FOCUS_WAIT: Duration = Duration::from_secs(10);
/// How far (in screen points) the pointer may land from where it was aimed and still click.
const POINTER_SLACK: u32 = 2;
/// Most bytes of pictures kept inline in the evidence (all are written to `output_dir`).
const MAX_INLINE: usize = 8 * 1024 * 1024;

pub(super) fn spec() -> ToolSpec {
    ToolSpec {
        id: ID,
        title: "Play a short script in a program",
        summary: "Start a program (an app or a game), wait for its window, then play a short script of key presses, typing, pointer moves and clicks into it, taking pictures of its window at the start, at each `picture` step and at the end, and close it (Windows and macOS). Click and move positions are pixels in the window's picture. Input is sent only while the program's own window is in front: if it isn't, the session stops at that step and says why. Pictures are saved as PNGs in `output_dir` when given. It runs the program and takes over the keyboard and mouse for the script's length, so it needs consent; on macOS it also needs Accessibility and Screen Recording permission.",
        beginner: "Opens a program, presses keys and clicks in it as a short script says, and takes pictures of what it shows. Don't use the keyboard or mouse while it plays.",
        toolset: Toolset::Runtime,
        effects: Effects::LAUNCHES_PROCESS.union(Effects::EXECUTES_FOREIGN_CODE).union(Effects::WRITES_FILESYSTEM).union(Effects::MUTATES_SESSION),
        requires: Requires::Os(&["windows", "macos"]),
        input_schema: schema_of::<SessionParams>,
        run: native_input_record,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionParams {
    /// The program to start (an absolute path).
    pub program: PathBuf,
    /// Arguments for it.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working folder for the program (an absolute directory). Defaults to its executable's folder.
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    /// Seconds to let it start and draw before the script begins (default 8, at most 120).
    #[serde(default)]
    pub wait_secs: Option<u64>,
    /// What to do, in order (at most 100 steps; waits and holds at most 120 s in all).
    pub steps: Vec<Step>,
    /// Save every picture here as `NN-label.png` (an absolute path; it's created if missing).
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
}

/// One step of a script, named by `do`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Do nothing for `ms` milliseconds (at most 10000).
    Wait { ms: u64 },
    /// Press these keys together, hold them for `hold_ms` (default 80, at most 10000) and release
    /// them: one letter or digit (`w`, `7`), `space`, `enter`, `escape`, `tab`, `backspace`,
    /// `delete`, `up`, `down`, `left`, `right`, `home`, `end`, `page_up`, `page_down`, `shift`,
    /// `control`, `alt`, `meta`, `f1`–`f12`. At most 4 keys.
    Keys {
        keys: Vec<String>,
        #[serde(default)]
        hold_ms: Option<u64>,
    },
    /// Type text as characters (at most 200, no line breaks or tabs: use a `keys` step).
    Type { text: String },
    /// Click at a pixel of the window's picture (`left` unless `button` says `right` or `middle`).
    Click {
        x: u32,
        y: u32,
        #[serde(default)]
        button: Option<ButtonName>,
    },
    /// Move the pointer to a pixel of the window's picture.
    Move { x: u32, y: u32 },
    /// Take a picture of the window now, named `label` (letters, digits, `-` and `_`; at most 40).
    Picture {
        #[serde(default)]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ButtonName {
    Left,
    Right,
    Middle,
}

/// A step once checked: keys resolved, times bounded, labels unique.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Wait(Duration),
    Keys(Vec<Key>, Duration),
    Type(String),
    Click(u32, u32, Button),
    Move(u32, u32),
    Picture(String),
}

impl Action {
    fn kind(&self) -> &'static str {
        match self {
            Action::Wait(_) => "wait",
            Action::Keys(..) => "keys",
            Action::Type(_) => "type",
            Action::Click(..) => "click",
            Action::Move(..) => "move",
            Action::Picture(_) => "picture",
        }
    }
}

/// Check a whole script before anything starts, and say which step is wrong.
fn plan(steps: &[Step]) -> Result<Vec<Action>, ToolError> {
    if steps.is_empty() {
        return Err(ToolError::invalid("steps is empty").with_remediation("Give at least one step, e.g. {\"do\": \"keys\", \"keys\": [\"space\"]}."));
    }
    if steps.len() > MAX_STEPS {
        return Err(ToolError::invalid(format!("steps has {} steps; at most {MAX_STEPS}", steps.len())).with_remediation("Split the script into shorter runs."));
    }
    let mut total: u64 = 0;
    let mut labels: Vec<String> = vec!["start".into(), "end".into()];
    let mut actions = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let n = i.saturating_add(1);
        let bad = |what: String| ToolError::invalid(format!("step {n}: {what}"));
        let action = match step {
            Step::Wait { ms } => {
                if *ms > MAX_STEP_MS {
                    return Err(bad(format!("a wait is at most {MAX_STEP_MS} ms")));
                }
                total = total.saturating_add(*ms);
                Action::Wait(Duration::from_millis(*ms))
            }
            Step::Keys { keys, hold_ms } => {
                if keys.is_empty() || keys.len() > MAX_CHORD {
                    return Err(bad(format!("give 1 to {MAX_CHORD} keys")));
                }
                let mut resolved: Vec<Key> = Vec::with_capacity(keys.len());
                for k in keys {
                    let key = Key::named(k).ok_or_else(|| {
                        bad(format!("\"{k}\" isn't a key")).with_remediation(
                            "Use one letter or digit, or space, enter, escape, tab, backspace, delete, up, down, left, right, home, end, page_up, page_down, shift, control, alt, meta, f1-f12.",
                        )
                    })?;
                    if resolved.contains(&key) {
                        return Err(bad(format!("\"{k}\" is given twice")));
                    }
                    resolved.push(key);
                }
                let hold = hold_ms.unwrap_or(DEFAULT_HOLD_MS);
                if hold > MAX_STEP_MS {
                    return Err(bad(format!("a key is held at most {MAX_STEP_MS} ms")));
                }
                total = total.saturating_add(hold);
                Action::Keys(resolved, Duration::from_millis(hold))
            }
            Step::Type { text } => {
                if text.is_empty() || text.chars().count() > MAX_TEXT {
                    return Err(bad(format!("type 1 to {MAX_TEXT} characters")));
                }
                if text.chars().any(char::is_control) {
                    return Err(bad("text can't hold line breaks, tabs or other control characters".into())
                        .with_remediation("Press enter or tab with a keys step instead."));
                }
                Action::Type(text.clone())
            }
            Step::Click { x, y, button } => {
                let b = match button.unwrap_or(ButtonName::Left) {
                    ButtonName::Left => Button::Left,
                    ButtonName::Right => Button::Right,
                    ButtonName::Middle => Button::Middle,
                };
                total = total.saturating_add(CLICK_MS);
                Action::Click(*x, *y, b)
            }
            Step::Move { x, y } => Action::Move(*x, *y),
            Step::Picture { label } => {
                let label = match label {
                    Some(l) => {
                        if l.is_empty() || l.len() > 40 || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                            return Err(bad(format!("\"{l}\" can't be a picture's label")).with_remediation("Use 1 to 40 letters, digits, - or _."));
                        }
                        l.to_ascii_lowercase()
                    }
                    None => format!("step-{n}"),
                };
                if labels.contains(&label) {
                    return Err(bad(format!("the label \"{label}\" is used already (start and end are taken)")));
                }
                labels.push(label.clone());
                Action::Picture(label)
            }
        };
        actions.push(action);
    }
    let pictures = actions.iter().filter(|a| matches!(a, Action::Picture(_))).count();
    if pictures > MAX_PICTURES {
        return Err(ToolError::invalid(format!("the script takes {pictures} pictures; at most {MAX_PICTURES}")));
    }
    if total > MAX_SCRIPT_MS {
        return Err(ToolError::invalid(format!("the script's waits and holds add up to {total} ms; at most {MAX_SCRIPT_MS}"))
            .with_remediation("Split it into shorter runs."));
    }
    Ok(actions)
}

/// The script's waits and holds, for the run's time limit.
fn script_length(actions: &[Action]) -> Duration {
    actions.iter().fold(Duration::ZERO, |sum, a| match a {
        Action::Wait(d) | Action::Keys(_, d) => sum.saturating_add(*d),
        Action::Click(..) => sum.saturating_add(Duration::from_millis(CLICK_MS)),
        _ => sum,
    })
}

/// The running program as the session sees it. The tool's is the real window; tests use their own.
trait Screen {
    /// Where the program's window is and whether it's in front.
    fn place(&mut self) -> Result<WindowPlace, ToolError>;
    /// A picture of the program's window.
    fn picture(&mut self) -> Result<WindowShot, ToolError>;
    /// Whether the program is still running.
    fn running(&mut self) -> bool;
    /// Whether the person or caller asked to stop.
    fn cancelled(&self) -> bool;
    /// Let time pass.
    fn sleep(&mut self, d: Duration);
    /// Tell the person something while it plays.
    fn say(&mut self, message: String);
}

/// Why a session stopped before the end of its script.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stop {
    /// The step it stopped at (1-based; 0 is before the first step).
    step: usize,
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct Taken {
    label: String,
    /// The last step done before it (0: before the script).
    after_step: usize,
    at_ms: u128,
    shot: WindowShot,
}

#[derive(Debug, Default)]
struct Played {
    /// Steps done, in order.
    done: usize,
    pictures: Vec<Taken>,
    stop: Option<Stop>,
}

/// What's held down now, so it's always let go again, however the session ends.
#[derive(Default)]
struct Held {
    keys: Vec<Key>,
    buttons: Vec<Button>,
}

impl Held {
    fn release(&mut self, input: &mut dyn InputSink) {
        for k in self.keys.drain(..).rev() {
            if let Err(e) = input.key(k, false) {
                log::warn!("couldn't release {k:?}: {}", e.message);
            }
        }
        for b in self.buttons.drain(..).rev() {
            if let Err(e) = input.button(b, false) {
                log::warn!("couldn't release {b:?}: {}", e.message);
            }
        }
    }
}

/// Whether input may be sent now: the program runs, nobody cancelled and its window is in front.
fn ready(screen: &mut dyn Screen, step: usize) -> Result<WindowPlace, Stop> {
    if screen.cancelled() {
        return Err(Stop { step, code: "cancelled", message: "Stopped when asked to.".into() });
    }
    if !screen.running() {
        return Err(Stop { step, code: "program_exited", message: "The program closed.".into() });
    }
    let place =
        screen.place().map_err(|e| Stop { step, code: if e.code == ErrorCode::NotFound { "window_gone" } else { "window_unreadable" }, message: e.message })?;
    if !place.focused {
        return Err(Stop {
            step,
            code: "focus_lost",
            message: "The program's window wasn't in front, so nothing more was sent (input would have gone to another window).".into(),
        });
    }
    Ok(place)
}

/// Let `d` pass in slices, stopping early when the program closes, someone cancels or, when
/// `need_focus` (a key is held), the window leaves the front.
fn pause(screen: &mut dyn Screen, d: Duration, step: usize, need_focus: bool) -> Result<(), Stop> {
    let mut left = d;
    while !left.is_zero() {
        let now = left.min(SLICE);
        screen.sleep(now);
        left = left.saturating_sub(now);
        if need_focus {
            ready(screen, step)?;
        } else if screen.cancelled() {
            return Err(Stop { step, code: "cancelled", message: "Stopped when asked to.".into() });
        } else if !screen.running() {
            return Err(Stop { step, code: "program_exited", message: "The program closed.".into() });
        }
    }
    Ok(())
}

/// A pixel of the window's picture on the screen. The picture may be larger than the window's
/// screen size (a high-density display), so it's scaled by the two.
fn to_screen(place: &WindowPlace, picture: (u32, u32), x: u32, y: u32, step: usize) -> Result<(i32, i32), Stop> {
    let (pw, ph) = picture;
    if pw == 0 || ph == 0 || x >= pw || y >= ph {
        return Err(Stop { step, code: "outside_window", message: format!("({x}, {y}) is outside the window's {pw}×{ph} picture.") });
    }
    let sx = u64::from(x).saturating_mul(u64::from(place.width)) / u64::from(pw);
    let sy = u64::from(y).saturating_mul(u64::from(place.height)) / u64::from(ph);
    let off = |o: i32, s: u64| i32::try_from(s).ok().and_then(|s| o.checked_add(s));
    match (off(place.x, sx), off(place.y, sy)) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(Stop { step, code: "outside_window", message: "The window's position is out of range.".into() }),
    }
}

/// Move the pointer and make sure it landed where it was aimed (a click elsewhere could land in
/// another window).
fn aim(input: &mut dyn InputSink, at: (i32, i32), step: usize) -> Result<(), Stop> {
    let landed = input.pointer_to(at.0, at.1).map_err(|e| Stop { step, code: "input_failed", message: e.message })?;
    // `abs_diff` can't overflow: the system may report a pointer far off any window
    if landed.0.abs_diff(at.0) > POINTER_SLACK || landed.1.abs_diff(at.1) > POINTER_SLACK {
        return Err(Stop {
            step,
            code: "pointer_elsewhere",
            message: format!(
                "The pointer landed at ({}, {}) instead of ({}, {}), so nothing was clicked (display scaling may differ between screens).",
                landed.0, landed.1, at.0, at.1
            ),
        });
    }
    Ok(())
}

fn take(screen: &mut dyn Screen, label: &str, after_step: usize, started: Instant, step: usize) -> Result<Taken, Stop> {
    let shot =
        screen.picture().map_err(|e| Stop { step, code: if e.code == ErrorCode::NotFound { "window_gone" } else { "picture_failed" }, message: e.message })?;
    Ok(Taken { label: label.to_string(), after_step, at_ms: started.elapsed().as_millis(), shot })
}

/// Bring the window to the front once (the person can click it), then play every step.
fn play(actions: &[Action], screen: &mut dyn Screen, input: &mut dyn InputSink) -> Played {
    let started = Instant::now();
    let mut played = Played::default();
    let mut held = Held::default();
    let result = play_steps(actions, screen, input, &mut held, &mut played, started);
    held.release(input);
    if let Err(stop) = result {
        played.stop = Some(stop);
    }
    // The end, whatever happened, while the window is still there to picture.
    if screen.running()
        && let Ok(end) = take(screen, "end", played.done, started, played.done)
    {
        played.pictures.push(end);
    }
    played
}

fn play_steps(
    actions: &[Action],
    screen: &mut dyn Screen,
    input: &mut dyn InputSink,
    held: &mut Held,
    played: &mut Played,
    started: Instant,
) -> Result<(), Stop> {
    let start = take(screen, "start", 0, started, 0)?;
    let picture = (start.shot.width, start.shot.height);
    played.pictures.push(start);
    // The first input waits for the window to come to the front: it may still be opening, or
    // need the person's click.
    let mut asked = false;
    let mut waited = Duration::ZERO;
    loop {
        match ready(screen, 0) {
            Ok(_) => break,
            Err(stop) if stop.code == "focus_lost" && waited < FOCUS_WAIT => {
                if !asked {
                    screen.say("Waiting for the program's window to be in front: click it to bring it forward.".into());
                    asked = true;
                }
                let look_again = Duration::from_millis(250);
                screen.sleep(look_again);
                waited = waited.saturating_add(look_again);
            }
            Err(mut stop) => {
                if stop.code == "focus_lost" {
                    stop.message = "The program's window didn't come to the front, so nothing was sent.".into();
                }
                return Err(stop);
            }
        }
    }
    for (i, action) in actions.iter().enumerate() {
        let step = i.saturating_add(1);
        match action {
            Action::Wait(d) => pause(screen, *d, step, false)?,
            Action::Keys(keys, hold) => {
                for k in keys {
                    ready(screen, step)?;
                    input.key(*k, true).map_err(|e| Stop { step, code: "input_failed", message: e.message })?;
                    held.keys.push(*k);
                }
                pause(screen, *hold, step, true)?;
                held.release(input);
            }
            Action::Type(text) => {
                ready(screen, step)?;
                input.text(text).map_err(|e| Stop { step, code: "input_failed", message: e.message })?;
            }
            Action::Move(x, y) => {
                let place = ready(screen, step)?;
                aim(input, to_screen(&place, picture, *x, *y, step)?, step)?;
            }
            Action::Click(x, y, b) => {
                let place = ready(screen, step)?;
                aim(input, to_screen(&place, picture, *x, *y, step)?, step)?;
                // Checked again: moving the pointer may have raised another window.
                ready(screen, step)?;
                input.button(*b, true).map_err(|e| Stop { step, code: "input_failed", message: e.message })?;
                held.buttons.push(*b);
                pause(screen, Duration::from_millis(CLICK_MS), step, false)?;
                held.release(input);
            }
            Action::Picture(label) => played.pictures.push(take(screen, label, i, started, step)?),
        }
        played.done = step;
    }
    Ok(())
}

/// The real program: its window found by pid, the run's cancel and the tool's progress.
struct LiveScreen<'a, 'b> {
    ctx: &'a mut Ctx<'b>,
    running: &'a mut craft_anything_platform::process::RunningProcess,
    pid: u32,
}

impl Screen for LiveScreen<'_, '_> {
    fn place(&mut self) -> Result<WindowPlace, ToolError> {
        craft_anything_platform::window::window_place(self.pid)
    }
    fn picture(&mut self) -> Result<WindowShot, ToolError> {
        craft_anything_platform::window::capture_window(self.pid, None)
    }
    fn running(&mut self) -> bool {
        !self.running.is_finished()
    }
    fn cancelled(&self) -> bool {
        self.ctx.cancel.is_cancelled()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn say(&mut self, message: String) {
        self.ctx.report(None, message);
    }
}

fn native_input_record(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SessionParams = parse_params(params)?;
    check_paths(&p.program, p.working_directory.as_deref())?;
    let actions = plan(&p.steps)?;
    if let Some(dir) = &p.output_dir {
        if !dir.is_absolute() {
            return Err(ToolError::invalid("output_dir must be an absolute path"));
        }
        if dir.exists() && !dir.is_dir() {
            return Err(ToolError::invalid(format!("{} isn't a folder", dir.display())));
        }
    }
    // Refused before the program starts where nothing can be pictured or sent.
    if !craft_anything_platform::window::SUPPORTED {
        return Err(craft_anything_platform::window::unsupported());
    }
    if !craft_anything_platform::input::SUPPORTED {
        return Err(craft_anything_platform::input::unsupported());
    }
    craft_anything_platform::window::check_can_see_windows()?;
    let mut input = craft_anything_platform::input::system_input()?;
    if let Some(dir) = &p.output_dir {
        std::fs::create_dir_all(dir).map_err(|e| ToolError::io(format!("Couldn't create {}", dir.display()), e))?;
    }
    let wait = Duration::from_secs(p.wait_secs.unwrap_or(8).clamp(1, 120));
    let dir = p.working_directory.clone().unwrap_or_else(|| p.program.parent().map(PathBuf::from).unwrap_or_default());
    // Room for the start, the window to come forward, every picture and the close.
    let limit = wait + script_length(&actions) + FOCUS_WAIT + Duration::from_secs(120);
    // A launcher that starts the real program and exits cleanly doesn't end the run (its group
    // lives until the session is done), so the program it handed over to keeps its window.
    let (mut running, _) = start_and_wait(ctx, &p.program, &p.args, &dir, wait, limit, true)?;
    // The process that owns the window: the program or one it started (a runner, a launcher's
    // child), remembered across looks so a child stays the program once its launcher is gone.
    // Input is only ever aimed at that process's window.
    let mut family = process_tree::Family::new(running.pid());
    let late_until = Instant::now() + super::LATE_WINDOW;
    let found = process_tree::keep_looking(
        || {
            family.capture(
                || process_tree::snapshot(&ctx.cancel),
                |pid| {
                    if ctx.cancel.is_cancelled() {
                        return Err(ToolError::cancelled());
                    }
                    craft_anything_platform::window::window_place(pid)
                },
            )
        },
        || Instant::now() < late_until && !ctx.cancel.is_cancelled() && !running.is_finished(),
        super::LOOK_AGAIN,
    );
    let (pid, first) = match found {
        Ok(found) => found,
        Err(e) => {
            running.kill_and_wait(Duration::from_secs(10));
            return Err(e);
        }
    };
    ctx.report(None, format!("Playing {} steps in \"{}\"…", actions.len(), first.title));
    let played = {
        let mut screen = LiveScreen { ctx, running: &mut running, pid };
        play(&actions, &mut screen, input.as_mut())
    };
    drop(input);
    // Always closed again and confirmed before we return (a caller such as the CLI may exit next).
    if !running.kill_and_wait(Duration::from_secs(10)) {
        log::warn!("{} may still be running", p.program.display());
    }
    if played.stop.as_ref().is_some_and(|s| s.code == "cancelled") {
        return Err(ToolError::cancelled());
    }
    evidence(ctx, &p, &dir, &actions, &first, played)
}

fn evidence(
    ctx: &mut Ctx<'_>,
    p: &SessionParams,
    dir: &std::path::Path,
    actions: &[Action],
    window: &WindowPlace,
    played: Played,
) -> Result<ToolOutput, ToolError> {
    let mut pictures = Vec::with_capacity(played.pictures.len());
    let mut inline_bytes = 0usize;
    let mut locations = Vec::new();
    for (n, taken) in played.pictures.into_iter().enumerate() {
        let png = encode_png(&Image { width: taken.shot.width, height: taken.shot.height, rgba: taken.shot.rgba })?;
        let mut entry =
            json!({"label": taken.label, "after_step": taken.after_step, "at_ms": taken.at_ms, "width": taken.shot.width, "height": taken.shot.height});
        if let Some(out) = &p.output_dir {
            let path = out.join(format!("{:02}-{}.png", n, taken.label));
            craft_anything_platform::fs::write_atomic(&path, &png)?;
            locations.push(Location::new("path", path.to_string_lossy()));
            entry["path"] = json!(path.display().to_string());
        }
        if inline_bytes.saturating_add(png.len()) <= MAX_INLINE
            && let Ok(artifact) = png_artifact(&png)
        {
            inline_bytes = inline_bytes.saturating_add(png.len());
            entry["screenshot"] = artifact;
        }
        pictures.push(entry);
    }
    let steps: Vec<Value> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let step = i.saturating_add(1);
            let status = if step <= played.done {
                "done"
            } else if played.stop.as_ref().is_some_and(|s| s.step == step) {
                "stopped"
            } else {
                "not_run"
            };
            json!({"step": step, "do": a.kind(), "status": status})
        })
        .collect();
    let total = actions.len();
    let mut result = json!({
        "window": {"title": window.title, "width": window.width, "height": window.height},
        "steps_done": played.done,
        "steps": steps,
        "pictures": pictures,
    });
    if let Some(stop) = &played.stop {
        result["stopped"] = json!({"step": stop.step, "code": stop.code, "message": stop.message});
    }
    let summary = match &played.stop {
        None => format!("Played all {total} steps in \"{}\" and took {} pictures.", window.title, pictures.len()),
        Some(stop) => format!("Played {} of {total} steps in \"{}\", then stopped at step {}: {}", played.done, window.title, stop.step, stop.message),
    };
    let coverage = if played.stop.is_none() {
        Coverage::complete(total as u64)
    } else {
        Coverage { status: CoverageStatus::Partial, examined: Some(played.done as u64), total: Some(total as u64), next_offset: None }
    };
    let name = p.program.file_stem().map(|s| s.to_string_lossy().into_owned());
    let mut b = EvidenceBuilder::new(ID, Subject { kind: "program".into(), sha256: None, name }, crate::tools::artifact_tools::env(ctx))
        .params(json!({"program": p.program.display().to_string(), "args": p.args, "working_directory": dir, "steps": total}))
        .result(result)
        .summary(summary)
        .confidence(Confidence::Observed)
        .coverage(coverage)
        .limitation(Limitation::new(
            "simulated_input",
            "Keys and clicks were sent as the system's own input. A program that reads its devices directly, or runs with more rights than this app, may ignore them: compare the pictures to see whether anything changed.",
        ))
        .limitation(Limitation::new("pictures_only", "The pictures show what the window drew after each step, not why; timing depends on how fast this computer ran the program."));
    for l in locations {
        b = b.location(l);
    }
    let ev = b.build().map_err(|e| ToolError::new(ErrorCode::Internal, e.message))?;
    Ok(ToolOutput::evidence(ev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn steps(v: Value) -> Vec<Step> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_script_is_checked_whole_before_anything_starts() {
        let ok = plan(&steps(json!([
            {"do": "wait", "ms": 500},
            {"do": "keys", "keys": ["shift", "W"], "hold_ms": 1500},
            {"do": "type", "text": "hello"},
            {"do": "click", "x": 10, "y": 20, "button": "right"},
            {"do": "move", "x": 1, "y": 2},
            {"do": "picture", "label": "After-Run"},
            {"do": "picture"},
        ])))
        .unwrap();
        assert_eq!(ok[1], Action::Keys(vec![Key::Shift, Key::Char('w')], Duration::from_millis(1500)));
        assert_eq!(ok[3], Action::Click(10, 20, Button::Right));
        assert_eq!(ok[5], Action::Picture("after-run".into()));
        assert_eq!(ok[6], Action::Picture("step-7".into()));
        assert_eq!(script_length(&ok), Duration::from_millis(500 + 1500 + CLICK_MS));

        let refused = |v: Value| plan(&steps(v)).unwrap_err().message;
        assert!(refused(json!([])).contains("empty"));
        assert!(refused(json!([{"do": "wait", "ms": 10_001}])).contains("step 1"));
        assert!(refused(json!([{"do": "wait", "ms": 1}, {"do": "keys", "keys": ["warp"]}])).contains("step 2: \"warp\""));
        assert!(refused(json!([{"do": "keys", "keys": []}])).contains("1 to 4"));
        assert!(refused(json!([{"do": "keys", "keys": ["a", "b", "c", "d", "e"]}])).contains("1 to 4"));
        assert!(refused(json!([{"do": "keys", "keys": ["a", "A"]}])).contains("twice"));
        assert!(refused(json!([{"do": "type", "text": "a\nb"}])).contains("control"));
        assert!(refused(json!([{"do": "type", "text": ""}])).contains("1 to 200"));
        assert!(refused(json!([{"do": "type", "text": "x".repeat(201)}])).contains("1 to 200"));
        assert!(refused(json!([{"do": "picture", "label": "../evil"}])).contains("label"));
        assert!(refused(json!([{"do": "picture", "label": "end"}])).contains("used already"));
        assert!(refused(json!([{"do": "picture", "label": "a"}, {"do": "picture", "label": "A"}])).contains("used already"));
        let long: Vec<Value> = (0..13).map(|_| json!({"do": "wait", "ms": 10_000})).collect();
        assert!(refused(Value::Array(long)).contains("add up"));
        let many: Vec<Value> = (0..101).map(|_| json!({"do": "wait", "ms": 0})).collect();
        assert!(refused(Value::Array(many)).contains("at most 100"));
        let pictures: Vec<Value> = (0..21).map(|_| json!({"do": "picture"})).collect();
        assert!(refused(Value::Array(pictures)).contains("pictures"));
        // Unknown steps and fields are refused by the schema.
        assert!(serde_json::from_value::<Vec<Step>>(json!([{"do": "launch"}])).is_err());
        assert!(serde_json::from_value::<Vec<Step>>(json!([{"do": "wait", "ms": 1, "extra": 2}])).is_err());
    }

    /// What a test's stand-in input saw, in order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Sent {
        Key(Key, bool),
        Text(String),
        Pointer(i32, i32),
        Button(Button, bool),
    }

    #[derive(Default)]
    struct FakeInput {
        sent: Rc<RefCell<Vec<Sent>>>,
        /// Where the pointer lands, when not where it was aimed.
        lands_at: Option<(i32, i32)>,
        /// Fail the key press with this index.
        fail_key: Option<usize>,
    }

    impl InputSink for FakeInput {
        fn key(&mut self, key: Key, down: bool) -> Result<(), ToolError> {
            let presses = self.sent.borrow().iter().filter(|s| matches!(s, Sent::Key(_, true))).count();
            if down && self.fail_key == Some(presses) {
                return Err(ToolError::new(ErrorCode::ProgramFailed, "refused"));
            }
            self.sent.borrow_mut().push(Sent::Key(key, down));
            Ok(())
        }
        fn text(&mut self, text: &str) -> Result<(), ToolError> {
            self.sent.borrow_mut().push(Sent::Text(text.into()));
            Ok(())
        }
        fn pointer_to(&mut self, x: i32, y: i32) -> Result<(i32, i32), ToolError> {
            self.sent.borrow_mut().push(Sent::Pointer(x, y));
            Ok(self.lands_at.unwrap_or((x, y)))
        }
        fn button(&mut self, button: Button, down: bool) -> Result<(), ToolError> {
            self.sent.borrow_mut().push(Sent::Button(button, down));
            Ok(())
        }
    }

    /// A window at (100, 50), 400×300 on screen, pictured at 800×600 (a 2× display). `focus`
    /// says, look by look, whether it's in front (the last answer repeats).
    struct FakeScreen {
        focus: Vec<bool>,
        looks: usize,
        running: bool,
        /// The program closes after this many looks.
        closes_after: Option<usize>,
        slept: Duration,
        said: Vec<String>,
    }

    impl FakeScreen {
        fn focused() -> Self {
            FakeScreen { focus: vec![true], looks: 0, running: true, closes_after: None, slept: Duration::ZERO, said: Vec::new() }
        }
    }

    impl Screen for FakeScreen {
        fn place(&mut self) -> Result<WindowPlace, ToolError> {
            let focused = self.focus.get(self.looks).or(self.focus.last()).copied().unwrap_or(false);
            self.looks += 1;
            if self.closes_after.is_some_and(|n| self.looks >= n) {
                self.running = false;
            }
            Ok(WindowPlace { pid: 7, title: "Game".into(), x: 100, y: 50, width: 400, height: 300, focused })
        }
        fn picture(&mut self) -> Result<WindowShot, ToolError> {
            if !self.running {
                return Err(ToolError::new(ErrorCode::NotFound, "gone"));
            }
            Ok(WindowShot { title: "Game".into(), app: "game".into(), width: 800, height: 600, rgba: vec![0; 800 * 600 * 4] })
        }
        fn running(&mut self) -> bool {
            self.running
        }
        fn cancelled(&self) -> bool {
            false
        }
        fn sleep(&mut self, d: Duration) {
            self.slept += d;
        }
        fn say(&mut self, message: String) {
            self.said.push(message);
        }
    }

    fn run(script: Value, screen: &mut FakeScreen, input: &mut FakeInput) -> Played {
        play(&plan(&steps(script)).unwrap(), screen, input)
    }

    #[test]
    fn a_whole_script_plays_in_order_and_lets_everything_go() {
        let mut screen = FakeScreen::focused();
        let mut input = FakeInput::default();
        let played = run(
            json!([
                {"do": "keys", "keys": ["shift", "w"], "hold_ms": 200},
                {"do": "type", "text": "hi"},
                {"do": "click", "x": 400, "y": 300},
                {"do": "picture", "label": "after"},
            ]),
            &mut screen,
            &mut input,
        );
        assert_eq!(played.stop, None);
        assert_eq!(played.done, 4);
        assert_eq!(
            *input.sent.borrow(),
            vec![
                Sent::Key(Key::Shift, true),
                Sent::Key(Key::Char('w'), true),
                Sent::Key(Key::Char('w'), false),
                Sent::Key(Key::Shift, false),
                Sent::Text("hi".into()),
                // The picture's middle (400, 300 of 800×600) is the window's middle on screen.
                Sent::Pointer(300, 200),
                Sent::Button(Button::Left, true),
                Sent::Button(Button::Left, false),
            ]
        );
        let labels: Vec<&str> = played.pictures.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["start", "after", "end"]);
        assert_eq!(played.pictures[1].after_step, 3);
        assert_eq!(screen.slept, Duration::from_millis(200 + CLICK_MS));
    }

    #[test]
    fn nothing_is_sent_while_another_window_is_in_front() {
        // Never in front: nothing at all is sent, and the person was asked to click it.
        let mut screen = FakeScreen { focus: vec![false], ..FakeScreen::focused() };
        let mut input = FakeInput::default();
        let played = run(json!([{"do": "keys", "keys": ["space"]}]), &mut screen, &mut input);
        assert!(input.sent.borrow().is_empty());
        let stop = played.stop.unwrap();
        assert_eq!((stop.step, stop.code), (0, "focus_lost"));
        assert!(stop.message.contains("didn't come to the front"));
        assert_eq!(screen.said.len(), 1, "asked once, not on every look");
        assert_eq!(played.done, 0);
    }

    #[test]
    fn a_window_that_comes_forward_late_is_waited_for() {
        let mut screen = FakeScreen { focus: vec![false, false, true], ..FakeScreen::focused() };
        let mut input = FakeInput::default();
        let played = run(json!([{"do": "keys", "keys": ["space"]}]), &mut screen, &mut input);
        assert_eq!(played.stop, None);
        assert_eq!(*input.sent.borrow(), vec![Sent::Key(Key::Space, true), Sent::Key(Key::Space, false)]);
    }

    #[test]
    fn losing_the_front_while_a_key_is_held_releases_it_and_stops() {
        // In front for the first two looks (ready, the press), then gone during the hold.
        let mut screen = FakeScreen { focus: vec![true, true, false], ..FakeScreen::focused() };
        let mut input = FakeInput::default();
        let played = run(json!([{"do": "keys", "keys": ["w"], "hold_ms": 2000}, {"do": "type", "text": "never"}]), &mut screen, &mut input);
        let stop = played.stop.unwrap();
        assert_eq!((stop.step, stop.code), (1, "focus_lost"));
        assert_eq!(*input.sent.borrow(), vec![Sent::Key(Key::Char('w'), true), Sent::Key(Key::Char('w'), false)], "released, and the text never typed");
        assert!(screen.slept < Duration::from_millis(2000), "stopped as soon as it was seen");
        assert_eq!(played.done, 0);
    }

    #[test]
    fn a_refused_key_still_lets_go_of_the_ones_already_down() {
        let mut screen = FakeScreen::focused();
        let mut input = FakeInput { fail_key: Some(1), ..FakeInput::default() };
        let played = run(json!([{"do": "keys", "keys": ["control", "s"]}]), &mut screen, &mut input);
        assert_eq!(played.stop.unwrap().code, "input_failed");
        assert_eq!(*input.sent.borrow(), vec![Sent::Key(Key::Control, true), Sent::Key(Key::Control, false)]);
    }

    #[test]
    fn a_click_is_never_sent_where_the_pointer_did_not_land() {
        let mut screen = FakeScreen::focused();
        let mut input = FakeInput { lands_at: Some((900, 900)), ..FakeInput::default() };
        let played = run(json!([{"do": "click", "x": 10, "y": 10}]), &mut screen, &mut input);
        let stop = played.stop.unwrap();
        assert_eq!(stop.code, "pointer_elsewhere");
        assert!(!input.sent.borrow().iter().any(|s| matches!(s, Sent::Button(..))), "no button was pressed");
    }

    #[test]
    fn the_pointer_check_never_overflows_and_keeps_its_slack() {
        // A pointer reported at the far end of the range from where it was aimed: a refusal, not
        // a panic or a wrapped difference that passes.
        let mut input = FakeInput { lands_at: Some((i32::MIN, i32::MIN)), ..FakeInput::default() };
        assert_eq!(aim(&mut input, (i32::MAX, i32::MAX), 1).unwrap_err().code, "pointer_elsewhere");
        let mut input = FakeInput { lands_at: Some((i32::MAX, 0)), ..FakeInput::default() };
        assert_eq!(aim(&mut input, (i32::MIN, 0), 1).unwrap_err().code, "pointer_elsewhere");
        // Within the slack still clicks; one more point doesn't.
        let mut input = FakeInput { lands_at: Some((102, 48)), ..FakeInput::default() };
        assert!(aim(&mut input, (100, 50), 1).is_ok());
        let mut input = FakeInput { lands_at: Some((103, 50)), ..FakeInput::default() };
        assert_eq!(aim(&mut input, (100, 50), 1).unwrap_err().code, "pointer_elsewhere");
    }

    #[test]
    fn a_click_outside_the_picture_is_refused() {
        let mut screen = FakeScreen::focused();
        let mut input = FakeInput::default();
        let played = run(json!([{"do": "click", "x": 800, "y": 10}]), &mut screen, &mut input);
        assert_eq!(played.stop.unwrap().code, "outside_window");
        assert!(input.sent.borrow().is_empty());
    }

    #[test]
    fn a_program_that_closes_ends_the_session_without_an_end_picture() {
        let mut screen = FakeScreen { closes_after: Some(3), ..FakeScreen::focused() };
        let mut input = FakeInput::default();
        let played = run(json!([{"do": "keys", "keys": ["a"]}, {"do": "keys", "keys": ["b"]}, {"do": "keys", "keys": ["c"]}]), &mut screen, &mut input);
        let stop = played.stop.unwrap();
        assert_eq!(stop.code, "program_exited");
        assert!(played.done < 3);
        assert_eq!(played.pictures.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["start"]);
        let sent = input.sent.borrow();
        let downs = sent.iter().filter(|s| matches!(s, Sent::Key(_, true))).count();
        let ups = sent.iter().filter(|s| matches!(s, Sent::Key(_, false))).count();
        assert_eq!(downs, ups, "every key pressed was let go");
    }

    #[test]
    fn screen_positions_scale_and_never_overflow() {
        let place = WindowPlace { pid: 1, title: String::new(), x: -1920, y: 0, width: 1000, height: 500, focused: true };
        assert_eq!(to_screen(&place, (2000, 1000), 0, 0, 1).unwrap(), (-1920, 0));
        assert_eq!(to_screen(&place, (2000, 1000), 1999, 999, 1).unwrap(), (-921, 499));
        assert_eq!(to_screen(&place, (0, 0), 0, 0, 1).unwrap_err().code, "outside_window");
        let far = WindowPlace { x: i32::MAX, ..place };
        assert_eq!(to_screen(&far, (10, 10), 5, 5, 1).unwrap_err().code, "outside_window");
    }

    #[test]
    fn it_needs_consent_and_a_valid_script_before_anything_starts() {
        use crate::{CallOptions, Caller, Consent, Engine};
        // Elsewhere the tool isn't offered at all (`Requires::Os`).
        if !craft_anything_platform::input::SUPPORTED {
            return;
        }
        let e = Engine::new(crate::testing::session());
        let exe = std::env::current_exe().unwrap();
        let p = json!({"program": exe, "steps": [{"do": "wait", "ms": 1}]});
        assert_eq!(e.call(ID, p, CallOptions::new(Caller::Mcp)).unwrap_err().code, ErrorCode::ConsentRequired);
        let granted = || CallOptions::new(Caller::Mcp).with_consent(Consent::Granted);
        let missing = json!({"program": std::env::temp_dir().join("no-such-program.exe"), "steps": [{"do": "wait", "ms": 1}]});
        assert!(e.call(ID, missing, granted()).unwrap_err().message.contains("isn't a program file"));
        // A bad script and a relative folder are refused before the program (this test) starts.
        let bad = json!({"program": exe, "steps": [{"do": "keys", "keys": ["warp"]}]});
        assert!(e.call(ID, bad, granted()).unwrap_err().message.contains("warp"));
        let relative = json!({"program": exe, "steps": [{"do": "wait", "ms": 1}], "output_dir": "pictures"});
        assert!(e.call(ID, relative, granted()).unwrap_err().message.contains("output_dir"));
    }
}
