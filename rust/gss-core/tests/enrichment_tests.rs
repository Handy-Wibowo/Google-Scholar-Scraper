//! Port of `test_enrichment.py` (Crossref mapping, client behavior,
//! enrichment, citation fields, resolve_citations) 1:1.

use gss_core::catalog::JournalCatalog;
use gss_core::crossref::{
    enrich_articles, record_from_crossref_message, resolve_citations, ArticleLookup,
    CrossrefClient, CrossrefFetcher, CrossrefRecord, TransportError, CITATION_WORK_TYPES,
    SCHOLAR_AUTHORS_WARNING, UNRESOLVED_WARNING,
};
use gss_core::models::{normalize_title_key, Article, CitationRecord};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::json;

fn journal_message() -> serde_json::Value {
    json!({
        "DOI": "10.1234/example.2024.001",
        "type": "journal-article",
        "title": ["Deep Learning for Medical Imaging"],
        "container-title": ["Journal of Examples"],
        "ISSN": ["1234-5679"],
        "published-print": {"date-parts": [[2024, 3]]},
        "author": [{"family": "Author", "given": "A"}],
    })
}

fn conference_message() -> serde_json::Value {
    json!({
        "DOI": "10.1234/conf.2023.001",
        "type": "proceedings-article",
        "title": ["A Conference Paper"],
        "container-title": ["Conference on Examples"],
        "ISSN": ["1234-5679"],
        "published-print": {"date-parts": [[2023]]},
    })
}

/// Transport stub: queued JSON payloads / errors / statuses.
struct FakeTransport {
    responses: RefCell<Vec<Result<(Vec<u8>, u16), ()>>>,
    pub requests: RefCell<Vec<String>>,
}

impl FakeTransport {
    fn json(responses: Vec<Result<serde_json::Value, ()>>) -> Self {
        Self {
            responses: RefCell::new(
                responses
                    .into_iter()
                    .map(|r| r.map(|v| (serde_json::to_vec(&v).unwrap(), 200u16)))
                    .collect(),
            ),
            requests: RefCell::new(Vec::new()),
        }
    }

    fn raw(responses: Vec<Result<(Vec<u8>, u16), ()>>) -> Self {
        Self {
            responses: RefCell::new(responses),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl CrossrefFetcher for FakeTransport {
    fn get(
        &mut self,
        url: &str,
        _headers: &[(String, String)],
    ) -> Result<(Vec<u8>, u16), TransportError> {
        self.requests.borrow_mut().push(url.to_string());
        self.responses
            .borrow_mut()
            .remove(0)
            .map_err(|()| TransportError)
    }
}

/// RecordingLookup: answers per constraints, records calls (`RecordingLookup`).
struct RecordingLookup {
    by_constraints: HashMap<(String, String, Option<i32>), CrossrefRecord>,
    by_doi: HashMap<String, CrossrefRecord>,
    title_calls: RefCell<Vec<(String, String, Option<i32>)>>,
    doi_calls: RefCell<Vec<String>>,
}

impl RecordingLookup {
    fn new() -> Self {
        Self {
            by_constraints: HashMap::new(),
            by_doi: HashMap::new(),
            title_calls: RefCell::new(Vec::new()),
            doi_calls: RefCell::new(Vec::new()),
        }
    }

    fn with(
        by_constraints: HashMap<(&str, &str, Option<i32>), CrossrefRecord>,
        by_doi: HashMap<&str, CrossrefRecord>,
    ) -> Self {
        Self {
            by_constraints: by_constraints
                .into_iter()
                .map(|((t, a, y), r)| ((t.to_string(), a.to_string(), y), r))
                .collect(),
            by_doi: by_doi
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            title_calls: RefCell::new(Vec::new()),
            doi_calls: RefCell::new(Vec::new()),
        }
    }
}

impl ArticleLookup for RecordingLookup {
    fn lookup(&mut self, doi: &str) -> Option<CrossrefRecord> {
        self.doi_calls.borrow_mut().push(doi.to_string());
        self.by_doi.get(doi).cloned()
    }

    fn lookup_title_citations(
        &mut self,
        title: &str,
        author: &str,
        year: Option<i32>,
        _work_types: &'static [&'static str],
    ) -> Option<CrossrefRecord> {
        self.title_calls
            .borrow_mut()
            .push((title.to_string(), author.to_string(), year));
        self.by_constraints
            .get(&(title.to_string(), author.to_string(), year))
            .cloned()
    }
}

// ---- TitleNormalizationTests ----

#[test]
fn normalize_title_key_decomposes_case_punctuation_and_diacritics() {
    assert_eq!(
        normalize_title_key("  Étude: DEEP-Learning!  "),
        "etudedeeplearning"
    );
}

// ---- CrossrefMappingTests ----

#[test]
fn journal_article_carries_container_identity() {
    let record = record_from_crossref_message(&journal_message(), "");
    assert_eq!(record.journal, "Journal of Examples");
    assert_eq!(record.issns, vec!["1234-5679".to_string()]);
    assert_eq!(record.year, Some(2024));
    assert_eq!(record.work_type, "journal-article");
}

#[test]
fn conference_paper_gets_no_journal_identity() {
    let record = record_from_crossref_message(&conference_message(), "");
    assert_eq!(record.journal, "");
    assert!(record.issns.is_empty());
    assert_eq!(record.work_type, "proceedings-article");
}

#[test]
fn public_pdf_candidate_requires_licence_and_pdf_link() {
    let mut without_licence = journal_message();
    without_licence["link"] =
        json!([{"URL": "https://repo.example/a.pdf", "content-type": "application/pdf"}]);
    let mut with_licence = without_licence.clone();
    with_licence["license"] = json!([{"URL": "https://creativecommons.org/licenses/by/4.0/"}]);

    assert_eq!(
        record_from_crossref_message(&without_licence, "").pdf_candidate_url,
        ""
    );
    assert_eq!(
        record_from_crossref_message(&with_licence, "").pdf_candidate_url,
        "https://repo.example/a.pdf"
    );
}

// ---- ClientBehaviourTests ----

#[test]
fn lookup_returns_none_on_transport_error() {
    let mut transport = FakeTransport::raw(vec![Err(())]);
    let mut client = CrossrefClient::new(&mut transport);
    assert!(client.lookup("10.1234/example").is_none());
}

#[test]
fn lookup_returns_none_on_non_200() {
    let mut transport = FakeTransport::raw(vec![Ok((b"{}".to_vec(), 404))]);
    let mut client = CrossrefClient::new(&mut transport);
    assert!(client.lookup("10.1234/example").is_none());
}

#[test]
fn lookup_returns_none_on_malformed_json() {
    let mut transport = FakeTransport::raw(vec![Ok((b"{not json".to_vec(), 200))]);
    let mut client = CrossrefClient::new(&mut transport);
    assert!(client.lookup("10.1234/example").is_none());
}

#[test]
fn lookup_refuses_an_oversized_body() {
    // A declared Content-Length over the bound is rejected before reading
    // (Python); the Rust port bounds the received body instead — both refuse
    // to parse anything over max_response_bytes. The observable contract is
    // tested: a body larger than the bound yields no record.
    let mut big = r#"{"message":"#.as_bytes().to_vec();
    big.extend(vec![b' '; 10_000]);
    big.push(b'}');
    let mut transport = FakeTransport::raw(vec![Ok((big, 200))]);
    let mut client = CrossrefClient::new(&mut transport);
    client.max_response_bytes = 1024;
    assert!(client.lookup("10.1234/example.2024.001").is_none());
}

#[test]
fn doi_is_url_encoded() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let _ = client.lookup("10.1234/example 2024");
    let url = transport.requests.borrow()[0].clone();
    assert!(!url.contains(' '), "{url}");
    assert!(url.contains("10.1234%2Fexample%202024"), "{url}");
}

#[test]
fn lookup_accepts_a_payload_echoing_the_requested_doi() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let record = client.lookup("10.1234/example.2024.001").unwrap();
    assert_eq!(record.doi, "10.1234/example.2024.001");
}

#[test]
fn lookup_rejects_a_payload_for_a_different_doi() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    assert!(client.lookup("10.9999/some.other.work").is_none());
}

#[test]
fn lookup_rejects_a_payload_with_no_doi_at_all() {
    let mut without_doi = journal_message();
    without_doi.as_object_mut().unwrap().remove("DOI");
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": without_doi}))]);
    let mut client = CrossrefClient::new(&mut transport);
    assert!(client.lookup("10.1234/example.2024.001").is_none());
}

#[test]
fn lookup_is_case_insensitive_about_the_echoed_doi() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    assert!(client.lookup("10.1234/EXAMPLE.2024.001").is_some());
}

#[test]
fn title_lookup_uses_query_title_and_accepts_one_exact_match() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    let record = client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "",
            None,
            20,
            &["journal-article"],
        )
        .unwrap();

    assert_eq!(record.doi, "10.1234/example.2024.001");
    let url = transport.requests.borrow()[0].clone();
    assert!(url.contains("query.title"), "{url}");
    // query.bibliographic would bury an exact title under unrelated works.
    assert!(!url.contains("query.bibliographic"), "{url}");
}

#[test]
fn title_lookup_pushes_a_known_year_into_the_api_filter() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    client.lookup_title(
        "Deep Learning for Medical Imaging",
        "",
        Some(2024),
        20,
        &["journal-article"],
    );

    let url = transport.requests.borrow()[0]
        .replace("%2D", "-")
        .replace("%3A", ":");
    assert!(url.contains("from-pub-date:2023-01-01"), "{url}");
    assert!(url.contains("until-pub-date:2025-12-31"), "{url}");
}

#[test]
fn title_lookup_sends_no_date_filter_when_the_year_is_unknown() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    client.lookup_title(
        "Deep Learning for Medical Imaging",
        "",
        None,
        20,
        &["journal-article"],
    );

    let url = transport.requests.borrow()[0].clone();
    assert!(!url.contains("filter="), "{url}");
}

#[test]
fn title_lookup_disambiguates_among_same_titled_works() {
    let nature = json!({
        "DOI": "10.1038/nature14539",
        "type": "journal-article",
        "title": ["Deep learning"],
        "container-title": ["Nature"],
        "ISSN": ["0028-0836", "1476-4687"],
        "published-print": {"date-parts": [[2015, 5]]},
        "author": [{"family": "LeCun", "given": "Y"}],
    });
    let scholarpedia = json!({
        "DOI": "10.4249/scholarpedia.32832",
        "type": "journal-article",
        "title": ["Deep learning"],
        "author": [{"family": "Schmidhuber", "given": "J"}],
    });
    let chapter = json!({
        "DOI": "10.1007/978-1-4302-5990-9_9",
        "type": "book-chapter",
        "title": ["Deep Learning"],
        "author": [{"family": "LeCun", "given": "Y"}],
    });
    let mut transport = FakeTransport::json(vec![Ok(
        json!({"message": {"items": [chapter, scholarpedia, nature]}}),
    )]);
    let mut client = CrossrefClient::new(&mut transport);

    let record = client
        .lookup_title(
            "Deep learning",
            "Y LeCun - Nature, 2015",
            Some(2015),
            20,
            &["journal-article"],
        )
        .unwrap();

    assert_eq!(record.doi, "10.1038/nature14539");
    assert_eq!(record.journal, "Nature");
}

#[test]
fn default_title_rows_covers_same_titled_alternatives() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    client.lookup_title(
        "Deep Learning for Medical Imaging",
        "",
        None,
        20,
        &["journal-article"],
    );

    let url = transport.requests.borrow()[0].clone();
    assert!(url.contains("rows=20"), "{url}");
}

#[test]
fn title_lookup_rejects_a_near_miss_title() {
    let mut near_miss = journal_message();
    near_miss["title"] = json!(["Deep Learning for Medical Imaging Systems"]);
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": {"items": [near_miss]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    assert!(client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "",
            None,
            20,
            &["journal-article"]
        )
        .is_none());
}

#[test]
fn title_lookup_rejects_a_non_journal_work_with_an_identical_title() {
    let mut book_chapter = journal_message();
    book_chapter["type"] = json!("book-chapter");
    book_chapter["DOI"] = json!("10.1201/b17103-3");
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [book_chapter]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    assert!(client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "",
            None,
            20,
            &["journal-article"]
        )
        .is_none());
}

#[test]
fn title_lookup_ignores_a_non_journal_duplicate_of_a_valid_article() {
    let mut book_chapter = journal_message();
    book_chapter["type"] = json!("book-chapter");
    book_chapter["DOI"] = json!("10.1201/b17103-3");
    let mut transport = FakeTransport::json(vec![Ok(
        json!({"message": {"items": [book_chapter, journal_message()]}}),
    )]);
    let mut client = CrossrefClient::new(&mut transport);

    let record = client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "",
            None,
            20,
            &["journal-article"],
        )
        .unwrap();
    assert_eq!(record.doi, "10.1234/example.2024.001");
}

#[test]
fn title_lookup_rejects_two_distinct_dois_behind_one_title() {
    let mut duplicate = journal_message();
    duplicate["DOI"] = json!("10.1234/example.2024.002");
    let mut transport = FakeTransport::json(vec![Ok(
        json!({"message": {"items": [journal_message(), duplicate]}}),
    )]);
    let mut client = CrossrefClient::new(&mut transport);

    assert!(client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "",
            None,
            20,
            &["journal-article"]
        )
        .is_none());
}

#[test]
fn title_lookup_rejects_a_contradicting_author() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    assert!(client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "Z Zhang - Journal, 2024",
            None,
            20,
            &["journal-article"]
        )
        .is_none());
}

#[test]
fn title_lookup_rejects_a_contradicting_year() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);

    assert!(client
        .lookup_title(
            "Deep Learning for Medical Imaging",
            "",
            Some(2010),
            20,
            &["journal-article"]
        )
        .is_none());
}

// ---- EnrichmentTests ----

fn catalog() -> JournalCatalog {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ranks.csv");
    std::fs::write(
        &path,
        "Title,Issn,SJR Best Quartile,Year,Categories\nJournal of Examples,1234-5679,Q1,2023,Multidisciplinary\n",
    )
    .unwrap();
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();
    catalog
}

struct ClientLookup<'a, 'b> {
    client: &'a mut CrossrefClient<'b>,
}

impl ArticleLookup for ClientLookup<'_, '_> {
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
            .lookup_title(title, author, year, 20, work_types)
    }
}

#[test]
fn doi_article_gains_identity_and_rank() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let cat = catalog();
    let article = Article {
        title: "Deep Learning for Medical Imaging".into(),
        doi: "10.1234/example.2024.001".into(),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &[article],
        Some(&cat),
        None,
        None,
        &mut lookup,
        None,
        None,
        true,
    );

    assert_eq!(result[0].journal, "Journal of Examples");
    assert_eq!(result[0].issns, vec!["1234-5679".to_string()]);
    assert_eq!(result[0].sjr_quartile, "Q1");
    assert_eq!(result[0].sjr_year, Some(2023));
    assert_eq!(result[0].sjr_category, "Multidisciplinary");
}

#[test]
fn missing_doi_article_is_resolved_by_exact_title() {
    let mut transport =
        FakeTransport::json(vec![Ok(json!({"message": {"items": [journal_message()]}}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let cat = catalog();
    let article = Article {
        title: "Deep Learning for Medical Imaging".into(),
        link: "https://publisher.example/paper".into(),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &[article],
        Some(&cat),
        None,
        None,
        &mut lookup,
        None,
        None,
        true,
    );

    assert_eq!(result[0].doi, "10.1234/example.2024.001");
    assert_eq!(result[0].journal, "Journal of Examples");
    assert_eq!(result[0].sjr_quartile, "Q1");
}

#[test]
fn conference_article_never_receives_a_journal_rank() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": conference_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let cat = catalog();
    let article = Article {
        title: "A Conference Paper".into(),
        doi: "10.1234/conf.2023.001".into(),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &[article],
        Some(&cat),
        None,
        None,
        &mut lookup,
        None,
        None,
        true,
    );

    assert_eq!(result[0].journal, "");
    assert_eq!(result[0].sjr_quartile, "");
}

#[test]
fn unreachable_provider_leaves_the_article_unchanged() {
    let mut transport = FakeTransport::raw(vec![Err(())]);
    let mut client = CrossrefClient::new(&mut transport);
    let cat = catalog();
    let article = Article {
        title: "Deep Learning for Medical Imaging".into(),
        doi: "10.1234/example.2024.001".into(),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &[article.clone()],
        Some(&cat),
        None,
        None,
        &mut lookup,
        None,
        None,
        true,
    );

    assert_eq!(result[0], article);
    // Only the DOI request was made: no title-guess retry.
    assert_eq!(transport.requests.borrow().len(), 1);
    assert!(transport.requests.borrow()[0].contains("10.1234%2Fexample.2024.001"));
}

#[test]
fn failed_doi_lookup_is_never_retried_by_title() {
    let mut transport = FakeTransport::raw(vec![Err(())]);
    let mut client = CrossrefClient::new(&mut transport);
    let article = Article {
        title: "Deep Learning for Medical Imaging".into(),
        doi: "10.1234/example.2024.001".into(),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(&[article], None, None, None, &mut lookup, None, None, true);

    assert_eq!(result[0].journal, "");
    assert_eq!(result[0].doi, "10.1234/example.2024.001");
    assert!(!transport
        .requests
        .borrow()
        .iter()
        .any(|url| url.contains("query.bibliographic")));
}

#[test]
fn known_values_are_never_overwritten() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let article = Article {
        title: "Deep Learning for Medical Imaging".into(),
        doi: "10.1234/example.2024.001".into(),
        journal: "Already Known Journal".into(),
        issns: vec!["0028-0836".into()],
        publication_year: Some(1999),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(&[article], None, None, None, &mut lookup, None, None, true);

    assert_eq!(result[0].journal, "Already Known Journal");
    assert_eq!(result[0].issns, vec!["0028-0836".to_string()]);
    assert_eq!(result[0].publication_year, Some(1999));
}

#[test]
fn unverified_pdf_candidate_keeps_status_unknown() {
    let mut message = journal_message();
    message["license"] = json!([{"URL": "https://creativecommons.org/licenses/by/4.0/"}]);
    message["link"] =
        json!([{"URL": "https://repo.example/a.pdf", "content-type": "application/pdf"}]);
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": message}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let article = Article {
        title: "Deep Learning for Medical Imaging".into(),
        doi: "10.1234/example.2024.001".into(),
        ..Article::default()
    };
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(&[article], None, None, None, &mut lookup, None, None, true);

    assert_eq!(result[0].pdf_url, "https://repo.example/a.pdf");
    assert_eq!(result[0].pdf_status, "unknown");
    assert_ne!(result[0].pdf_status, "available");
}

#[test]
fn cancellation_keeps_handled_articles_and_returns_the_remainder() {
    struct SetCancel;
    impl gss_core::client::CancelEvent for SetCancel {
        fn is_set(&self) -> bool {
            true
        }
        fn wait(&self, _t: f64) -> bool {
            true
        }
    }
    let mut transport = FakeTransport::json(vec![]);
    let mut client = CrossrefClient::new(&mut transport);
    let cancel = SetCancel;
    let articles = vec![Article::new("First Paper"), Article::new("Second Paper")];
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &articles,
        None,
        Some(&cancel),
        None,
        &mut lookup,
        None,
        None,
        true,
    );

    assert_eq!(result, articles);
}

#[test]
fn progress_callback_reports_each_article() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let seen: Rc<RefCell<Vec<(usize, usize, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let mut progress = move |current: usize, total: usize, phase: &str| {
        sink.borrow_mut().push((current, total, phase.to_string()));
    };
    let articles = vec![Article {
        title: "Deep Learning for Medical Imaging".into(),
        doi: "10.1234/example.2024.001".into(),
        ..Article::default()
    }];
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    enrich_articles(
        &articles,
        None,
        None,
        Some(&mut progress),
        &mut lookup,
        None,
        None,
        true,
    );

    assert_eq!(
        *seen.borrow(),
        vec![(1, 1, "Deep Learning for Medical Imaging".to_string())]
    );
}

#[test]
fn lookup_budget_bounds_network_work() {
    let mut transport = FakeTransport::json(vec![]);
    let mut client = CrossrefClient::new(&mut transport);
    let articles = vec![
        Article {
            title: "First Paper".into(),
            doi: "10.1234/a".into(),
            ..Article::default()
        },
        Article {
            title: "Second Paper".into(),
            doi: "10.1234/b".into(),
            ..Article::default()
        },
    ];
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &articles,
        None,
        None,
        None,
        &mut lookup,
        Some(0),
        None,
        false,
    );

    assert_eq!(result, articles);
}

#[test]
fn same_doi_is_only_looked_up_once() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let articles = vec![
        Article {
            title: "Deep Learning for Medical Imaging".into(),
            doi: "10.1234/example.2024.001".into(),
            ..Article::default()
        },
        Article {
            title: "Deep Learning for Medical Imaging (reprint)".into(),
            doi: "10.1234/example.2024.001".into(),
            ..Article::default()
        },
    ];
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    enrich_articles(&articles, None, None, None, &mut lookup, None, None, true);

    assert_eq!(transport.requests.borrow().len(), 1);
}

#[test]
fn same_title_with_different_authors_and_years_gets_distinct_records() {
    let first = CrossrefRecord {
        doi: "10.1000/first".into(),
        journal: "Journal of First".into(),
        issns: vec!["1234-5679".into()],
        year: Some(2015),
        ..CrossrefRecord::default()
    };
    let second = CrossrefRecord {
        doi: "10.1000/second".into(),
        journal: "Journal of Second".into(),
        issns: vec!["1234-5687".into()],
        year: Some(2024),
        ..CrossrefRecord::default()
    };
    let mut client = RecordingLookup::with(
        HashMap::from([
            (("Same exact title", "First Author", Some(2015)), first),
            (("Same exact title", "Second Author", Some(2024)), second),
        ]),
        HashMap::new(),
    );
    let articles = vec![
        Article {
            title: "Same exact title".into(),
            authors: "First Author".into(),
            publication_year: Some(2015),
            ..Article::default()
        },
        Article {
            title: "Same exact title".into(),
            authors: "Second Author".into(),
            publication_year: Some(2024),
            ..Article::default()
        },
    ];

    let result = enrich_articles(&articles, None, None, None, &mut client, None, None, true);

    assert_eq!(result[0].doi, "10.1000/first");
    assert_eq!(result[0].journal, "Journal of First");
    assert_eq!(result[1].doi, "10.1000/second");
    assert_eq!(result[1].journal, "Journal of Second");
    assert_eq!(client.title_calls.borrow().len(), 2);
}

#[test]
fn identical_constraints_reuse_one_query() {
    let record = CrossrefRecord {
        doi: "10.1000/only".into(),
        journal: "Journal of Examples".into(),
        issns: vec!["1234-5679".into()],
        ..CrossrefRecord::default()
    };
    let mut client = RecordingLookup::with(
        HashMap::from([(("Shared title", "A Author", Some(2020)), record)]),
        HashMap::new(),
    );
    let articles = vec![
        Article {
            title: "Shared title".into(),
            authors: "A Author".into(),
            publication_year: Some(2020),
            ..Article::default()
        },
        Article {
            title: "shared   TITLE".into(),
            authors: "A Author".into(),
            publication_year: Some(2020),
            ..Article::default()
        },
    ];

    let result = enrich_articles(&articles, None, None, None, &mut client, None, None, true);

    assert_eq!(result[0].doi, "10.1000/only");
    assert_eq!(result[1].doi, "10.1000/only");
    assert_eq!(client.title_calls.borrow().len(), 1);
}

#[test]
fn cached_title_answer_survives_an_exhausted_budget() {
    let record = CrossrefRecord {
        doi: "10.1000/only".into(),
        journal: "Journal of Examples".into(),
        issns: vec!["1234-5679".into()],
        ..CrossrefRecord::default()
    };
    let mut client = RecordingLookup::with(
        HashMap::from([(("Shared title", "A Author", Some(2020)), record)]),
        HashMap::new(),
    );
    let articles = vec![
        Article {
            title: "Shared title".into(),
            authors: "A Author".into(),
            publication_year: Some(2020),
            ..Article::default()
        },
        Article {
            title: "shared title".into(),
            authors: "A Author".into(),
            publication_year: Some(2020),
            ..Article::default()
        },
    ];

    let result = enrich_articles(
        &articles,
        None,
        None,
        None,
        &mut client,
        None,
        Some(1),
        true,
    );

    assert_eq!(result[0].doi, "10.1000/only");
    assert_eq!(result[1].doi, "10.1000/only");
    assert_eq!(client.title_calls.borrow().len(), 1);
}

#[test]
fn exhausted_budget_blocks_only_a_genuinely_new_query() {
    let mut client = RecordingLookup::with(
        HashMap::from([
            (
                ("Shared title", "A Author", Some(2020)),
                CrossrefRecord {
                    doi: "10.1000/a".into(),
                    ..CrossrefRecord::default()
                },
            ),
            (
                ("Other title", "B Author", Some(2021)),
                CrossrefRecord {
                    doi: "10.1000/b".into(),
                    ..CrossrefRecord::default()
                },
            ),
        ]),
        HashMap::new(),
    );
    let articles = vec![
        Article {
            title: "Shared title".into(),
            authors: "A Author".into(),
            publication_year: Some(2020),
            ..Article::default()
        },
        Article {
            title: "Other title".into(),
            authors: "B Author".into(),
            publication_year: Some(2021),
            ..Article::default()
        },
    ];

    let result = enrich_articles(
        &articles,
        None,
        None,
        None,
        &mut client,
        None,
        Some(1),
        true,
    );

    assert_eq!(result[0].doi, "10.1000/a");
    assert_eq!(result[1].doi, "");
    assert_eq!(client.title_calls.borrow().len(), 1);
}

#[test]
fn cached_doi_answer_survives_an_exhausted_budget() {
    let mut transport = FakeTransport::json(vec![Ok(json!({"message": journal_message()}))]);
    let mut client = CrossrefClient::new(&mut transport);
    let articles = vec![
        Article {
            title: "Deep Learning for Medical Imaging".into(),
            doi: "10.1234/example.2024.001".into(),
            ..Article::default()
        },
        Article {
            title: "Deep Learning for Medical Imaging (reprint)".into(),
            doi: "10.1234/example.2024.001".into(),
            ..Article::default()
        },
    ];
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(
        &articles,
        None,
        None,
        None,
        &mut lookup,
        Some(1),
        None,
        true,
    );

    assert_eq!(result[0].journal, "Journal of Examples");
    assert_eq!(result[1].journal, "Journal of Examples");
    assert_eq!(transport.requests.borrow().len(), 1);
}

#[test]
fn title_lookup_can_be_disabled() {
    let mut transport = FakeTransport::json(vec![]);
    let mut client = CrossrefClient::new(&mut transport);
    let articles = vec![Article::new("Deep Learning for Medical Imaging")];
    let mut lookup = ClientLookup {
        client: &mut client,
    };

    let result = enrich_articles(&articles, None, None, None, &mut lookup, None, None, false);

    assert_eq!(result, articles);
    assert!(transport.requests.borrow().is_empty());
}

// ---- CitationFieldTests ----

#[test]
fn proceedings_record_carries_citation_fields_without_a_journal_label() {
    let message = json!({
        "DOI": "10.1145/conf.2023.001",
        "type": "proceedings-article",
        "title": ["A Conference Paper"],
        "container-title": ["Conference on Examples"],
        "ISSN": ["1234-5679"],
        "publisher": "ACM",
        "volume": "2023",
        "issue": "1",
        "page": "1007-1016",
        "URL": "https://doi.org/10.1145/conf.2023.001",
        "published-print": {"date-parts": [[2023, 7]]},
        "author": [
            {"given": "Songgaojun", "family": "Deng"},
            {"given": "Huzefa", "family": "Rangwala"},
        ],
    });

    let record = record_from_crossref_message(&message, "");

    assert_eq!(record.authors, vec!["Songgaojun Deng", "Huzefa Rangwala"]);
    assert_eq!(record.container_any, "Conference on Examples");
    assert_eq!(record.volume, "2023");
    assert_eq!(record.issue, "1");
    assert_eq!(record.pages, "1007-1016");
    assert_eq!(record.publisher, "ACM");
    assert_eq!(record.url, "https://doi.org/10.1145/conf.2023.001");
    assert_eq!(record.issns_any, vec!["1234-5679".to_string()]);
    assert_eq!(record.journal, "");
    assert!(record.issns.is_empty());
}

#[test]
fn title_select_widened_with_citation_fields() {
    for field in ["volume", "issue", "page", "publisher", "URL"] {
        assert!(gss_core::crossref::TITLE_SELECT.contains(field), "{field}");
    }
}

// ---- ResolveCitationsTests ----

#[test]
fn doi_hit_builds_a_verified_record() {
    let mut client = RecordingLookup::with(
        HashMap::new(),
        HashMap::from([(
            "10.1234/example.2024.001",
            CrossrefRecord {
                doi: "10.1234/example.2024.001".into(),
                journal: "Journal of Examples".into(),
                authors: vec!["A Author".into()],
                container_any: "Journal of Examples".into(),
                issns_any: vec!["1234-5679".into()],
                year: Some(2024),
                volume: "12".into(),
                issue: "3".into(),
                pages: "1-9".into(),
                publisher: "Examples Press".into(),
                url: "https://doi.org/10.1234/example.2024.001".into(),
                work_type: "journal-article".into(),
                ..CrossrefRecord::default()
            },
        )]),
    );

    let result = resolve_citations(
        &[Article {
            title: "Deep Learning for Medical Imaging".into(),
            doi: "10.1234/example.2024.001".into(),
            ..Article::default()
        }],
        None,
        None,
        &mut client,
        None,
        None,
    );

    assert_eq!(result.len(), 1);
    let citation = &result[0];
    assert!(citation.verified);
    assert_eq!(citation.source, "crossref");
    assert_eq!(citation.authors, vec!["A Author".to_string()]);
    assert_eq!(citation.container, "Journal of Examples");
    assert_eq!(citation.volume, "12");
    assert_eq!(citation.issue, "3");
    assert_eq!(citation.pages, "1-9");
    assert_eq!(citation.publisher, "Examples Press");
    assert_eq!(citation.entry_type, "journal-article");
    assert!(citation.warnings.is_empty());
}

#[test]
fn doi_miss_stays_unresolved_without_a_title_fallback() {
    let mut client = RecordingLookup::new();

    let result = resolve_citations(
        &[Article {
            title: "Deep Learning for Medical Imaging".into(),
            authors: "A Author, B Author - Journal of Examples, 2024 - publisher.example".into(),
            doi: "10.1234/missing.2024.001".into(),
            ..Article::default()
        }],
        None,
        None,
        &mut client,
        None,
        None,
    );

    let citation = &result[0];
    assert!(!citation.verified);
    assert_eq!(citation.source, "scholar");
    assert!(citation.authors.is_empty());
    assert_eq!(
        citation.fallback_authors_line,
        "A Author, B Author - Journal of Examples, 2024 - publisher.example"
    );
    assert!(citation.warnings.contains(&UNRESOLVED_WARNING.to_string()));
    assert!(citation
        .warnings
        .contains(&SCHOLAR_AUTHORS_WARNING.to_string()));
    // A DOI failure must not fall back to a title guess.
    assert!(client.title_calls.borrow().is_empty());
}

#[test]
fn title_path_resolves_proceedings_and_journal_gate_untouched() {
    let mut client = RecordingLookup::with(
        HashMap::from([(
            ("A Conference Paper", "", None),
            CrossrefRecord {
                doi: "10.1145/conf.2023.001".into(),
                authors: vec!["S Deng".into()],
                container_any: "Conference on Examples".into(),
                year: Some(2023),
                work_type: "proceedings-article".into(),
                ..CrossrefRecord::default()
            },
        )]),
        HashMap::new(),
    );

    let result = resolve_citations(
        &[Article::new("A Conference Paper")],
        None,
        None,
        &mut client,
        None,
        None,
    );

    assert!(result[0].verified);
    assert_eq!(result[0].container, "Conference on Examples");
}

#[test]
fn same_doi_is_looked_up_once() {
    let mut client = RecordingLookup::with(
        HashMap::new(),
        HashMap::from([(
            "10.1000/same",
            CrossrefRecord {
                doi: "10.1000/same".into(),
                ..CrossrefRecord::default()
            },
        )]),
    );
    let articles = vec![
        Article {
            title: "First".into(),
            doi: "10.1000/same".into(),
            ..Article::default()
        },
        Article {
            title: "Second".into(),
            doi: "10.1000/same".into(),
            ..Article::default()
        },
    ];

    let result = resolve_citations(&articles, None, None, &mut client, None, None);

    assert_eq!(result.len(), 2);
    assert_eq!(*client.doi_calls.borrow(), vec!["10.1000/same".to_string()]);
}

#[test]
fn citation_work_types_gate_allows_proceedings() {
    // The title path for citations must accept proceedings-article.
    assert!(CITATION_WORK_TYPES.contains(&"proceedings-article"));
    assert!(CITATION_WORK_TYPES.contains(&"journal-article"));
    assert_eq!(CITATION_WORK_TYPES.len(), 2);
}

#[test]
fn unresolved_record_has_both_warnings() {
    let article = Article {
        title: "An Unresolved Paper".into(),
        authors: "A Author - Journal, 2024".into(),
        ..Article::default()
    };
    let mut client = RecordingLookup::new();
    let result = resolve_citations(&[article], None, None, &mut client, None, None);
    let citation: &CitationRecord = &result[0];
    assert_eq!(
        citation.warnings,
        vec![
            UNRESOLVED_WARNING.to_string(),
            SCHOLAR_AUTHORS_WARNING.to_string()
        ]
    );
}
