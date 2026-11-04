//! Study: look at the target before the first build (capture a website, triage a program, start
//! Ghidra's analysis in Deep mode) and write the notes the builder reads.

use super::*;

impl Worker {
    pub(super) fn study(&mut self, ws: &mut Workspace, kind: &dyn TargetKind) -> Result<(), ToolError> {
        match ws.manifest.target.clone() {
            TargetRef::Website { url } => {
                self.step(ws, Step::Study, Status::Inspecting, &format!("Opening {url} in a private browser…"))?;
                self.call("web_site_open", json!({"url": url}))?;
                self.step(ws, Step::Study, Status::Inspecting, "Photographing the page and reading its layout…")?;
                let capture = ws.capture_dir();
                if capture.exists() {
                    let _ = std::fs::remove_dir_all(&capture);
                }
                // The widths the check compares at, so every picture has its pair.
                let widths = self.engine.session().settings.build.widths();
                self.call("web_site_capture", json!({"output_directory": capture.display().to_string(), "widths": widths, "noise_pass": true}))?;
                // Menus, pop-ups and tabs, one at a time, on a computer and on a phone (where a menu
                // button often replaces the navigation); a page without them (or a failure) is fine.
                self.step(ws, Step::Study, Status::Inspecting, "Opening its menus, pop-ups and tabs…")?;
                let mut views = vec![(kinds::website::STATES_DIR.to_string(), widths.first().copied().unwrap_or(1440))];
                views.extend(kinds::website::phone_width(&widths).map(|w| (kinds::website::phone_states_dir(w), w)));
                for (dir, width) in views {
                    let states = capture.join(dir);
                    if let Err(e) = self.call("web_page_states_capture", json!({"output_directory": states.display().to_string(), "width": width})) {
                        self.step(ws, Step::Study, Status::Inspecting, &format!("Couldn't open its menus and pop-ups at {width} px: {}", e.message))?;
                    }
                }
                self.capture_routes(ws, &url, &widths)?;
                if !self.engine.session().clean_room() {
                    self.how_it_works(ws, &url)?;
                }
            }
            TargetRef::Local { path } if self.engine.session().clean_room() => self.study_from_outside(ws, &path)?,
            TargetRef::Local { path } => {
                self.step(ws, Step::Study, Status::Inspecting, "Looking inside it (nothing is run)…")?;
                let (program, folder) = locate(&path, kind.studies_folder());
                let mut notes = format!("# What we found in {}\n\n", path.display());
                if program != path {
                    notes.push_str(&format!("{} starts {}, so that's what we looked at.\n\n", path.display(), program.display()));
                }
                match self.call("core_target_triage", json!({"path": program.display().to_string()})) {
                    Ok(out) => notes.push_str(&describe(&out)),
                    Err(e) => notes.push_str(&format!("A first look failed: {}\n", e.message)),
                }
                if !program.is_file() && !self.engine.session().clean_room() {
                    self.step(
                        ws,
                        Step::Study,
                        Status::Inspecting,
                        "Couldn't find the program to decompile, so Ghidra can't be used. Pick the game's or app's main folder or its program.",
                    )?;
                    notes.push_str(&format!("\n## Ghidra\n\nNo program was found in {}, so Ghidra couldn't be used.\n", path.display()));
                }
                if let Some(done) = self.prepare_ghidra(&program) {
                    // Deep mode always waits for Ghidra: the builder then decompiles with it straight away.
                    let started = std::time::Instant::now();
                    // One line: the app's progress card shows how long it has been going.
                    self.step(ws, Step::Study, Status::Inspecting, "Ghidra is analysing the program (only the first time; a big game takes a while)…")?;
                    let result = loop {
                        match done.recv_timeout(Duration::from_secs(30)) {
                            Ok(r) => break r,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => self.stop.check()?,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err("the analysis stopped unexpectedly".into()),
                        }
                    };
                    notes.push_str(&match result {
                        Ok(()) => format!(
                            "\n## Ghidra\n\nDeep mode: Ghidra has analysed {} ({} min), so decompiling it is quick.\n",
                            program.display(),
                            started.elapsed().as_secs() / 60
                        ),
                        Err(e) => format!("\n## Ghidra\n\nGhidra couldn't analyse {} ({e}), so decompiling uses the built-in decompiler.\n", program.display()),
                    });
                }
                if let Some(f) = folder {
                    self.step(ws, Step::Study, Status::Inspecting, "Listing its maps, pictures, models and sounds…")?;
                    if let Some(found) = game::detect(&f) {
                        notes.push_str(&format!("\n## Made with {}\n\nThe giveaway: {}.", found.label(), found.clue));
                        if let Some(n) = &found.note {
                            notes.push_str(&format!(" {n}"));
                        }
                        notes.push('\n');
                    }
                    notes.push_str(&inventory(&f));
                }
                let is_js = path.extension().is_some_and(|e| e == "asar") || path.join("resources/app.asar").exists() || path.join("package.json").exists();
                if is_js && let Ok(out) = self.call("js_app_map", json!({"input_path": path.display().to_string(), "limit": 50})) {
                    notes.push_str("\n## JavaScript application\n\n");
                    notes.push_str(&describe(&out));
                }
                let capture = ws.capture_dir();
                std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
                let runtime = self.engine.session().settings.runtime_study;
                let mut console = false;
                if runtime && program.is_file() && kinds::runs::is_console(&program) {
                    // A console program draws no window: run it with each input instead.
                    self.step(ws, Step::Study, Status::Inspecting, "Running it with a few inputs to see what it prints…")?;
                    if let Some(section) = self.capture_runs(&program, &capture)? {
                        notes.push_str(&section);
                        console = true;
                    }
                }
                if !console && runtime && program.is_file() && is_program(&program) {
                    notes.push_str(&self.picture_window(ws, &program, &capture)?);
                }
                craft_anything_engine::project::write_atomic(&capture.join("README.md"), notes.as_bytes())?;
            }
            // An app on a device is only ever watched from outside, in either mode: what the
            // device says about itself, then a map of the app's screens made with the remote's
            // keys, into capture/map/, and a README the builder starts from.
            TargetRef::Device { serial, package } => self.study_device(ws, &serial, &package)?,
            TargetRef::None => {
                let capture = ws.capture_dir();
                std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
                craft_anything_engine::project::write_atomic(&capture.join("README.md"), b"# No target\n\nBuild from the request alone.\n")?;
            }
        }
        Ok(())
    }

    /// An app on a device: the device's facts, a question before the first key (only when the
    /// person answers questions; a project that decides for itself maps straight away), the map
    /// with the settings' limits and skip list, the README, and an optional pause to review it.
    fn study_device(&mut self, ws: &mut Workspace, serial: &str, package: &str) -> Result<(), ToolError> {
        use craft_anything_engine::workspace::Questions;
        self.step(ws, Step::Study, Status::Inspecting, &format!("Looking at {package} on {serial}…"))?;
        let capture = ws.capture_dir();
        if capture.exists() {
            let _ = std::fs::remove_dir_all(&capture);
        }
        std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
        let (build, questions) = {
            let s = self.engine.session();
            (s.settings.build.clone(), s.settings.questions_in(Some(&ws.manifest)))
        };
        let device = match self.call("adb_device_show", json!({"serial": serial})) {
            Ok(ToolOutput::Evidence(e)) => e.result.clone(),
            Ok(ToolOutput::Value(v)) => v,
            Err(e) => json!({"error": e.message}),
        };
        // The map presses keys on the device for a while: a person who answers questions is
        // asked first; a project that decides for itself goes ahead.
        let mut map_note = None;
        if questions == Questions::Ask {
            self.step(ws, Step::Study, Status::Inspecting, "Asking whether to map its screens…")?;
            let asked = self.call(
                "core_person_ask",
                json!({
                    "question": format!("Map {package} on {serial} by pressing its remote keys? The app will be started many times and every key pressed on every screen it reaches, within the limits in Settings."),
                    "choices": ["Yes, map its screens", "No, build from the request only"],
                    "allow_text": false,
                    "context": "Nothing inside the app is read. Screens whose text matches build.android_skip_texts are left alone.",
                }),
            );
            if let Ok(ToolOutput::Value(v)) = asked
                && v["choice"].as_u64() == Some(1)
            {
                map_note = Some("the person chose not to map it".to_string());
            }
        }
        let mut screens = None;
        if map_note.is_none() {
            self.step(ws, Step::Study, Status::Inspecting, "Mapping its screens with the remote's keys (this takes a while)…")?;
            let map_dir = capture.join("map");
            let params = json!({
                "serial": serial,
                "package": package,
                "max_states": build.android_max_states.clamp(1, 60),
                "skip_texts": build.android_skip_texts,
                "launch_wait_ms": build.android_launch_wait_ms,
                "wait_ms": build.android_wait_ms,
                "dump_lead_ms": build.android_dump_lead_ms,
                "output_directory": map_dir.display().to_string(),
            });
            match self.call("adb_app_map", params) {
                Ok(_) => {
                    screens = std::fs::read(map_dir.join("screens.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                    if screens.is_none() {
                        map_note = Some("the map wrote no screens.json".to_string());
                    }
                }
                Err(e) => {
                    self.step(ws, Step::Study, Status::Inspecting, &format!("Couldn't map its screens: {}", e.message))?;
                    map_note = Some(e.message);
                }
            }
        }
        let notes = device::readme(package, serial, &device, screens.as_ref(), map_note.as_deref());
        craft_anything_engine::project::write_atomic(&capture.join("README.md"), notes.as_bytes())?;
        if build.review_capture && questions == Questions::Ask {
            self.step(ws, Step::Study, Status::Inspecting, "Waiting for you to look at capture/…")?;
            let asked = self.call(
                "core_person_ask",
                json!({
                    "question": "capture/ is ready: the device's facts, the map of the app's screens and a picture of each. Look through it, remove anything that shouldn't be used, then continue.",
                    "choices": ["Continue", "Stop here"],
                    "allow_text": false,
                }),
            );
            if let Ok(ToolOutput::Value(v)) = asked
                && v["choice"].as_u64() == Some(1)
            {
                return Err(ToolError::cancelled());
            }
        }
        Ok(())
    }

    /// Deep mode: how the site works, not only how it looks, into capture/how-it-works.md: what its
    /// JavaScript is made of (with readable sources where its maps allow) and ten seconds of what the
    /// start page does (requests, console, WebSocket messages). A part that fails is said, not fatal.
    fn how_it_works(&mut self, ws: &mut Workspace, url: &str) -> Result<(), ToolError> {
        self.step(ws, Step::Study, Status::Inspecting, "Reading its JavaScript and watching what it does…")?;
        // The routes left the browser on another page; these look at the start page.
        let _ = self.call("web_site_open", json!({"url": url}));
        let mut notes = String::from("# How it works\n\nDeep mode: what the site's JavaScript is made of and what its start page did while we watched.\n");
        let looks = [
            ("Its JavaScript", "web_page_js_analyze", json!({"load_source_maps": true})),
            ("Ten seconds of it", "web_page_watch", json!({"watch_ms": 10_000, "with_websocket_outlines": true})),
        ];
        for (title, tool, params) in looks {
            notes.push_str(&format!("\n## {title}\n\n"));
            notes.push_str(&match self.call(tool, params) {
                Ok(out) => describe(&out),
                Err(e) => format!("{tool} didn't work: {}\n", e.message),
            });
        }
        craft_anything_engine::project::write_atomic(&ws.capture_dir().join(HOW_IT_WORKS), notes.as_bytes())
    }

    /// Clean room: nothing inside the original is read (no files, folder or code). When Settings
    /// allows running it and the person picked the program itself, it is started once and its
    /// window pictured, as anyone could see it.
    fn study_from_outside(&mut self, ws: &mut Workspace, path: &Path) -> Result<(), ToolError> {
        self.step(ws, Step::Study, Status::Inspecting, "Clean room: nothing inside it is read…")?;
        let capture = ws.capture_dir();
        std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
        let mut notes = format!(
            "# Clean room\n\nNothing inside {} was read: its files, its folder and its code stay closed, and the tools that look inside are off. Build from the person's request and description and the pictures in reference/, and ask them with core_person_ask when something isn't clear.\n",
            path.display()
        );
        let runtime = self.engine.session().settings.runtime_study;
        if runtime && path.is_file() && is_program(path) {
            notes.push_str(&self.picture_window(ws, path, &capture)?);
        }
        craft_anything_engine::project::write_atomic(&capture.join("README.md"), notes.as_bytes())
    }

    /// Runtime study (Settings): start the program once and picture its window into capture/, for
    /// the notes.
    fn picture_window(&mut self, ws: &mut Workspace, program: &Path, capture: &Path) -> Result<String, ToolError> {
        self.step(ws, Step::Study, Status::Inspecting, "Starting it for a moment to picture its window…")?;
        let shot = capture.join("window.png");
        let tree = capture.join("ui-tree.json");
        let params =
            json!({"program": program.display().to_string(), "wait_secs": 15, "output": shot.display().to_string(), "ui_tree": tree.display().to_string()});
        let picture = "capture/window.png is a picture of its window 15 seconds after it started. Open it with the Read tool: match its layout, colours and proportions.\n";
        let mut notes = String::from("\n## How it looks running\n\n");
        notes.push_str(&match self.call("native_window_capture", params) {
            Ok(_) if tree.is_file() => format!(
                "{picture}capture/ui-tree.json lists the controls in its windows at that moment, in order (role, title, identifier, value, position and size in pixels): use it for the names, order and placement of its menus, buttons and fields.\n"
            ),
            Ok(_) => picture.to_string(),
            Err(e) => format!("Starting it for a picture didn't work ({}), so there's no picture of it running.\n", e.message),
        });
        Ok(notes)
    }

    /// The site's other pages (up to `build.max_pages` same-site pages: links from the start
    /// page, then links from those pages while there's room), each captured into
    /// capture/routes/<name>/ and listed in capture/routes.json. A page that can't be captured is
    /// left out; it never stops the study.
    fn capture_routes(&mut self, ws: &mut Workspace, start: &str, widths: &[u32]) -> Result<(), ToolError> {
        let capture = ws.capture_dir();
        let read = |dir: &Path| std::fs::read(dir.join("capture.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let found = read(&capture).unwrap_or(Value::Null);
        let max = self.engine.session().settings.build.max_pages.min(kinds::website::MAX_ROUTES);
        let picked = kinds::website::pick_routes(&found, start, max);
        let mut kept = self.capture_pages(ws, &picked, widths)?;
        // One level deeper: pages linked from the ones just captured, while there's room.
        if kept.len() < max && !kept.is_empty() {
            let captures: Vec<Value> = kept.iter().filter_map(|r| read(&capture.join("routes").join(&r.slug))).collect();
            let deeper = kinds::website::pick_deeper_routes(&captures, start, &kept, max - kept.len());
            kept.extend(self.capture_pages(ws, &deeper, widths)?);
        }
        if !kept.is_empty() {
            let json = serde_json::to_vec_pretty(&kept).map_err(|e| ToolError::internal(e.to_string()))?;
            craft_anything_engine::project::write_atomic(&capture.join(kinds::website::ROUTES_FILE), &json)?;
        }
        Ok(())
    }

    /// Capture each page into capture/routes/<slug>/; returns the ones that worked.
    fn capture_pages(&mut self, ws: &mut Workspace, picked: &[kinds::website::Route], widths: &[u32]) -> Result<Vec<kinds::website::Route>, ToolError> {
        let capture = ws.capture_dir();
        let mut kept = Vec::new();
        if !picked.is_empty() {
            // web_site_capture creates its own folder, inside one that exists.
            std::fs::create_dir_all(capture.join("routes")).map_err(|e| ToolError::io(capture.display(), e))?;
        }
        for (i, r) in picked.iter().enumerate() {
            self.step(ws, Step::Study, Status::Inspecting, &format!("Photographing more pages ({} of {}): {}…", i + 1, picked.len(), r.path))?;
            let dir = capture.join("routes").join(&r.slug);
            let got = self.call("web_site_open", json!({"url": r.url})).and_then(|_| {
                self.call(
                    "web_site_capture",
                    json!({"output_directory": dir.display().to_string(), "include_html": false, "widths": widths, "noise_pass": true}),
                )
            });
            match got {
                Ok(_) => kept.push(r.clone()),
                // Said in the feed, not hidden: the page is left out and the study goes on.
                Err(e) => self.step(ws, Step::Study, Status::Inspecting, &format!("Couldn't photograph {}: {}", r.path, e.message))?,
            }
        }
        Ok(kept)
    }

    /// Deep mode with Ghidra installed: run Ghidra's one-time analysis of `program` on its own
    /// thread; the receiver gets its result. The builder's tools share the analysis (waiting for it
    /// rather than starting another), and it carries on if the person pauses, so nothing is lost.
    pub(super) fn prepare_ghidra(&self, program: &Path) -> Option<Receiver<Result<(), String>>> {
        // A folder (nothing in it recognised as the program) isn't something Ghidra can open.
        if !program.is_file() {
            return None;
        }
        let settings = {
            let s = self.engine.session();
            if s.clean_room() || s.settings.ghidra_launcher().is_none() {
                return None;
            }
            s.settings.clone()
        };
        let program = program.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ghidra-prepare".into())
            .spawn(move || {
                let result = std::fs::read(&program).map_err(|e| e.to_string()).and_then(|bytes| {
                    let sha = craft_anything_evidence::sha256_hex(&bytes);
                    craft_anything_engine::tools::deep::prepare(&settings, &bytes, &sha, &|_| {})
                });
                if let Err(e) = &result {
                    log::warn!("Ghidra's analysis of {} failed: {e}", program.display());
                }
                let _ = tx.send(result);
            })
            .ok()?;
        Some(rx)
    }
}

/// Deep mode's notes on how a website works, in capture/.
pub(super) const HOW_IT_WORKS: &str = "how-it-works.md";

/// A short text view of a tool result for the study notes.
pub(super) fn describe(out: &ToolOutput) -> String {
    match out {
        ToolOutput::Evidence(e) => {
            let body = serde_json::to_string_pretty(&e.result).unwrap_or_default();
            format!("{}\n\nEvidence {}.\n\n```json\n{}\n```\n", e.summary, e.id, body.chars().take(20_000).collect::<String>())
        }
        ToolOutput::Value(v) => format!("```json\n{}\n```\n", serde_json::to_string_pretty(v).unwrap_or_default().chars().take(20_000).collect::<String>()),
    }
}
