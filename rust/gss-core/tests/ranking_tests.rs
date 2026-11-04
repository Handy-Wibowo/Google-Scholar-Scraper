//! Port of `tests/test_ranking.py` (all classes) 1:1.

use gss_core::models::Article;
use gss_core::ranking::{
    article_tokens, cosine_similarity, rank_articles, ranked_articles, score_articles,
    tfidf_vector, tokenize,
};
use std::collections::HashMap;

#[test]
fn tokenize_normalizes_whitespace_case_and_punctuation() {
    assert_eq!(
        tokenize("  Deep,   LEARNING\tfor\nCancer! "),
        vec!["deep", "learning", "for", "cancer"]
    );
}

#[test]
fn tokenize_preserves_non_latin_text() {
    assert_eq!(
        tokenize("یادگیری عمیق برای سرطان"),
        vec!["یادگیری", "عمیق", "برای", "سرطان"]
    );
}

#[test]
fn tokenize_handles_empty_text() {
    assert!(tokenize("  \t\n ").is_empty());
}

#[test]
fn article_tokens_weight_title_more_than_metadata() {
    let a = Article {
        title: "cancer diagnosis".into(),
        authors: "journal cancer".into(),
        ..Article::default()
    };
    let tokens = article_tokens(&a);
    assert_eq!(tokens.iter().filter(|t| *t == "cancer").count(), 4);
    assert_eq!(tokens.iter().filter(|t| *t == "diagnosis").count(), 3);
    assert_eq!(tokens.iter().filter(|t| *t == "journal").count(), 1);
}

#[test]
fn tfidf_vector_handles_one_document() {
    let mut idf = HashMap::new();
    idf.insert("cancer".to_string(), 1.0);
    let tokens = vec!["cancer".to_string(), "cancer".to_string()];
    let vector = tfidf_vector(&tokens, &idf);
    assert_eq!(vector.get("cancer"), Some(&1.0));
}

#[test]
fn tfidf_vector_handles_empty_tokens() {
    let mut idf = HashMap::new();
    idf.insert("cancer".to_string(), 1.0);
    assert!(tfidf_vector(&[], &idf).is_empty());
}

#[test]
fn cosine_similarity_identical_vectors() {
    let mut a = HashMap::new();
    a.insert("a".to_string(), 1.0);
    assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-12);
}

#[test]
fn cosine_similarity_unrelated_vectors() {
    let mut a = HashMap::new();
    a.insert("a".to_string(), 1.0);
    let mut b = HashMap::new();
    b.insert("b".to_string(), 1.0);
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn cosine_similarity_empty_vector() {
    let empty: HashMap<String, f64> = HashMap::new();
    let mut a = HashMap::new();
    a.insert("a".to_string(), 1.0);
    assert_eq!(cosine_similarity(&empty, &a), 0.0);
}

#[test]
fn matching_title_ranks_above_unrelated_title() {
    let articles = vec![
        Article::new("Soil chemistry in arid climates"),
        Article::new("Machine learning for breast cancer diagnosis"),
    ];
    let ranked = ranked_articles("machine learning breast cancer diagnosis", articles, true);
    assert_eq!(
        ranked[0].title,
        "Machine learning for breast cancer diagnosis"
    );
    assert!(ranked[0].relevance_score.unwrap() > ranked[1].relevance_score.unwrap());
}

#[test]
fn scores_are_bounded_and_finite() {
    let scored = score_articles(
        "machine learning",
        vec![Article::new("machine learning"), Article::new("unrelated")],
    );
    for a in scored {
        let s = a.relevance_score.unwrap();
        assert!((0.0..=100.0).contains(&s));
        assert!(!s.is_nan());
        assert!(!s.is_infinite());
    }
}

#[test]
fn query_terms_not_seen_in_documents_return_zero_scores() {
    let scored = score_articles("quantum", vec![Article::new("machine learning")]);
    assert_eq!(scored[0].relevance_score, Some(0.0));
}

#[test]
fn repeated_query_terms_are_deterministic() {
    let first = score_articles(
        "cancer cancer diagnosis",
        vec![Article::new("cancer diagnosis")],
    );
    let second = score_articles(
        "cancer cancer diagnosis",
        vec![Article::new("cancer diagnosis")],
    );
    assert_eq!(first[0].relevance_score, second[0].relevance_score);
}

#[test]
fn single_matching_document_gets_high_score() {
    let scored = score_articles("machine learning", vec![Article::new("machine learning")]);
    assert_eq!(scored[0].relevance_score, Some(100.0));
}

#[test]
fn equal_scores_preserve_original_order() {
    let articles = vec![
        Article::new("Cancer diagnosis"),
        Article::new("Cancer diagnosis"),
    ];
    let ranked = rank_articles("cancer", articles, true);
    assert_eq!(ranked[0].title, "Cancer diagnosis");
    assert_eq!(ranked[1].title, "Cancer diagnosis");
}

#[test]
fn ranking_disabled_preserves_order_and_scores() {
    let articles = vec![Article::new("Unrelated"), Article::new("Machine learning")];
    let ranked = rank_articles("machine learning", articles, false);
    assert_eq!(ranked[0].title, "Unrelated");
    assert_eq!(ranked[1].title, "Machine learning");
    assert_eq!(ranked[0].relevance_score, None);
}

#[test]
fn ranking_preserves_every_metadata_field() {
    let article = Article {
        title: "Deep learning".into(),
        authors: "Y LeCun - Nature, 2015".into(),
        link: "https://example.edu/a".into(),
        doi: "10.1038/nature14539".into(),
        journal: "Nature".into(),
        issns: vec!["0028-0836".into(), "1476-4687".into()],
        publication_year: Some(2015),
        sjr_quartile: "Q1".into(),
        sjr_year: Some(2023),
        sjr_category: "Multidisciplinary".into(),
        sjr_source_url: "https://scimagojr.example/nature".into(),
        sinta_rank: "S2".into(),
        sinta_year: Some(2024),
        sinta_source_url: "https://sinta.example/nature".into(),
        pdf_url: "https://repo.example/a.pdf".into(),
        pdf_source_url: "https://repo.example/a.pdf".into(),
        pdf_status: "available".into(),
        pdf_detail: "verified".into(),
        pdf_local_path: "C:/pdfs/a.pdf".into(),
        relevance_score: None,
    };

    let ranked = rank_articles("deep learning", vec![article.clone()], true);
    let scored = &ranked[0];
    assert!(scored.relevance_score.is_some());
    assert_eq!(scored.doi, article.doi);
    assert_eq!(scored.journal, article.journal);
    assert_eq!(scored.issns, article.issns);
    assert_eq!(scored.publication_year, Some(2015));
    assert_eq!(scored.sjr_quartile, "Q1");
    assert_eq!(scored.sjr_year, Some(2023));
    assert_eq!(scored.sjr_category, "Multidisciplinary");
    assert_eq!(scored.sjr_source_url, "https://scimagojr.example/nature");
    assert_eq!(scored.sinta_rank, "S2");
    assert_eq!(scored.sinta_year, Some(2024));
    assert_eq!(scored.sinta_source_url, "https://sinta.example/nature");
    assert_eq!(scored.pdf_url, "https://repo.example/a.pdf");
    assert_eq!(scored.pdf_source_url, "https://repo.example/a.pdf");
    assert_eq!(scored.pdf_status, "available");
    assert_eq!(scored.pdf_detail, "verified");
    assert_eq!(scored.pdf_local_path, "C:/pdfs/a.pdf");
}
