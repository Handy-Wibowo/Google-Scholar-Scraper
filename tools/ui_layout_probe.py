"""Native DPI-aware layout probe: run it from an ordinary (non-DPI-aware) shell on Windows.

It re-executes itself in a child process with per-monitor DPI awareness enabled BEFORE Tk is
imported, then measures the production MainWindow at the supported sizes and prints every widget
bound. Exits non-zero when an essential control, a scrollbar, or the results table falls outside
the window, or when an input field is squeezed below FIELD_MIN_WIDTH.

    python tools/ui_layout_probe.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

FIELD_MIN_WIDTH = 300
SUPPORTED_SIZES = ("860x560", "880x620", "1080x700")
CHILD_FLAG = "UI_LAYOUT_PROBE_CHILD"


def _child() -> int:
    import ctypes

    try:  # before Tk: awareness must be set while the process is still DPI-unaware
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except (AttributeError, OSError):
        pass

    import tkinter as tk

    from google_scholar_scraper.models import Article, ExtractionResult, ExtractionStatus
    from google_scholar_scraper.ui.tkinter_app import MainWindow, SearchRequest

    app = MainWindow()
    app.current_request = SearchRequest("CONTROLLED TEST FIXTURE", 1, True, "", 2018, 2024)
    failures: list[str] = []
    report: dict[str, object] = {}
    try:
        app._handle_result(
            ExtractionResult(
                ExtractionStatus.SUCCESS,
                [
                    Article("Example paper one", authors="Author A", link="https://example.org/1"),
                    Article("Example paper two", authors="Author B"),
                ],
                requested_pages=1,
                successful_pages=1,
            )
        )
        app.window.update()
        report["scaling"] = round(app.window.tk.call("tk", "scaling"), 4)
        report["min_size"] = list(app.window.minsize())
        sizes: dict[str, object] = {}
        report["sizes"] = sizes

        for geometry in SUPPORTED_SIZES:
            app.window.geometry(geometry)
            app.window.update()
            base_x = app.window.winfo_rootx()
            base_y = app.window.winfo_rooty()
            width = app.window.winfo_width()
            height = app.window.winfo_height()

            def bounds(widget: tk.Misc) -> tuple[int, int, int, int]:
                return (
                    widget.winfo_rootx() - base_x,
                    widget.winfo_rooty() - base_y,
                    widget.winfo_width(),
                    widget.winfo_height(),
                )

            def inside(widget: tk.Misc) -> bool:
                x, y, w, h = bounds(widget)
                return x >= -1 and y >= -1 and x + w <= width + 1 and y + h <= height + 1

            entry: dict[str, object] = {"window": [width, height]}
            for name, widget in (
                ("entry_query", app.entry_query),
                ("entry_ranking_path", app.entry_ranking_path),
                ("entry_ranking_year", app.entry_ranking_year),
                ("combo_year", app.combo_year),
                ("entry_year_from", app.entry_year_from),
                ("entry_year_to", app.entry_year_to),
                ("label_applied_year", app.label_applied_year),
                ("button_search", app.button_search),
                ("button_cancel", app.button_cancel),
                ("button_import", app.button_import),
                ("button_check", app.button_check),
                ("button_download", app.button_download),
                ("combo_filter", app.combo_filter),
                ("button_export_csv", app.button_export_csv),
                ("results_table", app.results_table),
                ("table_container", app.table_container),
                ("label_ranking_notice", app.label_ranking_notice),
            ):
                x, y, w, h = bounds(widget)
                entry[name] = [x, y, w, h]
                if not inside(widget):
                    failures.append(f"{geometry}: {name} is outside the window at {[x, y, w, h]}")
            for name, widget in (
                ("scrollbar_v", app.table_container.winfo_children()[1]),
                ("scrollbar_h", app.table_container.winfo_children()[2]),
            ):
                x, y, w, h = bounds(widget)
                entry[name] = [x, y, w, h]
                if not inside(widget):
                    failures.append(f"{geometry}: {name} is clipped at {[x, y, w, h]}")
            if app.results_table.winfo_height() < 90:
                failures.append(f"{geometry}: results table only {app.results_table.winfo_height()}px tall")
            if app.entry_query.winfo_width() < FIELD_MIN_WIDTH:
                failures.append(f"{geometry}: query field only {app.entry_query.winfo_width()}px wide")
            if app.entry_ranking_path.winfo_width() < FIELD_MIN_WIDTH:
                failures.append(
                    f"{geometry}: ranking path field only {app.entry_ranking_path.winfo_width()}px wide"
                )
            if app.label_applied_year.winfo_reqwidth() > app.label_applied_year.winfo_width():
                failures.append(f"{geometry}: applied year range is clipped")
            notice = app.label_ranking_notice.winfo_reqwidth()
            if notice > app.label_ranking_notice.winfo_width() + 1:
                failures.append(f"{geometry}: ranking notice needs {notice}px, gets {app.label_ranking_notice.winfo_width()}px")
            sizes[geometry] = entry
    finally:
        app.on_close()

    print(json.dumps(report, indent=2, sort_keys=True))
    for line in failures:
        print("FAIL:", line)
    return 1 if failures else 0


def main() -> int:
    if os.environ.get(CHILD_FLAG) == "1":
        return _child()

    environment = dict(os.environ, **{CHILD_FLAG: "1"})
    source_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    environment["PYTHONPATH"] = os.pathsep.join(
        part for part in [os.path.join(source_root, "src"), environment.get("PYTHONPATH", "")] if part
    )
    return subprocess.call([sys.executable, os.path.abspath(__file__)], env=environment)


if __name__ == "__main__":
    raise SystemExit(main())
