//! Scholar HTTP client + scrape orchestration ported from `scraper/client.py`
//! verbatim. HTTP is abstracted behind `HttpFetcher` (mirrors the Python
//! `requests.Session` seam used by FakeSession tests); cancellation behind
//! `CancelEvent` (mirrors `threading.Event`, including `wait(timeout)`).

use crate::dedupe::deduplicate_articles;
use crate::models::{Article, ExtractionResult, ExtractionStatus};
use crate::ranking::ranked_articles;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub const BASE_URL: &str = "https://scholar.google.com/scholar";
pub const DEFAULT_PAGE_DELAY_SECONDS: f64 = 1.5;
pub const DEFAULT_BACKOFF_SECONDS: f64 = 2.0;
pub const DEFAULT_MAX_RETRIES: u32 = 1;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
pub const ACCEPT_HEADER: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
pub const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";

/// Default request headers (`DEFAULT_HEADERS`).
pub fn default_headers() -> Vec<(&'static str, String)> {
    vec![
        ("User-Agent", USER_AGENT.to_string()),
        ("Accept", ACCEPT_HEADER.to_string()),
        ("Accept-Language", ACCEPT_LANGUAGE.to_string()),
    ]
}

/// Ordered query parameters (`build_scholar_params`): start, q, hl, as_sdt,
/// then optional as_ylo/as_yhi. Order preserved for URL stability.
pub fn build_scholar_params(
    query: &str,
    page: i32,
    year_from: Option<i32>,
    year_to: Option<i32>,
) -> Result<Vec<(String, String)>, String> {
    validate_year_range(year_from, year_to)?;
    let mut params = vec![
        ("start".to_string(), (page * 10).to_string()),
        ("q".to_string(), query.to_string()),
        ("hl".to_string(), "en".to_string()),
        ("as_sdt".to_string(), "0,5".to_string()),
    ];
    if let Some(y) = year_from {
        params.push(("as_ylo".to_string(), y.to_string()));
    }
    if let Some(y) = year_to {
        params.push(("as_yhi".to_string(), y.to_string()));
    }
    Ok(params)
}

pub fn validate_year_range(year_from: Option<i32>, year_to: Option<i32>) -> Result<(), String> {
    for year in [year_from, year_to].into_iter().flatten() {
        if !(1000..=9999).contains(&year) {
            return Err("Year must be a whole four-digit number (1000–9999).".to_string());
        }
    }
    if let (Some(f), Some(t)) = (year_from, year_to) {
        if f > t {
            return Err("From year must not be later than To year.".to_string());
        }
    }
    Ok(())
}

/// `quote_plus` encoding of a query-string component (spaces → '+').
fn quote_plus(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(*byte as char),
            b' ' => out.push('+'),
            b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `build_scholar_url` — params in insertion order, `quote_plus`-encoded.
pub fn build_scholar_url(
    query: &str,
    page: i32,
    year_from: Option<i32>,
    year_to: Option<i32>,
) -> Result<String, String> {
    let params = build_scholar_params(query, page, year_from, year_to)?;
    let query_string = params
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("{BASE_URL}?{query_string}"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchResponse {
    pub text: String,
    pub status_code: u16,
}

/// Request failure classes mirroring `requests` exceptions; `is_retryable`
/// mirrors `_is_retryable_request_error` (ConnectionError, Timeout only).
#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// `requests.Timeout` (connect or read).
    Timeout { class: String },
    /// `requests.ConnectionError`.
    Connection { class: String },
    /// Any other `requests.RequestException` (not retryable).
    Other { class: String },
}

impl FetchError {
    pub fn class_name(&self) -> &str {
        match self {
            Self::Timeout { class } | Self::Connection { class } | Self::Other { class } => class,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::Connection { .. })
    }
}

/// HTTP seam (Python: `requests.Session.get`).
pub trait HttpFetcher {
    fn get(&mut self, url: &str, params: &[(String, String)]) -> Result<FetchResponse, FetchError>;
}

/// Cancellation seam (Python: `threading.Event`).
pub trait CancelEvent {
    fn is_set(&self) -> bool;
    /// Wait up to `seconds`; true if the event became set during the wait.
    fn wait(&self, seconds: f64) -> bool;
}

/// `AtomicBool`-backed event; `wait` sleeps in small slices checking the flag.
pub struct AtomicCancel(pub AtomicBool);

impl AtomicCancel {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Default for AtomicCancel {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelEvent for AtomicCancel {
    fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn wait(&self, seconds: f64) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
        loop {
            if self.is_set() {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50).min(deadline - now));
        }
    }
}

pub struct ScholarClient {
    pub fetcher: Box<dyn HttpFetcher>,
    pub page_delay_seconds: f64,
    pub backoff_seconds: f64,
    pub max_retries: u32,
}

impl ScholarClient {
    pub fn new(fetcher: Box<dyn HttpFetcher>) -> Self {
        Self {
            fetcher,
            page_delay_seconds: DEFAULT_PAGE_DELAY_SECONDS,
            backoff_seconds: DEFAULT_BACKOFF_SECONDS,
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

/// Progress callback shape (current_page, total_pages, phase).
pub type ProgressCallback<'a> = &'a mut dyn FnMut(i32, i32, &str);

fn emit_progress(cb: Option<&mut ProgressCallback<'_>>, current: i32, total: i32, phase: &str) {
    if let Some(f) = cb {
        f(current, total, phase);
    }
}

fn is_cancelled(cancel: Option<&dyn CancelEvent>) -> bool {
    cancel.map(|c| c.is_set()).unwrap_or(false)
}

/// `_wait`: with an event → event.wait; without → sleep, never cancelled.
fn wait_seconds(cancel: Option<&dyn CancelEvent>, seconds: f64) -> bool {
    if seconds <= 0.0 {
        return is_cancelled(cancel);
    }
    match cancel {
        Some(c) => c.wait(seconds),
        None => {
            thread::sleep(Duration::from_secs_f64(seconds));
            false
        }
    }
}

fn backoff_wait(client: &ScholarClient, cancel: Option<&dyn CancelEvent>) -> bool {
    if client.backoff_seconds > 0.0 {
        return wait_seconds(cancel, client.backoff_seconds);
    }
    is_cancelled(cancel)
}

fn cancelled_result(
    articles: Vec<Article>,
    requested_pages: i32,
    successful_pages: i32,
    query: &str,
    ranking_enabled: bool,
    duplicates_removed: i32,
    invalid_articles_removed: i32,
) -> ExtractionResult {
    let deduped = deduplicate_articles(articles);
    let ranked = ranked_articles(query, deduped.articles, ranking_enabled);
    let mut message = "Extraction cancelled.".to_string();
    if !ranked.is_empty() {
        message += &format!(" Preserved {} collected articles.", ranked.len());
    }
    ExtractionResult {
        status: ExtractionStatus::Cancelled,
        articles: ranked,
        requested_pages,
        successful_pages,
        failure_page: None,
        message,
        diagnostic: String::new(),
        duplicates_removed,
        invalid_articles_removed,
        duplicates_by_doi: deduped.duplicates_by_doi,
    }
}

#[allow(clippy::too_many_arguments)]
fn with_partial_if_needed(
    result: ExtractionResult,
    articles: Vec<Article>,
    requested_pages: i32,
    successful_pages: i32,
    failure_page: i32,
    duplicates_removed: i32,
    invalid_articles_removed: i32,
    query: &str,
    ranking_enabled: bool,
) -> ExtractionResult {
    if !articles.is_empty() {
        let deduped = deduplicate_articles(articles);
        let ranked = ranked_articles(query, deduped.articles, ranking_enabled);
        let duplicate_count = duplicates_removed + deduped.duplicates_removed;
        let invalid_count = invalid_articles_removed + deduped.invalid_removed;
        let mut message = format!(
            "Extraction stopped early on page {failure_page}: {} Preserved {} collected articles for review and export.",
            result.message,
            ranked.len()
        );
        if duplicate_count > 0 {
            message += &format!(" Removed {duplicate_count} duplicate articles.");
        }
        return ExtractionResult {
            status: ExtractionStatus::PartialSuccess,
            articles: ranked,
            requested_pages,
            successful_pages,
            failure_page: Some(failure_page),
            message,
            diagnostic: result.status.as_str().to_string(),
            duplicates_removed: duplicate_count,
            invalid_articles_removed: invalid_count,
            duplicates_by_doi: deduped.duplicates_by_doi,
        };
    }

    ExtractionResult {
        status: result.status,
        articles: Vec::new(),
        requested_pages,
        successful_pages,
        failure_page: Some(failure_page),
        message: result.message,
        diagnostic: result.diagnostic,
        duplicates_removed,
        invalid_articles_removed,
        // The empty-articles branch above performs no dedupe, so nothing can
        // have matched by DOI here.
        duplicates_by_doi: 0,
    }
}

/// Scrape orchestration (`scrape_scholar`): page loop with cancel checks,
/// per-page + cumulative dedupe, pacing, partial-success preservation, and
/// final ranking.
#[allow(clippy::too_many_arguments)]
pub fn scrape_scholar(
    query: &str,
    num_pages: i32,
    client: &mut ScholarClient,
    ranking_enabled: bool,
    mut progress_callback: Option<ProgressCallback<'_>>,
    cancel: Option<&dyn CancelEvent>,
    year_from: Option<i32>,
    year_to: Option<i32>,
) -> ExtractionResult {
    if let Err(msg) = validate_year_range(year_from, year_to) {
        // Python raises ValueError out of scrape_scholar before any network
        // use; the UI converts it to a setup-error status.
        panic!("validate_year_range: {msg}");
    }
    let mut articles: Vec<Article> = Vec::new();
    let mut duplicates_removed = 0;
    let mut invalid_articles_removed = 0;
    let mut successful_pages = 0;

    for page in 0..num_pages {
        let page_number = page + 1;
        if is_cancelled(cancel) {
            return cancelled_result(
                articles,
                num_pages,
                successful_pages,
                query,
                ranking_enabled,
                duplicates_removed,
                invalid_articles_removed,
            );
        }

        emit_progress(
            progress_callback.as_mut(),
            page_number,
            num_pages,
            "requesting",
        );
        let response_result = fetch_with_retries(client, query, page, cancel, year_from, year_to);
        match response_result {
            Ok(FetchOrResult::Cancelled) => {
                return cancelled_result(
                    articles,
                    num_pages,
                    successful_pages,
                    query,
                    ranking_enabled,
                    duplicates_removed,
                    invalid_articles_removed,
                );
            }
            Ok(FetchOrResult::Failed(result)) => {
                return with_partial_if_needed(
                    result,
                    articles,
                    num_pages,
                    successful_pages,
                    page_number,
                    duplicates_removed,
                    invalid_articles_removed,
                    query,
                    ranking_enabled,
                );
            }
            Ok(FetchOrResult::Response(response)) => {
                emit_progress(
                    progress_callback.as_mut(),
                    page_number,
                    num_pages,
                    "parsing",
                );
                let page_result =
                    crate::parser::parse_scholar_page(&response.text, response.status_code);
                if page_result.status != ExtractionStatus::Success {
                    return with_partial_if_needed(
                        ExtractionResult {
                            status: page_result.status,
                            articles: Vec::new(),
                            requested_pages: num_pages,
                            successful_pages,
                            failure_page: Some(page_number),
                            message: page_result.message,
                            diagnostic: page_result.diagnostic,
                            duplicates_removed: 0,
                            invalid_articles_removed: 0,
                            duplicates_by_doi: 0,
                        },
                        articles,
                        num_pages,
                        successful_pages,
                        page_number,
                        duplicates_removed,
                        invalid_articles_removed,
                        query,
                        ranking_enabled,
                    );
                }

                let page_deduped = deduplicate_articles(page_result.articles);
                articles.extend(page_deduped.articles);
                duplicates_removed += page_deduped.duplicates_removed;
                invalid_articles_removed += page_deduped.invalid_removed;

                let all_deduped = deduplicate_articles(articles);
                articles = all_deduped.articles;
                duplicates_removed += all_deduped.duplicates_removed;
                invalid_articles_removed += all_deduped.invalid_removed;
                successful_pages += 1;
                emit_progress(
                    progress_callback.as_mut(),
                    successful_pages,
                    num_pages,
                    "completed",
                );

                if is_cancelled(cancel) {
                    return cancelled_result(
                        articles,
                        num_pages,
                        successful_pages,
                        query,
                        ranking_enabled,
                        duplicates_removed,
                        invalid_articles_removed,
                    );
                }

                if page < num_pages - 1
                    && client.page_delay_seconds > 0.0
                    && wait_seconds(cancel, client.page_delay_seconds)
                {
                    return cancelled_result(
                        articles,
                        num_pages,
                        successful_pages,
                        query,
                        ranking_enabled,
                        duplicates_removed,
                        invalid_articles_removed,
                    );
                }
            }
            Err(_) => unreachable!("fetch_with_retries never yields Err"),
        }
    }

    if articles.is_empty() {
        return ExtractionResult {
            status: ExtractionStatus::NoResults,
            articles: Vec::new(),
            requested_pages: num_pages,
            successful_pages,
            failure_page: None,
            message: "Google Scholar returned no results for this query.".to_string(),
            diagnostic: String::new(),
            duplicates_removed,
            invalid_articles_removed,
            duplicates_by_doi: 0,
        };
    }

    let mut message = format!(
        "Extraction complete. Collected {} articles.",
        articles.len()
    );
    if duplicates_removed > 0 {
        message += &format!(" Removed {duplicates_removed} duplicate articles.");
    }

    let articles = ranked_articles(query, articles, ranking_enabled);

    ExtractionResult {
        status: ExtractionStatus::Success,
        articles,
        requested_pages: num_pages,
        successful_pages,
        failure_page: None,
        message,
        diagnostic: String::new(),
        duplicates_removed,
        invalid_articles_removed,
        duplicates_by_doi: 0,
    }
}

enum FetchOrResult {
    Response(FetchResponse),
    Cancelled,
    Failed(ExtractionResult),
}

/// `_fetch_with_retries`: attempts = max_retries + 1; timeouts and
/// connection errors retry after backoff; 502/503/504 retry; 429 never
/// retries (returned to the caller for classification).
fn fetch_with_retries(
    client: &mut ScholarClient,
    query: &str,
    page: i32,
    cancel: Option<&dyn CancelEvent>,
    year_from: Option<i32>,
    year_to: Option<i32>,
) -> Result<FetchOrResult, std::convert::Infallible> {
    let attempts = client.max_retries + 1;
    let mut last_error = String::new();

    for attempt in 0..attempts {
        if is_cancelled(cancel) {
            return Ok(FetchOrResult::Cancelled);
        }

        let url = build_scholar_url(query, page, year_from, year_to).unwrap_or_default();
        let params = build_scholar_params(query, page, year_from, year_to).unwrap_or_default();
        match client.fetcher.get(&url, &params) {
            Err(err) => {
                last_error = err.class_name().to_string();
                if attempt < attempts - 1 && err.is_retryable() {
                    if backoff_wait(client, cancel) {
                        return Ok(FetchOrResult::Cancelled);
                    }
                    continue;
                }
                let (message, diagnostic) = match err {
                    FetchError::Timeout { .. } => (
                        "Network timeout while contacting Google Scholar.".to_string(),
                        last_error,
                    ),
                    _ => (
                        "Network error while contacting Google Scholar.".to_string(),
                        last_error,
                    ),
                };
                return Ok(FetchOrResult::Failed(ExtractionResult {
                    status: ExtractionStatus::NetworkError,
                    articles: Vec::new(),
                    requested_pages: 0,
                    successful_pages: 0,
                    failure_page: None,
                    message,
                    diagnostic,
                    duplicates_removed: 0,
                    invalid_articles_removed: 0,
                    duplicates_by_doi: 0,
                }));
            }
            Ok(response) => {
                if response.status_code == 429 {
                    return Ok(FetchOrResult::Response(response));
                }
                if matches!(response.status_code, 502..=504) && attempt < attempts - 1 {
                    if backoff_wait(client, cancel) {
                        return Ok(FetchOrResult::Cancelled);
                    }
                    continue;
                }
                return Ok(FetchOrResult::Response(response));
            }
        }
    }

    Ok(FetchOrResult::Failed(ExtractionResult {
        status: ExtractionStatus::NetworkError,
        articles: Vec::new(),
        requested_pages: 0,
        successful_pages: 0,
        failure_page: None,
        message: "Network error while contacting Google Scholar.".to_string(),
        diagnostic: last_error,
        duplicates_removed: 0,
        invalid_articles_removed: 0,
        duplicates_by_doi: 0,
    }))
}
