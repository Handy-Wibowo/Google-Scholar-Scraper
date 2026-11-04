//! Port of `tests/test_dedupe.py` (all classes) 1:1.

use gss_core::dedupe::{
    canonical_link, deduplicate_articles, is_valid_article, normalize_article,
    normalize_display_text, normalize_link, title_key,
};
use gss_core::models::Article;

fn article(title: &str) -> Article {
    Article::new(title)
}

#[test]
fn display_text_collapses_whitespace_without_changing_punctuation_or_unicode() {
    let text = "  یادگیری\t عمیق\nبرای   پزشکی: α + β!  ";
    assert_eq!(
        normalize_display_text(text),
        "یادگیری عمیق برای پزشکی: α + β!"
    );
}

#[test]
fn article_normalization_preserves_readable_case() {
    let a = normalize_article(&Article {
        title: "  Deep   Learning  ".into(),
        authors: "\nA\tAuthor  ".into(),
        link: " https://Example.edu/paper#part ".into(),
        ..Article::default()
    });
    assert_eq!(a.title, "Deep Learning");
    assert_eq!(a.authors, "A Author");
    assert_eq!(a.link, "https://example.edu/paper");
}

#[test]
fn title_key_is_case_insensitive_and_whitespace_insensitive() {
    let first = title_key(&article("  Deep   Learning for Medical Imaging  "));
    let second = title_key(&article("DEEP LEARNING FOR MEDICAL IMAGING"));
    assert_eq!(first, second);
}

#[test]
fn link_normalization_preserves_empty_and_relative_values() {
    assert_eq!(normalize_link("   "), "");
    assert_eq!(
        normalize_link(" /scholar?cluster=1#frag "),
        "/scholar?cluster=1#frag"
    );
}

#[test]
fn canonical_link_requires_http_url() {
    assert_eq!(
        canonical_link(&Article {
            title: "Title".into(),
            link: "https://Example.edu/a#b".into(),
            ..Article::default()
        }),
        "https://example.edu/a"
    );
    assert_eq!(canonical_link(&article("/scholar?cluster=1")), "");
}

#[test]
fn normal_article_is_valid() {
    assert!(is_valid_article(&Article {
        title: "Valid title".into(),
        authors: "Author".into(),
        link: "".into(),
        ..Article::default()
    }));
}

#[test]
fn missing_link_remains_valid() {
    assert!(is_valid_article(&article("Citation only result")));
}

#[test]
fn missing_authors_remains_valid() {
    assert!(is_valid_article(&Article {
        title: "Title".into(),
        authors: "".into(),
        link: "https://example.edu".into(),
        ..Article::default()
    }));
}

#[test]
fn empty_title_is_invalid() {
    assert!(!is_valid_article(&Article {
        title: " \n\t ".into(),
        authors: "Author".into(),
        link: "https://example.edu".into(),
        ..Article::default()
    }));
}

#[test]
fn exact_duplicate_is_removed() {
    let result = deduplicate_articles(vec![
        Article {
            title: "Title".into(),
            authors: "Author".into(),
            link: "https://example.edu/a".into(),
            ..Article::default()
        },
        Article {
            title: "Title".into(),
            authors: "Author".into(),
            link: "https://example.edu/a".into(),
            ..Article::default()
        },
    ]);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.duplicates_removed, 1);
}

#[test]
fn duplicate_title_with_whitespace_and_capitalization_is_removed() {
    let result = deduplicate_articles(vec![
        Article {
            title: "  Deep   Learning for Medical Imaging  ".into(),
            authors: "A".into(),
            link: "".into(),
            ..Article::default()
        },
        Article {
            title: "DEEP LEARNING FOR MEDICAL IMAGING".into(),
            authors: "A".into(),
            link: "".into(),
            ..Article::default()
        },
    ]);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.duplicates_removed, 1);
    assert_eq!(
        result.articles[0].title,
        "Deep Learning for Medical Imaging"
    );
}

#[test]
fn same_canonical_link_is_duplicate() {
    let result = deduplicate_articles(vec![
        Article {
            title: "First title".into(),
            authors: "A".into(),
            link: "https://example.edu/paper#section".into(),
            ..Article::default()
        },
        Article {
            title: "Second title".into(),
            authors: "B".into(),
            link: "https://EXAMPLE.edu/paper".into(),
            ..Article::default()
        },
    ]);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.articles[0].title, "First title");
}

#[test]
fn same_title_merges_missing_link_from_later_record() {
    let result = deduplicate_articles(vec![
        Article {
            title: "Shared title".into(),
            authors: "Author".into(),
            link: "".into(),
            ..Article::default()
        },
        Article {
            title: " shared   title ".into(),
            authors: "".into(),
            link: "https://example.edu/shared".into(),
            ..Article::default()
        },
    ]);
    assert_eq!(result.articles.len(), 1);
    assert_eq!(result.articles[0].link, "https://example.edu/shared");
    assert_eq!(result.articles[0].authors, "Author");
}

#[test]
fn different_titles_remain_distinct() {
    let result = deduplicate_articles(vec![
        article("Deep Learning for Medical Imaging"),
        article("Deep Learning for Medical Image Segmentation"),
    ]);
    assert_eq!(result.articles.len(), 2);
    assert_eq!(result.duplicates_removed, 0);
}

#[test]
fn stable_first_seen_ordering() {
    let result = deduplicate_articles(vec![
        article("First"),
        article("Second"),
        Article {
            title: "first".into(),
            authors: "".into(),
            link: "https://example.edu/first".into(),
            ..Article::default()
        },
        article("Third"),
    ]);
    let titles: Vec<&str> = result.articles.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["First", "Second", "Third"]);
    assert_eq!(result.articles[0].link, "https://example.edu/first");
}

#[test]
fn invalid_empty_title_is_removed() {
    let result = deduplicate_articles(vec![Article {
        title: "  ".into(),
        authors: "Author".into(),
        link: "https://example.edu".into(),
        ..Article::default()
    }]);
    assert!(result.articles.is_empty());
    assert_eq!(result.invalid_removed, 1);
}

#[test]
fn merge_preserves_every_new_field_from_a_single_record() {
    let first = Article {
        title: "Deep Learning".into(),
        authors: "A Author".into(),
        link: "".into(),
        doi: "10.1038/nature14539".into(),
        journal: "Nature".into(),
        issns: vec!["0028-0836".into()],
        publication_year: Some(2015),
        sjr_quartile: "Q1".into(),
        sjr_year: Some(2023),
        sjr_category: "Multidisciplinary".into(),
        sjr_source_url: "https://scimagojr.example/nature".into(),
        sinta_rank: "S2".into(),
        sinta_year: Some(2024),
        sinta_source_url: "https://sinta.example/nature".into(),
        pdf_url: "https://repo.example/a.pdf".into(),
        pdf_source_url: "https://api.crossref.org/works/10.1038/nature14539".into(),
        pdf_status: "available".into(),
        pdf_detail: "verified".into(),
        pdf_local_path: "C:/pdfs/a.pdf".into(),
        relevance_score: None,
        ..Default::default()
    };
    let second = Article {
        title: " deep   learning ".into(),
        authors: "".into(),
        link: "https://example.edu/a".into(),
        ..Article::default()
    };

    let result = deduplicate_articles(vec![first, second]);
    assert_eq!(result.articles.len(), 1);
    let m = &result.articles[0];
    assert_eq!(m.doi, "10.1038/nature14539");
    assert_eq!(m.journal, "Nature");
    assert_eq!(m.issns, vec!["0028-0836".to_string()]);
    assert_eq!(m.publication_year, Some(2015));
    assert_eq!(
        (
            m.sjr_quartile.as_str(),
            m.sjr_year,
            m.sjr_category.as_str(),
            m.sjr_source_url.as_str()
        ),
        (
            "Q1",
            Some(2023),
            "Multidisciplinary",
            "https://scimagojr.example/nature"
        )
    );
    assert_eq!(
        (
            m.sinta_rank.as_str(),
            m.sinta_year,
            m.sinta_source_url.as_str()
        ),
        ("S2", Some(2024), "https://sinta.example/nature")
    );
    assert_eq!(
        (
            m.pdf_url.as_str(),
            m.pdf_source_url.as_str(),
            m.pdf_status.as_str(),
            m.pdf_detail.as_str()
        ),
        (
            "https://repo.example/a.pdf",
            "https://api.crossref.org/works/10.1038/nature14539",
            "available",
            "verified"
        )
    );
    assert_eq!(m.pdf_local_path, "C:/pdfs/a.pdf");
    assert_eq!(m.link, "https://example.edu/a");
}

#[test]
fn ranking_is_merged_as_one_claim_not_field_by_field() {
    let existing = Article {
        title: "Shared".into(),
        sjr_quartile: "Q1".into(),
        sjr_year: None,
        sjr_category: "Old category".into(),
        ..Article::default()
    };
    let incoming = Article {
        title: "shared".into(),
        sjr_quartile: "Q4".into(),
        sjr_year: Some(2024),
        sjr_category: "New category".into(),
        ..Article::default()
    };
    let merged = &deduplicate_articles(vec![existing, incoming]).articles[0];
    // A dated record beats an undated one whole: never Q1 with 2024's year.
    assert_eq!(
        (
            merged.sjr_quartile.as_str(),
            merged.sjr_year,
            merged.sjr_category.as_str()
        ),
        ("Q4", Some(2024), "New category")
    );
}

#[test]
fn latest_dated_ranking_wins_over_an_older_one() {
    let older = Article {
        title: "Shared".into(),
        sjr_quartile: "Q2".into(),
        sjr_year: Some(2019),
        ..Article::default()
    };
    let newer = Article {
        title: "shared".into(),
        sjr_quartile: "Q1".into(),
        sjr_year: Some(2023),
        ..Article::default()
    };
    let merged = &deduplicate_articles(vec![older, newer]).articles[0];
    assert_eq!(
        (merged.sjr_quartile.as_str(), merged.sjr_year),
        ("Q1", Some(2023))
    );
}

#[test]
fn available_pdf_is_never_attached_to_a_different_candidate() {
    let verified = Article {
        title: "Shared".into(),
        pdf_url: "https://repo.example/verified.pdf".into(),
        pdf_source_url: "https://repo.example/verified.pdf".into(),
        pdf_status: "available".into(),
        pdf_detail: "verified".into(),
        ..Article::default()
    };
    let candidate = Article {
        title: "shared".into(),
        pdf_url: "https://other.example/candidate.pdf".into(),
        pdf_source_url: "https://other.example/candidate.pdf".into(),
        pdf_status: "unknown".into(),
        ..Article::default()
    };
    let merged = &deduplicate_articles(vec![candidate, verified]).articles[0];
    assert_eq!(merged.pdf_url, "https://repo.example/verified.pdf");
    assert_eq!(merged.pdf_status, "available");
}

#[test]
fn available_status_with_no_anchor_degrades_to_unknown() {
    let broken = Article {
        title: "Shared".into(),
        pdf_url: "".into(),
        pdf_source_url: "".into(),
        pdf_status: "available".into(),
        ..Article::default()
    };
    let merged = &deduplicate_articles(vec![broken]).articles[0];
    assert_eq!(merged.pdf_status, "unknown");
}
