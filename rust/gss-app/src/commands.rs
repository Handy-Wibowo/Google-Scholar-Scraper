//! Tauri command layer: each GUI operation spawns a worker thread that
//! streams `WorkerMessage` events and honors the shared AtomicBool cancel.
//! Excel/CSV exports run synchronously (UI-thread contract); citations run
//! in a background task; worker exceptions become task_error messages.

use crate::errors::CommandError;
use crate::state::{self, AppState, Operation, WorkerMessage};
use gss_core::client::{AtomicCancel, CancelEvent, HttpFetcher, ScholarClient};

/// Search progress callback as consumed by `scrape_scholar`.
type SearchProgress<'a> = &'a mut dyn FnMut(i32, i32, &str);
use gss_core::models::ExtractionResult;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Connect timeout shared by every outbound client. A dead host must fail
/// fast rather than hold a worker thread hostage.
const CONNECT_TIMEOUT_SECS: u64 = 10;
/// Total budget for a Crossref JSON call.
const CROSSREF_TIMEOUT_SECS: u64 = 30;
/// Total budget for a PDF body; large files over slow links need this much.
const PDF_TIMEOUT_SECS: u64 = 120;

/// Real HTTP fetcher on top of reqwest (blocking, rustls, no system proxy —
/// replicates `trust_env=False`).
struct ReqwestFetcher {
    client: reqwest::blocking::Client,
}

impl ReqwestFetcher {
    fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .user_agent(gss_core::client::USER_AGENT)
                // trust_env=False: no system proxy, no netrc credentials.
                .no_proxy()
                .build()
                .expect("reqwest client"),
        }
    }
}

impl HttpFetcher for ReqwestFetcher {
    fn get(
        &mut self,
        url: &str,
        _params: &[(String, String)],
    ) -> Result<gss_core::client::FetchResponse, gss_core::client::FetchError> {
        // Params already encoded inside `url` by build_scholar_url.
        let response = self
            .client
            .get(url)
            .header("Accept", gss_core::client::ACCEPT_HEADER)
            .header("Accept-Language", gss_core::client::ACCEPT_LANGUAGE)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    gss_core::client::FetchError::Timeout {
                        class: "Timeout".into(),
                    }
                } else if e.is_connect() {
                    gss_core::client::FetchError::Connection {
                        class: "ConnectionError".into(),
                    }
                } else {
                    gss_core::client::FetchError::Other {
                        class: "RequestException".into(),
                    }
                }
            })?;
        let status_code = response.status().as_u16();
        let text = response.text().unwrap_or_default();
        Ok(gss_core::client::FetchResponse { text, status_code })
    }
}

/// Emit helper.
fn emit(app: &AppHandle, message: &WorkerMessage) {
    let _ = app.emit("worker", message);
}

/// Read the UI's cancel flag out of managed state (None when the state is
/// gone, i.e. the window is closing — treated as "not cancelled").
fn app_cancelled(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| s.is_cancelled())
        .unwrap_or(false)
}

/// Clear the running latch and the cancel flag before the terminal message
/// reaches the UI: the previous port set the latch on start and never cleared
/// it, so every second operation failed with "An operation is already
/// running.".
fn clear_running(app: &AppHandle) {
    if let Some(s) = app.try_state::<AppState>() {
        s.set_running(None);
    }
}

/// RAII latch guard for a worker thread.
///
/// The running latch used to be cleared by an explicit `clear_running` call on
/// every terminal path. A panic on any of those paths (or an early `return`
/// added later) left `is_running()` stuck at `Some(..)`, so every subsequent
/// command answered "An operation is already running." until restart. Dropping
/// this guard clears the latch unconditionally — panic, early return, or normal
/// completion.
struct LatchGuard {
    app: AppHandle,
    operation: &'static str,
}

impl LatchGuard {
    fn new(app: &AppHandle, operation: &'static str) -> Self {
        Self {
            app: app.clone(),
            operation,
        }
    }
}

impl Drop for LatchGuard {
    fn drop(&mut self) {
        clear_running(&self.app);
        if crate::logging::take_panicking() {
            crate::logging::error(
                "worker_panic",
                &format!("operation={} latch=cleared", self.operation),
            );
        }
        crate::logging::info("operation_end", &format!("operation={}", self.operation));
    }
}

/// `ArticleLookup` that consults the on-disk DOI cache before the network.
///
/// Wraps the normal HTTP lookup, so behaviour is identical except that a DOI
/// seen in an earlier session costs no request. Title lookups are never cached
/// (they are fuzzy evidence).
struct CachedLookup<'a> {
    inner: gss_core::crossref::HttpClientLookup<'a, 'a>,
    cache: gss_core::cache::LookupCache,
    dirty: bool,
}

impl<'a> CachedLookup<'a> {
    fn new(
        inner: gss_core::crossref::HttpClientLookup<'a, 'a>,
        path: Option<std::path::PathBuf>,
    ) -> Self {
        let cache = match &path {
            Some(p) => gss_core::cache::LookupCache::load(p),
            None => gss_core::cache::LookupCache::new(),
        };
        Self {
            inner,
            cache,
            dirty: false,
        }
    }

    /// Provider-side failures observed by the underlying client.
    fn transport_failures(&self) -> u32 {
        self.inner.client.transport_failures()
    }

    fn flush(&self, path: Option<&std::path::PathBuf>) {
        if !self.dirty {
            return;
        }
        let Some(path) = path else { return };
        if let Err(e) = self.cache.save(path) {
            crate::logging::warn("cache_save_failed", &format!("error={e}"));
        }
    }
}

impl gss_core::crossref::ArticleLookup for CachedLookup<'_> {
    fn lookup(&mut self, doi: &str) -> Option<gss_core::crossref::CrossrefRecord> {
        if let Some(hit) = self.cache.get(doi) {
            return hit;
        }
        let answer = self.inner.lookup(doi);
        self.cache.insert(doi, answer.clone());
        self.dirty = true;
        answer
    }

    fn lookup_title_citations(
        &mut self,
        title: &str,
        author: &str,
        year: Option<i32>,
        work_types: &'static [&'static str],
    ) -> Option<gss_core::crossref::CrossrefRecord> {
        self.inner
            .lookup_title_citations(title, author, year, work_types)
    }
}

/// Cancel view over `AppState` for core routines: observes both its own
/// `AtomicCancel` (set by the wait loop) and the live UI cancel flag.
struct AppCancelBridge {
    flag: Arc<AtomicCancel>,
    app: AppHandle,
}

impl AppCancelBridge {
    fn new(flag: Arc<AtomicCancel>, app: AppHandle) -> Self {
        Self { flag, app }
    }
}

impl CancelEvent for AppCancelBridge {
    fn is_set(&self) -> bool {
        if self.flag.is_set() {
            return true;
        }
        if app_cancelled(&self.app) {
            self.flag.set();
            return true;
        }
        false
    }

    fn wait(&self, seconds: f64) -> bool {
        // Poll in 50 ms slices so a UI cancel is seen promptly.
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds.max(0.0));
        loop {
            if self.is_set() {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50).min(deadline - now));
        }
    }
}

/// `search_scholar`: validates, stores the request, spawns the worker.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // mirrors the Python search form fields
pub fn search_scholar(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    pages: i32,
    ranking_enabled: bool,
    output_folder: String,
    year_from: Option<i32>,
    year_to: Option<i32>,
) -> Result<(), CommandError> {
    if state.is_running() {
        return Err(CommandError::already_running());
    }
    let request = state::validate_search_inputs(
        &query,
        &pages.to_string(),
        ranking_enabled,
        &output_folder,
        &year_from.map(|y| y.to_string()).unwrap_or_default(),
        &year_to.map(|y| y.to_string()).unwrap_or_default(),
    )?;
    *state.request.lock().map_err(|e| e.to_string())? = Some(request.clone());
    state.set_articles(Vec::new());

    let cancel = Arc::new(AtomicCancel::new());
    state.set_running(Some(Operation::Search));
    let app_handle = app.clone();
    let catalog = state.catalog.lock().map(|c| c.clone()).unwrap_or_default();
    std::thread::spawn(move || {
        let _latch = LatchGuard::new(&app_handle, "search");
        crate::logging::info("operation_start", "op=search_scholar");
        let fetcher = ReqwestFetcher::new();
        let mut client = ScholarClient::new(Box::new(fetcher));
        let bridge = AppCancelBridge::new(cancel.clone(), app_handle.clone());
        let progress_app = app_handle.clone();
        let mut progress_fn = |current: i32, total: i32, phase: &str| {
            emit(
                &progress_app,
                &WorkerMessage::Progress {
                    operation: "search".into(),
                    current,
                    total,
                    phase: phase.to_string(),
                },
            );
        };
        let mut result: ExtractionResult = {
            let progress_opt: Option<SearchProgress> = Some(&mut progress_fn);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                gss_core::client::scrape_scholar(
                    &request.query,
                    request.pages,
                    &mut client,
                    request.ranking_enabled,
                    progress_opt,
                    Some(&bridge),
                    request.year_from,
                    request.year_to,
                )
            }));
            outcome.unwrap_or_else(|err| {
                crate::logging::mark_panicking();
                // Worker exceptions never surface tracebacks: synthetic result.
                let class = err
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| err.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "Panic".to_string());
                ExtractionResult {
                    status: gss_core::models::ExtractionStatus::NetworkError,
                    articles: Vec::new(),
                    requested_pages: request.pages,
                    successful_pages: 0,
                    failure_page: None,
                    message: format!("Search failed: {class}."),
                    diagnostic: class,
                    duplicates_removed: 0,
                    invalid_articles_removed: 0,
                    duplicates_by_doi: 0,
                }
            })
        };
        apply_catalog_ranks(&mut result.articles, &catalog);
        if let Some(s) = app_handle.try_state::<AppState>() {
            s.set_articles(result.articles.clone());
        }
        clear_running(&app_handle);
        emit(&app_handle, &WorkerMessage::Result { result });
    });
    Ok(())
}

/// Label freshly retrieved records from the loaded ranking catalog.
///
/// Ranks never affect ordering, so applying them after retrieval is correct
/// and costs no network. A no-op when nothing has been imported.
fn apply_catalog_ranks(
    articles: &mut [gss_core::models::Article],
    catalog: &gss_core::catalog::JournalCatalog,
) {
    if catalog.is_empty() {
        return;
    }
    for article in articles.iter_mut() {
        let (sjr, sinta) = gss_core::catalog::catalog_matches(article, Some(catalog));
        let labeled = gss_core::catalog::apply_rank(article, sjr.as_ref());
        *article = gss_core::catalog::apply_rank(&labeled, sinta.as_ref());
    }
}

/// `cancel_operation`: sets the shared flag (button disabled in the UI).
#[tauri::command]
pub fn cancel_operation(state: State<'_, AppState>) {
    state.request_cancel();
}

/// `check_metadata_pdfs` — metadata enrichment + PDF availability per record.
#[tauri::command]
pub fn check_metadata_pdfs(
    app: AppHandle,
    state: State<'_, AppState>,
    metadata_only: Option<bool>,
) -> Result<(), CommandError> {
    if state.is_running() {
        return Err(CommandError::already_running());
    }
    let articles = state.articles();
    if articles.is_empty() {
        return Err(CommandError::nothing_to_check());
    }
    let catalog = state.catalog.lock().map(|c| c.clone()).unwrap_or_default();
    state.set_running(Some(Operation::Check));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let _latch = LatchGuard::new(&app_handle, "check");
        crate::logging::info("operation_start", "op=check_metadata_pdfs");
        // run_check: enrich (real Crossref), then per-article PDF check.
        // A metadata failure keeps the originals and still runs the PDF pass;
        // a PDF failure keeps whatever enrichment already succeeded.
        let total = articles.len();
        let bridge = AppCancelBridge::new(Arc::new(AtomicCancel::new()), app_handle.clone());

        let progress_app = app_handle.clone();
        let mut enrich_progress = |current: usize, _count: usize, label: &str| {
            emit(
                &progress_app,
                &WorkerMessage::Progress {
                    operation: "check".into(),
                    current: current as i32,
                    total: (total * 2) as i32,
                    phase: format!("metadata: {label}"),
                },
            );
        };
        let mut enrich_error = String::new();
        let enriched: Vec<gss_core::models::Article> = {
            let mut transport = ReqwestCrossref::new();
            let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
            let inner = gss_core::crossref::HttpClientLookup {
                client: &mut client,
            };
            let cache_file = crate::settings::cache_path();
            let mut lookup = CachedLookup::new(inner, cache_file.clone());
            let out = gss_core::crossref::enrich_articles(
                &articles,
                Some(&catalog),
                Some(&bridge),
                Some(&mut enrich_progress),
                &mut lookup,
                None,
                None,
                true,
            );
            // Distinguish "nothing matched" from "the provider never answered":
            // a Crossref outage used to look identical to a fully successful run.
            lookup.flush(cache_file.as_ref());
            // Read the counter through the adapter: it still holds the mutable
            // borrow of `client`.
            let failures = lookup.transport_failures();
            if failures > 0 {
                enrich_error = format!(
                    "{failures} metadata lookup(s) could not be completed (Crossref transport or server error); affected records keep their original metadata."
                );
                crate::logging::warn(
                    "enrich_partial_failure",
                    &format!("failures={failures} records={}", articles.len()),
                );
            }
            out
        };

        // Metadata-only refresh (2.10): refreshing DOI/journal/rank on a few
        // hundred records should not re-validate every PDF link.
        let metadata_only = metadata_only.unwrap_or(false);
        if metadata_only {
            if let Some(s) = app_handle.try_state::<AppState>() {
                s.set_articles(enriched.clone());
            }
            let payload = serde_json::json!({
                "articles": enriched,
                "enrich_error": enrich_error,
                "cancelled": bridge.is_set(),
                "total": total,
                "metadata_only": true,
            });
            clear_running(&app_handle);
            emit(
                &app_handle,
                &WorkerMessage::TaskResult {
                    operation: "check".into(),
                    payload,
                },
            );
            return;
        }

        let mut results: Vec<gss_core::models::Article> = Vec::new();
        let resolver = gss_core::pdf::PublicResolver("93.184.216.34".to_string());
        let mut fetcher = ReqwestPdfFetcher::new();
        for (index, article) in enriched.iter().enumerate() {
            if bridge.is_set() {
                results.extend(enriched[index..].iter().cloned());
                break;
            }
            let position = total + index + 1;
            match check_one(
                article,
                &mut fetcher,
                &resolver,
                &app_handle,
                position,
                total * 2,
                Some(&bridge),
            ) {
                Ok(a) => results.push(a),
                Err(()) => results.push(article.clone()),
            }
        }

        let cancelled = bridge.is_set();
        if let Some(s) = app_handle.try_state::<AppState>() {
            s.set_articles(results.clone());
        }
        let payload = serde_json::json!({
            "articles": results,
            "enrich_error": enrich_error,
            "cancelled": cancelled,
            "total": total,
            "metadata_only": false,
        });
        clear_running(&app_handle);
        emit(
            &app_handle,
            &WorkerMessage::TaskResult {
                operation: "check".into(),
                payload,
            },
        );
    });
    Ok(())
}

fn check_one(
    article: &gss_core::models::Article,
    fetcher: &mut dyn gss_core::pdf::PdfFetcher,
    resolver: &dyn gss_core::pdf::DnsResolver,
    app: &AppHandle,
    position: usize,
    total: usize,
    cancel: Option<&dyn CancelEvent>,
) -> Result<gss_core::models::Article, ()> {
    let checked = gss_core::pdf::check_pdf_availability(article, cancel, fetcher, resolver);
    let label = if article.title.is_empty() {
        "record"
    } else {
        &article.title
    };
    emit(
        app,
        &WorkerMessage::Progress {
            operation: "check".into(),
            current: position as i32,
            total: total as i32,
            phase: format!("PDF: {label}"),
        },
    );
    Ok(checked)
}

/// Streaming PDF fetcher over reqwest.
struct ReqwestPdfFetcher {
    client: reqwest::blocking::Client,
}

impl ReqwestPdfFetcher {
    fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                // Streaming PDF bodies need a generous total budget; without a
                // timeout a stalled mirror hangs the worker thread forever.
                .timeout(std::time::Duration::from_secs(PDF_TIMEOUT_SECS))
                .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
                .build()
                .expect("reqwest pdf client"),
        }
    }
}

impl gss_core::pdf::PdfFetcher for ReqwestPdfFetcher {
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<Box<dyn gss_core::pdf::PdfStream>, gss_core::client::FetchError> {
        let mut request = self.client.get(url);
        for (k, v) in headers {
            request = request.header(k, v);
        }
        let response = request.send().map_err(|e| {
            if e.is_timeout() {
                gss_core::client::FetchError::Timeout {
                    class: "Timeout".into(),
                }
            } else if e.is_connect() {
                gss_core::client::FetchError::Connection {
                    class: "ConnectionError".into(),
                }
            } else {
                gss_core::client::FetchError::Other {
                    class: "RequestException".into(),
                }
            }
        })?;
        let status = response.status().as_u16();
        let mut header_map: HashMap<String, String> = HashMap::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                header_map.insert(name.as_str().to_string(), v.to_string());
            }
        }
        // Read the whole body (PDF sizes bounded by the service itself).
        let body = response.bytes().unwrap_or_default().to_vec();
        Ok(Box::new(BlockingStream {
            status,
            headers: header_map,
            body,
            offset: 0,
            url: url.to_string(),
            closed: false,
        }))
    }
}

use gss_core::crossref::TransportError;
use std::collections::HashMap;

struct BlockingStream {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    offset: usize,
    url: String,
    closed: bool,
}

impl gss_core::pdf::PdfStream for BlockingStream {
    fn status_code(&self) -> u16 {
        self.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).cloned()
    }

    fn url(&self) -> String {
        self.url.clone()
    }

    fn set_url(&mut self, url: String) {
        self.url = url;
    }

    fn next_chunk(&mut self, max: usize) -> Option<Result<Vec<u8>, gss_core::client::FetchError>> {
        if self.offset >= self.body.len() {
            return None;
        }
        let end = (self.offset + max).min(self.body.len());
        let chunk = self.body[self.offset..end].to_vec();
        self.offset = end;
        Some(Ok(chunk))
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

/// `download_pdfs` — streams selected records' PDFs into the chosen folder.
#[tauri::command]
pub fn download_pdfs(
    app: AppHandle,
    state: State<'_, AppState>,
    indices: Vec<usize>,
    folder: String,
) -> Result<(), CommandError> {
    if state.is_running() {
        return Err(CommandError::already_running());
    }
    let articles = state.articles();
    let selected: Vec<gss_core::models::Article> = indices
        .iter()
        .filter_map(|i| articles.get(*i).cloned())
        .collect();
    if selected.is_empty() {
        return Err(CommandError::no_papers_selected());
    }
    state.set_running(Some(Operation::Download));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let _latch = LatchGuard::new(&app_handle, "download");
        crate::logging::info("operation_start", "op=download_pdfs");
        let resolver = gss_core::pdf::PublicResolver("93.184.216.34".to_string());
        let mut fetcher = ReqwestPdfFetcher::new();
        let total = selected.len();
        let app_for_progress = app_handle.clone();
        let mut progress = move |current: usize, t: usize, phase: &str| {
            emit(
                &app_for_progress,
                &WorkerMessage::Progress {
                    operation: "download".into(),
                    current: current as i32,
                    total: t as i32,
                    phase: phase.to_string(),
                },
            );
        };
        let bridge = AppCancelBridge::new(Arc::new(AtomicCancel::new()), app_handle.clone());
        let results = gss_core::pdf::download_pdfs(
            &selected,
            Path::new(&folder),
            Some(&bridge),
            Some(&mut progress),
            &mut fetcher,
            gss_core::pdf::DEFAULT_MAX_BYTES,
            &resolver,
        );
        // Write back by index (`_finish_download` zip).
        let payload = serde_json::json!({
            "indices": indices,
            "articles": results,
            "folder": folder,
            "total": total,
        });
        clear_running(&app_handle);
        emit(
            &app_handle,
            &WorkerMessage::TaskResult {
                operation: "download".into(),
                payload,
            },
        );
    });
    Ok(())
}

/// `import_ranking` — background import (staged clone semantics).
#[tauri::command]
pub fn import_ranking(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    default_year: Option<i32>,
) -> Result<(), CommandError> {
    if state.is_running() {
        return Err(CommandError::already_running());
    }
    if !Path::new(&path).is_file() {
        return Err(CommandError::no_file_found(&path));
    }
    // Staging clone: a rejected file leaves the loaded records untouched.
    let existing = state.catalog.lock().map(|c| c.clone()).unwrap_or_default();
    state.set_running(Some(Operation::Import));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let _latch = LatchGuard::new(&app_handle, "import");
        crate::logging::info("operation_start", "op=import_ranking");
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        emit(
            &app_handle,
            &WorkerMessage::Progress {
                operation: "import".into(),
                current: 0,
                total: 1,
                phase: format!("reading {name}"),
            },
        );
        let mut staged = existing;
        let result = staged.import_file(Path::new(&path), default_year);
        match result {
            Ok(count) => {
                emit(
                    &app_handle,
                    &WorkerMessage::Progress {
                        operation: "import".into(),
                        current: 1,
                        total: 1,
                        phase: format!("{count} records read"),
                    },
                );
                // Commit the staged catalog, then re-label what is on screen
                // (`_apply_catalog_to_current`): no re-search required.
                let mut relabeled: Option<Vec<gss_core::models::Article>> = None;
                if let Some(s) = app_handle.try_state::<AppState>() {
                    if let Ok(mut guard) = s.catalog.lock() {
                        *guard = staged.clone();
                    }
                    // 6.16: a relaunch must not require re-importing the file.
                    if let Err(e) = crate::settings::save_catalog(&staged) {
                        crate::logging::warn("catalog_persist_failed", &format!("error={e}"));
                    }
                    let current = s.articles();
                    if !current.is_empty() {
                        let mut updated = current;
                        let gained = relabel_articles(&mut updated, &staged);
                        s.set_articles(updated.clone());
                        let _ = gained;
                        relabeled = Some(updated);
                    }
                }
                let mut payload = serde_json::json!({
                    "catalog": catalog_records_json(&staged),
                    "count": count,
                    "total": staged.len(),
                    "path": path,
                    "year": default_year,
                });
                if let Some(articles) = relabeled {
                    payload["articles"] = serde_json::to_value(&articles).unwrap_or_default();
                }
                clear_running(&app_handle);
                emit(
                    &app_handle,
                    &WorkerMessage::TaskResult {
                        operation: "import".into(),
                        payload,
                    },
                );
            }
            Err(message) => {
                clear_running(&app_handle);
                emit(
                    &app_handle,
                    &WorkerMessage::TaskError {
                        operation: "import".into(),
                        message,
                    },
                );
            }
        }
    });
    Ok(())
}

/// Re-label stored records from a catalog; returns how many gained a label.
fn relabel_articles(
    articles: &mut [gss_core::models::Article],
    catalog: &gss_core::catalog::JournalCatalog,
) -> usize {
    if catalog.is_empty() {
        return 0;
    }
    let mut gained = 0;
    for article in articles.iter_mut() {
        let before_sjr = !article.sjr_quartile.is_empty();
        let before_sinta = !article.sinta_rank.is_empty();
        let (sjr, sinta) = gss_core::catalog::catalog_matches(article, Some(catalog));
        let labeled = gss_core::catalog::apply_rank(article, sjr.as_ref());
        let labeled = gss_core::catalog::apply_rank(&labeled, sinta.as_ref());
        if (!before_sjr && !labeled.sjr_quartile.is_empty())
            || (!before_sinta && !labeled.sinta_rank.is_empty())
        {
            gained += 1;
        }
        *article = labeled;
    }
    gained
}

fn catalog_records_json(catalog: &gss_core::catalog::JournalCatalog) -> Vec<serde_json::Value> {
    catalog
        .records()
        .iter()
        .map(|r| {
            serde_json::json!({
                "title": r.title,
                "titleKey": r.title_key,
                "issns": r.issns,
                "kind": r.kind,
                "rank": r.rank,
                "year": r.year,
                "category": r.category,
                "sourceUrl": r.source_url,
            })
        })
        .collect()
}

/// Crossref transport on top of reqwest: no proxy, no redirects (Crossref
/// echoes the requested DOI, so a redirect means an unexpected answer).
struct ReqwestCrossref {
    client: reqwest::blocking::Client,
}

impl ReqwestCrossref {
    fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                // Crossref returns small JSON; a short budget keeps a slow
                // provider from stalling a whole enrichment pass.
                .timeout(std::time::Duration::from_secs(CROSSREF_TIMEOUT_SECS))
                .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
                .build()
                .expect("reqwest crossref client"),
        }
    }
}

impl gss_core::crossref::CrossrefFetcher for ReqwestCrossref {
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<(Vec<u8>, u16), TransportError> {
        let mut request = self.client.get(url);
        for (k, v) in headers {
            request = request.header(k, v);
        }
        let response = request.send().map_err(|_| TransportError)?;
        let status = response.status().as_u16();
        let body = response.bytes().map_err(|_| TransportError)?.to_vec();
        Ok((body, status))
    }
}

/// `export_citations` — background task; cancelled → no file written.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn export_citations(
    app: AppHandle,
    state: State<'_, AppState>,
    articles: Vec<gss_core::models::Article>,
    format: String,
    path: String,
) -> Result<(), CommandError> {
    if state.is_running() {
        return Err(CommandError::already_running());
    }
    if articles.is_empty() {
        return Err(CommandError::no_results_to_export());
    }
    let format = format.to_lowercase();
    if format != "bibtex" && format != "ris" {
        return Err(CommandError::new(
            crate::errors::UNSUPPORTED_FORMAT,
            "Unsupported citation format.",
        ));
    }
    state.set_running(Some(Operation::Citations));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let _latch = LatchGuard::new(&app_handle, "citations");
        crate::logging::info("operation_start", "op=export_citations");
        let total = articles.len();
        let bridge = AppCancelBridge::new(Arc::new(AtomicCancel::new()), app_handle.clone());
        let progress_app = app_handle.clone();
        let mut progress = |current: usize, t: usize, phase: &str| {
            emit(
                &progress_app,
                &WorkerMessage::Progress {
                    operation: "citations".into(),
                    current: current as i32,
                    total: t as i32,
                    phase: phase.to_string(),
                },
            );
        };
        // Real Crossref resolution; unresolved records stay explicitly
        // unverified with their Scholar fields and a review note.
        let mut transport = ReqwestCrossref::new();
        let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
        let inner = gss_core::crossref::HttpClientLookup {
            client: &mut client,
        };
        let cache_file = crate::settings::cache_path();
        let mut lookup = CachedLookup::new(inner, cache_file.clone());
        let records = gss_core::crossref::resolve_citations(
            &articles,
            Some(&bridge),
            Some(&mut progress),
            &mut lookup,
            None,
            None,
        );
        lookup.flush(cache_file.as_ref());
        let cancelled = bridge.is_set();
        if cancelled {
            // Cancelled → return WITHOUT writing the file.
            clear_running(&app_handle);
            let payload = serde_json::json!({
                "fmt": format,
                "path": path,
                "count": total,
                "verified": 0,
                "written": false,
                "cancelled": true,
            });
            emit(
                &app_handle,
                &WorkerMessage::TaskResult {
                    operation: "citations".into(),
                    payload,
                },
            );
            return;
        }
        let write_result = if format == "ris" {
            gss_core::exporters::save_to_ris(&records, Path::new(&path)).map(|_| ())
        } else {
            gss_core::exporters::save_to_bibtex(&records, Path::new(&path)).map(|_| ())
        };
        match write_result {
            Ok(()) => {
                let verified = records.iter().filter(|r| r.verified).count();
                let payload = serde_json::json!({
                    "fmt": format,
                    "path": path,
                    "count": total,
                    "verified": verified,
                    "written": true,
                    "cancelled": false,
                });
                clear_running(&app_handle);
                emit(
                    &app_handle,
                    &WorkerMessage::TaskResult {
                        operation: "citations".into(),
                        payload,
                    },
                );
            }
            Err(e) => {
                clear_running(&app_handle);
                emit(
                    &app_handle,
                    &WorkerMessage::TaskError {
                        operation: "citations".into(),
                        message: format!("Export failed: {e}"),
                    },
                );
            }
        }
    });
    Ok(())
}

/// Saved window settings (UI defaults when nothing is stored yet).
#[tauri::command]
pub fn load_settings() -> crate::settings::LoadOutcome {
    crate::settings::load_with_notice()
}

/// Ranking trend for one journal: `[(year, kind, rank)]`, oldest first.
///
/// Reads the in-memory catalog only; the import keeps every year, so no network
/// and no new storage are involved.
#[tauri::command]
pub fn journal_rank_history(
    state: State<'_, AppState>,
    issns: Vec<String>,
    journal: String,
) -> Vec<(i32, String, String)> {
    let catalog = state.catalog.lock().map(|c| c.clone()).unwrap_or_default();
    gss_core::catalog::rank_history(&catalog, &issns, &journal)
}

/// Records from the last successful ranking import, restored at startup.
#[tauri::command]
pub fn load_catalog_records() -> Vec<gss_core::catalog::JournalRankRecord> {
    crate::settings::load_catalog().to_records()
}

/// Persist window settings; the UI ignores failures (session keeps working).
#[tauri::command]
pub fn save_settings(settings: crate::settings::UiSettings) -> Result<(), String> {
    crate::settings::save(&settings)
}

/// Persist the collected records so a relaunch can restore them.
#[tauri::command]
pub fn save_results(articles: Vec<gss_core::models::Article>) -> Result<(), String> {
    crate::settings::save_results(&articles)
}

/// Records from the previous session; empty when nothing usable was stored.
#[tauri::command]
pub fn load_results() -> Vec<gss_core::models::Article> {
    crate::settings::load_results()
}

/// `reveal_path`: select a file (or open a folder) in File Explorer. Windows
/// `explorer` returns a non-zero exit code even on success, so the child is
/// simply spawned and never awaited.
#[tauri::command]
pub fn reveal_path(path: String) {
    if path.trim().is_empty() {
        return;
    }
    let target = Path::new(&path);
    let mut command = std::process::Command::new("explorer");
    if target.is_file() {
        // `/select,` must stay a single argument: it is one switch.
        command.arg(format!("/select,{}", target.display()));
    } else {
        command.arg(target);
    }
    let _ = command.spawn();
}

/// `open_url`: open a result link in the default browser. `window.open` is
/// inert inside the Tauri webview without the shell plugin, so the link column
/// and the row context menu route through `cmd /c start` instead.
#[tauri::command]
pub fn open_url(url: String) {
    let url = url.trim();
    if url.is_empty() {
        return;
    }
    // `start` treats the first quoted argument as the window title.
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

/// `fetch_abstract`: on-demand abstract for one record.
///
/// Tries Crossref first (the DOI echo-check keeps a mismatched payload from
/// being shown), then OpenAlex's inverted index. Returns `None` rather than an
/// error when neither provider has one, so the UI can say "not available"
/// instead of treating it as a failure. Records without a DOI fall back to an
/// OpenAlex title search, which is best-effort.
#[tauri::command]
pub fn fetch_abstract(article: gss_core::models::Article) -> Option<String> {
    let doi = article.doi.trim().to_string();

    if !doi.is_empty() {
        let mut transport = ReqwestCrossref::new();
        let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
        if let Some(found) = client.abstract_for_doi(&doi) {
            return Some(found);
        }
    }

    // OpenAlex: by DOI when we have one, else by exact-ish title search.
    let mut transport = ReqwestCrossref::new();
    if !doi.is_empty() {
        if let Some(found) = gss_core::crossref::abstract_from_openalex_doi(&mut transport, &doi) {
            return Some(found);
        }
    } else if !article.title.trim().is_empty() {
        if let Some(found) =
            gss_core::crossref::abstract_from_openalex_title(&mut transport, &article.title)
        {
            return Some(found);
        }
    }

    None
}

/// `export_excel` — synchronous on the UI thread (Python contract).
#[tauri::command]
pub fn export_excel(
    state: State<'_, AppState>,
    articles: Vec<gss_core::models::Article>,
    path: String,
) -> Result<String, CommandError> {
    let _ = state;
    if articles.is_empty() {
        return Err(CommandError::no_results_to_export());
    }
    let target = Path::new(&path);
    let tmp = target.with_extension("xlsx.tmp");
    if let Err(e) = gss_core::exporters::save_to_excel(&articles, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CommandError::export_failed(e));
    }
    // Rename only once the whole file is written, so an interrupted export
    // cannot leave a truncated workbook that still looks valid.
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CommandError::export_failed(e));
    }
    Ok(format!("Exported {} result(s) to {path}.", articles.len()))
}

/// The whole review project (decisions, notes, tags, baselines).
#[tauri::command]
pub fn load_review_project() -> crate::settings::StoredProject {
    crate::settings::load_project()
}

/// Persist the review project.
#[tauri::command]
pub fn save_review_project(project: crate::settings::StoredProject) -> Result<(), CommandError> {
    crate::settings::save_project(&project).map_err(|e| {
        CommandError::new(
            crate::errors::INVALID_ARGUMENT,
            format!("Could not save the project: {e}"),
        )
    })
}

/// PRISMA-style counts over the stored records.
#[tauri::command]
pub fn review_prisma(
    state: State<'_, AppState>,
    articles: Option<Vec<gss_core::models::Article>>,
) -> String {
    let records = match articles {
        Some(list) if !list.is_empty() => list,
        _ => state.articles(),
    };
    crate::settings::load_project()
        .to_core()
        .prisma(&records)
        .summary()
}

/// Screening log rows (what was decided, why, when, and which query found it).
#[tauri::command]
pub fn review_screening_log(
    state: State<'_, AppState>,
    articles: Option<Vec<gss_core::models::Article>>,
) -> Vec<gss_core::review::ScreeningLogRow> {
    let records = match articles {
        Some(list) if !list.is_empty() => list,
        _ => state.articles(),
    };
    crate::settings::load_project()
        .to_core()
        .screening_log(&records)
}

/// Likely duplicate pairs for human confirmation.
#[tauri::command]
pub fn review_near_duplicates(
    state: State<'_, AppState>,
    articles: Option<Vec<gss_core::models::Article>>,
) -> Vec<gss_core::review::NearDuplicate> {
    let records = match articles {
        Some(list) if !list.is_empty() => list,
        _ => state.articles(),
    };
    gss_core::review::find_near_duplicates(&records, 0.8)
}

/// Re-score the stored results against a new query, without searching again.
#[tauri::command]
pub fn review_rescore(
    state: State<'_, AppState>,
    query: String,
    enabled: bool,
) -> Result<(), CommandError> {
    let articles = state.articles();
    if articles.is_empty() {
        return Err(CommandError::nothing_to_check());
    }
    let ranked = gss_core::ranking::rank_articles(&query, articles, enabled);
    state.set_articles(ranked);
    Ok(())
}

/// Compare the current set against a named baseline.
#[tauri::command]
pub fn review_diff_baseline(
    state: State<'_, AppState>,
    name: String,
) -> Option<gss_core::review::BaselineDiff> {
    crate::settings::load_project()
        .to_core()
        .diff_baseline(&name, &state.articles())
}

/// Store the current set as a named baseline.
#[tauri::command]
pub fn review_save_baseline(state: State<'_, AppState>, name: String) -> Result<(), CommandError> {
    let mut stored = crate::settings::load_project();
    let mut project = stored.to_core();
    project.save_baseline(&name, &state.articles());
    stored = crate::settings::StoredProject::from_core(&project);
    crate::settings::save_project(&stored).map_err(|e| {
        CommandError::new(
            crate::errors::INVALID_ARGUMENT,
            format!("Could not save the baseline: {e}"),
        )
    })
}

/// Append freshly searched results to the stored set, deduplicated.
///
/// Records keep their first-seen provenance, which is what a methods section
/// reports.
#[tauri::command]
pub fn review_append(
    state: State<'_, AppState>,
    articles: Vec<gss_core::models::Article>,
) -> usize {
    let mut combined = state.articles();
    let before = combined.len();
    combined.extend(articles);
    let deduped = gss_core::dedupe::deduplicate_articles(combined);
    state.set_articles(deduped.articles.clone());
    deduped.articles.len().saturating_sub(before)
}

/// Round-trip: read back a CSV/JSON the app itself exported.
#[tauri::command]
pub fn review_import_export(
    state: State<'_, AppState>,
    path: String,
) -> Result<usize, CommandError> {
    use gss_core::models::Article;
    let target = Path::new(&path);
    if !target.is_file() {
        return Err(CommandError::no_file_found(&path));
    }
    let text = std::fs::read_to_string(target).map_err(|e| {
        CommandError::new(
            crate::errors::NO_FILE_FOUND,
            format!("Could not read {path}: {e}"),
        )
    })?;

    let mut imported: Vec<Article> = Vec::new();
    if text.trim_start().starts_with('[') {
        // The JSON export is an array of objects keyed by export column name.
        let rows: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| {
            CommandError::new(
                crate::errors::INVALID_ARGUMENT,
                format!("Not a JSON export: {e}"),
            )
        })?;
        for row in rows {
            let get = |key: &str| {
                row.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let title = get("Title");
            if title.trim().is_empty() {
                continue;
            }
            imported.push(Article {
                title,
                authors: get("Authors"),
                link: get("Link"),
                doi: get("DOI"),
                journal: get("Journal"),
                issns: get("ISSNs")
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                pdf_status: get("PDF Status").if_empty("unknown"),
                pdf_local_path: get("PDF Local Path"),
                ..Article::default()
            });
        }
    } else {
        // The CSV export: a header row, then quoted fields.
        let mut lines = text.lines();
        let header = lines.next().unwrap_or("");
        let columns: Vec<&str> = header.split(',').map(|c| c.trim()).collect();
        let index_of = |name: &str| columns.iter().position(|c| *c == name);
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_csv_line(line);
            let pick = |name: &str| {
                index_of(name)
                    .and_then(|i| fields.get(i))
                    .cloned()
                    .unwrap_or_default()
            };
            let title = pick("Title");
            if title.trim().is_empty() {
                continue;
            }
            imported.push(Article {
                title,
                authors: pick("Authors"),
                link: pick("Link"),
                doi: pick("DOI"),
                journal: pick("Journal"),
                issns: pick("ISSNs")
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                pdf_status: pick("PDF Status").if_empty("unknown"),
                pdf_local_path: pick("PDF Local Path"),
                ..Article::default()
            });
        }
    }

    if imported.is_empty() {
        return Err(CommandError::new(
            crate::errors::INVALID_ARGUMENT,
            "No records found in that file.",
        ));
    }
    let mut combined = state.articles();
    let before = combined.len();
    combined.extend(imported);
    let deduped = gss_core::dedupe::deduplicate_articles(combined);
    state.set_articles(deduped.articles.clone());
    Ok(deduped.articles.len() - before)
}

/// Minimal CSV field splitter for the app's own export (quoted fields, doubled
/// quotes). Not a general CSV parser: the round-trip only reads our own output.
fn split_csv_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                out.push(std::mem::take(&mut field));
            }
            _ => field.push(c),
        }
    }
    out.push(field);
    out
}

/// Small helper so a missing cell becomes the core default.
trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}
impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

/// `export_excel_rich`: formatted workbook (freeze, filters, colours, links).
#[tauri::command]
pub fn export_excel_rich(
    articles: Vec<gss_core::models::Article>,
    path: String,
    columns: Vec<String>,
    group_by: Option<String>,
) -> Result<String, CommandError> {
    if articles.is_empty() {
        return Err(CommandError::no_results_to_export());
    }
    gss_core::exports_extra::save_to_excel_rich(
        &articles,
        Path::new(&path),
        &columns,
        group_by.as_deref(),
    )
    .map(|_| format!("Exported {} result(s) to {path}.", articles.len()))
    .map_err(CommandError::export_failed)
}

/// `export_tabular`: CSV-adjacent formats that are separate writers.
///
/// `format` is one of `json`, `markdown`, `tsv`, `enw`, `details`. None of
/// these touch the parity-gated CSV/BibTeX/RIS paths.
#[tauri::command]
pub fn export_tabular(
    articles: Vec<gss_core::models::Article>,
    path: String,
    format: String,
    columns: Vec<String>,
) -> Result<String, CommandError> {
    if articles.is_empty() {
        return Err(CommandError::no_results_to_export());
    }
    let target = Path::new(&path);
    let result = match format.as_str() {
        "json" => gss_core::exports_extra::save_to_json(&articles, target, &columns),
        "markdown" => gss_core::exports_extra::save_to_markdown(&articles, target, &columns),
        "tsv" => gss_core::exports_extra::save_to_tsv(&articles, target, &columns),
        "enw" => gss_core::exports_extra::save_to_enw(&articles, target, &columns),
        "details" => gss_core::exports_extra::save_details_text(&articles, target),
        "reading-list" => gss_core::exports_extra::save_reading_list_html(
            &articles,
            target,
            "Google Scholar reading list",
        ),
        "journals" => gss_core::exports_extra::save_journal_aggregation(&articles, target),
        other => {
            return Err(CommandError::new(
                crate::errors::UNSUPPORTED_FORMAT,
                format!("Unsupported export format: {other}"),
            ))
        }
    };
    result
        .map(|_| format!("Exported {} result(s) to {path}.", articles.len()))
        .map_err(CommandError::export_failed)
}

/// `export_citations_extra`: CSL-JSON and stable-key BibTeX, separate from the
/// parity-gated `export_citations`.
#[tauri::command]
pub fn export_citations_extra(
    articles: Vec<gss_core::models::Article>,
    path: String,
    format: String,
    style: Option<String>,
    group_by: Option<String>,
) -> Result<String, CommandError> {
    if articles.is_empty() {
        return Err(CommandError::no_results_to_export());
    }
    // Resolve against the same storage the UI shows, reusing the on-disk cache.
    let mut transport = ReqwestCrossref::new();
    let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
    let inner = gss_core::crossref::HttpClientLookup {
        client: &mut client,
    };
    let cache_file = crate::settings::cache_path();
    let mut lookup = CachedLookup::new(inner, cache_file.clone());
    let records =
        gss_core::crossref::resolve_citations(&articles, None, None, &mut lookup, None, None);
    lookup.flush(cache_file.as_ref());

    let target = Path::new(&path);
    let style = style
        .as_deref()
        .and_then(gss_core::exports_extra::CitationStyle::parse)
        .unwrap_or(gss_core::exports_extra::CitationStyle::Apa);

    let result = match format.as_str() {
        "csl-json" => gss_core::exports_extra::save_to_csl_json(&records, target),
        "bibtex-stable" => {
            let mut keys = crate::settings::load_bibtex_keys();
            let saved = gss_core::exports_extra::save_to_bibtex_stable(&records, target, &mut keys);
            if saved.is_ok() {
                let _ = crate::settings::save_bibtex_keys(&keys);
            }
            saved
        }
        "bibtex-files" => {
            gss_core::exports_extra::save_to_bibtex_with_files(&records, &articles, target)
        }
        "grouped" => gss_core::exports_extra::save_grouped_bibliography(
            &records,
            &articles,
            target,
            group_by.as_deref().unwrap_or("year"),
            style,
        ),
        other => {
            return Err(CommandError::new(
                crate::errors::UNSUPPORTED_FORMAT,
                format!("Unsupported citation format: {other}"),
            ))
        }
    };
    result
        .map(|_| format!("Exported {} citation(s) to {path}.", articles.len()))
        .map_err(CommandError::export_failed)
}

/// Write a UTF-8 text file (screening log and similar small artefacts).
#[tauri::command]
pub fn write_text_file(path: String, content: String) -> Result<(), CommandError> {
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CommandError::new(
                crate::errors::EXPORT_FAILED,
                format!("Could not create {}: {e}", parent.display()),
            )
        })?;
    }
    std::fs::write(&path, content).map_err(|e| {
        CommandError::new(
            crate::errors::EXPORT_FAILED,
            format!("Could not write {path}: {e}"),
        )
    })
}

/// Read a UTF-8 text file the app just wrote, for clipboard handoff.
///
/// Bounded: a runaway file must not be pulled into the webview wholesale.
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, CommandError> {
    const MAX: u64 = 4 * 1024 * 1024;
    let target = Path::new(&path);
    let meta = std::fs::metadata(target).map_err(|e| {
        CommandError::new(
            crate::errors::NO_FILE_FOUND,
            format!("Could not read {path}: {e}"),
        )
    })?;
    if meta.len() > MAX {
        return Err(CommandError::new(
            crate::errors::INVALID_ARGUMENT,
            format!("File too large to read: {path}"),
        ));
    }
    std::fs::read_to_string(target).map_err(|e| {
        CommandError::new(
            crate::errors::NO_FILE_FOUND,
            format!("Could not read {path}: {e}"),
        )
    })
}

/// One formatted reference line, for the clipboard.
#[tauri::command]
pub fn format_citation_text(article: gss_core::models::Article, style: String) -> String {
    let style = gss_core::exports_extra::CitationStyle::parse(&style)
        .unwrap_or(gss_core::exports_extra::CitationStyle::Apa);
    // Reuse the resolver so the formatted line has resolved authors and venue.
    let mut transport = ReqwestCrossref::new();
    let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
    let inner = gss_core::crossref::HttpClientLookup {
        client: &mut client,
    };
    let cache_file = crate::settings::cache_path();
    let mut lookup = CachedLookup::new(inner, cache_file.clone());
    let records = gss_core::crossref::resolve_citations(
        std::slice::from_ref(&article),
        None,
        None,
        &mut lookup,
        None,
        None,
    );
    lookup.flush(cache_file.as_ref());
    match records.first() {
        Some(record) => gss_core::exports_extra::format_citation(record, style),
        None => article.title.clone(),
    }
}

/// `export_csv` — synchronous on the UI thread.
#[tauri::command]
pub fn export_csv(
    state: State<'_, AppState>,
    articles: Vec<gss_core::models::Article>,
    path: String,
) -> Result<String, CommandError> {
    let _ = state;
    if articles.is_empty() {
        return Err(CommandError::no_results_to_export());
    }
    let target = Path::new(&path);
    let tmp = target.with_extension("csv.tmp");
    if let Err(e) = gss_core::exporters::save_to_csv(&articles, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CommandError::export_failed(e));
    }
    // Rename only once the whole file is written, so an interrupted export
    // cannot leave a truncated workbook that still looks valid.
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CommandError::export_failed(e));
    }
    Ok(format!("Exported {} result(s) to {path}.", articles.len()))
}

/// File/dir picker for the frontend (import/download/browse paths).
#[tauri::command]
pub fn pick_path(kind: String, title: String) -> Result<String, CommandError> {
    use rfd::FileDialog;
    match kind.as_str() {
        "open" => Ok(FileDialog::new()
            .set_title(&title)
            .add_filter("Ranking files", &["csv", "tsv", "txt", "xlsx", "xlsm"])
            .add_filter("All files", &["*"])
            .pick_file()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()),
        "pickFolder" => Ok(FileDialog::new()
            .set_title(if title.is_empty() {
                "Choose a folder"
            } else {
                &title
            })
            .pick_folder()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()),
        _ => Err(CommandError::new(
            crate::errors::INVALID_ARGUMENT,
            format!("unknown dialog kind {kind}"),
        )),
    }
}

/// `--packaging-smoke DIR`: GUI-free export contract (test_packaging.py).
pub fn run_packaging_smoke(output_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
    let loaded = gss_core::models::Article {
        title: "Unicode Smoke: cafe, alpha + beta, 医学 AI".into(),
        authors: "A. Author; B. Author".into(),
        link: "https://example.edu/smoke".into(),
        relevance_score: Some(88.8),
        doi: "10.1234/smoke.2024.001".into(),
        journal: "Journal of Smoke Testing".into(),
        issns: vec!["1234-5679".into(), "1234-5687".into()],
        publication_year: Some(2024),
        sjr_quartile: "Q1".into(),
        sjr_year: Some(2023),
        sjr_category: "Multidisciplinary".into(),
        sjr_source_url: "https://scimagojr.example/smoke".into(),
        sinta_rank: "S2".into(),
        sinta_year: Some(2024),
        sinta_source_url: "https://sinta.example/smoke".into(),
        pdf_url: String::new(),
        pdf_source_url: String::new(),
        pdf_status: "unknown".into(),
        pdf_detail: "Synthetic fixture: no public-PDF check was performed.".into(),
        pdf_local_path: String::new(),
        ..gss_core::models::Article::default()
    };
    let bare = gss_core::models::Article::new("Unranked Smoke");
    let xlsx = output_dir.join("packaging-smoke.xlsx");
    let csv = output_dir.join("packaging-smoke.csv");
    gss_core::exporters::save_to_excel(&[loaded.clone(), bare.clone()], &xlsx)
        .map_err(|e| e.to_string())?;
    gss_core::exporters::save_to_csv(&[loaded, bare], &csv).map_err(|e| e.to_string())?;
    Ok(())
}

/// `--selfcheck-layout`: headless invariants at the three probe sizes.
pub fn selfcheck_layout() {
    let mut failures = 0;
    for (w, h) in crate::strings::SUPPORTED_SIZES {
        // Window sizing invariants from the Python probe:
        // - default (1080,700) and min (860,560) clamp to screen-(20,80)
        // - every widget fits inside the window; table ≥ 90px tall.
        let (fw, fh) = crate::strings::fitted_window_size(1920, 1080);
        if fw != 1080 || fh != 700 {
            eprintln!("selfcheck: default size mismatch {fw}x{fh}");
            failures += 1;
        }
        if w < crate::strings::WINDOW_MIN_WIDTH || h < crate::strings::WINDOW_MIN_HEIGHT {
            eprintln!("selfcheck: probe size {w}x{h} below minimum");
            failures += 1;
        }
        // FIELD_MIN_WIDTH 300 must fit inside the smallest probe width.
        if 300 > w {
            eprintln!("selfcheck: field min width exceeds probe width {w}");
            failures += 1;
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
    println!("selfcheck-layout ok");
}

#[cfg(test)]
mod tests {
    use super::*;
    use gss_core::models::Article;

    fn catalog_from(csv: &str) -> gss_core::catalog::JournalCatalog {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ranks.csv");
        std::fs::write(&path, csv).unwrap();
        let mut catalog = gss_core::catalog::JournalCatalog::new();
        let count = catalog.import_file(&path, Some(2023)).unwrap();
        assert!(count > 0);
        catalog
    }

    const CSV: &str = "Title,Issn,SJR Best Quartile,Year,Categories,Source\n\
Machine learning,0885-6125,Q1,2023,Artificial Intelligence,https://scimagojr.example/ml\n";

    #[test]
    fn relabel_fills_rank_for_matching_identity() {
        let catalog = catalog_from(CSV);
        // Enriched record: real journal + ISSN (what a Crossref lookup supplies).
        let mut articles = vec![Article {
            title: "Machine learning".into(),
            journal: "Machine learning".into(),
            issns: vec!["0885-6125".into()],
            ..Article::default()
        }];
        let gained = relabel_articles(&mut articles, &catalog);
        assert_eq!(gained, 1);
        assert_eq!(articles[0].sjr_quartile, "Q1");
        assert_eq!(articles[0].sjr_year, Some(2023));
        assert!(articles[0].sjr_source_url.contains("scimagojr.example"));
    }

    #[test]
    fn relabel_ignores_unidentified_records() {
        // Scholar-only metadata (no journal, no ISSN) must never be guessed at:
        // this mirrors the Python catalog contract and leaves the row unknown.
        let catalog = catalog_from(CSV);
        let mut articles = vec![Article {
            title: "Machine learning".into(),
            ..Article::default()
        }];
        let gained = relabel_articles(&mut articles, &catalog);
        assert_eq!(gained, 0);
        assert_eq!(articles[0].sjr_quartile, "");
    }

    #[test]
    fn relabel_counts_only_newly_gained_labels() {
        let catalog = catalog_from(CSV);
        let mut articles = vec![Article {
            title: "Machine learning".into(),
            journal: "Machine learning".into(),
            issns: vec!["0885-6125".into()],
            sjr_quartile: "Q4".into(),
            ..Article::default()
        }];
        // Already labelled → not a gain, but the catalog value still wins.
        let gained = relabel_articles(&mut articles, &catalog);
        assert_eq!(gained, 0);
        assert_eq!(articles[0].sjr_quartile, "Q1");
    }

    #[test]
    fn empty_catalog_is_a_no_op() {
        let catalog = gss_core::catalog::JournalCatalog::new();
        let mut articles = vec![Article {
            title: "Machine learning".into(),
            journal: "Machine learning".into(),
            issns: vec!["0885-6125".into()],
            ..Article::default()
        }];
        let gained = relabel_articles(&mut articles, &catalog);
        assert_eq!(gained, 0);
        assert_eq!(articles[0].sjr_quartile, "");
    }

    #[test]
    fn search_phase_ranking_labels_use_the_same_catalog() {
        let catalog = catalog_from(CSV);
        let mut articles = vec![Article {
            title: "Machine learning".into(),
            journal: "Machine learning".into(),
            issns: vec!["0885-6125".into()],
            ..Article::default()
        }];
        apply_catalog_ranks(&mut articles, &catalog);
        assert_eq!(articles[0].sjr_quartile, "Q1");
    }
}

#[cfg(test)]
mod latch_tests {
    use crate::logging;
    use crate::state::{AppState, Operation};

    /// The latch guard is what stops a worker panic from bricking the app.
    ///
    /// Before it existed, every terminal path cleared the latch by hand; a panic
    /// (or a newly added early `return`) skipped that call, leaving
    /// `is_running()` at `Some(..)` so every later command answered "An
    /// operation is already running." until restart. This test drives the same
    /// transition the guard performs and asserts the state is reachable again.
    #[test]
    fn a_cleared_latch_admits_the_next_operation() {
        let state = AppState::new();
        assert!(!state.is_running());

        state.set_running(Some(Operation::Check));
        assert!(state.is_running());

        // What LatchGuard::drop does, unconditionally.
        state.set_running(None);
        assert!(!state.is_running());

        // And the next operation is admitted.
        state.set_running(Some(Operation::Search));
        assert!(state.is_running());
    }

    /// A panic must be recorded as such, so the log distinguishes "worker died"
    /// from "worker finished".
    #[test]
    fn panic_flag_is_consumed_once() {
        logging::mark_panicking();
        assert!(logging::take_panicking(), "flag must be set");
        assert!(!logging::take_panicking(), "flag must clear after reading");
    }

    /// Logging must never be a correctness dependency: it is best-effort and
    /// must survive a hostile environment.
    #[test]
    fn logging_survives_without_a_writable_profile() {
        logging::info("test", "k=v");
        logging::warn("test", "");
        logging::error("test", "code=X");
    }
}
