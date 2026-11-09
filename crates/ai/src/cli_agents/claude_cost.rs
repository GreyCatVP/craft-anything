//! What one Claude Code turn costs (#1187). Its `total_cost_usd` is the cost of the whole session,
//! so a turn that resumed a session reports everything before it too: a live check (2026-11-09)
//! showed the second of two resumed turns reporting $0.00350 where its own tokens cost $0.00147.
//! A turn's cost is therefore the total now less the total kept for the same session after the last
//! turn; a new session's cost is its total; a resumed session with no kept total has no cost,
//! rather than its whole running total.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file in the config folder that keeps each session's last total.
const FILE: &str = "claude-session-costs.json";
const VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
struct Kept {
    version: u32,
    /// Claude Code's session id → the session's running total (USD) after its last turn.
    #[serde(default)]
    totals: BTreeMap<String, f64>,
}

/// Where the kept totals live, in the config folder.
pub fn path_in(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// The kept totals; a missing or unreadable file keeps nothing, so a resumed turn has no cost
/// rather than an inflated one.
fn load(path: &Path) -> Kept {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .filter(|kept| kept.version == VERSION)
        .unwrap_or(Kept { version: VERSION, totals: BTreeMap::new() })
}

/// The cost of a turn that ran in `session`, which resumed `resumed` (None: a new session), given
/// the running total Claude Code reported. Keeps the new total for the next turn.
pub fn turn_cost(path: Option<&Path>, resumed: Option<&str>, session: Option<&str>, total: Option<f64>) -> Option<f64> {
    let total = total.filter(|t| t.is_finite() && *t >= 0.0)?;
    let Some(session) = session.or(resumed) else {
        // Without an id nothing can be kept: a new session's total is still its cost.
        return resumed.is_none().then_some(total);
    };
    let Some(path) = path else {
        return resumed.is_none().then_some(total);
    };
    let mut kept = load(path);
    let cost = match resumed {
        None => Some(total),
        Some(_) => kept.totals.get(session).map(|before| (total - before).max(0.0)),
    };
    kept.totals.insert(session.to_string(), total);
    match serde_json::to_vec_pretty(&kept) {
        Ok(bytes) => {
            if let Err(e) = craft_anything_platform::write_atomic(path, &bytes) {
                log::warn!("couldn't keep Claude Code's session cost in {}: {}", path.display(), e.message);
            }
        }
        Err(e) => log::warn!("couldn't encode Claude Code's session cost: {e}"),
    }
    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept_total(path: &Path, session: &str) -> Option<f64> {
        load(path).totals.get(session).copied()
    }

    /// The live check (2026-11-09, Claude Haiku 5.5): turn 1 reported $0.00203302; turn 2 reported
    /// $0.00350053, a running total. Its own tokens (2 in, 12,121 cache read, 6,723 cache write, 3
    /// out) cost $0.00146751, which is what the turn now reports.
    #[test]
    fn a_resumed_turn_costs_what_it_added_not_the_running_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        assert_eq!(turn_cost(Some(&path), None, Some("s1"), Some(0.00203302)), Some(0.00203302), "a new session's first turn");
        let second = turn_cost(Some(&path), Some("s1"), Some("s1"), Some(0.00350053)).unwrap();
        assert!((second - 0.00146751).abs() < 1e-9, "{second}");
        assert_eq!(kept_total(&path, "s1"), Some(0.00350053), "kept for the next turn");
        let third = turn_cost(Some(&path), Some("s1"), Some("s1"), Some(0.00500000)).unwrap();
        assert!((third - 0.00149947).abs() < 1e-9, "{third}");
    }

    /// Sessions are summed to the same total whatever the number of turns.
    #[test]
    fn the_turns_of_a_session_add_up_to_its_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        let totals = [0.01, 0.025, 0.04, 0.041];
        let mut sum = turn_cost(Some(&path), None, Some("s"), Some(totals[0])).unwrap();
        for pair in totals.windows(2) {
            sum += turn_cost(Some(&path), Some("s"), Some("s"), Some(pair[1])).unwrap();
        }
        assert!((sum - totals[3]).abs() < 1e-12, "{sum}");
    }

    /// A resumed session with no kept total (an older version's run, a lost file) has no cost:
    /// its running total is never counted as one turn.
    #[test]
    fn a_resumed_session_with_nothing_kept_has_no_cost_and_keeps_its_total_for_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        assert_eq!(turn_cost(Some(&path), Some("old"), Some("old"), Some(3.5)), None);
        assert_eq!(turn_cost(Some(&path), Some("old"), Some("old"), Some(3.75)), Some(0.25));
    }

    /// A fork (a question's copy) has its own id: nothing it reports is the builder's session's cost.
    #[test]
    fn a_fork_with_its_own_id_has_no_cost_and_leaves_the_session_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        turn_cost(Some(&path), None, Some("s1"), Some(1.0));
        assert_eq!(turn_cost(Some(&path), Some("s1"), Some("copy"), Some(1.2)), None);
        assert_eq!(kept_total(&path, "s1"), Some(1.0), "the builder's session keeps its own total");
    }

    /// A total that went down (the session was restarted under the same id) costs nothing, not less.
    #[test]
    fn a_total_that_went_down_costs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        turn_cost(Some(&path), None, Some("s"), Some(2.0));
        assert_eq!(turn_cost(Some(&path), Some("s"), Some("s"), Some(1.5)), Some(0.0));
    }

    /// No total, or a total that isn't a number of dollars, is no cost at all.
    #[test]
    fn a_missing_or_invalid_total_is_no_cost() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        for total in [None, Some(f64::NAN), Some(-1.0), Some(f64::INFINITY)] {
            assert_eq!(turn_cost(Some(&path), None, Some("s"), total), None, "{total:?}");
        }
    }

    /// A damaged file keeps nothing, and a later write replaces it.
    #[test]
    fn a_damaged_file_keeps_nothing_and_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(turn_cost(Some(&path), Some("s"), Some("s"), Some(2.0)), None, "nothing kept, so no cost");
        assert_eq!(kept_total(&path, "s"), Some(2.0), "the file is rewritten");
    }

    /// Without a folder to keep totals in, a new session still has its cost and a resumed one none.
    #[test]
    fn without_a_folder_only_new_sessions_have_a_cost() {
        assert_eq!(turn_cost(None, None, Some("s"), Some(0.5)), Some(0.5));
        assert_eq!(turn_cost(None, Some("s"), Some("s"), Some(0.9)), None);
    }
}
