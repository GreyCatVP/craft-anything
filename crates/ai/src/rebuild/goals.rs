//! Goal control around the existing study/build/check loop; no second agent loop.
use super::*;
use craft_anything_engine::goal::{self, Goal, GoalStatus};
use std::collections::BTreeSet;

pub(super) struct GoalRuntime {
    root: PathBuf,
    since: Instant,
    baseline_ms: u64,
    watching: Arc<AtomicBool>,
    cancel: CancelToken,
    watcher: Option<std::thread::JoinHandle<()>>,
    unknowns: BTreeSet<String>,
    last_round: u32,
}

impl GoalRuntime {
    pub(super) fn start(root: &Path, cancel: CancelToken) -> Result<Self, ToolError> {
        let g = goal::load(root)?.ok_or_else(|| ToolError::invalid("Save a goal before resuming it."))?;
        if g.status != GoalStatus::Active {
            return Err(ToolError::invalid("The goal was paused before its build could start."));
        }
        let unknowns = g
            .spec
            .milestones
            .iter()
            .flat_map(|m| &m.acceptance)
            .filter_map(|r| match r {
                goal::Requirement::ResolvedUnknown { id } => Some(id.clone()),
                _ => None,
            })
            .collect();
        let mut runtime = Self {
            root: root.to_path_buf(),
            since: Instant::now(),
            baseline_ms: g.work_ms,
            watching: Arc::new(AtomicBool::new(true)),
            cancel,
            watcher: None,
            unknowns,
            last_round: g.checkpoints.last().map_or(0, |c| c.round),
        };
        let (root, since, baseline, watching, stop) =
            (runtime.root.clone(), runtime.since, runtime.baseline_ms, runtime.watching.clone(), runtime.cancel.clone());
        runtime.watcher = Some(
            std::thread::Builder::new()
                .name("goal-control".into())
                .spawn(move || {
                    let mut saved_at = Instant::now();
                    while watching.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(500));
                        if !watching.load(Ordering::Acquire) {
                            break;
                        }
                        // Watch even during a long build/check or a question waiting for an answer.
                        let result = goal::load(&root).and_then(|g| {
                            let mut g = g.ok_or_else(|| ToolError::invalid("The active goal record was removed."))?;
                            account_time(&mut g, since, baseline, 0);
                            if g.status == GoalStatus::BudgetLimited || saved_at.elapsed() >= Duration::from_secs(5) {
                                saved_at = Instant::now();
                                save_usage(&root, since, baseline, 0)
                            } else {
                                Ok(g)
                            }
                        });
                        match result {
                            Ok(g) if g.status == GoalStatus::Active => {}
                            Err(e) if e.code == ErrorCode::StaleRevision => continue,
                            Err(e) => {
                                log::warn!("Goal control stopped: {}", e.message);
                                stop.cancel();
                                break;
                            }
                            Ok(_) => {
                                stop.cancel();
                                break;
                            }
                        }
                    }
                })
                .map_err(|e| ToolError::internal(format!("Couldn't start goal control: {e}")))?,
        );
        Ok(runtime)
    }

    pub(super) fn account(&self, tokens: u64) -> Result<Goal, ToolError> {
        let g = save_usage(&self.root, self.since, self.baseline_ms, tokens)?;
        if g.status != GoalStatus::Active {
            self.cancel.cancel();
        }
        Ok(g)
    }

    pub(super) fn prompt(&self) -> Result<String, ToolError> {
        let g = self.account(0)?;
        self.cancel.check()?;
        Ok(g.prompt())
    }

    pub(super) fn verify(&self, ws: &Workspace, round: u32, checks: &[CheckResult]) -> Result<Goal, ToolError> {
        if round == 0 || checks.iter().any(|c| c.round != round) {
            return Err(ToolError::invalid("A check returned results for a different round. Fix the check before resuming the goal."));
        }
        let resolved = resolved_unknowns(ws, &self.unknowns, &self.cancel)?;
        goal::update(&self.root, |g| {
            if g.checkpoints.last().is_some_and(|c| c.round >= round) {
                return Err(ToolError::invalid("The verification round did not advance beyond the saved goal checkpoint."));
            }
            account_time(g, self.since, self.baseline_ms, 0);
            if g.awaiting_check {
                g.verify(round, checks, &resolved);
            } else {
                g.revalidate(round, checks, &resolved);
            }
            Ok(())
        })
    }

    pub(super) fn last_round(&self) -> u32 {
        self.last_round
    }

    pub(super) fn built(&self) -> Result<(), ToolError> {
        goal::update(&self.root, |g| {
            if g.status == GoalStatus::Active {
                g.awaiting_check = true;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub(super) fn finish(&self, failed: bool, message: &str) {
        self.watching.store(false, Ordering::Release);
        if let Err(e) = goal::update(&self.root, |g| {
            account_time(g, self.since, self.baseline_ms, 0);
            // A person's pause and a budget/completion verdict always win over cleanup.
            if g.status == GoalStatus::Active {
                g.status = if failed { GoalStatus::Blocked } else { GoalStatus::Paused };
                g.note = message.chars().take(2000).collect();
            }
            Ok(())
        }) {
            log::warn!("Couldn't checkpoint the goal: {}", e.message);
        }
    }
}

impl Drop for GoalRuntime {
    fn drop(&mut self) {
        self.watching.store(false, Ordering::Release);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

fn account_time(g: &mut Goal, since: Instant, baseline: u64, tokens: u64) {
    let elapsed = u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX);
    g.work_ms = g.work_ms.max(baseline.saturating_add(elapsed));
    g.tokens = g.tokens.saturating_add(tokens);
    if g.status == GoalStatus::Active
        && let Some(reason) = g.budget_reason(false)
    {
        g.status = GoalStatus::BudgetLimited;
        g.note = reason.into();
    }
}

fn save_usage(root: &Path, since: Instant, baseline: u64, tokens: u64) -> Result<Goal, ToolError> {
    goal::update(root, |g| {
        account_time(g, since, baseline, tokens);
        Ok(())
    })
}

/// A resolved flag alone is insufficient: every cited record must exist and verify.
fn resolved_unknowns(ws: &Workspace, required: &BTreeSet<String>, cancel: &CancelToken) -> Result<BTreeSet<String>, ToolError> {
    if required.is_empty() || !ws.investigation_dir().exists() {
        return Ok(BTreeSet::new());
    }
    let p = Project::open(&ws.investigation_dir())?;
    let mut resolved = BTreeSet::new();
    for u in p.load_unknowns()? {
        cancel.check()?;
        if !required.contains(&u.id) || !u.resolution_is_valid() {
            continue;
        }
        let mut valid = true;
        for id in &u.evidence_for {
            cancel.check()?;
            if p.load_evidence(id).is_err() {
                valid = false;
                break;
            }
        }
        if valid {
            resolved.insert(u.id);
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engine::goal::{GoalSpec, Limits};
    fn ready() -> (tempfile::TempDir, GoalRuntime) {
        let d = tempfile::tempdir().unwrap();
        goal::set(d.path(), GoalSpec { objective: "Rebuild the sample".into(), constraints: vec![], milestones: vec![], limits: Limits::default() }, None)
            .unwrap();
        goal::update(d.path(), Goal::activate).unwrap();
        let r = GoalRuntime::start(d.path(), CancelToken::default()).unwrap();
        (d, r)
    }
    #[test]
    fn pause_survives_accounting_and_finish() {
        let (d, r) = ready();
        goal::update(d.path(), |g| {
            g.status = GoalStatus::Paused;
            Ok(())
        })
        .unwrap();
        assert_eq!(r.account(12).unwrap().status, GoalStatus::Paused);
        assert!(r.cancel.is_cancelled());
        r.finish(true, "A late cancellation error");
        let g = goal::load(d.path()).unwrap().unwrap();
        assert_eq!(g.status, GoalStatus::Paused);
        assert_eq!(g.tokens, 12);
    }
    #[test]
    fn time_and_tokens_stop_before_the_next_turn_without_completing() {
        let (d, r) = ready();
        goal::update(d.path(), |g| {
            g.spec.limits.max_tokens = Some(10);
            Ok(())
        })
        .unwrap();
        assert_eq!(r.account(10).unwrap().status, GoalStatus::BudgetLimited);
        assert!(r.cancel.is_cancelled());
        let mut g = goal::load(d.path()).unwrap().unwrap();
        g.status = GoalStatus::Active;
        g.spec.limits.max_tokens = None;
        let baseline = g.spec.limits.max_minutes * 60_000;
        account_time(&mut g, Instant::now(), baseline, 0);
        assert_eq!(g.status, GoalStatus::BudgetLimited);
    }
    #[test]
    fn watchdog_cancels_a_turn_without_any_agent_events() {
        let (d, r) = ready();
        goal::update(d.path(), |g| {
            g.status = GoalStatus::Paused;
            Ok(())
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !r.cancel.is_cancelled() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(r.cancel.is_cancelled());
    }

    #[test]
    fn a_finished_turn_can_be_checked_after_restarting_without_rebuilding_it() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Sample", "Rebuild it", TargetRef::None).unwrap();
        goal::set(&ws.root, GoalSpec { objective: "Rebuild the sample".into(), constraints: vec![], milestones: vec![], limits: Limits::default() }, None)
            .unwrap();
        goal::update(&ws.root, Goal::activate).unwrap();
        let first = GoalRuntime::start(&ws.root, CancelToken::default()).unwrap();
        first.verify(&ws, 9, &[CheckResult::new("build", 9, true, "Baseline before work")]).unwrap();
        first.built().unwrap();
        first.finish(false, "Host closed after the turn");
        drop(first);
        assert!(goal::load(&ws.root).unwrap().unwrap().awaiting_check);
        goal::update(&ws.root, Goal::activate).unwrap();
        let resumed = GoalRuntime::start(&ws.root, CancelToken::default()).unwrap();
        assert_eq!(resumed.last_round(), 9, "a restored older craft.json must not reuse a goal's check round");
        let g = resumed.verify(&ws, 10, &[CheckResult::new("build", 10, true, "Fresh build check")]).unwrap();
        assert_eq!(g.status, GoalStatus::Complete);
        assert_eq!(g.cycles, 1);
        assert!(!g.awaiting_check);
    }

    #[test]
    fn resolving_an_unknown_requires_real_unchanged_evidence() {
        use craft_anything_evidence::{Environment, EvidenceBuilder, Severity, Subject, Unknown, UnknownStatus};
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Sample", "Rebuild it", TargetRef::None).unwrap();
        let p = Project::create(&ws.investigation_dir(), "Study", Purpose::default()).unwrap();
        let mut u = Unknown::new("How does selection work?", "interaction", Severity::High);
        u.status = UnknownStatus::Resolved;
        u.resolution = Some("Observed a repeatable selection response".into());
        u.evidence_for = vec![format!("ev_{}", "0".repeat(64))];
        p.store_unknown(&u).unwrap();
        let required = BTreeSet::from([u.id.clone()]);
        let cancel = CancelToken::default();
        assert!(resolved_unknowns(&ws, &required, &cancel).unwrap().is_empty());
        let ev =
            EvidenceBuilder::new("runtime_ui_read", Subject::default(), Environment::builtin(true)).result(json!({"selection":"observed"})).build().unwrap();
        p.store_evidence(&ev).unwrap();
        u.evidence_for = vec![ev.id.clone()];
        p.store_unknown(&u).unwrap();
        assert_eq!(resolved_unknowns(&ws, &required, &cancel).unwrap(), required);
        let file = p.root.join("evidence").join(ev.id.get(3..5).unwrap()).join(format!("{}.json", ev.id));
        let mut changed = ev;
        changed.result = json!({"selection":"invented"});
        std::fs::write(file, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(resolved_unknowns(&ws, &required, &cancel).unwrap().is_empty());
    }
}
