//! Port of `test_journal_catalog.py` (Import + Matching tests) 1:1.

use gss_core::catalog::{apply_rank, catalog_matches, journal_title_key, JournalCatalog};
use gss_core::models::Article;

const SCIMAGO_SEMICOLON: &str = concat!(
    "Title;Issn;SJR Best Quartile;Year;Categories\n",
    "Journal of Examples;1234-5679;Q1;2023;Multidisciplinary\n",
    "Another Journal;2049-3630;Q3;2023;Education\n",
    "Journal of Examples;1234-5679;Q2;2021;Multidisciplinary\n",
);

const SINTA_TEMPLATE: &str = concat!(
    "journal,issn,sinta_rank,year,source_url\n",
    "Jurnal Contoh,1234-5679,S2,2024,https://sinta.example/journals/1\n",
);

fn write(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn semicolon_scimago_export_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "scimago.csv", SCIMAGO_SEMICOLON);
    let mut catalog = JournalCatalog::new();

    let imported = catalog.import_file(&path, None).unwrap();

    assert_eq!(imported, 3);
    assert_eq!(catalog.len(), 3);
    assert_eq!(catalog.records()[0].title, "Journal of Examples");
    assert_eq!(catalog.records()[0].issns, vec!["1234-5679".to_string()]);
    assert_eq!(catalog.records()[0].category, "Multidisciplinary");
}

#[test]
fn sinta_template_columns_are_recognized() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "sinta.csv", SINTA_TEMPLATE);
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();

    let rank = catalog.match_one(["1234-5679"], "").unwrap();
    assert_eq!(rank.kind, "sinta");
    assert_eq!(rank.rank, "S2");
    assert_eq!(rank.year, Some(2024));
    assert_eq!(rank.source_url, "https://sinta.example/journals/1");
}

#[test]
fn numeric_rank_column_never_becomes_a_quartile() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "scimago-real.csv",
        "Rank;Title;Issn;SJR Best Quartile;Year;Categories\n1;Journal of Examples;1234-5679;Q1;2023;Multidisciplinary\n2;Another Journal;2049-3630;Q3;2023;Education\n",
    );
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();

    assert_eq!(
        catalog.match_one([], "Journal of Examples").unwrap().rank,
        "Q1"
    );
    assert_eq!(catalog.match_one([], "Another Journal").unwrap().rank, "Q3");
}

#[test]
fn numeric_only_rank_column_yields_no_quartile() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "numeric-only.csv",
        "Rank;Title;Issn;Year\n1;Journal of Examples;1234-5679;2023\n",
    );
    let mut catalog = JournalCatalog::new();

    let result = catalog.import_file(&path, None);

    assert!(result.is_err());
    assert_eq!(catalog.len(), 0);
}

#[test]
fn successive_imports_accumulate() {
    let dir = tempfile::tempdir().unwrap();
    let sjr = write(
        dir.path(),
        "sjr.csv",
        "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\n",
    );
    let sinta = write(dir.path(), "sinta.csv", SINTA_TEMPLATE);
    let older = write(
        dir.path(),
        "sjr-2019.csv",
        "Title,Issn,SJR Best Quartile,Year\nAnother Journal,2049-3630,Q4,2019\n",
    );
    let mut catalog = JournalCatalog::new();

    catalog.import_file(&sjr, None).unwrap();
    catalog.import_file(&sinta, None).unwrap();
    catalog.import_file(&older, None).unwrap();

    assert_eq!(catalog.len(), 3);
    assert_eq!(catalog.match_one(["1234-5679"], "").unwrap().rank, "S2");
    assert_eq!(
        catalog.match_one([], "Journal of Examples").unwrap().rank,
        "Q1"
    );
    assert_eq!(catalog.match_one([], "Another Journal").unwrap().rank, "Q4");
}

#[test]
fn reimporting_a_newer_year_supersedes_the_older_record() {
    let dir = tempfile::tempdir().unwrap();
    let older = write(
        dir.path(),
        "sjr-2019.csv",
        "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q4,2019\n",
    );
    let newer = write(
        dir.path(),
        "sjr-2023.csv",
        "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\n",
    );
    let mut catalog = JournalCatalog::new();

    catalog.import_file(&older, None).unwrap();
    catalog.import_file(&newer, None).unwrap();

    let rank = catalog.match_one([], "Journal of Examples").unwrap();
    assert_eq!(rank.rank, "Q1");
    assert_eq!(rank.year, Some(2023));
}

#[test]
fn xlsx_import_reads_the_same_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ranks.xlsx");
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.write_string(0, 0, "Title").unwrap();
    sheet.write_string(0, 1, "Issn").unwrap();
    sheet.write_string(0, 2, "SJR Best Quartile").unwrap();
    sheet.write_string(0, 3, "Year").unwrap();
    sheet.write_string(0, 4, "Categories").unwrap();
    sheet.write_string(1, 0, "Journal of Examples").unwrap();
    sheet.write_string(1, 1, "1234-5679").unwrap();
    sheet.write_string(1, 2, "Q1").unwrap();
    sheet.write_number(1, 3, 2023.0).unwrap();
    sheet.write_string(1, 4, "Multidisciplinary").unwrap();
    workbook.save(&path).unwrap();

    let mut catalog = JournalCatalog::new();
    let imported = catalog.import_file(&path, None).unwrap();

    assert_eq!(imported, 1);
    assert_eq!(
        catalog.match_one([], "Journal of Examples").unwrap().rank,
        "Q1"
    );
}

#[test]
fn import_is_transactional_and_leaves_prior_records_intact() {
    let dir = tempfile::tempdir().unwrap();
    let good = write(dir.path(), "good.csv", SCIMAGO_SEMICOLON);
    let broken = write(
        dir.path(),
        "broken.csv",
        "Title,Issn,Note\nNo rank column,1234-5679,-\n",
    );
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&good, None).unwrap();

    let result = catalog.import_file(&broken, None);
    assert!(result.is_err());

    assert_eq!(catalog.len(), 3);
    assert_eq!(
        catalog.match_one([], "Journal of Examples").unwrap().rank,
        "Q1"
    );
}

#[test]
fn unrecognized_headers_raise() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "unrelated.csv", "alpha,beta\n1,2\n");
    let mut catalog = JournalCatalog::new();

    let err = catalog.import_file(&path, None).unwrap_err();
    assert!(
        err.contains("no recognizable journal title or ISSN column"),
        "{err}"
    );
}

#[test]
fn unsupported_extension_raises() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "ranks.json", "{}");
    let mut catalog = JournalCatalog::new();

    let err = catalog.import_file(&path, None).unwrap_err();
    assert!(err.contains("Unsupported ranking file type"), "{err}");
}

#[test]
fn rows_without_a_valid_rank_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "mixed.csv",
        "Title,Issn,SJR Best Quartile,Year\nGood Journal,1234-5679,Q2,2023\nUnranked Journal,2049-3630,Not ranked,2023\nBlank Journal,0317-8471,,2023\n",
    );
    let mut catalog = JournalCatalog::new();

    let imported = catalog.import_file(&path, None).unwrap();

    assert_eq!(imported, 1);
    assert_eq!(catalog.match_one([], "Good Journal").unwrap().rank, "Q2");
    assert!(catalog.match_one([], "Unranked Journal").is_none());
}

#[test]
fn invalid_issn_is_not_stored_and_cannot_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "bad-issn.csv",
        "Title,Issn,SJR Best Quartile,Year\nBad Issn Journal,1234-5678,Q1,2023\nGood Issn Journal,1234-5679,Q1,2023\n",
    );
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();

    let records: std::collections::HashMap<&str, &gss_core::catalog::JournalRankRecord> = catalog
        .records()
        .iter()
        .map(|r| (r.title.as_str(), r))
        .collect();
    assert!(records["Bad Issn Journal"].issns.is_empty());
    assert!(catalog.match_one(["1234-5678"], "").is_none());
    assert!(catalog.match_one(["1234-5679"], "").is_some());
}

#[test]
fn absent_year_stays_unknown_and_is_never_invented() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "no-year.csv",
        "Title,Issn,SJR Best Quartile\nJournal of Examples,1234-5679,Q1\n",
    );
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();

    let rank = catalog.match_one([], "Journal of Examples").unwrap();
    assert_eq!(rank.year, None);
}

#[test]
fn explicit_default_year_applies_only_when_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "no-year.csv",
        "Title,Issn,SJR Best Quartile\nJournal of Examples,1234-5679,Q1\n",
    );

    let mut without = JournalCatalog::new();
    without.import_file(&path, None).unwrap();
    let mut with_default = JournalCatalog::new();
    with_default.import_file(&path, Some(2024)).unwrap();

    assert_eq!(
        without.match_one([], "Journal of Examples").unwrap().year,
        None
    );
    assert_eq!(
        with_default
            .match_one([], "Journal of Examples")
            .unwrap()
            .year,
        Some(2024)
    );
}

#[test]
fn latest_actual_year_wins_and_undated_never_beats_dated() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "years.csv",
        "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q4,2020\nJournal of Examples,1234-5679,Q1,2023\n",
    );
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();

    let rank = catalog.match_one([], "Journal of Examples").unwrap();
    assert_eq!(rank.rank, "Q1");
    assert_eq!(rank.year, Some(2023));
}

#[test]
fn contradictory_ranks_for_the_same_latest_year_are_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "ambiguous.csv",
        "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\nJournal of Examples,1234-5679,Q3,2023\n",
    );
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();

    assert!(catalog.match_one([], "Journal of Examples").is_none());
}

// ---- MatchingTests ----

struct CatalogFixture {
    dir: tempfile::TempDir,
    catalog: JournalCatalog,
}

fn catalog_fixture() -> CatalogFixture {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "ranks.csv", SCIMAGO_SEMICOLON);
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&path, None).unwrap();
    CatalogFixture { dir, catalog }
}

#[test]
fn exact_issn_match_is_preferred_over_title() {
    let f = catalog_fixture();
    let rank = f
        .catalog
        .match_one(["1234-5679"], "A Completely Different Name")
        .unwrap();

    assert_eq!(rank.matched_by, "issn:1234-5679");
    assert_eq!(rank.rank, "Q1");
}

#[test]
fn title_fallback_requires_an_exact_normalized_match() {
    let f = catalog_fixture();
    assert_eq!(
        f.catalog
            .match_one([], "journal   of   examples")
            .unwrap()
            .matched_by,
        "title"
    );
    assert!(f.catalog.match_one([], "Journal of Example").is_none());
    assert!(f
        .catalog
        .match_one([], "Journal of Examples and More")
        .is_none());
}

#[test]
fn unmatched_journal_stays_unknown() {
    let f = catalog_fixture();
    assert!(f.catalog.match_one([], "Unlisted Journal").is_none());
    assert!(f.catalog.match_one([], "").is_none());
}

#[test]
fn title_key_is_conservative() {
    assert_eq!(
        journal_title_key("The Journal of Examples"),
        journal_title_key("Journal of Examples")
    );
    assert_ne!(
        journal_title_key("Journal of Examples"),
        journal_title_key("Journal of Example")
    );
}

#[test]
fn catalog_matches_returns_both_ranking_systems() {
    let dir = tempfile::tempdir().unwrap();
    let sjr = write(
        dir.path(),
        "sjr.csv",
        "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\n",
    );
    let sinta = write(dir.path(), "sinta.csv", SINTA_TEMPLATE);
    let mut catalog = JournalCatalog::new();
    catalog.import_file(&sjr, None).unwrap();
    catalog.import_file(&sinta, None).unwrap();

    let article = Article {
        title: "A Paper".into(),
        journal: "Journal of Examples".into(),
        issns: vec!["1234-5679".into()],
        ..Article::default()
    };
    let (sjr_rank, sinta_rank) = catalog_matches(&article, Some(&catalog));

    assert_eq!(sjr_rank.unwrap().rank, "Q1");
    assert_eq!(sinta_rank.unwrap().rank, "S2");
}

#[test]
fn conference_article_without_a_journal_identity_matches_nothing() {
    let f = catalog_fixture();
    let article = Article {
        title: "A Conference Paper".into(),
        authors: "C Author - Conference on Examples, 2023".into(),
        ..Article::default()
    };

    let (sjr, sinta) = catalog_matches(&article, Some(&f.catalog));
    assert!(sjr.is_none());
    assert!(sinta.is_none());
}

#[test]
fn apply_rank_fills_only_that_systems_fields() {
    let f = catalog_fixture();
    let article = Article {
        title: "A Paper".into(),
        journal: "Journal of Examples".into(),
        ..Article::default()
    };
    let rank = f.catalog.match_one([], "Journal of Examples");

    let updated = apply_rank(&article, rank.as_ref());

    assert_eq!(updated.sjr_quartile, "Q1");
    assert_eq!(updated.sjr_year, Some(2023));
    assert_eq!(updated.sjr_category, "Multidisciplinary");
    assert_eq!(updated.sinta_rank, "");
}

#[test]
fn apply_rank_with_no_match_returns_the_article_unchanged() {
    let article = Article::new("A Paper");
    let updated = apply_rank(&article, None);
    assert_eq!(updated, article);
}
