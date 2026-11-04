//! Verbatim user-facing strings (from tkinter_app.py:25-107) and the status
//! tables. En-dashes and ellipses preserved exactly. The JS UI layer holds
//! the live copies; these Rust mirrors are consumed by tests and
//! `--selfcheck-layout` so a string drift fails CI.
#![allow(dead_code)]

pub const APP_TITLE: &str = "Google Scholar Scraper";
pub const APP_VERSION: &str = "3.0.0";

pub const DEFAULT_EXCEL_FILENAME: &str = "scholar_articles.xlsx";
pub const DEFAULT_CSV_FILENAME: &str = "scholar_articles.csv";
pub const DEFAULT_BIBTEX_FILENAME: &str = "scholar_articles.bib";
pub const DEFAULT_RIS_FILENAME: &str = "scholar_articles.ris";

pub const PDF_FILTERS: [&str; 4] = [
    "All",
    "Public PDF available",
    "No public PDF found",
    "Unknown",
];

/// OPERATION_HEADINGS.
pub fn operation_heading(operation: &str) -> &'static str {
    match operation {
        "search" => "Searching Google Scholar",
        "check" => "Checking metadata and public PDFs",
        "import" => "Importing journal ranking records",
        "download" => "Downloading selected public PDFs",
        "citations" => "Exporting citations",
        _ => "Working",
    }
}

pub const NO_RANKING_NOTICE: &str = "No ranks imported.";

/// 5-line journal/PDF caveat paragraph (tkinter_app.py:88-94), verbatim.
pub const DETAIL_DISCLAIMER: &str = "Journal quartiles and ranks describe the journal, not this paper. Ranking data comes from files you import, and an unmatched journal stays unknown rather than unranked. Public-PDF labels describe what the check actually verified at the time it ran. Links and metadata come from Google Scholar and may be incomplete or wrong. Nothing here replaces reading the source.";

/// Status heading per ExtractionStatus (status_heading, L219).
pub fn status_heading(status: &str) -> &'static str {
    match status {
        "SUCCESS" => "Search complete",
        "PARTIAL_SUCCESS" => "Search partially completed",
        "NO_RESULTS" => "No results",
        "RATE_LIMITED" => "Rate limited",
        "BLOCKED" => "Blocked by Google Scholar",
        "NETWORK_ERROR" => "Search failed",
        "PARSING_ERROR" => "Search failed",
        "CANCELLED" => "Search cancelled",
        _ => "Ready",
    }
}

/// Status tone per ExtractionStatus (status_tone, L233).
pub fn status_tone(status: &str) -> &'static str {
    match status {
        "SUCCESS" => "success",
        "PARTIAL_SUCCESS" | "CANCELLED" | "RATE_LIMITED" => "warning",
        "BLOCKED" | "NETWORK_ERROR" | "PARSING_ERROR" | "NO_RESULTS" => "error",
        _ => "ready",
    }
}

/// Default status detail per status when result.message is empty (L202-217).
pub fn default_status_message(status: &str) -> &'static str {
    match status {
        "SUCCESS" => "Run 'Check metadata / PDFs' to look up journal details and public PDFs.",
        "PARTIAL_SUCCESS" => "Some pages could not be retrieved; collected results are kept.",
        "NO_RESULTS" => "Try different keywords or fewer filters.",
        "RATE_LIMITED" => "Google Scholar is throttling requests. Wait a moment, then retry.",
        "BLOCKED" => "Google Scholar is showing a challenge page. Try again later.",
        "NETWORK_ERROR" => "Google Scholar could not be reached. Check the connection.",
        "PARSING_ERROR" => "The results page could not be read; Google Scholar may have changed.",
        "CANCELLED" => "The search was cancelled.",
        _ => "",
    }
}

/// Year filter options (year_filter_options, L125) for the current year.
pub fn year_filter_options(current_year: i32) -> Vec<(String, Option<i32>, Option<i32>)> {
    vec![
        ("Any time".to_string(), None, None),
        (format!("Since {current_year}"), Some(current_year), None),
        (
            format!("Since {}", current_year - 1),
            Some(current_year - 1),
            None,
        ),
        ("Last five years".to_string(), Some(current_year - 4), None),
        ("Custom range".to_string(), None, None),
    ]
}

/// Year range label with en-dash (year_range_label, L136).
pub fn year_range_label(from: Option<i32>, to: Option<i32>) -> String {
    match (from, to) {
        (Some(f), Some(t)) => format!("Years: {f}\u{2013}{t}"),
        (Some(f), None) => format!("Years: since {f}"),
        (None, Some(t)) => format!("Years: through {t}"),
        _ => "Years: any time".to_string(),
    }
}

/// `result_summary` (L248).
pub fn result_summary(
    result_count: usize,
    successful_pages: i32,
    requested_pages: i32,
    duplicates: i32,
    invalid: i32,
) -> String {
    let mut parts = vec![format!(
        "{result_count} result(s) found | {successful_pages}/{requested_pages} pages completed"
    )];
    if duplicates > 0 {
        parts.push(format!("{duplicates} duplicate(s) removed"));
    }
    if invalid > 0 {
        parts.push(format!("{invalid} invalid record(s) removed"));
    }
    parts.join(" | ")
}

/// `progress_value` (L264).
pub fn progress_value(current: i32, total: i32) -> i32 {
    if total <= 0 {
        return 0;
    }
    ((current as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as i32
}

/// `article_row` score formatting (L270).
pub fn score_label(score: Option<f64>) -> String {
    match score {
        Some(v) => format!("{v:.1}"),
        None => String::new(),
    }
}

/// `pdf_status_label` (L281).
pub fn pdf_status_label(status: &str, local_path: &str) -> String {
    match status {
        "available" => {
            if !local_path.is_empty() {
                "Downloaded".to_string()
            } else {
                "Public PDF".to_string()
            }
        }
        "unavailable" => "No public PDF".to_string(),
        _ => "Unknown".to_string(),
    }
}

/// `journal_rank_label` (L276).
pub fn journal_rank_label(sjr: &str, sinta: &str) -> String {
    match (sjr.is_empty(), sinta.is_empty()) {
        (false, false) => format!("{sjr} / {sinta}"),
        (false, true) => sjr.to_string(),
        (true, false) => sinta.to_string(),
        (true, true) => String::new(),
    }
}

/// Window sizing: clamp (w,h) to screen − (20, 80) (`_fitted_window_size`).
pub fn fitted_window_size(screen_w: i32, screen_h: i32) -> (i32, i32) {
    let margin = (20, 80);
    let w = (1080i32).clamp(1, (screen_w - margin.0).max(1));
    let h = (700i32).clamp(1, (screen_h - margin.1).max(1));
    (w, h)
}

pub const WINDOW_MIN_WIDTH: i32 = 860;
pub const WINDOW_MIN_HEIGHT: i32 = 560;

/// Probe sizes from tools/ui_layout_probe.py.
pub const SUPPORTED_SIZES: [(i32, i32); 3] = [(860, 560), (880, 620), (1080, 700)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_labels_use_en_dash() {
        assert_eq!(
            year_range_label(Some(2018), Some(2024)),
            "Years: 2018\u{2013}2024"
        );
        assert_eq!(year_range_label(Some(2018), None), "Years: since 2018");
        assert_eq!(year_range_label(None, Some(2024)), "Years: through 2024");
        assert_eq!(year_range_label(None, None), "Years: any time");
    }

    #[test]
    fn year_options_order() {
        let opts = year_filter_options(2026);
        assert_eq!(opts[0].0, "Any time");
        assert_eq!(opts[1].0, "Since 2026");
        assert_eq!(opts[2].0, "Since 2025");
        assert_eq!(opts[3].0, "Last five years");
        assert_eq!(opts[4].0, "Custom range");
        assert_eq!(opts[3].1, Some(2022));
    }

    #[test]
    fn summary_and_progress() {
        assert_eq!(
            result_summary(3, 2, 3, 1, 0),
            "3 result(s) found | 2/3 pages completed | 1 duplicate(s) removed"
        );
        assert_eq!(progress_value(1, 4), 25);
        assert_eq!(progress_value(1, 0), 0);
        assert_eq!(progress_value(5, 4), 100);
    }

    #[test]
    fn window_sizing_clamps() {
        assert_eq!(fitted_window_size(1920, 1080), (1080, 700));
        // Small screen: width ≤ 1920-20, height ≤ 1080-80.
        let (w, h) = fitted_window_size(900, 600);
        assert_eq!(w, 880);
        assert_eq!(h, 520);
    }

    #[test]
    fn pdf_labels() {
        assert_eq!(pdf_status_label("available", "C:/x.pdf"), "Downloaded");
        assert_eq!(pdf_status_label("available", ""), "Public PDF");
        assert_eq!(pdf_status_label("unavailable", ""), "No public PDF");
        assert_eq!(pdf_status_label("unknown", ""), "Unknown");
    }
}
