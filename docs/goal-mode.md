# Project goals

A goal is a project's durable objective, not a longer rebuild turn. The first version reuses
the existing builder and check registry, retains explicit milestones and limits, and requires
fresh checks before progress can be accepted. It does not claim independent verification of
every feature described in prose: acceptance is exactly the configured project/plugin checks
and evidence-backed unknown resolutions.

## Contract and ownership

`goal.json` is a versioned engine-owned document. It contains the objective, constraints,
milestones, check requirements, resource limits, cumulative usage, latest verification and a
bounded checkpoint history (up to 32 entries within a 2 MiB record). The builder cannot write
this file through its project-file tools. Updates use a short file lock with bounded retries
and atomic replacement. An OS-held project build
lease prevents another desktop, CLI or MCP process from writing the same project concurrently;
it releases on normal exit or a crash without deleting a stale lock file.

Goal states are paused, active, complete, blocked and budget-limited. An active record with no
live project lease means starting or interrupted, not currently building. Restarting the app never silently starts a
goal. Resume keeps cumulative usage and completed milestone position, but runs fresh checks;
previous check records are history, not permission to declare success after another edit.
Changing only limits retains usage and progress; changing the objective, constraints or milestones
starts a fresh contract. A duplicated project starts without a goal.
An ordinary rebuild/follow-up invalidates an earlier completion verdict; the retained acceptance
results become history until the goal is resumed and freshly verified. Manual external file edits
are not continuously fingerprinted in V1.

## Execution

The AI rebuild worker owns goal execution in an independent engine session, preserving the
registered capabilities while leaving the controller free to pause and answer questions.
The first recheck after resume establishes a baseline and revalidates earlier milestones; it
does not skip an unattempted milestone merely because an older build passes. A persisted
`awaiting_check` marks a finished goal turn, allowing its fresh check after an interruption
without repeating that turn. Each bounded cycle targets one milestone, can ask
the original's tools for more evidence, builds, and runs the target kind's existing checks.
Explicit named check requirements must exist and pass; a missing check is never a success.
All target-kind checks must pass as well. Earlier milestones are revalidated against each
new check round. Requested unknowns require a resolution supported by retained evidence.
After repeated cycles without measured improvement, the goal reports its remaining gaps and
waits. A usable build may be previewed without marking the goal complete.

Limits cover completed work cycles, active time and reported input/output tokens. Active-time
limits and pause requests are watched while the builder or a check is running. Token totals
arrive at model/agent turn boundaries, so they are not a hard pre-billed spending guarantee.
Asset spending keeps its existing separate permission and budget.
The existing build concurrency setting limits goal workers in this host. When every slot is
busy, resume fails without queuing; resume again after a slot is free. Cross-process goal
queuing with durable reservations is a follow-up.
Working time is checkpointed at least every five seconds while active and at turn boundaries;
time while paused or with the application closed does not consume the active-time budget.

## Delivery

Lifecycle actions are tools: save/show/pause in the engine and resume in the AI layer. The
desktop uses these tools and reads goal/feed snapshots on background workers. The CLI's
attached `goal` command keeps its process alive and pauses on interruption; persistent MCP
clients can use the same lifecycle tools directly. UI state contains only the editor draft;
orchestration and persistence remain outside the UI.

V1 supports one goal per existing project. Creating the project still uses Make or rebuild.
Closing the app preserves progress and stops its processes; resume is explicit after reopening.
A daemon that keeps working after app exit, new independent behavioural evaluators, automatic
source rollback, and arbitrary verification-command authoring are follow-ups, not implied by V1.

## Using a goal

In the desktop, open a project, choose **Set a goal** in the project details, enter milestones,
acceptance requirements and limits, then **Save goal** and **Resume goal**. Pause before editing.
For tighter acceptance, name the stable `check` ids returned by `project_show`, require an actual
similarity score, or require a recorded unknown's evidence-backed resolution.

Headless clients use `project_goal_set`, `project_goal_show` (paged checkpoint history),
`project_goal_pause` and `project_goal_resume`. Their CLI spellings use hyphens. For example,
save `goal-request.json` with a project and contract:

```json
{
  "project": "./sample-project",
  "goal": {
    "objective": "Rebuild the original's visible interactions",
    "constraints": ["Keep keyboard navigation working"],
    "milestones": [
      {"title": "Core interactions", "instructions": "Implement and verify the main user flow"},
      {"title": "Visual comparison", "acceptance": [{"kind": "check", "id": "web-visual", "minimum_similarity": 95.0}]}
    ],
    "limits": {"max_cycles": 24, "max_minutes": 240, "max_tokens": 500000, "stalled_cycles": 5}
  }
}
```

Use the actual check id for your target (not every project has a visual check):

```sh
craft-anything-cli project-goal-set --json @goal-request.json
craft-anything-cli goal ./sample-project
craft-anything-cli project-goal-show --project ./sample-project --limit 5
craft-anything-cli project-goal-pause --project ./sample-project
```

The attached `goal` command keeps the worker alive; Ctrl+C saves a pause. Questions/approvals
are printed with their ids and can be answered from the app or `project-inbox-answer` in another
terminal. It never blocks control on stdin. Exit codes: 0 completed, 1 blocked/budget/error,
130 interrupted. A one-shot `project-goal-resume` CLI invocation cannot outlive its process;
use `goal` for terminal execution, or the lifecycle tools in a persistent MCP host.

## Verification

Synthetic worker tests cover fresh baselines, two successive milestones beyond the ordinary
round limit, missing requirements, stalled progress, cycle/time/token limits, evidence integrity,
pause precedence, restored check rounds and finished turns, and invalidation by ordinary follow-ups.
A CLI subprocess test runs a synthetic builder/compiler, refuses a second writer, pauses from
another client, and resumes to completion. UI tests save and reopen a goal and pause an interrupted
one; the existing 70 UI smoke tests pass. Live AI quality, a real target's fidelity and uninterrupted
multi-day operation have not been claimed from these tests.
