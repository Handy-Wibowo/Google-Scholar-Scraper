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

pub const TITLE_SELECT: &str = "DOI,title,container-title,short-container-title,ISSN,type,author,issued,published-print,published-online,link,license,volume,issue,page,publisher,URL";

/// Normalized bibliographic facts for one work (`CrossrefRecord`).
#[derive(Debug, Clone, Default, PartialEq)]
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
}

impl<'a> CrossrefClient<'a> {
    pub fn new(fetcher: &'a mut dyn CrossrefFetcher) -> Self {
        Self {
            fetcher,
            base_url: CROSSREF_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    fn get_json(&mut self, url: &str) -> Option<Value> {
        let headers = vec![
            ("User-Agent".to_string(), self.user_agent.clone()),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        let (body, status) = self.fetcher.get(url, &headers).ok()?;
        if status != 200 {
            return None;
        }
        if body.len() > self.max_response_bytes {
            return None;
        }
        let text = String::from_utf8(body).ok()?;
        let payload: Value = serde_json::from_str(&text).ok()?;
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
    }
}

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
