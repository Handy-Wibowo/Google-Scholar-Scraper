//! Port of `tests/test_client.py` 1:1 via the HttpFetcher/CancelEvent seams.

use gss_core::client::{
    build_scholar_params, scrape_scholar, AtomicCancel, FetchError, FetchResponse, HttpFetcher,
    ScholarClient,
};
use gss_core::models::ExtractionStatus;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const BASIC_HTML: &str = r#"
<html><body>
  <div class="gs_ri">
    <h3 class="gs_rt"><a href="https://example.edu/one">One</a></h3>
    <div class="gs_a">Author One</div>
  </div>
</body></html>
"#;

const DUPLICATE_HTML: &str = r#"
<html><body>
  <div class="gs_ri">
    <h3 class="gs_rt"><a href="https://example.edu/duplicate#section">  Shared
      Title  </a></h3>
    <div class="gs_a"> Author   One </div>
  </div>
  <div class="gs_ri">
    <h3 class="gs_rt"><a href="https://example.edu/unique">Unique Title</a></h3>
    <div class="gs_a">Author Two</div>
  </div>
</body></html>
"#;

const DUPLICATE_SECOND_PAGE_HTML: &str = r#"
<html><body>
  <div class="gs_ri">
    <h3 class="gs_rt"><a href="https://example.edu/duplicate">SHARED TITLE</a></h3>
    <div class="gs_a">Author One</div>
  </div>
</body></html>
"#;

const RATE_LIMIT_HTML: &str = "<html><body><h1>Too many requests</h1></body></html>";
const BLOCKED_HTML: &str =
    "<html><body>Our systems have detected unusual traffic. CAPTCHA</body></html>";
const EMPTY_HTML: &str = "<html><body>Your search did not match any articles.</body></html>";

/// FakeSession: scripted list of responses/exceptions + call log.
#[derive(Clone)]
struct FakeSession {
    responses: Rc<RefCell<Vec<Result<FetchResponse, FetchError>>>>,
    calls: Rc<RefCell<Vec<Vec<(String, String)>>>>,
}

impl FakeSession {
    fn new(responses: Vec<Result<FetchResponse, FetchError>>) -> Self {
        Self {
            responses: Rc::new(RefCell::new(responses)),
            calls: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn call_starts(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|p| {
                p.iter()
                    .find(|(k, _)| k == "start")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            })
            .collect()
    }
}

impl HttpFetcher for FakeSession {
    fn get(
        &mut self,
        _url: &str,
        params: &[(String, String)],
    ) -> Result<FetchResponse, FetchError> {
        self.calls.borrow_mut().push(params.to_vec());
        let mut queue = self.responses.borrow_mut();
        let first = queue.first().cloned().expect("no more scripted responses");
        queue.remove(0);
        first
    }
}

fn resp(text: &str, status: u16) -> Result<FetchResponse, FetchError> {
    Ok(FetchResponse {
        text: text.to_string(),
        status_code: status,
    })
}

fn timeout_err() -> Result<FetchResponse, FetchError> {
    Err(FetchError::Timeout {
        class: "Timeout".to_string(),
    })
}

fn conn_err() -> Result<FetchResponse, FetchError> {
    Err(FetchError::Connection {
        class: "ConnectionError".to_string(),
    })
}

/// CancellingWait: starts unset; first wait() sets and returns true.
struct CancellingWait(AtomicBool);

impl CancellingWait {
    fn new() -> Self {
        Self(AtomicBool::new(false))
    }
}

impl gss_core::client::CancelEvent for CancellingWait {
    fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn wait(&self, _timeout: f64) -> bool {
        self.0.store(true, Ordering::SeqCst);
        true
    }
}

fn make_client(session: FakeSession, delay: f64, backoff: f64, retries: u32) -> ScholarClient {
    ScholarClient {
        fetcher: Box::new(session),
        page_delay_seconds: delay,
        backoff_seconds: backoff,
        max_retries: retries,
    }
}

#[test]
fn build_scholar_params_preserves_pagination_meaning() {
    let params = build_scholar_params("machine learning", 2, None, None).unwrap();
    let get = |k: &str| {
        params
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(get("q"), "machine learning");
    assert_eq!(get("start"), "20");
    assert_eq!(get("hl"), "en");
    assert_eq!(get("as_sdt"), "0,5");
}

#[test]
fn successful_multi_page_extraction() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 2, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.successful_pages, 2);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.duplicates_removed, 1);
    assert_eq!(calls.borrow().len(), 2);
}

#[test]
fn successful_multi_page_extraction_removes_duplicates() {
    let session = FakeSession::new(vec![
        resp(DUPLICATE_HTML, 200),
        resp(DUPLICATE_SECOND_PAGE_HTML, 200),
    ]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 2, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.articles.len(), 2);
    assert_eq!(result.duplicates_removed, 1);
    let titles: Vec<&str> = result.articles.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["Shared Title", "Unique Title"]);
    assert_eq!(result.articles[0].link, "https://example.edu/duplicate");
    assert!(result.message.contains("Removed 1 duplicate articles."));
    assert!(result.articles[0].relevance_score.is_some());
}

#[test]
fn ranking_disabled_preserves_deduplicated_scholar_order() {
    let session = FakeSession::new(vec![
        resp(DUPLICATE_HTML, 200),
        resp(DUPLICATE_SECOND_PAGE_HTML, 200),
    ]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar(
        "shared title",
        2,
        &mut client,
        false,
        None,
        None,
        None,
        None,
    );

    assert_eq!(result.status, ExtractionStatus::Success);
    let titles: Vec<&str> = result.articles.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["Shared Title", "Unique Title"]);
    assert_eq!(result.articles[0].relevance_score, None);
}

#[test]
fn later_page_rate_limit_preserves_partial_results() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(RATE_LIMIT_HTML, 429)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 2, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::PartialSuccess);
    assert_eq!(result.successful_pages, 1);
    assert_eq!(result.failure_page, Some(2));
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.diagnostic, "RATE_LIMITED");
    assert!(result
        .message
        .contains("Preserved 1 collected articles for review and export."));
    assert!(!result.message.contains("Exported"));
}

#[test]
fn partial_success_removes_duplicates_from_successful_pages() {
    let session = FakeSession::new(vec![
        resp(DUPLICATE_HTML, 200),
        resp(DUPLICATE_SECOND_PAGE_HTML, 200),
        resp(RATE_LIMIT_HTML, 429),
    ]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 3, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::PartialSuccess);
    assert_eq!(result.successful_pages, 2);
    assert_eq!(result.failure_page, Some(3));
    assert_eq!(result.articles.len(), 2);
    assert_eq!(result.duplicates_removed, 1);
    assert_eq!(result.diagnostic, "RATE_LIMITED");
    assert!(result.articles[0].relevance_score.is_some());
}

#[test]
fn rate_limited_first_page_is_not_success() {
    let session = FakeSession::new(vec![resp(RATE_LIMIT_HTML, 429)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::RateLimited);
    assert!(result.articles.is_empty());
}

#[test]
fn blocked_page_is_not_no_results() {
    let session = FakeSession::new(vec![resp(BLOCKED_HTML, 200)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::Blocked);
}

#[test]
fn no_results_page_is_explicit() {
    let session = FakeSession::new(vec![resp(EMPTY_HTML, 200)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::NoResults);
}

#[test]
fn zero_score_articles_are_preserved() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("quantum", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.articles[0].relevance_score, Some(0.0));
}

#[test]
fn network_timeout_is_network_error_after_bounded_retry() {
    let session = FakeSession::new(vec![timeout_err(), timeout_err()]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::NetworkError);
    assert_eq!(calls.borrow().len(), 2);
}

#[test]
fn connection_error_can_retry_successfully() {
    let session = FakeSession::new(vec![conn_err(), resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(calls.borrow().len(), 2);
}

#[test]
fn temporary_http_error_can_retry_successfully() {
    let session = FakeSession::new(vec![resp("temporary outage", 503), resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(calls.borrow().len(), 2);
}

#[test]
fn parser_error_after_success_preserves_partial_results() {
    let incompatible = "<html><body><div id='gs_res_ccl_mid'>changed structure</div></body></html>";
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(incompatible, 200)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 2, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::PartialSuccess);
    assert_eq!(result.diagnostic, "PARSING_ERROR");
    assert_eq!(result.articles.len(), 1);
}

#[test]
fn empty_results_page_after_success_preserves_partial_results() {
    let empty_shell = r#"
        <html><head><title>Google Scholar</title></head><body>
        <div id="gs_md_err" style="display:none">The system can&#39;t perform the operation now. Try again later.</div>
        <div id="gs_ab_md">Page 2 of 7 results (0,06 sec)</div>
        </body></html>
        "#;
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(empty_shell, 200)]);
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 2, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::PartialSuccess);
    assert_eq!(result.diagnostic, "NO_RESULTS");
    assert_eq!(result.articles.len(), 1);
}

#[test]
fn no_retry_for_rate_limit() {
    let session = FakeSession::new(vec![resp(RATE_LIMIT_HTML, 429), resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 1, &mut client, true, None, None, None, None);

    assert_eq!(result.status, ExtractionStatus::RateLimited);
    assert_eq!(calls.borrow().len(), 1);
}

#[test]
fn request_pacing_between_successful_pages() {
    // Python asserts time.sleep(1.5) called exactly once; here the seam is
    // CancelEvent::wait on a recording event (delay > 0 → wait path).
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(BASIC_HTML, 200)]);
    let waits: Arc<Mutex<Vec<f64>>> = Arc::new(Mutex::new(Vec::new()));
    struct RecordingCancel(Arc<Mutex<Vec<f64>>>);
    impl gss_core::client::CancelEvent for RecordingCancel {
        fn is_set(&self) -> bool {
            false
        }
        fn wait(&self, seconds: f64) -> bool {
            self.0.lock().unwrap().push(seconds);
            false
        }
    }
    let mut client = make_client(session, 1.5, 0.0, 1);

    let result = scrape_scholar(
        "query",
        2,
        &mut client,
        true,
        None,
        Some(&RecordingCancel(waits.clone())),
        None,
        None,
    );

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(*waits.lock().unwrap(), vec![1.5]);
}

#[test]
fn progress_callback_reports_page_phases() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200)]);
    let mut client = make_client(session, 0.0, 0.0, 1);
    let events: Arc<Mutex<Vec<(i32, i32, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();

    let mut cb = move |current: i32, total: i32, phase: &str| {
        sink.lock()
            .unwrap()
            .push((current, total, phase.to_string()));
    };

    let result = scrape_scholar(
        "query",
        1,
        &mut client,
        true,
        Some(&mut cb),
        None,
        None,
        None,
    );

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            (1, 1, "requesting".to_string()),
            (1, 1, "parsing".to_string()),
            (1, 1, "completed".to_string())
        ]
    );
}

#[test]
fn cancellation_before_first_page_returns_cancelled() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);
    let cancel = AtomicCancel::new();
    cancel.set();

    let result = scrape_scholar(
        "query",
        1,
        &mut client,
        true,
        None,
        Some(&cancel),
        None,
        None,
    );

    assert_eq!(result.status, ExtractionStatus::Cancelled);
    assert!(result.articles.is_empty());
    assert_eq!(calls.borrow().len(), 0);
}

#[test]
fn cancellation_during_pacing_wait_preserves_results() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 1.5, 0.0, 1);
    let cancel = CancellingWait::new();

    let result = scrape_scholar(
        "query",
        2,
        &mut client,
        true,
        None,
        Some(&cancel),
        None,
        None,
    );

    assert_eq!(result.status, ExtractionStatus::Cancelled);
    assert_eq!(result.successful_pages, 1);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(calls.borrow().len(), 1);
}

#[test]
fn cancellation_during_backoff_returns_cancelled() {
    let session = FakeSession::new(vec![timeout_err(), resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 1.5, 1);
    let cancel = CancellingWait::new();

    let result = scrape_scholar(
        "query",
        1,
        &mut client,
        true,
        None,
        Some(&cancel),
        None,
        None,
    );

    assert_eq!(result.status, ExtractionStatus::Cancelled);
    assert!(result.articles.is_empty());
    assert_eq!(calls.borrow().len(), 1);
}

// ---- YearFilterClientTests ----

#[test]
fn any_time_omits_both_bounds() {
    let params = build_scholar_params("query", 0, None, None).unwrap();
    let flat: Vec<(String, String)> = params;
    assert_eq!(
        flat,
        vec![
            ("start".to_string(), "0".to_string()),
            ("q".to_string(), "query".to_string()),
            ("hl".to_string(), "en".to_string()),
            ("as_sdt".to_string(), "0,5".to_string()),
        ]
    );
}

#[test]
fn closed_open_equal_and_future_ranges() {
    for (lower, upper) in [
        (Some(2018), Some(2024)),
        (Some(2024), None),
        (None, Some(2024)),
        (Some(2024), Some(2024)),
        (Some(9999), Some(9999)),
        (Some(1000), None),
    ] {
        let params = build_scholar_params("query", 2, lower, upper).unwrap();
        let get = |k: &str| {
            params
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("start").as_deref(), Some("20"));
        assert_eq!(get("as_ylo"), lower.map(|y| y.to_string()));
        assert_eq!(get("as_yhi"), upper.map(|y| y.to_string()));
    }
}

#[test]
fn invalid_years_rejected_before_network() {
    // Out-of-range values are rejected; the Python type-checks (bool/float/str)
    // are compile-time impossible in Rust's Option<i32> signature.
    for (lower, upper) in [
        (Some(2025), Some(2020)),
        (Some(999), None),
        (None, Some(10000)),
    ] {
        let session = FakeSession::new(vec![]);
        let calls = session.calls.clone();
        let mut client = make_client(session, 1.5, 2.0, 1);
        // scrape_scholar panics on invalid range (mirrors the uncaught ValueError
        // path through the UI boundary); assert via catch_unwind.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scrape_scholar("query", 1, &mut client, true, None, None, lower, upper)
        }));
        assert!(
            result.is_err(),
            "expected rejection for {lower:?}/{upper:?}"
        );
        assert_eq!(calls.borrow().len(), 0);
    }
    for (lower, upper) in [
        (Some(2025), Some(2020)),
        (Some(999), None),
        (None, Some(10000)),
    ] {
        assert!(build_scholar_params("query", 1, lower, upper).is_err());
    }
}

#[test]
fn url_encoding_matches_parameter_builder() {
    let query = "\"SOMLAP\" & C++ / café?";
    let params = build_scholar_params(query, 1, Some(2018), Some(2024)).unwrap();
    let url = gss_core::client::build_scholar_url(query, 1, Some(2018), Some(2024)).unwrap();
    let query_part = url.split('?').nth(1).unwrap();
    for (k, v) in params {
        let expected = format!("{}={}", k, v);
        // Each param appears encoded; compare via naive quote_plus round-trip.
        let encoded = quote_plus_str(&k) + "=" + &quote_plus_str(&v);
        assert!(
            query_part.contains(&encoded) || query_part.contains(&expected),
            "param {k}={v} not found in {query_part}"
        );
    }
}

fn quote_plus_str(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
            b' ' => out.push('+'),
            b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[test]
fn bounds_survive_timeouts_server_retries_and_pagination() {
    let session = FakeSession::new(vec![
        timeout_err(),
        resp(BASIC_HTML, 200),
        resp("temporary", 503),
        resp(DUPLICATE_HTML, 200),
    ]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar(
        "query",
        2,
        &mut client,
        true,
        None,
        None,
        Some(2018),
        Some(2024),
    );

    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.successful_pages, 2);
    let starts: Vec<String> = calls
        .borrow()
        .iter()
        .map(|c| {
            c.iter()
                .find(|(k, _)| k == "start")
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(starts, vec!["0", "0", "10", "10"]);
    for call in calls.borrow().iter() {
        let get = |k: &str| {
            call.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("as_ylo"), "2018");
        assert_eq!(get("as_yhi"), "2024");
    }
}

#[test]
fn partial_results_retained_with_bounds() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200), resp(RATE_LIMIT_HTML, 429)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 0.0, 0.0, 1);

    let result = scrape_scholar("query", 2, &mut client, true, None, None, Some(2024), None);

    assert_eq!(result.status, ExtractionStatus::PartialSuccess);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.successful_pages, 1);
    for call in calls.borrow().iter() {
        assert!(call.iter().any(|(k, v)| k == "as_ylo" && v == "2024"));
    }
}

#[test]
fn cancellation_retains_first_page_with_bounds() {
    let session = FakeSession::new(vec![resp(BASIC_HTML, 200)]);
    let calls = session.calls.clone();
    let mut client = make_client(session, 1.0, 0.0, 1);
    let cancel = CancellingWait::new();

    let result = scrape_scholar(
        "query",
        2,
        &mut client,
        true,
        None,
        Some(&cancel),
        None,
        Some(2024),
    );

    assert_eq!(result.status, ExtractionStatus::Cancelled);
    assert_eq!(result.articles.len(), 1);
    let first = &calls.borrow()[0];
    assert!(first.iter().any(|(k, v)| k == "as_yhi" && v == "2024"));
}
