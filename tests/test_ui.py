import queue
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch
from dataclasses import replace

from google_scholar_scraper.journal_catalog import JournalCatalog, apply_rank, catalog_matches
from google_scholar_scraper.models import (
    PDF_STATUS_AVAILABLE,
    PDF_STATUS_UNAVAILABLE,
    PDF_STATUS_UNKNOWN,
    Article,
    ExtractionResult,
    ExtractionStatus,
)
from google_scholar_scraper.ui import tkinter_app
from google_scholar_scraper.ui.tkinter_app import (
    NO_RANKING_NOTICE,
    PDF_FILTER_ALL,
    PDF_FILTER_AVAILABLE,
    PDF_FILTER_UNAVAILABLE,
    PDF_FILTER_UNKNOWN,
    MainWindow,
    SearchRequest,
    SearchWorker,
    TaskWorker,
    WorkerMessage,
    WINDOW_DEFAULT_SIZE,
    WINDOW_MIN_SIZE,
    article_detail_text,
    article_iid,
    article_row,
    article_table_row,
    export_path,
    filtered_articles,
    journal_rank_label,
    pdf_status_label,
    progress_value,
    result_summary,
    run_check,
    run_download,
    run_import,
    status_message,
    validate_search_inputs,
    export_error_message,
)


class DummyVar:
    def __init__(self, value=None) -> None:
        self.value = value

    def get(self):
        return self.value

    def set(self, value) -> None:
        self.value = value


class DummyWidget:
    def __init__(self) -> None:
        self.state = None
        self.visible = True

    def configure(self, **kwargs) -> None:
        if "state" in kwargs:
            self.state = kwargs["state"]

    def grid(self, **_kwargs) -> None:
        self.visible = True

    def grid_remove(self) -> None:
        self.visible = False

    def winfo_exists(self) -> bool:
        return True


class DummyTable:
    """Minimal Treeview stand-in that keeps iid -> row identity like the real widget."""

    def __init__(self) -> None:
        self.rows = []
        self.iids = []
        self._selection = ()

    def insert(self, _parent, _index, iid=None, values=()) -> None:
        self.iids.append(iid)
        self.rows.append(values)

    def get_children(self):
        return list(self.iids)

    def delete(self, item) -> None:
        position = self.iids.index(item)
        self.iids.pop(position)
        self.rows.pop(position)

    def selection(self):
        return tuple(iid for iid in self._selection if iid in self.iids)

    def selection_set(self, items) -> None:
        self._selection = tuple(items)


class DummyStyle:
    def configure(self, *_args, **_kwargs) -> None:
        return None


class FakeRunningWorker:
    def is_alive(self) -> bool:
        return True


class UiHelperTests(unittest.TestCase):
    def test_validate_search_inputs_rejects_empty_query(self) -> None:
        with self.assertRaisesRegex(ValueError, "Enter a search query"):
            validate_search_inputs("   ", "1", True)

    def test_validate_search_inputs_rejects_invalid_page_count(self) -> None:
        with self.assertRaisesRegex(ValueError, "whole number"):
            validate_search_inputs("query", "abc", True)

    def test_validate_search_inputs_rejects_zero_or_negative_page_count(self) -> None:
        with self.assertRaisesRegex(ValueError, "greater than zero"):
            validate_search_inputs("query", "0", True)
        with self.assertRaisesRegex(ValueError, "greater than zero"):
            validate_search_inputs("query", "-1", True)

    def test_validate_search_inputs_accepts_valid_input(self) -> None:
        request = validate_search_inputs("  machine learning  ", "2", False, " out ")

        self.assertEqual(request, SearchRequest("machine learning", 2, False, "out"))

    def test_progress_value_never_reports_false_full_for_partial(self) -> None:
        self.assertEqual(progress_value(1, 4), 25)
        self.assertEqual(progress_value(4, 4), 100)
        self.assertEqual(progress_value(0, 0), 0)

    def test_article_row_formats_score_and_missing_link(self) -> None:
        ranked = Article(title="Title", authors="Authors", link="https://example.edu", relevance_score=91.24)
        unranked = Article(title="No Link", authors="", link="")

        self.assertEqual(article_row(ranked), ("Title", "Authors", "91.2", "Open link"))
        self.assertEqual(article_row(unranked), ("No Link", "", "", "No link"))

    def test_export_path_preserves_default_filename(self) -> None:
        self.assertEqual(export_path(""), Path("scholar_articles.xlsx"))
        self.assertEqual(export_path("out"), Path("out") / "scholar_articles.xlsx")

    def test_every_status_has_user_message(self) -> None:
        for status in ExtractionStatus:
            result = ExtractionResult(status=status, articles=[], requested_pages=1)
            self.assertTrue(status_message(result))

    def test_result_summary_includes_counts(self) -> None:
        result = ExtractionResult(
            status=ExtractionStatus.SUCCESS,
            articles=[Article("A")],
            requested_pages=3,
            successful_pages=2,
            duplicates_removed=1,
            invalid_articles_removed=1,
        )

        self.assertEqual(result_summary(result), "1 result found | 2/3 pages completed | 1 duplicate removed | 1 invalid record removed")

    def test_export_error_message_is_user_facing(self) -> None:
        self.assertEqual(export_error_message(PermissionError("denied")), "Export failed: denied")


class WorkerTests(unittest.TestCase):
    def test_worker_emits_progress_and_result_without_tkinter(self) -> None:
        messages: queue.Queue[WorkerMessage] = queue.Queue()
        cancel_event = threading.Event()

        def fake_scrape(_query, _pages, ranking_enabled, progress_callback, cancel_event):
            self.assertTrue(ranking_enabled)
            self.assertFalse(cancel_event.is_set())
            progress_callback(1, 2, "requesting")
            return ExtractionResult(
                status=ExtractionStatus.SUCCESS,
                articles=[Article("A", relevance_score=50.0)],
                requested_pages=2,
                successful_pages=1,
                message="Done",
            )

        worker = SearchWorker(SearchRequest("query", 2, True), messages, cancel_event, scrape_func=fake_scrape)
        worker.start()
        worker.join(timeout=2)

        self.assertFalse(worker.is_alive())
        self.assertEqual(messages.get_nowait().kind, "progress")
        result_message = messages.get_nowait()
        self.assertEqual(result_message.kind, "result")
        self.assertEqual(result_message.payload.status, ExtractionStatus.SUCCESS)

    def test_worker_surfaces_exception_as_network_error_result(self) -> None:
        messages: queue.Queue[WorkerMessage] = queue.Queue()
        cancel_event = threading.Event()

        def failing_scrape(*_args, **_kwargs):
            raise RuntimeError("boom")

        worker = SearchWorker(SearchRequest("query", 1, True), messages, cancel_event, scrape_func=failing_scrape)
        worker.start()
        worker.join(timeout=2)

        result_message = messages.get_nowait()
        self.assertEqual(result_message.kind, "result")
        self.assertEqual(result_message.payload.status, ExtractionStatus.NETWORK_ERROR)


def state_window() -> MainWindow:
    """A MainWindow instance with all Tk widgets replaced by stand-ins."""
    window = MainWindow.__new__(MainWindow)
    window.current_articles = []
    window.current_request = SearchRequest("query", 1, True, "")
    window.worker = None
    window.cancel_event = None
    window.operation = None
    window.catalog = JournalCatalog()
    window.summary_base = "No search has run yet."
    window._closed = False
    window.detail_window = None
    window.detail_body = None
    window.query_var = DummyVar("query")
    window.pages_var = DummyVar("1")
    window.ranking_var = DummyVar(True)
    window.year_options = tkinter_app.year_filter_options(2030)
    window.year_preset_var = DummyVar("Any time")
    window.year_from_var = DummyVar("")
    window.year_to_var = DummyVar("")
    window.combo_year = DummyWidget()
    window.entry_year_from = DummyWidget()
    window.entry_year_to = DummyWidget()
    window.output_folder_var = DummyVar("")
    window.ranking_path_var = DummyVar("")
    window.ranking_year_var = DummyVar("")
    window.ranking_notice_var = DummyVar("")
    window.pdf_filter_var = DummyVar(PDF_FILTER_ALL)
    window.status_var = DummyVar("")
    window.status_heading_var = DummyVar("")
    window.summary_var = DummyVar("")
    window.applied_year_var = DummyVar("Years: any time")
    window.empty_title_var = DummyVar("")
    window.empty_detail_var = DummyVar("")
    window.progress_var = DummyVar(0)
    window.status_frame = DummyWidget()
    window.status_accent = DummyWidget()
    window.status_heading_label = DummyWidget()
    window.status_detail_label = DummyWidget()
    window.style = DummyStyle()
    window.empty_frame = DummyWidget()
    window.table_container = DummyWidget()
    window.button_search = DummyWidget()
    window.button_cancel = DummyWidget()
    window.button_export_excel = DummyWidget()
    window.button_export_csv = DummyWidget()
    window.button_check = DummyWidget()
    window.button_download = DummyWidget()
    window.button_details = DummyWidget()
    window.button_import = DummyWidget()
    window.combo_filter = DummyWidget()
    window.entry_query = DummyWidget()
    window.entry_pages = DummyWidget()
    window.entry_folder = DummyWidget()
    window.entry_ranking_path = DummyWidget()
    window.entry_ranking_year = DummyWidget()
    window.button_browse = DummyWidget()
    window.check_ranking = DummyWidget()
    window.results_table = DummyTable()
    return window


class MainWindowStateTests(unittest.TestCase):
    def make_window(self) -> MainWindow:
        return state_window()

    def test_running_state_disables_search_and_enables_cancel(self) -> None:
        window = self.make_window()

        window._set_running_state()

        self.assertEqual(window.button_search.state, tkinter_app.tk.DISABLED)
        self.assertEqual(window.button_cancel.state, tkinter_app.tk.NORMAL)
        self.assertEqual(window.button_export_excel.state, tkinter_app.tk.DISABLED)
        self.assertEqual(window.button_export_csv.state, tkinter_app.tk.DISABLED)

    def test_idle_state_enables_export_only_with_results(self) -> None:
        window = self.make_window()
        window._set_idle_state()
        self.assertEqual(window.button_export_excel.state, tkinter_app.tk.DISABLED)
        self.assertEqual(window.button_export_csv.state, tkinter_app.tk.DISABLED)

        window.current_articles = [Article("A")]
        window._set_idle_state()
        self.assertEqual(window.button_export_excel.state, tkinter_app.tk.NORMAL)
        self.assertEqual(window.button_export_csv.state, tkinter_app.tk.NORMAL)

    def test_search_does_not_start_second_worker(self) -> None:
        window = self.make_window()
        window.worker = FakeRunningWorker()

        window.start_search()

        self.assertIsInstance(window.worker, FakeRunningWorker)

    def test_cancel_sets_event_and_status(self) -> None:
        window = self.make_window()
        window.worker = FakeRunningWorker()
        window.cancel_event = threading.Event()

        window.cancel_search()

        self.assertTrue(window.cancel_event.is_set())
        self.assertEqual(window.button_cancel.state, tkinter_app.tk.DISABLED)
        self.assertEqual(window.status_heading_var.get(), "Cancelling search")
        self.assertEqual(window.status_var.get(), "Finishing the current safe stop point...")

    def test_handle_progress_updates_status_and_progress(self) -> None:
        window = self.make_window()

        window._handle_progress({"current_page": 1, "total_pages": 4, "phase": "requesting"})

        self.assertEqual(window.progress_var.get(), 25)
        self.assertEqual(window.status_heading_var.get(), "Searching Google Scholar")
        self.assertEqual(window.status_var.get(), "Requesting page 1 of 4...")

    def test_handle_success_result_renders_articles_and_enables_export(self) -> None:
        window = self.make_window()
        result = ExtractionResult(
            status=ExtractionStatus.SUCCESS,
            articles=[Article("A", "Author", "https://example.edu", 80.0)],
            requested_pages=1,
            successful_pages=1,
            message="Done",
        )

        window._handle_result(result)

        self.assertEqual(window.current_articles, result.articles)
        self.assertEqual(
            window.results_table.rows,
            [("A", "Author", "80.0", "", "Unknown", "Open link")],
        )
        self.assertEqual(window.results_table.iids, [article_iid(0)])
        self.assertEqual(window.status_heading_var.get(), "Search complete")
        self.assertEqual(window.status_var.get(), "Done")
        self.assertEqual(window.button_export_excel.state, tkinter_app.tk.NORMAL)
        self.assertEqual(window.button_export_csv.state, tkinter_app.tk.NORMAL)

    def test_new_search_is_labelled_from_the_loaded_catalogue(self) -> None:
        window = self.make_window()
        with tempfile.TemporaryDirectory() as tmp:
            sjr = Path(tmp) / "sjr.csv"
            sjr.write_text("Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q2,2024\n", encoding="utf-8")
            sinta = Path(tmp) / "sinta.csv"
            sinta.write_text("Title,ISSN,SINTA Rank,Year\nExample Journal,0028-0836,S3,2025\n", encoding="utf-8")
            first = run_import(str(sjr), None, lambda *args: None, JournalCatalog())
            window.catalog = run_import(str(sinta), None, lambda *args: None, first["catalog"])["catalog"]

        result = ExtractionResult(
            ExtractionStatus.SUCCESS,
            [Article("Example paper", issns=("0028-0836",))],
            requested_pages=1,
            successful_pages=1,
        )
        window._handle_result(result)

        labelled = window.current_articles[0]
        self.assertEqual(labelled.sjr_quartile, "Q2")
        self.assertEqual(labelled.sinta_rank, "S3")
        self.assertEqual(window.results_table.rows[0][3], "Q2 / S3")

    def test_new_search_stays_unlabelled_without_a_catalogue(self) -> None:
        window = self.make_window()
        result = ExtractionResult(
            ExtractionStatus.SUCCESS,
            [Article("Example paper", issns=("0028-0836",))],
            requested_pages=1,
            successful_pages=1,
        )
        window._handle_result(result)

        self.assertEqual(window.current_articles[0].sjr_quartile, "")
        self.assertEqual(window.results_table.rows[0][3], "")

    def test_partial_and_cancelled_results_are_rendered(self) -> None:
        window = self.make_window()
        for status in (ExtractionStatus.PARTIAL_SUCCESS, ExtractionStatus.CANCELLED):
            result = ExtractionResult(
                status=status,
                articles=[Article("A")],
                requested_pages=3,
                successful_pages=1,
                message=status.value,
            )
            window._handle_result(result)
            self.assertEqual(window.current_articles, result.articles)
            self.assertEqual(window.button_export_excel.state, tkinter_app.tk.NORMAL)
            self.assertEqual(window.button_export_csv.state, tkinter_app.tk.NORMAL)

    def test_export_uses_final_displayed_articles(self) -> None:
        window = self.make_window()
        window.current_articles = [Article("A")]
        with tempfile.TemporaryDirectory() as temp_dir:
            window.current_request = SearchRequest("query", 1, True, temp_dir)
            expected_path = Path(temp_dir) / "scholar_articles.xlsx"

            with patch("google_scholar_scraper.ui.tkinter_app.save_to_excel") as save_to_excel:
                window.export_excel_results()

        save_to_excel.assert_called_once_with([Article("A")], expected_path)

    def test_excel_export_failure_updates_status_without_crashing(self) -> None:
        window = self.make_window()
        window.current_articles = [Article("A")]
        with tempfile.TemporaryDirectory() as temp_dir:
            window.current_request = SearchRequest("query", 1, True, temp_dir)

            with (
                patch("google_scholar_scraper.ui.tkinter_app.save_to_excel", side_effect=PermissionError("denied")),
                patch("google_scholar_scraper.ui.tkinter_app.messagebox.showerror") as showerror,
            ):
                window.export_excel_results()

        self.assertEqual(window.status_var.get(), "Export failed: denied")
        showerror.assert_called_once_with("Export failed", "Export failed: denied")

    def test_csv_export_uses_final_displayed_articles(self) -> None:
        window = self.make_window()
        window.current_articles = [Article("A")]
        with tempfile.TemporaryDirectory() as temp_dir:
            window.current_request = SearchRequest("query", 1, True, temp_dir)
            expected_path = Path(temp_dir) / "scholar_articles.csv"

            with patch("google_scholar_scraper.ui.tkinter_app.save_to_csv") as save_to_csv:
                window.export_csv_results()

        save_to_csv.assert_called_once_with([Article("A")], expected_path)

    def test_csv_export_failure_updates_status_without_crashing(self) -> None:
        window = self.make_window()
        window.current_articles = [Article("A")]
        with tempfile.TemporaryDirectory() as temp_dir:
            window.current_request = SearchRequest("query", 1, True, temp_dir)

            with (
                patch("google_scholar_scraper.ui.tkinter_app.save_to_csv", side_effect=OSError("disk full")),
                patch("google_scholar_scraper.ui.tkinter_app.messagebox.showerror") as showerror,
            ):
                window.export_csv_results()

        self.assertEqual(window.status_var.get(), "Export failed: disk full")
        showerror.assert_called_once_with("Export failed", "Export failed: disk full")


class FilterAndMappingTests(unittest.TestCase):
    def sample(self) -> list[Article]:
        return [
            Article("Available", pdf_status=PDF_STATUS_AVAILABLE),
            Article("Missing", pdf_status=PDF_STATUS_UNAVAILABLE),
            Article("Unknown", pdf_status=PDF_STATUS_UNKNOWN),
            Article("Also missing", pdf_status=PDF_STATUS_UNAVAILABLE),
        ]

    def test_filter_all_keeps_stored_order_and_indices(self) -> None:
        pairs = filtered_articles(self.sample(), PDF_FILTER_ALL)

        self.assertEqual([index for index, _ in pairs], [0, 1, 2, 3])

    def test_each_filter_selects_only_its_status(self) -> None:
        articles = self.sample()

        self.assertEqual([i for i, _ in filtered_articles(articles, PDF_FILTER_AVAILABLE)], [0])
        self.assertEqual([i for i, _ in filtered_articles(articles, PDF_FILTER_UNAVAILABLE)], [1, 3])
        self.assertEqual([i for i, _ in filtered_articles(articles, PDF_FILTER_UNKNOWN)], [2])

    def test_filtering_never_mutates_or_reorders_stored_records(self) -> None:
        articles = self.sample()
        before = list(articles)

        filtered_articles(articles, PDF_FILTER_UNAVAILABLE)

        self.assertEqual(articles, before)

    def test_unknown_filter_name_falls_back_to_all(self) -> None:
        pairs = filtered_articles(self.sample(), "not a filter")

        self.assertEqual(len(pairs), 4)

    def test_iid_round_trips_the_stored_index(self) -> None:
        self.assertEqual(article_iid(0), "article-0")
        self.assertEqual(article_iid(12), "article-12")

    def test_table_row_carries_rank_pdf_and_link_columns(self) -> None:
        ranked = Article(
            "T",
            "A",
            "https://example.edu",
            80.0,
            sjr_quartile="Q1",
            sinta_rank="S2",
            pdf_status=PDF_STATUS_AVAILABLE,
        )

        self.assertEqual(article_table_row(ranked), ("T", "A", "80.0", "Q1 / S2", "Public PDF", "Open link"))
        self.assertEqual(journal_rank_label(Article("plain")), "")
        self.assertEqual(pdf_status_label(Article("plain")), "Unknown")

    def test_downloaded_record_is_labelled_downloaded(self) -> None:
        article = Article("T", pdf_status=PDF_STATUS_AVAILABLE, pdf_local_path="C:/pdfs/t.pdf")

        self.assertEqual(pdf_status_label(article), "Downloaded")

    def test_detail_text_reports_source_year_and_disclaimer(self) -> None:
        article = Article(
            "T",
            journal="Journal of Testing",
            publication_year=2021,
            sjr_quartile="Q1",
            sjr_year=2023,
            sjr_category="Library science",
            sjr_source_url="https://scimagojr.com/x",
            pdf_status=PDF_STATUS_UNAVAILABLE,
        )

        text = article_detail_text(article)

        self.assertIn("Journal: Journal of Testing", text)
        self.assertIn("Publication year: 2021", text)
        self.assertIn("SJR quartile: Q1   (ranking year: 2023)", text)
        self.assertIn("SJR category: Library science", text)
        self.assertIn("SJR source: https://scimagojr.com/x", text)
        self.assertIn("not proof of current Scopus indexing", text)
        self.assertIn("unknown", text)

    def test_detail_text_without_selection_still_states_the_disclaimer(self) -> None:
        self.assertIn("not proof of current Scopus indexing", article_detail_text(None))


class RunCheckTests(unittest.TestCase):
    def test_check_enriches_then_validates_and_preserves_order(self) -> None:
        articles = [Article("One"), Article("Two")]
        enriched = [Article("One", journal="J"), Article("Two", journal="J")]
        seen: list[tuple] = []

        def fake_check(article, cancel_event):
            seen.append((article.title, cancel_event))
            return replace(article, pdf_status=PDF_STATUS_AVAILABLE)

        cancel = threading.Event()
        with (
            patch("google_scholar_scraper.ui.tkinter_app.enrich_articles", return_value=enriched) as enrich,
            patch("google_scholar_scraper.ui.tkinter_app.check_pdf_availability", side_effect=fake_check),
        ):
            result = run_check(articles, None, cancel, lambda *args: None)

        self.assertEqual([a.title for a in result["articles"]], ["One", "Two"])
        self.assertTrue(all(a.pdf_status == PDF_STATUS_AVAILABLE for a in result["articles"]))
        self.assertEqual(result["enrich_error"], "")
        self.assertEqual([title for title, _ in seen], ["One", "Two"])
        self.assertTrue(all(event is cancel for _, event in seen))
        self.assertEqual(enrich.call_count, 1)

    def test_metadata_failure_keeps_records_and_still_checks_pdfs(self) -> None:
        articles = [Article("One"), Article("Two")]

        with (
            patch(
                "google_scholar_scraper.ui.tkinter_app.enrich_articles",
                side_effect=RuntimeError("provider down"),
            ),
            patch(
                "google_scholar_scraper.ui.tkinter_app.check_pdf_availability",
                side_effect=lambda article, _cancel: replace(article, pdf_status=PDF_STATUS_UNAVAILABLE),
            ),
        ):
            result = run_check(articles, None, threading.Event(), lambda *args: None)

        self.assertEqual([a.title for a in result["articles"]], ["One", "Two"])
        self.assertTrue(all(a.pdf_status == PDF_STATUS_UNAVAILABLE for a in result["articles"]))
        self.assertIn("Metadata lookup failed (RuntimeError)", result["enrich_error"])
        self.assertIn("original records were kept", result["enrich_error"])

    def test_pdf_failure_keeps_the_successful_enrichment(self) -> None:
        articles = [Article("One")]
        enriched = [Article("One", journal="Kept Journal")]

        def explode(_article, _cancel):
            raise OSError("connection reset")

        with (
            patch("google_scholar_scraper.ui.tkinter_app.enrich_articles", return_value=enriched),
            patch("google_scholar_scraper.ui.tkinter_app.check_pdf_availability", side_effect=explode),
        ):
            result = run_check(articles, None, threading.Event(), lambda *args: None)

        self.assertEqual(result["articles"], enriched)

    def test_cancelled_check_returns_every_record(self) -> None:
        articles = [Article("One"), Article("Two"), Article("Three")]
        cancel = threading.Event()

        def cancel_after_first(article, _cancel):
            cancel.set()
            return replace(article, pdf_status=PDF_STATUS_AVAILABLE)

        with (
            patch("google_scholar_scraper.ui.tkinter_app.enrich_articles", return_value=articles),
            patch("google_scholar_scraper.ui.tkinter_app.check_pdf_availability", side_effect=cancel_after_first),
        ):
            result = run_check(articles, None, cancel, lambda *args: None)

        self.assertTrue(result["cancelled"])
        self.assertEqual(len(result["articles"]), 3)
        self.assertEqual([a.title for a in result["articles"]], ["One", "Two", "Three"])

    def test_check_reports_progress_for_both_phases(self) -> None:
        articles = [Article("One"), Article("Two")]
        progress: list[tuple] = []

        with (
            patch("google_scholar_scraper.ui.tkinter_app.enrich_articles", return_value=articles),
            patch(
                "google_scholar_scraper.ui.tkinter_app.check_pdf_availability",
                side_effect=lambda article, _cancel: article,
            ),
        ):
            run_check(articles, None, threading.Event(), lambda *args: progress.append(args))

        self.assertTrue(progress)
        self.assertTrue(all(total == 4 for _, total, _ in progress))
        self.assertEqual([current for current, _, _ in progress], sorted(current for current, _, _ in progress))


class RunImportTests(unittest.TestCase):
    def test_import_returns_a_fresh_catalogue(self) -> None:
        catalog = JournalCatalog()
        with patch.object(JournalCatalog, "import_file", return_value=3) as import_file:
            result = run_import("ranks.csv", 2023, lambda *args: None)

        self.assertIsInstance(result["catalog"], JournalCatalog)
        self.assertIsNot(result["catalog"], catalog)
        self.assertEqual(result["count"], 3)
        self.assertEqual(result["year"], 2023)
        import_file.assert_called_once_with("ranks.csv", 2023)

    def test_import_error_leaves_the_called_catalogue_untouched(self) -> None:
        with patch.object(JournalCatalog, "import_file", side_effect=ValueError("bad header")):
            with self.assertRaisesRegex(ValueError, "bad header"):
                run_import("ranks.csv", None, lambda *args: None)

    def test_import_does_not_mutate_the_existing_catalogue(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            first = Path(tmp) / "sjr.csv"
            first.write_text("Title,ISSN,SJR Quartile,SJR Year\nA Journal,0028-0836,Q2,2024\n", encoding="utf-8")
            second = Path(tmp) / "sinta.csv"
            second.write_text("Title,ISSN,SINTA Rank,Year\nB Journal,1234-5678,S3,2025\n", encoding="utf-8")

            loaded = run_import(str(first), None, lambda *args: None, JournalCatalog())["catalog"]
            result = run_import(str(second), None, lambda *args: None, loaded)

        self.assertEqual(len(loaded), 1, "the window's catalogue must not be touched by the worker")
        self.assertEqual(result["count"], 1)
        self.assertEqual(result["total"], 2)

    def test_sequential_sjr_and_sinta_imports_keep_both_label_sets(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            sjr = Path(tmp) / "sjr.csv"
            sjr.write_text(
                "Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q2,2024\n", encoding="utf-8"
            )
            sinta = Path(tmp) / "sinta.csv"
            sinta.write_text("Title,ISSN,SINTA Rank,Year\nExample Journal,0028-0836,S3,2025\n", encoding="utf-8")

            first = run_import(str(sjr), None, lambda *args: None, JournalCatalog())
            second = run_import(str(sinta), None, lambda *args: None, first["catalog"])

        catalog = second["catalog"]
        self.assertEqual(len(catalog), 2, "both imports must be retained")
        ranks = catalog.match_all(issns=("0028-0836",))
        self.assertEqual({rank.kind for rank in ranks}, {"sjr", "sinta"})

    def test_newer_year_of_the_same_source_accumulates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            old = Path(tmp) / "sjr-2023.csv"
            old.write_text("Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q3,2023\n", encoding="utf-8")
            new = Path(tmp) / "sjr-2024.csv"
            new.write_text("Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q2,2024\n", encoding="utf-8")

            first = run_import(str(old), None, lambda *args: None, JournalCatalog())
            second = run_import(str(new), None, lambda *args: None, first["catalog"])

        catalog = second["catalog"]
        self.assertEqual(len(catalog), 2)
        self.assertEqual(catalog.match(issns=("0028-0836",)).year, 2024, "the newest year must win the match")

    def test_rejected_file_keeps_previously_imported_records(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            good = Path(tmp) / "sjr.csv"
            good.write_text("Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q2,2024\n", encoding="utf-8")
            bad = Path(tmp) / "broken.csv"
            bad.write_text("this is not a ranking file\n", encoding="utf-8")

            first = run_import(str(good), None, lambda *args: None, JournalCatalog())
            with self.assertRaises(Exception):
                run_import(str(bad), None, lambda *args: None, first["catalog"])

        self.assertEqual(len(first["catalog"]), 1, "a rejected file must not disturb loaded records")
        self.assertEqual(first["catalog"].match(issns=("0028-0836",)).rank, "Q2")

    def test_combined_catalogue_labels_a_new_search_result(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            sjr = Path(tmp) / "sjr.csv"
            sjr.write_text("Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q2,2024\n", encoding="utf-8")
            sinta = Path(tmp) / "sinta.csv"
            sinta.write_text("Title,ISSN,SINTA Rank,Year\nExample Journal,0028-0836,S3,2025\n", encoding="utf-8")

            first = run_import(str(sjr), None, lambda *args: None, JournalCatalog())
            second = run_import(str(sinta), None, lambda *args: None, first["catalog"])

        catalog = second["catalog"]
        unchecked = Article("Example paper", issns=("0028-0836",))
        sjr_rank, sinta_rank = catalog_matches(unchecked, catalog)
        labelled = apply_rank(apply_rank(unchecked, sjr_rank), sinta_rank)

        self.assertEqual(labelled.sjr_quartile, "Q2")
        self.assertEqual(labelled.sinta_rank, "S3")
        self.assertEqual(journal_rank_label(labelled), "Q2 / S3")

    def test_unchecked_journal_stays_unlabelled(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            sjr = Path(tmp) / "sjr.csv"
            sjr.write_text("Title,ISSN,SJR Quartile,SJR Year\nExample Journal,0028-0836,Q2,2024\n", encoding="utf-8")
            first = run_import(str(sjr), None, lambda *args: None, JournalCatalog())

        other = Article("Other paper", issns=("1234-5678",))
        sjr_rank, sinta_rank = catalog_matches(other, first["catalog"])
        labelled = apply_rank(apply_rank(other, sjr_rank), sinta_rank)

        self.assertEqual(labelled, other)
        self.assertEqual(journal_rank_label(labelled), "")


class FinishDownloadTests(unittest.TestCase):
    """The download summary must count files this run actually wrote, never retained old paths."""

    def make_window(self, articles: list[Article]) -> MainWindow:
        window = state_window()
        window.current_articles = list(articles)
        return window

    def test_failed_retry_does_not_count_the_retained_old_path(self) -> None:
        original = replace(
            Article("One", pdf_status=PDF_STATUS_AVAILABLE),
            pdf_local_path="C:/existing-fixture/old.pdf",
        )
        window = self.make_window([original])
        failed = replace(
            original,
            pdf_status=PDF_STATUS_UNAVAILABLE,
            pdf_detail="permission denied",
            pdf_local_path="C:/existing-fixture/old.pdf",
        )

        window._finish_download({"indices": [0], "articles": [failed], "folder": "C:/unwritable-fixture"})

        message = window.status_var.get()
        self.assertNotIn("Saved 1", message)
        self.assertIn("did not write a new file", message)
        self.assertEqual(window.current_articles[0].pdf_local_path, "C:/existing-fixture/old.pdf")

    def test_newly_written_file_is_counted(self) -> None:
        window = self.make_window([Article("One", pdf_status=PDF_STATUS_AVAILABLE)])
        saved = replace(window.current_articles[0], pdf_local_path="C:/pdfs/one.pdf")

        window._finish_download({"indices": [0], "articles": [saved], "folder": "C:/pdfs"})

        self.assertIn("Saved 1 new public PDF(s) to C:/pdfs.", window.status_var.get())
        self.assertEqual(window.current_articles[0].pdf_local_path, "C:/pdfs/one.pdf")

    def test_mixed_new_and_failed_retry_is_counted_accurately(self) -> None:
        retried = replace(Article("Old", pdf_status=PDF_STATUS_AVAILABLE), pdf_local_path="C:/old/old.pdf")
        fresh = Article("Fresh", pdf_status=PDF_STATUS_AVAILABLE)
        window = self.make_window([retried, fresh])
        results = [
            replace(retried, pdf_status=PDF_STATUS_UNAVAILABLE, pdf_detail="permission denied"),
            replace(fresh, pdf_local_path="C:/pdfs/fresh.pdf"),
        ]

        window._finish_download({"indices": [0, 1], "articles": results, "folder": "C:/pdfs"})

        message = window.status_var.get()
        self.assertIn("Saved 1 new public PDF(s) to C:/pdfs.", message)
        self.assertIn("1 record(s) kept the PDF already saved earlier", message)
        self.assertNotIn("Saved 2", message)
        self.assertEqual(window.current_articles[0].pdf_local_path, "C:/old/old.pdf")
        self.assertEqual(window.current_articles[0].pdf_detail, "permission denied")
        self.assertEqual(window.current_articles[1].pdf_local_path, "C:/pdfs/fresh.pdf")

    def test_reported_path_change_is_counted_as_a_new_file(self) -> None:
        original = replace(Article("One", pdf_status=PDF_STATUS_AVAILABLE), pdf_local_path="C:/old/one.pdf")
        window = self.make_window([original])
        replaced_path = replace(original, pdf_local_path="C:/pdfs/one.pdf")

        window._finish_download({"indices": [0], "articles": [replaced_path], "folder": "C:/pdfs"})

        self.assertIn("Saved 1 new public PDF(s)", window.status_var.get())

    def test_all_failed_reports_no_saved_file(self) -> None:
        window = self.make_window([Article("One", pdf_status=PDF_STATUS_AVAILABLE)])
        failed = replace(window.current_articles[0], pdf_status=PDF_STATUS_UNAVAILABLE)

        window._finish_download({"indices": [0], "articles": [failed], "folder": "C:/pdfs"})

        self.assertIn("No new public PDF was saved.", window.status_var.get())
        self.assertNotIn("Saved 1", window.status_var.get())


class RunDownloadTests(unittest.TestCase):
    def test_download_forwards_selection_and_reports_folder(self) -> None:
        articles = [Article("One")]
        downloaded = [replace(articles[0], pdf_local_path="C:/pdfs/one.pdf")]
        cancel = threading.Event()

        with patch(
            "google_scholar_scraper.ui.tkinter_app.download_pdfs", return_value=downloaded
        ) as download:
            result = run_download([4], articles, "C:/pdfs", cancel, lambda *args: None)

        self.assertEqual(result["indices"], [4])
        self.assertEqual(result["articles"], downloaded)
        self.assertEqual(result["folder"], "C:/pdfs")
        download.assert_called_once()
        self.assertEqual(download.call_args.args[0], articles)
        self.assertEqual(download.call_args.args[1], "C:/pdfs")
        self.assertIs(download.call_args.args[2], cancel)


class TaskWorkerTests(unittest.TestCase):
    def test_worker_reports_result_through_the_queue(self) -> None:
        messages: queue.Queue[WorkerMessage] = queue.Queue()
        worker = TaskWorker("check", lambda _cancel, _progress: {"ok": True}, messages, threading.Event())

        worker.start()
        worker.join(timeout=2)

        message = messages.get_nowait()
        self.assertEqual(message.kind, "task_result")
        self.assertEqual(message.payload, {"operation": "check", "payload": {"ok": True}})

    def test_worker_surfaces_exception_as_task_error(self) -> None:
        messages: queue.Queue[WorkerMessage] = queue.Queue()

        def boom(_cancel, _progress):
            raise RuntimeError("nope")

        worker = TaskWorker("import", boom, messages, threading.Event())
        worker.start()
        worker.join(timeout=2)

        message = messages.get_nowait()
        self.assertEqual(message.kind, "task_error")
        self.assertEqual(message.payload["operation"], "import")
        self.assertIn("RuntimeError: nope", message.payload["message"])

    def test_worker_forwards_progress_messages(self) -> None:
        messages: queue.Queue[WorkerMessage] = queue.Queue()

        def work(_cancel, progress):
            progress(1, 3, "halfway")
            return {}

        worker = TaskWorker("download", work, messages, threading.Event())
        worker.start()
        worker.join(timeout=2)

        first = messages.get_nowait()
        self.assertEqual(first.kind, "task_progress")
        self.assertEqual(first.payload, {"operation": "download", "current": 1, "total": 3, "phase": "halfway"})
        self.assertEqual(messages.get_nowait().kind, "task_result")


class DummyScreen:
    def __init__(self, width: int, height: int) -> None:
        self._width = width
        self._height = height

    def winfo_screenwidth(self) -> int:
        return self._width

    def winfo_screenheight(self) -> int:
        return self._height


class WindowSizeTests(unittest.TestCase):
    def test_default_and_minimum_fit_on_ordinary_displays(self) -> None:
        for screen in ((1920, 1080), (1366, 768), (1280, 720)):
            with self.subTest(screen=screen):
                width, height = tkinter_app._fitted_window_size(DummyScreen(*screen), *WINDOW_DEFAULT_SIZE)
                self.assertLessEqual(width, screen[0])
                self.assertLessEqual(height, screen[1])

    def test_minimum_size_is_clamped_down_on_short_displays(self) -> None:
        width, height = tkinter_app._fitted_window_size(DummyScreen(1024, 600), *WINDOW_MIN_SIZE)
        self.assertLessEqual(height, 600)
        self.assertLessEqual(width, 1024)

    def test_minimum_never_exceeds_default_on_a_large_display(self) -> None:
        minimum = tkinter_app._fitted_window_size(DummyScreen(3840, 2160), *WINDOW_MIN_SIZE)
        default = tkinter_app._fitted_window_size(DummyScreen(3840, 2160), *WINDOW_DEFAULT_SIZE)
        self.assertLessEqual(minimum[0], default[0])
        self.assertLessEqual(minimum[1], default[1])

    def test_tcl_error_falls_back_to_requested_size(self) -> None:
        class Broken:
            def winfo_screenwidth(self):
                raise tkinter_app.tk.TclError("no display")

        self.assertEqual(tkinter_app._fitted_window_size(Broken(), 900, 600), (900, 600))


def _tk_available() -> bool:
    try:
        root = tkinter_app.tk.Tk()
    except Exception:
        return False
    root.destroy()
    return True


TK_AVAILABLE = _tk_available()


@unittest.skipUnless(TK_AVAILABLE, "Tk display is unavailable in this environment")
class RealTkGeometryTests(unittest.TestCase):
    """Real-widget bounds checks: essential controls and a usable table must be on screen."""

    ESSENTIAL_WIDGETS = (
        "button_search",
        "button_cancel",
        "button_check",
        "button_download",
        "button_details",
        "combo_filter",
        "button_export_csv",
        "button_export_excel",
        "button_import",
        "button_browse",
        "combo_year",
        "entry_year_from",
        "entry_year_to",
        "results_table",
    )

    def setUp(self) -> None:
        self.window = MainWindow()
        self.window.current_articles = [
            Article("Alpha paper", link="https://example.org/a", pdf_status=PDF_STATUS_AVAILABLE),
            Article("Beta paper", link="https://example.org/b", pdf_status=PDF_STATUS_UNAVAILABLE),
        ]
        self.window._render_results()

    def tearDown(self) -> None:
        try:
            self.window.on_close()
        except Exception:
            pass

    def _settle(self, geometry: str) -> tuple[int, int, int, int]:
        self.window.window.geometry(geometry)
        self.window.window.update_idletasks()
        self.window.window.update()
        window = self.window.window
        return (
            window.winfo_rootx(),
            window.winfo_rooty(),
            window.winfo_width(),
            window.winfo_height(),
        )

    def test_essential_controls_and_table_visible_at_supported_sizes(self) -> None:
        for geometry in ("860x560", "880x620", "1080x700"):
            with self.subTest(geometry=geometry):
                left, top, width, height = self._settle(geometry)
                for name in self.ESSENTIAL_WIDGETS:
                    widget = getattr(self.window, name)
                    x = widget.winfo_rootx() - left
                    y = widget.winfo_rooty() - top
                    self.assertGreaterEqual(x, 0, f"{name} starts left of the window")
                    self.assertGreaterEqual(y, 0, f"{name} starts above the window")
                    self.assertLessEqual(
                        x + widget.winfo_width(), width, f"{name} is clipped on the right at {geometry}"
                    )
                    self.assertLessEqual(
                        y + widget.winfo_height(), height, f"{name} is clipped at the bottom at {geometry}"
                    )

    def test_results_table_keeps_usable_height(self) -> None:
        for geometry in ("860x560", "880x620", "1080x700"):
            with self.subTest(geometry=geometry):
                self._settle(geometry)
                self.assertGreaterEqual(
                    self.window.results_table.winfo_height(),
                    tkinter_app.TABLE_MIN_HEIGHT,
                    f"results table is unusably short at {geometry}",
                )

    def test_input_fields_expand_without_squeezing(self) -> None:
        for geometry in ("860x560", "880x620", "1080x700"):
            with self.subTest(geometry=geometry):
                self._settle(geometry)
                self.assertGreaterEqual(self.window.entry_query.winfo_width(), 300)
                self.assertGreaterEqual(self.window.entry_ranking_path.winfo_width(), 300)
                self.assertGreaterEqual(
                    self.window.combo_filter.winfo_width(), self.window.combo_filter.winfo_reqwidth()
                )
        self._settle("860x560")
        narrow = self.window.entry_query.winfo_width()
        self._settle("1080x700")
        self.assertGreater(self.window.entry_query.winfo_width(), narrow)

    def test_scrollbars_are_inside_the_minimum_window(self) -> None:
        left, top, width, height = self._settle("860x560")
        scrollbars = self.window.table_container.winfo_children()[1:]
        self.assertEqual(len(scrollbars), 2)
        for widget in scrollbars:
            self.assertTrue(widget.winfo_ismapped())
            self.assertLessEqual(widget.winfo_rootx() - left + widget.winfo_width(), width)
            self.assertLessEqual(widget.winfo_rooty() - top + widget.winfo_height(), height)

    def test_column_headings_fit_at_the_minimum_width(self) -> None:
        self._settle("860x560")
        for column in self.window.results_table.cget("columns"):
            text = self.window.results_table.heading(column, "text")
            width = self.window.results_table.column(column, "width")
            self.assertLessEqual(self.window.table_heading_font.measure(text) + 12, width)

    def test_table_sits_below_the_results_toolbar(self) -> None:
        _, top, _, _ = self._settle("1080x700")
        toolbar_bottom = self.window.button_check.winfo_rooty() - top + self.window.button_check.winfo_height()
        table_top = self.window.results_table.winfo_rooty() - top
        self.assertGreaterEqual(table_top, toolbar_bottom)

    def test_search_button_and_cancel_share_the_header_row(self) -> None:
        _, top, _, _ = self._settle("860x560")
        search_y = self.window.button_search.winfo_rooty() - top
        cancel_y = self.window.button_cancel.winfo_rooty() - top
        self.assertLessEqual(abs(search_y - cancel_y), 8)

    def test_layout_survives_no_articles_state(self) -> None:
        self.window._clear_results()
        self.window._show_empty_state("No results yet", "Run a search to see articles.")
        _, top, _, height = self._settle("860x560")
        for name in ("button_check", "combo_filter", "button_search"):
            widget = getattr(self.window, name)
            y = widget.winfo_rooty() - top
            self.assertLessEqual(y + widget.winfo_height(), height, f"{name} clipped in empty state")


class YearFilterUITests(unittest.TestCase):
    def test_dynamic_presets_and_default(self) -> None:
        self.assertEqual(tkinter_app.year_filter_options(2030), {
            "Any time": (None, None), "Since 2030": (2030, None),
            "Since 2029": (2029, None), "Last five years": (2026, None),
            "Custom range": (None, None),
        })
        from datetime import date
        self.assertIn(f"Since {date.today().year}", tkinter_app.year_filter_options())
        self.assertEqual(SearchRequest("query", 1, True, "").year_from, None)

    def test_parse_custom_bounds_without_affecting_other_inputs(self) -> None:
        for lower, upper, expected in (
            (" 2018 ", "2024", (2018, 2024)), ("2024", "", (2024, None)),
            ("", "2024", (None, 2024)), ("", "", (None, None)),
            ("9999", "9999", (9999, 9999)),
        ):
            with self.subTest(lower=lower, upper=upper):
                request = tkinter_app.validate_search_inputs(' "SOMLAP" ', "2", False, " out ", year_from=lower, year_to=upper)
                self.assertEqual((request.year_from, request.year_to), expected)
                self.assertEqual((request.query, request.pages, request.ranking_enabled, request.output_folder), ('"SOMLAP"', 2, False, "out"))

    def test_rejects_malformed_reversed_and_out_of_range_years(self) -> None:
        for lower, upper in (("x", ""), ("2024.0", ""), ("+2024", ""), ("٢٠٢٤", ""), ("999", ""), ("0999", ""), ("", "10000"), ("2025", "2024")):
            with self.subTest(lower=lower, upper=upper):
                with self.assertRaises(ValueError):
                    tkinter_app.validate_search_inputs("query", "1", True, year_from=lower, year_to=upper)

    def test_preset_switch_clears_stale_bounds_and_restores_custom_states(self) -> None:
        window = state_window()
        window.year_from_var.set("2018")
        window.year_to_var.set("2020")
        window._on_year_preset_changed()
        self.assertEqual((window.year_from_var.get(), window.year_to_var.get()), ("", ""))
        self.assertEqual(window.entry_year_from.state, tkinter_app.tk.DISABLED)
        window.year_preset_var.set("Last five years")
        window._on_year_preset_changed()
        self.assertEqual(window.year_from_var.get(), "2026")
        window.year_preset_var.set("Custom range")
        window._on_year_preset_changed()
        self.assertEqual(window.entry_year_from.state, tkinter_app.tk.NORMAL)
        window._set_running_state()
        self.assertEqual(window.combo_year.state, tkinter_app.tk.DISABLED)
        self.assertEqual(window.entry_year_from.state, tkinter_app.tk.DISABLED)
        window.operation = None
        window._set_idle_state()
        self.assertEqual(window.combo_year.state, "readonly")
        self.assertEqual(window.entry_year_from.state, tkinter_app.tk.NORMAL)

    def test_invalid_year_does_not_start_worker_or_clear_results(self) -> None:
        window = state_window()
        window.current_articles = [Article("Existing")]
        window.year_preset_var.set("Custom range")
        window.year_from_var.set("invalid")
        with patch.object(tkinter_app, "SearchWorker") as worker:
            window.start_search()
        worker.assert_not_called()
        self.assertEqual(window.current_articles, [Article("Existing")])
        self.assertEqual(window.status_heading_var.get(), "Check search setup")

    def test_worker_forwards_immutable_year_snapshot(self) -> None:
        received = {}
        def fake_scrape(query, pages, **kwargs):
            received.update(kwargs)
            return ExtractionResult(ExtractionStatus.SUCCESS, [])
        request = SearchRequest("query", 2, False, "", 2018, 2024)
        messages = queue.Queue()
        SearchWorker(request, messages, threading.Event(), fake_scrape).run()
        self.assertEqual((received["year_from"], received["year_to"]), (2018, 2024))
        self.assertFalse(received["ranking_enabled"])
        self.assertEqual(messages.get_nowait().kind, "result")

    def test_result_label_uses_completed_request_not_changed_controls(self) -> None:
        window = state_window()
        window.current_request = SearchRequest("query", 1, True, "", 2018, 2024)
        window.year_preset_var.set("Any time")
        window.year_from_var.set("")
        window._handle_result(ExtractionResult(ExtractionStatus.SUCCESS, [Article("Paper")], requested_pages=1, successful_pages=1))
        self.assertEqual(window.applied_year_var.get(), "Years: 2018–2024")
        self.assertEqual(window.summary_base, result_summary(ExtractionResult(ExtractionStatus.SUCCESS, [Article("Paper")], requested_pages=1, successful_pages=1)))

    def test_labels_for_open_and_unbounded_ranges(self) -> None:
        self.assertEqual(tkinter_app.year_range_label(None, None), "Years: any time")
        self.assertEqual(tkinter_app.year_range_label(2024, None), "Years: since 2024")
        self.assertEqual(tkinter_app.year_range_label(None, 2024), "Years: through 2024")


if __name__ == "__main__":
    unittest.main()
