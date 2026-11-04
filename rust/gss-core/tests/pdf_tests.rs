//! Port of `tests/test_pdf_service.py` — all classes 1:1 on the PdfFetcher/
//! PdfStream/DnsResolver seams that mirror FakeSession/FakeResponse/resolver.

use gss_core::client::{CancelEvent, FetchError};
use gss_core::models::Article;
use gss_core::pdf::{
    check_pdf_availability, download_pdfs, finalize_download, is_public_http_url,
    pdf_filename_stem, DnsResolver, PdfFetcher, PdfStream, MIN_PDF_BYTES, PDF_AVAILABLE,
    PDF_UNAVAILABLE, PDF_UNKNOWN,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

const PUBLIC_HOST: &str = "https://repo.example.org";
const PUBLIC_IP: &str = "93.184.216.34";

fn openalex_doi_url(doi: &str) -> String {
    format!("https://api.openalex.org/works/doi:{doi}")
}

fn openalex_title_url(title: &str) -> String {
    format!(
        "https://api.openalex.org/works?filter=title.search:{}&per-page=5&select=id,title,open_access,best_oa_location,locations",
        quote_all(title)
    )
}

fn quote_all(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        let ch = b as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '~') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn public_resolver() -> PublicResolver {
    PublicResolver(PUBLIC_IP.to_string())
}

pub struct PublicResolver(String);

impl DnsResolver for PublicResolver {
    fn resolve(&self, _host: &str) -> std::io::Result<Vec<String>> {
        Ok(vec![self.0.clone()])
    }
}

struct FailingResolver;

impl DnsResolver for FailingResolver {
    fn resolve(&self, _host: &str) -> std::io::Result<Vec<String>> {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "dns down"))
    }
}

fn private_resolver() -> PrivateResolver {
    PrivateResolver
}

struct PrivateResolver;

impl DnsResolver for PrivateResolver {
    fn resolve(&self, _host: &str) -> std::io::Result<Vec<String>> {
        Ok(vec!["10.1.2.3".to_string()])
    }
}

fn pdf_bytes(size: usize) -> Vec<u8> {
    let mut body = b"%PDF-1.4\n".to_vec();
    body.resize(size.max(9), b'x');
    body
}

/// Chunk iterator content: bytes or an injected error mid-stream.
#[derive(Clone)]
enum Chunk {
    Body(Vec<u8>),
    Err(FetchError),
}

/// Hook run when a chunk is served (used to set cancel flags mid-stream).
type ServeHook = Rc<RefCell<Option<Box<dyn FnMut()>>>>;

/// FakeResponse: status, headers, body or explicit chunk list.
/// Shared via Rc<RefCell<..>> so every download of the same URL observes a
/// fresh stream (mirrors Python creating one FakeResponse per session.get).
struct FakeResponse {
    status_code: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    chunks: Option<Vec<Chunk>>,
    on_serve: ServeHook,
    close_count: Rc<RefCell<usize>>,
    url: RefCell<String>,
    requested_chunk_size: Rc<RefCell<usize>>,
}

impl FakeResponse {
    fn new(body: &[u8], status: u16, content_type: &str) -> Self {
        let mut headers = HashMap::new();
        if !content_type.is_empty() {
            headers.insert("Content-Type".to_string(), content_type.to_string());
        }
        Self {
            status_code: status,
            headers,
            body: body.to_vec(),
            chunks: None,
            on_serve: Rc::new(RefCell::new(None)),
            close_count: Rc::new(RefCell::new(0)),
            url: RefCell::new(String::new()),
            requested_chunk_size: Rc::new(RefCell::new(0)),
        }
    }

    fn with_chunks(body: &[u8], status: u16, content_type: &str, chunks: Vec<Chunk>) -> Self {
        let mut r = Self::new(body, status, content_type);
        r.chunks = Some(chunks);
        r
    }

    fn with_redirect(status: u16, location: &str) -> Self {
        let mut r = Self::new(b"", status, "");
        r.headers
            .insert("Location".to_string(), location.to_string());
        r
    }

    fn set_on_serve(&self, hook: impl FnMut() + 'static) {
        *self.on_serve.borrow_mut() = Some(Box::new(hook));
    }

    /// A fresh copy of this response spec: same status/headers/body/chunks,
    /// new stream state (counters, url, hooks). Mirrors Python constructing
    /// one FakeResponse object per session.get call.
    fn respawn(&self) -> FakeResponse {
        FakeResponse {
            status_code: self.status_code,
            headers: self.headers.clone(),
            body: self.body.clone(),
            chunks: self.chunks.clone(),
            on_serve: Rc::new(RefCell::new(None)),
            close_count: Rc::new(RefCell::new(0)),
            url: RefCell::new(String::new()),
            requested_chunk_size: Rc::new(RefCell::new(0)),
        }
    }
}

impl PdfStream for FakeResponse {
    fn status_code(&self) -> u16 {
        self.status_code
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).cloned()
    }

    fn url(&self) -> String {
        self.url.borrow().clone()
    }

    fn set_url(&mut self, url: String) {
        *self.url.borrow_mut() = url;
    }

    fn next_chunk(&mut self, max: usize) -> Option<Result<Vec<u8>, FetchError>> {
        *self.requested_chunk_size.borrow_mut() = max;
        if let Some(hook) = &mut *self.on_serve.borrow_mut() {
            hook();
        }
        if let Some(chunks) = &mut self.chunks {
            if chunks.is_empty() {
                return None;
            }
            let chunk = chunks.remove(0);
            return Some(match chunk {
                Chunk::Body(b) => Ok(b),
                Chunk::Err(e) => Err(e),
            });
        }
        if self.body.is_empty() {
            None
        } else {
            // Serve at most `max` bytes per call, like iter_content.
            let take = self.body.len().min(max);
            let head = self.body.drain(..take).collect::<Vec<u8>>();
            Some(Ok(head))
        }
    }

    fn close(&mut self) {
        *self.close_count.borrow_mut() += 1;
    }
}

type SharedResponse = Rc<RefCell<FakeResponse>>;

/// Boxed stream adapter so the service can hold it generically.
struct StreamHandle {
    shared: SharedResponse,
}

impl PdfStream for StreamHandle {
    fn status_code(&self) -> u16 {
        self.shared.borrow().status_code()
    }
    fn header(&self, name: &str) -> Option<String> {
        self.shared.borrow().header(name)
    }
    fn url(&self) -> String {
        self.shared.borrow().url()
    }
    fn set_url(&mut self, url: String) {
        self.shared.borrow_mut().set_url(url)
    }
    fn next_chunk(&mut self, max: usize) -> Option<Result<Vec<u8>, FetchError>> {
        self.shared.borrow_mut().next_chunk(max)
    }
    fn close(&mut self) {
        self.shared.borrow_mut().close()
    }
}

/// Factory: each request gets a FRESH stream from the same template
/// (mirrors Python constructing one FakeResponse object per get()).
type ResponseTemplate = Rc<dyn Fn() -> SharedResponse>;

/// FakeSession: URL-routed response templates or errors; records call order.
struct FakeSession {
    routes: HashMap<String, Entry>,
    calls: Rc<RefCell<Vec<String>>>,
}

enum Entry {
    Template(ResponseTemplate),
    ConnError,
    Timeout,
}

impl FakeSession {
    fn new() -> Self {
        Self {
            routes: HashMap::new(),
            calls: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn route(mut self, url: &str, response: FakeResponse) -> Self {
        // Every request to this URL serves a FRESH stream from this spec.
        self.routes.insert(
            url.to_string(),
            Entry::Template(Rc::new(move || Rc::new(RefCell::new(response.respawn())))),
        );
        self
    }

    fn route_template(mut self, url: &str, template: ResponseTemplate) -> Self {
        self.routes
            .insert(url.to_string(), Entry::Template(template));
        self
    }

    fn route_shared(mut self, url: &str, shared: SharedResponse) -> Self {
        self.routes.insert(
            url.to_string(),
            Entry::Template(Rc::new(move || shared.clone())),
        );
        self
    }

    fn conn_error(mut self, url: &str) -> Self {
        self.routes.insert(url.to_string(), Entry::ConnError);
        self
    }

    fn timeout(mut self, url: &str) -> Self {
        self.routes.insert(url.to_string(), Entry::Timeout);
        self
    }

    fn call_count(&self) -> usize {
        self.calls.borrow().len()
    }
}

impl PdfFetcher for FakeSession {
    fn get(
        &mut self,
        url: &str,
        _headers: &[(String, String)],
    ) -> Result<Box<dyn PdfStream>, FetchError> {
        self.calls.borrow_mut().push(url.to_string());
        match self.routes.get(url) {
            None => Err(FetchError::Connection {
                class: "ConnectionError".to_string(),
            }),
            Some(Entry::ConnError) => Err(FetchError::Connection {
                class: "ConnectionError".to_string(),
            }),
            Some(Entry::Timeout) => Err(FetchError::Timeout {
                class: "Timeout".to_string(),
            }),
            Some(Entry::Template(make)) => Ok(Box::new(StreamHandle { shared: make() })),
        }
    }
}

/// routes_with_no_title_match: default empty title index for `title="Paper"`.
fn with_title_index(routes: HashMap<String, Entry>, title: &str) -> Vec<(String, Entry)> {
    let mut all: Vec<(String, Entry)> = routes.into_iter().collect();
    let index_url = openalex_title_url(title);
    if !all.iter().any(|(u, _)| *u == index_url) {
        all.push((
            index_url,
            Entry::Template(Rc::new(move || {
                Rc::new(RefCell::new(FakeResponse::new(
                    b"{\"results\": []}",
                    200,
                    "application/json",
                )))
            })),
        ));
    }
    all
}

fn session_from(entries: Vec<(String, Entry)>) -> FakeSession {
    let mut s = FakeSession::new();
    for (url, entry) in entries {
        s.routes.insert(url, entry);
    }
    s
}

fn session_from_vec(entries: Vec<(String, Entry)>) -> FakeSession {
    session_from(entries)
}

fn check(article: &Article, session: &mut FakeSession) -> Article {
    check_pdf_availability(article, None, session, &public_resolver())
}

// ---- PublicUrlValidationTests ----

#[test]
fn rejects_credentials_and_non_http_schemes() {
    for url in [
        "file:///etc/passwd",
        "ftp://repo.example.org/a.pdf",
        "https://user:secret@repo.example.org/a.pdf",
        "//repo.example.org/a.pdf",
        "",
    ] {
        assert!(
            !is_public_http_url(url, &public_resolver()),
            "should reject {url}"
        );
    }
}

#[test]
fn accepts_bare_public_origin() {
    assert!(is_public_http_url(
        "https://repo.example.org",
        &public_resolver()
    ));
    assert!(is_public_http_url(
        "https://repo.example.org/",
        &public_resolver()
    ));
}

#[test]
fn rejects_local_and_reserved_destinations() {
    for url in [
        "http://127.0.0.1/a.pdf",
        "http://10.0.0.1/a.pdf",
        "http://192.168.1.5/a.pdf",
        "http://169.254.169.254/latest/meta-data/",
        "http://0.0.0.0/a.pdf",
        "http://224.0.0.1/a.pdf",
        "http://192.0.2.5/a.pdf",
        "http://100.64.0.1/a.pdf",
        "http://[::1]/a.pdf",
        "http://[fe80::1]/a.pdf",
        "http://[fc00::1]/a.pdf",
    ] {
        assert!(
            !is_public_http_url(url, &public_resolver()),
            "should reject {url}"
        );
    }
}

#[test]
fn accepts_public_http_destinations() {
    assert!(is_public_http_url(
        "https://repo.example.org/a.pdf",
        &public_resolver()
    ));
    assert!(is_public_http_url(
        "https://93.184.216.34/a.pdf",
        &public_resolver()
    ));
}

#[test]
fn host_resolving_to_private_address_is_rejected() {
    assert!(!is_public_http_url(
        "https://evil.example.org/a.pdf",
        &private_resolver()
    ));
}

#[test]
fn unsafe_link_is_not_fetched_when_title_index_answers() {
    let index_url = openalex_title_url("Paper");
    let mut session = session_from_vec(with_title_index(HashMap::new(), "Paper"));
    let article = Article {
        title: "Paper".into(),
        pdf_url: "http://127.0.0.1/local.pdf".into(),
        ..Article::default()
    };

    let result = check(&article, &mut session);

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
}

// ---- PdfContentTests ----

#[test]
fn real_pdf_content_is_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url.clone(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(result.pdf_source_url, url);
    assert!(result.pdf_detail.contains("Validated PDF content"));
}

#[test]
fn html_page_is_not_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"<html><body>login</body></html>",
                200,
                "text/html",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert!(result.pdf_detail.contains("HTML"));
}

#[test]
fn html_challenge_page_stays_unknown() {
    let url = format!("{PUBLIC_HOST}/challenge.pdf");
    let response = Rc::new(RefCell::new(FakeResponse::new(
        b"<html><script src='/_fs-ch-/challenge.js'></script></html>",
        200,
        "text/html; charset=utf-8",
    )));
    let mut session = FakeSession::new().route_shared(&url, response.clone());

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result.pdf_detail.contains("could not be verified"));
    assert!(!result.pdf_detail.contains("No public PDF found"));
    assert!(*response.borrow().close_count.borrow() > 0);
}

#[test]
fn html_challenge_preview_honors_cancellation() {
    let url = format!("{PUBLIC_HOST}/challenge.pdf");
    let cancel_flag = Rc::new(AtomicBool::new(false));
    struct Cancel(Rc<AtomicBool>);
    impl CancelEvent for Cancel {
        fn is_set(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self, _t: f64) -> bool {
            self.0.store(true, Ordering::SeqCst);
            true
        }
    }
    // chunks are served lazily; the cancel flag is set when the FIRST chunk
    // has been served (mirrors the Python generator that sets the event
    // between yields).
    let response: SharedResponse = Rc::new(RefCell::new(FakeResponse::with_chunks(
        b"",
        200,
        "text/html",
        vec![
            Chunk::Body(b"<html>".to_vec()),
            Chunk::Body(b"_fs-ch-".to_vec()),
        ],
    )));
    let flag = cancel_flag.clone();
    response.borrow().set_on_serve(move || {
        // Called before each chunk; set after the first serve.
        if flag.swap(true, Ordering::SeqCst) {
            // already set
        }
    });
    let mut session = FakeSession::new().route_template(&url, {
        let response = response.clone();
        Rc::new(move || response.clone())
    });
    let cancel = Cancel(cancel_flag);
    let calls = session.calls.clone();
    let close_count = Rc::clone(&response.borrow().close_count);

    let result = check_pdf_availability(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        Some(&cancel),
        &mut session,
        &public_resolver(),
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result.pdf_detail.contains("Cancelled"));
    assert_eq!(calls.borrow().len(), 1);
    assert!(*close_count.borrow() > 0);
}

#[test]
fn html_challenge_preview_is_bounded_to_16_kb() {
    let url = format!("{PUBLIC_HOST}/challenge.pdf");
    let mut body = vec![b'x'; 16 * 1024];
    body.extend_from_slice(b"_fs-ch-");
    let response: SharedResponse =
        Rc::new(RefCell::new(FakeResponse::new(&body, 200, "text/html")));
    let shared_size = Rc::clone(&response.borrow().requested_chunk_size);
    let mut routes: HashMap<String, Entry> = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || response.clone())),
    );
    routes.insert(
        openalex_title_url("Paper"),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"{\"results\": []}",
                200,
                "application/json",
            )))
        })),
    );
    let mut session = session_from_vec(routes.into_iter().collect());

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert!(*shared_size.borrow() <= 16 * 1024);
}

#[test]
fn html_masquerading_as_pdf_header_is_not_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let body = b"%PDF-1.4\n<html><body>Please sign in to download</body></html>".to_vec();
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                &body,
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert!(result.pdf_detail.contains("HTML"));
}

#[test]
fn pdf_content_type_without_signature_is_not_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"not a pdf at all",
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert!(result.pdf_detail.contains("signature"));
}

#[test]
fn empty_body_is_not_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(b"", 200, "application/pdf")))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
}

#[test]
fn header_only_body_is_not_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"%PDF-1.4",
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert!(result.pdf_detail.contains("too small"));
}

#[test]
fn truncated_header_is_not_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"%PDF-",
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert!(result.pdf_detail.contains("signature"));
}

#[test]
fn pdf_split_across_small_chunks_is_available() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let body = pdf_bytes(MIN_PDF_BYTES + 32);
    let chunks = vec![
        Chunk::Body(body[0..4].to_vec()),
        Chunk::Body(body[4..8].to_vec()),
        Chunk::Body(body[8..20].to_vec()),
        Chunk::Body(body[20..MIN_PDF_BYTES].to_vec()),
        Chunk::Body(body[MIN_PDF_BYTES..].to_vec()),
    ];
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::with_chunks(&body, 200, "application/octet-stream", chunks),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url.clone(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(result.pdf_source_url, url);
}

#[test]
fn empty_chunks_between_real_content_are_ignored() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let body = pdf_bytes(MIN_PDF_BYTES + 16);
    let chunks = vec![
        Chunk::Body(vec![]),
        Chunk::Body(body[..4].to_vec()),
        Chunk::Body(vec![]),
        Chunk::Body(body[4..].to_vec()),
        Chunk::Body(vec![]),
    ];
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::with_chunks(&body, 200, "application/pdf", chunks),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
}

#[test]
fn cancellation_during_preview_is_unknown() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let cancel_flag = Rc::new(AtomicBool::new(false));
    struct Cancel(Rc<AtomicBool>);
    impl CancelEvent for Cancel {
        fn is_set(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self, _t: f64) -> bool {
            self.0.store(true, Ordering::SeqCst);
            true
        }
    }
    // Cancel becomes pending once the first chunk (header) has been served.
    let response: SharedResponse = Rc::new(RefCell::new(FakeResponse::with_chunks(
        b"",
        200,
        "application/pdf",
        vec![
            Chunk::Body(b"%PDF-1.4\n".to_vec()),
            Chunk::Body(vec![b'x'; MIN_PDF_BYTES * 2]),
        ],
    )));
    let flag = cancel_flag.clone();
    response.borrow().set_on_serve(move || {
        flag.store(true, Ordering::SeqCst);
    });
    let mut session = FakeSession::new().route_template(&url, {
        let response = response.clone();
        Rc::new(move || response.clone())
    });
    let cancel = Cancel(cancel_flag);

    let result = check_pdf_availability(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        Some(&cancel),
        &mut session,
        &public_resolver(),
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result.pdf_detail.contains("Cancelled"));
}

#[test]
fn exact_minimum_size_is_available_in_check_and_download() {
    let url = format!("{PUBLIC_HOST}/boundary.pdf");
    let mut body = b"%PDF-1.4\n".to_vec();
    body.extend(vec![b'x'; MIN_PDF_BYTES - 9]);
    assert_eq!(body.len(), MIN_PDF_BYTES);

    let mut session =
        FakeSession::new().route(&url, FakeResponse::new(&body, 200, "application/pdf"));
    let body2 = body.clone();
    let checked = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url.clone(),
            ..Article::default()
        },
        &mut session,
    );

    let tmp = tempfile::tempdir().unwrap();
    let mut session2 =
        FakeSession::new().route(&url, FakeResponse::new(&body2, 200, "application/pdf"));
    let downloaded = &download_pdfs(
        &[Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        }],
        tmp.path(),
        None,
        None,
        &mut session2,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    )[0];
    let stored = std::fs::read(&downloaded.pdf_local_path).unwrap();

    assert_eq!(checked.pdf_status, PDF_AVAILABLE);
    assert_eq!(downloaded.pdf_status, PDF_AVAILABLE);
    assert_eq!(stored, body);
}

#[test]
fn one_byte_below_minimum_is_unavailable_in_both_paths() {
    let url = format!("{PUBLIC_HOST}/short.pdf");
    let mut body = b"%PDF-1.4\n".to_vec();
    body.extend(vec![b'x'; MIN_PDF_BYTES - 10]);
    let body_for_second = body.clone();

    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                &body,
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));
    let checked = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url.clone(),
            ..Article::default()
        },
        &mut session,
    );

    let tmp = tempfile::tempdir().unwrap();
    let mut routes2 = HashMap::new();
    routes2.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                &body_for_second,
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session2 = session_from_vec(with_title_index(routes2, "Paper"));
    let downloaded = &download_pdfs(
        &[Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        }],
        tmp.path(),
        None,
        None,
        &mut session2,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    )[0];

    assert_eq!(checked.pdf_status, PDF_UNAVAILABLE);
    assert_eq!(downloaded.pdf_status, PDF_UNAVAILABLE);
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

// ---- RedirectTests ----

#[test]
fn public_redirect_chain_is_followed() {
    let start = format!("{PUBLIC_HOST}/start");
    let middle = format!("{PUBLIC_HOST}/middle");
    let final_url = format!("{PUBLIC_HOST}/final.pdf");
    let mut session = FakeSession::new()
        .route(&start, FakeResponse::with_redirect(302, &middle))
        .route(&middle, FakeResponse::with_redirect(301, &final_url))
        .route(
            &final_url,
            FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
        );

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: start,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(result.pdf_source_url, final_url);
    assert_eq!(session.call_count(), 3);
}

#[test]
fn redirect_to_private_host_is_never_fetched() {
    let start = format!("{PUBLIC_HOST}/start");
    let index_url = openalex_title_url("Paper");
    let mut routes = HashMap::new();
    routes.insert(
        start.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::with_redirect(
                302,
                "http://10.0.0.1/secret.pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: start,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    let calls = session.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert!(calls.contains(&index_url));
}

#[test]
fn redirect_to_link_local_metadata_endpoint_is_never_fetched() {
    let start = format!("{PUBLIC_HOST}/start");
    let index_url = openalex_title_url("Paper");
    let mut routes = HashMap::new();
    routes.insert(
        start.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::with_redirect(
                302,
                "http://169.254.169.254/latest/meta-data/",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Paper"));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: start,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    let calls = session.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert!(calls.contains(&index_url));
}

#[test]
fn redirect_loop_is_bounded() {
    let url = format!("{PUBLIC_HOST}/loop");
    let mut session = FakeSession::new().route(&url, FakeResponse::with_redirect(302, &url));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(session.call_count() <= 7);
}

#[test]
fn redirect_without_location_is_unknown() {
    let url = format!("{PUBLIC_HOST}/start");
    let mut session = FakeSession::new().route(&url, FakeResponse::with_redirect(302, ""));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
}

// ---- TransportTests ----

#[test]
fn connection_error_is_unknown() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().conn_error(&url);

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result.pdf_detail.contains("ConnectionError"));
}

#[test]
fn timeout_is_unknown() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().timeout(&url);

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
}

#[test]
fn forbidden_and_server_errors_are_unknown() {
    for status in [401u16, 403, 429, 500, 503] {
        let url = format!("{PUBLIC_HOST}/a{status}.pdf");
        let mut session = FakeSession::new().route(&url, FakeResponse::new(b"", status, ""));
        let result = check(
            &Article {
                title: "Paper".into(),
                pdf_url: url,
                ..Article::default()
            },
            &mut session,
        );
        assert_eq!(result.pdf_status, PDF_UNKNOWN, "status {status}");
    }
}

#[test]
fn missing_and_gone_are_unavailable() {
    for status in [404u16, 410] {
        let url = format!("{PUBLIC_HOST}/a{status}.pdf");
        let mut routes = HashMap::new();
        routes.insert(
            url.clone(),
            Entry::Template(Rc::new(move || {
                Rc::new(RefCell::new(FakeResponse::new(b"", status, "")))
            })),
        );
        let mut session = session_from_vec(with_title_index(routes, "Paper"));
        let result = check(
            &Article {
                title: "Paper".into(),
                pdf_url: url,
                ..Article::default()
            },
            &mut session,
        );
        assert_eq!(result.pdf_status, PDF_UNAVAILABLE, "status {status}");
        assert!(result.pdf_detail.contains(&status.to_string()));
    }
}

#[test]
fn dns_failure_is_unknown() {
    let mut session = FakeSession::new();
    let article = Article {
        title: "Paper".into(),
        pdf_url: format!("{PUBLIC_HOST}/a.pdf"),
        ..Article::default()
    };
    let result = check_pdf_availability(&article, None, &mut session, &FailingResolver);
    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert_eq!(session.call_count(), 0);
}

// ---- AbsentCandidateTests ----

#[test]
fn record_without_link_or_identifier_uses_title_index() {
    let index_url = openalex_title_url("Paper");
    let mut routes = HashMap::new();
    routes.insert(
        index_url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"{\"results\": []}",
                200,
                "application/json",
            )))
        })),
    );
    let mut session = session_from_vec(routes.into_iter().collect());

    let result = check(
        &Article {
            title: "Paper".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    let calls = session.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], index_url);
}

#[test]
fn missing_link_without_identifier_is_unavailable() {
    let url = format!("{PUBLIC_HOST}/gone.pdf");
    let index_url = openalex_title_url("Paper");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(b"", 404, "")))
        })),
    );
    routes.insert(
        index_url,
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"{\"results\": []}",
                200,
                "application/json",
            )))
        })),
    );
    let mut session = session_from_vec(routes.into_iter().collect());

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: url,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert_eq!(session.call_count(), 2);
}

#[test]
fn unknown_candidate_is_not_reported_as_unavailable() {
    let good = format!("{PUBLIC_HOST}/a.pdf");
    let bad = format!("{PUBLIC_HOST}/b.pdf");
    let mut session = FakeSession::new()
        .conn_error(&good)
        .route(&bad, FakeResponse::new(b"", 404, ""));

    let result = check(
        &Article {
            title: "Paper".into(),
            pdf_url: good,
            pdf_source_url: bad,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
}

// ---- FieldPreservationTests ----

#[test]
fn check_preserves_unrelated_fields() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );
    let article = Article {
        title: "Original Title".into(),
        authors: "A. Author".into(),
        link: "https://scholar.google.com/x".into(),
        relevance_score: Some(0.75),
        doi: "10.1000/test".into(),
        journal: "Journal of Tests".into(),
        issns: vec!["1234-5678".into()],
        publication_year: Some(2021),
        sjr_quartile: "Q2".into(),
        sjr_year: Some(2023),
        sjr_category: "Education".into(),
        sjr_source_url: "https://scimagojr.example/x".into(),
        sinta_rank: "S3".into(),
        sinta_year: Some(2024),
        sinta_source_url: "https://sinta.example/y".into(),
        pdf_url: url.clone(),
        pdf_source_url: "https://repo.example.org/landing".into(),
        pdf_status: "unknown".into(),
        pdf_detail: "".into(),
        pdf_local_path: "".into(),
    };

    let result = check(&article, &mut session);

    assert_eq!(result.title, "Original Title");
    assert_eq!(result.authors, "A. Author");
    assert_eq!(result.link, "https://scholar.google.com/x");
    assert_eq!(result.relevance_score, Some(0.75));
    assert_eq!(result.doi, "10.1000/test");
    assert_eq!(result.journal, "Journal of Tests");
    assert_eq!(result.issns, vec!["1234-5678".to_string()]);
    assert_eq!(result.publication_year, Some(2021));
    assert_eq!(result.sjr_quartile, "Q2");
    assert_eq!(result.sjr_year, Some(2023));
    assert_eq!(result.sjr_category, "Education");
    assert_eq!(result.sjr_source_url, "https://scimagojr.example/x");
    assert_eq!(result.sinta_rank, "S3");
    assert_eq!(result.sinta_year, Some(2024));
    assert_eq!(result.sinta_source_url, "https://sinta.example/y");
    // The candidate link is owned by the metadata module: never rewritten.
    assert_eq!(result.pdf_url, url);
    assert_eq!(result.pdf_local_path, "");
    assert_eq!(result.pdf_status, PDF_AVAILABLE);
}

#[test]
fn download_preserves_unrelated_fields_and_sets_local_path() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );
    let article = Article {
        title: "Preserved".into(),
        authors: "B. Author".into(),
        link: "https://scholar.google.com/y".into(),
        relevance_score: Some(0.5),
        doi: "10.1000/keep".into(),
        journal: "Some Journal".into(),
        pdf_url: url.clone(),
        ..Article::default()
    };

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[article],
        tmp.path(),
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results.len(), 1);
    let saved = &results[0];
    assert_eq!(saved.title, "Preserved");
    assert_eq!(saved.authors, "B. Author");
    assert_eq!(saved.link, "https://scholar.google.com/y");
    assert_eq!(saved.relevance_score, Some(0.5));
    assert_eq!(saved.doi, "10.1000/keep");
    assert_eq!(saved.journal, "Some Journal");
    assert_eq!(saved.pdf_url, url);
    assert_eq!(saved.pdf_status, PDF_AVAILABLE);
    assert!(saved.pdf_local_path.ends_with(".pdf"));
}

// ---- FilenameTests ----

#[test]
fn windows_unsafe_characters_are_removed() {
    let stem = pdf_filename_stem("A/B\\C:D*E?F\"G<H>I|J");
    for bad in ['/', '\\', ':', '*', '?', '"', '<', '>', '|'] {
        assert!(!stem.contains(bad), "{stem}");
    }
}

#[test]
fn reserved_device_names_are_escaped() {
    for name in ["CON", "con", "PRN", "AUX", "NUL", "COM1", "LPT9"] {
        assert_ne!(pdf_filename_stem(name).to_uppercase(), name.to_uppercase());
    }
}

#[test]
fn reserved_device_names_with_extension_are_escaped() {
    for name in [
        "CON.txt",
        "con.pdf",
        "PRN.data",
        "AUX.tar.gz",
        "COM1.notes",
        "LPT9.x.y",
    ] {
        let stem = pdf_filename_stem(name);
        assert!(stem.starts_with('_'), "{stem}");
        assert_ne!(
            stem.split('.').next().unwrap().to_uppercase(),
            name.split('.').next().unwrap().to_uppercase()
        );
    }
}

#[test]
fn empty_and_trailing_dot_titles_fall_back() {
    assert_eq!(pdf_filename_stem(""), "paper");
    assert_eq!(pdf_filename_stem("   "), "paper");
    assert!(!pdf_filename_stem("Title.").ends_with('.'));
    assert!(!pdf_filename_stem(&"A".repeat(500)).is_empty());
}

#[test]
fn colliding_titles_do_not_overwrite() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );
    let articles: Vec<Article> = (0..3)
        .map(|_| Article {
            title: "Same Title".into(),
            pdf_url: url.clone(),
            ..Article::default()
        })
        .collect();

    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path();
    let existing = folder.join("Same Title.pdf");
    std::fs::write(&existing, b"PRE-EXISTING").unwrap();

    let results = download_pdfs(
        &articles,
        folder,
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(std::fs::read(&existing).unwrap(), b"PRE-EXISTING");
    let paths: Vec<&str> = results.iter().map(|r| r.pdf_local_path.as_str()).collect();
    assert_eq!(paths.len(), 3);
    assert_eq!(
        std::fs::read_dir(folder).unwrap().count(),
        4,
        "3 downloads + 1 pre-existing"
    );
}

#[test]
fn same_name_different_article_in_one_batch_gets_unique_files() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[
            Article {
                title: "Dup".into(),
                pdf_url: url.clone(),
                ..Article::default()
            },
            Article {
                title: "Dup".into(),
                pdf_url: url,
                ..Article::default()
            },
        ],
        tmp.path(),
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    let mut names: Vec<String> = results
        .iter()
        .map(|r| {
            Path::new(&r.pdf_local_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["Dup (1).pdf", "Dup.pdf"]);
    for r in &results {
        let content = std::fs::read(&r.pdf_local_path).unwrap();
        assert!(content.starts_with(b"%PDF-"));
    }
}

// ---- SizeAndCleanupTests ----

#[test]
fn oversize_download_is_unknown_and_leaves_no_file() {
    let url = format!("{PUBLIC_HOST}/big.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(5000), 200, "application/pdf"),
    );

    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path();
    let results = download_pdfs(
        &[Article {
            title: "Big".into(),
            pdf_url: url,
            ..Article::default()
        }],
        folder,
        None,
        None,
        &mut session,
        1024,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_UNKNOWN);
    assert!(results[0].pdf_detail.contains("size limit"));
    assert_eq!(results[0].pdf_local_path, "");
    assert_eq!(std::fs::read_dir(folder).unwrap().count(), 0);
}

#[test]
fn transport_failure_mid_stream_cleans_partial_file() {
    let url = format!("{PUBLIC_HOST}/broken.pdf");
    let chunks = vec![
        Chunk::Body(pdf_bytes(256)),
        Chunk::Err(FetchError::Connection {
            class: "ConnectionError".to_string(),
        }),
    ];
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::with_chunks(b"", 200, "application/pdf", chunks),
    );

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[Article {
            title: "Broken".into(),
            pdf_url: url,
            ..Article::default()
        }],
        tmp.path(),
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_UNKNOWN);
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn cancellation_mid_stream_cleans_partial_file() {
    let url = format!("{PUBLIC_HOST}/slow.pdf");
    let cancel_flag = Rc::new(AtomicBool::new(false));
    struct Cancel(Rc<AtomicBool>);
    impl CancelEvent for Cancel {
        fn is_set(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self, _t: f64) -> bool {
            self.0.store(true, Ordering::SeqCst);
            true
        }
    }
    // Cancel becomes pending once the first chunk has been served.
    let response: SharedResponse = Rc::new(RefCell::new(FakeResponse::with_chunks(
        b"",
        200,
        "application/pdf",
        vec![Chunk::Body(pdf_bytes(256)), Chunk::Body(pdf_bytes(256))],
    )));
    let flag = cancel_flag.clone();
    response.borrow().set_on_serve(move || {
        flag.store(true, Ordering::SeqCst);
    });
    let mut session = FakeSession::new().route_template(&url, {
        let response = response.clone();
        Rc::new(move || response.clone())
    });
    let cancel = Cancel(cancel_flag);

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[Article {
            title: "Slow".into(),
            pdf_url: url,
            ..Article::default()
        }],
        tmp.path(),
        Some(&cancel),
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_UNKNOWN);
    assert!(results[0].pdf_detail.contains("Cancelled"));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn cancel_before_start_keeps_every_entry_in_order() {
    let cancel_flag = AtomicBool::new(true);
    struct Cancel(AtomicBool);
    impl CancelEvent for Cancel {
        fn is_set(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self, _t: f64) -> bool {
            true
        }
    }
    let cancel = Cancel(cancel_flag);
    let mut session = FakeSession::new();
    let articles: Vec<Article> = (0..3)
        .map(|i| Article {
            title: format!("P{i}"),
            ..Article::default()
        })
        .collect();

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &articles,
        tmp.path(),
        Some(&cancel),
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["P0", "P1", "P2"]);
    assert!(results.iter().all(|r| r.pdf_status == PDF_UNKNOWN));
    assert_eq!(session.call_count(), 0);
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn unwritable_folder_keeps_ordering_and_reports_failure() {
    let base = tempfile::tempdir().unwrap();
    let blocker = base.path().join("file-not-dir");
    std::fs::write(&blocker, b"x").unwrap();

    let mut session = FakeSession::new();
    let results = download_pdfs(
        &[
            Article {
                title: "A".into(),
                ..Article::default()
            },
            Article {
                title: "B".into(),
                ..Article::default()
            },
        ],
        &blocker,
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["A", "B"]);
    assert!(results.iter().all(|r| r.pdf_status == PDF_UNKNOWN));
    assert!(results[0].pdf_detail.contains("could not be created"));
}

// ---- DownloadFlowTests ----

#[test]
fn mixed_batch_preserves_order_and_reports_progress() {
    let good = format!("{PUBLIC_HOST}/good.pdf");
    let missing = format!("{PUBLIC_HOST}/missing.pdf");
    let broken = format!("{PUBLIC_HOST}/broken.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        good.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                &pdf_bytes(2048),
                200,
                "application/pdf",
            )))
        })),
    );
    routes.insert(
        missing.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(b"", 404, "")))
        })),
    );
    routes.insert(broken.clone(), Entry::ConnError);
    let mut session = session_from_vec(with_title_index(routes, "Missing"));

    let articles = vec![
        Article {
            title: "Good".into(),
            pdf_url: good,
            ..Article::default()
        },
        Article {
            title: "Missing".into(),
            pdf_url: missing,
            ..Article::default()
        },
        Article {
            title: "Broken".into(),
            pdf_url: broken,
            ..Article::default()
        },
    ];
    let events: Rc<RefCell<Vec<(usize, usize, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let mut progress = move |current: usize, total: usize, phase: &str| {
        sink.borrow_mut().push((current, total, phase.to_string()));
    };

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &articles,
        tmp.path(),
        None,
        Some(&mut progress),
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["Good", "Missing", "Broken"]);
    let statuses: Vec<&str> = results.iter().map(|r| r.pdf_status.as_str()).collect();
    assert_eq!(statuses, vec![PDF_AVAILABLE, PDF_UNAVAILABLE, PDF_UNKNOWN]);
    let ev = events.borrow();
    let currents: Vec<usize> = ev.iter().map(|e| e.0).collect();
    assert_eq!(currents, vec![1, 1, 2, 2, 3, 3]);
    assert!(ev.iter().all(|e| e.1 == 3));
    let phases: Vec<&str> = ev.iter().map(|e| e.2.as_str()).collect();
    assert_eq!(
        phases,
        vec![
            "checking",
            "saved",
            "checking",
            "unavailable",
            "checking",
            "unknown"
        ]
    );
}

#[test]
fn download_creates_missing_folder() {
    let url = format!("{PUBLIC_HOST}/a.pdf");
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );

    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("nested").join("pdfs");
    let results = download_pdfs(
        &[Article {
            title: "Nested".into(),
            pdf_url: url,
            ..Article::default()
        }],
        &folder,
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_AVAILABLE);
    assert!(Path::new(&results[0].pdf_local_path).is_file());
}

#[test]
fn empty_input_returns_empty_list() {
    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[],
        tmp.path(),
        None,
        None,
        &mut FakeSession::new(),
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );
    assert!(results.is_empty());
}

// ---- DoiDiscoveryTests ----

fn openalex_payload(pdf_url: &str, landing: &str, is_oa: bool) -> Vec<u8> {
    format!(
        "{{\"doi\": \"https://doi.org/10.1000/test\", \
         \"open_access\": {{\"oa_url\": \"\"}}, \
         \"best_oa_location\": {{\"is_oa\": {is_oa}, \"pdf_url\": \"{pdf_url}\", \
         \"landing_page_url\": \"{landing}\"}}, \
         \"locations\": []}}"
    )
    .into_bytes()
}

#[test]
fn doi_discovery_finds_public_pdf() {
    let pdf = format!("{PUBLIC_HOST}/discovered.pdf");
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new()
        .route(
            &doi_url,
            FakeResponse::new(&openalex_payload(&pdf, "", true), 200, "application/json"),
        )
        .route(
            &pdf,
            FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
        );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(result.pdf_source_url, pdf);
    let calls = session.calls.borrow();
    assert!(calls.iter().any(|c| c == &doi_url));
}

#[test]
fn doi_discovery_ignores_private_discovered_url() {
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().route(
        &doi_url,
        FakeResponse::new(
            &openalex_payload("http://127.0.0.1/internal.pdf", "", true),
            200,
            "application/json",
        ),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    let calls = session.calls.borrow();
    assert!(!calls.iter().any(|c| c.contains("127.0.0.1")));
}

#[test]
fn doi_discovery_provider_failure_stays_unknown() {
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().conn_error(&doi_url);

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
}

#[test]
fn doi_discovery_without_result_is_unavailable() {
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().route(
        &doi_url,
        FakeResponse::new(
            b"{\"open_access\": {\"oa_url\": \"\"}, \"locations\": []}",
            200,
            "application/json",
        ),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
}

#[test]
fn direct_candidate_is_used_before_doi_lookup() {
    let direct = format!("{PUBLIC_HOST}/direct.pdf");
    let mut session = FakeSession::new().route(
        &direct,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            pdf_url: direct.clone(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(session.call_count(), 1);
}

#[test]
fn closed_location_is_not_treated_as_a_public_pdf() {
    let closed = format!("{PUBLIC_HOST}/closed.pdf");
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().route(
        &doi_url,
        FakeResponse::new(
            &openalex_payload(&closed, "", false),
            200,
            "application/json",
        ),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    let calls = session.calls.borrow();
    assert!(!calls.iter().any(|c| c == &closed));
}

#[test]
fn landing_page_only_location_is_not_a_pdf_candidate() {
    let landing = format!("{PUBLIC_HOST}/landing");
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().route(
        &doi_url,
        FakeResponse::new(
            &openalex_payload("", &landing, true),
            200,
            "application/json",
        ),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    let calls = session.calls.borrow();
    assert!(!calls.iter().any(|c| c == &landing));
}

#[test]
fn index_redirect_to_private_host_is_never_followed() {
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().route(
        &doi_url,
        FakeResponse::with_redirect(302, "http://169.254.169.254/meta"),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert_eq!(session.call_count(), 1);
}

#[test]
fn index_metadata_body_is_size_bounded() {
    let doi_url = openalex_doi_url("10.1000/test");
    let mut huge = b"{\"locations\": [".to_vec();
    huge.extend(vec![b' '; 2 * 1024 * 1024]);
    let mut session =
        FakeSession::new().route(&doi_url, FakeResponse::new(&huge, 200, "application/json"));

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result.pdf_detail.contains("index"));
}

#[test]
fn index_malformed_body_stays_unknown() {
    let doi_url = openalex_doi_url("10.1000/test");
    let mut session = FakeSession::new().route(
        &doi_url,
        FakeResponse::new(b"<html>nope</html>", 200, "text/html"),
    );

    let result = check(
        &Article {
            title: "Paper".into(),
            doi: "10.1000/test".into(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
}

// ---- TitleDiscoveryTests ----

fn title_work(title: &str, pdf_url: &str, work_id: &str) -> String {
    format!(
        "{{\"id\": \"https://openalex.org/{work_id}\", \"title\": \"{title}\", \
         \"open_access\": {{\"is_oa\": {}}}, \
         \"best_oa_location\": {{\"is_oa\": {}, \"pdf_url\": \"{}\"}}, \
         \"locations\": []}}",
        !pdf_url.is_empty(),
        !pdf_url.is_empty(),
        pdf_url
    )
}

fn title_payload(works: &[String]) -> Vec<u8> {
    format!("{{\"results\": [{}]}}", works.join(",")).into_bytes()
}

#[test]
fn doi_less_title_discovery_finds_exact_match_public_pdf() {
    let title = "Multimodal techniques for malware classification using deep learning";
    let index_url = openalex_title_url(title);
    let pdf = format!("{PUBLIC_HOST}/title-discovered.pdf");
    let mut session = FakeSession::new()
        .route(
            &index_url,
            FakeResponse::new(
                &title_payload(&[title_work(title, &pdf, "W1")]),
                200,
                "application/json",
            ),
        )
        .route(
            &pdf,
            FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
        );

    let result = check(
        &Article {
            title: title.to_string(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(result.pdf_source_url, pdf);
    assert_eq!(session.call_count(), 2);
}

#[test]
fn zero_exact_title_matches_keeps_prior_outcome() {
    let title = "Exact Paper";
    let index_url = openalex_title_url(title);
    let mut session = FakeSession::new().route(
        &index_url,
        FakeResponse::new(
            &title_payload(&[title_work("Different Paper", "", "W1")]),
            200,
            "application/json",
        ),
    );

    let result = check(
        &Article {
            title: title.to_string(),
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert_eq!(session.call_count(), 1);
}

#[test]
fn ambiguous_exact_title_matches_keep_prior_outcome() {
    let title = "Shared Title";
    let direct = format!("{PUBLIC_HOST}/gone.pdf");
    let index_url = openalex_title_url(title);
    let mut session = FakeSession::new()
        .route(&direct, FakeResponse::new(b"", 404, ""))
        .route(
            &index_url,
            FakeResponse::new(
                &title_payload(&[
                    title_work(title, &format!("{PUBLIC_HOST}/one.pdf"), "W1"),
                    title_work(title, &format!("{PUBLIC_HOST}/two.pdf"), "W2"),
                ]),
                200,
                "application/json",
            ),
        );

    let result = check(
        &Article {
            title: title.to_string(),
            pdf_url: direct,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNAVAILABLE);
    assert_eq!(session.call_count(), 2);
}

#[test]
fn unreachable_title_index_upgrades_clean_rejection_to_unknown() {
    let title = "Paper";
    let direct = format!("{PUBLIC_HOST}/gone.pdf");
    let index_url = openalex_title_url(title);
    let mut session = FakeSession::new()
        .route(&direct, FakeResponse::new(b"", 404, ""))
        .conn_error(&index_url);

    let result = check(
        &Article {
            title: title.to_string(),
            pdf_url: direct,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result
        .pdf_detail
        .contains("could not be consulted by title"));
    assert_eq!(session.call_count(), 2);
}

#[test]
fn available_direct_candidate_skips_unreachable_title_index() {
    let title = "Paper";
    let direct = format!("{PUBLIC_HOST}/direct.pdf");
    let mut session = FakeSession::new().route(
        &direct,
        FakeResponse::new(&pdf_bytes(2048), 200, "application/pdf"),
    );

    let result = check(
        &Article {
            title: title.to_string(),
            pdf_url: direct,
            ..Article::default()
        },
        &mut session,
    );

    assert_eq!(result.pdf_status, PDF_AVAILABLE);
    assert_eq!(session.call_count(), 1);
}

#[test]
fn cancellation_during_title_index_read_stops_without_more_requests() {
    let title = "Paper";
    let index_url = openalex_title_url(title);
    let cancel_flag = Rc::new(AtomicBool::new(false));
    struct Cancel(Rc<AtomicBool>);
    impl CancelEvent for Cancel {
        fn is_set(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self, _t: f64) -> bool {
            self.0.store(true, Ordering::SeqCst);
            true
        }
    }
    let body = title_payload(&[title_work(
        title,
        &format!("{PUBLIC_HOST}/never-requested.pdf"),
        "W1",
    )]);
    let response: SharedResponse = Rc::new(RefCell::new(FakeResponse::with_chunks(
        &body,
        200,
        "application/json",
        vec![
            Chunk::Body(body[..10].to_vec()),
            Chunk::Body(body[10..].to_vec()),
        ],
    )));
    let flag = cancel_flag.clone();
    response.borrow().set_on_serve(move || {
        flag.store(true, Ordering::SeqCst);
    });
    let close_count = Rc::clone(&response.borrow().close_count);
    let mut session = FakeSession::new().route_template(&index_url, {
        let response = response.clone();
        Rc::new(move || response.clone())
    });
    let cancel = Cancel(cancel_flag);

    let result = check_pdf_availability(
        &Article {
            title: title.to_string(),
            ..Article::default()
        },
        Some(&cancel),
        &mut session,
        &public_resolver(),
    );

    assert_eq!(result.pdf_status, PDF_UNKNOWN);
    assert!(result.pdf_detail.contains("Cancelled"));
    assert_eq!(session.call_count(), 1);
    assert!(*close_count.borrow() > 0);
}

// ---- LocalFailureStateTests ----

#[test]
fn local_failure_does_not_downgrade_a_verified_status() {
    let base = tempfile::tempdir().unwrap();
    let blocker = base.path().join("file-not-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let verified = Article {
        title: "Known".into(),
        pdf_status: PDF_AVAILABLE.into(),
        pdf_detail: "previously verified".into(),
        ..Article::default()
    };

    let mut session = FakeSession::new();
    let results = download_pdfs(
        &[verified],
        &blocker,
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_AVAILABLE);
    assert!(results[0].pdf_detail.contains("could not be created"));
}

#[test]
fn cancellation_does_not_downgrade_a_verified_status() {
    let cancel_flag = AtomicBool::new(true);
    struct Cancel(AtomicBool);
    impl CancelEvent for Cancel {
        fn is_set(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn wait(&self, _t: f64) -> bool {
            true
        }
    }
    let cancel = Cancel(cancel_flag);
    let verified = Article {
        title: "Known".into(),
        pdf_status: PDF_AVAILABLE.into(),
        pdf_detail: "previously verified".into(),
        ..Article::default()
    };

    let tmp = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new();
    let results = download_pdfs(
        &[verified],
        tmp.path(),
        Some(&cancel),
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_AVAILABLE);
    assert!(results[0].pdf_detail.contains("Cancelled"));
}

// ---- FinalizationRaceTests ----

#[test]
fn existing_destination_is_never_overwritten_by_finalize() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path();
    let destination = folder.join("Race.pdf");
    std::fs::write(&destination, b"WINNER").unwrap();

    let temp_path = folder.join(".pdf-part-race.tmp");
    std::fs::write(&temp_path, b"%PDF-1.4\nloser").unwrap();

    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let claimed =
        finalize_download(temp_path.to_str().unwrap(), folder, "Race", &mut taken).unwrap();

    assert_ne!(claimed, destination);
    assert_eq!(std::fs::read(&destination).unwrap(), b"WINNER");
    assert!(std::fs::read(&claimed).unwrap().starts_with(b"%PDF-"));
}

#[test]
fn repeated_finalize_of_same_title_never_collides() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut written: Vec<PathBuf> = Vec::new();
    for index in 0..3u8 {
        let temp_path = folder.join(format!(".pdf-part-{index}.tmp"));
        let mut content = b"%PDF-1.4\n".to_vec();
        content.push(index);
        std::fs::write(&temp_path, &content).unwrap();
        written.push(
            finalize_download(temp_path.to_str().unwrap(), folder, "Dup", &mut taken).unwrap(),
        );
    }

    let unique: std::collections::HashSet<String> = written
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(unique.len(), 3);
    let mut names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Dup (1).pdf", "Dup (2).pdf", "Dup.pdf"]);
    for (index, path) in written.iter().enumerate() {
        let mut expected = b"%PDF-1.4\n".to_vec();
        expected.push(index as u8);
        assert_eq!(std::fs::read(path).unwrap(), expected);
    }
}

// ---- FinalizationFallbackTests ----
// The no-hardlink fallback cannot be forced portably from the test process
// (hard links succeed on NTFS/tmpfs); the reservation path is exercised
// structurally: claim_path falls back on any non-AlreadyExists error.

#[test]
fn finalize_reports_error_when_no_free_name_is_found() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path();
    // Simulate the taken-set exhaustion path via a reserved-name conflict.
    let temp_path = folder.join(".pdf-part-x.tmp");
    std::fs::write(&temp_path, b"%PDF-1.4\nbody").unwrap();
    let mut taken: std::collections::HashSet<String> = (0..=10001u64)
        .map(|i| format!("dup ({i}).pdf").to_lowercase())
        .collect();
    taken.insert("dup.pdf".to_string());
    let result = finalize_download(temp_path.to_str().unwrap(), folder, "dup", &mut taken);
    assert!(result.is_err());
}

// ---- DownloadStreamValidationTests ----

#[test]
fn download_accepts_pdf_split_across_small_chunks() {
    let url = format!("{PUBLIC_HOST}/split.pdf");
    let body = pdf_bytes(MIN_PDF_BYTES + 64);
    let chunks = vec![
        Chunk::Body(body[0..3].to_vec()),
        Chunk::Body(body[3..9].to_vec()),
        Chunk::Body(body[9..40].to_vec()),
        Chunk::Body(body[40..].to_vec()),
    ];
    let mut session = FakeSession::new().route(
        &url,
        FakeResponse::with_chunks(&body, 200, "application/octet-stream", chunks),
    );

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[Article {
            title: "Split".into(),
            pdf_url: url,
            ..Article::default()
        }],
        tmp.path(),
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_AVAILABLE);
    let stored = std::fs::read(&results[0].pdf_local_path).unwrap();
    assert_eq!(stored, body);
    let mut names: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Split.pdf"]);
}

#[test]
fn download_rejects_header_only_body_and_cleans_up() {
    let url = format!("{PUBLIC_HOST}/header-only.pdf");
    let mut routes = HashMap::new();
    routes.insert(
        url.clone(),
        Entry::Template(Rc::new(move || {
            Rc::new(RefCell::new(FakeResponse::new(
                b"%PDF-1.4",
                200,
                "application/pdf",
            )))
        })),
    );
    let mut session = session_from_vec(with_title_index(routes, "Header"));

    let tmp = tempfile::tempdir().unwrap();
    let results = download_pdfs(
        &[Article {
            title: "Header".into(),
            pdf_url: url,
            ..Article::default()
        }],
        tmp.path(),
        None,
        None,
        &mut session,
        gss_core::pdf::DEFAULT_MAX_BYTES,
        &public_resolver(),
    );

    assert_eq!(results[0].pdf_status, PDF_UNAVAILABLE);
    assert_eq!(results[0].pdf_local_path, "");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}
