//! Discovery expansion: PubMed/OpenAlex identity helpers used by the citation
//! graph and by DOI-paste import.
//!
//! Everything here is pure: parsers take an already-fetched JSON payload and
//! return records. Network access stays in the transport seams (`CrossrefFetcher`
//! / `PdfFetcher`), which the app wires up.

use crate::models::Article;
use serde_json::Value;

/// One work referenced by, or citing, a seed paper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelatedWork {
    pub title: String,
    pub doi: String,
    pub year: Option<i32>,
    /// `"references"` (backward) or `"citations"` (forward).
    pub direction: String,
}

/// OpenAlex work ids referenced by this work (`referenced_works`), backward
/// chasing.
pub fn referenced_work_ids(payload: &Value) -> Vec<String> {
    let Some(list) = payload.get("referenced_works").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let Some(id) = entry.as_str() else { continue };
        let id = id.trim();
        // OpenAlex ids arrive as full URLs; the bare id is what the API accepts.
        let bare = id.rsplit('/').next().unwrap_or(id).to_string();
        if !bare.is_empty() && !out.contains(&bare) {
            out.push(bare);
        }
    }
    out
}

/// Parse a `/works?filter=cites:<id>` response into related works (forward
/// chasing).
pub fn citing_works(payload: &Value, limit: usize) -> Vec<RelatedWork> {
    let Some(results) = payload.get("results").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<RelatedWork> = Vec::new();
    for work in results.iter().take(limit) {
        let title = work
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let doi = work
            .get("doi")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .trim_start_matches("https://doi.org/")
            .to_string();
        let year = work
            .get("publication_year")
            .and_then(|y| y.as_i64())
            .map(|y| y as i32);
        out.push(RelatedWork {
            title,
            doi,
            year,
            direction: "citations".to_string(),
        });
    }
    out
}

/// Parse a batch of `/works/<id>` payloads into `Article`s for the result table.
///
/// Used by backward citation chasing, where the referenced works are fetched
/// individually.
pub fn articles_from_works(payloads: &[Value], direction: &str) -> Vec<Article> {
    let mut out: Vec<Article> = Vec::new();
    for work in payloads {
        let title = work
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let doi = work
            .get("doi")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .trim_start_matches("https://doi.org/")
            .to_string();
        let year = work
            .get("publication_year")
            .and_then(|y| y.as_i64())
            .map(|y| y as i32);
        // The landing page is the DOI resolver when a DOI exists.
        let link = if doi.is_empty() {
            String::new()
        } else {
            format!("https://doi.org/{doi}")
        };
        let journal = work
            .get("primary_location")
            .and_then(|l| l.get("source"))
            .and_then(|s| s.get("display_name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let cited_by = work.get("cited_by_count").and_then(|c| c.as_i64());
        let mut article = Article {
            title,
            authors: String::new(),
            link,
            doi,
            journal,
            publication_year: year,
            cited_by,
            pdf_status: crate::models::PDF_STATUS_UNKNOWN.to_string(),
            pdf_detail: format!("Discovered by {direction} citation chasing; not yet checked."),
            ..Article::default()
        };
        // Keep the discovery direction visible in the record's provenance.
        if !direction.is_empty() {
            article.authors = String::new();
        }
        out.push(article);
    }
    out
}

/// Parse pasted identifiers (one per line, or comma/semicolon separated) into
/// bare DOIs.
///
/// Accepts `10.x/y`, `doi:10.x/y`, `https://doi.org/10.x/y` and
/// `http://dx.doi.org/10.x/y`, skips blanks and comments, and de-duplicates.
pub fn parse_pasted_dois(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in input.split(['\n', '\r', ',', ';']) {
        let mut token = raw.trim();
        if token.is_empty() || token.starts_with('#') {
            continue;
        }
        for prefix in [
            "https://doi.org/",
            "http://doi.org/",
            "http://dx.doi.org/",
            "https://dx.doi.org/",
            "doi:",
            "DOI:",
        ] {
            if let Some(rest) = token.strip_prefix(prefix) {
                token = rest.trim();
                break;
            }
        }
        if !token.starts_with("10.") {
            continue;
        }
        let normalized = crate::text::normalize_doi(token);
        let key = normalized.to_lowercase();
        if !key.is_empty() && !out.iter().any(|d| d.to_lowercase() == key) {
            out.push(normalized);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_referenced_ids_without_duplicates() {
        let payload = json!({
            "referenced_works": [
                "https://openalex.org/W123",
                "https://openalex.org/W456",
                "https://openalex.org/W123"
            ]
        });
        assert_eq!(referenced_work_ids(&payload), vec!["W123", "W456"]);
    }

    #[test]
    fn missing_referenced_works_is_empty() {
        assert!(referenced_work_ids(&json!({})).is_empty());
    }

    #[test]
    fn parses_citing_works_with_doi_and_year() {
        let payload = json!({
            "results": [
                {"title": "A citing paper", "doi": "https://doi.org/10.1/abc", "publication_year": 2023},
                {"title": "", "doi": "10.1/skip"},
                {"title": "No DOI paper", "publication_year": 2021}
            ]
        });
        let found = citing_works(&payload, 10);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].doi, "10.1/abc");
        assert_eq!(found[0].year, Some(2023));
        assert_eq!(found[1].doi, "");
        assert_eq!(found[0].direction, "citations");
    }

    #[test]
    fn citing_works_respects_the_limit() {
        let payload = json!({"results": [
            {"title": "a"}, {"title": "b"}, {"title": "c"}
        ]});
        assert_eq!(citing_works(&payload, 2).len(), 2);
    }

    #[test]
    fn articles_from_works_build_doi_links() {
        let payloads = vec![json!({
            "title": "Referenced paper",
            "doi": "https://doi.org/10.1/ref",
            "publication_year": 2019,
            "cited_by_count": 12,
            "primary_location": {"source": {"display_name": "J Reference"}}
        })];
        let arts = articles_from_works(&payloads, "backward");
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].link, "https://doi.org/10.1/ref");
        assert_eq!(arts[0].doi, "10.1/ref");
        assert_eq!(arts[0].journal, "J Reference");
        assert_eq!(arts[0].cited_by, Some(12));
        assert!(arts[0].pdf_detail.contains("backward"));
    }

    #[test]
    fn pasted_dois_accept_every_common_shape() {
        let input = "10.1000/ABC\n\
                     doi:10.1000/def\n\
                     https://doi.org/10.1000/ghi\n\
                     http://dx.doi.org/10.1000/jkl; 10.1000/mno\n\
                     # a comment\n\
                     not-a-doi\n\
                     10.1000/abc";
        let parsed = parse_pasted_dois(input);
        assert_eq!(
            parsed,
            // normalize_doi lowercases: DOIs are case-insensitive, and the
            // normalized form is what the cache and dedupe keys use.
            vec![
                "10.1000/abc",
                "10.1000/def",
                "10.1000/ghi",
                "10.1000/jkl",
                "10.1000/mno"
            ],
            "duplicates (case-insensitive) and non-DOIs must be dropped"
        );
    }

    #[test]
    fn empty_paste_is_empty() {
        assert!(parse_pasted_dois("   \n\n ,, ").is_empty());
    }
}
