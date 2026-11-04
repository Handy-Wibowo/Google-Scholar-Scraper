//! Rich exports that sit outside the parity-gated surface.
//!
//! `save_to_csv`, `save_to_bibtex` and `save_to_ris` are byte-compared against
//! the Python oracle by `tools/parity_check.py` gate 4. Every writer here is a
//! **separate** entry point, so the gated files stay identical while the new
//! formats, formatting and column sets can grow freely.
//!
//! `save_to_excel` is the exception this module specialises: the gate does not
//! compare XLSX bytes, so the rich workbook may replace the plain one.

use crate::models::{Article, CitationRecord, EXPORT_COLUMNS};
use std::collections::BTreeMap;
use std::path::Path;

/// Column keys the UI can ask for, in the order they appear in the sheet.
pub const SELECTABLE_COLUMNS: [&str; 20] = EXPORT_COLUMNS;

// ---------------------------------------------------------------- Excel ----

/// A rich workbook: frozen header, autofilter, sized columns, hyperlinks,
/// colour-coded journal rank and PDF status, plus a summary sheet.
///
/// `columns` selects and orders the data columns; an empty slice means all.
/// `group_by` adds one worksheet per distinct value of `"year"`, `"journal"` or
/// `"pdf"`, written after the main sheet.
pub fn save_to_excel_rich(
    articles: &[Article],
    filename: &Path,
    columns: &[String],
    group_by: Option<&str>,
) -> Result<(), String> {
    use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook};

    let selected = select_columns(columns);
    if selected.is_empty() {
        return Err("No columns selected for export.".to_string());
    }

    let mut workbook = Workbook::new();

    let header = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0xEE_F3_F8))
        .set_border(FormatBorder::Thin)
        .set_align(FormatAlign::Left)
        .set_text_wrap();
    let q1 = Format::new().set_background_color(Color::RGB(0xD6_F5_D8));
    let q2 = Format::new().set_background_color(Color::RGB(0xE8_F6_D0));
    let q3 = Format::new().set_background_color(Color::RGB(0xFF_F6_CC));
    let q4 = Format::new().set_background_color(Color::RGB(0xFF_E3_E0));
    let pdf_yes = Format::new().set_background_color(Color::RGB(0xD6_F5_D8));
    let pdf_no = Format::new().set_background_color(Color::RGB(0xF1_F4_F8));
    let retracted = Format::new()
        .set_background_color(Color::RGB(0xCF_22_2E))
        .set_font_color(Color::White)
        .set_bold();
    let wrap = Format::new().set_text_wrap();

    let sheet = workbook.add_worksheet();
    sheet
        .set_name("Scholar Articles")
        .map_err(|e| e.to_string())?;

    // Header row.
    for (col, (label, _)) in selected.iter().enumerate() {
        sheet
            .write_string_with_format(0, col as u16, label.as_str(), &header)
            .map_err(|e| e.to_string())?;
    }
    // Widths: title wide and wrapped, authors wide, the rest compact.
    for (col, (label, _)) in selected.iter().enumerate() {
        let width = match label.as_str() {
            "Title" => 60.0,
            "Authors" | "Journal" | "SJR Category" | "PDF Detail" => 32.0,
            "Link" | "DOI" | "PDF URL" | "PDF Source" | "PDF Local Path" | "SJR Source"
            | "SINTA Source" => 40.0,
            _ => 14.0,
        };
        sheet
            .set_column_width(col as u16, width)
            .map_err(|e| e.to_string())?;
    }

    write_rows(
        sheet,
        articles,
        &selected,
        &RichFormats {
            wrap: &wrap,
            q1: &q1,
            q2: &q2,
            q3: &q3,
            q4: &q4,
            pdf_yes: &pdf_yes,
            pdf_no: &pdf_no,
            retracted: &retracted,
        },
    )?;

    // Freeze the header row and the title column.
    sheet.set_freeze_panes(1, 1).map_err(|e| e.to_string())?;
    sheet
        .autofilter(0, 0, articles.len() as u32, (selected.len() - 1) as u16)
        .map_err(|e| e.to_string())?;

    if let Some(key) = group_by {
        write_groups(&mut workbook, articles, &selected, key).map_err(|e| e.to_string())?;
    }
    write_summary(&mut workbook, articles, &header).map_err(|e| e.to_string())?;

    workbook.save(filename).map_err(|e| e.to_string())
}

/// Formats threaded into the row writer.
struct RichFormats<'a> {
    wrap: &'a rust_xlsxwriter::Format,
    q1: &'a rust_xlsxwriter::Format,
    q2: &'a rust_xlsxwriter::Format,
    q3: &'a rust_xlsxwriter::Format,
    q4: &'a rust_xlsxwriter::Format,
    pdf_yes: &'a rust_xlsxwriter::Format,
    pdf_no: &'a rust_xlsxwriter::Format,
    retracted: &'a rust_xlsxwriter::Format,
}

/// Resolve requested column labels against `EXPORT_COLUMNS`.
///
/// Unknown labels are dropped rather than erroring: a stale UI preference must
/// not stop an export that would otherwise succeed.
fn select_columns(requested: &[String]) -> Vec<(String, String)> {
    let wanted: Vec<String> = requested
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    EXPORT_COLUMNS
        .iter()
        .filter(|name| wanted.is_empty() || wanted.iter().any(|w| w == *name))
        .map(|name| ((*name).to_string(), (*name).to_string()))
        .collect()
}

fn write_rows(
    sheet: &mut rust_xlsxwriter::Worksheet,
    articles: &[Article],
    selected: &[(String, String)],
    fmt: &RichFormats<'_>,
) -> Result<(), String> {
    for (row_index, article) in articles.iter().enumerate() {
        let row = (row_index + 1) as u32;
        let row_cells = article.to_export_row();
        let is_retracted = !article.retraction.trim().is_empty();
        for (col, (label, _)) in selected.iter().enumerate() {
            let col_num = col as u16;
            let Some((_, cell)) = row_cells.iter().find(|(name, _)| name == label) else {
                continue;
            };
            let text = match cell {
                crate::models::ExportCell::Str(s) => crate::exporters::sanitize_cell(s),
                crate::models::ExportCell::Double(v) => v.to_string(),
                crate::models::ExportCell::Int(v) => v.to_string(),
            };

            // Hyperlinks: the visible text stays the URL/DOI, the target is the
            // canonical destination.
            let link_target = match label.as_str() {
                "Link" if !text.is_empty() => Some(text.clone()),
                "DOI" if !text.is_empty() => Some(format!("https://doi.org/{text}")),
                _ => None,
            };
            if let Some(target) = link_target {
                // Keep the visible text as the URL/DOI but make the cell a
                // hyperlink; the title cell keeps its wrap format.
                sheet
                    .write_string_with_format(row, col_num, &text, fmt.wrap)
                    .map_err(|e| e.to_string())?;
                if let Err(e) = sheet.write_url(row, col_num, target.as_str()) {
                    // A malformed URL must not abort the whole export.
                    let _ = e;
                    sheet
                        .write_string_with_format(row, col_num, &text, fmt.wrap)
                        .map_err(|err| err.to_string())?;
                }
                continue;
            }

            let style = if is_retracted && label == "Title" {
                Some(fmt.retracted)
            } else {
                match label.as_str() {
                    "SJR Quartile" | "SINTA Rank" => match text.as_str() {
                        "Q1" | "S1" => Some(fmt.q1),
                        "Q2" | "S2" => Some(fmt.q2),
                        "Q3" | "S3" => Some(fmt.q3),
                        "Q4" | "S4" => Some(fmt.q4),
                        _ => None,
                    },
                    "PDF Status" => match text.as_str() {
                        "available" => Some(fmt.pdf_yes),
                        "unavailable" => Some(fmt.pdf_no),
                        _ => None,
                    },
                    "Title" => Some(fmt.wrap),
                    _ => None,
                }
            };
            match style {
                Some(f) => sheet
                    .write_string_with_format(row, col_num, &text, f)
                    .map_err(|e| e.to_string())?,
                None => sheet
                    .write_string(row, col_num, &text)
                    .map_err(|e| e.to_string())?,
            };
            // (style is Option<&Format>; both arms above are valid writers)
        }
    }
    Ok(())
}

/// One worksheet per distinct group value.
fn write_groups(
    workbook: &mut rust_xlsxwriter::Workbook,
    articles: &[Article],
    selected: &[(String, String)],
    key: &str,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    let mut groups: BTreeMap<String, Vec<&Article>> = BTreeMap::new();
    for article in articles {
        let value = match key {
            "year" => article
                .publication_year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "Unknown year".to_string()),
            "journal" => {
                let j = article.journal.trim();
                if j.is_empty() {
                    "Unknown journal".to_string()
                } else {
                    j.to_string()
                }
            }
            "pdf" => match article.pdf_status.as_str() {
                "available" => "PDF available".to_string(),
                "unavailable" => "No public PDF".to_string(),
                _ => "PDF unknown".to_string(),
            },
            _ => return Ok(()),
        };
        groups.entry(value).or_default().push(article);
    }
    // Excel sheet names: ≤31 chars, no []:*?/\
    fn safe_name(raw: &str, used: &mut Vec<String>) -> String {
        let mut base: String = raw
            .chars()
            .map(|c| if "[]:*?/\\".contains(c) { '-' } else { c })
            .take(28)
            .collect();
        if base.trim().is_empty() {
            base = "Sheet".to_string();
        }
        let mut candidate = base.clone();
        let mut n = 2;
        while used.iter().any(|u| u.eq_ignore_ascii_case(&candidate)) {
            candidate = format!("{}-{}", base, n);
            n += 1;
        }
        used.push(candidate.clone());
        candidate
    }

    let mut used: Vec<String> = vec!["Scholar Articles".to_string(), "Summary".to_string()];
    for (value, rows) in groups {
        let name = safe_name(&value, &mut used);
        let sheet = workbook.add_worksheet();
        sheet.set_name(&name)?;
        for (col, (label, _)) in selected.iter().enumerate() {
            sheet.write_string(0, col as u16, label.as_str())?;
        }
        for (row_index, article) in rows.iter().enumerate() {
            let row_cells = article.to_export_row();
            for (col, (label, _)) in selected.iter().enumerate() {
                let Some((_, cell)) = row_cells.iter().find(|(name, _)| name == label) else {
                    continue;
                };
                let text = match cell {
                    crate::models::ExportCell::Str(s) => crate::exporters::sanitize_cell(s),
                    crate::models::ExportCell::Double(v) => v.to_string(),
                    crate::models::ExportCell::Int(v) => v.to_string(),
                };
                sheet.write_string((row_index + 1) as u32, col as u16, &text)?;
            }
        }
    }
    Ok(())
}

/// Counts by year, rank and PDF status — the first thing a supervisor asks.
fn write_summary(
    workbook: &mut rust_xlsxwriter::Workbook,
    articles: &[Article],
    header: &rust_xlsxwriter::Format,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    let sheet = workbook.add_worksheet();
    sheet.set_name("Summary")?;
    sheet.set_column_width(0, 34)?;
    sheet.set_column_width(1, 12)?;
    sheet.set_column_width(2, 34)?;
    sheet.set_column_width(3, 12)?;

    let titles = ["Records", "Count", "PDF status", "Count"];
    for (col, title) in titles.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *title, header)?;
    }

    let total = articles.len();
    let by_rank = count_by(articles, |a| {
        let q = a.sjr_quartile.trim();
        let s = a.sinta_rank.trim();
        if !q.is_empty() {
            format!("SJR {q}")
        } else if !s.is_empty() {
            format!("SINTA {s}")
        } else {
            "Unranked / unknown".to_string()
        }
    });
    let by_pdf = count_by(articles, |a| match a.pdf_status.as_str() {
        "available" => "Public PDF available".to_string(),
        "unavailable" => "No public PDF found".to_string(),
        _ => "Unknown (not checked)".to_string(),
    });

    sheet.write_string(1, 0, "Total records")?;
    sheet.write_number(1, 1, total as f64)?;
    for (offset, (label, count)) in by_pdf.iter().enumerate() {
        let row = (offset + 1) as u32;
        sheet.write_string(row, 2, label)?;
        sheet.write_number(row, 3, *count as f64)?;
    }
    for (offset, (label, count)) in by_rank.iter().enumerate() {
        let row = (offset + 1) as u32 + by_pdf.len() as u32 + 1;
        sheet.write_string(row, 0, label)?;
        sheet.write_number(row, 1, *count as f64)?;
    }
    Ok(())
}

fn count_by<F: Fn(&Article) -> String>(articles: &[Article], key: F) -> Vec<(String, usize)> {
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    for article in articles {
        *map.entry(key(article)).or_insert(0) += 1;
    }
    map.into_iter().collect()
}

// ------------------------------------------------------- tabular text ------

/// JSON array of the exported records (the columns the UI selected).
pub fn save_to_json(
    articles: &[Article],
    filename: &Path,
    columns: &[String],
) -> Result<(), String> {
    let selected = select_columns(columns);
    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(articles.len());
    for article in articles {
        let row = article.to_export_row();
        let mut object = serde_json::Map::new();
        for (label, _) in &selected {
            if let Some((name, cell)) = row.iter().find(|(n, _)| *n == label.as_str()) {
                let value = match cell {
                    crate::models::ExportCell::Str(s) => serde_json::Value::String(s.clone()),
                    crate::models::ExportCell::Double(v) => {
                        serde_json::json!(v)
                    }
                    crate::models::ExportCell::Int(v) => serde_json::json!(v),
                };
                object.insert(name.to_string(), value);
            }
        }
        rows.push(serde_json::Value::Object(object));
    }
    let body = serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?;
    std::fs::write(filename, body).map_err(|e| e.to_string())
}

/// Markdown table of the selected columns.
pub fn save_to_markdown(
    articles: &[Article],
    filename: &Path,
    columns: &[String],
) -> Result<(), String> {
    let selected = select_columns(columns);
    let mut out = String::new();
    out.push('|');
    for (label, _) in &selected {
        out.push_str(&format!(" {label} |"));
    }
    out.push('\n');
    out.push('|');
    for _ in &selected {
        out.push_str(" --- |");
    }
    out.push('\n');
    for article in articles {
        let row = article.to_export_row();
        out.push('|');
        for (label, _) in &selected {
            let text = row
                .iter()
                .find(|(n, _)| n == label)
                .map(|(_, c)| cell_text(c))
                .unwrap_or_default();
            // Pipes would break the table; newlines would break the row.
            let safe = text.replace('|', "\\|").replace(['\n', '\r'], " ");
            out.push_str(&format!(" {safe} |"));
        }
        out.push('\n');
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

/// Tab-separated values of the selected columns (no quoted-field machinery).
pub fn save_to_tsv(
    articles: &[Article],
    filename: &Path,
    columns: &[String],
) -> Result<(), String> {
    let selected = select_columns(columns);
    let mut out = String::new();
    out.push_str(
        &selected
            .iter()
            .map(|(l, _)| l.replace('\t', " "))
            .collect::<Vec<_>>()
            .join("\t"),
    );
    out.push_str("\r\n");
    for article in articles {
        let row = article.to_export_row();
        let cells: Vec<String> = selected
            .iter()
            .map(|(label, _)| {
                row.iter()
                    .find(|(n, _)| n == label)
                    .map(|(_, c)| cell_text(c).replace(['\t', '\n', '\r'], " "))
                    .unwrap_or_default()
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push_str("\r\n");
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

/// EndNote ENW (`%0` reference type, `%A` author, `%T` title, …).
pub fn save_to_enw(
    articles: &[Article],
    filename: &Path,
    columns: &[String],
) -> Result<(), String> {
    let _ = select_columns(columns); // ENW is record-shaped, not column-shaped
    let mut out = String::new();
    for article in articles {
        out.push_str("%0 Journal Article\r\n");
        // `authors_structured` is populated only by a Crossref match; the raw
        // Scholar line is never split into %A fields (SCHOLAR_AUTHORS_WARNING).
        for author in &article.authors_structured {
            out.push_str(&format!("%A {author}\r\n"));
        }
        if article.authors_structured.is_empty() && !article.authors.trim().is_empty() {
            out.push_str(&format!("%O {}\r\n", article.authors));
        }
        out.push_str(&format!("%T {}\r\n", article.title));
        if !article.journal.is_empty() {
            out.push_str(&format!("%J {}\r\n", article.journal));
        }
        if let Some(year) = article.publication_year {
            out.push_str(&format!("%D {year}\r\n"));
        }
        if !article.volume.is_empty() {
            out.push_str(&format!("%V {}\r\n", article.volume));
        }
        if !article.issue.is_empty() {
            out.push_str(&format!("%N {}\r\n", article.issue));
        }
        if !article.pages.is_empty() {
            out.push_str(&format!("%P {}\r\n", article.pages));
        }
        if !article.doi.is_empty() {
            out.push_str(&format!("%R {}\r\n", article.doi));
        }
        if !article.link.is_empty() {
            out.push_str(&format!("%U {}\r\n", article.link));
        }
        out.push_str("\r\n");
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

/// The details-dialog text for every record, one file.
pub fn save_details_text(articles: &[Article], filename: &Path) -> Result<(), String> {
    let mut out = String::new();
    for (index, article) in articles.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
            out.push_str(&"=".repeat(70));
            out.push_str("\n\n");
        }
        out.push_str(&article_detail_block(article));
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

// -------------------------------------------------------- bibliography -----

/// CSL-JSON — the interchange format Zotero, Mendeley, Paperpile and Pandoc read.
pub fn save_to_csl_json(records: &[CitationRecord], filename: &Path) -> Result<(), String> {
    let items: Vec<serde_json::Value> = records.iter().map(csl_item).collect();
    let body = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
    std::fs::write(filename, body).map_err(|e| e.to_string())
}

fn csl_item(record: &CitationRecord) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert(
        "id".into(),
        serde_json::Value::String(if record.doi.is_empty() {
            record.title.clone()
        } else {
            record.doi.clone()
        }),
    );
    // CSL type names differ from Crossref's; a journal article is the common case.
    item.insert(
        "type".into(),
        serde_json::Value::String(match record.entry_type.as_str() {
            "book-chapter" | "proceedings-article" => "paper-conference".to_string(),
            "book" | "monograph" => "book".to_string(),
            _ => "article-journal".to_string(),
        }),
    );
    item.insert(
        "title".into(),
        serde_json::Value::String(record.title.clone()),
    );
    // Only verified (Crossref-resolved) authors may be structured. An unresolved
    // record carries the raw Google Scholar citation line, which mixes authors,
    // venue and year — splitting it would fabricate structured names. That line
    // travels in the note instead, matching SCHOLAR_AUTHORS_WARNING.
    if !record.authors.is_empty() && record.verified {
        let authors: Vec<serde_json::Value> = record
            .authors
            .iter()
            .map(|name| {
                let (family, given) = split_name(name);
                let mut person = serde_json::Map::new();
                if !family.is_empty() {
                    person.insert("family".into(), serde_json::Value::String(family));
                }
                if !given.is_empty() {
                    person.insert("given".into(), serde_json::Value::String(given));
                }
                if person.is_empty() {
                    person.insert("literal".into(), serde_json::Value::String(name.clone()));
                }
                serde_json::Value::Object(person)
            })
            .collect();
        item.insert("author".into(), serde_json::Value::Array(authors));
    }
    if let Some(year) = record.year {
        item.insert("issued".into(), serde_json::json!({"date-parts": [[year]]}));
    }
    let put = |key: &str, value: &str, item: &mut serde_json::Map<String, serde_json::Value>| {
        if !value.trim().is_empty() {
            item.insert(key.into(), serde_json::Value::String(value.to_string()));
        }
    };
    put("container-title", &record.container, &mut item);
    put("volume", &record.volume, &mut item);
    put("issue", &record.issue, &mut item);
    put("page", &record.pages, &mut item);
    put("publisher", &record.publisher, &mut item);
    put("DOI", &record.doi, &mut item);
    put("URL", &record.url, &mut item);
    if !record.issns.is_empty() {
        item.insert(
            "ISSN".into(),
            serde_json::Value::String(record.issns.join(", ")),
        );
    }
    if !record.verified {
        item.insert(
            "note".into(),
            serde_json::Value::String(record.warnings.join(" ")),
        );
    }
    serde_json::Value::Object(item)
}

/// BibTeX with `file = {...}` attachment fields, as a **separate** writer so the
/// parity-gated `save_to_bibtex` output stays byte-identical.
pub fn save_to_bibtex_with_files(
    records: &[CitationRecord],
    articles: &[Article],
    filename: &Path,
) -> Result<(), String> {
    // Reuse the gated writer for the body, then inject the file field.
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let staged = dir.path().join("staged.bib");
    crate::exporters::save_to_bibtex(records, &staged).map_err(|e| e.to_string())?;
    let body = std::fs::read_to_string(&staged).map_err(|e| e.to_string())?;

    // Map DOI -> local path.
    let paths: BTreeMap<String, String> = articles
        .iter()
        .filter(|a| !a.pdf_local_path.trim().is_empty())
        .map(|a| (a.doi.trim().to_lowercase(), a.pdf_local_path.clone()))
        .collect();

    let mut out = String::with_capacity(body.len() + 256);
    for block in body.split("\n\n") {
        let trimmed = block.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let doi = trimmed
            .lines()
            .find_map(|line| {
                let line = line.trim();
                line.strip_prefix("doi = {")
                    .and_then(|rest| rest.strip_suffix("},"))
            })
            .unwrap_or("")
            .trim()
            .to_lowercase();
        match paths.get(&doi) {
            Some(path) => {
                // Insert before the closing brace of the entry.
                let (head, tail) = match trimmed.rfind('}') {
                    Some(pos) => (&trimmed[..pos], &trimmed[pos..]),
                    None => (trimmed, ""),
                };
                out.push_str(head);
                out.push_str(&format!("  file = {{{path}}},\n"));
                out.push_str(tail);
            }
            None => out.push_str(trimmed),
        }
        out.push_str("\n\n");
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

/// A BibTeX key per DOI that stays stable across exports.
///
/// `save_to_bibtex` allocates keys from a set rebuilt per call, so adding one
/// record renumbers later entries and breaks `\cite{}` in a written manuscript.
/// This writer consults a caller-supplied map first and only generates a key for
/// a DOI it has not seen, returning the updated map for persistence.
pub fn save_to_bibtex_stable(
    records: &[CitationRecord],
    filename: &Path,
    keys: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let mut body = String::new();
    let mut taken: Vec<String> = keys.values().cloned().collect();
    for record in records {
        let doi = record.doi.trim().to_lowercase();
        let key = match keys.get(&doi) {
            Some(existing) => existing.clone(),
            None => {
                let generated = unique_key(record, &taken);
                taken.push(generated.clone());
                if !doi.is_empty() {
                    keys.insert(doi, generated.clone());
                }
                generated
            }
        };
        body.push_str(&render_bibtex_entry(record, &key));
        body.push('\n');
    }
    std::fs::write(filename, body).map_err(|e| e.to_string())
}

fn unique_key(record: &CitationRecord, taken: &[String]) -> String {
    let first = record
        .authors
        .first()
        .map(|a| a.split_whitespace().last().unwrap_or("anon").to_lowercase())
        .unwrap_or_else(|| "anon".to_string());
    let word = record
        .title
        .split_whitespace()
        .find(|w| w.chars().any(|c| c.is_alphabetic()))
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .unwrap_or_else(|| "untitled".to_string());
    let year = record.year.map(|y| y.to_string()).unwrap_or_default();
    let base = format!(
        "{}_{}{}",
        sanitize_key(&first),
        sanitize_key(&word),
        if year.is_empty() {
            String::new()
        } else {
            format!("_{year}")
        }
    );
    let mut candidate = base.clone();
    let mut n = 1;
    while taken.iter().any(|t| t == &candidate) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    candidate
}

fn sanitize_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
}

fn render_bibtex_entry(record: &CitationRecord, key: &str) -> String {
    let entry_type = match record.entry_type.as_str() {
        "journal-article" | "" => "article",
        "book-chapter" | "proceedings-article" => "inproceedings",
        "book" | "monograph" => "book",
        _ => "misc",
    };
    let mut out = format!("@{entry_type}{{{key},\n");
    if !record.authors.is_empty() {
        out.push_str(&format!(
            "  author = {{{}}},\n",
            record.authors.join(" and ")
        ));
    }
    out.push_str(&format!("  title = {{{{{}}}}},\n", record.title));
    if !record.container.is_empty() {
        out.push_str(&format!("  journal = {{{}}},\n", record.container));
    }
    if let Some(year) = record.year {
        out.push_str(&format!("  year = {{{year}}},\n"));
    }
    if !record.volume.is_empty() {
        out.push_str(&format!("  volume = {{{}}},\n", record.volume));
    }
    if !record.issue.is_empty() {
        out.push_str(&format!("  number = {{{}}},\n", record.issue));
    }
    if !record.pages.is_empty() {
        out.push_str(&format!("  pages = {{{}}},\n", record.pages));
    }
    if !record.doi.is_empty() {
        out.push_str(&format!("  doi = {{{}}},\n", record.doi));
    }
    if !record.url.is_empty() {
        out.push_str(&format!("  url = {{{}}},\n", record.url));
    }
    if !record.warnings.is_empty() {
        out.push_str(&format!("  note = {{{}}},\n", record.warnings.join(" ")));
    }
    out.push_str("}\n");
    out
}

// ------------------------------------------------------ formatted citations -

/// Inline citation styles the app can render without a CSL engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CitationStyle {
    Apa,
    Ieee,
    Vancouver,
}

impl CitationStyle {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "apa" => Some(Self::Apa),
            "ieee" => Some(Self::Ieee),
            "vancouver" => Some(Self::Vancouver),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apa => "apa",
            Self::Ieee => "ieee",
            Self::Vancouver => "vancouver",
        }
    }
}

/// One ready-to-paste reference line.
pub fn format_citation(record: &CitationRecord, style: CitationStyle) -> String {
    let authors = &record.authors;
    let title = record.title.trim();
    let container = record.container.trim();
    let year = record
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "n.d.".to_string());
    let volume = record.volume.trim();
    let issue = record.issue.trim();
    let pages = record.pages.trim();
    let doi = record.doi.trim();

    let tail = {
        let mut parts: Vec<String> = Vec::new();
        if !volume.is_empty() {
            parts.push(if issue.is_empty() {
                volume.to_string()
            } else {
                format!("{volume}({issue})")
            });
        }
        if !pages.is_empty() {
            parts.push(pages.to_string());
        }
        if !doi.is_empty() {
            parts.push(format!("https://doi.org/{doi}"));
        }
        parts.join(", ")
    };

    match style {
        // Author, A. A. (Year). Title. Journal, Volume(Issue), pages. doi
        CitationStyle::Apa => {
            let names = format_authors_apa(authors);
            let mut out = String::new();
            if !names.is_empty() {
                out.push_str(&format!("{names} ({year}). "));
            } else {
                out.push_str(&format!("({year}). "));
            }
            out.push_str(title);
            if !out.ends_with('.') {
                out.push('.');
            }
            if !container.is_empty() {
                out.push_str(&format!(" {container}"));
                if !tail.is_empty() {
                    out.push_str(&format!(", {tail}"));
                }
                if !out.ends_with('.') {
                    out.push('.');
                }
            } else if !tail.is_empty() {
                out.push_str(&format!(" {tail}."));
            }
            out
        }
        // A. Author, B. Author, "Title," Journal, vol. V, no. N, pp. P, Year.
        CitationStyle::Ieee => {
            let names = format_authors_ieee(authors);
            let mut out = String::new();
            if !names.is_empty() {
                out.push_str(&names);
                out.push_str(", ");
            }
            out.push_str(&format!("\"{title},\""));
            if !container.is_empty() {
                out.push_str(&format!(" {container},"));
            }
            if !volume.is_empty() {
                out.push_str(&format!(" vol. {volume},"));
            }
            if !issue.is_empty() {
                out.push_str(&format!(" no. {issue},"));
            }
            if !pages.is_empty() {
                out.push_str(&format!(" pp. {pages},"));
            }
            out.push_str(&format!(" {year}"));
            if !doi.is_empty() {
                out.push_str(&format!(", doi: {doi}"));
            }
            out.push('.');
            out
        }
        // Author AA, Author BB. Title. Journal. Year;Volume(Issue):pages.
        CitationStyle::Vancouver => {
            let names = format_authors_vancouver(authors);
            let mut out = String::new();
            if !names.is_empty() {
                out.push_str(&names);
                out.push_str(". ");
            }
            out.push_str(title);
            if !out.ends_with('.') {
                out.push('.');
            }
            if !container.is_empty() {
                out.push_str(&format!(" {container}."));
            }
            out.push_str(&format!(" {year}"));
            if !volume.is_empty() {
                out.push_str(&format!(";{volume}"));
                if !issue.is_empty() {
                    out.push_str(&format!("({issue})"));
                }
                if !pages.is_empty() {
                    out.push_str(&format!(":{pages}"));
                }
            }
            out.push('.');
            if !doi.is_empty() {
                out.push_str(&format!(" doi:{doi}."));
            }
            out
        }
    }
}

fn split_name(value: &str) -> (String, String) {
    match value.rfind(' ') {
        Some(pos) => (
            value[pos + 1..].to_string(),
            value[..pos].trim().to_string(),
        ),
        None => (value.to_string(), String::new()),
    }
}

/// "A. Author, B. Author & C. Author" with an APA-style trailing `&`.
fn format_authors_apa(authors: &[String]) -> String {
    if authors.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = authors
        .iter()
        .map(|name| {
            let (family, given) = split_name(name);
            if given.is_empty() {
                family
            } else {
                let initials: String = given
                    .split_whitespace()
                    .filter_map(|w| w.chars().next())
                    .map(|c| format!("{}.", c.to_uppercase()))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{family}, {initials}")
            }
        })
        .collect();
    if parts.len() > 1 {
        let last = parts.pop().unwrap();
        format!("{} & {}", parts.join(", "), last)
    } else {
        parts.pop().unwrap()
    }
}

/// IEEE lists up to six names, then `et al.`
fn format_authors_ieee(authors: &[String]) -> String {
    let names: Vec<String> = authors
        .iter()
        .take(6)
        .map(|name| {
            let (family, given) = split_name(name);
            if given.is_empty() {
                family
            } else {
                let initials: String = given
                    .split_whitespace()
                    .filter_map(|w| w.chars().next())
                    .map(|c| format!("{}. ", c.to_uppercase()))
                    .collect();
                format!("{}{}", initials, family)
            }
        })
        .collect();
    let joined = names.join(", ");
    if authors.len() > 6 {
        format!("{joined}, et al.")
    } else {
        joined
    }
}

fn format_authors_vancouver(authors: &[String]) -> String {
    let names: Vec<String> = authors
        .iter()
        .take(6)
        .map(|name| {
            let (family, given) = split_name(name);
            let initials: String = given
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .map(|c| c.to_uppercase().to_string())
                .collect();
            if initials.is_empty() {
                family
            } else {
                format!("{family} {initials}")
            }
        })
        .collect();
    let joined = names.join(", ");
    if authors.len() > 6 {
        format!("{joined}, et al")
    } else {
        joined
    }
}

// ------------------------------------------------------------- sharing -----

/// Markdown bibliography for pasting into chat, a wiki or an email.
pub fn bibliography_markdown(records: &[CitationRecord], style: CitationStyle) -> String {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| format!("{}. {}", index + 1, format_citation(record, style)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Self-contained HTML reading list: opens in any browser, no app required.
pub fn save_reading_list_html(
    articles: &[Article],
    filename: &Path,
    title: &str,
) -> Result<(), String> {
    let mut rows = String::new();
    for article in articles {
        let badge = if article.retraction.trim().is_empty() {
            String::new()
        } else {
            format!(
                "<span class=\"retracted\">{}</span> ",
                escape_html(&article.retraction.to_uppercase())
            )
        };
        let doi = if article.doi.trim().is_empty() {
            String::new()
        } else {
            format!(
                " · <a href=\"https://doi.org/{}\">doi</a>",
                escape_html(&article.doi)
            )
        };
        let pdf = if article.pdf_url.trim().is_empty() {
            String::new()
        } else {
            format!(" · <a href=\"{}\">pdf</a>", escape_html(&article.pdf_url))
        };
        let link = if article.link.trim().is_empty() {
            String::new()
        } else {
            format!(" · <a href=\"{}\">source</a>", escape_html(&article.link))
        };
        let rank = {
            let q = article.sjr_quartile.trim();
            let s = article.sinta_rank.trim();
            if !q.is_empty() {
                format!("<span class=\"rank\">SJR {}</span>", escape_html(q))
            } else if !s.is_empty() {
                format!("<span class=\"rank\">SINTA {}</span>", escape_html(s))
            } else {
                String::new()
            }
        };
        rows.push_str(&format!(
            "<tr><td class=\"t\">{badge}<a href=\"{}\">{}</a><div class=\"meta\">{} · {}{}{}{}</div></td><td class=\"r\">{rank}</td></tr>\n",
            escape_html(&article.link),
            escape_html(&article.title),
            escape_html(&article.authors),
            article
                .publication_year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "year unknown".to_string()),
            doi,
            pdf,
            link,
        ));
    }
    let html = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>{}</title><style>\
body{{font:14px/1.5 -apple-system,Segoe UI,system-ui,sans-serif;max-width:1100px;margin:32px auto;padding:0 16px;color:#111827;background:#f8fafc}}\
h1{{font-size:20px;margin:0 0 4px}}p.sub{{color:#5b677a;margin:0 0 20px}}\
table{{border-collapse:collapse;width:100%}}\
td{{padding:8px 10px;border-bottom:1px solid #d8e0ea;vertical-align:top}}\
td.r{{white-space:nowrap;text-align:right;width:110px}}\
a{{color:#185abc;text-decoration:none}}a:hover{{text-decoration:underline}}\
.meta{{color:#5b677a;font-size:12px;margin-top:2px}}\
.rank{{background:#dbeafe;border-radius:3px;padding:1px 6px;font-size:11px;font-weight:600}}\
.retracted{{background:#cf222e;color:#fff;border-radius:3px;padding:1px 5px;font-size:10px;font-weight:700}}\
</style></head><body><h1>{}</h1><p class=\"sub\">{} record(s) · exported {} · ranking labels describe the journal, not the paper</p>\
<table>{}</table></body></html>",
        escape_html(title),
        escape_html(title),
        articles.len(),
        utc_stamp(),
        rows
    );
    std::fs::write(filename, html).map_err(|e| e.to_string())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02} UTC")
}

/// Bibliography grouped under headings (year, journal, or rank).
pub fn save_grouped_bibliography(
    records: &[CitationRecord],
    articles: &[Article],
    filename: &Path,
    group_by: &str,
    style: CitationStyle,
) -> Result<(), String> {
    // Records and articles share order, so grouping can read either one.
    let mut groups: BTreeMap<String, Vec<&CitationRecord>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        let key = match group_by {
            "year" => record
                .year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "Year unknown".to_string()),
            "journal" => {
                let j = record.container.trim();
                if j.is_empty() {
                    "Journal unknown".to_string()
                } else {
                    j.to_string()
                }
            }
            "rank" => articles
                .get(index)
                .map(|a| {
                    let q = a.sjr_quartile.trim();
                    let s = a.sinta_rank.trim();
                    if !q.is_empty() {
                        format!("SJR {q}")
                    } else if !s.is_empty() {
                        format!("SINTA {s}")
                    } else {
                        "Unranked / unknown".to_string()
                    }
                })
                .unwrap_or_else(|| "Unranked / unknown".to_string()),
            _ => "All references".to_string(),
        };
        groups.entry(key).or_default().push(record);
    }

    let mut out = String::new();
    for (heading, items) in groups {
        out.push_str(&format!("## {heading}\n\n"));
        for (index, record) in items.iter().enumerate() {
            out.push_str(&format!(
                "{}. {}\n",
                index + 1,
                format_citation(record, style)
            ));
        }
        out.push('\n');
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

// ------------------------------------------------------ journal aggregation -

/// One row per journal: records, ISSNs, best quartile, mean year, PDF coverage.
pub fn save_journal_aggregation(articles: &[Article], filename: &Path) -> Result<(), String> {
    let mut map: BTreeMap<String, JournalAgg> = BTreeMap::new();
    for article in articles {
        let key = {
            let j = article.journal.trim();
            if j.is_empty() {
                "(no journal recorded)".to_string()
            } else {
                j.to_lowercase()
            }
        };
        let entry = map.entry(key).or_default();
        if entry.name.is_empty() {
            entry.name = if article.journal.trim().is_empty() {
                "(no journal recorded)".to_string()
            } else {
                article.journal.trim().to_string()
            };
        }
        for issn in &article.issns {
            if !entry.issns.contains(issn) {
                entry.issns.push(issn.clone());
            }
        }
        entry.records += 1;
        if let Some(year) = article.publication_year {
            entry.year_sum += year as i64;
            entry.year_count += 1;
        }
        let q = article.sjr_quartile.trim();
        if !q.is_empty() {
            entry.ranks.push(q.to_string());
        }
        let s = article.sinta_rank.trim();
        if !s.is_empty() {
            entry.ranks.push(s.to_string());
        }
        if article.pdf_status == "available" {
            entry.pdf_available += 1;
        }
    }

    let mut out = String::new();
    out.push_str("Journal,Records,ISSNs,Best rank,Mean year,Public PDF available\r\n");
    for agg in map.values() {
        let best = agg.ranks.iter().min().cloned().unwrap_or_default();
        let mean_year = if agg.year_count > 0 {
            (agg.year_sum as f64 / agg.year_count as f64).round() as i64
        } else {
            0
        };
        out.push_str(&format!(
            "{},{},{},{},{},{}\r\n",
            csv_escape(&agg.name),
            agg.records,
            csv_escape(&agg.issns.join("; ")),
            csv_escape(&best),
            if mean_year == 0 {
                String::new()
            } else {
                mean_year.to_string()
            },
            agg.pdf_available
        ));
    }
    std::fs::write(filename, out).map_err(|e| e.to_string())
}

#[derive(Default)]
struct JournalAgg {
    name: String,
    issns: Vec<String>,
    records: usize,
    ranks: Vec<String>,
    year_sum: i64,
    year_count: usize,
    pdf_available: usize,
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn cell_text(cell: &crate::models::ExportCell) -> String {
    match cell {
        crate::models::ExportCell::Str(s) => crate::exporters::sanitize_cell(s),
        crate::models::ExportCell::Double(v) => v.to_string(),
        crate::models::ExportCell::Int(v) => v.to_string(),
    }
}

/// Plain-text record block, used by the details-text export.
///
/// Deliberately core-side and minimal: the UI's richer dialog text lives in
/// `gss-app`, which core cannot reach.
fn article_detail_block(article: &Article) -> String {
    let mut out = String::new();
    out.push_str(&format!("Title: {}\n", article.title));
    out.push_str(&format!("Authors: {}\n", article.authors));
    out.push_str(&format!("Link: {}\n", article.link));
    out.push_str(&format!("DOI: {}\n", article.doi));
    out.push_str(&format!("Journal: {}\n", article.journal));
    out.push_str(&format!("ISSNs: {}\n", article.issns.join(", ")));
    if let Some(year) = article.publication_year {
        out.push_str(&format!("Year: {year}\n"));
    }
    if !article.volume.is_empty() || !article.issue.is_empty() {
        out.push_str(&format!(
            "Volume/issue: {}({})\n",
            article.volume, article.issue
        ));
    }
    if !article.pages.is_empty() {
        out.push_str(&format!("Pages: {}\n", article.pages));
    }
    if !article.publisher.is_empty() {
        out.push_str(&format!("Publisher: {}\n", article.publisher));
    }
    if !article.sjr_quartile.is_empty() {
        out.push_str(&format!("SJR quartile: {}\n", article.sjr_quartile));
    }
    if !article.sinta_rank.is_empty() {
        out.push_str(&format!("SINTA rank: {}\n", article.sinta_rank));
    }
    if let Some(cited) = article.cited_by {
        out.push_str(&format!("Cited by: {cited}\n"));
    }
    if !article.oa_status.is_empty() {
        out.push_str(&format!("Open access: {}\n", article.oa_status));
    }
    if !article.retraction.is_empty() {
        out.push_str(&format!("Notice: {}\n", article.retraction));
    }
    out.push_str(&format!("PDF status: {}\n", article.pdf_status));
    out.push_str(&format!("PDF local path: {}\n", article.pdf_local_path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_article(doi: &str, title: &str) -> Article {
        Article {
            title: title.to_string(),
            authors: "Ada Lovelace; Alan Turing".to_string(),
            doi: doi.to_string(),
            journal: "Journal of Tests".to_string(),
            publication_year: Some(2024),
            sjr_quartile: "Q1".to_string(),
            pdf_status: "available".to_string(),
            ..Article::default()
        }
    }

    fn sample_record() -> CitationRecord {
        CitationRecord {
            title: "A study of things".to_string(),
            authors: vec!["Ada Lovelace".to_string(), "Alan Turing".to_string()],
            year: Some(2024),
            container: "Journal of Tests".to_string(),
            volume: "12".to_string(),
            issue: "3".to_string(),
            pages: "45-67".to_string(),
            doi: "10.1/x".to_string(),
            entry_type: "journal-article".to_string(),
            verified: true,
            ..CitationRecord::default()
        }
    }

    #[test]
    fn csl_json_is_valid_and_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        save_to_csl_json(&[sample_record()], &path).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let first = &parsed[0];
        assert_eq!(first["type"], "article-journal");
        assert_eq!(first["DOI"], "10.1/x");
        assert_eq!(first["issued"]["date-parts"][0][0], 2024);
        assert_eq!(first["author"][0]["family"], "Lovelace");
        assert_eq!(first["author"][0]["given"], "Ada");
    }

    #[test]
    fn markdown_escapes_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.md");
        let mut article = sample_article("10.1/x", "A | B");
        article.authors = "X | Y".to_string();
        save_to_markdown(&[article], &path, &["Title".to_string()]).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("A \\| B"), "{body}");
        assert!(body.lines().count() >= 3);
    }

    #[test]
    fn tsv_has_a_header_and_one_row_per_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.tsv");
        save_to_tsv(
            &[sample_article("10.1/a", "A"), sample_article("10.1/b", "B")],
            &path,
            &["Title".to_string(), "DOI".to_string()],
        )
        .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.trim_end().split("\r\n").collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Title\tDOI");
        assert!(lines[1].starts_with("A\t"));
    }

    #[test]
    fn journal_aggregation_counts_per_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.csv");
        let mut second = sample_article("10.1/y", "Another");
        second.pdf_status = "unknown".to_string();
        save_journal_aggregation(&[sample_article("10.1/x", "One"), second], &path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 2); // header + one journal
        assert!(body.contains("Journal of Tests,2,"), "{body}");
        assert!(body.contains(",Q1,"), "{body}");
    }

    #[test]
    fn stable_keys_reuse_the_persisted_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.bib");
        let mut keys: BTreeMap<String, String> = BTreeMap::new();
        save_to_bibtex_stable(&[sample_record()], &path, &mut keys).unwrap();
        let first = keys.get("10.1/x").cloned().expect("key persisted");
        let first_body = std::fs::read_to_string(&path).unwrap();

        // Exporting again with the same map must reproduce the same key, so an
        // existing \cite{} keeps resolving.
        save_to_bibtex_stable(&[sample_record()], &path, &mut keys).unwrap();
        let second_body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first_body, second_body);
        assert!(second_body.contains(&format!("@article{{{first},")));
    }

    #[test]
    fn citation_styles_differ_and_mention_the_year() {
        let record = sample_record();
        let apa = format_citation(&record, CitationStyle::Apa);
        let ieee = format_citation(&record, CitationStyle::Ieee);
        let vancouver = format_citation(&record, CitationStyle::Vancouver);
        assert!(apa.contains("(2024)"), "{apa}");
        assert!(apa.contains("Lovelace"), "{apa}");
        assert!(ieee.contains("vol. 12"), "{ieee}");
        assert!(vancouver.contains("2024;12(3):45-67"), "{vancouver}");
        assert_ne!(apa, ieee);
        assert_ne!(ieee, vancouver);
    }

    #[test]
    fn unresolved_records_never_gain_structured_authors() {
        // An unresolved record's `authors` is the raw Scholar citation line
        // ("A Author, B Author - Journal, 2024 - example.com"). Splitting it
        // would fabricate structured names, which SCHOLAR_AUTHORS_WARNING
        // forbids. Both CSL-JSON and ENW must keep it out of the author field.
        let mut record = sample_record();
        record.verified = false;
        record.authors = vec!["A Author - Journal, 2024 - example.com".to_string()];

        let dir = tempfile::tempdir().unwrap();
        let csl_path = dir.path().join("u.json");
        save_to_csl_json(std::slice::from_ref(&record), &csl_path).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&csl_path).unwrap()).unwrap();
        assert!(
            parsed[0].get("author").is_none(),
            "unverified record must not carry structured authors: {}",
            parsed[0]
        );

        // ENW: no %A line unless the names came from Crossref.
        let enw_path = dir.path().join("u.enw");
        let article = Article {
            title: "T".to_string(),
            authors: "A Author - Journal, 2024 - example.com".to_string(),
            ..Article::default()
        };
        save_to_enw(&[article], &enw_path, &[]).unwrap();
        let enw = std::fs::read_to_string(&enw_path).unwrap();
        assert!(!enw.contains("%A"), "unexpected structured author: {enw}");
        assert!(enw.contains("%O A Author"), "raw line belongs in %O: {enw}");
    }

    #[test]
    fn verified_records_do_get_structured_authors() {
        let dir = tempfile::tempdir().unwrap();
        let csl_path = dir.path().join("v.json");
        save_to_csl_json(&[sample_record()], &csl_path).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&csl_path).unwrap()).unwrap();
        assert_eq!(parsed[0]["author"][0]["family"], "Lovelace");
    }

    #[test]
    fn reading_list_html_is_self_contained() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.html");
        let mut article = sample_article("10.1/x", "A <dangerous> title");
        article.retraction = "retraction".to_string();
        save_reading_list_html(&[article], &path, "My list").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("<!DOCTYPE html>"));
        assert!(body.contains("RETRACTION"));
        // The angle brackets must be escaped, not injected as markup.
        assert!(body.contains("&lt;dangerous&gt;"));
        assert!(!body.contains("<dangerous>"));
        // No external stylesheet or script references.
        assert!(!body.contains("<script"));
        assert!(!body.contains("rel=\"stylesheet\""));
    }

    #[test]
    fn grouped_bibliography_splits_by_year() {
        let mut older = sample_record();
        older.year = Some(2019);
        older.title = "Old".to_string();
        let body = save_grouped_bibliography_to_string(&[sample_record(), older.clone()], &[]);
        assert!(body.contains("## 2024"));
        assert!(body.contains("## 2019"));
    }

    fn save_grouped_bibliography_to_string(
        records: &[CitationRecord],
        articles: &[Article],
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.md");
        save_grouped_bibliography(records, articles, &path, "year", CitationStyle::Apa).unwrap();
        std::fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn excel_rich_writes_a_workbook_and_rejects_no_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.xlsx");
        save_to_excel_rich(&[sample_article("10.1/x", "One")], &path, &[], Some("pdf")).unwrap();
        assert!(path.is_file());
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"PK"); // xlsx is a zip

        // An explicit column list that matches nothing is an error, not an
        // empty workbook.
        let bad = dir.path().join("bad.xlsx");
        assert!(save_to_excel_rich(&[], &bad, &["Nonexistent".to_string()], None).is_err());
    }
}
