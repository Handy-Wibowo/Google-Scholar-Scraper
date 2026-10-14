from collections.abc import Iterable
from pathlib import Path
import csv
import re

from openpyxl import Workbook

from google_scholar_scraper.models import Article


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
