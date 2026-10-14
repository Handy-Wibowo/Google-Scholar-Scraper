import tempfile
import unittest
from pathlib import Path
import csv

from openpyxl import load_workbook

from google_scholar_scraper.exporters import EXPORT_COLUMNS, save_to_csv, save_to_excel
from google_scholar_scraper.models import Article


class ExporterTests(unittest.TestCase):
    def test_export_receives_normalized_article_rows(self) -> None:
        article = Article(
            title="Deep Learning",
            authors="A Author",
            link="https://example.edu/paper",
            relevance_score=87.5,
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.xlsx"

            save_to_excel([article], path)

            worksheet = load_workbook(path).active

        rows = list(worksheet.iter_rows(values_only=True))
        self.assertEqual(list(rows[0]), EXPORT_COLUMNS)
        self.assertEqual(rows[1][0], "Deep Learning")
        self.assertEqual(rows[1][1], "A Author")
        self.assertEqual(rows[1][2], "https://example.edu/paper")
        self.assertEqual(rows[1][3], 87.5)

    def test_export_uses_blank_relevance_score_when_unranked(self) -> None:
        article = Article(title="Deep Learning", authors="A Author", link="https://example.edu/paper")
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.xlsx"

            save_to_excel([article], path)

            worksheet = load_workbook(path).active

        rows = list(worksheet.iter_rows(values_only=True))
        self.assertEqual(list(rows[0]), EXPORT_COLUMNS)
        self.assertIsNone(rows[1][3])

    def test_csv_export_preserves_unicode_and_escapes_structured_text(self) -> None:
        articles = [
            Article(
                title='یادگیری عمیق, "پزشکی"\nنسخه دوم',
                authors='Author, "A"',
                link="https://example.edu/paper?x=1,y=2",
                relevance_score=91.2,
            ),
            Article(title="Unranked", authors="", link=""),
        ]
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.csv"

            save_to_csv(articles, path)

            raw = path.read_bytes()
            self.assertTrue(raw.startswith(b"\xef\xbb\xbf"))
            with path.open(newline="", encoding="utf-8-sig") as handle:
                rows = list(csv.DictReader(handle))

        self.assertEqual(list(rows[0].keys()), EXPORT_COLUMNS)
        self.assertEqual(rows[0]["Title"], 'یادگیری عمیق, "پزشکی"\nنسخه دوم')
        self.assertEqual(rows[0]["Authors"], 'Author, "A"')
        self.assertEqual(rows[0]["Link"], "https://example.edu/paper?x=1,y=2")
        self.assertEqual(rows[0]["Relevance Score"], "91.2")
        self.assertEqual(rows[1]["Relevance Score"], "")


    def test_new_metadata_columns_are_exported(self) -> None:
        article = Article(
            title="Deep Learning",
            authors="A Author",
            link="https://example.edu/paper",
            relevance_score=87.5,
            doi="10.1038/nature14539",
            journal="Nature",
            issns=("0028-0836", "1476-4687"),
            publication_year=2015,
            sjr_quartile="Q1",
            sjr_year=2023,
            sjr_category="Multidisciplinary",
            sjr_source_url="https://scimagojr.example/journal",
            sinta_rank="S2",
            sinta_year=2024,
            sinta_source_url="https://sinta.example/journals/1",
            pdf_url="https://repo.example/a.pdf",
            pdf_source_url="https://api.crossref.org/works/10.1038/nature14539",
            pdf_status="available",
            pdf_detail="verified",
            pdf_local_path="C:/pdfs/a.pdf",
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.csv"

            save_to_csv([article], path)

            with path.open(newline="", encoding="utf-8-sig") as handle:
                row = next(csv.DictReader(handle))

        for column in (
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
        ):
            with self.subTest(column=column):
                self.assertIn(column, EXPORT_COLUMNS)
        self.assertEqual(row["DOI"], "10.1038/nature14539")
        self.assertEqual(row["Journal"], "Nature")
        self.assertEqual(row["ISSNs"], "0028-0836; 1476-4687")
        self.assertEqual(row["Publication Year"], "2015")
        self.assertEqual(row["SJR Quartile"], "Q1")
        self.assertEqual(row["SINTA Rank"], "S2")
        self.assertEqual(row["PDF Status"], "available")

    def test_unknown_metadata_exports_blank_rather_than_a_guess(self) -> None:
        article = Article(title="Deep Learning", authors="A Author", link="https://example.edu/paper")
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.csv"

            save_to_csv([article], path)

            with path.open(newline="", encoding="utf-8-sig") as handle:
                row = next(csv.DictReader(handle))

        self.assertEqual(row["Journal"], "")
        self.assertEqual(row["SJR Quartile"], "")
        self.assertEqual(row["SINTA Rank"], "")
        self.assertEqual(row["Publication Year"], "")
        self.assertEqual(row["PDF Status"], "unknown")

    def test_formula_injection_is_defused_without_altering_scientific_text(self) -> None:
        articles = [
            Article(title="=cmd|'/c calc'!A0", authors="@SUM(1+1)", link=""),
            Article(title="-A hyphenated title is real text", authors="A Author", link=""),
            Article(title="+3 trials", authors="A Author", link=""),
        ]
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.csv"

            save_to_csv(articles, path)

            with path.open(newline="", encoding="utf-8-sig") as handle:
                rows = list(csv.DictReader(handle))

        self.assertEqual(rows[0]["Title"], "'=cmd|'/c calc'!A0")
        self.assertEqual(rows[0]["Authors"], "'@SUM(1+1)")
        # A leading "-" or "+" is escaped too, so no spreadsheet evaluates it.
        self.assertEqual(rows[1]["Title"], "'-A hyphenated title is real text")
        self.assertEqual(rows[2]["Title"], "'+3 trials")

    def test_excel_export_defuses_formula_injection(self) -> None:
        article = Article(title="=1+1", authors="A Author", link="")
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "articles.xlsx"

            save_to_excel([article], path)

            rows = list(load_workbook(path).active.iter_rows(values_only=True))

        self.assertEqual(rows[1][0], "'=1+1")


if __name__ == "__main__":
    unittest.main()
