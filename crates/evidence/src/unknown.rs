//! Residual unknowns: open questions, recorded with what would settle them.

use crate::{ErrorCode, ToolError, canonical_json, sha256_hex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnknownStatus {
    #[default]
    Open,
    Investigating,
    Blocked,
    Contradicted,
    Resolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

/// Who settled a resolved unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedBy {
    /// Evidence did: the resolution cites it in `evidence_for`.
    #[default]
    Evidence,
    /// The person answered it: their answer is the resolution, and no evidence is needed.
    Person,
}

impl ResolvedBy {
    /// The default, left out of saved records (so records from before it existed read the same).
    pub fn is_evidence(&self) -> bool {
        *self == ResolvedBy::Evidence
    }
}

/// One open question. Revisions use optimistic concurrency: an update must name the revision it
/// was based on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unknown {
    /// `unk_` + 16 hex digits, derived from the first revision's question and domain.
    pub id: String,
    pub revision: u64,
    pub question: String,
    pub severity: Severity,
    /// Area: `native`, `network`, `ui`, `storage`, …
    pub domain: String,
    pub status: UnknownStatus,
    /// What would settle it: probes, tools to run, facts to check.
    #[serde(default)]
    pub probes: Vec<String>,
    #[serde(default)]
    pub evidence_for: Vec<String>,
    #[serde(default)]
    pub evidence_against: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// Short answers the person could pick, best first (at most [`Unknown::MAX_ANSWERS`], each
    /// at most [`Unknown::MAX_ANSWER_CHARS`] characters; set them with
    /// [`Unknown::set_suggested_answers`]). Absent in records from before they existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_answers: Vec<String>,
    /// Who settled it, once resolved (evidence unless the person answered it).
    #[serde(default, skip_serializing_if = "ResolvedBy::is_evidence")]
    pub resolved_by: ResolvedBy,
}

impl Unknown {
    pub fn new(question: impl Into<String>, domain: impl Into<String>, severity: Severity) -> Self {
        let question = question.into();
        let domain = domain.into();
        let seed = serde_json::json!({"question": question, "domain": domain});
        let digest = sha256_hex(canonical_json(&seed).as_bytes());
        let short = digest.get(..16).unwrap_or(&digest).to_string();
        Unknown {
            id: format!("unk_{short}"),
            revision: 1,
            question,
            severity,
            domain,
            status: UnknownStatus::Open,
            probes: Vec::new(),
            evidence_for: Vec::new(),
            evidence_against: Vec::new(),
            resolution: None,
            suggested_answers: Vec::new(),
            resolved_by: ResolvedBy::Evidence,
        }
    }

    /// How many suggested answers a question keeps.
    pub const MAX_ANSWERS: usize = 3;
    /// How long one suggested answer may be, in characters.
    pub const MAX_ANSWER_CHARS: usize = 120;

    /// Replace the suggested answers with `answers`, tidied: trimmed, blanks and repeats left
    /// out, each shortened to [`Self::MAX_ANSWER_CHARS`] characters, the first
    /// [`Self::MAX_ANSWERS`] kept. Returns a plain note for each change made, so a caller can
    /// say what happened instead of dropping anything silently.
    pub fn set_suggested_answers(&mut self, answers: &[String]) -> Vec<String> {
        let mut notes = Vec::new();
        let mut kept: Vec<String> = Vec::new();
        let mut blank = 0usize;
        let mut repeated = 0usize;
        let mut over = 0usize;
        for (i, raw) in answers.iter().enumerate() {
            let a = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if a.is_empty() {
                blank = blank.saturating_add(1);
                continue;
            }
            let long = a.chars().count() > Self::MAX_ANSWER_CHARS;
            let a = if long {
                let cut: String = a.chars().take(Self::MAX_ANSWER_CHARS.saturating_sub(1)).collect();
                format!("{}…", cut.trim_end())
            } else {
                a
            };
            if kept.iter().any(|k| k.eq_ignore_ascii_case(&a)) {
                repeated = repeated.saturating_add(1);
            } else if kept.len() >= Self::MAX_ANSWERS {
                over = over.saturating_add(1);
            } else {
                // Said only of an answer that's kept: one left out is counted below instead.
                if long {
                    notes.push(format!("suggested answer {} was shortened to {} characters; keep answers short", i.saturating_add(1), Self::MAX_ANSWER_CHARS));
                }
                kept.push(a);
            }
        }
        if blank > 0 {
            notes.push(format!("{blank} blank suggested answer(s) left out"));
        }
        if repeated > 0 {
            notes.push(format!("{repeated} repeated suggested answer(s) left out"));
        }
        if over > 0 {
            notes.push(format!("kept the first {} suggested answers; {over} more left out", Self::MAX_ANSWERS));
        }
        self.suggested_answers = kept;
        notes
    }

    /// Replace this record with `next` if `based_on_revision` matches; bumps the revision.
    pub fn apply_update(&mut self, based_on_revision: u64, mut next: Unknown) -> Result<(), ToolError> {
        if based_on_revision != self.revision {
            return Err(ToolError::new(ErrorCode::StaleRevision, format!("{} is at revision {}, not {}", self.id, self.revision, based_on_revision))
                .with_remediation("Read the unknown again and retry with its current revision."));
        }
        if self.status == UnknownStatus::Resolved && next.status == UnknownStatus::Resolved && next.resolution.is_none() {
            return Err(ToolError::invalid("A resolved unknown needs a resolution."));
        }
        next.id = self.id.clone();
        next.revision = self.revision.saturating_add(1);
        *self = next;
        Ok(())
    }

    /// A resolved unknown must say how, and cite at least one piece of evidence, unless the
    /// person answered it ([`ResolvedBy::Person`]: their answer is the resolution).
    pub fn resolution_is_valid(&self) -> bool {
        self.status == UnknownStatus::Resolved
            && self.resolution.as_deref().is_some_and(|r| !r.trim().is_empty())
            && (self.resolved_by == ResolvedBy::Person || !self.evidence_for.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimistic_concurrency() {
        let mut u = Unknown::new("Where is the % rule implemented?", "native", Severity::High);
        assert!(u.id.starts_with("unk_"));
        let mut next = u.clone();
        next.status = UnknownStatus::Investigating;
        u.apply_update(1, next.clone()).unwrap();
        assert_eq!(u.revision, 2);
        let err = u.apply_update(1, next).unwrap_err();
        assert_eq!(err.code, ErrorCode::StaleRevision);
    }

    #[test]
    fn a_record_from_before_suggested_answers_still_loads_and_saves_the_same() {
        let old = r#"{"id":"unk_0123456789abcdef","revision":2,"question":"Which font?","severity":"high","domain":"ui","status":"open","probes":["look at the CSS"]}"#;
        let u: Unknown = serde_json::from_str(old).unwrap();
        assert!(u.suggested_answers.is_empty());
        assert_eq!(u.probes, ["look at the CSS"]);
        let saved = serde_json::to_string(&u).unwrap();
        assert!(!saved.contains("suggested_answers"), "an empty list isn't written: {saved}");
        let mut with = u.clone();
        with.suggested_answers = vec!["Inter".into(), "A system font".into()];
        let back: Unknown = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back, with);
        assert_eq!(u.resolved_by, ResolvedBy::Evidence, "records from before were settled by evidence");
    }

    #[test]
    fn the_persons_answer_settles_a_question_without_evidence() {
        let mut u = Unknown::new("Reuse the logo?", "ui", Severity::Medium);
        u.status = UnknownStatus::Resolved;
        u.resolution = Some("The person said: use placeholders".into());
        assert!(!u.resolution_is_valid(), "evidence is still needed by default");
        u.resolved_by = ResolvedBy::Person;
        assert!(u.resolution_is_valid());
        let saved = serde_json::to_string(&u).unwrap();
        assert!(saved.contains(r#""resolved_by":"person""#), "{saved}");
        assert_eq!(serde_json::from_str::<Unknown>(&saved).unwrap(), u);
        u.resolution = Some("  ".into());
        assert!(!u.resolution_is_valid(), "it still has to say what was decided");
        u.resolved_by = ResolvedBy::Evidence;
        assert!(!serde_json::to_string(&u).unwrap().contains("resolved_by"), "the default isn't written");
    }

    #[test]
    fn suggested_answers_are_tidied_and_every_change_is_said() {
        let mut u = Unknown::new("Which font?", "ui", Severity::Medium);
        let long = "word ".repeat(40);
        let notes = u.set_suggested_answers(&["  Inter \n".into(), String::new(), "inter".into(), long, "Roboto".into(), "A system font".into()]);
        assert_eq!(u.suggested_answers.len(), Unknown::MAX_ANSWERS);
        assert_eq!(u.suggested_answers[0], "Inter");
        assert!(u.suggested_answers[1].ends_with('…') && u.suggested_answers[1].chars().count() <= Unknown::MAX_ANSWER_CHARS);
        assert_eq!(u.suggested_answers[2], "Roboto");
        let all = notes.join("; ");
        for said in ["shortened", "1 blank", "1 repeated", "kept the first 3"] {
            assert!(all.contains(said), "{said} in {all}");
        }
        assert!(u.set_suggested_answers(&["Yes".into(), "No".into()]).is_empty(), "nothing to say when nothing changed");
        assert_eq!(u.suggested_answers, ["Yes", "No"]);
        // A long answer that's left out anyway is counted as left out, not also as shortened.
        let notes = u.set_suggested_answers(&["A".into(), "B".into(), "C".into(), "word ".repeat(40)]).join("; ");
        assert!(!notes.contains("shortened") && notes.contains("1 more left out"), "{notes}");
        let notes = u.set_suggested_answers(&["word ".repeat(40), "word ".repeat(41)]).join("; ");
        assert_eq!(u.suggested_answers.len(), 1, "the same once shortened");
        assert!(notes.contains("suggested answer 1 was shortened") && !notes.contains("answer 2 was") && notes.contains("1 repeated"), "{notes}");
        u.set_suggested_answers(&[]);
        assert!(u.suggested_answers.is_empty());
    }
}
