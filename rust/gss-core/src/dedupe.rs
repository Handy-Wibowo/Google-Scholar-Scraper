//! Deduplication + record normalization ported from `dedupe.py` verbatim.

use crate::models::{
    normalize_doi, normalize_issn_list, Article, PDF_STATUS_AVAILABLE, PDF_STATUS_UNKNOWN,
};
use crate::text::CaseFold;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

#[derive(Debug, Clone, PartialEq)]
pub struct DeduplicationResult {
    pub articles: Vec<Article>,
    pub duplicates_removed: i32,
    pub invalid_removed: i32,
}

/// Whitespace-collapsed, trimmed display text (`normalize_display_text`).
pub fn normalize_display_text(value: &str) -> String {
    WHITESPACE_RE.replace_all(value, " ").trim().to_string()
}

/// Scheme/netloc lowercased; relative or scheme-less links unchanged
/// (`normalize_link`). Fragment is always dropped.
pub fn normalize_link(value: &str) -> String {
    let link = value.trim();
    if link.is_empty() {
        return String::new();
    }
    match crate::url::split(link) {
        Some(parsed) => {
            if parsed.scheme.is_empty() || parsed.netloc.is_empty() {
                link.to_string()
            } else {
                crate::url::unsplit(
                    &parsed.scheme.to_lowercase(),
                    &parsed.netloc.to_lowercase(),
                    &parsed.path,
                    &parsed.query,
                    "",
                )
            }
        }
        None => link.to_string(),
    }
}

/// Positive PDF status is only meaningful with a candidate or verified location.
pub fn normalize_pdf_status(value: &str) -> String {
    let status = normalize_display_text(value).to_lowercase();
    if crate::models::PDF_STATUSES.contains(&status.as_str()) {
        status
    } else {
        PDF_STATUS_UNKNOWN.to_string()
    }
}

/// Field-level normalization pass (`normalize_article`).
pub fn normalize_article(article: &Article) -> Article {
    let pdf_url = normalize_link(&article.pdf_url);
    let pdf_source_url = normalize_link(&article.pdf_source_url);
    let mut pdf_status = normalize_pdf_status(&article.pdf_status);
    if pdf_status == PDF_STATUS_AVAILABLE && pdf_url.is_empty() && pdf_source_url.is_empty() {
        // `available` is a positive claim: with no candidate and no verified
        // location the record names nothing it checked → downgraded.
        pdf_status = PDF_STATUS_UNKNOWN.to_string();
    }
    Article {
        title: normalize_display_text(&article.title),
        authors: normalize_display_text(&article.authors),
        link: normalize_link(&article.link),
        relevance_score: article.relevance_score,
        doi: normalize_doi(&article.doi),
        journal: normalize_display_text(&article.journal),
        issns: normalize_issn_list(article.issns.iter().map(|s| s.as_str())),
        publication_year: article.publication_year,
        sjr_quartile: normalize_display_text(&article.sjr_quartile).to_uppercase(),
        sjr_year: article.sjr_year,
        sjr_category: normalize_display_text(&article.sjr_category),
        sjr_source_url: normalize_link(&article.sjr_source_url),
        sinta_rank: normalize_display_text(&article.sinta_rank).to_uppercase(),
        sinta_year: article.sinta_year,
        sinta_source_url: normalize_link(&article.sinta_source_url),
        pdf_url,
        pdf_source_url,
        pdf_status,
        pdf_detail: normalize_display_text(&article.pdf_detail),
        pdf_local_path: article.pdf_local_path.trim().to_string(),
    }
}

pub fn is_valid_article(article: &Article) -> bool {
    !normalize_display_text(&article.title).is_empty()
}

/// NFKC → whitespace-collapse → casefold (`_comparison_text_key`).
fn comparison_text_key(value: &str) -> String {
    let normalized: String = value.nfkc().collect();
    normalize_display_text(&normalized).casefold()
}

pub fn title_key(article: &Article) -> String {
    comparison_text_key(&article.title)
}

/// Lowercased scheme+host link with http(s) scheme only (`canonical_link`).
pub fn canonical_link(article: &Article) -> String {
    let link = normalize_link(&article.link);
    match crate::url::split(&link) {
        Some(parsed)
            if (parsed.scheme == "http" || parsed.scheme == "https")
                && !parsed.netloc.is_empty() =>
        {
            link
        }
        _ => String::new(),
    }
}

/// Merge duplicates by canonical link, then title key; first occurrence wins
/// position; merges are bundle-coherent (`deduplicate_articles`).
pub fn deduplicate_articles<I: IntoIterator<Item = Article>>(articles: I) -> DeduplicationResult {
    let mut deduped: Vec<Article> = Vec::new();
    let mut link_to_index: HashMap<String, usize> = HashMap::new();
    let mut title_to_index: HashMap<String, usize> = HashMap::new();
    let mut duplicates_removed = 0;
    let mut invalid_removed = 0;

    for article in articles {
        let normalized = normalize_article(&article);
        if !is_valid_article(&normalized) {
            invalid_removed += 1;
            continue;
        }

        let link = canonical_link(&normalized);
        let title = title_key(&normalized);
        let existing_index = find_existing_index(&link, &title, &link_to_index, &title_to_index);

        let Some(existing_index) = existing_index else {
            let index = deduped.len();
            deduped.push(normalized);
            if !link.is_empty() {
                link_to_index.insert(link, index);
            }
            if !title.is_empty() {
                title_to_index.insert(title, index);
            }
            continue;
        };

        duplicates_removed += 1;
        let merged = merge_articles(&deduped[existing_index].clone(), &normalized);
        deduped[existing_index] = merged;

        let merged_link = canonical_link(&deduped[existing_index]);
        let merged_title = title_key(&deduped[existing_index]);
        if !merged_link.is_empty() {
            link_to_index.insert(merged_link, existing_index);
        }
        if !merged_title.is_empty() {
            title_to_index.insert(merged_title, existing_index);
        }
    }

    DeduplicationResult {
        articles: deduped,
        duplicates_removed,
        invalid_removed,
    }
}

fn find_existing_index(
    link: &str,
    title: &str,
    link_to_index: &HashMap<String, usize>,
    title_to_index: &HashMap<String, usize>,
) -> Option<usize> {
    if !link.is_empty() {
        if let Some(i) = link_to_index.get(link) {
            return Some(*i);
        }
    }
    if !title.is_empty() {
        if let Some(i) = title_to_index.get(title) {
            return Some(*i);
        }
    }
    None
}

fn merge_rank_bundle(
    existing: (&str, Option<i32>, &str, &str),
    incoming: (&str, Option<i32>, &str, &str),
) -> (String, Option<i32>, String, String) {
    // Pick one coherent claim: dated beats undated; two undated → first;
    // dated incoming replaces undated existing; older never replaces newer.
    let existing_rank = existing.0;
    let incoming_rank = incoming.0;
    if incoming_rank.is_empty() {
        return (
            existing.0.into(),
            existing.1,
            existing.2.into(),
            existing.3.into(),
        );
    }
    if existing_rank.is_empty() {
        return (
            incoming.0.into(),
            incoming.1,
            incoming.2.into(),
            incoming.3.into(),
        );
    }
    let (existing_year, incoming_year) = (existing.1, incoming.1);
    match (existing_year, incoming_year) {
        (None, _) => (
            incoming.0.into(),
            incoming.1,
            incoming.2.into(),
            incoming.3.into(),
        ),
        (Some(_), None) => (
            existing.0.into(),
            existing.1,
            existing.2.into(),
            existing.3.into(),
        ),
        (Some(e), Some(i)) => {
            if i > e {
                (
                    incoming.0.into(),
                    incoming.1,
                    incoming.2.into(),
                    incoming.3.into(),
                )
            } else {
                (
                    existing.0.into(),
                    existing.1,
                    existing.2.into(),
                    existing.3.into(),
                )
            }
        }
    }
}

fn merge_rank_bundle3(
    existing: (&str, Option<i32>, &str),
    incoming: (&str, Option<i32>, &str),
) -> (String, Option<i32>, String) {
    // Same claim-coherence rules as the 4-tuple bundle; third slot returned
    // separately because the SINTA bundle has no category field.
    let existing_rank = existing.0;
    let incoming_rank = incoming.0;
    if incoming_rank.is_empty() {
        return (existing.0.to_string(), existing.1, existing.2.to_string());
    }
    if existing_rank.is_empty() {
        return (incoming.0.to_string(), incoming.1, incoming.2.to_string());
    }
    match (existing.1, incoming.1) {
        (None, _) => (incoming.0.to_string(), incoming.1, incoming.2.to_string()),
        (Some(_), None) => (existing.0.to_string(), existing.1, existing.2.to_string()),
        (Some(e), Some(i)) => {
            if i > e {
                (incoming.0.to_string(), incoming.1, incoming.2.to_string())
            } else {
                (existing.0.to_string(), existing.1, existing.2.to_string())
            }
        }
    }
}

fn pdf_anchor(article: &Article) -> &str {
    if !article.pdf_url.is_empty() {
        &article.pdf_url
    } else {
        &article.pdf_source_url
    }
}

fn pdf_claim_strength(article: &Article) -> i32 {
    if pdf_anchor(article).is_empty() {
        return -1;
    }
    match article.pdf_status.as_str() {
        "available" => 2,
        "unavailable" => 1,
        _ => 0,
    }
}

fn merge_pdf_bundle(existing: &Article, incoming: &Article) -> (String, String, String, String) {
    let existing_checked = pdf_claim_strength(existing);
    let incoming_checked = pdf_claim_strength(incoming);
    let take = if incoming_checked > existing_checked
        || (incoming_checked == existing_checked && pdf_anchor(existing).is_empty())
    {
        incoming
    } else {
        existing
    };
    (
        take.pdf_url.clone(),
        take.pdf_source_url.clone(),
        take.pdf_status.clone(),
        take.pdf_detail.clone(),
    )
}

fn merge_articles(existing: &Article, incoming: &Article) -> Article {
    let (sjr_quartile, sjr_year, sjr_category, sjr_source_url) = merge_rank_bundle(
        (
            &existing.sjr_quartile,
            existing.sjr_year,
            &existing.sjr_category,
            &existing.sjr_source_url,
        ),
        (
            &incoming.sjr_quartile,
            incoming.sjr_year,
            &incoming.sjr_category,
            &incoming.sjr_source_url,
        ),
    );
    let (sinta_rank, sinta_year, sinta_source_url) = merge_rank_bundle3(
        (
            &existing.sinta_rank,
            existing.sinta_year,
            &existing.sinta_source_url,
        ),
        (
            &incoming.sinta_rank,
            incoming.sinta_year,
            &incoming.sinta_source_url,
        ),
    );
    let (pdf_url, pdf_source_url, pdf_status, pdf_detail) = merge_pdf_bundle(existing, incoming);
    let mut issns = existing.issns.clone();
    issns.extend(incoming.issns.iter().cloned());
    Article {
        title: if existing.title.is_empty() {
            incoming.title.clone()
        } else {
            existing.title.clone()
        },
        authors: if existing.authors.is_empty() {
            incoming.authors.clone()
        } else {
            existing.authors.clone()
        },
        link: if existing.link.is_empty() {
            incoming.link.clone()
        } else {
            existing.link.clone()
        },
        relevance_score: existing.relevance_score,
        doi: if existing.doi.is_empty() {
            incoming.doi.clone()
        } else {
            existing.doi.clone()
        },
        journal: if existing.journal.is_empty() {
            incoming.journal.clone()
        } else {
            existing.journal.clone()
        },
        issns: normalize_issn_list(issns.iter().map(|s| s.as_str())),
        publication_year: existing.publication_year.or(incoming.publication_year),
        sjr_quartile,
        sjr_year,
        sjr_category,
        sjr_source_url,
        sinta_rank,
        sinta_year,
        sinta_source_url,
        pdf_url,
        pdf_source_url,
        pdf_status,
        pdf_detail,
        pdf_local_path: if existing.pdf_local_path.is_empty() {
            incoming.pdf_local_path.clone()
        } else {
            existing.pdf_local_path.clone()
        },
    }
}
