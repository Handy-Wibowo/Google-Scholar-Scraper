//! Review state: the reviewer's decisions, notes and tags per record.
//!
//! `Article` is a pure bibliographic row with no user-owned fields, and
//! `AppState` holds one flat `Vec<Article>`. Screening a systematic review needs
//! per-record intellectual state, so it lives here instead: a project document
//! keyed by DOI (or normalized title when a record has no DOI), which survives
//! re-searches because the key is bibliographic rather than positional.
//!
//! Everything in this module is pure — no filesystem, no network — so the rules
//! are unit-testable offline. `gss-app` owns the file.

use crate::models::Article;
use std::collections::BTreeMap;

/// What a reviewer decided about a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Decision {
    Include,
    Exclude,
    Maybe,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
            Self::Maybe => "maybe",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "include" => Some(Self::Include),
            "exclude" => Some(Self::Exclude),
            "maybe" => Some(Self::Maybe),
            _ => None,
        }
    }
}

/// Everything the reviewer recorded about one record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewEntry {
    /// Empty when undecided.
    pub decision: String,
    /// Coded exclusion reason (`"wrong population"`, `"duplicate"`, …).
    pub reason: String,
    pub note: String,
    pub tags: Vec<String>,
    /// Unix seconds of the last change, for the audit log.
    pub updated_at: u64,
    /// The query that surfaced the record, for PRISMA provenance.
    pub found_by: String,
}

impl ReviewEntry {
    pub fn is_empty(&self) -> bool {
        self.decision.is_empty()
            && self.reason.is_empty()
            && self.note.is_empty()
            && self.tags.is_empty()
    }

    pub fn decision(&self) -> Option<Decision> {
        Decision::parse(&self.decision)
    }
}

/// Stable identity for a record: DOI when present, else the normalized title.
///
/// Deliberately not the row index: re-running a search or appending results
/// reorders rows, and a decision must follow the paper, not the position.
pub fn record_key(article: &Article) -> String {
    let doi = crate::text::normalize_doi(&article.doi);
    if !doi.is_empty() {
        return format!("doi:{}", doi.to_lowercase());
    }
    let title = crate::text::normalize_title_key(&article.title);
    if title.is_empty() {
        // Last resort: a record with neither is not addressable; callers skip it.
        return String::new();
    }
    format!("title:{}", title)
}

/// The project document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReviewProject {
    pub version: u32,
    /// Keyed by `record_key`.
    pub entries: BTreeMap<String, ReviewEntry>,
    /// Named baselines: query -> set of record keys, for the "what's new" diff.
    pub baselines: BTreeMap<String, Vec<String>>,
}

/// Layout version for the project document.
pub const PROJECT_VERSION: u32 = 1;

impl ReviewProject {
    pub fn new() -> Self {
        Self {
            version: PROJECT_VERSION,
            entries: BTreeMap::new(),
            baselines: BTreeMap::new(),
        }
    }

    /// Record a decision, creating the entry when needed.
    pub fn set_decision(&mut self, key: &str, decision: Decision, reason: &str, now: u64) {
        if key.is_empty() {
            return;
        }
        let entry = self.entries.entry(key.to_string()).or_default();
        entry.decision = decision.as_str().to_string();
        // Only an exclusion carries a reason; clearing it otherwise keeps stale
        // text from reappearing when the reviewer changes their mind.
        entry.reason = if decision == Decision::Exclude {
            reason.trim().to_string()
        } else {
            String::new()
        };
        entry.updated_at = now;
    }

    /// Attach a note (empty clears it).
    pub fn set_note(&mut self, key: &str, note: &str, now: u64) {
        if key.is_empty() {
            return;
        }
        let entry = self.entries.entry(key.to_string()).or_default();
        entry.note = note.trim().to_string();
        entry.updated_at = now;
    }

    /// Add a tag (idempotent) or remove it with `remove = true`.
    pub fn toggle_tag(&mut self, key: &str, tag: &str, remove: bool, now: u64) {
        if key.is_empty() {
            return;
        }
        let tag = tag.trim();
        if tag.is_empty() {
            return;
        }
        let entry = self.entries.entry(key.to_string()).or_default();
        if remove {
            entry.tags.retain(|t| t != tag);
        } else if !entry.tags.iter().any(|t| t == tag) {
            entry.tags.push(tag.to_string());
        }
        entry.updated_at = now;
    }

    /// Remember which query surfaced a record (first query wins: the earliest
    /// provenance is what a methods section reports).
    pub fn note_source(&mut self, key: &str, query: &str) {
        if key.is_empty() || query.trim().is_empty() {
            return;
        }
        let entry = self.entries.entry(key.to_string()).or_default();
        if entry.found_by.is_empty() {
            entry.found_by = query.trim().to_string();
        }
    }

    pub fn entry(&self, key: &str) -> Option<&ReviewEntry> {
        self.entries.get(key)
    }

    /// PRISMA-style counts over a result set.
    pub fn prisma(&self, articles: &[Article]) -> PrismaCounts {
        let mut counts = PrismaCounts {
            identified: articles.len(),
            ..PrismaCounts::default()
        };
        for article in articles {
            let key = record_key(article);
            let Some(entry) = self.entries.get(&key) else {
                counts.undecided += 1;
                continue;
            };
            match entry.decision() {
                Some(Decision::Include) => counts.included += 1,
                Some(Decision::Exclude) => {
                    counts.excluded += 1;
                    if !entry.reason.is_empty() {
                        *counts.by_reason.entry(entry.reason.clone()).or_insert(0) += 1;
                    }
                }
                Some(Decision::Maybe) => counts.maybe += 1,
                None => counts.undecided += 1,
            }
        }
        counts
    }

    /// One audit row per decided record, in a stable order.
    pub fn screening_log(&self, articles: &[Article]) -> Vec<ScreeningLogRow> {
        let mut rows: Vec<ScreeningLogRow> = Vec::new();
        for article in articles {
            let key = record_key(article);
            let Some(entry) = self.entries.get(&key) else {
                continue;
            };
            if entry.decision.is_empty() && entry.note.is_empty() && entry.tags.is_empty() {
                continue;
            }
            rows.push(ScreeningLogRow {
                title: article.title.clone(),
                doi: article.doi.clone(),
                decision: entry.decision.clone(),
                reason: entry.reason.clone(),
                tags: entry.tags.join("; "),
                note: entry.note.clone(),
                found_by: entry.found_by.clone(),
                updated_at: entry.updated_at,
                sjr_quartile: article.sjr_quartile.clone(),
                sinta_rank: article.sinta_rank.clone(),
            });
        }
        rows
    }

    /// Remember the current result set under a name, for a later diff.
    pub fn save_baseline(&mut self, name: &str, articles: &[Article]) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        let keys: Vec<String> = articles
            .iter()
            .map(record_key)
            .filter(|k| !k.is_empty())
            .collect();
        self.baselines.insert(trimmed.to_string(), keys);
    }

    /// Which records are new or gone relative to a stored baseline.
    pub fn diff_baseline(&self, name: &str, articles: &[Article]) -> Option<BaselineDiff> {
        let previous = self.baselines.get(name.trim())?;
        let current: Vec<String> = articles
            .iter()
            .map(record_key)
            .filter(|k| !k.is_empty())
            .collect();
        let added: Vec<usize> = current
            .iter()
            .enumerate()
            .filter(|(_, key)| !previous.contains(key))
            .map(|(index, _)| index)
            .collect();
        let removed = previous.iter().filter(|key| !current.contains(key)).count();
        let unchanged = current.len() - added.len();
        Some(BaselineDiff {
            added,
            removed,
            unchanged,
        })
    }
}

/// PRISMA flow counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrismaCounts {
    pub identified: usize,
    pub included: usize,
    pub excluded: usize,
    pub maybe: usize,
    pub undecided: usize,
    pub by_reason: BTreeMap<String, usize>,
}

impl PrismaCounts {
    /// Human summary for the status line.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} identified · {} included · {} excluded · {} maybe · {} undecided",
            self.identified, self.included, self.excluded, self.maybe, self.undecided
        );
        if !self.by_reason.is_empty() {
            let parts: Vec<String> = self
                .by_reason
                .iter()
                .map(|(reason, count)| format!("{reason} ({count})"))
                .collect();
            out.push_str(&format!(" | excluded by reason: {}", parts.join(", ")));
        }
        out
    }
}

/// One row of the screening log export.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ScreeningLogRow {
    pub title: String,
    pub doi: String,
    pub decision: String,
    pub reason: String,
    pub tags: String,
    pub note: String,
    pub found_by: String,
    pub updated_at: u64,
    pub sjr_quartile: String,
    pub sinta_rank: String,
}

/// Result of comparing the current set to a baseline.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct BaselineDiff {
    /// Indices into the current slice.
    pub added: Vec<usize>,
    pub removed: usize,
    pub unchanged: usize,
}

/// A near-duplicate pair for human confirmation.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct NearDuplicate {
    /// Indices into the current slice, `first < second`.
    pub first: usize,
    pub second: usize,
    pub similarity: f64,
}

/// Find likely duplicate pairs by title-token overlap.
///
/// Exact normalized-title dedupe already ran at search time; this catches the
/// classic review double-count — "Deep learning for X" beside "Deep learning for
/// X: a review" — and leaves the merge decision to a human.
pub fn find_near_duplicates(articles: &[Article], threshold: f64) -> Vec<NearDuplicate> {
    let tokens: Vec<Vec<String>> = articles
        .iter()
        .map(|a| crate::ranking::tokenize(&a.title))
        .collect();
    let mut out: Vec<NearDuplicate> = Vec::new();
    for first in 0..articles.len() {
        if tokens[first].is_empty() {
            continue;
        }
        for second in (first + 1)..articles.len() {
            if tokens[second].is_empty() {
                continue;
            }
            // Skip pairs that are already identical records.
            if !articles[first].doi.is_empty()
                && crate::text::normalize_doi(&articles[first].doi)
                    == crate::text::normalize_doi(&articles[second].doi)
            {
                continue;
            }
            let similarity = cosine(&tokens[first], &tokens[second]);
            if similarity >= threshold {
                out.push(NearDuplicate {
                    first,
                    second,
                    similarity,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Token-count cosine, used for near-duplicate scoring.
fn cosine(a: &[String], b: &[String]) -> f64 {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, (f64, f64)> = HashMap::new();
    for token in a {
        counts.entry(token.as_str()).or_insert((0.0, 0.0)).0 += 1.0;
    }
    for token in b {
        counts.entry(token.as_str()).or_insert((0.0, 0.0)).1 += 1.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in counts.values() {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(doi: &str, title: &str) -> Article {
        Article {
            doi: doi.to_string(),
            title: title.to_string(),
            ..Article::default()
        }
    }

    #[test]
    fn key_prefers_doi_then_title() {
        assert_eq!(record_key(&article("10.1/ABC", "x")), "doi:10.1/abc");
        assert_eq!(record_key(&article("", "A Title!")), "title:atitle");
        assert_eq!(record_key(&article("", "")), "");
    }

    #[test]
    fn decisions_survive_a_key_stable_identity() {
        let mut project = ReviewProject::new();
        let a = article("10.1/x", "A");
        let key = record_key(&a);
        project.set_decision(&key, Decision::Include, "", 100);
        // The same paper found again, different row order, still resolves.
        assert_eq!(
            project.entry(&key).and_then(|e| e.decision()),
            Some(Decision::Include)
        );
    }

    #[test]
    fn exclusion_keeps_a_reason_and_other_decisions_clear_it() {
        let mut project = ReviewProject::new();
        project.set_decision("doi:a", Decision::Exclude, "wrong population", 1);
        assert_eq!(project.entry("doi:a").unwrap().reason, "wrong population");

        project.set_decision("doi:a", Decision::Include, "wrong population", 2);
        assert!(
            project.entry("doi:a").unwrap().reason.is_empty(),
            "an include must not keep the exclusion reason"
        );
    }

    #[test]
    fn tags_are_idempotent_and_removable() {
        let mut project = ReviewProject::new();
        project.toggle_tag("doi:a", "theory", false, 1);
        project.toggle_tag("doi:a", "theory", false, 2);
        assert_eq!(project.entry("doi:a").unwrap().tags, vec!["theory"]);
        project.toggle_tag("doi:a", "theory", true, 3);
        assert!(project.entry("doi:a").unwrap().tags.is_empty());
    }

    #[test]
    fn empty_key_is_ignored_everywhere() {
        let mut project = ReviewProject::new();
        project.set_decision("", Decision::Include, "", 1);
        project.set_note("", "x", 1);
        project.toggle_tag("", "t", false, 1);
        assert!(project.entries.is_empty());
    }

    #[test]
    fn prisma_counts_every_record_exactly_once() {
        let mut project = ReviewProject::new();
        let arts = vec![
            article("10.1/a", "A"),
            article("10.1/b", "B"),
            article("10.1/c", "C"),
            article("10.1/d", "D"),
        ];
        project.set_decision(&record_key(&arts[0]), Decision::Include, "", 1);
        project.set_decision(&record_key(&arts[1]), Decision::Exclude, "off topic", 1);
        project.set_decision(&record_key(&arts[2]), Decision::Maybe, "", 1);
        let counts = project.prisma(&arts);
        assert_eq!(counts.identified, 4);
        assert_eq!(counts.included, 1);
        assert_eq!(counts.excluded, 1);
        assert_eq!(counts.maybe, 1);
        assert_eq!(counts.undecided, 1);
        assert_eq!(counts.by_reason.get("off topic"), Some(&1));
        assert!(counts.summary().contains("1 included"));
    }

    #[test]
    fn screening_log_only_lists_touched_records() {
        let mut project = ReviewProject::new();
        let arts = vec![article("10.1/a", "A"), article("10.1/b", "B")];
        project.set_decision(&record_key(&arts[0]), Decision::Exclude, "dup", 5);
        project.note_source(&record_key(&arts[0]), "crispr");
        let rows = project.screening_log(&arts);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, "exclude");
        assert_eq!(rows[0].reason, "dup");
        assert_eq!(rows[0].found_by, "crispr");
    }

    #[test]
    fn baseline_diff_reports_added_and_removed() {
        let mut project = ReviewProject::new();
        let before = vec![article("10.1/a", "A"), article("10.1/b", "B")];
        project.save_baseline("week1", &before);

        let after = vec![
            article("10.1/a", "A"),
            article("10.1/b", "B"),
            article("10.1/c", "C"),
        ];
        let diff = project.diff_baseline("week1", &after).expect("baseline");
        assert_eq!(diff.added, vec![2]);
        assert_eq!(diff.removed, 0);
        assert_eq!(diff.unchanged, 2);

        let shrunk = vec![article("10.1/a", "A")];
        let diff = project.diff_baseline("week1", &shrunk).unwrap();
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, 1);
    }

    #[test]
    fn unknown_baseline_is_none() {
        let project = ReviewProject::new();
        assert!(project.diff_baseline("nope", &[]).is_none());
    }

    #[test]
    fn near_duplicates_find_a_subtitle_variant_but_not_unrelated() {
        let arts = vec![
            article("10.1/a", "Deep learning for molecules"),
            article("10.1/b", "Deep learning for molecules: a review"),
            article("10.1/c", "Protein folding with transformers"),
        ];
        let found = find_near_duplicates(&arts, 0.8);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].first, found[0].second), (0, 1));
        assert!(found[0].similarity >= 0.8);
    }

    #[test]
    fn near_duplicates_skip_the_same_doi() {
        let arts = vec![
            article("10.1/same", "One title"),
            article("10.1/SAME", "One title"),
        ];
        assert!(find_near_duplicates(&arts, 0.5).is_empty());
    }
}
