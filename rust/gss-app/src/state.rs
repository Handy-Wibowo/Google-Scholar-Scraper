//! Shared GUI state: the two-mode state machine data, stored articles
//! (row identity = index), catalog, request, and the shared cancel flag.

use gss_core::catalog::JournalCatalog;
use gss_core::models::Article;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// The current search request (`SearchRequest` mirror).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub pages: i32,
    pub ranking_enabled: bool,
    pub output_folder: String,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
}

/// One operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Search,
    Check,
    Import,
    Download,
    Citations,
}

impl Operation {
    #[allow(dead_code)] // parity surface mirroring Python operation strings
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Check => "check",
            Self::Import => "import",
            Self::Download => "download",
            Self::Citations => "citations",
        }
    }
}

/// Worker → UI message envelope (`WorkerMessage`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerMessage {
    /// search finished.
    Result {
        result: gss_core::models::ExtractionResult,
    },
    /// a background task finished.
    TaskResult {
        operation: String,
        payload: serde_json::Value,
    },
    /// a background task failed (worker exceptions never surface tracebacks).
    TaskError { operation: String, message: String },
    /// progress tick.
    Progress {
        operation: String,
        current: i32,
        total: i32,
        phase: String,
    },
}

/// Shareable cancel flag (AtomicBool cannot be cloned; wrap in Arc).
#[derive(Default)]
pub struct CancelFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelFlag {
    pub fn store(&self, value: bool) {
        self.0.store(value, Ordering::SeqCst);
    }

    pub fn load(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn clone_arc(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.0.clone()
    }
}

/// Mutable application state guarded by the UI thread via Tauri commands.
pub struct AppState {
    pub cancel: CancelFlag,
    pub operation: Mutex<Option<Operation>>,
    pub articles: Mutex<Vec<Article>>,
    pub request: Mutex<Option<SearchRequest>>,
    pub catalog: Mutex<JournalCatalog>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            cancel: CancelFlag::default(),
            operation: Mutex::new(None),
            articles: Mutex::new(Vec::new()),
            request: Mutex::new(None),
            catalog: Mutex::new(JournalCatalog::new()),
        }
    }

    pub fn is_running(&self) -> bool {
        self.operation
            .lock()
            .map(|op| op.is_some())
            .unwrap_or(false)
    }

    pub fn set_running(&self, op: Option<Operation>) {
        if let Ok(mut guard) = self.operation.lock() {
            *guard = op;
        }
        // Fresh cancel_event per operation: reset the flag when starting.
        if op.is_some() {
            self.cancel.store(false);
        }
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true);
    }

    /// Handle to the shared cancel flag for watcher threads.
    pub fn cancel_flag_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        // The AtomicBool lives inside the Mutex-free state; share via Arc by
        // reconstructing from the state struct itself. Because AppState is
        // managed by Tauri (Arc'd), we expose the flag through a clone.
        self.cancel.clone_arc()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load()
    }

    pub fn articles(&self) -> Vec<Article> {
        self.articles.lock().map(|a| a.clone()).unwrap_or_default()
    }

    pub fn set_articles(&self, articles: Vec<Article>) {
        if let Ok(mut guard) = self.articles.lock() {
            *guard = articles;
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// `validate_search_inputs` (L155): errors returned as strings.
pub fn validate_search_inputs(
    query: &str,
    pages_text: &str,
    ranking_enabled: bool,
    output_folder: &str,
    year_from_text: &str,
    year_to_text: &str,
) -> Result<SearchRequest, String> {
    if query.trim().is_empty() {
        return Err("Enter a search query.".to_string());
    }
    let pages: i32 = pages_text
        .trim()
        .parse()
        .map_err(|_| "Page count must be a whole number.".to_string())?;
    if pages <= 0 {
        return Err("Page count must be greater than zero.".to_string());
    }
    let year_from = parse_search_year(year_from_text)?;
    let year_to = parse_search_year(year_to_text)?;
    if let (Some(f), Some(t)) = (year_from, year_to) {
        gss_core::client::validate_year_range(Some(f), Some(t))?;
    }
    Ok(SearchRequest {
        query: query.to_string(),
        pages,
        ranking_enabled,
        output_folder: output_folder.to_string(),
        year_from,
        year_to,
    })
}

/// `_parse_search_year` (L146): empty → None; exactly 4 digits → int.
pub fn parse_search_year(text: &str) -> Result<Option<i32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() == 4 && text.chars().all(|c| c.is_ascii_digit()) {
        return Ok(text.parse::<i32>().ok());
    }
    Err("Year must be a whole four-digit number (1000\u{2013}9999).".to_string())
}

/// `export_path` (L183).
#[allow(dead_code)] // used by the JS layer naming; kept for parity tests
pub fn export_path(output_folder: &str, filename: &str) -> std::path::PathBuf {
    if output_folder.trim().is_empty() {
        std::path::PathBuf::from(filename)
    } else {
        std::path::Path::new(output_folder).join(filename)
    }
}

/// Filter application preserving stored-index identity (`filtered_articles`).
#[allow(dead_code)] // JS layer implements the same filter; kept for parity tests
pub fn filtered_articles(articles: &[Article], pdf_filter: &str) -> Vec<(usize, Article)> {
    articles
        .iter()
        .enumerate()
        .filter(|(_i, a)| match pdf_filter {
            "Public PDF available" => a.pdf_status == "available",
            "No public PDF found" => a.pdf_status == "unavailable",
            "Unknown" => a.pdf_status == "unknown",
            _ => true,
        })
        .map(|(i, a)| (i, a.clone()))
        .collect()
}

/// `article_iid` (L294): row identity is the stored index.
#[allow(dead_code)] // JS layer implements the same iid scheme; kept for parity tests
pub fn article_iid(index: usize) -> String {
    format!("article-{index}")
}

/// Table row values (`article_table_row`, L289).
#[allow(dead_code)] // JS layer implements the same row shape; kept for parity tests
pub fn article_table_row(article: &Article) -> [String; 6] {
    [
        article.title.clone(),
        article.authors.clone(),
        crate::strings::score_label(article.relevance_score),
        crate::strings::journal_rank_label(&article.sjr_quartile, &article.sinta_rank),
        crate::strings::pdf_status_label(&article.pdf_status, &article.pdf_local_path),
        if article.link.is_empty() {
            "No link".to_string()
        } else {
            "Open link".to_string()
        },
    ]
}

/// `article_detail_text` (L310).
#[allow(dead_code)] // JS layer implements the same text; kept for parity tests
pub fn article_detail_text(article: Option<&Article>) -> String {
    let disclaimer = crate::strings::DETAIL_DISCLAIMER;
    let Some(a) = article else {
        return format!(
            "Select a row to see its journal, ranking and public-PDF details.\n\n{disclaimer}"
        );
    };
    let none = "unknown";
    let none_list = "none";
    let mut lines = vec![if a.title.is_empty() {
        "(untitled)".to_string()
    } else {
        a.title.clone()
    }];
    lines.push(format!(
        "Authors: {}",
        if a.authors.is_empty() {
            none
        } else {
            &a.authors
        }
    ));
    lines.push(format!(
        "Link: {}",
        if a.link.is_empty() { none } else { &a.link }
    ));
    lines.push(format!(
        "DOI: {}",
        if a.doi.is_empty() { none } else { &a.doi }
    ));
    lines.push(format!(
        "Journal: {}",
        if a.journal.is_empty() {
            none
        } else {
            &a.journal
        }
    ));
    let issns_joined = a.issns.join(", ");
    lines.push(format!(
        "ISSN: {}",
        if a.issns.is_empty() {
            none_list
        } else {
            issns_joined.as_str()
        }
    ));
    lines.push(format!(
        "Publication year: {}",
        a.publication_year
            .map(|y| y.to_string())
            .unwrap_or_else(|| none.to_string())
    ));
    lines.push(String::new());
    lines.push(format!(
        "SJR quartile: {}",
        if a.sjr_quartile.is_empty() {
            none
        } else {
            &a.sjr_quartile
        }
    ));
    if let Some(y) = a.sjr_year {
        lines.push(format!("SJR ranking year: {y}"));
    }
    lines.push(format!(
        "SJR category: {}",
        if a.sjr_category.is_empty() {
            none
        } else {
            &a.sjr_category
        }
    ));
    lines.push(format!(
        "SJR source: {}",
        if a.sjr_source_url.is_empty() {
            "no imported record"
        } else {
            &a.sjr_source_url
        }
    ));
    lines.push(format!(
        "SINTA rank: {}",
        if a.sinta_rank.is_empty() {
            none
        } else {
            &a.sinta_rank
        }
    ));
    if let Some(y) = a.sinta_year {
        lines.push(format!("SINTA ranking year: {y}"));
    }
    lines.push(format!(
        "SINTA source: {}",
        if a.sinta_source_url.is_empty() {
            "no imported record"
        } else {
            &a.sinta_source_url
        }
    ));
    lines.push(String::new());
    lines.push(format!(
        "Public PDF: {}",
        crate::strings::pdf_status_label(&a.pdf_status, &a.pdf_local_path)
    ));
    lines.push(format!(
        "PDF detail: {}",
        if a.pdf_detail.is_empty() {
            "not checked"
        } else {
            &a.pdf_detail
        }
    ));
    lines.push(format!(
        "PDF source: {}",
        if a.pdf_source_url.is_empty() {
            "none"
        } else {
            &a.pdf_source_url
        }
    ));
    lines.push(format!(
        "PDF local path: {}",
        if a.pdf_local_path.is_empty() {
            "not downloaded"
        } else {
            &a.pdf_local_path
        }
    ));
    lines.push(String::new());
    lines.push(disclaimer.to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rules_match_python() {
        assert!(validate_search_inputs("", "1", true, "", "", "").is_err());
        assert_eq!(
            validate_search_inputs("q", "x", true, "", "", "").unwrap_err(),
            "Page count must be a whole number."
        );
        assert_eq!(
            validate_search_inputs("q", "0", true, "", "", "").unwrap_err(),
            "Page count must be greater than zero."
        );
        assert_eq!(
            validate_search_inputs("q", "1", true, "", "20", "").unwrap_err(),
            "Year must be a whole four-digit number (1000\u{2013}9999)."
        );
        let ok = validate_search_inputs("q", "2", true, "out", "2018", "2024").unwrap();
        assert_eq!(ok.pages, 2);
        assert_eq!(ok.year_from, Some(2018));
    }

    #[test]
    fn filtered_articles_keep_stored_index() {
        let articles = vec![
            Article {
                title: "A".into(),
                pdf_status: "available".into(),
                ..Article::default()
            },
            Article {
                title: "B".into(),
                pdf_status: "unknown".into(),
                ..Article::default()
            },
            Article {
                title: "C".into(),
                pdf_status: "available".into(),
                ..Article::default()
            },
        ];
        let visible = filtered_articles(&articles, "Public PDF available");
        let ids: Vec<String> = visible.iter().map(|(i, _)| article_iid(*i)).collect();
        assert_eq!(ids, vec!["article-0", "article-2"]);
    }

    #[test]
    fn iid_format() {
        assert_eq!(article_iid(0), "article-0");
        assert_eq!(article_iid(12), "article-12");
    }

    #[test]
    fn detail_text_has_prompt_and_disclaimer() {
        let text = article_detail_text(None);
        assert!(text.starts_with("Select a row"));
        assert!(text.contains("describe the journal, not this paper"));
    }
}
