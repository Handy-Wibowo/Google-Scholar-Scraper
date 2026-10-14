from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import copy
import queue
import threading
import webbrowser
import tkinter as tk
from tkinter import filedialog, font as tkfont, messagebox, ttk

from google_scholar_scraper import __version__
from google_scholar_scraper.enrichment import enrich_articles
from google_scholar_scraper.exporters import save_to_csv, save_to_excel
from google_scholar_scraper.journal_catalog import JournalCatalog, apply_rank, catalog_matches
from google_scholar_scraper.models import (
    PDF_STATUS_AVAILABLE,
    PDF_STATUS_UNAVAILABLE,
    PDF_STATUS_UNKNOWN,
    Article,
    ExtractionResult,
    ExtractionStatus,
)
from google_scholar_scraper.pdf_service import check_pdf_availability, download_pdfs
from google_scholar_scraper.scraper.client import scrape_scholar


DEFAULT_EXCEL_FILENAME = "scholar_articles.xlsx"
DEFAULT_CSV_FILENAME = "scholar_articles.csv"
APP_TITLE = "Google Scholar Scraper"

# PDF filter labels map one-to-one onto the three stored states, so the filter can never
# invent a fourth meaning for a record.
PDF_FILTER_ALL = "All"
PDF_FILTER_AVAILABLE = "Public PDF available"
PDF_FILTER_UNAVAILABLE = "No public PDF found"
PDF_FILTER_UNKNOWN = "Unknown"
PDF_FILTERS = (PDF_FILTER_ALL, PDF_FILTER_AVAILABLE, PDF_FILTER_UNAVAILABLE, PDF_FILTER_UNKNOWN)
_PDF_FILTER_STATUS = {
    PDF_FILTER_AVAILABLE: PDF_STATUS_AVAILABLE,
    PDF_FILTER_UNAVAILABLE: PDF_STATUS_UNAVAILABLE,
    PDF_FILTER_UNKNOWN: PDF_STATUS_UNKNOWN,
}

_IID_PREFIX = "article-"

# Layout budget: the results table keeps a usable height at the minimum supported window, and
# window geometry is clamped to the real display so the default never lands off-screen.
WINDOW_DEFAULT_SIZE = (1080, 700)
WINDOW_MIN_SIZE = (860, 560)
WINDOW_WORK_AREA_MARGIN = (20, 80)
TABLE_MIN_HEIGHT = 100
# The table requests a bounded height instead of a full min-height, so it can never push its own
# scrollbars or the action row past the bottom edge of the smallest supported window.
TOOLBAR_HEIGHT = 40
# At 125% Windows scaling a table row is about 25 real pixels; 4 rows fit the bounded budget.
TABLE_ROW_COUNT = 4

OPERATION_HEADINGS = {
    "search": "Searching Google Scholar",
    "check": "Checking metadata and public PDFs",
    "import": "Importing journal ranking records",
    "download": "Downloading selected public PDFs",
}

NO_RANKING_NOTICE = "No ranking data — import CSV/XLSX."

DETAIL_DISCLAIMER = (
    "Journal labels (SJR quartile, SINTA rank) come from the ranking file you imported. They describe "
    "the journal only: they are not proof of current Scopus indexing and not a paper-quality score. "
    "\"unknown\" means no verified record or no check has been made, never that a journal is unranked or "
    "unaccredited. Public-PDF status stays \"unknown\" until a check actually validates a link, and \"No "
    "public PDF\" only means the checked public sources found none."
)

COLORS = {
    "app_bg": "#f3f6fb",
    "surface": "#ffffff",
    "surface_alt": "#f8fafc",
    "text": "#111827",
    "muted": "#5b677a",
    "border": "#d8e0ea",
    "border_strong": "#b9c6d6",
    "primary": "#185abc",
    "primary_hover": "#1f6feb",
    "secondary_hover": "#f4f7fb",
    "disabled": "#a6afbd",
    "selection": "#dbeafe",
}

STATUS_COLORS = {
    "ready": {"bg": "#f8fafc", "border": "#d8e0ea", "accent": "#64748b", "heading": "#1f2937", "detail": "#5b677a"},
    "running": {"bg": "#f5f9ff", "border": "#c5dcfb", "accent": "#185abc", "heading": "#174ea6", "detail": "#355f96"},
    "success": {"bg": "#f4fbf7", "border": "#bde7cd", "accent": "#16803c", "heading": "#166534", "detail": "#366b49"},
    "warning": {"bg": "#fffaf0", "border": "#edd79a", "accent": "#b7791f", "heading": "#8a5a00", "detail": "#725a18"},
    "error": {"bg": "#fff5f6", "border": "#f2bdc5", "accent": "#c0264d", "heading": "#9f1239", "detail": "#7f1d1d"},
}


@dataclass(frozen=True)
class SearchRequest:
    query: str
    pages: int
    ranking_enabled: bool
    output_folder: str = ""


@dataclass(frozen=True)
class WorkerMessage:
    kind: str
    payload: object


def validate_search_inputs(query: str, pages: str, ranking_enabled: bool, output_folder: str = "") -> SearchRequest:
    clean_query = query.strip()
    if not clean_query:
        raise ValueError("Enter a search query.")

    try:
        page_count = int(pages.strip())
    except ValueError as exc:
        raise ValueError("Page count must be a whole number.") from exc

    if page_count <= 0:
        raise ValueError("Page count must be greater than zero.")

    return SearchRequest(
        query=clean_query,
        pages=page_count,
        ranking_enabled=ranking_enabled,
        output_folder=output_folder.strip(),
    )


def export_path(output_folder: str, filename: str = DEFAULT_EXCEL_FILENAME) -> Path:
    if output_folder:
        return Path(output_folder) / filename
    return Path(filename)


def _fitted_window_size(window: tk.Misc, width: int, height: int) -> tuple[int, int]:
    """Clamp a requested window size to the real display so the app never opens off-screen."""
    try:
        screen_width = window.winfo_screenwidth()
        screen_height = window.winfo_screenheight()
    except tk.TclError:
        return width, height
    if screen_width <= 1 or screen_height <= 1:
        return width, height
    margin_width, margin_height = WINDOW_WORK_AREA_MARGIN
    return max(1, min(width, screen_width - margin_width)), max(1, min(height, screen_height - margin_height))


def status_message(result: ExtractionResult) -> str:
    messages = {
        ExtractionStatus.SUCCESS: result.message or "Search completed successfully.",
        ExtractionStatus.PARTIAL_SUCCESS: result.message or "Search stopped early. Collected results were preserved.",
        ExtractionStatus.NO_RESULTS: result.message or "No results found.",
        ExtractionStatus.RATE_LIMITED: result.message or "Google Scholar temporarily limited requests.",
        ExtractionStatus.BLOCKED: result.message or "Google Scholar returned a blocked or challenge page.",
        ExtractionStatus.NETWORK_ERROR: result.message or "Network request failed.",
        ExtractionStatus.PARSING_ERROR: result.message or "The response could not be interpreted safely.",
        ExtractionStatus.CANCELLED: result.message or "Search was cancelled.",
    }
    return messages[result.status]


def status_heading(result: ExtractionResult) -> str:
    headings = {
        ExtractionStatus.SUCCESS: "Search complete",
        ExtractionStatus.PARTIAL_SUCCESS: "Search stopped early",
        ExtractionStatus.NO_RESULTS: "No results found",
        ExtractionStatus.RATE_LIMITED: "Google Scholar temporarily limited requests",
        ExtractionStatus.BLOCKED: "Google Scholar returned a challenge page",
        ExtractionStatus.NETWORK_ERROR: "Network request failed",
        ExtractionStatus.PARSING_ERROR: "Results could not be parsed safely",
        ExtractionStatus.CANCELLED: "Search cancelled",
    }
    return headings[result.status]


def status_tone(result: ExtractionResult) -> str:
    if result.status == ExtractionStatus.SUCCESS:
        return "success"
    if result.status in {ExtractionStatus.PARTIAL_SUCCESS, ExtractionStatus.CANCELLED, ExtractionStatus.RATE_LIMITED}:
        return "warning"
    if result.status in {
        ExtractionStatus.BLOCKED,
        ExtractionStatus.NETWORK_ERROR,
        ExtractionStatus.PARSING_ERROR,
        ExtractionStatus.NO_RESULTS,
    }:
        return "error"
    return "ready"


def result_summary(result: ExtractionResult) -> str:
    result_word = "result" if len(result.articles) == 1 else "results"
    parts = [
        f"{len(result.articles)} {result_word} found",
        f"{result.successful_pages}/{result.requested_pages} pages completed",
    ]
    if result.duplicates_removed:
        parts.append(f"{result.duplicates_removed} duplicate removed" + ("" if result.duplicates_removed == 1 else "s"))
    if result.invalid_articles_removed:
        parts.append(
            f"{result.invalid_articles_removed} invalid record removed"
            + ("" if result.invalid_articles_removed == 1 else "s")
        )
    return " | ".join(parts)


def progress_value(current_page: int, total_pages: int) -> int:
    if total_pages <= 0:
        return 0
    return max(0, min(100, int((current_page / total_pages) * 100)))


def article_row(article: Article) -> tuple[str, str, str, str]:
    score = "" if article.relevance_score is None else f"{article.relevance_score:.1f}"
    link = "Open link" if article.link else "No link"
    return (article.title, article.authors, score, link)


def journal_rank_label(article: Article) -> str:
    """Compact journal-level label; empty when no verified record matched."""
    return " / ".join(value for value in (article.sjr_quartile, article.sinta_rank) if value)


def pdf_status_label(article: Article) -> str:
    if article.pdf_status == PDF_STATUS_AVAILABLE:
        return "Downloaded" if article.pdf_local_path else "Public PDF"
    if article.pdf_status == PDF_STATUS_UNAVAILABLE:
        return "No public PDF"
    return "Unknown"


def article_table_row(article: Article) -> tuple[str, str, str, str, str, str]:
    title, authors, score, link = article_row(article)
    return (title, authors, score, journal_rank_label(article), pdf_status_label(article), link)


def article_iid(index: int) -> str:
    return f"{_IID_PREFIX}{index}"


def filtered_articles(articles: list[Article], pdf_filter: str) -> list[tuple[int, Article]]:
    """Return (stored index, article) pairs for the current filter.

    The index is the position in the stored result list, so a row keeps its identity when
    filtering changes which rows are visible. Filtering never edits or reorders records.
    """
    status = _PDF_FILTER_STATUS.get(pdf_filter)
    if status is None:
        return list(enumerate(articles))
    return [(index, article) for index, article in enumerate(articles) if article.pdf_status == status]


def article_detail_text(article: Article | None) -> str:
    if article is None:
        return "Select a paper to see its journal, ranking and public-PDF details.\n\n" + DETAIL_DISCLAIMER

    publication_year = "unknown" if article.publication_year is None else str(article.publication_year)
    sjr_year = "unknown" if article.sjr_year is None else str(article.sjr_year)
    sinta_year = "unknown" if article.sinta_year is None else str(article.sinta_year)
    lines = [
        article.title or "(untitled)",
        f"Authors: {article.authors or 'unknown'}",
        f"Link: {article.link or 'none'}",
        f"DOI: {article.doi or 'unknown'}",
        f"Journal: {article.journal or 'unknown'}",
        f"ISSN: {', '.join(article.issns) if article.issns else 'unknown'}",
        f"Publication year: {publication_year}",
        "",
        f"SJR quartile: {article.sjr_quartile or 'unknown'}   (ranking year: {sjr_year})",
        f"SJR category: {article.sjr_category or 'unknown'}",
        f"SJR source: {article.sjr_source_url or 'no imported record'}",
        f"SINTA rank: {article.sinta_rank or 'unknown'}   (ranking year: {sinta_year})",
        f"SINTA source: {article.sinta_source_url or 'no imported record'}",
        "",
        f"PDF status: {article.pdf_status}",
        f"PDF detail: {article.pdf_detail or 'not checked'}",
        f"PDF source: {article.pdf_source_url or 'none'}",
        f"PDF local path: {article.pdf_local_path or 'not downloaded'}",
        "",
        DETAIL_DISCLAIMER,
    ]
    return "\n".join(lines)


def export_error_message(exc: OSError) -> str:
    detail = str(exc).strip() or exc.__class__.__name__
    return f"Export failed: {detail}"


def run_check(articles: list[Article], catalog: JournalCatalog | None, cancel_event: threading.Event, progress) -> dict:
    """Enrich metadata, then validate public PDFs for each record.

    Runs off the Tk thread. Every input record comes back: a metadata lookup failure keeps
    the original records and still runs the PDF check, and a PDF check failure keeps
    whatever enrichment already succeeded.
    """
    total = len(articles)
    enrich_error = ""

    def enrich_progress(current: int, _count: int, label: str) -> None:
        progress(current, total * 2, f"metadata: {label}")

    try:
        enriched = enrich_articles(articles, catalog, cancel_event, enrich_progress)
    except Exception as exc:  # provider failure must not discard collected records
        enriched = list(articles)
        enrich_error = f"Metadata lookup failed ({exc.__class__.__name__}); original records were kept."

    checked: list[Article] = []
    for index, article in enumerate(enriched):
        if cancel_event.is_set():
            checked.extend(enriched[index:])
            break
        try:
            checked.append(check_pdf_availability(article, cancel_event))
        except Exception:
            checked.append(article)
        progress(total + index + 1, total * 2, f"PDF: {article.title or 'record'}")

    return {"articles": checked, "enrich_error": enrich_error, "cancelled": cancel_event.is_set()}


def run_import(path: str, default_year: int | None, progress, existing: JournalCatalog | None = None) -> dict:
    """Parse a ranking file into a catalogue staged on top of the already-loaded records.

    Imports accumulate: an SJR file and a SINTA file imported one after another leave both
    label sets available, and a newer year of the same source is added alongside the older
    one. The staging clone is independent of the window's catalogue — it is never mutated
    here — so a rejected file leaves the loaded records completely untouched.
    """
    progress(0, 1, f"reading {Path(path).name}")
    catalog = copy.deepcopy(existing) if existing is not None else JournalCatalog()
    count = catalog.import_file(path, default_year)
    progress(1, 1, f"{count} records read")
    return {"catalog": catalog, "count": count, "total": len(catalog), "path": path, "year": default_year}


def run_download(
    indices: list[int], articles: list[Article], folder: str, cancel_event: threading.Event, progress
) -> dict:
    """Download the selected records; input order is preserved, failures included."""
    results = download_pdfs(articles, folder, cancel_event, progress)
    return {"indices": list(indices), "articles": results, "folder": folder}


class SearchWorker(threading.Thread):
    def __init__(
        self,
        request: SearchRequest,
        message_queue: queue.Queue[WorkerMessage],
        cancel_event: threading.Event,
        scrape_func=scrape_scholar,
    ) -> None:
        super().__init__(daemon=True)
        self.request = request
        self.message_queue = message_queue
        self.cancel_event = cancel_event
        self.scrape_func = scrape_func

    def run(self) -> None:
        try:
            result = self.scrape_func(
                self.request.query,
                self.request.pages,
                ranking_enabled=self.request.ranking_enabled,
                progress_callback=self._progress,
                cancel_event=self.cancel_event,
            )
        except Exception as exc:  # defensive UI boundary: do not surface tracebacks in normal UI
            result = ExtractionResult(
                status=ExtractionStatus.NETWORK_ERROR,
                articles=[],
                requested_pages=self.request.pages,
                message=f"Search failed: {exc.__class__.__name__}.",
                diagnostic=exc.__class__.__name__,
            )
        self.message_queue.put(WorkerMessage("result", result))

    def _progress(self, current_page: int, total_pages: int, phase: str) -> None:
        self.message_queue.put(
            WorkerMessage(
                "progress",
                {
                    "current_page": current_page,
                    "total_pages": total_pages,
                    "phase": phase,
                },
            )
        )


class TaskWorker(threading.Thread):
    """Runs one background job (check / import / download) and reports through the queue."""

    def __init__(
        self, operation: str, work, message_queue: queue.Queue[WorkerMessage], cancel_event: threading.Event
    ) -> None:
        super().__init__(daemon=True)
        self.operation = operation
        self.work = work
        self.message_queue = message_queue
        self.cancel_event = cancel_event

    def run(self) -> None:
        try:
            payload = self.work(self.cancel_event, self._progress)
        except Exception as exc:  # defensive UI boundary: report instead of raising into Tk
            self.message_queue.put(
                WorkerMessage(
                    "task_error", {"operation": self.operation, "message": f"{exc.__class__.__name__}: {exc}"}
                )
            )
            return
        self.message_queue.put(WorkerMessage("task_result", {"operation": self.operation, "payload": payload}))

    def _progress(self, current: int, total: int, phase: str) -> None:
        self.message_queue.put(
            WorkerMessage(
                "task_progress",
                {"operation": self.operation, "current": current, "total": total, "phase": phase},
            )
        )


class MainWindow:
    def __init__(self) -> None:
        self.window = tk.Tk()
        self.window.title(f"{APP_TITLE} v{__version__}")
        default_width, default_height = _fitted_window_size(self.window, *WINDOW_DEFAULT_SIZE)
        self.window.geometry(f"{default_width}x{default_height}")
        min_width, min_height = _fitted_window_size(self.window, *WINDOW_MIN_SIZE)
        self.window.minsize(min_width, min_height)
        self.window.configure(bg=COLORS["app_bg"])
        self._configure_style()

        self.message_queue: queue.Queue[WorkerMessage] = queue.Queue()
        self.cancel_event: threading.Event | None = None
        self.worker: SearchWorker | TaskWorker | None = None
        self.operation: str | None = None
        self.current_articles: list[Article] = []
        self.current_request: SearchRequest | None = None
        self.catalog = JournalCatalog()
        self.summary_base = "No search has run yet."
        self._closed = False
        self.detail_window: tk.Toplevel | None = None
        self.detail_body: tk.Text | None = None

        self.query_var = tk.StringVar()
        self.pages_var = tk.StringVar(value="1")
        self.output_folder_var = tk.StringVar()
        self.ranking_var = tk.BooleanVar(value=True)
        self.ranking_path_var = tk.StringVar()
        self.ranking_year_var = tk.StringVar()
        self.ranking_notice_var = tk.StringVar(value=NO_RANKING_NOTICE)
        self.pdf_filter_var = tk.StringVar(value=PDF_FILTER_ALL)
        self.status_heading_var = tk.StringVar(value="Ready")
        self.status_var = tk.StringVar(value="Enter a query, choose pages, then start a search.")
        self.summary_var = tk.StringVar(value="No search has run yet.")
        self.empty_title_var = tk.StringVar(value="No results yet")
        self.empty_detail_var = tk.StringVar(value="Run a search to review Google Scholar results here.")
        self.progress_var = tk.IntVar(value=0)

        self._build_ui()
        self._set_idle_state()
        self._set_status_tone("ready")
        self.entry_query.focus_set()
        self.window.bind("<Return>", self._handle_enter)
        self.window.bind("<Escape>", self._handle_escape)
        self.window.protocol("WM_DELETE_WINDOW", self.on_close)

    def _configure_style(self) -> None:
        self.default_font = tkfont.nametofont("TkDefaultFont")
        try:
            self.default_font.configure(family="Segoe UI", size=10)
        except tk.TclError:
            self.default_font.configure(size=10)
        font_family = self.default_font.actual("family")
        self.heading_font = tkfont.Font(family=font_family, size=16, weight="bold")
        self.section_font = tkfont.Font(family=self.default_font.actual("family"), size=11, weight="bold")
        self.status_font = tkfont.Font(family=font_family, size=10, weight="bold")
        self.table_font = tkfont.Font(family=font_family, size=10)
        self.table_heading_font = tkfont.Font(family=font_family, size=10, weight="bold")

        self.style = ttk.Style(self.window)
        try:
            self.style.theme_use("clam")
        except tk.TclError:
            pass
        self.style.configure("App.TFrame", background=COLORS["app_bg"])
        self.style.configure("Header.TFrame", background=COLORS["app_bg"])
        self.style.configure("Title.TLabel", background=COLORS["app_bg"], foreground=COLORS["text"], font=self.heading_font)
        self.style.configure("Muted.TLabel", background=COLORS["app_bg"], foreground=COLORS["muted"])
        self.style.configure("Surface.TLabel", background=COLORS["surface"], foreground=COLORS["text"])
        self.style.configure("SurfaceMuted.TLabel", background=COLORS["surface"], foreground=COLORS["muted"])
        self.style.configure("SectionTitle.TLabel", background=COLORS["surface"], foreground=COLORS["text"], font=self.section_font)
        self.style.configure("TEntry", fieldbackground="#ffffff", bordercolor=COLORS["border_strong"], lightcolor=COLORS["border"], darkcolor=COLORS["border"])
        self.style.map("TEntry", bordercolor=[("focus", COLORS["primary"])])
        self.style.configure("Horizontal.TProgressbar", troughcolor="#edf2f7", background=COLORS["primary"], bordercolor="#edf2f7", lightcolor=COLORS["primary"], darkcolor=COLORS["primary"])
        self.style.configure(
            "Treeview",
            background="#ffffff",
            fieldbackground="#ffffff",
            foreground=COLORS["text"],
            rowheight=30,
            borderwidth=0,
            relief="flat",
            font=self.table_font,
        )
        self.style.map(
            "Treeview",
            background=[("selected", COLORS["selection"])],
            foreground=[("selected", COLORS["text"])],
        )
        self.style.configure(
            "Treeview.Heading",
            background="#eef3f8",
            foreground=COLORS["text"],
            bordercolor=COLORS["border"],
            relief="flat",
            font=self.table_heading_font,
        )

    def _surface(self, parent, *, border: str | None = None) -> tk.Frame:
        return tk.Frame(
            parent,
            bg=COLORS["surface"],
            bd=0,
            highlightthickness=1 if border else 0,
            highlightbackground=border or COLORS["surface"],
            highlightcolor=border or COLORS["surface"],
        )

    def _button(self, parent, *, text: str, command, kind: str = "secondary", width: int | None = None) -> tk.Button:
        is_primary = kind == "primary"
        button = tk.Button(
            parent,
            text=text,
            command=command,
            width=width or (16 if is_primary else 12),
            padx=12,
            pady=3,
            bd=0,
            relief=tk.FLAT,
            cursor="hand2",
            font=(self.default_font.actual("family"), 10, "bold" if is_primary else "normal"),
            bg=COLORS["primary"] if is_primary else COLORS["surface_alt"],
            fg="#ffffff" if is_primary else COLORS["text"],
            activebackground=COLORS["primary_hover"] if is_primary else COLORS["secondary_hover"],
            activeforeground="#ffffff" if is_primary else COLORS["text"],
            disabledforeground="#edf4ff" if is_primary else COLORS["disabled"],
            highlightthickness=1,
            highlightbackground=COLORS["primary"] if is_primary else COLORS["border_strong"],
            highlightcolor=COLORS["primary"] if is_primary else COLORS["border_strong"],
        )
        return button

    def _build_ui(self) -> None:
        self.window.columnconfigure(0, weight=1)
        self.window.rowconfigure(4, weight=1, minsize=TABLE_MIN_HEIGHT + TOOLBAR_HEIGHT * 2)

        header_frame = ttk.Frame(self.window, style="Header.TFrame")
        header_frame.grid(row=0, column=0, sticky="ew", padx=18, pady=(10, 6))
        header_frame.columnconfigure(1, weight=1)
        ttk.Label(header_frame, text=f"{APP_TITLE} v{__version__}", style="Title.TLabel").grid(row=0, column=0, sticky="w")
        ttk.Label(
            header_frame,
            text="Search, rank, review, and export Scholar results.",
            style="Muted.TLabel",
        ).grid(row=0, column=1, sticky="w", padx=(10, 0))

        self.button_cancel = self._button(header_frame, text="Cancel", command=self.cancel_search, width=10)
        self.button_cancel.grid(row=0, column=2, padx=(10, 6))
        self.button_search = self._button(header_frame, text="Search Scholar", command=self.start_search, kind="primary", width=16)
        self.button_search.grid(row=0, column=3)

        search_frame = self._surface(self.window, border=COLORS["border"])
        search_frame.grid(row=1, column=0, sticky="ew", padx=18, pady=(0, 6))
        self.search_frame = search_frame
        search_frame.columnconfigure(0, weight=1)

        query_row = self._surface(search_frame)
        query_row.grid(row=0, column=0, sticky="ew", padx=(14, 14), pady=2)
        query_row.columnconfigure(1, weight=1)
        ttk.Label(query_row, text="Query", style="Surface.TLabel").grid(row=0, column=0, sticky="w", padx=(0, 10))
        self.entry_query = ttk.Entry(query_row, textvariable=self.query_var)
        self.entry_query.grid(row=0, column=1, sticky="ew", ipady=2)
        ttk.Label(query_row, text="Pages", style="Surface.TLabel").grid(row=0, column=2, sticky="e", padx=(10, 6))
        self.entry_pages = ttk.Entry(query_row, textvariable=self.pages_var, width=6)
        self.entry_pages.grid(row=0, column=3, sticky="e", ipady=2)

        folder_row = self._surface(search_frame)
        folder_row.grid(row=1, column=0, sticky="ew", padx=(14, 14), pady=2)
        folder_row.columnconfigure(1, weight=1)
        ttk.Label(folder_row, text="Export folder", style="Surface.TLabel").grid(row=0, column=0, sticky="w", padx=(0, 10))
        self.entry_folder = ttk.Entry(folder_row, textvariable=self.output_folder_var)
        self.entry_folder.grid(row=0, column=1, sticky="ew", padx=(0, 10), ipady=2)
        self.button_browse = self._button(folder_row, text="Browse", command=self.browse_folder, width=11)
        self.button_browse.grid(row=0, column=2, sticky="e")

        ranking_row = self._surface(search_frame)
        ranking_row.grid(row=2, column=0, sticky="ew", padx=(14, 14), pady=2)
        ranking_row.columnconfigure(1, weight=1)
        ttk.Label(ranking_row, text="Ranking file", style="Surface.TLabel").grid(row=0, column=0, sticky="w", padx=(0, 10))
        self.entry_ranking_path = ttk.Entry(ranking_row, textvariable=self.ranking_path_var)
        self.entry_ranking_path.grid(row=0, column=1, sticky="ew", padx=(0, 10), ipady=2)
        self.label_ranking_year = ttk.Label(ranking_row, text="Rank year", style="Surface.TLabel")
        self.label_ranking_year.grid(row=0, column=2, sticky="e", padx=(0, 6))
        self.entry_ranking_year = ttk.Entry(ranking_row, textvariable=self.ranking_year_var, width=6)
        self.entry_ranking_year.grid(row=0, column=3, sticky="e", padx=(0, 10), ipady=2)
        self.button_import = self._button(ranking_row, text="Import", command=self.import_rankings, width=9)
        self.button_import.grid(row=0, column=4, sticky="e")

        options_row = self._surface(search_frame)
        options_row.grid(row=3, column=0, sticky="ew", padx=14, pady=(0, 6))
        options_row.columnconfigure(1, weight=1)
        self.check_ranking = tk.Checkbutton(
            options_row,
            text="Rank by relevance (local scoring)",
            variable=self.ranking_var,
            bg=COLORS["surface"],
            fg=COLORS["text"],
            activebackground=COLORS["surface"],
            activeforeground=COLORS["text"],
            selectcolor="#ffffff",
            bd=0,
            highlightthickness=0,
            font=(self.default_font.actual("family"), 10),
        )
        self.check_ranking.grid(row=0, column=0, sticky="w", padx=(0, 12))
        self.label_ranking_notice = ttk.Label(
            options_row, textvariable=self.ranking_notice_var, style="SurfaceMuted.TLabel"
        )
        self.label_ranking_notice.grid(row=0, column=1, sticky="ew")

        self.status_frame = self._surface(self.window, border=COLORS["border"])
        self.status_frame.grid(row=3, column=0, sticky="ew", padx=18, pady=(0, 6))
        self.status_frame.columnconfigure(1, weight=1)
        self.status_accent = tk.Frame(self.status_frame, width=4, bg=STATUS_COLORS["ready"]["accent"])
        self.status_accent.grid(row=0, column=0, rowspan=3, sticky="nsw")

        self.status_heading_label = tk.Label(
            self.status_frame,
            textvariable=self.status_heading_var,
            anchor="w",
            font=self.status_font,
        )
        self.status_heading_label.grid(row=0, column=1, sticky="ew", padx=14, pady=(8, 2))
        self.status_detail_label = tk.Label(self.status_frame, textvariable=self.status_var, anchor="w", justify="left")
        self.status_detail_label.grid(row=1, column=1, sticky="ew", padx=14, pady=(0, 5))
        self.progress_bar = ttk.Progressbar(self.status_frame, variable=self.progress_var, maximum=100)
        self.progress_bar.grid(row=2, column=1, sticky="ew", padx=14, pady=(0, 9))

        results_frame = ttk.Frame(self.window, style="App.TFrame")
        results_frame.grid(row=4, column=0, sticky="nsew", padx=18, pady=(0, 14))
        results_frame.columnconfigure(0, weight=1)
        results_frame.rowconfigure(1, weight=1)

        results_toolbar = ttk.Frame(results_frame, style="App.TFrame")
        results_toolbar.grid(row=0, column=0, sticky="ew", pady=(0, 5))
        results_toolbar.columnconfigure(1, weight=1)

        ttk.Label(results_toolbar, text="Results", style="Title.TLabel").grid(row=0, column=0, sticky="w")
        ttk.Label(results_toolbar, textvariable=self.summary_var, style="Muted.TLabel").grid(
            row=0, column=1, sticky="w", padx=10
        )
        self.button_check = self._button(
            results_toolbar, text="Check metadata / PDFs", command=self.check_metadata_and_pdfs, width=21
        )
        self.button_check.grid(row=0, column=2, padx=(8, 0))
        self.button_download = self._button(
            results_toolbar, text="Download selected PDFs", command=self.download_selected_pdfs, width=21
        )
        self.button_download.grid(row=0, column=3, padx=(6, 0))
        self.button_details = self._button(results_toolbar, text="Details", command=self.show_selected_details, width=9)
        self.button_details.grid(row=0, column=4, padx=(6, 0))

        filter_toolbar = ttk.Frame(results_toolbar, style="App.TFrame")
        filter_toolbar.grid(row=1, column=0, columnspan=5, sticky="ew", pady=(5, 0))
        filter_toolbar.columnconfigure(2, weight=1)
        ttk.Label(filter_toolbar, text="PDF filter", style="Muted.TLabel").grid(row=0, column=0, sticky="w")
        self.combo_filter = ttk.Combobox(
            filter_toolbar,
            textvariable=self.pdf_filter_var,
            values=list(PDF_FILTERS),
            state="readonly",
            width=20,
        )
        self.combo_filter.grid(row=0, column=1, sticky="w", padx=8)
        self.combo_filter.bind("<<ComboboxSelected>>", self.apply_pdf_filter)
        self.button_export_excel = self._button(filter_toolbar, text="Export Excel", command=self.export_excel_results, width=12)
        self.button_export_excel.grid(row=0, column=3, padx=(8, 0))
        self.button_export_csv = self._button(filter_toolbar, text="Export CSV", command=self.export_csv_results, width=12)
        self.button_export_csv.grid(row=0, column=4, padx=(6, 0))

        self.results_content = ttk.Frame(results_frame, style="App.TFrame")
        self.results_content.grid(row=1, column=0, sticky="nsew")
        self.results_content.columnconfigure(0, weight=1)
        self.results_content.rowconfigure(0, weight=1)

        columns = ("title", "authors", "score", "rank", "pdf", "link")
        self.table_container = self._surface(self.results_content, border=COLORS["border"])
        self.table_container.grid(row=0, column=0, sticky="nsew")
        self.table_container.columnconfigure(0, weight=1)
        self.table_container.rowconfigure(0, weight=1)

        self.results_table = ttk.Treeview(
            self.table_container,
            columns=columns,
            show="headings",
            selectmode="extended",
            height=TABLE_ROW_COUNT,
        )
        self.results_table.heading("title", text="Title")
        self.results_table.heading("authors", text="Authors")
        self.results_table.heading("score", text="Rel.")
        self.results_table.heading("rank", text="Rank")
        self.results_table.heading("pdf", text="PDF")
        self.results_table.heading("link", text="Link")
        self.results_table.column("title", width=330, minwidth=110, stretch=True)
        self.results_table.column("authors", width=200, minwidth=90, stretch=True)
        self.results_table.column("score", width=70, minwidth=55, anchor="center", stretch=False)
        self.results_table.column("rank", width=90, minwidth=70, anchor="center", stretch=False)
        self.results_table.column("pdf", width=125, minwidth=125, anchor="center", stretch=False)
        self.results_table.column("link", width=70, minwidth=55, anchor="center", stretch=False)
        self.results_table.grid(row=0, column=0, sticky="nsew")
        self.results_table.bind("<Double-1>", self.open_selected_link)
        scrollbar = ttk.Scrollbar(self.table_container, orient="vertical", command=self.results_table.yview)
        scrollbar.grid(row=0, column=1, sticky="ns")
        horizontal_scrollbar = ttk.Scrollbar(self.table_container, orient="horizontal", command=self.results_table.xview)
        horizontal_scrollbar.grid(row=1, column=0, columnspan=2, sticky="ew")
        self.results_table.configure(yscrollcommand=scrollbar.set, xscrollcommand=horizontal_scrollbar.set)

        self.empty_frame = ttk.Frame(self.results_content, style="App.TFrame")
        self.empty_frame.grid(row=0, column=0, sticky="nsew")
        self.empty_frame.columnconfigure(0, weight=1)
        self.empty_frame.rowconfigure(0, weight=1)
        empty_inner = ttk.Frame(self.empty_frame, style="App.TFrame")
        empty_inner.grid(row=0, column=0)
        ttk.Label(empty_inner, textvariable=self.empty_title_var, style="Title.TLabel").grid(row=0, column=0, pady=(0, 6))
        ttk.Label(empty_inner, textvariable=self.empty_detail_var, style="Muted.TLabel").grid(row=1, column=0)
        self.table_container.grid_remove()

    def browse_folder(self) -> None:
        folder_path = filedialog.askdirectory()
        if folder_path:
            self.output_folder_var.set(folder_path)

    def start_search(self) -> None:
        if self._is_running():
            return

        try:
            request = validate_search_inputs(
                self.query_var.get(),
                self.pages_var.get(),
                self.ranking_var.get(),
                self.output_folder_var.get(),
            )
        except ValueError as exc:
            self._set_status("Check search setup", str(exc), "error")
            return

        self.current_request = request
        self.current_articles = []
        self._clear_results()
        self._show_empty_state("Searching", "Results will appear here as soon as the search completes.")
        self.progress_var.set(0)
        self.summary_var.set("Searching...")
        self._set_status("Searching Google Scholar", "Starting search...", "running")
        self._set_running_state("search")

        self.cancel_event = threading.Event()
        self.message_queue = queue.Queue()
        self.worker = SearchWorker(request, self.message_queue, self.cancel_event)
        self.worker.start()
        self.window.after(100, self.poll_worker_queue)

    def check_metadata_and_pdfs(self) -> None:
        if self._is_running():
            return
        if not self.current_articles:
            self._set_status("Nothing to check", "Run a search first, then check metadata and public PDFs.", "warning")
            return

        articles = list(self.current_articles)
        catalog = self.catalog
        self.progress_var.set(0)
        self._set_status(
            OPERATION_HEADINGS["check"],
            "Looking up verified bibliographic metadata, then validating public PDF links. Records stay usable if a provider fails.",
            "running",
        )
        self._start_task("check", lambda cancel, progress: run_check(articles, catalog, cancel, progress))

    def import_rankings(self) -> None:
        if self._is_running():
            return

        path = self.ranking_path_var.get().strip()
        if not path:
            path = filedialog.askopenfilename(
                title="Choose a journal ranking CSV or XLSX file",
                filetypes=[("Ranking files", "*.csv *.tsv *.txt *.xlsx *.xlsm"), ("All files", "*.*")],
            )
            if not path:
                self._set_status("Import cancelled", "No ranking file was chosen.", "warning")
                return
            self.ranking_path_var.set(path)

        if not Path(path).is_file():
            self._set_status("Check ranking file", f"No file found at {path}.", "error")
            return

        year_text = self.ranking_year_var.get().strip()
        default_year = None
        if year_text:
            try:
                default_year = int(year_text)
            except ValueError:
                self._set_status(
                    "Check ranking year",
                    "Ranking year must be a whole number, or empty when the file supplies it.",
                    "error",
                )
                return
            if not 1500 <= default_year <= 2100:
                self._set_status("Check ranking year", "Ranking year must be between 1500 and 2100.", "error")
                return

        self.progress_var.set(0)
        self._set_status(
            OPERATION_HEADINGS["import"],
            "Validating the file in the background. A rejected file leaves the loaded records untouched.",
            "running",
        )
        self._start_task(
            "import", lambda cancel, progress: run_import(path, default_year, progress, self.catalog)
        )

    def download_selected_pdfs(self) -> None:
        if self._is_running():
            return

        selected = self._selected_entries()
        if not selected:
            self._set_status(
                "No papers selected",
                "Select one or more rows (Ctrl/Shift click), then download the accessible public PDFs.",
                "warning",
            )
            return

        folder = filedialog.askdirectory(title="Choose a folder for the downloaded PDFs")
        if not folder:
            self._set_status("Download cancelled", "No destination folder was chosen.", "warning")
            return

        indices = [index for index, _ in selected]
        articles = [article for _, article in selected]
        self.progress_var.set(0)
        self._set_status(
            OPERATION_HEADINGS["download"],
            f"Checking and saving public PDFs for {len(articles)} selected record(s).",
            "running",
        )
        self._start_task("download", lambda cancel, progress: run_download(indices, articles, folder, cancel, progress))

    def cancel_search(self) -> None:
        if self.cancel_event is not None and self._is_running():
            self.cancel_event.set()
            self._set_button_state(self.button_cancel, tk.DISABLED)
            operation = self.operation or "search"
            if operation == "import":
                self._set_status("Cancelling import", "The loaded ranking records are kept.", "warning")
            elif operation == "download":
                self._set_status("Cancelling download", "Finishing the current safe stop point...", "warning")
            elif operation == "check":
                self._set_status("Cancelling check", "Finishing the current safe stop point...", "warning")
            else:
                self._set_status("Cancelling search", "Finishing the current safe stop point...", "warning")

    def poll_worker_queue(self) -> None:
        if self._closed:
            return

        while True:
            try:
                message = self.message_queue.get_nowait()
            except queue.Empty:
                break

            if message.kind == "progress":
                self._handle_progress(message.payload)
            elif message.kind == "result":
                self._handle_result(message.payload)
            elif message.kind == "task_progress":
                self._handle_task_progress(message.payload)
            elif message.kind == "task_result":
                self._handle_task_result(message.payload)
            elif message.kind == "task_error":
                self._handle_task_error(message.payload)

        if self._is_running() and not self._closed:
            self.window.after(100, self.poll_worker_queue)

    def export_excel_results(self) -> None:
        visible = self._visible_articles()
        if not visible:
            self.status_var.set("No results to export.")
            return

        folder = self.current_request.output_folder if self.current_request else self.output_folder_var.get().strip()
        path = export_path(folder)
        try:
            save_to_excel(visible, path)
        except OSError as exc:
            self._handle_export_error(exc)
            return
        self.status_var.set(self._export_message(visible, path))

    def export_csv_results(self) -> None:
        visible = self._visible_articles()
        if not visible:
            self.status_var.set("No results to export.")
            return

        folder = self.current_request.output_folder if self.current_request else self.output_folder_var.get().strip()
        path = export_path(folder, DEFAULT_CSV_FILENAME)
        try:
            save_to_csv(visible, path)
        except OSError as exc:
            self._handle_export_error(exc)
            return
        self.status_var.set(self._export_message(visible, path))

    def _export_message(self, visible: list[Article], path: Path) -> str:
        word = "result" if len(visible) == 1 else "results"
        total = len(self.current_articles)
        if len(visible) == total:
            return f"Exported {len(visible)} {word} to {path}."
        return f"Exported {len(visible)} filtered {word} of {total} stored records to {path}."

    def apply_pdf_filter(self, _event=None) -> None:
        self._render_results()
        self._set_idle_state()

    def show_selected_details(self) -> None:
        article = self._first_selected_article()
        if article is None:
            self._set_status("No paper selected", "Select a row to see its journal, ranking and public-PDF details.", "warning")
            return
        self._open_detail_window(article)

    def open_selected_link(self, _event=None) -> None:
        article = self._first_selected_article()
        if article is None or not article.link:
            return
        webbrowser.open(article.link)

    def _handle_export_error(self, exc: OSError) -> None:
        message = export_error_message(exc)
        self.status_var.set(message)
        try:
            messagebox.showerror("Export failed", message)
        except tk.TclError:
            pass

    def on_close(self) -> None:
        self._closed = True
        if self._is_running() and self.cancel_event is not None:
            self.cancel_event.set()
        if self.detail_window is not None:
            try:
                self.detail_window.destroy()
            except tk.TclError:
                pass
            self.detail_window = None
        self.window.destroy()

    def run(self) -> None:
        self.window.mainloop()

    def _handle_progress(self, payload: object) -> None:
        data = payload if isinstance(payload, dict) else {}
        current_page = int(data.get("current_page", 0))
        total_pages = int(data.get("total_pages", 0))
        phase = str(data.get("phase", "working"))
        self.progress_var.set(progress_value(current_page, total_pages))
        self._set_status(
            OPERATION_HEADINGS["search"],
            f"{phase.capitalize()} page {current_page} of {total_pages}...",
            "running",
        )

    def _handle_result(self, payload: object) -> None:
        result = payload if isinstance(payload, ExtractionResult) else None
        if result is None:
            self._set_status("Search failed", "The application did not receive a valid result.", "error")
            self._show_empty_state("No results to display", "The search ended before any results could be shown.")
            self._finish_operation()
            self._set_idle_state()
            return

        self.current_articles = result.articles
        self.summary_base = result_summary(result)
        if len(self.catalog):
            # Local, offline labelling from the already-imported records; no network here.
            self._apply_catalog_to_current()
        self._render_results()
        if not result.articles:
            self._show_empty_state("No results to display", status_message(result))
        self._set_status(status_heading(result), status_message(result), status_tone(result))
        self.progress_var.set(progress_value(result.successful_pages, result.requested_pages))
        self._finish_operation()
        self._set_idle_state()

    def _handle_task_progress(self, payload: object) -> None:
        data = payload if isinstance(payload, dict) else {}
        operation = str(data.get("operation", ""))
        current = int(data.get("current", 0))
        total = int(data.get("total", 0))
        phase = str(data.get("phase", "working"))
        self.progress_var.set(progress_value(current, total))
        self._set_status(
            OPERATION_HEADINGS.get(operation, "Working"),
            f"{phase} ({current}/{total})",
            "running",
        )

    def _handle_task_result(self, payload: object) -> None:
        data = payload if isinstance(payload, dict) else {}
        operation = str(data.get("operation", ""))
        result = data.get("payload")

        self._finish_operation()

        if operation == "check":
            self._finish_check(result)
        elif operation == "import":
            self._finish_import(result)
        elif operation == "download":
            self._finish_download(result)

        self._set_idle_state()

    def _handle_task_error(self, payload: object) -> None:
        data = payload if isinstance(payload, dict) else {}
        operation = str(data.get("operation", ""))
        message = str(data.get("message", "")) or "The background task failed."

        self._finish_operation()

        if operation == "import":
            heading = "Ranking import failed"
            message = f"{message} Existing ranking records were kept."
        elif operation == "download":
            heading = "Download failed"
        elif operation == "check":
            heading = "Check failed"
        else:
            heading = "Operation failed"

        self._set_status(heading, message, "error")
        try:
            messagebox.showerror(heading, message)
        except tk.TclError:
            pass
        self._set_idle_state()

    def _finish_operation(self) -> None:
        self.worker = None
        self.cancel_event = None
        self.operation = None

    def _finish_check(self, result: object) -> None:
        data = result if isinstance(result, dict) else {}
        articles = data.get("articles")
        if not isinstance(articles, list) or len(articles) != len(self.current_articles):
            self._set_status(
                "Check failed",
                "The check returned an unexpected result; the existing records were kept unchanged.",
                "error",
            )
            return

        self.current_articles = articles
        self._render_results()
        note = str(data.get("enrich_error", ""))
        cancelled = bool(data.get("cancelled"))
        checked = sum(1 for article in articles if article.pdf_status != PDF_STATUS_UNKNOWN)
        detail = f"{checked} of {len(articles)} records now have a checked public-PDF status."

        if cancelled:
            self._set_status(
                "Check cancelled", f"Collected records were kept. {detail}" + (f" {note}" if note else ""), "warning"
            )
        elif note:
            self._set_status("Check finished with errors", f"{note} {detail}", "warning")
        else:
            self._set_status(
                "Check complete",
                f"Updated metadata and public-PDF status for {len(articles)} records. {detail} Records that stay "
                "unknown were not verified by the checked sources.",
                "success",
            )

    def _finish_import(self, result: object) -> None:
        data = result if isinstance(result, dict) else {}
        catalog = data.get("catalog")
        if not isinstance(catalog, JournalCatalog):
            self._set_status(
                "Ranking import failed", "The import did not return a usable catalogue; loaded records were kept.", "error"
            )
            return

        self.catalog = catalog
        count = int(data.get("count", len(catalog)))
        total = int(data.get("total", len(catalog)))
        year = data.get("year")
        path = str(data.get("path", ""))
        self._refresh_ranking_notice()
        updated = self._apply_catalog_to_current()
        year_note = f" Ranking year {year} was used for records without one." if year else " Records without a year stay unknown."
        self._set_status(
            "Rankings imported",
            f"{count} ranking records added from {Path(path).name}; {total} record(s) loaded in total "
            f"(earlier imports were kept).{year_note} {updated} displayed record(s) gained a journal label.",
            "success",
        )

    def _finish_download(self, result: object) -> None:
        data = result if isinstance(result, dict) else {}
        indices = data.get("indices")
        articles = data.get("articles")
        if not isinstance(indices, list) or not isinstance(articles, list) or len(indices) != len(articles):
            self._set_status(
                "Download failed",
                "The download returned an unexpected result; stored metadata was kept unchanged.",
                "error",
            )
            return

        newly_saved = 0
        retained = 0
        for index, article in zip(indices, articles):
            if isinstance(index, int) and 0 <= index < len(self.current_articles):
                previous_path = self.current_articles[index].pdf_local_path
                # A record that already had a file keeps its old path when this attempt failed or
                # was cancelled; only a path that actually changed is a file saved by this run.
                if article.pdf_local_path:
                    if article.pdf_local_path != previous_path:
                        newly_saved += 1
                    else:
                        retained += 1
                self.current_articles[index] = article

        failed = len(articles) - newly_saved - retained
        self._render_results()
        folder = str(data.get("folder", ""))
        parts = [
            f"Saved {newly_saved} new public PDF(s) to {folder}." if newly_saved else "No new public PDF was saved."
        ]
        if retained:
            parts.append(
                f" {retained} record(s) kept the PDF already saved earlier; this attempt did not write a new file."
            )
        if failed:
            parts.append(
                f" {failed} selected record(s) had no downloadable public PDF or were cancelled; their metadata was kept."
            )
        self._set_status("Download finished", "".join(parts), "success" if newly_saved else "warning")

    def _apply_catalog_to_current(self) -> int:
        """Re-label stored records from the imported catalogue. Local only, no network."""
        if not self.current_articles:
            return 0
        updated = 0
        relabelled: list[Article] = []
        for article in self.current_articles:
            sjr, sinta = catalog_matches(article, self.catalog)
            merged = apply_rank(apply_rank(article, sjr), sinta)
            if merged != article:
                updated += 1
            relabelled.append(merged)
        if updated:
            self.current_articles = relabelled
            self._render_results()
        return updated

    def _refresh_ranking_notice(self) -> None:
        self.ranking_notice_var.set(
            f"{len(self.catalog)} imported ranking records" if len(self.catalog) else NO_RANKING_NOTICE
        )

    def _render_results(self) -> None:
        previous = list(self.results_table.selection())
        self._clear_results()
        rows = filtered_articles(self.current_articles, self.pdf_filter_var.get())
        for index, article in rows:
            self.results_table.insert("", tk.END, iid=article_iid(index), values=article_table_row(article))

        if rows:
            self._show_results_table()
            visible_iids = {article_iid(index) for index, _ in rows}
            keep = tuple(iid for iid in previous if iid in visible_iids)
            if keep:
                self.results_table.selection_set(keep)
        elif self.current_articles:
            self._show_empty_state(
                "No papers match this filter",
                "Change the PDF filter to see the stored records. Filtering never removes stored results.",
            )
        else:
            self._show_empty_state("No results yet", "Run a search to review Google Scholar results here.")

        self._refresh_summary()

    def _refresh_summary(self) -> None:
        total = len(self.current_articles)
        if not total:
            self.summary_var.set(self.summary_base)
            return
        visible = len(self._visible_articles())
        filter_name = self.pdf_filter_var.get()
        if visible == total and filter_name == PDF_FILTER_ALL:
            self.summary_var.set(self.summary_base)
            return
        self.summary_var.set(f"{self.summary_base} | filter '{filter_name}': {visible} of {total} shown")

    def _visible_articles(self) -> list[Article]:
        return [article for _, article in filtered_articles(self.current_articles, self.pdf_filter_var.get())]

    def _selected_entries(self) -> list[tuple[int, Article]]:
        entries: list[tuple[int, Article]] = []
        for iid in self.results_table.selection():
            article = self._article_for_iid(str(iid))
            if article is not None:
                entries.append((self._index_for_iid(str(iid)), article))
        return entries

    def _first_selected_article(self) -> Article | None:
        entries = self._selected_entries()
        return entries[0][1] if entries else None

    def _index_for_iid(self, iid: str) -> int:
        return int(iid[len(_IID_PREFIX):])

    def _article_for_iid(self, iid: str) -> Article | None:
        if not iid.startswith(_IID_PREFIX):
            return None
        try:
            index = self._index_for_iid(iid)
        except ValueError:
            return None
        if 0 <= index < len(self.current_articles):
            return self.current_articles[index]
        return None

    def _clear_results(self) -> None:
        for item in self.results_table.get_children():
            self.results_table.delete(item)

    def _show_results_table(self) -> None:
        self.empty_frame.grid_remove()
        self.table_container.grid()

    def _show_empty_state(self, title: str, detail: str) -> None:
        self.empty_title_var.set(title)
        self.empty_detail_var.set(detail)
        self.table_container.grid_remove()
        self.empty_frame.grid()

    def _open_detail_window(self, article: Article) -> None:
        if self.detail_window is None or not self.detail_window.winfo_exists():
            self.detail_window = self._create_detail_window()
        if self.detail_body is not None:
            self.detail_body.configure(state=tk.NORMAL)
            self.detail_body.delete("1.0", tk.END)
            self.detail_body.insert("1.0", article_detail_text(article))
            self.detail_body.configure(state=tk.DISABLED)
        self.detail_window.deiconify()
        self.detail_window.lift()

    def _create_detail_window(self) -> tk.Toplevel:
        window = tk.Toplevel(self.window)
        window.title(f"{APP_TITLE} — paper details")
        window.geometry("820x520")
        window.minsize(520, 340)
        window.configure(bg=COLORS["app_bg"])
        window.transient(self.window)
        window.columnconfigure(0, weight=1)
        window.rowconfigure(1, weight=1)

        ttk.Label(window, text="Selected paper details", style="Title.TLabel").grid(row=0, column=0, sticky="w", padx=16, pady=(14, 8))

        frame = self._surface(window, border=COLORS["border"])
        frame.grid(row=1, column=0, sticky="nsew", padx=16)
        frame.columnconfigure(0, weight=1)
        frame.rowconfigure(0, weight=1)
        self.detail_body = tk.Text(
            frame,
            wrap="word",
            bd=0,
            height=14,
            bg=COLORS["surface"],
            fg=COLORS["text"],
            font=self.table_font,
            highlightthickness=0,
            padx=12,
            pady=10,
        )
        self.detail_body.grid(row=0, column=0, sticky="nsew")
        detail_scroll = ttk.Scrollbar(frame, orient="vertical", command=self.detail_body.yview)
        detail_scroll.grid(row=0, column=1, sticky="ns")
        self.detail_body.configure(yscrollcommand=detail_scroll.set, state=tk.DISABLED)

        ttk.Label(
            window,
            text="Double-click a row to open its link. Ranking labels describe the journal only.",
            style="Muted.TLabel",
        ).grid(row=2, column=0, sticky="w", padx=16, pady=(8, 4))
        self._detail_close = self._button(window, text="Close", command=self._close_detail_window, width=12)
        self._detail_close.grid(row=3, column=0, sticky="e", padx=16, pady=(0, 14))
        return window

    def _close_detail_window(self) -> None:
        if self.detail_window is not None:
            try:
                self.detail_window.destroy()
            except tk.TclError:
                pass
            self.detail_window = None
            self.detail_body = None

    def _set_status(self, heading: str, detail: str, tone: str) -> None:
        self.status_heading_var.set(heading)
        self.status_var.set(detail)
        self._set_status_tone(tone)

    def _set_status_tone(self, tone: str) -> None:
        colors = STATUS_COLORS.get(tone, STATUS_COLORS["ready"])
        self.status_frame.configure(bg=colors["bg"], highlightbackground=colors["border"])
        self.status_accent.configure(bg=colors["accent"])
        self.status_heading_label.configure(bg=colors["bg"], fg=colors["heading"])
        self.status_detail_label.configure(bg=colors["bg"], fg=colors["detail"])
        self.style.configure("Horizontal.TProgressbar", background=colors["accent"], lightcolor=colors["accent"], darkcolor=colors["accent"])

    def _start_task(self, operation: str, work) -> None:
        self.cancel_event = threading.Event()
        self.message_queue = queue.Queue()
        self._set_running_state(operation)
        self.worker = TaskWorker(operation, work, self.message_queue, self.cancel_event)
        self.worker.start()
        self.window.after(100, self.poll_worker_queue)

    def _set_running_state(self, operation: str = "search") -> None:
        self.operation = operation
        self._set_button_state(self.button_search, tk.DISABLED, kind="primary")
        self._set_button_state(self.button_cancel, tk.NORMAL)
        self._set_button_state(self.button_export_excel, tk.DISABLED)
        self._set_button_state(self.button_export_csv, tk.DISABLED)
        self._set_button_state(self.button_check, tk.DISABLED)
        self._set_button_state(self.button_download, tk.DISABLED)
        self._set_button_state(self.button_details, tk.DISABLED)
        self._set_button_state(self.button_import, tk.DISABLED)
        self.entry_query.configure(state=tk.DISABLED)
        self.entry_pages.configure(state=tk.DISABLED)
        self.entry_folder.configure(state=tk.DISABLED)
        self.entry_ranking_path.configure(state=tk.DISABLED)
        self.entry_ranking_year.configure(state=tk.DISABLED)
        self.button_browse.configure(state=tk.DISABLED)
        self.check_ranking.configure(state=tk.DISABLED)
        self.combo_filter.configure(state=tk.DISABLED)

    def _set_idle_state(self) -> None:
        has_articles = bool(self.current_articles)
        has_visible = bool(self._visible_articles())
        self._set_button_state(self.button_search, tk.NORMAL, kind="primary")
        self._set_button_state(self.button_cancel, tk.DISABLED)
        export_state = tk.NORMAL if has_visible else tk.DISABLED
        self._set_button_state(self.button_export_excel, export_state)
        self._set_button_state(self.button_export_csv, export_state)
        self._set_button_state(self.button_check, tk.NORMAL if has_articles else tk.DISABLED)
        self._set_button_state(self.button_download, tk.NORMAL if has_articles else tk.DISABLED)
        self._set_button_state(self.button_details, tk.NORMAL if has_articles else tk.DISABLED)
        self._set_button_state(self.button_import, tk.NORMAL)
        self.entry_query.configure(state=tk.NORMAL)
        self.entry_pages.configure(state=tk.NORMAL)
        self.entry_folder.configure(state=tk.NORMAL)
        self.entry_ranking_path.configure(state=tk.NORMAL)
        self.entry_ranking_year.configure(state=tk.NORMAL)
        self.button_browse.configure(state=tk.NORMAL)
        self.check_ranking.configure(state=tk.NORMAL)
        self.combo_filter.configure(state="readonly")

    def _set_button_state(self, button, state: str, *, kind: str = "secondary") -> None:
        is_primary = kind == "primary"
        if state == tk.DISABLED:
            button.configure(
                state=tk.DISABLED,
                bg="#9bbbe8" if is_primary else "#f1f4f8",
                fg="#edf4ff" if is_primary else COLORS["disabled"],
                highlightbackground="#9bbbe8" if is_primary else COLORS["border"],
                cursor="arrow",
            )
            return
        button.configure(
            state=tk.NORMAL,
            bg=COLORS["primary"] if is_primary else COLORS["surface_alt"],
            fg="#ffffff" if is_primary else COLORS["text"],
            highlightbackground=COLORS["primary"] if is_primary else COLORS["border_strong"],
            cursor="hand2",
        )

    def _is_running(self) -> bool:
        return bool(self.worker and self.worker.is_alive())

    def _handle_enter(self, _event=None) -> None:
        if not self._is_running():
            self.start_search()

    def _handle_escape(self, _event=None) -> None:
        if self._is_running():
            self.cancel_search()


def run() -> None:
    app = MainWindow()
    app.run()
