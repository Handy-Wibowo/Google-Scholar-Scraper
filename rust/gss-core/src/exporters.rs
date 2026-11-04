//! Exporters ported from `exporters.py` verbatim: Excel/CSV (utf-8-sig,
//! formula defusing), BibTeX/RIS citation writers (byte-compatible escaping,
//! type maps, unique keys, CRLF RIS framing).

use crate::models::{Article, CitationRecord, EXPORT_COLUMNS};
use regex::Regex;
use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;
use std::sync::LazyLock;

static CONTROL_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\t\r\n]+").unwrap());
static BIBTEX_SPECIALS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([{}%$&#_^~\\])").unwrap());
static RIS_SPECIALS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n]+").unwrap());
static WHITESPACE_RUN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static TITLE_WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9]+").unwrap());
static KEY_CLEAN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9_-]").unwrap());

const FORMULA_PREFIXES: [char; 4] = ['=', '+', '-', '@'];

// Crossref work type → (BibTeX entry type, RIS TY value).
fn bibtex_type(entry_type: &str) -> &'static str {
    match entry_type {
        "journal-article" => "article",
        "proceedings-article" => "inproceedings",
        "book-chapter" => "incollection",
        _ => "misc",
    }
}

fn ris_type(entry_type: &str) -> &'static str {
    match entry_type {
        "journal-article" => "JOUR",
        "proceedings-article" => "CONF",
        "book-chapter" => "CHAP",
        _ => "GEN",
    }
}

/// Neutralize formula injection without altering scientific text
/// (`sanitize_cell`): a leading `= + - @` (after control chars) gets `'`.
pub fn sanitize_cell(value: &str) -> String {
    let stripped = CONTROL_PREFIX_RE.replace(value, "");
    if stripped.is_empty() {
        return value.to_string();
    }
    if FORMULA_PREFIXES.contains(&stripped.chars().next().unwrap()) {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

fn cell_display(value: &crate::models::ExportCell) -> String {
    match value {
        crate::models::ExportCell::Str(s) => sanitize_cell(s),
        crate::models::ExportCell::Double(v) => format_score(*v),
        crate::models::ExportCell::Int(v) => v.to_string(),
    }
}

/// Python str(float) for relevance scores: 87.5 → "87.5", 100.0 → "100.0".
fn format_score(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        let s = format!("{v}");
        s
    }
}

/// `save_to_csv`: utf-8-sig BOM, RFC-quoted fields, exact column order.
pub fn save_to_csv(articles: &[Article], filename: &Path) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(filename)?);
    out.write_all(b"\xef\xbb\xbf")?;
    // Header
    let header: Vec<String> = EXPORT_COLUMNS.iter().map(|c| csv_field(c)).collect();
    out.write_all(header.join(",").as_bytes())?;
    out.write_all(b"\r\n")?;
    for article in articles {
        let row = article.to_export_row();
        let line: Vec<String> = row
            .iter()
            .map(|(k, v)| {
                let _ = k;
                csv_field(&cell_display(v))
            })
            .collect();
        out.write_all(line.join(",").as_bytes())?;
        out.write_all(b"\r\n")?;
    }
    out.flush()
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// `save_to_excel`: sheet "Scholar Articles", header = model order,
/// formula defusing identical; numbers stay numeric.
pub fn save_to_excel(articles: &[Article], filename: &Path) -> Result<(), String> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet
        .set_name("Scholar Articles")
        .map_err(|e| e.to_string())?;

    for (col, name) in EXPORT_COLUMNS.iter().enumerate() {
        sheet
            .write_string(0, col as u16, *name)
            .map_err(|e| e.to_string())?;
    }
    for (row_index, article) in articles.iter().enumerate() {
        let row = article.to_export_row();
        for (col, (_name, cell)) in row.iter().enumerate() {
            let row_num = (row_index + 1) as u32;
            let col_num = col as u16;
            let res = match cell {
                crate::models::ExportCell::Str(s) => {
                    sheet.write_string(row_num, col_num, sanitize_cell(s))
                }
                crate::models::ExportCell::Double(v) => sheet.write_number(row_num, col_num, *v),
                crate::models::ExportCell::Int(v) => {
                    sheet.write_number(row_num, col_num, *v as f64)
                }
            };
            res.map_err(|e| e.to_string())?;
        }
    }
    workbook.save(filename).map_err(|e| e.to_string())
}

fn single_line(value: &str) -> String {
    WHITESPACE_RUN_RE.replace_all(value, " ").trim().to_string()
}

fn bibtex_escape(value: &str) -> String {
    BIBTEX_SPECIALS_RE.replace_all(value, "\\$1").into_owned()
}

fn ris_author(value: &str) -> String {
    if !value.contains(' ') {
        return value.to_string();
    }
    // Python rpartition(" "): (head, sep, tail) with tail = last space-separated word.
    let (given, family) = match value.rfind(' ') {
        Some(pos) => (value[..pos].to_string(), value[pos + 1..].to_string()),
        None => (String::new(), value.to_string()),
    };
    if family.trim().is_empty() {
        value.to_string()
    } else {
        format!("{}, {}", family, given.trim())
    }
}

fn ris_pages(pages: &str) -> (String, String) {
    let value = single_line(pages);
    if value.is_empty() {
        return (String::new(), String::new());
    }
    for separator in ['-', '–'] {
        if value.contains(separator) {
            let (first, last) = value.split_once(separator).unwrap();
            return (first.trim().to_string(), last.trim().to_string());
        }
    }
    (value, String::new())
}

fn bibtex_note(record: &CitationRecord) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !record.fallback_authors_line.is_empty() {
        parts.push(record.fallback_authors_line.clone());
    }
    parts.extend(record.warnings.iter().cloned());
    single_line(&parts.join("; "))
}

/// Deterministic, unique citation key: surname_titleword_year[-N]
/// (`_bibtex_key`).
fn bibtex_key(record: &CitationRecord, taken: &mut HashSet<String>) -> String {
    let surname = if !record.authors.is_empty() {
        let first_author = single_line(&record.authors[0]);
        match first_author.rfind(' ') {
            Some(pos) => first_author[pos + 1..].to_string(),
            None => first_author,
        }
    } else {
        "anon".to_string()
    };
    let title_word = TITLE_WORD_RE
        .find(&record.title)
        .map(|m| m.as_str().to_lowercase())
        .unwrap_or_else(|| "title".to_string());
    let year_part = record
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "nd".to_string());
    let base_raw = format!("{surname}_{title_word}_{year_part}")
        .replace(' ', "")
        .to_lowercase();
    let base = {
        let cleaned = KEY_CLEAN_RE.replace_all(&base_raw, "").into_owned();
        if cleaned.is_empty() {
            "citation".to_string()
        } else {
            cleaned
        }
    };
    let mut key = base.clone();
    let mut counter = 2;
    while taken.contains(&key) {
        key = format!("{base}-{counter}");
        counter += 1;
    }
    taken.insert(key.clone());
    key
}

/// `save_to_bibtex`: one entry per record, input order, file overwritten.
pub fn save_to_bibtex(records: &[CitationRecord], filename: &Path) -> std::io::Result<()> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut blocks: Vec<String> = Vec::new();
    for record in records {
        let entry_type = bibtex_type(&record.entry_type);
        let container_field = if entry_type == "article" || entry_type == "misc" {
            "journal"
        } else {
            "booktitle"
        };
        let mut lines = vec![format!(
            "@{}{{{},",
            entry_type,
            bibtex_key(record, &mut taken)
        )];
        if !record.authors.is_empty() {
            lines.push(format!(
                "  author = {{{}}},",
                bibtex_escape(&record.authors.join(" and "))
            ));
        }
        lines.push(format!(
            "  title = {{{{{}}}}},",
            bibtex_escape(&single_line(&record.title))
        ));
        if !record.container.is_empty() {
            lines.push(format!(
                "  {} = {{{}}},",
                container_field,
                bibtex_escape(&record.container)
            ));
        }
        if let Some(year) = record.year {
            lines.push(format!("  year = {{{year}}},"));
        }
        if !record.volume.is_empty() {
            lines.push(format!("  volume = {{{}}},", bibtex_escape(&record.volume)));
        }
        if !record.issue.is_empty() {
            lines.push(format!("  number = {{{}}},", bibtex_escape(&record.issue)));
        }
        if !record.pages.is_empty() {
            lines.push(format!("  pages = {{{}}},", bibtex_escape(&record.pages)));
        }
        if !record.publisher.is_empty() {
            lines.push(format!(
                "  publisher = {{{}}},",
                bibtex_escape(&record.publisher)
            ));
        }
        if !record.doi.is_empty() {
            lines.push(format!("  doi = {{{}}},", record.doi));
        }
        if !record.issns.is_empty() {
            lines.push(format!("  issn = {{{}}},", record.issns.join(", ")));
        }
        if !record.url.is_empty() {
            lines.push(format!("  url = {{{}}},", record.url));
        }
        let note = bibtex_note(record);
        if !note.is_empty() {
            lines.push(format!("  note = {{{}}},", bibtex_escape(&note)));
        }
        blocks.push(format!("{}\n}}\n", lines.join("\n")));
    }

    let mut out = std::io::BufWriter::new(std::fs::File::create(filename)?);
    out.write_all(blocks.join("\n").as_bytes())
}

/// `save_to_ris`: one record per input record, CRLF line endings, `ER  - `
/// terminator; unverified records carry no AU tag, note in N1.
pub fn save_to_ris(records: &[CitationRecord], filename: &Path) -> std::io::Result<()> {
    let mut rendered: Vec<String> = Vec::new();
    for record in records {
        let mut lines = vec![format!("TY  - {}", ris_type(&record.entry_type))];
        for author in &record.authors {
            lines.push(format!("AU  - {}", ris_author(author)));
        }
        lines.push(format!("TI  - {}", single_line(&record.title)));
        if !record.container.is_empty() {
            lines.push(format!("T2  - {}", single_line(&record.container)));
        }
        if let Some(year) = record.year {
            lines.push(format!("PY  - {year}"));
        }
        if !record.volume.is_empty() {
            lines.push(format!("VL  - {}", single_line(&record.volume)));
        }
        if !record.issue.is_empty() {
            lines.push(format!("IS  - {}", single_line(&record.issue)));
        }
        let (first_page, last_page) = ris_pages(&record.pages);
        if !first_page.is_empty() {
            lines.push(format!("SP  - {first_page}"));
        }
        if !last_page.is_empty() {
            lines.push(format!("EP  - {last_page}"));
        }
        if !record.doi.is_empty() {
            lines.push(format!("DO  - {}", record.doi));
        }
        if !record.url.is_empty() {
            lines.push(format!("UR  - {}", record.url));
        }
        for issn in &record.issns {
            lines.push(format!("SN  - {issn}"));
        }
        if !record.publisher.is_empty() {
            lines.push(format!("PB  - {}", single_line(&record.publisher)));
        }
        let mut note_parts: Vec<String> = Vec::new();
        if !record.fallback_authors_line.is_empty() {
            note_parts.push(record.fallback_authors_line.clone());
        }
        note_parts.extend(record.warnings.iter().cloned());
        if !note_parts.is_empty() {
            let joined = RIS_SPECIALS_RE
                .replace_all(&note_parts.join(" | "), " ")
                .into_owned();
            lines.push(format!("N1  - {joined}"));
        }
        lines.push("ER  - ".to_string());
        rendered.push(lines.join("\r\n"));
    }

    let mut out = std::io::BufWriter::new(std::fs::File::create(filename)?);
    out.write_all(rendered.join("\r\n").as_bytes())
}
