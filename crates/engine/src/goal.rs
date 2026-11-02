//! Durable project goals. Only fresh check results can advance a milestone.

use crate::inbox::now_ms;
use crate::project::write_atomic;
use crate::workspace::{CheckResult, Workspace};
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Read;
use std::path::Path;

pub const FILE: &str = "goal.json";
pub const VERSION: u32 = 1;
pub const BUILD_LOCK: &str = ".build.lock";
pub const STATE_LOCK: &str = ".goal.lock";
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const HISTORY: usize = 32;
const STEPS: &[crate::versioned::Step<serde_json::Value>] = &[v0_to_v1];
const _: () = assert!(VERSION as usize == STEPS.len());

fn v0_to_v1(value: &mut serde_json::Value) -> Result<(), String> {
    let map = value.as_object_mut().ok_or("A goal must be a JSON object.")?;
    map.insert("version".into(), 1.into());
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Paused,
    Active,
    Complete,
    Blocked,
    BudgetLimited,
}

impl GoalStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Paused => "Paused",
            Self::Active => "Working",
            Self::Complete => "Complete",
            Self::Blocked => "Needs attention",
            Self::BudgetLimited => "Budget reached",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_cycles: u32,
    pub max_minutes: u64,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    pub stalled_cycles: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_cycles: 24, max_minutes: 240, max_tokens: None, stalled_cycles: 5 }
    }
}

/// Machine check ids are stable identifiers, never parsed from a check's prose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Requirement {
    ProjectChecks {
        #[serde(default = "default_similarity")]
        minimum_similarity: f64,
    },
    Check {
        id: String,
        #[serde(default)]
        minimum_similarity: Option<f64>,
    },
    ResolvedUnknown {
        id: String,
    },
}

fn default_similarity() -> f64 {
    90.0
}
pub fn default_requirements() -> Vec<Requirement> {
    vec![Requirement::ProjectChecks { minimum_similarity: default_similarity() }]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Milestone {
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default = "default_requirements")]
    pub acceptance: Vec<Requirement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalSpec {
    pub objective: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    #[serde(default)]
    pub limits: Limits,
}

impl GoalSpec {
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.objective.trim().is_empty() || self.objective.len() > 16_000 {
            return Err(ToolError::invalid("Give the goal an objective of 1 to 16000 bytes."));
        }
        let l = &self.limits;
        if !(1..=1000).contains(&l.max_cycles) || !(1..=10_080).contains(&l.max_minutes) || l.max_tokens == Some(0) || !(2..=100).contains(&l.stalled_cycles) {
            return Err(ToolError::invalid("Use 1–1000 cycles, 1–10080 minutes, a positive token limit, and 2–100 stalled cycles."));
        }
        if self.constraints.len() > 64 || self.constraints.iter().any(|s| s.len() > 4000) || self.milestones.len() > 64 {
            return Err(ToolError::invalid("A goal supports at most 64 constraints (4000 bytes each) and 64 milestones."));
        }
        for m in &self.milestones {
            if m.title.trim().is_empty() || m.title.len() > 500 || m.instructions.len() > 16_000 || m.acceptance.is_empty() || m.acceptance.len() > 64 {
                return Err(ToolError::invalid("Each milestone needs a title, bounded instructions and 1–64 acceptance requirements."));
            }
            for r in &m.acceptance {
                let (id, score) = match r {
                    Requirement::ProjectChecks { minimum_similarity } => (None, Some(*minimum_similarity)),
                    Requirement::Check { id, minimum_similarity } => (Some(id), *minimum_similarity),
                    Requirement::ResolvedUnknown { id } => (Some(id), None),
                };
                if id.is_some_and(|s| s.trim().is_empty() || s.len() > 200) || score.is_some_and(|n| !n.is_finite() || !(0.0..=100.0).contains(&n)) {
                    return Err(ToolError::invalid("Check/unknown ids must be nonempty and similarity must be between 0 and 100."));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GateResult {
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Checkpoint {
    #[serde(default)]
    pub verification_only: bool,
    pub cycle: u32,
    pub round: u32,
    pub milestone: usize,
    pub at_ms: u64,
    pub results: Vec<Vec<GateResult>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Goal {
    pub version: u32,
    pub revision: u64,
    pub spec: GoalSpec,
    pub status: GoalStatus,
    pub current_milestone: usize,
    pub cycles: u32,
    /// A builder turn for this contract finished; resume may verify it without rebuilding.
    #[serde(default)]
    pub awaiting_check: bool,
    pub work_ms: u64,
    pub tokens: u64,
    pub stalled: u32,
    pub best_progress: f64,
    pub note: String,
    pub updated_ms: u64,
    pub checkpoints: Vec<Checkpoint>,
}

impl Goal {
    pub fn new(mut spec: GoalSpec) -> Result<Self, ToolError> {
        spec.validate()?;
        if spec.milestones.is_empty() {
            spec.milestones.push(Milestone { title: "Complete the goal".into(), instructions: spec.objective.clone(), acceptance: default_requirements() });
        }
        Ok(Self {
            version: VERSION,
            revision: 1,
            spec,
            status: GoalStatus::Paused,
            current_milestone: 0,
            cycles: 0,
            awaiting_check: false,
            work_ms: 0,
            tokens: 0,
            stalled: 0,
            best_progress: -1.0,
            note: "Ready to start.".into(),
            updated_ms: now_ms(),
            checkpoints: Vec::new(),
        })
    }

    pub fn budget_reason(&self, include_cycles: bool) -> Option<&'static str> {
        if self.work_ms >= self.spec.limits.max_minutes.saturating_mul(60_000) {
            Some("The active-time budget was reached.")
        } else if self.spec.limits.max_tokens.is_some_and(|max| self.tokens >= max) {
            Some("The reported token budget was reached.")
        } else if include_cycles && self.cycles >= self.spec.limits.max_cycles {
            Some("The work-cycle budget was reached.")
        } else {
            None
        }
    }

    pub fn activate(&mut self) -> Result<(), ToolError> {
        if self.status == GoalStatus::Complete {
            return Err(ToolError::invalid("This goal is complete. Save a new goal to change its scope."));
        }
        if let Some(reason) = self.budget_reason(true) {
            return Err(ToolError::new(ErrorCode::OverLimit, reason).with_remediation("Save the goal with larger limits, then resume."));
        }
        self.status = GoalStatus::Active;
        self.stalled = 0;
        self.note = "Starting the next milestone; saved checks will be verified again.".into();
        Ok(())
    }

    /// One fresh round. Earlier milestones must still hold in the current build.
    pub fn verify(&mut self, round: u32, checks: &[CheckResult], resolved: &BTreeSet<String>) {
        self.verify_round(round, checks, resolved, true);
    }

    /// Establish the current state after a restart, without accepting unattempted milestones.
    pub fn revalidate(&mut self, round: u32, checks: &[CheckResult], resolved: &BTreeSet<String>) {
        self.verify_round(round, checks, resolved, false);
    }

    fn verify_round(&mut self, round: u32, checks: &[CheckResult], resolved: &BTreeSet<String>, advance: bool) {
        if self.status != GoalStatus::Active {
            return;
        }
        // A saved or self-check round is history, never a new acceptance result.
        if round == 0 || checks.iter().any(|c| c.round != round) || self.checkpoints.last().is_some_and(|c| c.round >= round) {
            return;
        }
        if advance {
            self.cycles = self.cycles.saturating_add(1);
            self.awaiting_check = false;
        }
        let all: Vec<Vec<GateResult>> = self.spec.milestones.iter().map(|m| m.acceptance.iter().map(|r| evaluate(r, checks, resolved)).collect()).collect();
        // A requirement supplies its own similarity bar; every check still has to pass its
        // own explicit bar, claims and unknowns even when it isn't named by a milestone.
        let project_ok = !checks.is_empty() && checks.iter().all(|c| passes(c, 0.0, false));
        let earlier_failure = all.iter().take(self.current_milestone).position(|rs| rs.iter().any(|r| !r.passed));
        if let Some(index) = earlier_failure {
            self.current_milestone = index;
            self.best_progress = -1.0;
            self.stalled = 0;
        }
        let accepted = advance && project_ok && all.get(self.current_milestone).is_some_and(|rs| !rs.is_empty() && rs.iter().all(|r| r.passed));
        if accepted {
            self.current_milestone = self.current_milestone.saturating_add(1);
            self.stalled = 0;
        }
        let passed = all.iter().flatten().filter(|r| r.passed).count() as f64;
        let (sum, count) = checks
            .iter()
            .filter_map(|c| c.similarity)
            .filter(|s| s.is_finite() && (0.0..=100.0).contains(s))
            .fold((0.0, 0_u32), |(sum, count), score| (sum + score, count.saturating_add(1)));
        let likeness = if count == 0 { 0.0 } else { sum / f64::from(count) / 100.0 };
        let progress = self.current_milestone as f64 * 1000.0 + passed + likeness;
        if progress > self.best_progress + 0.0001 {
            self.best_progress = progress;
            self.stalled = 0;
        } else if advance && !accepted {
            self.stalled = self.stalled.saturating_add(1);
        }
        self.note = if self.current_milestone >= self.spec.milestones.len() && project_ok {
            self.status = GoalStatus::Complete;
            "Every milestone and all project checks passed in this verification round.".into()
        } else if self.stalled >= self.spec.limits.stalled_cycles {
            self.status = GoalStatus::Blocked;
            "Several cycles made no measured progress. Review the remaining checks or provide more evidence before resuming.".into()
        } else if let Some(reason) = self.budget_reason(true) {
            self.status = GoalStatus::BudgetLimited;
            reason.into()
        } else {
            all.get(self.current_milestone).into_iter().flatten().filter(|r| !r.passed).take(8).map(|r| r.detail.as_str()).collect::<Vec<_>>().join("; ")
        };
        if self.note.is_empty() && self.status == GoalStatus::Active {
            self.note = if advance {
                "Continue with the next milestone; all project checks must still pass."
            } else {
                "Current build rechecked. Work on the current milestone before accepting it."
            }
            .into();
        }
        self.checkpoints.push(Checkpoint {
            verification_only: !advance,
            cycle: self.cycles,
            round,
            milestone: self.current_milestone,
            at_ms: now_ms(),
            results: all,
        });
        if self.checkpoints.len() > HISTORY {
            self.checkpoints.remove(0);
        }
    }

    pub fn prompt(&self) -> String {
        let milestone = self.spec.milestones.get(self.current_milestone);
        let focus = milestone.map(|m| format!("{}\n{}", m.title, m.instructions)).unwrap_or_default();
        let contract = serde_json::to_string(&milestone.map(|m| &m.acceptance)).unwrap_or_default();
        format!(
            "Project goal: {}\nConstraints:\n{}\nCurrent milestone:\n{focus}\nAcceptance requirements (structured): {contract}\nLatest verification: {}\nWork toward this milestone. Study the original again with the permitted tools when evidence is missing; do not just repeat the same implementation change. Keep earlier behaviour working and use the verification steps available to your builder. Missing checks or missing evidence cannot count as success. Report what you changed and what remains uncertain; the goal controller decides completion.",
            self.spec.objective,
            self.spec.constraints.join("\n"),
            self.note
        )
    }
}

fn evaluate(r: &Requirement, checks: &[CheckResult], resolved: &BTreeSet<String>) -> GateResult {
    match r {
        Requirement::ResolvedUnknown { id } => GateResult { passed: resolved.contains(id), detail: format!("Evidence-backed resolution required: {id}") },
        Requirement::ProjectChecks { minimum_similarity } => {
            let failures: Vec<_> =
                checks.iter().filter(|c| !passes(c, *minimum_similarity, false)).take(4).map(|c| check_failure(c, *minimum_similarity)).collect();
            GateResult {
                passed: !checks.is_empty() && failures.is_empty(),
                detail: if checks.is_empty() { "No project checks ran.".into() } else { format!("Project checks not yet satisfied: {}", failures.join(", ")) },
            }
        }
        Requirement::Check { id, minimum_similarity } => {
            let found = checks.iter().any(|c| &c.check == id);
            GateResult {
                passed: found && checks.iter().filter(|c| &c.check == id).all(|c| passes(c, minimum_similarity.unwrap_or(0.0), minimum_similarity.is_some())),
                detail: if found {
                    let failed: Vec<_> = checks
                        .iter()
                        .filter(|c| &c.check == id && !passes(c, minimum_similarity.unwrap_or(0.0), minimum_similarity.is_some()))
                        .take(4)
                        .map(|c| check_failure(c, minimum_similarity.unwrap_or(0.0)))
                        .collect();
                    if failed.is_empty() { format!("Check passed: {id}") } else { failed.join("; ") }
                } else {
                    format!("The required check did not run: {id}")
                },
            }
        }
    }
}

fn passes(c: &CheckResult, minimum: f64, require_score: bool) -> bool {
    c.good(minimum)
        && match c.similarity {
            Some(s) => s.is_finite() && (0.0..=100.0).contains(&s) && s >= minimum,
            None => !require_score,
        }
}

fn check_failure(c: &CheckResult, minimum: f64) -> String {
    let score = c
        .similarity
        .map(|s| format!("{s:.2}% alike, needs at least {:.2}%", minimum.max(c.good_enough.unwrap_or(minimum))))
        .unwrap_or_else(|| "no measured similarity score".into());
    let note: String = c.note.chars().take(160).collect();
    format!("{}: passed={}, {score}, {} unknowns, {} failed claims. {note}", c.check, c.passed, c.unknowns.len(), c.claims.iter().filter(|c| !c.passed).count())
}

pub fn load(root: &Path) -> Result<Option<Goal>, ToolError> {
    let path = root.join(FILE);
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ToolError::io(path.display(), e)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| ToolError::io(path.display(), e))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ToolError::new(ErrorCode::OverLimit, "The goal record is too large."));
    }
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| ToolError::invalid(format!("The goal record is unreadable: {e}")))?;
    let version = match value.get("version") {
        None => 0,
        Some(v) => v.as_u64().ok_or_else(|| ToolError::invalid("The goal version must be an integer."))?,
    };
    if version > u64::from(VERSION) {
        return Err(ToolError::unsupported("This goal needs a newer Craft Anything."));
    }
    let version = u32::try_from(version).map_err(|_| ToolError::unsupported("This goal needs a newer Craft Anything."))?;
    crate::versioned::upgrade(&mut value, version, 0, STEPS).map_err(ToolError::invalid)?;
    let goal: Goal = serde_json::from_value(value).map_err(|e| ToolError::invalid(format!("The goal record is unreadable: {e}")))?;
    goal.spec.validate()?;
    if goal.spec.milestones.is_empty() || goal.current_milestone > goal.spec.milestones.len() || goal.checkpoints.len() > HISTORY {
        return Err(ToolError::invalid("The goal's saved progress is invalid."));
    }
    Ok(Some(goal))
}

fn save(root: &Path, goal: &mut Goal) -> Result<(), ToolError> {
    let mut bytes = serde_json::to_vec_pretty(goal).map_err(|e| ToolError::internal(e.to_string()))?;
    while bytes.len() as u64 > MAX_BYTES && goal.checkpoints.len() > 1 {
        goal.checkpoints.remove(0);
        bytes = serde_json::to_vec_pretty(goal).map_err(|e| ToolError::internal(e.to_string()))?;
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ToolError::new(ErrorCode::OverLimit, "The goal record is too large."));
    }
    write_atomic(&root.join(FILE), &bytes)
}

pub fn update(root: &Path, edit: impl FnOnce(&mut Goal) -> Result<(), ToolError>) -> Result<Goal, ToolError> {
    let _guard = state_lock(root)?;
    let mut goal = load(root)?.ok_or_else(|| ToolError::new(ErrorCode::NotFound, "This project has no goal yet."))?;
    edit(&mut goal)?;
    goal.revision = next_revision(goal.revision)?;
    goal.updated_ms = now_ms();
    save(root, &mut goal)?;
    Ok(goal)
}

/// Replacing the scope starts a fresh goal; changing only limits retains progress and usage.
pub fn set(root: &Path, spec: GoalSpec, expected_revision: Option<u64>) -> Result<Goal, ToolError> {
    let _guard = state_lock(root)?;
    let _writer = BuildLease::acquire(root)?;
    let old = load(root)?;
    if old.as_ref().is_some_and(|g| g.status == GoalStatus::Active) {
        return Err(ToolError::invalid("Pause the active or queued goal before editing its contract."));
    }
    if old.as_ref().map(|g| g.revision) != expected_revision {
        return Err(ToolError::new(ErrorCode::StaleRevision, "The goal changed. Read it again before saving."));
    }
    let mut goal = Goal::new(spec)?;
    if let Some(old) = old {
        goal.revision = next_revision(old.revision)?;
        let mut previous = old.spec.clone();
        previous.limits = goal.spec.limits.clone();
        if previous == goal.spec {
            goal = Goal { spec: goal.spec, status: GoalStatus::Paused, revision: goal.revision, note: "Limits updated. Resume when ready.".into(), ..old };
        }
    }
    goal.updated_ms = now_ms();
    save(root, &mut goal)?;
    Ok(goal)
}

fn lock(root: &Path, name: &str) -> Result<File, ToolError> {
    let path = root.join(name);
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).map_err(|e| ToolError::io(path.display(), e))?;
    file.try_lock().map_err(|e| match e {
        TryLockError::WouldBlock => ToolError::new(ErrorCode::StaleRevision, "This project is busy. Try again in a moment."),
        TryLockError::Error(e) => ToolError::io(path.display(), e),
    })?;
    Ok(file)
}

fn state_lock(root: &Path) -> Result<File, ToolError> {
    // Short, bounded contention between a pause and a worker checkpoint; never wait on a build.
    for _ in 0..20 {
        match lock(root, STATE_LOCK) {
            Err(e) if e.code == ErrorCode::StaleRevision => std::thread::sleep(std::time::Duration::from_millis(10)),
            r => return r,
        }
    }
    lock(root, STATE_LOCK)
}

fn next_revision(revision: u64) -> Result<u64, ToolError> {
    revision.checked_add(1).ok_or_else(|| ToolError::new(ErrorCode::OverLimit, "The goal's revision limit was reached."))
}

/// An OS-held, non-blocking lease. No stale-PID guessing and no waiting on another writer.
pub struct BuildLease {
    _file: File,
}
impl BuildLease {
    pub fn acquire(root: &Path) -> Result<Self, ToolError> {
        Ok(Self { _file: lock(root, BUILD_LOCK)? })
    }
    pub fn busy(root: &Path) -> Result<bool, ToolError> {
        let path = root.join(BUILD_LOCK);
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(ToolError::io(path.display(), e)),
        };
        match file.try_lock() {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(e)) => Err(ToolError::io(path.display(), e)),
        }
    }
}

pub fn schema() -> String {
    let schema = schemars::generate::SchemaSettings::draft2020_12().into_generator().into_root_schema_for::<Goal>();
    serde_json::to_string_pretty(&schema).map(|s| s + "\n").unwrap_or_default()
}

/// Confirm the destination before lifecycle tools write a goal beside it.
pub fn project(root: &Path) -> Result<Workspace, ToolError> {
    Workspace::open(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> GoalSpec {
        GoalSpec { objective: "Rebuild the sample".into(), constraints: vec![], milestones: vec![], limits: Limits::default() }
    }
    #[test]
    fn completion_requires_fresh_checks_and_all_earlier_milestones_still_passing() {
        let mut s = spec();
        s.milestones = vec![
            Milestone {
                title: "First".into(),
                instructions: String::new(),
                acceptance: vec![Requirement::Check { id: "build".into(), minimum_similarity: None }],
            },
            Milestone {
                title: "Then compare".into(),
                instructions: String::new(),
                acceptance: vec![Requirement::Check { id: "visual".into(), minimum_similarity: Some(95.0) }],
            },
        ];
        let mut g = Goal::new(s).unwrap();
        g.activate().unwrap();
        g.verify(1, &[], &BTreeSet::new());
        assert_ne!(g.status, GoalStatus::Complete);
        let build = CheckResult::new("build", 2, true, "Built");
        g.verify(2, std::slice::from_ref(&build), &BTreeSet::new());
        assert_eq!(g.current_milestone, 1);
        g.verify(3, &[CheckResult { passed: false, round: 3, ..build.clone() }], &BTreeSet::new());
        assert_eq!(g.current_milestone, 0);
        let mut checks = [CheckResult { round: 4, ..build }, CheckResult { similarity: Some(96.0), ..CheckResult::new("visual", 4, true, "Compared") }];
        g.verify(4, &checks, &BTreeSet::new());
        g.verify(5, &checks, &BTreeSet::new());
        assert_ne!(g.status, GoalStatus::Complete);
        for c in &mut checks {
            c.round = 5;
        }
        g.verify(5, &checks, &BTreeSet::new());
        assert_eq!(g.status, GoalStatus::Complete);
    }
    #[test]
    fn a_missing_requirement_or_unknown_never_passes_and_budget_does_not_complete() {
        let mut s = spec();
        s.limits.max_cycles = 1;
        s.milestones = vec![Milestone {
            title: "Evidence".into(),
            instructions: String::new(),
            acceptance: vec![Requirement::ResolvedUnknown { id: "unk_sample".into() }],
        }];
        let mut g = Goal::new(s).unwrap();
        g.activate().unwrap();
        g.verify(1, &[CheckResult::new("build", 1, true, "Built")], &BTreeSet::new());
        assert_eq!(g.status, GoalStatus::BudgetLimited);
    }
    #[test]
    fn file_updates_preserve_pause_use_revisions_and_release_the_project_lease() {
        let d = crate::testing::temp_dir();
        let g = set(&d, spec(), None).unwrap();
        assert!(set(&d, spec(), None).is_err());
        let lease = BuildLease::acquire(&d).unwrap();
        assert!(BuildLease::busy(&d).unwrap());
        assert!(BuildLease::acquire(&d).is_err());
        assert!(set(&d, spec(), Some(g.revision)).is_err());
        drop(lease);
        assert!(!BuildLease::busy(&d).unwrap());
        update(&d, |g| {
            g.status = GoalStatus::Paused;
            Ok(())
        })
        .unwrap();
        assert_eq!(load(&d).unwrap().unwrap().status, GoalStatus::Paused);
    }
    #[test]
    fn a_goal_roundtrips_migrates_and_never_overwrites_a_newer_record() {
        let d = crate::testing::temp_dir();
        let g = set(&d, spec(), None).unwrap();
        let mut value = serde_json::to_value(&g).unwrap();
        value.as_object_mut().unwrap().remove("version");
        std::fs::write(d.join(FILE), serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(load(&d).unwrap().unwrap().version, VERSION);
        value["version"] = 100.into();
        let future = serde_json::to_vec(&value).unwrap();
        std::fs::write(d.join(FILE), &future).unwrap();
        assert!(update(&d, Goal::activate).is_err());
        assert!(set(&d, spec(), Some(g.revision)).is_err());
        assert_eq!(std::fs::read(d.join(FILE)).unwrap(), future);
    }

    #[test]
    fn raising_only_limits_keeps_usage_and_milestones_but_changed_scope_starts_fresh() {
        let d = crate::testing::temp_dir();
        set(&d, spec(), None).unwrap();
        let g = update(&d, |g| {
            g.cycles = 8;
            g.work_ms = 120_000;
            g.tokens = 42;
            g.status = GoalStatus::BudgetLimited;
            Ok(())
        })
        .unwrap();
        let mut raised = g.spec.clone();
        raised.limits.max_cycles = 40;
        let g = set(&d, raised, Some(g.revision)).unwrap();
        assert_eq!((g.cycles, g.work_ms, g.tokens), (8, 120_000, 42));
        assert_eq!(g.status, GoalStatus::Paused);
        let mut changed = g.spec.clone();
        changed.objective = "A different reconstruction".into();
        let g = set(&d, changed, Some(g.revision)).unwrap();
        assert_eq!((g.cycles, g.work_ms, g.tokens), (0, 0, 0));
        assert!(g.checkpoints.is_empty());
    }

    #[test]
    fn published_goal_schema_matches() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/schemas/goal.schema.json");
        assert!(std::fs::read_to_string(path).unwrap() == schema(), "goal.schema.json is stale: run cargo xtask schemas");
    }
}
