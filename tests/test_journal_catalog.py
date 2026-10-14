import tempfile
import unittest
from pathlib import Path

from openpyxl import Workbook

from google_scholar_scraper.journal_catalog import (
    JournalCatalog,
    _read_csv,
    _resolve_columns,
    apply_rank,
    catalog_matches,
    journal_title_key,
)
from google_scholar_scraper.models import Article


SCIMAGO_SEMICOLON = (
    "Title;Issn;SJR Best Quartile;Year;Categories\n"
    "Journal of Examples;1234-5679;Q1;2023;Multidisciplinary\n"
    "Another Journal;2049-3630;Q3;2023;Education\n"
    "Journal of Examples;1234-5679;Q2;2021;Multidisciplinary\n"
)

SINTA_TEMPLATE = (
    "journal,issn,sinta_rank,year,source_url\n"
    "Jurnal Contoh,1234-5679,S2,2024,https://sinta.example/journals/1\n"
)


def write(directory: str, name: str, text: str) -> Path:
    path = Path(directory) / name
    path.write_text(text, encoding="utf-8")
    return path


class ImportTests(unittest.TestCase):
    def test_semicolon_scimago_export_is_imported(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(temp_dir, "scimago.csv", SCIMAGO_SEMICOLON)
            catalog = JournalCatalog()

            imported = catalog.import_file(path)

        self.assertEqual(imported, 3)
        self.assertEqual(len(catalog), 3)
        self.assertEqual(catalog.records[0].title, "Journal of Examples")
        self.assertEqual(catalog.records[0].issns, ("1234-5679",))
        self.assertEqual(catalog.records[0].category, "Multidisciplinary")

    def test_sinta_template_columns_are_recognized(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(temp_dir, "sinta.csv", SINTA_TEMPLATE)
            catalog = JournalCatalog()

            catalog.import_file(path)

        rank = catalog.match(issns=["1234-5679"])
        self.assertEqual(rank.kind, "sinta")
        self.assertEqual(rank.rank, "S2")
        self.assertEqual(rank.year, 2024)
        self.assertEqual(rank.source_url, "https://sinta.example/journals/1")

    def test_numeric_rank_column_never_becomes_a_quartile(self) -> None:
        """A real SCImago export has a numeric `Rank` column next to `SJR Best Quartile`."""
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(
                temp_dir,
                "scimago-real.csv",
                "Rank;Title;Issn;SJR Best Quartile;Year;Categories\n"
                "1;Journal of Examples;1234-5679;Q1;2023;Multidisciplinary\n"
                "2;Another Journal;2049-3630;Q3;2023;Education\n",
            )
            catalog = JournalCatalog()

            catalog.import_file(path)

        self.assertEqual(catalog.match(journal="Journal of Examples").rank, "Q1")
        self.assertEqual(catalog.match(journal="Another Journal").rank, "Q3")

    def test_numeric_only_rank_column_yields_no_quartile(self) -> None:
        """Without `SJR Best Quartile` the numeric position must not be read as a rank."""
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(
                temp_dir,
                "numeric-only.csv",
                "Rank;Title;Issn;Year\n1;Journal of Examples;1234-5679;2023\n",
            )
            catalog = JournalCatalog()

            with self.assertRaises(ValueError):
                catalog.import_file(path)

        self.assertEqual(len(catalog), 0)

    def test_successive_imports_accumulate(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            sjr = write(temp_dir, "sjr.csv", "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\n")
            sinta = write(temp_dir, "sinta.csv", SINTA_TEMPLATE)
            older = write(temp_dir, "sjr-2019.csv", "Title,Issn,SJR Best Quartile,Year\nAnother Journal,2049-3630,Q4,2019\n")
            catalog = JournalCatalog()

            catalog.import_file(sjr)
            catalog.import_file(sinta)
            catalog.import_file(older)

        self.assertEqual(len(catalog), 3)
        self.assertEqual(catalog.match(issns=["1234-5679"]).rank, "S2")
        self.assertEqual(catalog.match(journal="Journal of Examples").rank, "Q1")
        self.assertEqual(catalog.match(journal="Another Journal").rank, "Q4")

    def test_reimporting_a_newer_year_supersedes_the_older_record(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            older = write(temp_dir, "sjr-2019.csv", "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q4,2019\n")
            newer = write(temp_dir, "sjr-2023.csv", "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\n")
            catalog = JournalCatalog()

            catalog.import_file(older)
            catalog.import_file(newer)

        rank = catalog.match(journal="Journal of Examples")
        self.assertEqual(rank.rank, "Q1")
        self.assertEqual(rank.year, 2023)

    def test_shipped_templates_exist_and_resolve_their_headers(self) -> None:
        """Release builds copy these exact paths, so a rename must fail here first."""
        templates = Path(__file__).resolve().parent.parent / "docs" / "ranking_templates"
        for name, expected_fields in (
            ("scimago.csv", {"title", "issn", "rank", "year", "category"}),
            ("sinta.csv", {"title", "issn", "rank", "year", "source_url"}),
        ):
            with self.subTest(template=name):
                path = templates / name
                self.assertTrue(path.is_file(), f"missing template {path}")
                headers, rows = _read_csv(path, None)
                self.assertTrue(expected_fields.issubset(_resolve_columns(headers)))
                self.assertEqual(rows, [], "templates must ship header-only, no fabricated rows")

    def test_xlsx_import_reads_the_same_schema(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "ranks.xlsx"
            workbook = Workbook()
            worksheet = workbook.active
            worksheet.append(["Title", "Issn", "SJR Best Quartile", "Year", "Categories"])
            worksheet.append(["Journal of Examples", "1234-5679", "Q1", 2023, "Multidisciplinary"])
            workbook.save(path)

            catalog = JournalCatalog()
            imported = catalog.import_file(path)

        self.assertEqual(imported, 1)
        self.assertEqual(catalog.match(journal="Journal of Examples").rank, "Q1")

    def test_import_is_transactional_and_leaves_prior_records_intact(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            good = write(temp_dir, "good.csv", SCIMAGO_SEMICOLON)
            broken = write(temp_dir, "broken.csv", "Title,Issn,Note\nNo rank column,1234-5679,-\n")
            catalog = JournalCatalog()
            catalog.import_file(good)

            with self.assertRaises(ValueError):
                catalog.import_file(broken)

        self.assertEqual(len(catalog), 3)
        self.assertEqual(catalog.match(journal="Journal of Examples").rank, "Q1")

    def test_unrecognized_headers_raise(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(temp_dir, "unrelated.csv", "alpha,beta\n1,2\n")
            catalog = JournalCatalog()

            with self.assertRaises(ValueError) as context:
                catalog.import_file(path)

        self.assertIn("no recognizable journal title or ISSN column", str(context.exception))

    def test_unsupported_extension_raises(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(temp_dir, "ranks.json", "{}")
            catalog = JournalCatalog()

            with self.assertRaises(ValueError) as context:
                catalog.import_file(path)

        self.assertIn("Unsupported ranking file type", str(context.exception))

    def test_rows_without_a_valid_rank_are_dropped(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(
                temp_dir,
                "mixed.csv",
                "Title,Issn,SJR Best Quartile,Year\n"
                "Good Journal,1234-5679,Q2,2023\n"
                "Unranked Journal,2049-3630,Not ranked,2023\n"
                "Blank Journal,0317-8471,,2023\n",
            )
            catalog = JournalCatalog()

            imported = catalog.import_file(path)

        self.assertEqual(imported, 1)
        self.assertEqual(catalog.match(journal="Good Journal").rank, "Q2")
        self.assertIsNone(catalog.match(journal="Unranked Journal"))

    def test_invalid_issn_is_not_stored_and_cannot_match(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(
                temp_dir,
                "bad-issn.csv",
                "Title,Issn,SJR Best Quartile,Year\n"
                "Bad Issn Journal,1234-5678,Q1,2023\n"
                "Good Issn Journal,1234-5679,Q1,2023\n",
            )
            catalog = JournalCatalog()
            catalog.import_file(path)

        records = {record.title: record for record in catalog.records}
        self.assertEqual(records["Bad Issn Journal"].issns, ())
        self.assertIsNone(catalog.match(issns=["1234-5678"]))
        self.assertIsNotNone(catalog.match(issns=["1234-5679"]))

    def test_absent_year_stays_unknown_and_is_never_invented(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(temp_dir, "no-year.csv", "Title,Issn,SJR Best Quartile\nJournal of Examples,1234-5679,Q1\n")
            catalog = JournalCatalog()

            catalog.import_file(path)

        rank = catalog.match(journal="Journal of Examples")
        self.assertIsNone(rank.year)

    def test_explicit_default_year_applies_only_when_asked_for(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(temp_dir, "no-year.csv", "Title,Issn,SJR Best Quartile\nJournal of Examples,1234-5679,Q1\n")

            without = JournalCatalog()
            without.import_file(path)
            with_default = JournalCatalog()
            with_default.import_file(path, default_year=2024)

        self.assertIsNone(without.match(journal="Journal of Examples").year)
        self.assertEqual(with_default.match(journal="Journal of Examples").year, 2024)

    def test_latest_actual_year_wins_and_undated_never_beats_dated(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(
                temp_dir,
                "years.csv",
                "Title,Issn,SJR Best Quartile,Year\n"
                "Journal of Examples,1234-5679,Q4,2020\n"
                "Journal of Examples,1234-5679,Q1,2023\n",
            )
            catalog = JournalCatalog()
            catalog.import_file(path)

        rank = catalog.match(journal="Journal of Examples")
        self.assertEqual(rank.rank, "Q1")
        self.assertEqual(rank.year, 2023)

    def test_contradictory_ranks_for_the_same_latest_year_are_ambiguous(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = write(
                temp_dir,
                "ambiguous.csv",
                "Title,Issn,SJR Best Quartile,Year\n"
                "Journal of Examples,1234-5679,Q1,2023\n"
                "Journal of Examples,1234-5679,Q3,2023\n",
            )
            catalog = JournalCatalog()
            catalog.import_file(path)

        self.assertIsNone(catalog.match(journal="Journal of Examples"))


class MatchingTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temp_dir = tempfile.TemporaryDirectory()
        path = write(self._temp_dir.name, "ranks.csv", SCIMAGO_SEMICOLON)
        self.catalog = JournalCatalog()
        self.catalog.import_file(path)

    def tearDown(self) -> None:
        self._temp_dir.cleanup()

    def test_exact_issn_match_is_preferred_over_title(self) -> None:
        rank = self.catalog.match(issns=["1234-5679"], journal="A Completely Different Name")

        self.assertEqual(rank.matched_by, "issn:1234-5679")
        self.assertEqual(rank.rank, "Q1")

    def test_title_fallback_requires_an_exact_normalized_match(self) -> None:
        self.assertEqual(self.catalog.match(journal="journal   of   examples").matched_by, "title")
        self.assertIsNone(self.catalog.match(journal="Journal of Example"))
        self.assertIsNone(self.catalog.match(journal="Journal of Examples and More"))

    def test_unmatched_journal_stays_unknown(self) -> None:
        self.assertIsNone(self.catalog.match(journal="Unlisted Journal"))
        self.assertIsNone(self.catalog.match())

    def test_title_key_is_conservative(self) -> None:
        self.assertEqual(journal_title_key("The Journal of Examples"), journal_title_key("Journal of Examples"))
        self.assertNotEqual(journal_title_key("Journal of Examples"), journal_title_key("Journal of Example"))

    def test_catalog_matches_returns_both_ranking_systems(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            sjr = write(temp_dir, "sjr.csv", "Title,Issn,SJR Best Quartile,Year\nJournal of Examples,1234-5679,Q1,2023\n")
            sinta = write(temp_dir, "sinta.csv", SINTA_TEMPLATE)
            catalog = JournalCatalog()
            catalog.import_file(sjr)
            catalog.import_file(sinta)

            article = Article(title="A Paper", journal="Journal of Examples", issns=("1234-5679",))
            sjr_rank, sinta_rank = catalog_matches(article, catalog)

        self.assertEqual(sjr_rank.rank, "Q1")
        self.assertEqual(sinta_rank.rank, "S2")

    def test_conference_article_without_a_journal_identity_matches_nothing(self) -> None:
        article = Article(title="A Conference Paper", authors="C Author - Conference on Examples, 2023")

        self.assertEqual(catalog_matches(article, self.catalog), (None, None))

    def test_apply_rank_fills_only_that_systems_fields(self) -> None:
        article = Article(title="A Paper", journal="Journal of Examples")
        rank = self.catalog.match(journal="Journal of Examples")

        updated = apply_rank(article, rank)

        self.assertEqual(updated.sjr_quartile, "Q1")
        self.assertEqual(updated.sjr_year, 2023)
        self.assertEqual(updated.sjr_category, "Multidisciplinary")
        self.assertEqual(updated.sinta_rank, "")

    def test_apply_rank_with_no_match_returns_the_article_unchanged(self) -> None:
        article = Article(title="A Paper", journal="Unlisted Journal")

        self.assertIs(apply_rank(article, None), article)


if __name__ == "__main__":
    unittest.main()
