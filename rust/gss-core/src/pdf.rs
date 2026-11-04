//! PDF availability/download service ported from `pdf_service.py` verbatim.
//! Three-state contract: available | unavailable | unknown. Every redirect hop
//! is SSRF-revalidated; atomic finalize never overwrites.

use crate::client::{CancelEvent, FetchError};
use crate::models::Article;
use crate::text::{safe_filename, CaseFold};
use regex::Regex;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const PDF_AVAILABLE: &str = "available";
pub const PDF_UNAVAILABLE: &str = "unavailable";
pub const PDF_UNKNOWN: &str = "unknown";

pub const DEFAULT_TIMEOUT: (u64, u64) = (5, 20);
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 5;
pub const CHUNK_SIZE: usize = 64 * 1024;
pub const MIN_PDF_BYTES: usize = 64;
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const HTML_PREVIEW_BYTES: usize = 16 * 1024;
pub const MAX_NAME_ATTEMPTS: u64 = 10000;

pub const OPENALEX_WORK_URL: &str = "https://api.openalex.org/works/doi:{}";
pub const OPENALEX_TITLE_SEARCH_URL: &str = "https://api.openalex.org/works?filter=title.search:{}&per-page=5&select=id,title,open_access,best_oa_location,locations";

pub fn default_pdf_headers() -> Vec<(&'static str, String)> {
    vec![
        (
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"
                .to_string(),
        ),
        ("Accept", "application/pdf,*/*;q=0.8".to_string()),
    ]
}

/// 4xx meaning "there is nothing here", as opposed to "we could not check".
const REJECT_STATUSES: [u16; 6] = [400, 404, 405, 406, 410, 451];

static CHALLENGE_MARKERS: LazyLock<Vec<&'static [u8]>> = LazyLock::new(|| {
    vec![
        b"_fs-ch-" as &[u8],
        b"cf-chl",
        b"cf-browser-verification",
        b"just a moment",
        b"checking your browser",
        b"enable javascript and cookies",
        b"captcha",
        b"pardon our interruption",
        b"attention required",
        b"ddos-guard",
        b"verifying you are human",
    ]
});

static PDF_SIGNATURE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^%PDF-\d+\.\d+").unwrap());

// ---- error taxonomy (mirrors the private exception classes) ----

#[derive(Debug, Clone, PartialEq)]
pub enum PdfError {
    /// `_CandidateRejected`: reachable but not a public PDF.
    Rejected(String),
    /// `_CandidateUnverified`: transport or provider problem.
    Unverified(String),
    /// `_Cancelled`: cancellation was requested mid-work.
    Cancelled,
}

impl PdfError {
    pub fn message(&self) -> &str {
        match self {
            Self::Rejected(m) | Self::Unverified(m) => m,
            Self::Cancelled => "Cancelled",
        }
    }
}

/// `_Outcome`.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub status: String,
    pub detail: String,
    pub source_url: String,
    pub local_path: String,
    /// True when the reason for a non-available status is a local/operational
    /// failure rather than a statement about public availability.
    pub local_failure: bool,
}

impl Outcome {
    fn new(status: &str, detail: &str) -> Self {
        Self {
            status: status.to_string(),
            detail: detail.to_string(),
            source_url: String::new(),
            local_path: String::new(),
            local_failure: false,
        }
    }

    fn with_local_failure(mut self, status: &str, detail: &str) -> Self {
        self.status = status.to_string();
        self.detail = detail.to_string();
        self.local_failure = true;
        self
    }
}

fn status_rank(status: &str) -> i32 {
    match status {
        PDF_UNAVAILABLE => 0,
        PDF_UNKNOWN => 1,
        PDF_AVAILABLE => 2,
        _ => -1,
    }
}

// ---- HTTP seam for PDF fetching (streamed, no auto-redirects) ----

/// A streamed response. `iter_content` yields chunks until EOF (None).
pub trait PdfStream {
    fn status_code(&self) -> u16;
    fn header(&self, name: &str) -> Option<String>;
    /// The URL this response object is currently associated with; the service
    /// rewrites it on each redirect hop (Python sets `response.url`).
    fn url(&self) -> String;
    fn set_url(&mut self, url: String);
    /// Next body chunk of at most `max` bytes (mirrors requests'
    /// `iter_content(chunk_size)`); `None` = end of stream.
    fn next_chunk(&mut self, max: usize) -> Option<Result<Vec<u8>, FetchError>>;
    fn close(&mut self);
}

impl PdfStream for Box<dyn PdfStream> {
    fn status_code(&self) -> u16 {
        (**self).status_code()
    }
    fn header(&self, name: &str) -> Option<String> {
        (**self).header(name)
    }
    fn url(&self) -> String {
        (**self).url()
    }
    fn set_url(&mut self, url: String) {
        (**self).set_url(url)
    }
    fn next_chunk(&mut self, max: usize) -> Option<Result<Vec<u8>, FetchError>> {
        (**self).next_chunk(max)
    }
    fn close(&mut self) {
        (**self).close()
    }
}

/// Transport seam (Python: `requests.Session.get(..., allow_redirects=False, stream=True)`).
pub trait PdfFetcher {
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<Box<dyn PdfStream>, FetchError>;
}

/// DNS seam (Python: `Resolver = Callable[[str], list[str]]`; OSError → unknown).
pub trait DnsResolver {
    fn resolve(&self, host: &str) -> std::io::Result<Vec<String>>;
}

/// Resolver that maps every host to a fixed public IP (mirrors `public_resolver`).
pub struct PublicResolver(pub String);

impl DnsResolver for PublicResolver {
    fn resolve(&self, _host: &str) -> std::io::Result<Vec<String>> {
        Ok(vec![self.0.clone()])
    }
}

fn is_cancelled(cancel: Option<&dyn CancelEvent>) -> bool {
    cancel.map(|c| c.is_set()).unwrap_or(false)
}

// ---- public helpers ----

/// Windows-safe, non-empty PDF file stem (`safe_pdf_filename`).
pub fn pdf_filename_stem(title: &str) -> String {
    safe_filename(title)
}

/// True when `url` is a public http(s) destination (`is_public_http_url`).
pub fn is_public_http_url(url: &str, resolver: &dyn DnsResolver) -> bool {
    matches!(require_public_url(url, resolver), Ok(()))
}

/// `_direct_candidates`: pdf_url then pdf_source_url, deduped, order kept.
fn direct_candidates(article: &Article) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for value in [&article.pdf_url, &article.pdf_source_url] {
        let url = value.trim();
        if !url.is_empty() && !seen.iter().any(|s| s == url) {
            seen.push(url.to_string());
        }
    }
    seen
}

/// `check_pdf_availability`: validate candidates, label pdf_status; unrelated
/// fields preserved; local failures keep any previously verified status.
pub fn check_pdf_availability(
    article: &Article,
    cancel: Option<&dyn CancelEvent>,
    fetcher: &mut dyn PdfFetcher,
    resolver: &dyn DnsResolver,
) -> Article {
    let outcome = evaluate(
        article,
        fetcher,
        DEFAULT_MAX_BYTES,
        resolver,
        cancel,
        None,
        &mut std::collections::HashSet::new(),
    );
    if outcome.local_failure {
        return Article {
            pdf_detail: outcome.detail,
            ..article.clone()
        };
    }
    Article {
        pdf_status: outcome.status,
        pdf_detail: outcome.detail,
        pdf_source_url: if outcome.source_url.is_empty() {
            article.pdf_source_url.clone()
        } else {
            outcome.source_url
        },
        ..article.clone()
    }
}

/// `download_pdfs`: input order preserved; streamed to temp then atomically
/// finalized; no existing file is overwritten; partial files removed.
pub fn download_pdfs(
    articles: &[Article],
    folder: &Path,
    cancel: Option<&dyn CancelEvent>,
    mut progress: Option<crate::crossref::ProgressFn<'_>>,
    fetcher: &mut dyn PdfFetcher,
    max_bytes: usize,
    resolver: &dyn DnsResolver,
) -> Vec<Article> {
    if let Err(_e) = std::fs::create_dir_all(folder) {
        let detail = "Download folder could not be created: OSError".to_string();
        let total = articles.len();
        let results: Vec<Article> = articles.iter().map(|a| preserve(a, &detail)).collect();
        for index in 0..total {
            if let Some(p) = progress.as_mut() {
                p(index + 1, total, "failed");
            }
        }
        return results;
    }

    let mut taken_store: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut results: Vec<Article> = Vec::new();
    let total = articles.len();

    for (index, article) in articles.iter().enumerate() {
        let position = index + 1;
        if is_cancelled(cancel) {
            results.push(preserve(article, "Cancelled before this item was checked."));
            if let Some(p) = progress.as_mut() {
                p(position, total, "cancelled");
            }
            continue;
        }

        if let Some(p) = progress.as_mut() {
            p(position, total, "checking");
        }
        let mut taken = std::mem::take(&mut taken_store);
        let outcome = evaluate(
            article,
            fetcher,
            max_bytes,
            resolver,
            cancel,
            Some(folder),
            &mut taken,
        );
        taken_store = taken;

        if outcome.status == PDF_AVAILABLE {
            if let Some(p) = progress.as_mut() {
                p(position, total, "saved");
            }
            results.push(Article {
                pdf_status: PDF_AVAILABLE.to_string(),
                pdf_detail: outcome.detail,
                pdf_source_url: outcome.source_url,
                pdf_local_path: outcome.local_path,
                ..article.clone()
            });
        } else if outcome.local_failure {
            let phase = if outcome.detail.starts_with("Cancelled") {
                "cancelled"
            } else {
                "failed"
            };
            if let Some(p) = progress.as_mut() {
                p(position, total, phase);
            }
            results.push(preserve(article, &outcome.detail));
        } else {
            if let Some(p) = progress.as_mut() {
                p(position, total, &outcome.status);
            }
            results.push(Article {
                pdf_status: outcome.status,
                pdf_detail: outcome.detail,
                pdf_source_url: if outcome.source_url.is_empty() {
                    article.pdf_source_url.clone()
                } else {
                    outcome.source_url
                },
                ..article.clone()
            });
        }
    }

    results
}

/// `_preserve`: record a local/operational failure without touching status.
fn preserve(article: &Article, detail: &str) -> Article {
    Article {
        pdf_detail: detail.to_string(),
        ..article.clone()
    }
}

/// `_evaluate`: direct candidates → OA discovery (DOI or exact title).
#[allow(clippy::too_many_arguments)]
fn evaluate(
    article: &Article,
    fetcher: &mut dyn PdfFetcher,
    max_bytes: usize,
    resolver: &dyn DnsResolver,
    cancel: Option<&dyn CancelEvent>,
    folder: Option<&Path>,
    taken: &mut std::collections::HashSet<String>,
) -> Outcome {
    let direct = direct_candidates(article);
    let doi = article.doi.trim().to_string();
    let outcome = try_candidates(
        &direct,
        fetcher,
        max_bytes,
        resolver,
        cancel,
        folder,
        taken,
        &article.title,
    );
    if outcome.status == PDF_AVAILABLE || is_cancelled(cancel) {
        return outcome;
    }

    let discovery_kind;
    let discovered = if !doi.is_empty() {
        discovery_kind = "DOI";
        match discover_oa_candidates(&doi, fetcher, resolver, cancel) {
            Ok(list) => Some(list),
            Err(PdfError::Cancelled) => {
                return Outcome::new(
                    PDF_UNKNOWN,
                    "Cancelled while consulting the public PDF index.",
                )
            }
            Err(_) => None,
        }
    } else {
        discovery_kind = "title";
        match discover_oa_candidates_by_title(&article.title, fetcher, resolver, cancel) {
            Ok(list) => Some(list),
            Err(PdfError::Cancelled) => {
                return Outcome::new(
                    PDF_UNKNOWN,
                    "Cancelled while consulting the public PDF index.",
                )
            }
            Err(_) => None,
        }
    };

    let Some(discovered) = discovered else {
        // The index could not be consulted: a clean rejection from the direct
        // candidates is not a complete search result.
        if outcome.status == PDF_UNAVAILABLE {
            return Outcome::new(
                PDF_UNKNOWN,
                &format!(
                    "No public PDF at the record's own link, and the public PDF index could not be consulted by {discovery_kind}."
                ),
            );
        }
        return outcome;
    };
    if discovered.is_empty() {
        return outcome;
    }

    let second = try_candidates(
        &discovered,
        fetcher,
        max_bytes,
        resolver,
        cancel,
        folder,
        taken,
        &article.title,
    );
    // A confirmed PDF wins; otherwise the more informative attempt.
    if status_rank(&second.status) > status_rank(&outcome.status) {
        second
    } else {
        outcome
    }
}

#[allow(clippy::too_many_arguments)]
fn try_candidates(
    urls: &[String],
    fetcher: &mut dyn PdfFetcher,
    max_bytes: usize,
    resolver: &dyn DnsResolver,
    cancel: Option<&dyn CancelEvent>,
    folder: Option<&Path>,
    taken: &mut std::collections::HashSet<String>,
    title: &str,
) -> Outcome {
    if urls.is_empty() {
        return Outcome::new(PDF_UNAVAILABLE, "No public PDF link found on this record.");
    }

    let mut rejection = String::new();
    let mut unverified = String::new();

    for url in urls {
        if is_cancelled(cancel) {
            return Outcome::new(PDF_UNKNOWN, "Cancelled while checking public PDF links.");
        }

        match open_candidate(url, fetcher, resolver, cancel) {
            Ok(mut response) => {
                let resolved = {
                    let u = response.url();
                    if u.is_empty() {
                        url.clone()
                    } else {
                        u
                    }
                };
                let result: Outcome = match folder {
                    None => {
                        let preview = match read_preview(&mut response, cancel, MIN_PDF_BYTES) {
                            Ok(p) => p,
                            Err(PdfError::Cancelled) => {
                                response.close();
                                return Outcome::with_local_failure(
                                    Outcome::new(PDF_UNKNOWN, ""),
                                    PDF_UNKNOWN,
                                    "Cancelled while downloading a public PDF.",
                                );
                            }
                            Err(PdfError::Unverified(msg)) => {
                                unverified =
                                    format!("Transport error while reading the public PDF: {msg}");
                                response.close();
                                continue;
                            }
                            Err(PdfError::Rejected(msg)) => {
                                rejection = msg;
                                response.close();
                                continue;
                            }
                        };
                        if let Err(e) = validate_pdf_preview(&preview) {
                            response.close();
                            match e {
                                PdfError::Rejected(msg) => rejection = msg,
                                other => unverified = other.message().to_string(),
                            }
                            continue;
                        }
                        Outcome {
                            status: PDF_AVAILABLE.to_string(),
                            detail: format!("Validated PDF content from {resolved}"),
                            source_url: resolved.clone(),
                            local_path: String::new(),
                            local_failure: false,
                        }
                    }
                    Some(folder) => {
                        let stream_result =
                            stream_to_file(&mut response, folder, max_bytes, cancel, title);
                        if let Err(e) = stream_result {
                            response.close();
                            match e {
                                PdfError::Cancelled => {
                                    return Outcome::with_local_failure(
                                        Outcome::new(PDF_UNKNOWN, ""),
                                        PDF_UNKNOWN,
                                        "Cancelled while downloading a public PDF.",
                                    );
                                }
                                PdfError::Rejected(msg) => rejection = msg,
                                PdfError::Unverified(msg) => unverified = msg,
                            }
                            continue;
                        }
                        let temp_path = stream_result.unwrap();
                        let finalize_result = finalize_download(&temp_path, folder, title, taken);
                        discard(Path::new(&temp_path));
                        match finalize_result {
                            Err(e) => {
                                response.close();
                                return Outcome::with_local_failure(
                                    Outcome::new(PDF_UNKNOWN, ""),
                                    PDF_UNKNOWN,
                                    &format!("Could not store the public PDF locally: {e}"),
                                );
                            }
                            Ok(final_path) => Outcome {
                                status: PDF_AVAILABLE.to_string(),
                                detail: format!("Downloaded PDF from {resolved}"),
                                source_url: resolved.clone(),
                                local_path: final_path.to_string_lossy().into_owned(),
                                local_failure: false,
                            },
                        }
                    }
                };
                response.close();
                return result;
            }
            Err(PdfError::Cancelled) => {
                return Outcome::new(PDF_UNKNOWN, "Cancelled while checking public PDF links.")
            }
            Err(PdfError::Rejected(msg)) => rejection = msg,
            Err(PdfError::Unverified(msg)) => unverified = msg,
        }
    }

    if !unverified.is_empty() {
        return Outcome::new(PDF_UNKNOWN, &unverified);
    }
    Outcome::new(
        PDF_UNAVAILABLE,
        &if rejection.is_empty() {
            "No public PDF found at the checked link(s).".to_string()
        } else {
            rejection
        },
    )
}

/// `_open_candidate`: manual redirects with per-hop public revalidation.
fn open_candidate(
    url: &str,
    fetcher: &mut dyn PdfFetcher,
    resolver: &dyn DnsResolver,
    cancel: Option<&dyn CancelEvent>,
) -> Result<Box<dyn PdfStream>, PdfError> {
    let mut current = url.to_string();
    for hop in 0..=MAX_REDIRECTS {
        if let Err(e) = require_public_url(&current, resolver) {
            if hop == 0 {
                return Err(e);
            }
            // A redirect leaving the public internet is inconclusive.
            return Err(PdfError::Unverified(format!(
                "Redirect left the public internet: {}",
                e.message()
            )));
        }

        let mut response = fetcher.get(&current, &pdf_request_headers()).map_err(|e| {
            PdfError::Unverified(format!(
                "Transport error while fetching the public PDF link: {}",
                fetch_error_class(&e)
            ))
        })?;

        let status = response.status_code();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response.header("Location").unwrap_or_default();
            response.close();
            if location.is_empty() {
                return Err(PdfError::Unverified(
                    "Redirect response carried no Location header.".to_string(),
                ));
            }
            current = urljoin(&current, &location);
            continue;
        }

        if REJECT_STATUSES.contains(&status) {
            response.close();
            return Err(PdfError::Rejected(format!(
                "Public PDF link returned HTTP {status}."
            )));
        }

        if !(200..400).contains(&status) {
            response.close();
            return Err(PdfError::Unverified(format!(
                "Public PDF link returned HTTP {status}."
            )));
        }

        let content_type = response
            .header("Content-Type")
            .unwrap_or_default()
            .to_lowercase();
        if content_type.starts_with("text/html") {
            let preview = read_preview(&mut response, cancel, HTML_PREVIEW_BYTES);
            response.close();
            let preview = match preview {
                Ok(p) => p,
                Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
                Err(PdfError::Unverified(m)) => {
                    return Err(PdfError::Unverified(format!(
                        "Transport error while reading the public PDF page: {m}"
                    )))
                }
                Err(PdfError::Rejected(m)) => return Err(PdfError::Rejected(m)),
            };
            let lowered = preview.to_ascii_lowercase();
            if CHALLENGE_MARKERS
                .iter()
                .any(|marker| contains_sub(&lowered, marker))
            {
                return Err(PdfError::Unverified(
                    "Public PDF source could not be verified because it returned a bot-challenge page."
                        .to_string(),
                ));
            }
            return Err(PdfError::Rejected(
                "Public PDF link returned an HTML page, not a PDF.".to_string(),
            ));
        }

        // Record where the chain actually landed.
        response.set_url(current);
        return Ok(response);
    }
    Err(PdfError::Unverified(
        "Too many redirects while resolving the public PDF link.".to_string(),
    ))
}

fn pdf_request_headers() -> Vec<(String, String)> {
    default_pdf_headers()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn fetch_error_class(e: &FetchError) -> &str {
    e.class_name()
}

fn contains_sub(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// `_validate_pdf_chunk`: versioned signature + no HTML markers in first 1KB.
fn validate_pdf_chunk(chunk: &[u8]) -> Result<(), PdfError> {
    if chunk.is_empty() {
        return Err(PdfError::Rejected(
            "Public PDF link returned an empty response.".to_string(),
        ));
    }
    let head_str = String::from_utf8_lossy(&chunk[..chunk.len().min(16)]);
    if !PDF_SIGNATURE.is_match(&head_str) {
        return Err(PdfError::Rejected(
            "Response does not start with a PDF signature; not treated as a public PDF."
                .to_string(),
        ));
    }
    let head: Vec<u8> = chunk[..chunk.len().min(1024)].to_ascii_lowercase();
    for marker in [
        &b"<html"[..],
        &b"<!doctype html"[..],
        &b"<head"[..],
        &b"<body"[..],
    ] {
        if contains_sub(&head, marker) {
            return Err(PdfError::Rejected(
                "Response carries a PDF header but HTML content.".to_string(),
            ));
        }
    }
    Ok(())
}

/// `_validate_pdf_preview`: chunk validation + minimum size.
fn validate_pdf_preview(preview: &[u8]) -> Result<(), PdfError> {
    validate_pdf_chunk(preview)?;
    if preview.len() < MIN_PDF_BYTES {
        return Err(PdfError::Rejected(
            "Response is too small to be a valid PDF.".to_string(),
        ));
    }
    Ok(())
}

/// `_read_preview`: accumulate up to `needed` bytes; handles split headers.
fn read_preview(
    stream: &mut dyn PdfStream,
    cancel: Option<&dyn CancelEvent>,
    needed: usize,
) -> Result<Vec<u8>, PdfError> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut total = 0;
    let chunk_size = CHUNK_SIZE.min(needed);
    loop {
        if is_cancelled(cancel) {
            return Err(PdfError::Cancelled);
        }
        let chunk = match stream.next_chunk(chunk_size) {
            None => break,
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(PdfError::Unverified(e.class_name().to_string())),
        };
        if chunk.is_empty() {
            continue;
        }
        let remaining = needed - total;
        let take = chunk.len().min(remaining);
        parts.push(chunk[..take].to_vec());
        total += take;
        if total >= needed {
            break;
        }
    }
    Ok(parts.concat())
}

/// `_stream_to_file`: streamed download with head validation and cleanup.
fn stream_to_file(
    stream: &mut dyn PdfStream,
    folder: &Path,
    max_bytes: usize,
    cancel: Option<&dyn CancelEvent>,
    _title: &str,
) -> Result<String, PdfError> {
    // mkstemp(dir=folder, prefix=".pdf-part-", suffix=".tmp")
    let temp_path = make_temp_path(folder)?;
    let handle = std::fs::File::create(&temp_path);
    let mut handle = match handle {
        Ok(h) => h,
        Err(e) => {
            return Err(PdfError::Unverified(format!(
                "Transport error while downloading the public PDF: {e}"
            )))
        }
    };
    let mut total = 0;
    let mut head: Vec<u8> = Vec::new();
    let mut validated = false;

    let result = (|| -> Result<(), PdfError> {
        loop {
            if is_cancelled(cancel) {
                return Err(PdfError::Cancelled);
            }
            let chunk = match stream.next_chunk(CHUNK_SIZE) {
                None => break,
                Some(Ok(c)) => c,
                Some(Err(e)) => {
                    return Err(PdfError::Unverified(format!(
                        "Transport error while downloading the public PDF: {}",
                        e.class_name()
                    )))
                }
            };
            if chunk.is_empty() {
                continue;
            }
            total += chunk.len();
            if total > max_bytes {
                return Err(PdfError::Unverified(format!(
                    "Public PDF exceeds the {} MB size limit; not stored.",
                    max_bytes / (1024 * 1024)
                )));
            }
            if !validated {
                head.extend_from_slice(&chunk);
                if head.len() < MIN_PDF_BYTES {
                    continue;
                }
                validate_pdf_preview(&head)?;
                handle.write_all(&head).map_err(|e| {
                    PdfError::Unverified(format!(
                        "Transport error while downloading the public PDF: {e}"
                    ))
                })?;
                head.clear();
                validated = true;
                continue;
            }
            handle.write_all(&chunk).map_err(|e| {
                PdfError::Unverified(format!(
                    "Transport error while downloading the public PDF: {e}"
                ))
            })?;
        }
        if !validated {
            validate_pdf_preview(&head)?;
            handle.write_all(&head).map_err(|e| {
                PdfError::Unverified(format!(
                    "Transport error while downloading the public PDF: {e}"
                ))
            })?;
        }
        handle.flush().map_err(|e| {
            PdfError::Unverified(format!(
                "Transport error while downloading the public PDF: {e}"
            ))
        })?;
        Ok(())
    })();

    drop(handle);
    match result {
        Ok(()) => Ok(temp_path.to_string_lossy().into_owned()),
        Err(e) => {
            discard(&temp_path);
            Err(e)
        }
    }
}

fn make_temp_path(folder: &Path) -> Result<PathBuf, PdfError> {
    for attempt in 0..10000u64 {
        let candidate = folder.join(format!(".pdf-part-{attempt:x}.tmp"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(PdfError::Unverified(format!(
                    "Transport error while downloading the public PDF: {e}"
                )))
            }
        }
    }
    Err(PdfError::Unverified(
        "Transport error while downloading the public PDF: OSError".to_string(),
    ))
}

fn discard(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// `finalize_download`: safe name, atomic exclusive claim, `(n)` suffixes.
pub fn finalize_download(
    temp_path: &str,
    folder: &Path,
    title: &str,
    taken: &mut std::collections::HashSet<String>,
) -> Result<PathBuf, String> {
    let stem = safe_filename(title);
    let mut counter: u64 = 0;
    loop {
        let name = if counter == 0 {
            format!("{stem}.pdf")
        } else {
            format!("{stem} ({counter}).pdf")
        };
        let candidate = folder.join(&name);
        if taken.contains(&name.casefold()) {
            counter += 1;
            if counter > MAX_NAME_ATTEMPTS {
                return Err("Could not find a free file name for the downloaded PDF.".to_string());
            }
            continue;
        }
        if claim_path(temp_path, &candidate) {
            taken.insert(name.casefold());
            return Ok(candidate);
        }
        counter += 1;
        if counter > MAX_NAME_ATTEMPTS {
            return Err("Could not find a free file name for the downloaded PDF.".to_string());
        }
    }
}

/// `_claim_path`: hard link (exclusive); fallback: O_CREAT|O_EXCL reservation.
fn claim_path(temp_path: &str, candidate: &Path) -> bool {
    match std::fs::hard_link(temp_path, candidate) {
        Ok(()) => {
            discard(Path::new(temp_path));
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => claim_path_by_reservation(temp_path, candidate),
    }
}

fn claim_path_by_reservation(temp_path: &str, candidate: &Path) -> bool {
    let descriptor = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(candidate);
    let file = match descriptor {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return false,
        Err(_) => return false,
    };
    drop(file);
    match std::fs::rename(temp_path, candidate) {
        Ok(()) => true,
        Err(_) => {
            discard(candidate);
            false
        }
    }
}

/// OA discovery by DOI (`_discover_oa_candidates`); None = index unreachable.
fn discover_oa_candidates(
    doi: &str,
    fetcher: &mut dyn PdfFetcher,
    resolver: &dyn DnsResolver,
    cancel: Option<&dyn CancelEvent>,
) -> Result<Vec<String>, PdfError> {
    let api_url = OPENALEX_WORK_URL.replace("{}", &quote_url(doi, "/"));
    match open_metadata(&api_url, fetcher, resolver, cancel) {
        Ok(payload) => Ok(oa_pdf_urls(&payload)),
        Err(e) => Err(e),
    }
}

/// OA discovery by exact normalized title (`_discover_oa_candidates_by_title`).
fn discover_oa_candidates_by_title(
    title: &str,
    fetcher: &mut dyn PdfFetcher,
    resolver: &dyn DnsResolver,
    cancel: Option<&dyn CancelEvent>,
) -> Result<Vec<String>, PdfError> {
    let wanted = crate::text::normalize_title_key(title);
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let api_url = OPENALEX_TITLE_SEARCH_URL.replace("{}", &quote_url(title.trim(), ""));

    let payload = open_metadata(&api_url, fetcher, resolver, cancel)?;
    let Some(results) = payload.get("results").and_then(|r| r.as_array()) else {
        return Err(PdfError::Unverified(
            "Public PDF index returned an unexpected payload.".to_string(),
        ));
    };

    let mut matches: Vec<(String, &serde_json::Value)> = Vec::new();
    for (index, work) in results.iter().enumerate() {
        let Some(work_title) = work.get("title").and_then(|t| t.as_str()) else {
            continue;
        };
        if crate::text::normalize_title_key(work_title) != wanted {
            continue;
        }
        let work_id = work
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .trim()
            .casefold();
        let distinct_key = if work_id.is_empty() {
            format!("missing-id:{index}")
        } else {
            work_id.to_string()
        };
        if !matches.iter().any(|(k, _)| *k == distinct_key) {
            matches.push((distinct_key, work));
            if matches.len() > 1 {
                return Ok(Vec::new());
            }
        }
    }

    if matches.len() != 1 {
        return Ok(Vec::new());
    }
    Ok(oa_pdf_urls(matches[0].1))
}

/// `_oa_pdf_urls`: ≤3 distinct PDF URLs from OA-flagged locations.
fn oa_pdf_urls(work: &serde_json::Value) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut locations: Vec<&serde_json::Value> = Vec::new();
    if let Some(best) = work.get("best_oa_location") {
        locations.push(best);
    }
    if let Some(raw) = work.get("locations").and_then(|l| l.as_array()) {
        locations.extend(raw.iter());
    }
    for location in locations {
        if location.get("is_oa").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        if let Some(url) = location.get("pdf_url").and_then(|u| u.as_str()) {
            let candidate = url.trim().to_string();
            if !candidate.is_empty() && !urls.contains(&candidate) {
                urls.push(candidate);
            }
        }
        if urls.len() >= 3 {
            break;
        }
    }
    urls
}

/// `_open_metadata`: small JSON doc with the same hop validation.
fn open_metadata(
    url: &str,
    fetcher: &mut dyn PdfFetcher,
    resolver: &dyn DnsResolver,
    cancel: Option<&dyn CancelEvent>,
) -> Result<serde_json::Value, PdfError> {
    let mut current = url.to_string();
    for _hop in 0..=MAX_REDIRECTS {
        require_public_url(&current, resolver)?;
        if is_cancelled(cancel) {
            return Err(PdfError::Cancelled);
        }

        let mut response = fetcher.get(&current, &pdf_request_headers()).map_err(|e| {
            PdfError::Unverified(format!(
                "Transport error while consulting the public PDF index: {}",
                e.class_name()
            ))
        })?;

        let result = (|| -> Result<serde_json::Value, PdfError> {
            let status = response.status_code();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response.header("Location").unwrap_or_default();
                if location.is_empty() {
                    return Err(PdfError::Unverified(
                        "Index redirect carried no Location header.".to_string(),
                    ));
                }
                current = urljoin(&current, &location);
                return Ok(serde_json::Value::Null); // sentinel: redirect continue
            }
            if status != 200 {
                return Err(PdfError::Unverified(format!(
                    "Public PDF index returned HTTP {status}."
                )));
            }
            let body = read_bounded(&mut response, MAX_METADATA_BYTES, cancel)?;
            let text = String::from_utf8(body).map_err(|_| {
                PdfError::Unverified("Public PDF index returned a malformed JSON body.".to_string())
            })?;
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) if v.is_object() => Ok(v),
                Ok(_) => Err(PdfError::Unverified(
                    "Public PDF index returned an unexpected payload.".to_string(),
                )),
                Err(_) => Err(PdfError::Unverified(
                    "Public PDF index returned a malformed JSON body.".to_string(),
                )),
            }
        })();

        let payload = match result {
            Ok(serde_json::Value::Null) => continue, // redirect sentinel
            Ok(v) => v,
            Err(e) => {
                response.close();
                return Err(e);
            }
        };
        response.close();
        return Ok(payload);
    }
    Err(PdfError::Unverified(
        "Too many redirects while consulting the public PDF index.".to_string(),
    ))
}

/// `_read_bounded`: at most `limit` bytes from a streamed response.
fn read_bounded(
    stream: &mut dyn PdfStream,
    limit: usize,
    cancel: Option<&dyn CancelEvent>,
) -> Result<Vec<u8>, PdfError> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut total = 0;
    loop {
        if is_cancelled(cancel) {
            return Err(PdfError::Cancelled);
        }
        match stream.next_chunk(CHUNK_SIZE) {
            None => break,
            Some(Ok(chunk)) => {
                if chunk.is_empty() {
                    continue;
                }
                total += chunk.len();
                if total > limit {
                    return Err(PdfError::Unverified(format!(
                        "Public PDF index response exceeded the {} KB metadata limit.",
                        limit / 1024
                    )));
                }
                parts.push(chunk);
            }
            Some(Err(e)) => return Err(PdfError::Unverified(e.class_name().to_string())),
        }
    }
    Ok(parts.concat())
}

/// `_require_public_url`: scheme/credentials/host/IP checks + resolution.
pub fn require_public_url(url: &str, resolver: &dyn DnsResolver) -> Result<(), PdfError> {
    let candidate = url.trim();
    if candidate.is_empty() {
        return Err(PdfError::Rejected("Empty PDF link.".to_string()));
    }

    let parsed = crate::url::split(candidate)
        .ok_or_else(|| PdfError::Rejected("PDF link is not an http(s) URL.".to_string()))?;
    let scheme = parsed.scheme.to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(PdfError::Rejected(
            "PDF link is not an http(s) URL.".to_string(),
        ));
    }
    if parsed.netloc.contains('@') {
        let (userinfo, _) = parsed.netloc.rsplit_once('@').unwrap();
        if !userinfo.is_empty() {
            return Err(PdfError::Rejected(
                "PDF link carries embedded credentials.".to_string(),
            ));
        }
    }
    let host = parsed
        .netloc
        .rsplit_once('@')
        .map(|(_, h)| h.to_string())
        .unwrap_or_else(|| parsed.netloc.clone());
    // Strip port for host checks.
    let host = strip_port(&host);
    if host.is_empty() {
        return Err(PdfError::Rejected("PDF link has no host.".to_string()));
    }

    if parse_address(&host).is_some() {
        return require_public_address(&host);
    }

    let addresses = resolver.resolve(&host).map_err(|_| {
        PdfError::Unverified("Could not resolve the PDF link host: OSError".to_string())
    })?;
    if addresses.is_empty() {
        return Err(PdfError::Unverified(
            "PDF link host resolved to no addresses.".to_string(),
        ));
    }
    for address in &addresses {
        require_public_address(address)?;
    }
    Ok(())
}

fn strip_port(host: &str) -> String {
    if host.starts_with('[') {
        // [::1]:8080
        if let Some(end) = host.find(']') {
            return host[1..end].to_string();
        }
        return host.trim_matches(['[', ']']).to_string();
    }
    match host.rfind(':') {
        Some(pos)
            if host[pos + 1..].chars().all(|c| c.is_ascii_digit())
                && !host[pos + 1..].is_empty() =>
        {
            host[..pos].to_string()
        }
        _ => host.to_string(),
    }
}

/// `_require_public_address`: global, non-multicast addresses only.
fn require_public_address(address: &str) -> Result<(), PdfError> {
    let Some(parsed) = parse_address(address) else {
        return Err(PdfError::Unverified(format!(
            "Unrecognised address for the PDF link host: {address}"
        )));
    };
    if !parsed.is_global() || parsed.is_multicast() {
        return Err(PdfError::Rejected(
            "PDF link resolves to a local, private, link-local, multicast or reserved address."
                .to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IpAddress {
    V4([u8; 4]),
    V6([u16; 8]),
}

impl IpAddress {
    fn is_multicast(&self) -> bool {
        match self {
            Self::V4(o) => o[0] & 0xf0 == 0xe0,
            Self::V6(o) => o[0] & 0xff00 == 0xff00,
        }
    }

    /// Python `ipaddress.is_global` semantics for the classes the SSRF guard
    /// must reject: loopback, private, link-local, CGNAT, TEST-NETs,
    /// reserved, multicast, unspecified.
    fn is_global(&self) -> bool {
        match self {
            Self::V4(o) => {
                let ip = u32::from_be_bytes([o[0], o[1], o[2], o[3]]);
                let in_range = |net: u32, bits: u32| {
                    let mask = if bits == 0 {
                        0
                    } else {
                        u32::MAX << (32 - bits)
                    };
                    ip & mask == net & mask
                };
                let private = in_range(0x0000_0000, 8)
                    || in_range(0x0A00_0000, 8)
                    || in_range(0x7F00_0000, 8)
                    || in_range(0xA9FE_0000, 16)
                    || in_range(0xAC10_0000, 12)
                    || in_range(0xC000_0000, 29)
                    || in_range(0xC000_0200, 24)
                    || in_range(0xC0A8_0000, 16)
                    || in_range(0xC612_0000, 15)
                    || in_range(0xC633_6400, 24)
                    || in_range(0xCB00_7100, 24)
                    || in_range(0xF000_0000, 4);
                let cgnat = in_range(0x6440_0000, 10);
                let multicast = in_range(0xE000_0000, 4);
                let link_local = in_range(0xA9FE_0000, 16);
                !(private || cgnat || multicast || link_local)
            }
            Self::V6(o) => {
                let is_unspecified = *o == [0; 8];
                let is_loopback = *o == [0, 0, 0, 0, 0, 0, 0, 1];
                let first = o[0];
                let unique_local = first & 0xfe00 == 0xfc00;
                let link_local = first & 0xffc0 == 0xfe80;
                let multicast = first & 0xff00 == 0xff00;
                let v4_mapped = o[..5] == [0, 0, 0, 0, 0xffff]
                    && IpAddress::V4([
                        (o[5] >> 8) as u8,
                        (o[5] & 0xff) as u8,
                        (o[6] >> 8) as u8,
                        (o[6] & 0xff) as u8,
                    ])
                    .is_global();
                !(is_unspecified
                    || is_loopback
                    || unique_local
                    || link_local
                    || multicast
                    || !v4_mapped && false)
                    && !(is_unspecified || is_loopback || unique_local || link_local || multicast)
            }
        }
    }
}

/// `ipaddress.ip_address`: IPv4 then IPv6 literal parsing.
pub fn parse_address(host: &str) -> Option<IpAddress> {
    if let Some(v4) = parse_ipv4(host) {
        return Some(IpAddress::V4(v4));
    }
    parse_ipv6(host).map(IpAddress::V6)
}

fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || part.len() > 3 || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        octets[i] = part.parse::<u8>().ok()?;
    }
    Some(octets)
}

fn parse_ipv6(host: &str) -> Option<[u16; 8]> {
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    if !host.contains(':') {
        return None;
    }
    let (head, tail) = match host.split_once("::") {
        Some((h, t)) => (h, Some(t)),
        None => (host, None),
    };
    let mut groups: Vec<u16> = Vec::new();
    for part in head.split(':').filter(|p| !p.is_empty()) {
        groups.push(u16::from_str_radix(part, 16).ok()?);
    }
    if let Some(tail) = tail {
        let mut tail_groups: Vec<u16> = Vec::new();
        for part in tail.split(':').filter(|p| !p.is_empty()) {
            tail_groups.push(u16::from_str_radix(part, 16).ok()?);
        }
        let head_len = groups.len();
        let tail_len = tail_groups.len();
        if head_len + tail_len > 8 {
            return None;
        }
        groups.extend(std::iter::repeat_n(0u16, 8 - head_len - tail_len));
        groups.extend(tail_groups);
    }
    if groups.len() != 8 {
        return None;
    }
    Some(groups.try_into().unwrap())
}

/// `requests.compat.urljoin` for the redirect targets this service meets.
pub fn urljoin(base: &str, location: &str) -> String {
    let location = location.trim();
    if location.is_empty() {
        return base.to_string();
    }
    if let Some(parsed) = crate::url::split(location) {
        if !parsed.scheme.is_empty() {
            return location.to_string();
        }
    }
    let base_parsed = match crate::url::split(base) {
        Some(p) => p,
        None => return location.to_string(),
    };
    if let Some(rest) = location.strip_prefix("//") {
        return format!("{}://{}", base_parsed.scheme, rest);
    }
    if location.starts_with('/') {
        return format!(
            "{}://{}{}",
            base_parsed.scheme, base_parsed.netloc, location
        );
    }
    // Relative path: resolve against the base directory.
    let base_dir = match base_parsed.path.rfind('/') {
        Some(pos) => &base_parsed.path[..=pos],
        None => "/",
    };
    let tail = format!("{base_dir}{location}");
    format!("{}://{}{}", base_parsed.scheme, base_parsed.netloc, tail)
}

/// Python `urllib.parse.quote(value, safe=...)`: unreserved + safe kept.
pub fn quote_url(value: &str, safe: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let ch = byte as char;
        let unreserved = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '~');
        if unreserved || safe.contains(ch) {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
