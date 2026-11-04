//! Data model + normalization contracts ported from `models.py` (v2.2.2) verbatim.

use serde::{Deserialize, Serialize};

/// Typed export cell: string | float | int. Converters in the exporters map
/// these to Excel/CSV values exactly as the Python model does
/// ("" for None, numbers as numbers).
#[derive(Debug, Clone, PartialEq)]
pub enum ExportCell {
    Str(String),
    Double(f64),
    Int(i64),
}

pub const PDF_STATUS_AVAILABLE: &str = "available";
pub const PDF_STATUS_UNAVAILABLE: &str = "unavailable";
pub const PDF_STATUS_UNKNOWN: &str = "unknown";

pub const PDF_STATUSES: [&str; 3] = [
    PDF_STATUS_AVAILABLE,
    PDF_STATUS_UNAVAILABLE,
    PDF_STATUS_UNKNOWN,
];

/// The 20-column export header, in exact `to_export_row()` order.
pub const EXPORT_COLUMNS: [&str; 20] = [
    "Title",
    "Authors",
    "Link",
    "Relevance Score",
    "DOI",
    "Journal",
    "ISSNs",
    "Publication Year",
    "SJR Quartile",
    "SJR Year",
    "SJR Category",
    "SJR Source",
    "SINTA Rank",
    "SINTA Year",
    "SINTA Source",
    "PDF Status",
    "PDF URL",
    "PDF Source",
    "PDF Detail",
    "PDF Local Path",
];

// Text normalization functions live in `text`; re-exported here to mirror
// `models.py`, where they were originally defined.
pub use crate::text::{
    normalize_doi, normalize_issn, normalize_issn_list, normalize_title_key, safe_filename,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Article {
    pub title: String,
    pub authors: String,
    pub link: String,
    pub relevance_score: Option<f64>,
    pub doi: String,
    pub journal: String,
    pub issns: Vec<String>,
    pub publication_year: Option<i32>,
    pub sjr_quartile: String,
    pub sjr_year: Option<i32>,
    pub sjr_category: String,
    pub sjr_source_url: String,
    pub sinta_rank: String,
    pub sinta_year: Option<i32>,
    pub sinta_source_url: String,
    pub pdf_url: String,
    pub pdf_source_url: String,
    pub pdf_status: String,
    pub pdf_detail: String,
    pub pdf_local_path: String,

    // ---- v3.4.0 metadata, all from payloads already fetched ----
    // Every field is `#[serde(default)]` so a results sidecar written by 3.3.x
    // still loads: `load_results` deserializes the whole array, and one missing
    // key would otherwise discard the entire restored session.
    /// Crossref `is-referenced-by-count`, or OpenAlex `cited_by_count`.
    #[serde(default)]
    pub cited_by: Option<i64>,
    /// Crossref volume / issue / page — the citation-grade venue line.
    #[serde(default)]
    pub volume: String,
    #[serde(default)]
    pub issue: String,
    #[serde(default)]
    pub pages: String,
    #[serde(default)]
    pub publisher: String,
    /// Crossref `license[].URL` for the version of record.
    #[serde(default)]
    pub license: String,
    /// OpenAlex `open_access.oa_status`: gold/green/bronze/hybrid/closed.
    #[serde(default)]
    pub oa_status: String,
    /// OpenAlex `ids.pmid` / `ids.pmcid` (bare numeric ids, no URL).
    #[serde(default)]
    pub pmid: String,
    #[serde(default)]
    pub pmcid: String,
    /// Crossref `author[].ORCID`, de-duplicated, without the URL prefix.
    #[serde(default)]
    pub orcids: Vec<String>,
    /// Structured author names from Crossref, when the record was resolved.
    #[serde(default)]
    pub authors_structured: Vec<String>,
    /// `"retraction"`, `"correction"`, `"expression-of-concern"`, or empty.
    #[serde(default)]
    pub retraction: String,
    /// DOI of the notice that flags this record, when known.
    #[serde(default)]
    pub retraction_doi: String,
    /// Crossref funder names with award numbers, e.g. `"NWO (840.11.001)"`.
    #[serde(default)]
    pub funders: Vec<String>,
    /// How the SJR label was matched: `"issn:1234-5678"` or `"title"`.
    #[serde(default)]
    pub sjr_matched_by: String,
    /// How the SINTA label was matched.
    #[serde(default)]
    pub sinta_matched_by: String,
}

impl Default for Article {
    fn default() -> Self {
        Self {
            title: String::new(),
            authors: String::new(),
            link: String::new(),
            relevance_score: None,
            doi: String::new(),
            journal: String::new(),
            issns: Vec::new(),
            publication_year: None,
            sjr_quartile: String::new(),
            sjr_year: None,
            sjr_category: String::new(),
            sjr_source_url: String::new(),
            sinta_rank: String::new(),
            sinta_year: None,
            sinta_source_url: String::new(),
            pdf_url: String::new(),
            pdf_source_url: String::new(),
            pdf_status: PDF_STATUS_UNKNOWN.to_string(),
            pdf_detail: String::new(),
            pdf_local_path: String::new(),
            cited_by: None,
            volume: String::new(),
            issue: String::new(),
            pages: String::new(),
            publisher: String::new(),
            license: String::new(),
            oa_status: String::new(),
            pmid: String::new(),
            pmcid: String::new(),
            orcids: Vec::new(),
            authors_structured: Vec::new(),
            retraction: String::new(),
            retraction_doi: String::new(),
            funders: Vec::new(),
            sjr_matched_by: String::new(),
            sinta_matched_by: String::new(),
        }
    }
}

impl Article {
    /// `Article(title)` with every other field defaulted (`pdf_status = "unknown"`).
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// Ordered 20-column export row mirroring `to_export_row()`.
    /// Empty optional values become empty cells exactly as in Python.
    pub fn to_export_row(&self) -> Vec<(&'static str, ExportCell)> {
        fn cell(k: &'static str, v: ExportCell) -> (&'static str, ExportCell) {
            (k, v)
        }
        let s = |v: &str| ExportCell::Str(v.to_string());
        let empty = || ExportCell::Str(String::new());
        vec![
            cell("Title", s(&self.title)),
            cell("Authors", s(&self.authors)),
            cell("Link", s(&self.link)),
            cell(
                "Relevance Score",
                match self.relevance_score {
                    Some(v) => ExportCell::Double(v),
                    None => empty(),
                },
            ),
            cell("DOI", s(&self.doi)),
            cell("Journal", s(&self.journal)),
            cell("ISSNs", ExportCell::Str(self.issns.join("; "))),
            cell(
                "Publication Year",
                match self.publication_year {
                    Some(v) => ExportCell::Int(v as i64),
                    None => empty(),
                },
            ),
            cell("SJR Quartile", s(&self.sjr_quartile)),
            cell(
                "SJR Year",
                match self.sjr_year {
                    Some(v) => ExportCell::Int(v as i64),
                    None => empty(),
                },
            ),
            cell("SJR Category", s(&self.sjr_category)),
            cell("SJR Source", s(&self.sjr_source_url)),
            cell("SINTA Rank", s(&self.sinta_rank)),
            cell(
                "SINTA Year",
                match self.sinta_year {
                    Some(v) => ExportCell::Int(v as i64),
                    None => empty(),
                },
            ),
            cell("SINTA Source", s(&self.sinta_source_url)),
            cell("PDF Status", s(&self.pdf_status)),
            cell("PDF URL", s(&self.pdf_url)),
            cell("PDF Source", s(&self.pdf_source_url)),
            cell("PDF Detail", s(&self.pdf_detail)),
            cell("PDF Local Path", s(&self.pdf_local_path)),
        ]
    }
}

/// Export-only citation record for one article (`CitationRecord`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CitationRecord {
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub container: String,
    pub entry_type: String,
    pub doi: String,
    pub issns: Vec<String>,
    pub volume: String,
    pub issue: String,
    pub pages: String,
    pub publisher: String,
    pub url: String,
    pub source: String,
    pub verified: bool,
    pub fallback_authors_line: String,
    pub warnings: Vec<String>,
}

/// The 8 extraction statuses, exact string values.
///
/// `rename_all` is load-bearing: the UI status tables key on the Python enum
/// values ("SUCCESS", "PARTIAL_SUCCESS", ...). Without it serde emits "Success"
/// and every heading/tone lookup silently falls through to the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExtractionStatus {
    Success,
    PartialSuccess,
    NoResults,
    RateLimited,
    Blocked,
    NetworkError,
    ParsingError,
    Cancelled,
}

impl ExtractionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::PartialSuccess => "PARTIAL_SUCCESS",
            Self::NoResults => "NO_RESULTS",
            Self::RateLimited => "RATE_LIMITED",
            Self::Blocked => "BLOCKED",
            Self::NetworkError => "NETWORK_ERROR",
            Self::ParsingError => "PARSING_ERROR",
            Self::Cancelled => "CANCELLED",
        }
    }
}

/// `ExtractionResult`; `has_articles` mirrors the Python property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub status: ExtractionStatus,
    pub articles: Vec<Article>,
    pub requested_pages: i32,
    #[serde(default)]
    pub successful_pages: i32,
    #[serde(default)]
    pub failure_page: Option<i32>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub diagnostic: String,
    #[serde(default)]
    pub duplicates_removed: i32,
    #[serde(default)]
    pub invalid_articles_removed: i32,
    /// Of `duplicates_removed`, how many matched only by DOI (1.3).
    #[serde(default)]
    pub duplicates_by_doi: i32,
}

impl ExtractionResult {
    pub fn has_articles(&self) -> bool {
        !self.articles.is_empty()
    }
}

/// `SearchRequest` (frozen dataclass, tkinter_app L110).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRequestPlaceholder {
    pub query: String,
    pub pages: i32,
    pub ranking_enabled: bool,
    #[serde(default)]
    pub output_folder: String,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
}
