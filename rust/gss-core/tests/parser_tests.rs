//! Port of `tests/test_parser.py` 1:1 — all 21 cases against the verbatim fixtures.

use gss_core::models::ExtractionStatus;
use gss_core::parser::{classify_scholar_page, parse_scholar_page};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).expect("fixture readable")
}

#[test]
fn successful_multiple_result_parsing() {
    let result = parse_scholar_page(&fixture("scholar_results_basic.html"), 200);
    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.articles.len(), 2);
    assert_eq!(result.articles[0].title, "Example Paper One");
    assert_eq!(result.articles[0].authors, "A Author - Journal, 2024");
    assert_eq!(result.articles[0].link, "https://example.edu/paper-one");
}

#[test]
fn missing_optional_metadata_and_link_do_not_crash() {
    let result = parse_scholar_page(&fixture("scholar_results_missing_optional.html"), 200);
    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(
        result.articles[0].title,
        "Citation only result without external link"
    );
    assert_eq!(result.articles[0].authors, "");
    assert_eq!(result.articles[0].link, "");
}

#[test]
fn scholar_classification_labels_are_stripped_from_titles() {
    let result = parse_scholar_page(&fixture("scholar_results_labels.html"), 200);
    assert_eq!(
        result.articles[0].title,
        "A Machine-Learning-Powered Real-Time SIEM Framework"
    );
    assert_eq!(
        result.articles[1].title,
        "The malware detection challenge of accuracy"
    );
    for article in &result.articles {
        assert!(!article.title.starts_with('['), "{}", article.title);
        assert!(!article.title.contains("[PDF]"), "{}", article.title);
        assert!(!article.title.contains("[HTML]"), "{}", article.title);
        assert!(!article.title.contains("[CITATION]"), "{}", article.title);
    }
}

#[test]
fn citation_only_result_drops_only_the_structural_label() {
    let result = parse_scholar_page(&fixture("scholar_results_labels.html"), 200);
    assert_eq!(
        result.articles[2].title,
        "Citation only result with a Scholar label"
    );
    assert_eq!(result.articles[2].link, "");
}

#[test]
fn genuine_brackets_and_emphasis_inside_a_title_are_preserved() {
    let result = parse_scholar_page(&fixture("scholar_results_labels.html"), 200);
    assert_eq!(
        result.articles[3].title,
        "Structural analysis of [RNA] complexes in solution"
    );
}

#[test]
fn ui_expander_is_never_mistaken_for_a_pdf_candidate() {
    let result = parse_scholar_page(&fixture("scholar_results_labels.html"), 200);
    assert_eq!(
        result.articles[0].pdf_url,
        "https://repo.example.org/machine-learning.pdf"
    );
    assert_eq!(result.articles[0].pdf_status, "unknown");
    for article in &result.articles {
        assert!(
            !article.pdf_url.starts_with("javascript:"),
            "{}",
            article.pdf_url
        );
    }
}

#[test]
fn declared_pdf_link_is_captured_but_left_unverified() {
    let result = parse_scholar_page(&fixture("scholar_results_metadata.html"), 200);
    let paper_one = &result.articles[0];
    assert_eq!(paper_one.pdf_url, "https://repo.example.org/paper-one.pdf");
    assert_eq!(paper_one.pdf_status, "unknown");
    assert!(paper_one.pdf_detail.contains("not verified"));
}

#[test]
fn html_and_unbracketed_labels_are_never_treated_as_pdf() {
    let result = parse_scholar_page(&fixture("scholar_results_metadata.html"), 200);
    // Paper Two declares only [HTML]; Paper Four has a bare "PDF" with no brackets.
    assert_eq!(result.articles[1].pdf_url, "");
    assert_eq!(result.articles[3].pdf_url, "");
}

#[test]
fn explicit_doi_link_is_captured_and_others_are_not() {
    let result = parse_scholar_page(&fixture("scholar_results_metadata.html"), 200);
    assert_eq!(result.articles[0].doi, "10.1234/example.2024.001");
    assert_eq!(result.articles[1].doi, "");
}

#[test]
fn publication_year_requires_an_unambiguous_citation_year() {
    let result = parse_scholar_page(&fixture("scholar_results_metadata.html"), 200);
    assert_eq!(result.articles[0].publication_year, Some(2024));
    assert_eq!(result.articles[1].publication_year, Some(2023));
    // Paper Three's citation spans "2020 - 2022": two years, so neither is claimed.
    assert_eq!(result.articles[2].publication_year, None);
}

#[test]
fn journal_is_never_fabricated_from_the_citation_line() {
    let result = parse_scholar_page(&fixture("scholar_results_metadata.html"), 200);
    for article in &result.articles {
        assert_eq!(article.journal, "", "{}", article.title);
    }
}

#[test]
fn parser_preserves_raw_article_text_for_data_quality_layer() {
    let result = parse_scholar_page(&fixture("scholar_results_duplicates.html"), 200);
    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.articles.len(), 2);
    assert!(result.articles[0].title.contains('\n'));
}

#[test]
fn genuine_no_results_detection() {
    let result = parse_scholar_page(&fixture("scholar_results_empty.html"), 200);
    assert_eq!(result.status, ExtractionStatus::NoResults);
    assert!(result.articles.is_empty());
}

#[test]
fn rate_limit_text_detection() {
    let result = parse_scholar_page(&fixture("scholar_rate_limited.html"), 200);
    assert_eq!(result.status, ExtractionStatus::RateLimited);
}

#[test]
fn http_429_detection() {
    let status = classify_scholar_page("<html><body>anything</body></html>", 429);
    assert_eq!(status, ExtractionStatus::RateLimited);
}

#[test]
fn blocked_challenge_detection() {
    let result = parse_scholar_page(&fixture("scholar_blocked.html"), 200);
    assert_eq!(result.status, ExtractionStatus::Blocked);
}

#[test]
fn incompatible_page_is_parsing_error() {
    let result = parse_scholar_page(&fixture("scholar_incompatible.html"), 200);
    assert_eq!(result.status, ExtractionStatus::ParsingError);
}

#[test]
fn results_page_carrying_hidden_error_template_is_still_success() {
    let html = concat!(
        "<html><head><title>Google Scholar</title></head><body>",
        "<div id=\"gs_md_err\" style=\"display:none\">",
        "The system can&#39;t perform the operation now. Try again later.</div>",
        "<div class=\"gs_r gs_or gs_scl\"><div class=\"gs_ri\">",
        "<h3 class=\"gs_rt\"><a href=\"https://example.edu/paper\">Real result title</a></h3>",
        "<div class=\"gs_a\">A Author - Journal, 2024</div>",
        "</div></div></body></html>"
    );
    let result = parse_scholar_page(html, 200);
    assert_eq!(result.status, ExtractionStatus::Success);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.articles[0].title, "Real result title");
}

#[test]
fn empty_results_page_carrying_hidden_error_template_is_no_results() {
    let html = concat!(
        "<html><head><title>Google Scholar</title></head><body>",
        "<div id=\"gs_md_err\" style=\"display:none\">",
        "The system can&#39;t perform the operation now. Try again later.</div>",
        "<div>Your search did not match any articles.</div>",
        "</body></html>"
    );
    let result = parse_scholar_page(html, 200);
    assert_eq!(result.status, ExtractionStatus::NoResults);
}

#[test]
fn live_style_empty_results_page_is_no_results_not_network_error() {
    let html = concat!(
        "<html><head><title>Google Scholar</title></head><body>",
        "<div id=\"gs_md_err\" style=\"display:none\">",
        "The system can&#39;t perform the operation now. Try again later.</div>",
        "<div id=\"gs_ab_md\">Page 2 of 7 results (0,06 sec)</div>",
        "<div id=\"gs_res_ccl\"><div id=\"gs_res_ccl_mid\"></div></div>",
        "</body></html>"
    );
    let result = parse_scholar_page(html, 200);
    assert_eq!(result.status, ExtractionStatus::NoResults);
    assert!(result.articles.is_empty());
}

#[test]
fn google_internal_server_error_page_is_network_error() {
    let html = r#"
        <html><head><title>Google Scholar</title></head>
        <body>The system can't perform the operation now. Try again later.
        Server Error We're sorry but it appears that there has been an internal server error.</body></html>
        "#;
    let result = parse_scholar_page(html, 200);
    assert_eq!(result.status, ExtractionStatus::NetworkError);
}
