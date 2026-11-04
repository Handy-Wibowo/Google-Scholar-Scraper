//! gss-parity: JSON dump harness used by `tools/parity_check.py` to diff the
//! Rust implementation against the Python oracle, offline.
//!
//! Subcommands (stdin JSON in → stdout JSON out):
//! - `title-keys`  : lines `{"title": "..."}` → lines `{"title","key"}`
//! - `parse-page`  : lines `{"html_path", "status_code"}` → parse results
//!   (one JSON object per line with status/message/diagnostic/articles)
//! - `export-rows` : lines `{"articles":[{...}]}` → export-row JSON per line
//! - `citations`   : lines `{"records":[{...}], "format":"bibtex"|"ris"}`
//!   → one JSON object per line `{"text": "..."}`

use gss_core::exporters::{save_to_bibtex, save_to_csv, save_to_ris};
use gss_core::models::{Article, CitationRecord};
use gss_core::parser::parse_scholar_page;
use gss_core::text::normalize_title_key;
use serde_json::Value;
use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let result = match cmd {
        "title-keys" => title_keys(),
        "parse-page" => parse_page(),
        "export-rows" => export_rows(),
        "citations" => citations(),
        "extract" => extract(),
        _ => Err(format!(
            "unknown subcommand {cmd:?}; expected: title-keys | parse-page | export-rows | citations | extract"
        )),
    };
    if let Err(msg) = result {
        eprintln!("gss-parity: {msg}");
        std::process::exit(2);
    }
}

fn title_keys() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad JSON line: {e}"))?;
        let title = v
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        let key = normalize_title_key(&title);
        let record = serde_json::json!({"title": title, "key": key});
        writeln!(out, "{record}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn article_json(a: &gss_core::models::Article) -> Value {
    serde_json::json!({
        "title": a.title,
        "authors": a.authors,
        "link": a.link,
        "relevance_score": a.relevance_score,
        "doi": a.doi,
        "journal": a.journal,
        "issns": a.issns,
        "publication_year": a.publication_year,
        "sjr_quartile": a.sjr_quartile,
        "sjr_year": a.sjr_year,
        "sjr_category": a.sjr_category,
        "sjr_source_url": a.sjr_source_url,
        "sinta_rank": a.sinta_rank,
        "sinta_year": a.sinta_year,
        "sinta_source_url": a.sinta_source_url,
        "pdf_url": a.pdf_url,
        "pdf_source_url": a.pdf_source_url,
        "pdf_status": a.pdf_status,
        "pdf_detail": a.pdf_detail,
        "pdf_local_path": a.pdf_local_path,
    })
}

fn parse_page() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad JSON line: {e}"))?;
        let path = v
            .get("html_path")
            .and_then(|p| p.as_str())
            .ok_or("missing html_path")?
            .to_string();
        let status_code = v.get("status_code").and_then(|s| s.as_u64()).unwrap_or(200) as u16;
        let html =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let result = parse_scholar_page(&html, status_code);
        let articles: Vec<Value> = result.articles.iter().map(article_json).collect();
        let record = serde_json::json!({
            "html_path": path,
            "status": result.status.as_str(),
            "message": result.message,
            "diagnostic": result.diagnostic,
            "articles": articles,
        });
        writeln!(out, "{record}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn article_from_json(v: &Value) -> Article {
    serde_json::from_value(v.clone())
        .unwrap_or_else(|_| Article::new(v.get("title").and_then(|t| t.as_str()).unwrap_or("")))
}

fn citation_from_json(v: &Value) -> CitationRecord {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

fn export_rows() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad JSON line: {e}"))?;
        let articles: Vec<Article> = v
            .get("articles")
            .and_then(|a| a.as_array())
            .map(|list| list.iter().map(article_from_json).collect())
            .unwrap_or_default();

        let csv_path = dir.path().join("parity.csv");
        save_to_csv(&articles, &csv_path).map_err(|e| e.to_string())?;
        let csv_bytes = std::fs::read(&csv_path).map_err(|e| e.to_string())?;

        let record = serde_json::json!({
            "csv_base64": base64_encode(&csv_bytes),
        });
        writeln!(out, "{record}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn citations() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad JSON line: {e}"))?;
        let records: Vec<CitationRecord> = v
            .get("records")
            .and_then(|a| a.as_array())
            .map(|list| list.iter().map(citation_from_json).collect())
            .unwrap_or_default();
        let format = v
            .get("format")
            .and_then(|f| f.as_str())
            .unwrap_or("bibtex")
            .to_string();

        let path = if format == "ris" {
            dir.path().join("parity.ris")
        } else {
            dir.path().join("parity.bib")
        };
        if format == "ris" {
            save_to_ris(&records, &path).map_err(|e| e.to_string())?;
        } else {
            save_to_bibtex(&records, &path).map_err(|e| e.to_string())?;
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let record = serde_json::json!({
            "format": format,
            "text_base64": base64_encode(&bytes),
        });
        writeln!(out, "{record}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `extract`: full scrape_scholar over a scripted fixture-serving fetcher.
/// Input lines: {"pages": ["<html>", ...], "query": "...", "ranking": bool}
/// Output: one JSON object per line = ExtractionResult dump.
struct ScriptedFetcher {
    pages: std::collections::VecDeque<String>,
}

impl gss_core::client::HttpFetcher for ScriptedFetcher {
    fn get(
        &mut self,
        _url: &str,
        _params: &[(String, String)],
    ) -> Result<gss_core::client::FetchResponse, gss_core::client::FetchError> {
        match self.pages.pop_front() {
            Some(text) => Ok(gss_core::client::FetchResponse {
                text,
                status_code: 200,
            }),
            None => Err(gss_core::client::FetchError::Connection {
                class: "ConnectionError".into(),
            }),
        }
    }
}

struct NoCancel;
impl gss_core::client::CancelEvent for NoCancel {
    fn is_set(&self) -> bool {
        false
    }
    fn wait(&self, _s: f64) -> bool {
        false
    }
}

fn extract() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad JSON line: {e}"))?;
        let pages: Vec<String> = v
            .get("pages")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let query = v
            .get("query")
            .and_then(|q| q.as_str())
            .unwrap_or("query")
            .to_string();
        let ranking = v.get("ranking").and_then(|r| r.as_bool()).unwrap_or(true);
        let year_from = v
            .get("year_from")
            .and_then(|y| y.as_i64())
            .map(|y| y as i32);
        let year_to = v.get("year_to").and_then(|y| y.as_i64()).map(|y| y as i32);

        let fetcher = ScriptedFetcher {
            pages: pages.into(),
        };
        let mut client = gss_core::client::ScholarClient::new(Box::new(fetcher));
        client.page_delay_seconds = 0.0;
        client.backoff_seconds = 0.0;
        client.max_retries = 1;
        let cancel = NoCancel;
        let result = gss_core::client::scrape_scholar(
            &query,
            pages_len(&v),
            &mut client,
            ranking,
            None,
            Some(&cancel),
            year_from,
            year_to,
        );
        let record = serde_json::json!({
            "status": result.status.as_str(),
            "articles": result.articles.iter().map(article_json).collect::<Vec<_>>(),
            "requested_pages": result.requested_pages,
            "successful_pages": result.successful_pages,
            "failure_page": result.failure_page,
            "message": result.message,
            "diagnostic": result.diagnostic,
            "duplicates_removed": result.duplicates_removed,
            "invalid_articles_removed": result.invalid_articles_removed,
        });
        writeln!(out, "{record}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pages_len(v: &Value) -> i32 {
    v.get("pages")
        .and_then(|p| p.as_array())
        .map(|a| a.len() as i32)
        .unwrap_or(0)
}
