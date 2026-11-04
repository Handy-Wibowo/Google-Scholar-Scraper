//! TF-IDF relevance ranking ported from `ranking.py` verbatim.

use crate::models::Article;
use crate::text::CaseFold;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());

const TITLE_WEIGHT: usize = 3;

/// NFKC + casefold + `\w+` tokens (`tokenize`).
pub fn tokenize(text: &str) -> Vec<String> {
    let normalized: String = text.nfkc().collect();
    TOKEN_RE
        .find_iter(&normalized.casefold())
        .map(|m| m.as_str().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Title tokens ×3 + author tokens (`article_tokens`); title first, in order.
pub fn article_tokens(article: &Article) -> Vec<String> {
    let title_tokens = tokenize(&article.title);
    let author_tokens = tokenize(&article.authors);
    let mut weighted: Vec<String> =
        Vec::with_capacity(title_tokens.len() * TITLE_WEIGHT + author_tokens.len());
    for _ in 0..TITLE_WEIGHT {
        weighted.extend(title_tokens.iter().cloned());
    }
    weighted.extend(author_tokens);
    weighted
}

/// Score with TF-IDF + cosine, bounded to [0, 100], rounded to 1 decimal
/// (`score_articles`). Empty article list → empty; empty query → all zeros.
pub fn score_articles<I: IntoIterator<Item = Article>>(query: &str, articles: I) -> Vec<Article> {
    let article_list: Vec<Article> = articles.into_iter().collect();
    if article_list.is_empty() {
        return Vec::new();
    }
    let query_tokens = tokenize(query);
    let document_tokens: Vec<Vec<String>> = article_list.iter().map(article_tokens).collect();
    let scores = score_tokenized_documents(&query_tokens, &document_tokens);
    article_list
        .into_iter()
        .zip(scores)
        .map(|(mut a, score)| {
            a.relevance_score = Some(score);
            a
        })
        .collect()
}

/// Stable sort by descending score then original index (`rank_articles`).
/// Disabled → order and scores untouched.
pub fn rank_articles<I: IntoIterator<Item = Article>>(
    query: &str,
    articles: I,
    enabled: bool,
) -> Vec<Article> {
    let article_list: Vec<Article> = articles.into_iter().collect();
    if !enabled {
        return article_list;
    }
    let scored = score_articles(query, article_list);
    let mut ranked: Vec<(usize, Article)> = scored.into_iter().enumerate().collect();
    ranked.sort_by(|a, b| {
        let sa = a.1.relevance_score.unwrap_or(0.0);
        let sb = b.1.relevance_score.unwrap_or(0.0);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    ranked.into_iter().map(|(_, a)| a).collect()
}

pub fn ranked_articles<I: IntoIterator<Item = Article>>(
    query: &str,
    articles: I,
    enabled: bool,
) -> Vec<Article> {
    rank_articles(query, articles, enabled)
}

/// TF vector scaled by precomputed IDF (`tfidf_vector`).
pub fn tfidf_vector(tokens: &[String], idf: &HashMap<String, f64>) -> HashMap<String, f64> {
    if tokens.is_empty() {
        return HashMap::new();
    }
    let mut counts: HashMap<String, f64> = HashMap::new();
    for t in tokens {
        *counts.entry(t.clone()).or_insert(0.0) += 1.0;
    }
    let total: f64 = counts.values().sum();
    counts
        .into_iter()
        .map(|(t, c)| {
            let scaled = (c / total) * idf.get(&t).copied().unwrap_or(0.0);
            (t, scaled)
        })
        .collect()
}

/// Cosine similarity over sparse TF-IDF vectors (`cosine_similarity`).
pub fn cosine_similarity(first: &HashMap<String, f64>, second: &HashMap<String, f64>) -> f64 {
    if first.is_empty() || second.is_empty() {
        return 0.0;
    }
    let dot: f64 = first
        .iter()
        .filter_map(|(t, v)| second.get(t).map(|w| v * w))
        .sum();
    let first_norm = first.values().map(|v| v * v).sum::<f64>().sqrt();
    let second_norm = second.values().map(|v| v * v).sum::<f64>().sqrt();
    if first_norm == 0.0 || second_norm == 0.0 {
        return 0.0;
    }
    dot / (first_norm * second_norm)
}

fn score_tokenized_documents(query_tokens: &[String], document_tokens: &[Vec<String>]) -> Vec<f64> {
    if query_tokens.is_empty() {
        return vec![0.0; document_tokens.len()];
    }
    let mut corpus: Vec<&[String]> = vec![query_tokens];
    corpus.extend(document_tokens.iter().map(|d| d.as_slice()));
    let idf = idf(&corpus);
    let query_vector = tfidf_vector(query_tokens, &idf);

    document_tokens
        .iter()
        .map(|tokens| {
            let similarity = cosine_similarity(&query_vector, &tfidf_vector(tokens, &idf));
            (similarity * 100.0).clamp(0.0, 100.0)
        })
        .map(round1)
        .collect()
}

/// `idf[t] = ln((N+1)/(df+1)) + 1` over the corpus (`_idf`).
fn idf(corpus: &[&[String]]) -> HashMap<String, f64> {
    let document_count = corpus.len() as f64;
    let document_sets: Vec<std::collections::HashSet<&String>> =
        corpus.iter().map(|d| d.iter().collect()).collect();
    let mut vocabulary: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for d in &document_sets {
        vocabulary.extend(d.iter().copied());
    }
    let mut out = HashMap::new();
    for token in vocabulary {
        let containing = document_sets.iter().filter(|d| d.contains(token)).count() as f64;
        out.insert(
            token.clone(),
            ((document_count + 1.0) / (containing + 1.0)).ln() + 1.0,
        );
    }
    out
}

/// Round-half-away-from-zero to 1 decimal, matching Python's `round(x, 1)`
/// for the values this produces (Python banker's rounding only matters at
/// exact .x5 binary edges, which f64 intermediate results effectively never
/// land on here; verified by the parity suite).
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
