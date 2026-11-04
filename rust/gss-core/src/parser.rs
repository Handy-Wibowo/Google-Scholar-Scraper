//! Scholar HTML parsing + page classification ported from `scraper/parser.py`
//! verbatim. Selection semantics follow BeautifulSoup("html.parser") for the
//! constructs Scholar serves; verified per-fixture by Gate 2 parity.

use crate::models::{normalize_doi, Article, ExtractionStatus};
use crate::text::CaseFold;
use regex::Regex;
use scraper::node::Element;
use scraper::{CaseSensitivity, Html, Node};
use std::sync::LazyLock;

static DOI_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^/(?:doi/)?(?:(?:abs|full|pdf|epdf|meta|abstract)/)*?(10\.\d{4,9}/[^\s?#]+)")
        .unwrap()
});
static YEAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(1[5-9]\d\d|2[01]\d\d)\b").unwrap());

const DOI_HOSTS: [&str; 4] = ["doi.org", "dx.doi.org", "www.doi.org", "www.dx.doi.org"];
/// Structural classification labels removed from citation-only headings.
const SCHOLAR_LABEL_CLASSES: [&str; 2] = ["gs_ctc", "gs_ctu"];

#[derive(Debug, Clone, PartialEq)]
pub struct PageParseResult {
    pub status: ExtractionStatus,
    pub articles: Vec<Article>,
    pub message: String,
    pub diagnostic: String,
}

pub fn parse_scholar_articles(html: &str) -> Vec<Article> {
    parse_scholar_page(html, 200).articles
}

pub fn parse_scholar_page(html: &str, status_code: u16) -> PageParseResult {
    let page_status = classify_scholar_page(html, status_code);
    if page_status != ExtractionStatus::Success {
        return PageParseResult {
            status: page_status,
            articles: Vec::new(),
            message: message_for_status(page_status).to_string(),
            diagnostic: String::new(),
        };
    }

    let document = Html::parse_document(html);
    let mut articles: Vec<Article> = Vec::new();
    let mut skipped_missing_title = 0;

    for row in find_all(*document.root_element(), &|el| {
        el.name() == "div" && el.has_class("gs_ri", CaseSensitivity::CaseSensitive)
    }) {
        let Some(title_node) = find_first(row, &|el| {
            el.name() == "h3" && el.has_class("gs_rt", CaseSensitivity::CaseSensitive)
        }) else {
            skipped_missing_title += 1;
            continue;
        };
        let (title, link) = scholar_title_and_link(title_node);
        if title.is_empty() {
            skipped_missing_title += 1;
            continue;
        }

        let authors = match find_first(row, &|el| {
            el.name() == "div" && el.has_class("gs_a", CaseSensitivity::CaseSensitive)
        }) {
            Some(authors_node) => bs_text(authors_node, false),
            None => String::new(),
        };

        let wrapper = find_closest_ancestor(row, &|el| {
            el.name() == "div" && el.has_class("gs_r", CaseSensitivity::CaseSensitive)
        })
        .unwrap_or(row);
        let candidate_pdf_url = scholar_pdf_candidate(wrapper);
        let publication_year = conservative_year(&authors);
        articles.push(Article {
            title,
            authors: authors.clone(),
            link,
            relevance_score: None,
            doi: first_doi_url(wrapper),
            journal: String::new(),
            issns: Vec::new(),
            publication_year,
            sjr_quartile: String::new(),
            sjr_year: None,
            sjr_category: String::new(),
            sjr_source_url: String::new(),
            sinta_rank: String::new(),
            sinta_year: None,
            sinta_source_url: String::new(),
            pdf_url: candidate_pdf_url.clone(),
            pdf_source_url: String::new(),
            pdf_status: crate::models::PDF_STATUS_UNKNOWN.to_string(),
            pdf_detail: if candidate_pdf_url.is_empty() {
                String::new()
            } else {
                "Candidate link declared by Google Scholar; not verified.".to_string()
            },
            pdf_local_path: String::new(),
        });
    }

    if articles.is_empty() {
        return PageParseResult {
            status: ExtractionStatus::ParsingError,
            articles: Vec::new(),
            message:
                "Google Scholar returned result containers, but no valid article titles were found."
                    .to_string(),
            diagnostic: format!("skipped_missing_title={skipped_missing_title}"),
        };
    }

    let diagnostic = if skipped_missing_title > 0 {
        format!("skipped_missing_title={skipped_missing_title}")
    } else {
        String::new()
    };

    PageParseResult {
        status: ExtractionStatus::Success,
        articles,
        message: "Parsed Google Scholar results.".to_string(),
        diagnostic,
    }
}

/// Title extraction (`_scholar_title`): anchor text wins; citation-only
/// headings drop ONLY the gs_ctc/gs_ctu label spans. Second return value is
/// the heading's first anchor href (post-decompose tree), "" when absent.
fn scholar_title_and_link(title_node: ego_tree::NodeRef<'_, Node>) -> (String, String) {
    if let Some(anchor) = find_first(title_node, &|el| el.name() == "a") {
        let text = bs_text(anchor, false);
        if !text.is_empty() {
            let link = anchor_href(anchor);
            return (text, link);
        }
    }
    let text = bs_text(title_node, true);
    // find("a") after decompose: any anchor NOT nested inside a label span.
    let link = title_node
        .descendants()
        .find(|n| {
            n.id() != title_node.id()
                && matches!(element_of(*n), Some(el) if el.name() == "a")
                && !inside_label(title_node, *n)
        })
        .map(anchor_href)
        .unwrap_or_default();
    (text, link)
}

fn inside_label(root: ego_tree::NodeRef<'_, Node>, target: ego_tree::NodeRef<'_, Node>) -> bool {
    let mut current = target.parent();
    while let Some(node) = current {
        if node.id() == root.id() {
            return false;
        }
        if let Node::Element(el) = node.value() {
            if SCHOLAR_LABEL_CLASSES.contains(&el.name())
                || SCHOLAR_LABEL_CLASSES
                    .iter()
                    .any(|c| el.has_class(c, CaseSensitivity::CaseSensitive))
            {
                return true;
            }
        }
        current = node.parent();
    }
    false
}

fn anchor_href(node: ego_tree::NodeRef<'_, Node>) -> String {
    if let Node::Element(el) = node.value() {
        el.attr("href").unwrap_or("").to_string()
    } else {
        String::new()
    }
}

/// BeautifulSoup `get_text(" ", strip=True)`: all descendant strings in
/// document order, each trimmed, empties dropped, joined with single spaces.
/// `skip_labels` removes gs_ctc/gs_ctu subtrees first (mirrors decompose()).
fn bs_text(root: ego_tree::NodeRef<'_, Node>, skip_labels: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    collect_strings(root, skip_labels, &mut parts);
    parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_strings(root: ego_tree::NodeRef<'_, Node>, skip_labels: bool, out: &mut Vec<String>) {
    for child in root.children() {
        match child.value() {
            Node::Text(t) => out.push(t.to_string()),
            Node::Element(el) => {
                if skip_labels
                    && SCHOLAR_LABEL_CLASSES
                        .iter()
                        .any(|c| el.has_class(c, CaseSensitivity::CaseSensitive))
                {
                    continue;
                }
                collect_strings(child, skip_labels, out);
            }
            _ => {}
        }
    }
}

/// Right-column `[PDF]` marker URL (`_scholar_pdf_candidate`). Only a real
/// http(s) target counts; `javascript:` expanders are controls, not documents.
fn scholar_pdf_candidate(wrapper: ego_tree::NodeRef<'_, Node>) -> String {
    for anchor in find_all(wrapper, &|el| el.name() == "a" && el.attr("href").is_some()) {
        let label = bs_text(anchor, false).casefold();
        if !label.starts_with("[pdf]") {
            continue;
        }
        let href = anchor_href(anchor);
        let href = href.trim();
        if let Some(parsed) = crate::url::split(href) {
            if parsed.scheme.casefold() == "http" || parsed.scheme.casefold() == "https" {
                return href.to_string();
            }
        }
    }
    String::new()
}

fn first_doi_url(wrapper: ego_tree::NodeRef<'_, Node>) -> String {
    for anchor in find_all(wrapper, &|el| el.name() == "a" && el.attr("href").is_some()) {
        let href = anchor_href(anchor);
        let doi = doi_from_url(&href);
        if !doi.is_empty() {
            return doi;
        }
    }
    String::new()
}

fn doi_from_url(href: &str) -> String {
    let Some(parsed) = crate::url::split(href.trim()) else {
        return String::new();
    };
    if parsed.scheme != "http" && parsed.scheme != "https" {
        return String::new();
    }
    if !DOI_HOSTS.contains(&parsed.netloc.casefold().as_str()) {
        return String::new();
    }
    let path = parsed.path.trim_end_matches('/');
    match DOI_URL_RE.captures(path) {
        Some(caps) => normalize_doi(&caps[1]),
        None => String::new(),
    }
}

/// Publication year only when the citation line names exactly one
/// (1500–2099 by the regex) year (`_conservative_year`).
fn conservative_year(text: &str) -> Option<i32> {
    let mut years: Vec<i32> = Vec::new();
    for cap in YEAR_RE.find_iter(text) {
        let y: i32 = cap.as_str().parse().unwrap();
        if !years.contains(&y) {
            years.push(y);
        }
    }
    if years.len() != 1 {
        return None;
    }
    years.pop()
}

pub fn classify_scholar_page(html: &str, status_code: u16) -> ExtractionStatus {
    if status_code == 429 {
        return ExtractionStatus::RateLimited;
    }
    if status_code == 401 || status_code == 403 {
        return ExtractionStatus::Blocked;
    }
    if !(200..300).contains(&status_code) {
        return ExtractionStatus::NetworkError;
    }

    let document = Html::parse_document(html);
    if !find_all(*document.root_element(), &|el| {
        el.name() == "div" && el.has_class("gs_ri", CaseSensitivity::CaseSensitive)
    })
    .is_empty()
    {
        return ExtractionStatus::Success;
    }

    let text = html.to_lowercase();

    if has_rate_limit_signal(&text) {
        return ExtractionStatus::RateLimited;
    }
    if has_blocked_signal(&text) {
        return ExtractionStatus::Blocked;
    }
    if is_results_page_shell(&document) {
        return ExtractionStatus::NoResults;
    }
    if has_no_results_signal(&text) {
        return ExtractionStatus::NoResults;
    }
    if has_network_error_signal(&text) {
        return ExtractionStatus::NetworkError;
    }
    ExtractionStatus::ParsingError
}

fn has_rate_limit_signal(text: &str) -> bool {
    ["too many requests", "rate limit", "temporarily unavailable"]
        .iter()
        .any(|s| text.contains(s))
}

fn has_blocked_signal(text: &str) -> bool {
    [
        "captcha",
        "recaptcha",
        "our systems have detected unusual traffic",
        "unusual traffic from your computer network",
        "not a robot",
        "/sorry/",
        "sorry, but your computer or network may be sending automated queries",
        "before you continue to google",
        "consent.google",
    ]
    .iter()
    .any(|s| text.contains(s))
}

/// A real Scholar results page always carries the result-count header even
/// when it renders zero entries → empty result page, not a failure.
fn is_results_page_shell(document: &Html) -> bool {
    !find_all(*document.root_element(), &|el| {
        el.name() == "div" && el.attr("id") == Some("gs_ab_md")
    })
    .is_empty()
}

fn has_network_error_signal(text: &str) -> bool {
    [
        "internal server error",
        "the system can't perform the operation now",
        "try again later",
    ]
    .iter()
    .any(|s| text.contains(s))
}

fn has_no_results_signal(text: &str) -> bool {
    [
        "did not match any articles",
        "did not match any results",
        "try different keywords",
        "try fewer keywords",
    ]
    .iter()
    .any(|s| text.contains(s))
}

pub fn message_for_status(status: ExtractionStatus) -> &'static str {
    match status {
        ExtractionStatus::NoResults => {
            "Google Scholar returned a results page with no article entries."
        }
        ExtractionStatus::RateLimited => "Google Scholar rate-limited the request.",
        ExtractionStatus::Blocked => "Google Scholar returned a block, consent, or challenge page.",
        ExtractionStatus::NetworkError => "Google Scholar returned an unsuccessful HTTP response.",
        ExtractionStatus::ParsingError => {
            "Google Scholar returned a page that could not be parsed as results."
        }
        ExtractionStatus::Cancelled => "Extraction was cancelled.",
        _ => "Google Scholar page was classified.",
    }
}

// ---- tree helpers over ego_tree (BeautifulSoup find_all/find semantics) ----

pub(crate) type NodeRef<'a> = ego_tree::NodeRef<'a, Node>;

fn element_of(node: NodeRef<'_>) -> Option<&Element> {
    match node.value() {
        Node::Element(el) => Some(el),
        _ => None,
    }
}

fn find_all<'a>(root: NodeRef<'a>, pred: &dyn Fn(&Element) -> bool) -> Vec<NodeRef<'a>> {
    let mut out = Vec::new();
    collect_matches(root, pred, &mut out);
    out
}

fn collect_matches<'a>(
    node: NodeRef<'a>,
    pred: &dyn Fn(&Element) -> bool,
    out: &mut Vec<NodeRef<'a>>,
) {
    for child in node.descendants() {
        if let Some(el) = element_of(child) {
            if pred(el) {
                out.push(child);
            }
        }
    }
}

fn find_first<'a>(root: NodeRef<'a>, pred: &dyn Fn(&Element) -> bool) -> Option<NodeRef<'a>> {
    find_all(root, pred).into_iter().next()
}

/// `find_parent`: closest STRICT ancestor matching the predicate.
fn find_closest_ancestor<'a>(
    node: NodeRef<'a>,
    pred: &dyn Fn(&Element) -> bool,
) -> Option<NodeRef<'a>> {
    let mut current = node.parent();
    while let Some(n) = current {
        if let Some(el) = element_of(n) {
            if pred(el) {
                return Some(n);
            }
        }
        current = n.parent();
    }
    None
}
