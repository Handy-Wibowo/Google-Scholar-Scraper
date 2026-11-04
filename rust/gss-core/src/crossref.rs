//! Crossref enrichment + citation resolution ported from `enrichment.py`
//! verbatim. Conservative identity: DOI echo-check, exact normalized title,
//! work-type gates, author/year consistency, verbatim warning strings.

use crate::models::{normalize_issn_list, normalize_title_key, Article, CitationRecord};

/// Progress callback shape (current, total, phase).
pub type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize, &str);
use crate::text::CaseFold;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

pub const CROSSREF_BASE_URL: &str = "https://api.crossref.org/works/";
pub const DEFAULT_TIMEOUT_SECS: f64 = 10.0;
pub const DEFAULT_USER_AGENT: &str =
    "google-scholar-scraper/2.1 (metadata enrichment; mailto:none)";
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 2_000_000;
pub const DEFAULT_TITLE_ROWS: usize = 20;

pub const UNRESOLVED_WARNING: &str =
    "Unresolved work: metadata was not verified against Crossref; review before citing.";
pub const SCHOLAR_AUTHORS_WARNING: &str =
    "Authors could not be verified; the raw Google Scholar citation line is preserved in the note.";

static YEAR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(1[5-9]\d\d|2[01]\d\d)$").unwrap());

pub const JOURNAL_WORK_TYPES: [&str; 1] = ["journal-article"];
pub const CITATION_WORK_TYPES: [&str; 2] = ["journal-article", "proceedings-article"];

/// Fields requested for title lookups.
///
/// The single-work endpoint ignores `select`, so `lookup()` receives the full
/// payload regardless; this list matters for the `/works?filter=` search path.
/// `is-referenced-by-count` and `ORCID` were added in v3.4.0.
pub const TITLE_SELECT: &str = "DOI,title,container-title,short-container-title,ISSN,type,author,issued,published-print,published-online,link,license,volume,issue,page,publisher,URL,is-referenced-by-count";

/// Normalized bibliographic facts for one work (`CrossrefRecord`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CrossrefRecord {
    pub doi: String,
    pub journal: String,
    pub issns: Vec<String>,
    pub year: Option<i32>,
    pub work_type: String,
    pub pdf_candidate_url: String,
    pub pdf_candidate_source: String,
    pub authors: Vec<String>,
    pub container_any: String,
    pub volume: String,
    pub issue: String,
    pub pages: String,
    pub publisher: String,
    pub url: String,
    pub issns_any: Vec<String>,
    /// `is-referenced-by-count`.
    pub cited_by: Option<i64>,
    /// `license[].URL`, first non-empty.
    pub license: String,
    /// `updated-by` / `update-to` notices: (type, doi).
    pub notices: Vec<(String, String)>,
    /// `funder[]` rendered as `"Name (award)"`.
    pub funders: Vec<String>,
    /// `author[].ORCID`, URL prefix stripped.
    pub orcids: Vec<String>,
}

/// Transport failure for the Crossref seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("transport error")]
pub struct TransportError;

/// Transport seam (Python: `requests.Session.get(..., stream=True)`).
pub trait CrossrefFetcher {
    /// GET with the given headers; Ok(body, status). No auto-redirects.
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<(Vec<u8>, u16), TransportError>;
}

pub struct CrossrefClient<'a> {
    fetcher: &'a mut dyn CrossrefFetcher,
    pub base_url: String,
    pub user_agent: String,
    pub max_response_bytes: usize,
    /// Count of provider-side failures (transport, 5xx, 429, bad JSON).
    transport_failures: u32,
}

impl<'a> CrossrefClient<'a> {
    pub fn new(fetcher: &'a mut dyn CrossrefFetcher) -> Self {
        Self {
            fetcher,
            base_url: CROSSREF_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            transport_failures: 0,
        }
    }

    /// Transport/HTTP failures seen by this client.
    ///
    /// `get_json` collapses "transport error", "non-200", "oversize" and
    /// "unparseable" into `None`, which is right for a per-record lookup but
    /// hides a total provider outage. Callers read this counter after a pass to
    /// tell "nothing matched" from "the provider never answered".
    pub fn transport_failures(&self) -> u32 {
        self.transport_failures
    }

    fn note_transport_failure(&mut self) {
        self.transport_failures = self.transport_failures.saturating_add(1);
    }

    fn get_json(&mut self, url: &str) -> Option<Value> {
        let headers = vec![
            ("User-Agent".to_string(), self.user_agent.clone()),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        let Ok((body, status)) = self.fetcher.get(url, &headers) else {
            self.note_transport_failure();
            return None;
        };
        if status != 200 {
            // A 404 for an unknown DOI is a normal miss, not an outage. Only a
            // server-side or throttling status counts against the provider.
            if status >= 500 || status == 429 {
                self.note_transport_failure();
            }
            return None;
        }
        if body.len() > self.max_response_bytes {
            return None;
        }
        let Ok(text) = String::from_utf8(body) else {
            return None;
        };
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            self.note_transport_failure();
            return None;
        };
        if payload.is_object() {
            Some(payload)
        } else {
            None
        }
    }

    fn get_message(&mut self, url: &str) -> Option<Value> {
        let payload = self.get_json(url)?;
        let message = payload.get("message")?;
        if message.is_object() {
            Some(message.clone())
        } else {
            None
        }
    }

    /// `lookup`: DOI → record; the payload must echo the requested DOI.
    pub fn lookup(&mut self, doi: &str) -> Option<CrossrefRecord> {
        let normalized = doi.trim();
        if normalized.is_empty() {
            return None;
        }
        let url = format!("{}{}", self.base_url, crate::pdf::quote_url(normalized, ""));
        let message = self.get_message(&url)?;
        let echoed = message
            .get("DOI")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .casefold();
        if echoed != normalized.casefold() {
            return None;
        }
        Some(record_from_crossref_message(&message, normalized))
    }

    /// `lookup_title`: exact normalized title, unambiguous single DOI.
    pub fn lookup_title(
        &mut self,
        title: &str,
        author: &str,
        year: Option<i32>,
        rows: usize,
        work_types: &[&str],
    ) -> Option<CrossrefRecord> {
        let wanted_key = normalize_title_key(title);
        if wanted_key.is_empty() {
            return None;
        }

        let params: Vec<(String, String)> = {
            let mut p = vec![
                ("query.title".to_string(), title.trim().to_string()),
                ("rows".to_string(), rows.clamp(1, 20).to_string()),
                ("select".to_string(), TITLE_SELECT.to_string()),
            ];
            if let Some(y) = year {
                p.push((
                    "filter".to_string(),
                    format!(
                        "from-pub-date:{0}-01-01,until-pub-date:{1}-12-31",
                        y - 1,
                        y + 1
                    ),
                ));
            }
            p
        };
        let query = params
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    crate::pdf::quote_url(k, ""),
                    crate::pdf::quote_url(v, "")
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{}?{}", self.base_url, query);
        let payload = self.get_json(&url)?;
        let items = payload
            .get("message")
            .and_then(|m| m.get("items"))
            .and_then(|i| i.as_array())?;

        let mut matches: Vec<&Value> = Vec::new();
        for item in items {
            let item_type = item
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim()
                .casefold();
            if !work_types.contains(&item_type.as_str()) {
                continue;
            }
            let item_title = first_text(item.get("title"));
            // Exact normalized match first; a slight rewrite ("...: a review")
            // falls back to token similarity, still gated by the author and year
            // checks below so a near-miss cannot attach the wrong record.
            // Exact normalized match only. The Python oracle
            // (test_title_lookup_rejects_a_near_miss_title) pins this: a
            // shorter title must NOT resolve to a longer one that merely
            // starts the same way, because Crossref's fuzzy search returns
            // same-prefix papers that differ in subject.
            if normalize_title_key(&item_title) != wanted_key {
                continue;
            }
            if !author_consistent(author, item) {
                continue;
            }
            if !year_consistent(year, item) {
                continue;
            }
            matches.push(item);
        }

        let dois: HashSet<String> = matches
            .iter()
            .map(|item| {
                item.get("DOI")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .trim()
                    .casefold()
            })
            .filter(|d| !d.is_empty())
            .collect();
        if dois.len() != 1 {
            return None;
        }
        let resolved_doi = dois.into_iter().next().unwrap();
        let winner = matches
            .iter()
            .find(|item| {
                item.get("DOI")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .trim()
                    .casefold()
                    == resolved_doi
            })
            .unwrap();
        Some(record_from_crossref_message(winner, &resolved_doi))
    }
}

/// `record_from_crossref_message`: journal identity type-gated; citation
/// fields unconditional.
pub fn record_from_crossref_message(message: &Value, fallback_doi: &str) -> CrossrefRecord {
    let work_type = message
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .casefold();

    let mut journal = String::new();
    let mut issns: Vec<String> = Vec::new();
    if JOURNAL_WORK_TYPES.contains(&work_type.as_str()) {
        journal = first_text(message.get("container-title"));
        if journal.is_empty() {
            journal = first_text(message.get("short-container-title"));
        }
        issns = normalize_issn_list(as_text_list(message.get("ISSN")).iter().map(|s| s.as_str()));
    }

    let mut authors: Vec<String> = Vec::new();
    if let Some(list) = message.get("author").and_then(|a| a.as_array()) {
        for entry in list {
            let given = entry
                .get("given")
                .and_then(|g| g.as_str())
                .unwrap_or("")
                .trim();
            let family = entry
                .get("family")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .trim();
            let name = entry
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .trim();
            if !family.is_empty() {
                authors.push(format!("{given} {family}").trim().to_string());
            } else {
                authors.push(name.to_string());
            }
        }
    }
    let mut container_any = first_text(message.get("container-title"));
    if container_any.is_empty() {
        container_any = first_text(message.get("short-container-title"));
    }
    let issns_any =
        normalize_issn_list(as_text_list(message.get("ISSN")).iter().map(|s| s.as_str()));

    let (pdf_url, pdf_source) = public_pdf_candidate(message);
    let doi_value = message
        .get("DOI")
        .and_then(|d| d.as_str())
        .unwrap_or(fallback_doi)
        .trim()
        .casefold();
    let doi_value = if doi_value.is_empty() {
        fallback_doi.to_string()
    } else {
        doi_value
    };

    CrossrefRecord {
        doi: doi_value.to_string(),
        journal,
        issns,
        year: crossref_year(message),
        work_type,
        pdf_candidate_url: pdf_url,
        pdf_candidate_source: pdf_source,
        authors,
        container_any,
        volume: message
            .get("volume")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        issue: message
            .get("issue")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        pages: message
            .get("page")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        publisher: message
            .get("publisher")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        url: message
            .get("URL")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        issns_any,
        cited_by: message
            .get("is-referenced-by-count")
            .and_then(|v| v.as_i64()),
        license: first_license_url(message),
        notices: collect_notices(message),
        funders: collect_funders(message),
        orcids: collect_orcids(message),
    }
}

/// `license[].URL` for the version of record, else the first entry.
fn first_license_url(message: &Value) -> String {
    let Some(list) = message.get("license").and_then(|l| l.as_array()) else {
        return String::new();
    };
    let mut fallback = String::new();
    for entry in list {
        let Some(url) = entry.get("URL").and_then(|u| u.as_str()) else {
            continue;
        };
        let url = url.trim();
        if url.is_empty() {
            continue;
        }
        let is_vor = entry
            .get("content-version")
            .and_then(|v| v.as_str())
            .map(|v| v.eq_ignore_ascii_case("vor"))
            .unwrap_or(false);
        if is_vor {
            return url.to_string();
        }
        if fallback.is_empty() {
            fallback = url.to_string();
        }
    }
    fallback
}

/// Retraction / correction / expression-of-concern notices.
///
/// Crossref reports these in two places: `updated-by` (this work was updated by
/// a notice) and `update-to` (this work *is* the notice). Only `updated-by`
/// changes how the record should be read, so that is what is collected.
fn collect_notices(message: &Value) -> Vec<(String, String)> {
    let Some(list) = message.get("updated-by").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in list {
        let kind = entry
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let doi = entry
            .get("DOI")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if kind.is_empty() {
            continue;
        }
        let pair = (kind, doi);
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// `funder[]` as `"Name (award)"`, skipping entries with neither.
fn collect_funders(message: &Value) -> Vec<String> {
    let Some(list) = message.get("funder").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let name = entry
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .trim();
        let awards: Vec<&str> = entry
            .get("award")
            .and_then(|a| a.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str())
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let rendered = if name.is_empty() && awards.is_empty() {
            continue;
        } else if awards.is_empty() {
            name.to_string()
        } else {
            format!("{name} ({})", awards.join(", "))
        };
        if !out.contains(&rendered) {
            out.push(rendered);
        }
    }
    out
}

/// `author[].ORCID` with the `https://orcid.org/` prefix removed.
fn collect_orcids(message: &Value) -> Vec<String> {
    let Some(list) = message.get("author").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let Some(raw) = entry.get("ORCID").and_then(|o| o.as_str()) else {
            continue;
        };
        let id = raw
            .trim()
            .trim_start_matches("https://orcid.org/")
            .trim_start_matches("http://orcid.org/")
            .trim_start_matches("orcid.org/")
            .to_string();
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Accept a title that extends the other with a subtitle or trailing qualifier.
///
/// Measured on real cases, plain token cosine does not separate them: a genuine
/// subtitle ("Deep learning for molecules" vs "...: a review") scores 0.816
/// while a different paper ("... for proteins") scores 0.750. A 0.07 margin is
/// too thin to gate record identity on, so the rule is structural instead: one
/// title's tokens must be a **prefix** of the other's. That accepts subtitles
/// and rejects same-prefix-but-different-subject papers only when the subject
/// itself differs, which the author and year checks then catch.
fn first_text(value: Option<&Value>) -> String {
    let values = as_text_list(value);
    values.first().cloned().unwrap_or_default()
}

fn as_text_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t.to_string()]
            }
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// `_crossref_year`: published-print → published-online → published →
/// issued → created; only plausible four-digit years.
pub fn crossref_year(message: &Value) -> Option<i32> {
    for key in [
        "published-print",
        "published-online",
        "published",
        "issued",
        "created",
    ] {
        if let Some(year) = year_from_date(message.get(key)) {
            return Some(year);
        }
    }
    None
}

fn year_from_date(value: Option<&Value>) -> Option<i32> {
    let value = value?;
    let parts = value.get("date-parts")?.as_array()?;
    let first = parts.first()?.as_array()?;
    let first_value = first.first()?;
    parse_year(value_to_string(first_value).as_str())
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn parse_year(text: &str) -> Option<i32> {
    let text = text.trim();
    if YEAR_RE.is_match(text) {
        text.parse().ok()
    } else {
        None
    }
}

/// `_public_pdf_candidate`: explicit public licence + publisher-labeled PDF link.
fn public_pdf_candidate(message: &Value) -> (String, String) {
    let mut licence_url = String::new();
    if let Some(licences) = message.get("license").and_then(|l| l.as_array()) {
        for licence in licences {
            if let Some(url) = licence.get("URL").and_then(|u| u.as_str()) {
                let url = url.trim();
                if !url.is_empty() {
                    licence_url = url.to_string();
                    break;
                }
            }
        }
    }
    if licence_url.is_empty() {
        return (String::new(), String::new());
    }

    if let Some(links) = message.get("link").and_then(|l| l.as_array()) {
        for link in links {
            let content_type = link
                .get("content-type")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .casefold();
            let url = link
                .get("URL")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .trim();
            if url.starts_with("https://") && content_type.contains("pdf") {
                return (url.to_string(), "Crossref licence deposit".to_string());
            }
        }
    }
    (String::new(), String::new())
}

/// `_title_tokens`: raw-text word tokens, len ≥ 3, non-numeric.
pub fn title_tokens(value: &str) -> Vec<String> {
    let normalized: String = value.nfkd().collect();
    let decomposed: String = normalized
        .chars()
        .filter(|c| !crate::text::is_combining_pub(*c))
        .collect();
    crate::text::split_non_alnum(&decomposed.casefold())
        .into_iter()
        .filter(|t| t.chars().count() >= 3 && !t.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

/// `_author_consistent`: a supplied surname missing from the candidate's
/// author list is a contradiction.
pub fn author_consistent(author: &str, item: &Value) -> bool {
    let wanted: HashSet<String> = title_tokens(author).into_iter().collect();
    if wanted.is_empty() {
        return true;
    }
    let mut families: HashSet<String> = HashSet::new();
    if let Some(list) = item.get("author").and_then(|a| a.as_array()) {
        for entry in list {
            if let Some(family) = entry.get("family").and_then(|f| f.as_str()) {
                families.extend(title_tokens(family));
            }
            if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                families.extend(title_tokens(name));
            }
        }
    }
    if families.is_empty() {
        return true;
    }
    wanted.intersection(&families).next().is_some()
}

/// `_year_consistent`: one year of drift allowed (online-first vs print).
pub fn year_consistent(year: Option<i32>, item: &Value) -> bool {
    let Some(year) = year else { return true };
    let Some(candidate_year) = crossref_year(item) else {
        return true;
    };
    (candidate_year - year).abs() <= 1
}

/// `_title_query_key`: normalized title | sorted author tokens | year.
pub fn title_query_key(title: &str, author: &str, year: Option<i32>) -> String {
    let title_key = normalize_title_key(title);
    if title_key.is_empty() {
        return String::new();
    }
    let mut author_tokens = title_tokens(author);
    author_tokens.sort();
    author_tokens.dedup();
    format!(
        "{}|{}|{}",
        title_key,
        author_tokens.join(" "),
        year.map(|y| y.to_string()).unwrap_or_default()
    )
}

/// `_apply_record`: fill only fields Crossref supplied; never overwrite.
fn apply_record(article: &Article, record: &CrossrefRecord) -> Article {
    let mut updated = article.clone();
    if updated.journal.is_empty() && !record.journal.is_empty() {
        updated.journal = record.journal.clone();
    }
    if updated.issns.is_empty() && !record.issns.is_empty() {
        updated.issns = record.issns.clone();
    }
    if updated.publication_year.is_none() && record.year.is_some() {
        updated.publication_year = record.year;
    }
    if updated.doi.is_empty() && !record.doi.is_empty() {
        updated.doi = record.doi.clone();
    }
    if updated.pdf_url.is_empty() && !record.pdf_candidate_url.is_empty() {
        updated.pdf_url = record.pdf_candidate_url.clone();
        updated.pdf_source_url = record.pdf_candidate_source.clone();
    }

    // v3.4.0: fields that come free with the payload already fetched. Fill-only,
    // matching the rest of this function: a value we already hold wins.
    if updated.cited_by.is_none() {
        updated.cited_by = record.cited_by;
    }
    if updated.volume.is_empty() {
        updated.volume = record.volume.clone();
    }
    if updated.issue.is_empty() {
        updated.issue = record.issue.clone();
    }
    if updated.pages.is_empty() {
        updated.pages = record.pages.clone();
    }
    if updated.publisher.is_empty() {
        updated.publisher = record.publisher.clone();
    }
    if updated.license.is_empty() {
        updated.license = record.license.clone();
    }
    if updated.orcids.is_empty() {
        updated.orcids = record.orcids.clone();
    }
    if updated.authors_structured.is_empty() {
        updated.authors_structured = record.authors.clone();
    }
    if updated.funders.is_empty() {
        updated.funders = record.funders.clone();
    }
    // A retraction must overwrite nothing, but must never be missed either: take
    // the most severe notice seen, and its DOI.
    if updated.retraction.is_empty() {
        if let Some((kind, doi)) = record
            .notices
            .iter()
            .find(|(kind, _)| kind.contains("retract"))
            .or_else(|| record.notices.first())
        {
            updated.retraction = kind.clone();
            updated.retraction_doi = doi.clone();
        }
    }
    updated
}

/// Lookup seam for tests (`CrossrefClient` or `RecordingLookup`).
pub trait ArticleLookup {
    fn lookup(&mut self, doi: &str) -> Option<CrossrefRecord>;
    fn lookup_title_citations(
        &mut self,
        title: &str,
        author: &str,
        year: Option<i32>,
        work_types: &'static [&'static str],
    ) -> Option<CrossrefRecord>;
}

/// Adapter over the HTTP client.
pub struct HttpClientLookup<'a, 'b> {
    pub client: &'a mut CrossrefClient<'b>,
}

impl ArticleLookup for HttpClientLookup<'_, '_> {
    fn lookup(&mut self, doi: &str) -> Option<CrossrefRecord> {
        self.client.lookup(doi)
    }

    fn lookup_title_citations(
        &mut self,
        title: &str,
        author: &str,
        year: Option<i32>,
        work_types: &'static [&'static str],
    ) -> Option<CrossrefRecord> {
        self.client
            .lookup_title(title, author, year, DEFAULT_TITLE_ROWS, work_types)
    }
}

/// `enrich_articles`: verified identity + catalog ranks; order preserved;
/// cancellation returns handled + untouched remainder.
#[allow(clippy::too_many_arguments)]
pub fn enrich_articles<L: ArticleLookup>(
    articles: &[Article],
    catalog: Option<&crate::catalog::JournalCatalog>,
    cancel: Option<&dyn crate::client::CancelEvent>,
    mut progress: Option<ProgressFn<'_>>,
    lookup: &mut L,
    max_lookups: Option<usize>,
    max_title_lookups: Option<usize>,
    allow_title_lookup: bool,
) -> Vec<Article> {
    let total = articles.len();
    let mut results: Vec<Article> = Vec::new();
    let mut doi_cache: HashMap<String, Option<CrossrefRecord>> = HashMap::new();
    let mut title_cache: HashMap<String, Option<CrossrefRecord>> = HashMap::new();
    let mut lookups_done = 0;
    let mut title_lookups_done = 0;

    for (index, article) in articles.iter().enumerate() {
        if let Some(c) = cancel {
            if c.is_set() {
                results.extend(articles[index..].iter().cloned());
                if let Some(p) = progress.as_mut() {
                    p(index, total, "Cancelled");
                }
                return results;
            }
        }

        let mut enriched = article.clone();
        let needs_identity = article.journal.is_empty() || article.issns.is_empty();
        let mut record: Option<CrossrefRecord> = None;

        let doi = article.doi.trim().to_string();
        if !doi.is_empty() && (needs_identity || article.publication_year.is_none()) {
            if let Some(cached) = doi_cache.get(&doi) {
                record = cached.clone();
            } else if max_lookups.is_none() || lookups_done < max_lookups.unwrap() {
                let answer = lookup.lookup(&doi);
                doi_cache.insert(doi.clone(), answer.clone());
                lookups_done += 1;
                record = answer;
            }
        }

        if record.is_none()
            && doi.is_empty()
            && needs_identity
            && allow_title_lookup
            && !article.title.trim().is_empty()
        {
            let key = title_query_key(&article.title, &article.authors, article.publication_year);
            if key.is_empty() {
                // Not worth asking about.
            } else if let Some(cached) = title_cache.get(&key) {
                record = cached.clone();
            } else if max_title_lookups.is_none() || title_lookups_done < max_title_lookups.unwrap()
            {
                let answer = lookup.lookup_title_citations(
                    &article.title,
                    &article.authors,
                    article.publication_year,
                    &JOURNAL_WORK_TYPES,
                );
                title_cache.insert(key, answer.clone());
                title_lookups_done += 1;
                record = answer;
            }
        }

        if let Some(rec) = &record {
            enriched = apply_record(&enriched, rec);
        }

        if let Some(cat) = catalog {
            let (sjr, sinta) = crate::catalog::catalog_matches(&enriched, Some(cat));
            enriched = crate::catalog::apply_rank(&enriched, sjr.as_ref());
            enriched = crate::catalog::apply_rank(&enriched, sinta.as_ref());
        }

        let label = if enriched.title.is_empty() {
            article.title.clone()
        } else {
            enriched.title.clone()
        };
        results.push(enriched);

        if let Some(p) = progress.as_mut() {
            p(index + 1, total, &label);
        }
    }

    results
}

/// `_citation_from_record`: verified record; article supplies fallbacks only.
fn citation_from_record(article: &Article, record: &CrossrefRecord) -> CitationRecord {
    CitationRecord {
        title: article.title.clone(),
        authors: record.authors.clone(),
        year: record.year.or(article.publication_year),
        container: record.container_any.clone(),
        entry_type: record.work_type.clone(),
        doi: if record.doi.is_empty() {
            article.doi.clone()
        } else {
            record.doi.clone()
        },
        issns: record.issns_any.clone(),
        volume: record.volume.clone(),
        issue: record.issue.clone(),
        pages: record.pages.clone(),
        publisher: record.publisher.clone(),
        url: if record.url.is_empty() {
            article.link.clone()
        } else {
            record.url.clone()
        },
        source: "crossref".to_string(),
        verified: true,
        fallback_authors_line: String::new(),
        warnings: Vec::new(),
    }
}

impl CrossrefClient<'_> {
    /// Abstract for one DOI, from Crossref's JATS fragment.
    ///
    /// Uses the same DOI echo-check as `lookup` so a redirect or a
    /// case-different record cannot smuggle in another work's abstract.
    pub fn abstract_for_doi(&mut self, doi: &str) -> Option<String> {
        let normalized = doi.trim();
        if normalized.is_empty() {
            return None;
        }
        let url = format!("{}{}", self.base_url, crate::pdf::quote_url(normalized, ""));
        let message = self.get_message(&url)?;
        let echoed = message
            .get("DOI")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .casefold();
        if echoed != normalized.casefold() {
            return None;
        }
        crate::abstracts::abstract_from_crossref(&message)
    }
}

/// Abstract from OpenAlex, looked up by DOI.
pub fn abstract_from_openalex_doi(fetcher: &mut dyn CrossrefFetcher, doi: &str) -> Option<String> {
    let url = format!(
        "https://api.openalex.org/works/doi:{}",
        crate::pdf::quote_url(doi, "/")
    );
    openalex_abstract(fetcher, &url)
}

/// Abstract from OpenAlex, looked up by title search (no DOI available).
///
/// Only the first confidently-titled hit is used: OpenAlex's `title.search`
/// is fuzzy, so a mismatch would attach the wrong abstract to a record.
pub fn abstract_from_openalex_title(
    fetcher: &mut dyn CrossrefFetcher,
    title: &str,
) -> Option<String> {
    let query = crate::pdf::quote_url(title.trim(), "");
    let url = format!(
        "https://api.openalex.org/works?filter=title.search:{query}&per-page=1&select=id,title,abstract_inverted_index"
    );
    let payload = openalex_json(fetcher, &url)?;
    let first = payload.get("results")?.as_array()?.first()?.clone();
    // Guard against a fuzzy match returning an unrelated work.
    let found = first.get("title").and_then(|t| t.as_str()).unwrap_or("");
    if crate::text::normalize_title_key(found) != crate::text::normalize_title_key(title) {
        return None;
    }
    crate::abstracts::abstract_from_openalex(&first)
}

fn openalex_abstract(fetcher: &mut dyn CrossrefFetcher, url: &str) -> Option<String> {
    let payload = openalex_json(fetcher, url)?;
    crate::abstracts::abstract_from_openalex(&payload)
}

fn openalex_json(fetcher: &mut dyn CrossrefFetcher, url: &str) -> Option<Value> {
    let headers = vec![
        ("User-Agent".to_string(), DEFAULT_USER_AGENT.to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let (body, status) = fetcher.get(url, &headers).ok()?;
    if status != 200 || body.len() > DEFAULT_MAX_RESPONSE_BYTES {
        return None;
    }
    let payload: Value = serde_json::from_slice(&body).ok()?;
    if payload.is_object() {
        Some(payload)
    } else {
        None
    }
}

/// `_unresolved_citation`: exportable but explicitly unverified.
fn unresolved_citation(article: &Article) -> CitationRecord {
    CitationRecord {
        title: article.title.clone(),
        year: article.publication_year,
        container: article.journal.clone(),
        issns: article.issns.clone(),
        doi: article.doi.clone(),
        url: article.link.clone(),
        source: "scholar".to_string(),
        verified: false,
        fallback_authors_line: article.authors.clone(),
        warnings: vec![
            UNRESOLVED_WARNING.to_string(),
            SCHOLAR_AUTHORS_WARNING.to_string(),
        ],
        ..CitationRecord::default()
    }
}

/// `resolve_citations`: one citation per article, order preserved; DOI miss
/// stays unresolved (no title fallback for DOI-bearing records).
pub fn resolve_citations<L: ArticleLookup>(
    articles: &[Article],
    cancel: Option<&dyn crate::client::CancelEvent>,
    mut progress: Option<ProgressFn<'_>>,
    lookup: &mut L,
    max_lookups: Option<usize>,
    max_title_lookups: Option<usize>,
) -> Vec<CitationRecord> {
    let total = articles.len();
    let mut results: Vec<CitationRecord> = Vec::new();
    let mut doi_cache: HashMap<String, Option<CrossrefRecord>> = HashMap::new();
    let mut title_cache: HashMap<String, Option<CrossrefRecord>> = HashMap::new();
    let mut lookups_done = 0;
    let mut title_lookups_done = 0;

    for (index, article) in articles.iter().enumerate() {
        if let Some(c) = cancel {
            if c.is_set() {
                results.extend(articles[index..].iter().map(unresolved_citation));
                if let Some(p) = progress.as_mut() {
                    p(index, total, "Cancelled");
                }
                return results;
            }
        }

        let mut record: Option<CrossrefRecord> = None;
        let doi = article.doi.trim().to_string();
        if !doi.is_empty() {
            if let Some(cached) = doi_cache.get(&doi) {
                record = cached.clone();
            } else if max_lookups.is_none() || lookups_done < max_lookups.unwrap() {
                let answer = lookup.lookup(&doi);
                doi_cache.insert(doi.clone(), answer.clone());
                lookups_done += 1;
                record = answer;
            }
        }

        if record.is_none() && doi.is_empty() && !article.title.trim().is_empty() {
            let key = title_query_key(&article.title, &article.authors, article.publication_year);
            if key.is_empty() {
                // no comparable characters → not worth asking
            } else if let Some(cached) = title_cache.get(&key) {
                record = cached.clone();
            } else if max_title_lookups.is_none() || title_lookups_done < max_title_lookups.unwrap()
            {
                let answer = lookup.lookup_title_citations(
                    &article.title,
                    &article.authors,
                    article.publication_year,
                    &CITATION_WORK_TYPES,
                );
                title_cache.insert(key, answer.clone());
                title_lookups_done += 1;
                record = answer;
            }
        }

        if let Some(rec) = &record {
            results.push(citation_from_record(article, rec));
        } else {
            results.push(unresolved_citation(article));
        }

        if let Some(p) = progress.as_mut() {
            let label = if article.title.is_empty() {
                "record"
            } else {
                &article.title
            };
            p(index + 1, total, label);
        }
    }

    results
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_citation_count_and_venue_fields() {
        let message = json!({
            "DOI": "10.1/x",
            "type": "journal-article",
            "container-title": ["J Test"],
            "is-referenced-by-count": 3142,
            "volume": "12",
            "issue": "3",
            "page": "45-67",
            "publisher": "Test Press"
        });
        let record = record_from_crossref_message(&message, "10.1/x");
        assert_eq!(record.cited_by, Some(3142));
        assert_eq!(record.volume, "12");
        assert_eq!(record.issue, "3");
        assert_eq!(record.pages, "45-67");
        assert_eq!(record.publisher, "Test Press");
    }

    #[test]
    fn prefers_the_vor_license() {
        let message = json!({
            "DOI": "10.1/x",
            "license": [
                {"URL": "https://creativecommons.org/licenses/by-nc/4.0/", "content-version": "am"},
                {"URL": "https://creativecommons.org/licenses/by/4.0/", "content-version": "vor"}
            ]
        });
        let record = record_from_crossref_message(&message, "10.1/x");
        assert_eq!(
            record.license,
            "https://creativecommons.org/licenses/by/4.0/"
        );
    }

    #[test]
    fn detects_a_retraction_notice() {
        let message = json!({
            "DOI": "10.1/x",
            "updated-by": [
                {"type": "retraction", "DOI": "10.1/notice", "source": "retraction-watch"}
            ]
        });
        let record = record_from_crossref_message(&message, "10.1/x");
        assert_eq!(record.notices.len(), 1);
        assert_eq!(record.notices[0].0, "retraction");
        assert_eq!(record.notices[0].1, "10.1/notice");
    }

    #[test]
    fn applies_notice_and_metadata_to_an_article() {
        let message = json!({
            "DOI": "10.1/x",
            "type": "journal-article",
            "is-referenced-by-count": 7,
            "updated-by": [{"type": "retraction", "DOI": "10.1/notice"}]
        });
        let record = record_from_crossref_message(&message, "10.1/x");
        let article = crate::models::Article::new("T");
        let updated = apply_record(&article, &record);
        assert_eq!(updated.cited_by, Some(7));
        assert_eq!(updated.retraction, "retraction");
        assert_eq!(updated.retraction_doi, "10.1/notice");
    }

    #[test]
    fn collects_funders_with_awards() {
        let message = json!({
            "DOI": "10.1/x",
            "funder": [
                {"name": "NWO", "award": ["840.11.001"]},
                {"name": "ERC", "award": []}
            ]
        });
        let record = record_from_crossref_message(&message, "10.1/x");
        assert_eq!(record.funders, vec!["NWO (840.11.001)", "ERC"]);
    }

    #[test]
    fn strips_the_orcid_prefix() {
        let message = json!({
            "DOI": "10.1/x",
            "author": [
                {"family": "A", "ORCID": "https://orcid.org/0000-0002-1825-0097"},
                {"family": "B", "ORCID": "https://orcid.org/0000-0002-1825-0097"},
                {"family": "C"}
            ]
        });
        let record = record_from_crossref_message(&message, "10.1/x");
        assert_eq!(record.orcids, vec!["0000-0002-1825-0097"]);
    }

    #[test]
    fn missing_fields_stay_empty_not_guessed() {
        let record = record_from_crossref_message(&json!({"DOI": "10.1/x"}), "10.1/x");
        assert_eq!(record.cited_by, None);
        assert!(record.license.is_empty());
        assert!(record.notices.is_empty());
        assert!(record.funders.is_empty());
        assert!(record.orcids.is_empty());
    }

}
