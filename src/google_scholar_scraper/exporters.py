from collections.abc import Iterable
from pathlib import Path
import csv
import re

from openpyxl import Workbook

from google_scholar_scraper.models import Article, CitationRecord
from google_scholar_scraper.enrichment import (
    SCHOLAR_AUTHORS_WARNING,
    UNRESOLVED_WARNING,
)


# Column order is the model's own export order, so adding an Article field cannot
# silently drop it from an export. `EXPORT_COLUMNS` stays a module-level list for
# existing callers and tests.
EXPORT_COLUMNS: list[str] = list(Article(title="").to_export_row().keys())

# A cell is only dangerous when a spreadsheet would evaluate it as a formula. The
# prefixes below are the ones Excel/LibreOffice treat as formula starts; a leading tab
# or CR can also be stripped by some importers and expose one, so those count too.
_FORMULA_PREFIXES = ("=", "+", "-", "@")
_CONTROL_PREFIX_RE = re.compile(r"^[\t\r\n]+")


def article_export_rows(articles: Iterable[Article]) -> list[dict[str, object]]:
    return [article.to_export_row() for article in articles]


def save_to_excel(articles: Iterable[Article], filename: str | Path) -> None:
    rows = article_export_rows(articles)
    workbook = Workbook()
    worksheet = workbook.active
    worksheet.title = "Scholar Articles"
    worksheet.append(EXPORT_COLUMNS)
    for row in rows:
        worksheet.append([sanitize_cell(row.get(column, "")) for column in EXPORT_COLUMNS])
    workbook.save(filename)


def save_to_csv(articles: Iterable[Article], filename: str | Path) -> None:
    rows = [
        {column: sanitize_cell(row.get(column, "")) for column in EXPORT_COLUMNS}
        for row in article_export_rows(articles)
    ]
    with Path(filename).open("w", newline="", encoding="utf-8-sig") as output:
        writer = csv.DictWriter(output, fieldnames=EXPORT_COLUMNS, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)


def sanitize_cell(value: object) -> object:
    """Neutralize spreadsheet formula injection without altering scientific text.

    Titles, journal names, categories and free-text details come from third parties and
    may legitimately start with `-` or `+` (a hyphenated title, a range). Numbers keep
    their numeric type so Excel still treats them as numbers. Only text that a
    spreadsheet would actually evaluate as a formula is defused, by prefixing a single
    quote — the standard spreadsheet escape — so the visible text is unchanged.
    """
    if not isinstance(value, str):
        return value

    stripped = _CONTROL_PREFIX_RE.sub("", value)
    if not stripped:
        return value

    if stripped[0] in _FORMULA_PREFIXES:
        # "-12.5" and "+3" are numbers, not formulas, but as text they would still be
        # evaluated. They are exported as text with the escape so no formula runs.
        return "'" + value
    return value


# ---------------------------------------------------------------------------
# Citation writers (BibTeX / RIS).
#
# These are deliberately separate from the spreadsheet surface above: citation
# formats need their own escaping (braces, LaTeX specials, RIS line framing) and
# `sanitize_cell` is a spreadsheet-only defense. Values are single-lined because
# both formats treat newlines as record/field boundaries.


_BIBTEX_SPECIALS_RE = re.compile(r"([{}%$&#_^~\\])")
_RIS_SPECIALS_RE = re.compile(r"[\r\n]+")
_WHITESPACE_RUN_RE = re.compile(r"\s+")

# Crossref work type -> (BibTeX entry type, RIS TY value).
_BIBTEX_TYPE_MAP = {
    "journal-article": "article",
    "proceedings-article": "inproceedings",
    "book-chapter": "incollection",
}
_RIS_TYPE_MAP = {
    "journal-article": "JOUR",
    "proceedings-article": "CONF",
    "book-chapter": "CHAP",
}
_BIBTEX_DEFAULT_TYPE = "misc"
_RIS_DEFAULT_TYPE = "GEN"


def _single_line(value: object) -> str:
    """Collapse a value to one line: both formats treat newlines as boundaries."""
    return _WHITESPACE_RUN_RE.sub(" ", str(value or "")).strip()


def _bibtex_escape(value: str) -> str:
    """Escape characters a BibTeX/LaTeX reader would otherwise interpret."""
    return _BIBTEX_SPECIALS_RE.sub(r"\\\1", value)


def _ris_escape(value: str) -> str:
    """RIS carries raw UTF-8 text; only line structure must be neutralized."""
    return value


def _ris_author(value: str) -> str:
    """Render one structured author ("Given Family") as RIS "Family, Given"."""
    if " " not in value:
        return value
    given, _, family = value.rpartition(" ")
    return f"{family}, {given.strip()}" if family.strip() else value


def _ris_pages(pages: str) -> tuple[str, str]:
    """Split a page range into (SP, EP); an open range keeps the whole value in SP."""
    value = _single_line(pages)
    if not value:
        return "", ""
    for separator in ("-", "–"):
        if separator in value:
            first, _, last = value.partition(separator)
            return first.strip(), last.strip()
    return value, ""


def _bibtex_note(record: CitationRecord) -> str:
    """Assemble the review note: the Scholar line verbatim, then the warnings."""
    parts = []
    if record.fallback_authors_line:
        parts.append(record.fallback_authors_line)
    parts.extend(record.warnings)
    return _single_line("; ".join(parts))


def _bibtex_key(record: CitationRecord, taken: set[str]) -> str:
    """Deterministic, unique citation key: surname_titleword_year[-N]."""
    if record.authors:
        first_author = _single_line(record.authors[0])
        surname = first_author.rpartition(" ")[2] or first_author
    else:
        surname = "anon"
    match = re.search(r"[A-Za-z0-9]+", record.title or "")
    title_word = match.group(0).lower() if match else "title"
    base = _WHITESPACE_RUN_RE.sub("", f"{surname}_{title_word}_{record.year or 'nd'}").lower()
    base = re.sub(r"[^a-z0-9_-]", "", base) or "citation"
    key = base
    counter = 2
    while key in taken:
        key = f"{base}-{counter}"
        counter += 1
    taken.add(key)
    return key


def save_to_bibtex(records: Iterable[CitationRecord], filename: str | Path) -> None:
    """Write one BibTeX entry per record, in input order, overwriting the file.

    Unverified records carry no ``author`` field — the Scholar display line is a
    mixed author/venue/year string and must never be presented as structured
    authors. Their review note travels in ``note``.
    """
    taken: set[str] = set()
    blocks: list[str] = []
    for record in records:
        entry_type = _BIBTEX_TYPE_MAP.get(record.entry_type, _BIBTEX_DEFAULT_TYPE)
        container_field = "journal" if entry_type in ("article", "misc") else "booktitle"
        lines = [f"@{entry_type}{{{_bibtex_key(record, taken)},"]
        if record.authors:
            lines.append(f"  author = {{{_bibtex_escape(' and '.join(record.authors))}}},")
        lines.append(f"  title = {{{{{_bibtex_escape(_single_line(record.title))}}}}},")
        if record.container:
            lines.append(f"  {container_field} = {{{_bibtex_escape(record.container)}}},")
        if record.year is not None:
            lines.append(f"  year = {{{record.year}}},")
        if record.volume:
            lines.append(f"  volume = {{{_bibtex_escape(record.volume)}}},")
        if record.issue:
            lines.append(f"  number = {{{_bibtex_escape(record.issue)}}},")
        if record.pages:
            lines.append(f"  pages = {{{_bibtex_escape(record.pages)}}},")
        if record.publisher:
            lines.append(f"  publisher = {{{_bibtex_escape(record.publisher)}}},")
        if record.doi:
            lines.append(f"  doi = {{{record.doi}}},")
        if record.issns:
            lines.append(f"  issn = {{{', '.join(record.issns)}}},")
        if record.url:
            lines.append(f"  url = {{{record.url}}},")
        note = _bibtex_note(record)
        if note:
            lines.append(f"  note = {{{_bibtex_escape(note)}}},")
        blocks.append("\n".join(lines) + "\n}\n")

    with Path(filename).open("w", encoding="utf-8", newline="") as output:
        output.write("\n".join(blocks))


def save_to_ris(records: Iterable[CitationRecord], filename: str | Path) -> None:
    """Write one RIS record per input record, in order, overwriting the file.

    Same author rule as BibTeX: unverified records emit no ``AU`` tag; their
    review note (Scholar line + warnings) travels in ``N1``.
    """
    rendered: list[str] = []
    for record in records:
        lines = [f"TY  - {_RIS_TYPE_MAP.get(record.entry_type, _RIS_DEFAULT_TYPE)}"]
        for author in record.authors:
            lines.append(f"AU  - {_ris_author(author)}")
        lines.append(f"TI  - {_single_line(record.title)}")
        if record.container:
            lines.append(f"T2  - {_single_line(record.container)}")
        if record.year is not None:
            lines.append(f"PY  - {record.year}")
        if record.volume:
            lines.append(f"VL  - {_single_line(record.volume)}")
        if record.issue:
            lines.append(f"IS  - {_single_line(record.issue)}")
        first_page, last_page = _ris_pages(record.pages)
        if first_page:
            lines.append(f"SP  - {first_page}")
        if last_page:
            lines.append(f"EP  - {last_page}")
        if record.doi:
            lines.append(f"DO  - {record.doi}")
        if record.url:
            lines.append(f"UR  - {record.url}")
        for issn in record.issns:
            lines.append(f"SN  - {issn}")
        if record.publisher:
            lines.append(f"PB  - {_single_line(record.publisher)}")
        note_parts = []
        if record.fallback_authors_line:
            note_parts.append(record.fallback_authors_line)
        note_parts.extend(record.warnings)
        if note_parts:
            lines.append(f"N1  - {_RIS_SPECIALS_RE.sub(' ', ' | '.join(note_parts))}")
        lines.append("ER  - ")
        rendered.append("\r\n".join(lines))

    with Path(filename).open("w", encoding="utf-8", newline="") as output:
        output.write("\r\n".join(rendered))
