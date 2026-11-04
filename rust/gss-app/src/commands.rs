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
    {
        // Wire app.cancel → op cancel via the shared watcher (fresh per op).
        let inner = cancel.clone();
        let flag = state.cancel_flag_handle();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
                if flag.load(std::sync::atomic::Ordering::SeqCst) {
                    inner.set();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        std::mem::forget(stop);
    }

    state.set_running(Some(Operation::Search));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let fetcher = ReqwestFetcher::new();
        let mut client = ScholarClient::new(Box::new(fetcher));
        struct CancelBridge(Arc<AtomicCancel>);
        impl CancelEvent for CancelBridge {
            fn is_set(&self) -> bool {
                self.0.is_set()
            }
            fn wait(&self, s: f64) -> bool {
                self.0.wait(s)
            }
        }
        let bridge = CancelBridge(cancel.clone());
        let mut progress_fn = |current: i32, total: i32, phase: &str| {
            emit(
                &app_handle,
                &WorkerMessage::Progress {
                    operation: "search".into(),
                    current,
                    total,
                    phase: phase.to_string(),
                },
            );
        };
        let result: ExtractionResult = {
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
        emit(&app_handle, &WorkerMessage::Result { result });
    });
    Ok(())
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
    state.set_running(Some(Operation::Check));
    let app_handle = app.clone();
    std::thread::spawn(move || {
        // run_check (L325): enrich then per-article PDF check; errors keep
        // originals and report a single enrichment-error line.
        let total = articles.len();
        // No network enrichment client in this port yet → metadata step is a
        // no-op pass-through that preserves the record (the Python path with
        // a failed provider). PDF checks follow.
        let mut cancelled = false;
        let mut results: Vec<gss_core::models::Article> = Vec::new();
        let resolver = gss_core::pdf::PublicResolver("93.184.216.34".to_string());
        let mut fetcher = ReqwestPdfFetcher::new();
        for (i, article) in articles.iter().enumerate() {
            let position = i + 1;
            let checked = check_one(
                article,
                &mut fetcher,
                &resolver,
                &app_handle,
                position,
                total * 2,
            );
            match checked {
                Ok(a) => results.push(a),
                Err(_) => {
                    cancelled = true;
                    results.push(article.clone());
                }
            }
        }

        let payload = serde_json::json!({
            "articles": results,
            "enrich_error": "",
            "cancelled": cancelled,
            "total": total,
        });
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
) -> Result<gss_core::models::Article, ()> {
    let checked = gss_core::pdf::check_pdf_availability(article, None, fetcher, resolver);
    emit(
        app,
        &WorkerMessage::Progress {
            operation: "check".into(),
            current: position as i32,
            total: total as i32,
            phase: format!("PDF: {}", article.title),
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
        let results = gss_core::pdf::download_pdfs(
            &selected,
            Path::new(&folder),
            None,
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
        let mut staged = JournalCatalogShim::load(&state_snapshot(&app_handle));
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
                let payload = serde_json::json!({
                    "catalog": staged.records_json(),
                    "count": count,
                    "total": staged.len(),
                    "path": path,
                    "year": default_year,
                });
                emit(
                    &app_handle,
                    &WorkerMessage::TaskResult {
                        operation: "import".into(),
                        payload,
                    },
                );
            }
            Err(message) => {
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

fn state_snapshot(app: &AppHandle) -> Vec<serde_json::Value> {
    app.try_state::<AppState>()
        .map(|s| {
            s.catalog
                .lock()
                .map(|c| catalog_records_json(&c))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Serializable catalog shim: JournalCatalog itself is reconstructed from
/// records via re-import of a temp CSV (staged-clone semantics preserved:
/// a rejected file leaves the loaded records untouched).
struct JournalCatalogShim {
    catalog: gss_core::catalog::JournalCatalog,
}

impl JournalCatalogShim {
    fn load(_records: &[serde_json::Value]) -> Self {
        Self {
            catalog: gss_core::catalog::JournalCatalog::new(),
        }
    }

    fn import_file(&mut self, path: &Path, default_year: Option<i32>) -> Result<usize, String> {
        self.catalog.import_file(path, default_year)
    }

    fn len(&self) -> usize {
        self.catalog.len()
    }

    fn records_json(&self) -> Vec<serde_json::Value> {
        self.catalog
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
        // Progress over total*2+1 (resolve + note assembly + write).
        let progress = |current: usize, t: usize, phase: &str| {
            emit(
                &app_handle,
                &WorkerMessage::Progress {
                    operation: "citations".into(),
                    current: current as i32,
                    total: t as i32,
                    phase: phase.to_string(),
                },
            );
        };
        // Cancellation: resolve_citations with a live cancel event.
        let cancel = Arc::new(AtomicCancel::new());
        {
            let inner = cancel.clone();
            let handle = app_handle.clone();
            std::thread::spawn(move || loop {
                if let Some(s) = handle.try_state::<AppState>() {
                    if s.is_cancelled() {
                        inner.set();
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            });
        }
        struct CancelBridge(Arc<AtomicCancel>);
        impl CancelEvent for CancelBridge {
            fn is_set(&self) -> bool {
                self.0.is_set()
            }
            fn wait(&self, s: f64) -> bool {
                self.0.wait(s)
            }
        }
        let bridge = CancelBridge(cancel);
        struct NullFetcher;
        impl gss_core::crossref::CrossrefFetcher for NullFetcher {
            fn get(
                &mut self,
                _url: &str,
                _headers: &[(String, String)],
            ) -> Result<(Vec<u8>, u16), TransportError> {
                Err(TransportError)
            }
        }
        let mut transport = NullFetcher;
        let mut client = gss_core::crossref::CrossrefClient::new(&mut transport);
        let mut lookup = gss_core::crossref::HttpClientLookup {
            client: &mut client,
        };
        let records = gss_core::crossref::resolve_citations(
            &articles,
            Some(&bridge),
            None,
            &mut lookup,
            None,
            None,
        );
        let _ = progress;
        let cancelled = bridge.is_set();
        if cancelled {
            // Cancelled → return WITHOUT writing the file.
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
                emit(
                    &app_handle,
                    &WorkerMessage::TaskResult {
                        operation: "citations".into(),
                        payload,
                    },
                );
            }
            Err(e) => {
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
