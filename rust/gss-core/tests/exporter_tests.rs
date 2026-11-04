//! Ports of `test_exporters.py` (ExporterTests + BibTeX/RIS writers).

use gss_core::crossref::{SCHOLAR_AUTHORS_WARNING, UNRESOLVED_WARNING};
use gss_core::exporters::{save_to_bibtex, save_to_csv, save_to_excel, save_to_ris};
use gss_core::models::{Article, CitationRecord, EXPORT_COLUMNS};

fn tmp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn export_receives_normalized_article_rows() {
    let article = Article {
        title: "Deep Learning".into(),
        authors: "A Author".into(),
        link: "https://example.edu/paper".into(),
        relevance_score: Some(87.5),
        ..Article::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("articles.xlsx");

    save_to_excel(&[article], &path).unwrap();

    let mut workbook: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
    use calamine::{DataType as _, Reader as _};
    let range = workbook.worksheet_range("Scholar Articles").unwrap();
    let rows: Vec<Vec<calamine::Data>> = range.rows().map(|r| r.to_vec()).collect();
    let header: Vec<String> = rows[0]
        .iter()
        .map(|c| c.get_string().unwrap_or_default().to_string())
        .collect();
    assert_eq!(header, EXPORT_COLUMNS.to_vec());
    assert_eq!(rows[1][0].get_string().unwrap(), "Deep Learning");
    assert_eq!(rows[1][1].get_string().unwrap(), "A Author");
    assert_eq!(
        rows[1][2].get_string().unwrap(),
        "https://example.edu/paper"
    );
    assert_eq!(rows[1][3].get_float().unwrap(), 87.5);
}

#[test]
fn export_uses_blank_relevance_score_when_unranked() {
    let article = Article {
        title: "Deep Learning".into(),
        authors: "A Author".into(),
        link: "https://example.edu/paper".into(),
        ..Article::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("articles.xlsx");

    save_to_excel(&[article], &path).unwrap();

    let mut workbook: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
    use calamine::{DataType as _, Reader as _};
    let range = workbook.worksheet_range("Scholar Articles").unwrap();
    let rows: Vec<Vec<calamine::Data>> = range.rows().map(|r| r.to_vec()).collect();
    let header: Vec<String> = rows[0]
        .iter()
        .map(|c| c.get_string().unwrap_or_default().to_string())
        .collect();
    assert_eq!(header, EXPORT_COLUMNS.to_vec());
    // Unranked score exports as an empty CELL, not a number.
    assert!(rows[1][3].is_empty());
}

#[test]
fn csv_export_preserves_unicode_and_escapes_structured_text() {
    let articles = vec![
        Article {
            title: "یادگیری عمیق, \"پزشکی\"\nنسخه دوم".into(),
            authors: "Author, \"A\"".into(),
            link: "https://example.edu/paper?x=1,y=2".into(),
            relevance_score: Some(91.2),
            ..Article::default()
        },
        Article::new("Unranked"),
    ];
    let dir = tmp_dir();
    let path = dir.path().join("articles.csv");

    save_to_csv(&articles, &path).unwrap();

    let raw = std::fs::read(&path).unwrap();
    assert!(raw.starts_with(b"\xef\xbb\xbf"));
    let text = String::from_utf8_lossy(&raw[3..]);
    let mut lines = text.split("\r\n");
    let header_line = lines.next().unwrap();
    let first = lines.next().unwrap();
    let second = lines.next().unwrap();
    let header: Vec<String> = parse_csv_line(header_line);
    assert_eq!(header, EXPORT_COLUMNS.to_vec());
    let row: Vec<String> = parse_csv_line(first);
    let map: std::collections::HashMap<String, String> =
        header.clone().into_iter().zip(row.into_iter()).collect();
    assert_eq!(map["Title"], "یادگیری عمیق, \"پزشکی\"\nنسخه دوم");
    assert_eq!(map["Authors"], "Author, \"A\"");
    assert_eq!(map["Link"], "https://example.edu/paper?x=1,y=2");
    assert_eq!(map["Relevance Score"], "91.2");
    let row2: Vec<String> = parse_csv_line(second);
    let map2: std::collections::HashMap<String, String> =
        header.into_iter().zip(row2.into_iter()).collect();
    assert_eq!(map2["Relevance Score"], "");
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            ',' => {
                fields.push(std::mem::take(&mut field));
            }
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

#[test]
fn new_metadata_columns_are_exported() {
    let article = Article {
        title: "Deep Learning".into(),
        authors: "A Author".into(),
        link: "https://example.edu/paper".into(),
        relevance_score: Some(87.5),
        doi: "10.1038/nature14539".into(),
        journal: "Nature".into(),
        issns: vec!["0028-0836".into(), "1476-4687".into()],
        publication_year: Some(2015),
        sjr_quartile: "Q1".into(),
        sjr_year: Some(2023),
        sjr_category: "Multidisciplinary".into(),
        sjr_source_url: "https://scimagojr.example/journal".into(),
        sinta_rank: "S2".into(),
        sinta_year: Some(2024),
        sinta_source_url: "https://sinta.example/journals/1".into(),
        pdf_url: "https://repo.example/a.pdf".into(),
        pdf_source_url: "https://api.crossref.org/works/10.1038/nature14539".into(),
        pdf_status: "available".into(),
        pdf_detail: "verified".into(),
        pdf_local_path: "C:/pdfs/a.pdf".into(),
    };
    let dir = tmp_dir();
    let path = dir.path().join("articles.csv");

    save_to_csv(&[article], &path).unwrap();

    let raw = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&raw[3..]);
    let mut lines = text.split("\r\n");
    let header_line = lines.next().unwrap();
    let first = lines.next().unwrap();
    let header: Vec<String> = parse_csv_line(header_line);
    let row: Vec<String> = parse_csv_line(first);
    let map: std::collections::HashMap<String, String> =
        header.iter().cloned().zip(row.iter().cloned()).collect();
    for column in [
        "DOI",
        "Journal",
        "ISSNs",
        "Publication Year",
        "SJR Quartile",
        "SJR Year",
        "SJR Category",
        "SJR Source",
        "SINTA Rank",
        "SINTA Year",
        "SINTA Source",
        "PDF Status",
        "PDF URL",
        "PDF Source",
        "PDF Detail",
        "PDF Local Path",
    ] {
        assert!(EXPORT_COLUMNS.contains(&column), "{column}");
    }
    assert_eq!(map["DOI"], "10.1038/nature14539");
    assert_eq!(map["Journal"], "Nature");
    assert_eq!(map["ISSNs"], "0028-0836; 1476-4687");
    assert_eq!(map["Publication Year"], "2015");
    assert_eq!(map["SJR Quartile"], "Q1");
    assert_eq!(map["SINTA Rank"], "S2");
    assert_eq!(map["PDF Status"], "available");
}

#[test]
fn unknown_metadata_exports_blank_rather_than_a_guess() {
    let article = Article {
        title: "Deep Learning".into(),
        authors: "A Author".into(),
        link: "https://example.edu/paper".into(),
        ..Article::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("articles.csv");
    save_to_csv(&[article], &path).unwrap();

    let raw = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&raw[3..]);
    let mut lines = text.split("\r\n");
    let header_line = lines.next().unwrap();
    let first = lines.next().unwrap();
    let header: Vec<String> = parse_csv_line(header_line);
    let row: Vec<String> = parse_csv_line(first);
    let map: std::collections::HashMap<String, String> =
        header.into_iter().zip(row.into_iter()).collect();
    assert_eq!(map["Journal"], "");
    assert_eq!(map["SJR Quartile"], "");
    assert_eq!(map["SINTA Rank"], "");
    assert_eq!(map["Publication Year"], "");
    assert_eq!(map["PDF Status"], "unknown");
}

#[test]
fn formula_injection_is_defused_without_altering_scientific_text() {
    let articles = vec![
        Article {
            title: "=cmd|'/c calc'!A0".into(),
            authors: "@SUM(1+1)".into(),
            link: "".into(),
            ..Article::default()
        },
        Article {
            title: "-A hyphenated title is real text".into(),
            authors: "A Author".into(),
            link: "".into(),
            ..Article::default()
        },
        Article {
            title: "+3 trials".into(),
            authors: "A Author".into(),
            link: "".into(),
            ..Article::default()
        },
    ];
    let dir = tmp_dir();
    let path = dir.path().join("articles.csv");
    save_to_csv(&articles, &path).unwrap();

    let raw = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&raw[3..]);
    let mut lines = text.split("\r\n");
    let header_line = lines.next().unwrap();
    let header: Vec<String> = parse_csv_line(header_line);
    for (index, expected_title) in [
        "'=cmd|'/c calc'!A0",
        "'-A hyphenated title is real text",
        "'+3 trials",
    ]
    .into_iter()
    .enumerate()
    {
        let line = lines.next().unwrap();
        let row: Vec<String> = parse_csv_line(line);
        let map: std::collections::HashMap<String, String> =
            header.clone().into_iter().zip(row.into_iter()).collect();
        assert_eq!(map["Title"], expected_title, "row {index}");
        if index == 0 {
            assert_eq!(map["Authors"], "'@SUM(1+1)");
        }
    }
}

#[test]
fn excel_export_defuses_formula_injection() {
    let article = Article {
        title: "=1+1".into(),
        authors: "A Author".into(),
        link: "".into(),
        ..Article::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("articles.xlsx");
    save_to_excel(&[article], &path).unwrap();

    let mut workbook: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
    use calamine::{DataType as _, Reader as _};
    let range = workbook.worksheet_range("Scholar Articles").unwrap();
    let rows: Vec<Vec<calamine::Data>> = range.rows().map(|r| r.to_vec()).collect();
    assert_eq!(rows[1][0].get_string().unwrap(), "'=1+1");
}

// ---- BibTeXWriterTests ----

fn verified_record() -> CitationRecord {
    CitationRecord {
        title: "Deep learning".into(),
        authors: vec![
            "Yann LeCun".into(),
            "Yoshua Bengio".into(),
            "Geoffrey Hinton".into(),
        ],
        year: Some(2015),
        container: "Nature".into(),
        entry_type: "journal-article".into(),
        doi: "10.1038/nature14539".into(),
        issns: vec!["0028-0836".into(), "1476-4687".into()],
        volume: "521".into(),
        issue: "7553".into(),
        pages: "436-444".into(),
        publisher: "Springer".into(),
        url: "https://doi.org/10.1038/nature14539".into(),
        source: "crossref".into(),
        verified: true,
        ..CitationRecord::default()
    }
}

#[test]
fn verified_record_renders_full_entry() {
    let dir = tmp_dir();
    let path = dir.path().join("citations.bib");
    save_to_bibtex(&[verified_record()], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(text.contains("@article{lecun_deep_2015,"), "{text}");
    assert!(text.contains("author = {Yann LeCun and Yoshua Bengio and Geoffrey Hinton},"));
    assert!(text.contains("title = {{Deep learning}},"));
    assert!(text.contains("journal = {Nature},"));
    assert!(text.contains("year = {2015},"));
    assert!(text.contains("volume = {521},"));
    assert!(text.contains("number = {7553},"));
    assert!(text.contains("pages = {436-444},"));
    assert!(text.contains("doi = {10.1038/nature14539},"));
    assert!(text.contains("issn = {0028-0836, 1476-4687},"));
    assert!(!text.contains("note ="));
    assert!(!text.contains("booktitle"));
}

#[test]
fn specials_escaped_and_title_double_braced() {
    let record = CitationRecord {
        title: "Escaping {Braces} & 100% of _Cases_".into(),
        container: "Proceedings & Notes".into(),
        entry_type: "proceedings-article".into(),
        year: Some(2023),
        ..CitationRecord::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("citations.bib");
    save_to_bibtex(&[record], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(
        text.contains("@inproceedings{anon_escaping_2023,"),
        "{text}"
    );
    assert!(
        text.contains("title = {{Escaping \\{Braces\\} \\& 100\\% of \\_Cases\\_}},"),
        "{text}"
    );
    assert!(text.contains("booktitle = {Proceedings \\& Notes},"));
    assert!(!text.contains("author ="));
}

#[test]
fn duplicate_titles_get_unique_keys() {
    let records: Vec<CitationRecord> = (0..3)
        .map(|_| CitationRecord {
            title: "Learning Deep Things".into(),
            year: Some(2020),
            ..CitationRecord::default()
        })
        .collect();
    let dir = tmp_dir();
    let path = dir.path().join("citations.bib");
    save_to_bibtex(&records, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(text.contains("@misc{anon_learning_2020,"));
    assert!(text.contains("@misc{anon_learning_2020-2,"));
    assert!(text.contains("@misc{anon_learning_2020-3,"));
}

#[test]
fn unresolved_record_has_no_author_and_carries_the_review_note() {
    let record = CitationRecord {
        title: "An Unresolved Paper".into(),
        year: Some(2024),
        source: "scholar".into(),
        fallback_authors_line: "A Author, B Author - Journal of Examples, 2024 - publisher.example"
            .into(),
        warnings: vec![
            UNRESOLVED_WARNING.to_string(),
            SCHOLAR_AUTHORS_WARNING.to_string(),
        ],
        ..CitationRecord::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("citations.bib");
    save_to_bibtex(&[record], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(!text.contains("author ="));
    assert!(
        text.contains(
            "note = {A Author, B Author - Journal of Examples, 2024 - publisher.example; "
        ),
        "{text}"
    );
    assert!(text.contains(UNRESOLVED_WARNING));
    assert!(text.contains(SCHOLAR_AUTHORS_WARNING));
}

#[test]
fn values_are_single_lined() {
    let record = CitationRecord {
        title: "Broken\nTitle\r\nHere".into(),
        ..CitationRecord::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("citations.bib");
    save_to_bibtex(&[record], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(text.contains("title = {{Broken Title Here}},"));
}

// ---- RISWriterTests ----

#[test]
fn verified_journal_record_frames_and_fields() {
    let record = CitationRecord {
        title: "Deep learning".into(),
        authors: vec!["Yann LeCun".into(), "Yoshua Bengio".into()],
        year: Some(2015),
        container: "Nature".into(),
        entry_type: "journal-article".into(),
        doi: "10.1038/nature14539".into(),
        issns: vec!["0028-0836".into()],
        pages: "436-444".into(),
        publisher: "Springer".into(),
        url: "https://doi.org/10.1038/nature14539".into(),
        source: "crossref".into(),
        verified: true,
        ..CitationRecord::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("citations.ris");
    save_to_ris(&[record], &path).unwrap();
    let raw = std::fs::read(&path).unwrap();

    assert!(!raw.starts_with(b"\xef\xbb\xbf"));
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("TY  - JOUR"));
    assert!(text.contains("AU  - LeCun, Yann"));
    assert!(text.contains("AU  - Bengio, Yoshua"));
    assert!(text.contains("TI  - Deep learning"));
    assert!(text.contains("T2  - Nature"));
    assert!(text.contains("PY  - 2015"));
    assert!(text.contains("SP  - 436"));
    assert!(text.contains("EP  - 444"));
    assert!(text.contains("DO  - 10.1038/nature14539"));
    assert!(text.contains("UR  - https://doi.org/10.1038/nature14539"));
    assert!(text.contains("SN  - 0028-0836"));
    assert!(text.contains("PB  - Springer"));
    assert!(text.trim_end_matches(['\r', '\n']).ends_with("ER  - "));
    assert!(text.contains("\r\n"));
}

#[test]
fn proceedings_maps_to_conf_and_open_page_range_stays_in_sp() {
    let record = CitationRecord {
        title: "A Conference Paper".into(),
        container: "Conference on Examples".into(),
        entry_type: "proceedings-article".into(),
        year: Some(2023),
        pages: "1007".into(),
        ..CitationRecord::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("citations.ris");
    save_to_ris(&[record], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(text.contains("TY  - CONF"));
    assert!(text.contains("SP  - 1007"));
    assert!(!text.contains("EP  - "));
}

#[test]
fn unresolved_record_has_no_au_and_carries_the_note() {
    let record = CitationRecord {
        title: "An Unresolved Paper".into(),
        source: "scholar".into(),
        fallback_authors_line: "A Author, B Author - Journal of Examples, 2024 - publisher.example"
            .into(),
        warnings: vec![
            UNRESOLVED_WARNING.to_string(),
            SCHOLAR_AUTHORS_WARNING.to_string(),
        ],
        ..CitationRecord::default()
    };
    let dir = tmp_dir();
    let path = dir.path().join("citations.ris");
    save_to_ris(&[record], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();

    assert!(text.contains("TY  - GEN"));
    assert!(!text.contains("AU  - "));
    assert!(
        text.contains(
            "N1  - A Author, B Author - Journal of Examples, 2024 - publisher.example | "
        ),
        "{text}"
    );
    assert!(text.contains(UNRESOLVED_WARNING));
    assert!(text.contains(SCHOLAR_AUTHORS_WARNING));
}
