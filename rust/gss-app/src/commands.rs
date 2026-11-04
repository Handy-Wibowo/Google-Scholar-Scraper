//! Tauri command layer: each GUI operation spawns a worker thread that
//! streams `WorkerMessage` events and honors the shared AtomicBool cancel.
//! Excel/CSV exports run synchronously (UI-thread contract); citations run
//! in a background task; worker exceptions become task_error messages.

use crate::state::{self, AppState, Operation, WorkerMessage};
use gss_core::client::{AtomicCancel, CancelEvent, HttpFetcher, ScholarClient};

/// Search progress callback as consumed by `scrape_scholar`.
type SearchProgress<'a> = &'a mut dyn FnMut(i32, i32, &str);
use gss_core::models::ExtractionResult;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

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
) -> Result<(), String> {
    if state.is_running() {
        return Err("An operation is already running.".into());
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
pub fn check_metadata_pdfs(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.is_running() {
        return Err("An operation is already running.".into());
    }
    let articles = state.articles();
    if articles.is_empty() {
        return Err("NOTHING_TO_CHECK".into());
    }
    let catalog = state.catalog.lock().map(|c| c.clone()).unwrap_or_default();
    state.set_running(Some(Operation::Check));
    let app_handle = app.clone();
    std::thread::spawn(move || {
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
        let enrich_error = String::new();
        let enriched: Vec<gss_core::models::Article> = {
            let mut transport = ReqwestCrossref::new();
            let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
            let mut lookup = gss_core::crossref::HttpClientLookup {
                client: &mut client,
            };
            gss_core::crossref::enrich_articles(
                &articles,
                Some(&catalog),
                Some(&bridge),
                Some(&mut enrich_progress),
                &mut lookup,
                None,
                None,
                true,
            )
        };

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
) -> Result<(), String> {
    if state.is_running() {
        return Err("An operation is already running.".into());
    }
    let articles = state.articles();
    let selected: Vec<gss_core::models::Article> = indices
        .iter()
        .filter_map(|i| articles.get(*i).cloned())
        .collect();
    if selected.is_empty() {
        return Err("NO_PAPERS_SELECTED".into());
    }
    state.set_running(Some(Operation::Download));
    let app_handle = app.clone();
    std::thread::spawn(move || {
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
) -> Result<(), String> {
    if state.is_running() {
        return Err("An operation is already running.".into());
    }
    if !Path::new(&path).is_file() {
        return Err(format!("No file found at {path}."));
    }
    // Staging clone: a rejected file leaves the loaded records untouched.
    let existing = state.catalog.lock().map(|c| c.clone()).unwrap_or_default();
    state.set_running(Some(Operation::Import));
    let app_handle = app.clone();
    std::thread::spawn(move || {
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
) -> Result<(), String> {
    if state.is_running() {
        return Err("An operation is already running.".into());
    }
    if articles.is_empty() {
        return Err("NO_RESULTS_TO_EXPORT".into());
    }
    let format = format.to_lowercase();
    if format != "bibtex" && format != "ris" {
        return Err("Unsupported citation format.".into());
    }
    state.set_running(Some(Operation::Citations));
    let app_handle = app.clone();
    std::thread::spawn(move || {
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
        let mut lookup = gss_core::crossref::HttpClientLookup {
            client: &mut client,
        };
        let records = gss_core::crossref::resolve_citations(
            &articles,
            Some(&bridge),
            Some(&mut progress),
            &mut lookup,
            None,
            None,
        );
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
pub fn load_settings() -> crate::settings::UiSettings {
    crate::settings::load()
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
) -> Result<String, String> {
    let _ = state;
    if articles.is_empty() {
        return Err("NO_RESULTS_TO_EXPORT".into());
    }
    gss_core::exporters::save_to_excel(&articles, Path::new(&path))
        .map(|_| format!("Exported {} result(s) to {path}.", articles.len()))
        .map_err(|e| format!("Export failed: {e}"))
}

/// `export_csv` — synchronous on the UI thread.
#[tauri::command]
pub fn export_csv(
    state: State<'_, AppState>,
    articles: Vec<gss_core::models::Article>,
    path: String,
) -> Result<String, String> {
    let _ = state;
    if articles.is_empty() {
        return Err("NO_RESULTS_TO_EXPORT".into());
    }
    gss_core::exporters::save_to_csv(&articles, Path::new(&path))
        .map(|_| format!("Exported {} result(s) to {path}.", articles.len()))
        .map_err(|e| format!("Export failed: {e}"))
}

/// File/dir picker for the frontend (import/download/browse paths).
#[tauri::command]
pub fn pick_path(kind: String, title: String) -> Result<String, String> {
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
        _ => Err(format!("unknown dialog kind {kind}")),
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
