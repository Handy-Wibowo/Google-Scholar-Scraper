import tempfile
import unittest
from pathlib import Path
import csv

from openpyxl import load_workbook

from google_scholar_scraper.enrichment import SCHOLAR_AUTHORS_WARNING, UNRESOLVED_WARNING
from google_scholar_scraper.exporters import (
    EXPORT_COLUMNS,
    save_to_bibtex,
    save_to_csv,
    save_to_excel,
    save_to_ris,
)
from google_scholar_scraper.models import Article, CitationRecord


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


class BibTeXWriterTests(unittest.TestCase):
    def test_verified_record_renders_full_entry(self) -> None:
        record = CitationRecord(
            title="Deep learning",
            authors=("Yann LeCun", "Yoshua Bengio", "Geoffrey Hinton"),
            year=2015,
            container="Nature",
            entry_type="journal-article",
            doi="10.1038/nature14539",
            issns=("0028-0836", "1476-4687"),
            volume="521",
            issue="7553",
            pages="436-444",
            publisher="Springer",
            url="https://doi.org/10.1038/nature14539",
            source="crossref",
            verified=True,
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.bib"

            save_to_bibtex([record], path)
            text = path.read_text(encoding="utf-8")

        self.assertIn("@article{lecun_deep_2015,", text)
        self.assertIn("author = {Yann LeCun and Yoshua Bengio and Geoffrey Hinton},", text)
        self.assertIn("title = {{Deep learning}},", text)
        self.assertIn("journal = {Nature},", text)
        self.assertIn("year = {2015},", text)
        self.assertIn("volume = {521},", text)
        self.assertIn("number = {7553},", text)
        self.assertIn("pages = {436-444},", text)
        self.assertIn("doi = {10.1038/nature14539},", text)
        self.assertIn("issn = {0028-0836, 1476-4687},", text)
        self.assertNotIn("note =", text)
        self.assertNotIn("booktitle", text)

    def test_specials_escaped_and_title_double_braced(self) -> None:
        record = CitationRecord(
            title="Escaping {Braces} & 100% of _Cases_",
            container="Proceedings & Notes",
            entry_type="proceedings-article",
            year=2023,
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.bib"

            save_to_bibtex([record], path)
            text = path.read_text(encoding="utf-8")

        self.assertIn("@inproceedings{anon_escaping_2023,", text)
        self.assertIn("title = {{Escaping \\{Braces\\} \\& 100\\% of \\_Cases\\_}},", text)
        self.assertIn("booktitle = {Proceedings \\& Notes},", text)
        self.assertNotIn("author =", text)

    def test_duplicate_titles_get_unique_keys(self) -> None:
        records = [
            CitationRecord(title="Learning Deep Things", year=2020),
            CitationRecord(title="Learning Deep Things", year=2020),
            CitationRecord(title="Learning Deep Things", year=2020),
        ]
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.bib"

            save_to_bibtex(records, path)
            text = path.read_text(encoding="utf-8")

        self.assertIn("@misc{anon_learning_2020,", text)
        self.assertIn("@misc{anon_learning_2020-2,", text)
        self.assertIn("@misc{anon_learning_2020-3,", text)

    def test_unresolved_record_has_no_author_and_carries_the_review_note(self) -> None:
        record = CitationRecord(
            title="An Unresolved Paper",
            year=2024,
            source="scholar",
            fallback_authors_line="A Author, B Author - Journal of Examples, 2024 - publisher.example",
            warnings=(UNRESOLVED_WARNING, SCHOLAR_AUTHORS_WARNING),
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.bib"

            save_to_bibtex([record], path)
            text = path.read_text(encoding="utf-8")

        self.assertNotIn("author =", text)
        self.assertIn("note = {A Author, B Author - Journal of Examples, 2024 - publisher.example; ", text)
        self.assertIn(UNRESOLVED_WARNING, text)
        self.assertIn(SCHOLAR_AUTHORS_WARNING, text)

    def test_values_are_single_lined(self) -> None:
        record = CitationRecord(title="Broken\nTitle\r\nHere")
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.bib"

            save_to_bibtex([record], path)
            text = path.read_text(encoding="utf-8")

        self.assertIn("title = {{Broken Title Here}},", text)


class RISWriterTests(unittest.TestCase):
    def test_verified_journal_record_frames_and_fields(self) -> None:
        record = CitationRecord(
            title="Deep learning",
            authors=("Yann LeCun", "Yoshua Bengio"),
            year=2015,
            container="Nature",
            entry_type="journal-article",
            doi="10.1038/nature14539",
            issns=("0028-0836",),
            pages="436-444",
            publisher="Springer",
            url="https://doi.org/10.1038/nature14539",
            source="crossref",
            verified=True,
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.ris"

            save_to_ris([record], path)
            raw = path.read_bytes()

        self.assertFalse(raw.startswith(b"\xef\xbb\xbf"))
        text = raw.decode("utf-8")
        self.assertIn("TY  - JOUR", text)
        self.assertIn("AU  - LeCun, Yann", text)
        self.assertIn("AU  - Bengio, Yoshua", text)
        self.assertIn("TI  - Deep learning", text)
        self.assertIn("T2  - Nature", text)
        self.assertIn("PY  - 2015", text)
        self.assertIn("SP  - 436", text)
        self.assertIn("EP  - 444", text)
        self.assertIn("DO  - 10.1038/nature14539", text)
        self.assertIn("UR  - https://doi.org/10.1038/nature14539", text)
        self.assertIn("SN  - 0028-0836", text)
        self.assertIn("PB  - Springer", text)
        self.assertTrue(text.rstrip("\r\n").endswith("ER  - "))
        self.assertIn("\r\n", text)

    def test_proceedings_maps_to_conf_and_open_page_range_stays_in_sp(self) -> None:
        record = CitationRecord(
            title="A Conference Paper",
            container="Conference on Examples",
            entry_type="proceedings-article",
            year=2023,
            pages="1007",
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.ris"

            save_to_ris([record], path)
            text = path.read_text(encoding="utf-8")

        self.assertIn("TY  - CONF", text)
        self.assertIn("SP  - 1007", text)
        self.assertNotIn("EP  - ", text)

    def test_unresolved_record_has_no_au_and_carries_the_note(self) -> None:
        record = CitationRecord(
            title="An Unresolved Paper",
            source="scholar",
            fallback_authors_line="A Author, B Author - Journal of Examples, 2024 - publisher.example",
            warnings=(UNRESOLVED_WARNING, SCHOLAR_AUTHORS_WARNING),
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "citations.ris"

            save_to_ris([record], path)
            text = path.read_text(encoding="utf-8")

        self.assertIn("TY  - GEN", text)
        self.assertNotIn("AU  - ", text)
        self.assertIn("N1  - A Author, B Author - Journal of Examples, 2024 - publisher.example | ", text)
        self.assertIn(UNRESOLVED_WARNING, text)
        self.assertIn(SCHOLAR_AUTHORS_WARNING, text)


if __name__ == "__main__":
    unittest.main()
