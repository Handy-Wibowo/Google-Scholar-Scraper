//! Local journal ranking catalog ported from `journal_catalog.py` verbatim.
//! CSV/XLSX imports (transactional), ISSN-then-title matching, one winner
//! per ranking system, ambiguous latest year dropped.

use crate::models::{normalize_issn, normalize_issn_list, Article};
use crate::text::{is_combining_pub, CaseFold};
use regex::Regex;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

pub const SJR_QUARTILES: [&str; 4] = ["Q1", "Q2", "Q3", "Q4"];
pub const SINTA_RANKS: [&str; 6] = ["S1", "S2", "S3", "S4", "S5", "S6"];

static SJR_RANK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Q[1-4]$").unwrap());
static SINTA_RANK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^S[1-6]$").unwrap());
static NON_ALNUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^0-9a-z]+").unwrap());
static HEADER_NON_ALNUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]").unwrap());
static ISSN_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[;,/|\s]+").unwrap());
static YEAR_SEARCH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(1[5-9]\d\d|2[01]\d\d)").unwrap());
static ISSN_FROM_DOI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"10\.\d{4,9}/(\d{4}-\d{3}[\dX])").unwrap());
static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Header aliases (`_COLUMN_ALIASES`), all keys pre-reduced via header_key.
const COLUMN_ALIASES: [(&str, [&str; 7], usize); 6] = [
    (
        "title",
        [
            "title",
            "journal",
            "journaltitle",
            "journalname",
            "name",
            "source",
            "sourcetitle",
        ],
        7,
    ),
    (
        "issn",
        [
            "issn",
            "issns",
            "issnprint",
            "issnonline",
            "pissn",
            "eissn",
            "issnnumbers",
        ],
        7,
    ),
    (
        "rank",
        [
            "sjrbestquartile",
            "bestquartile",
            "quartile",
            "sjrquartile",
            "sinta rank",
            "sintarank",
            "rank",
        ],
        7,
    ),
    (
        "year",
        [
            "year",
            "rankingyear",
            "sjryear",
            "sintayear",
            "accreditationyear",
            "date",
            "",
        ],
        6,
    ),
    (
        "category",
        [
            "categories",
            "category",
            "subjectarea",
            "subjectareas",
            "subjectcategory",
            "field",
            "",
        ],
        6,
    ),
    (
        "source_url",
        ["sourceurl", "url", "link", "source", "sourcelink", "", ""],
        5,
    ),
];

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalRankRecord {
    pub title: String,
    pub title_key: String,
    pub issns: Vec<String>,
    pub kind: String,
    pub rank: String,
    pub year: Option<i32>,
    pub category: String,
    pub source_url: String,
}

/// The winning record for one journal, provenance intact (`JournalRank`).
#[derive(Debug, Clone, PartialEq)]
pub struct JournalRank {
    pub kind: String,
    pub rank: String,
    pub year: Option<i32>,
    pub category: String,
    pub source_url: String,
    pub matched_by: String,
}

/// A local store of imported journal ranking records.
#[derive(Debug, Default, Clone)]
pub struct JournalCatalog {
    records: Vec<JournalRankRecord>,
    by_issn: HashMap<String, Vec<usize>>,
    by_title: HashMap<String, Vec<usize>>,
}

impl JournalCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &[JournalRankRecord] {
        &self.records
    }

    /// `import_file`: CSV/XLSX in; transactional — rows are parsed and
    /// validated first; tables rebuilt once the whole file parsed.
    /// Returns the number of imported records.
    pub fn import_file(&mut self, path: &Path, default_year: Option<i32>) -> Result<usize, String> {
        let suffix = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let (headers, rows): (Vec<String>, Vec<Vec<Cell>>) = match suffix.as_str() {
            "xlsx" | "xlsm" => read_xlsx(path)?,
            "csv" | "tsv" | "txt" => {
                read_csv(path, if suffix == "tsv" { Some('\t') } else { None })?
            }
            other => {
                let label = if other.is_empty() {
                    "(none)".to_string()
                } else {
                    format!(".{other}")
                };
                return Err(format!("Unsupported ranking file type: {label}"));
            }
        };

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let incoming = build_records(&headers, &rows, &file_name, default_year)?;

        let mut records = self.records.clone();
        records.extend(incoming.iter().cloned());

        let mut by_issn: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, record) in records.iter().enumerate() {
            for issn in &record.issns {
                by_issn.entry(issn.clone()).or_default().push(index);
            }
            if !record.title_key.is_empty() {
                by_title
                    .entry(record.title_key.clone())
                    .or_default()
                    .push(index);
            }
        }

        self.records = records;
        self.by_issn = by_issn;
        self.by_title = by_title;
        Ok(incoming.len())
    }

    /// Rebuild a catalog from previously exported records.
    ///
    /// The ISSN/title index maps are derived, so a restored catalog behaves
    /// exactly like one produced by `import_file`. Records are taken as given:
    /// they were validated on the way in.
    pub fn from_records(records: Vec<JournalRankRecord>) -> Self {
        let mut by_issn: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, record) in records.iter().enumerate() {
            for issn in &record.issns {
                by_issn.entry(issn.clone()).or_default().push(index);
            }
            if !record.title_key.is_empty() {
                by_title
                    .entry(record.title_key.clone())
                    .or_default()
                    .push(index);
            }
        }
        Self {
            records,
            by_issn,
            by_title,
        }
    }

    /// Records in a form that survives a JSON round-trip.
    pub fn to_records(&self) -> Vec<JournalRankRecord> {
        self.records.clone()
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.by_issn.clear();
        self.by_title.clear();
    }

    /// `match_all`: at most one best record per ranking system; ISSNs first
    /// (all considered together), exact normalized title only as fallback.
    pub fn match_all<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        issns: I,
        journal: &str,
    ) -> Vec<JournalRank> {
        let mut matched_by = String::new();
        let mut candidates: Vec<&JournalRankRecord> = Vec::new();
        for issn in normalize_issn_list(issns) {
            if let Some(indices) = self.by_issn.get(&issn) {
                candidates.extend(indices.iter().map(|i| &self.records[*i]));
                if matched_by.is_empty() {
                    matched_by = format!("issn:{issn}");
                }
            }
        }

        if candidates.is_empty() {
            let title_key = journal_title_key(journal);
            if title_key.is_empty() {
                return Vec::new();
            }
            if let Some(indices) = self.by_title.get(&title_key) {
                candidates.extend(indices.iter().map(|i| &self.records[*i]));
            }
            matched_by = "title".to_string();
        }

        resolve_candidates(&candidates)
            .into_iter()
            .map(|record| JournalRank {
                kind: record.kind.clone(),
                rank: record.rank.clone(),
                year: record.year,
                category: record.category.clone(),
                source_url: record.source_url.clone(),
                matched_by: matched_by.clone(),
            })
            .collect()
    }

    /// `match`: single best — latest year wins, SJR preferred on a tie.
    pub fn match_one<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        issns: I,
        journal: &str,
    ) -> Option<JournalRank> {
        let ranks = self.match_all(issns, journal);
        let mut best = ranks.first()?.clone();
        for rank in &ranks[1..] {
            let candidate_better = year_or_floor(rank.year) > year_or_floor(best.year)
                || (year_or_floor(rank.year) == year_or_floor(best.year) && rank.kind == "sjr");
            if candidate_better {
                best = rank.clone();
            }
        }
        Some(best)
    }
}

/// Conservative journal-title key: drop standalone leading "The" as a word,
/// remove case/punct/diacritics/whitespace. No fuzzy matching.
pub fn journal_title_key(value: &str) -> String {
    let normalized: String = value.nfkd().collect();
    let decomposed: String = normalized
        .chars()
        .filter(|c| !is_combining_pub(*c))
        .collect();
    let mut words: Vec<&str> = decomposed.split_whitespace().collect();
    if words.len() > 1 && words[0].casefold() == "the" {
        words.remove(0);
    }
    NON_ALNUM_RE
        .replace_all(&words.join("").casefold(), "")
        .into_owned()
}

fn year_or_floor(year: Option<i32>) -> i32 {
    year.unwrap_or(-1)
}

fn resolve_candidates(candidates: &[&JournalRankRecord]) -> Vec<JournalRankRecord> {
    let mut winners: HashMap<String, JournalRankRecord> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();

    for candidate in candidates {
        match winners.get(&candidate.kind) {
            None => {
                winners.insert(candidate.kind.clone(), (*candidate).clone());
                order.push(candidate.kind.clone());
            }
            Some(current) => {
                let current_year = year_or_floor(current.year);
                let candidate_year = year_or_floor(candidate.year);
                if candidate_year > current_year {
                    winners.insert(candidate.kind.clone(), (*candidate).clone());
                    ambiguous.remove(&candidate.kind);
                } else if candidate_year == current_year && candidate.rank != current.rank {
                    ambiguous.insert(candidate.kind.clone());
                }
            }
        }
    }

    order
        .into_iter()
        .filter_map(|kind| winners.remove(&kind))
        .filter(|record| !ambiguous.contains(&record.kind))
        .collect()
}

/// Cell values as the catalog sees them (strings, or numbers preserved for
/// year parsing).
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Num(f64),
    Empty,
}

impl Cell {
    pub fn as_raw(&self) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Num(n) => {
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            Self::Empty => String::new(),
        }
    }

    pub fn is_blank(&self) -> bool {
        match self {
            Self::Text(t) => t.trim().is_empty(),
            Self::Num(_) => false,
            Self::Empty => true,
        }
    }
}

fn header_key(value: &str) -> String {
    HEADER_NON_ALNUM_RE
        .replace_all(&value.casefold(), "")
        .into_owned()
}

fn resolve_columns(headers: &[String]) -> HashMap<&'static str, usize> {
    let keyed: HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (header_key(h), i))
        .collect();
    let mut resolved = HashMap::new();
    for (field, aliases, _) in COLUMN_ALIASES {
        for alias in aliases {
            if alias.is_empty() {
                break;
            }
            let alias_key = header_key(alias);
            if let Some(index) = keyed.get(&alias_key) {
                resolved.insert(field, *index);
                break;
            }
        }
    }
    resolved
}

fn read_csv(path: &Path, delimiter: Option<char>) -> Result<(Vec<String>, Vec<Vec<Cell>>), String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| format!("Ranking file could not be read: {e}"))?;
    // utf-8-sig
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();

    let delim = delimiter.unwrap_or_else(|| {
        let sample = &text.chars().take(8192).collect::<String>();
        let semis = sample.matches(';').count();
        let commas = sample.matches(',').count();
        if semis > commas {
            ';'
        } else {
            ','
        }
    });

    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut record: Vec<Cell> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut saw_any = false;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            c if c == delim => {
                record.push(Cell::Text(field.clone()));
                field.clear();
            }
            '\r' => {}
            '\n' => {
                record.push(Cell::Text(field.clone()));
                field.clear();
                rows.push(std::mem::take(&mut record));
                saw_any = true;
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(Cell::Text(field));
        rows.push(record);
        saw_any = true;
    }
    if !saw_any && rows.is_empty() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(format!("Ranking file {name} is empty."));
    }

    let header_row = rows.remove(0);
    let headers = header_row.iter().map(|c| c.as_raw()).collect();
    Ok((headers, rows))
}

fn read_xlsx(path: &Path) -> Result<(Vec<String>, Vec<Vec<Cell>>), String> {
    use calamine::Reader as _;
    let mut workbook: calamine::Xlsx<_> = calamine::open_workbook(path)
        .map_err(|e| format!("Ranking file could not be opened: {e}"))?;
    let sheet_name = workbook.sheet_names().first().cloned().ok_or_else(|| {
        format!(
            "Ranking file {} is empty.",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        )
    })?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|e| format!("Ranking file could not be read: {e:?}"))?;

    if range.height() == 0 {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(format!("Ranking file {name} is empty."));
    }
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for row in range.rows() {
        let cells: Vec<Cell> = row
            .iter()
            .map(|v| match v {
                calamine::Data::Empty => Cell::Empty,
                calamine::Data::String(s) => Cell::Text(s.clone()),
                calamine::Data::Float(f) => Cell::Num(*f),
                calamine::Data::Int(i) => Cell::Num(*i as f64),
                calamine::Data::Bool(b) => Cell::Text(b.to_string()),
                other => Cell::Text(other.to_string()),
            })
            .collect();
        rows.push(cells);
    }
    if rows.is_empty() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(format!("Ranking file {name} is empty."));
    }
    let header_row = rows.remove(0);
    let headers = header_row
        .iter()
        .map(|c| match c {
            Cell::Empty => String::new(),
            other => other.as_raw(),
        })
        .collect();
    Ok((headers, rows))
}

fn build_records(
    headers: &[String],
    rows: &[Vec<Cell>],
    source_name: &str,
    default_year: Option<i32>,
) -> Result<Vec<JournalRankRecord>, String> {
    let columns = resolve_columns(headers);
    if !columns.contains_key("title") && !columns.contains_key("issn") {
        let found: Vec<String> = headers
            .iter()
            .filter(|h| !h.trim().is_empty())
            .cloned()
            .collect();
        return Err(format!(
            "Ranking file {source_name} has no recognizable journal title or ISSN column. Headers found: {}",
            found.join(", ")
        ));
    }
    if !columns.contains_key("rank") {
        return Err(format!(
            "Ranking file {source_name} has no recognizable rank/quartile column."
        ));
    }

    let mut records: Vec<JournalRankRecord> = Vec::new();
    for row in rows {
        if row.iter().all(|c| c.is_blank()) {
            continue;
        }

        let title = cell(row, columns.get("title")).trim().to_string();
        let issns = normalize_issn_list(
            split_issns(cell(row, columns.get("issn")))
                .iter()
                .map(|s| s.as_str()),
        );
        if title.is_empty() && issns.is_empty() {
            continue;
        }

        let raw_rank = cell(row, columns.get("rank"));
        let (kind, rank) = classify_rank(raw_rank);
        if rank.is_empty() {
            continue;
        }

        let mut year = parse_year(cell(row, columns.get("year")));
        if year.is_none() {
            year = default_year;
        }

        let source_url = cell(row, columns.get("source_url")).trim().to_string();
        records.push(JournalRankRecord {
            title: title.clone(),
            title_key: journal_title_key(&title),
            issns,
            kind,
            rank,
            year,
            category: cell(row, columns.get("category")).trim().to_string(),
            source_url,
        });
    }

    if records.is_empty() {
        return Err(format!(
            "Ranking file {source_name} contained no valid ranking rows (ranks must be Q1-Q4 or S1-S6)."
        ));
    }
    Ok(records)
}

fn cell<'a>(row: &'a [Cell], index: Option<&usize>) -> &'a str {
    match index {
        Some(i) if *i < row.len() => match &row[*i] {
            Cell::Text(t) => t.as_str(),
            Cell::Num(_) => "",
            Cell::Empty => "",
        },
        _ => "",
    }
}

fn split_issns(value: &str) -> Vec<String> {
    ISSN_SPLIT_RE
        .split(value)
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

fn classify_rank(value: &str) -> (String, String) {
    let token = WS_RE.replace_all(value, "").to_uppercase();
    if SJR_RANK_RE.is_match(&token) {
        return ("sjr".to_string(), token);
    }
    if SINTA_RANK_RE.is_match(&token) {
        return ("sinta".to_string(), token);
    }
    (String::new(), String::new())
}

fn parse_year(value: &str) -> Option<i32> {
    YEAR_SEARCH_RE
        .captures(value)
        .and_then(|m| m[1].parse().ok())
}

/// `apply_rank`: fill only that ranking system's fields.
pub fn apply_rank(article: &Article, rank: Option<&JournalRank>) -> Article {
    let Some(rank) = rank else {
        return article.clone();
    };
    if rank.kind == "sjr" {
        Article {
            sjr_quartile: rank.rank.clone(),
            sjr_year: rank.year,
            sjr_category: rank.category.clone(),
            sjr_source_url: rank.source_url.clone(),
            ..article.clone()
        }
    } else {
        Article {
            sinta_rank: rank.rank.clone(),
            sinta_year: rank.year,
            sinta_source_url: rank.source_url.clone(),
            ..article.clone()
        }
    }
}

/// Abstracting/indexing services a journal can be listed in.
///
/// SJR (Scimago) and SINTA both arrive through imported catalog files and are
/// mutually exclusive per file row; a journal can appear in both. DOAJ and
/// SCOPUS are not distributed in those files, so they are reported only when
/// the record itself proves membership (an OA journal with a DOAJ source, or a
/// SCOPUS-branded source URL).
///
/// Index membership describes the JOURNAL, never the individual paper — the UI
/// must not imply otherwise.
pub fn indexing_labels(article: &Article) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();

    let sjr = article.sjr_quartile.trim();
    if !sjr.is_empty() {
        labels.push(if sjr.eq_ignore_ascii_case("SJR") {
            "SJR".to_string()
        } else {
            format!("SJR {sjr}")
        });
    }

    let sinta = article.sinta_rank.trim();
    if !sinta.is_empty() {
        labels.push(if sinta.eq_ignore_ascii_case("SINTA") {
            "SINTA".to_string()
        } else {
            format!("SINTA {sinta}")
        });
    }

    // SCOPUS has no free bulk dataset, so it is inferred only from an explicit
    // source marker; a guessed "SCOPUS" label would be misinformation.
    let hunts = [
        article.sjr_source_url.as_str(),
        article.sinta_source_url.as_str(),
        article.pdf_source_url.as_str(),
    ];
    if hunts
        .iter()
        .any(|u| u.to_ascii_lowercase().contains("scopus"))
    {
        labels.push("SCOPUS".to_string());
    }
    if hunts
        .iter()
        .any(|u| u.to_ascii_lowercase().contains("doaj"))
    {
        labels.push("DOAJ".to_string());
    }

    labels
}

/// One-line index summary for the table cell; empty when nothing is known.
pub fn indexing_label(article: &Article) -> String {
    indexing_labels(article).join(", ")
}

/// `catalog_matches`: (SJR, SINTA) via the most specific winning identity.
pub fn catalog_matches(
    article: &Article,
    catalog: Option<&JournalCatalog>,
) -> (Option<JournalRank>, Option<JournalRank>) {
    let Some(catalog) = catalog else {
        return (None, None);
    };

    for (issns, journal) in journal_identities(article) {
        let ranks = catalog.match_all(issns.iter().map(|s| s.as_str()), &journal);
        if ranks.is_empty() {
            continue;
        }
        let sjr = ranks.iter().find(|r| r.kind == "sjr").cloned();
        let sinta = ranks.iter().find(|r| r.kind == "sinta").cloned();
        if sjr.is_some() || sinta.is_some() {
            return (sjr, sinta);
        }
    }

    (None, None)
}

/// Candidate identities, most specific first: DOI-borne ISSN, then verified
/// journal field + ISSNs. The Scholar citation line is never used.
fn journal_identities(article: &Article) -> Vec<(Vec<String>, String)> {
    let mut identities = vec![(article.issns.clone(), article.journal.clone())];
    if let Some(from_doi) = issn_from_doi(&article.doi) {
        identities.insert(0, (vec![from_doi], String::new()));
    }
    identities
}

fn issn_from_doi(doi: &str) -> Option<String> {
    ISSN_FROM_DOI_RE
        .captures(&doi.to_uppercase())
        .map(|m| normalize_issn(&m[1]))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod indexing_tests {
    use super::*;

    fn with(sjr: &str, sinta: &str) -> Article {
        Article {
            sjr_quartile: sjr.to_string(),
            sinta_rank: sinta.to_string(),
            ..Article::default()
        }
    }

    #[test]
    fn labels_sjr_and_sinta_together() {
        let a = with("Q1", "S2");
        assert_eq!(indexing_label(&a), "SJR Q1, SINTA S2");
        assert_eq!(indexing_labels(&a).len(), 2);
    }

    #[test]
    fn labels_each_individually() {
        assert_eq!(indexing_label(&with("Q3", "")), "SJR Q3");
        assert_eq!(indexing_label(&with("", "S1")), "SINTA S1");
    }

    #[test]
    fn unknown_journal_has_no_labels() {
        // An unmatched journal stays empty rather than claiming an index.
        assert_eq!(indexing_label(&with("", "")), "");
        assert!(indexing_labels(&with("", "")).is_empty());
    }

    #[test]
    fn source_urls_add_scopus_and_doaj() {
        let a = Article {
            sjr_quartile: "Q2".into(),
            sjr_source_url: "https://www.scopus.com/sourceid/123".into(),
            ..Article::default()
        };
        assert_eq!(indexing_label(&a), "SJR Q2, SCOPUS");

        let b = Article {
            sinta_rank: "S3".into(),
            sinta_source_url: "https://doaj.org/toc/1234-5678".into(),
            ..Article::default()
        };
        assert_eq!(indexing_label(&b), "SINTA S3, DOAJ");
    }

    #[test]
    fn bare_kind_markers_do_not_duplicate() {
        let a = with("SJR", "SINTA");
        assert_eq!(indexing_label(&a), "SJR, SINTA");
    }

    #[test]
    fn whitespace_only_rank_is_unknown() {
        assert_eq!(indexing_label(&with("   ", "  ")), "");
    }
}
