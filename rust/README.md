# Google Scholar Scraper — Rust (v3.8.0)

Native Rust rewrite of the Python desktop app (`../src`, frozen at v2.2.2 as the
behavioral oracle). Same core logic — Scholar scraping, PDF
availability/download, Crossref enrichment, citations, ranking, dedupe, journal
catalog, exports — with a Tauri v2 GUI (plain static `ui/`, no node).

## Layout

| Crate | Role |
|---|---|
| `gss-core` | All logic: `client`, `parser`, `pdf`, `crossref`, `catalog`, `exporters`, `dedupe`, `ranking`, `models`, `text`, `url`. Contracts in `gss-core/docs/CONTRACTS.md`. |
| `gss-app` | Tauri GUI shell + `--packaging-smoke` / `--selfcheck-layout` headless modes. |
| `gss-parity` | JSON-dump harness driven by `tools/parity_check.py`. |

## Build

Use `tools/build.cmd` — windres's inner `cmd /c` preprocessor needs
`CONDA_SHLVL=1` (silences the machine's cmd AutoRun conda hook) and a
backslash-format mingw64 PATH:

```bat
tools\build.cmd build --release
```

Requirements: Rust `stable-x86_64-pc-windows-gnu` + mingw-w64 binutils/gcc
(no MSVC Build Tools on this workstation — recorded deviation).

## Tests

```bat
tools\build.cmd test --workspace        :: 25 + 262 tests
tools\build.cmd clippy --workspace -- -D warnings
tools\build.cmd fmt --all --check
```

## GUI (v3.1.0)

The window is a plain static page (`gss-app/ui/`) over Tauri IPC. Beyond the
search table it carries: a `File → Export citations…` entry with `Ctrl+E`;
click-to-sort column headers (`▲`/`▼`); `Ctrl+A` to select every visible row; a
session-only `Filter results…` box beside the PDF filter; an `Open folder`
button after an export; a row context menu (open link, reveal the downloaded PDF
in Explorer, copy title, copy DOI, details); a progress bar that resets when an
operation ends; error dialogs that dismiss with Escape; and settings
(query, pages, folders, ranking file/year, year preset, PDF filter) persisted to
`%APPDATA%\com.googlescholarscraper.app\ui-settings.json`.

Cancelling, metadata enrichment, citation resolution, and rank application are
all live against the network — nothing is a stub.

### v3.3.0 additions

**Abstracts on demand** (`gss-core/src/abstracts.rs`): Crossref JATS is flattened
to text and OpenAlex's `abstract_inverted_index` is reconstructed, exposed through
`fetch_abstract` and shown in an in-app dialog from the row context menu or the
details dialog. Records without a DOI fall back to an OpenAlex title search that
only accepts a normalized-title match.

**Journal indexing column**: `catalog::indexing_labels` derives `SJR Q1`,
`SINTA S2`, `SCOPUS`, or `DOAJ` badges from the imported catalog and source
markers; the local relevance score remains the default sort but is no longer a
column. Empty means unknown, never unranked.

See `docs/CAPABILITY-BACKLOG.md` for the reviewed backlog of further work.

Carried from 3.2.0: keyboard grid navigation, selection counts, export scope
(visible/selected), retry after failure, collapsible Filters panel, results
restored across launches (`last-results.json`), clickable PDF/link cells, query
history, column visibility, compact density, and details-dialog actions. Settings
for sort order, density, hidden columns, and query history persist in
`ui-settings.json`.

## Parity vs the Python oracle

```bat
python tools\parity_check.py --python ..\src --rust .
```

Gates (all must be zero-diff):
1. `normalize_title_key` over fixtures + adversarial edge cases.
2. `parse_scholar_page` over all 9 HTML fixtures + a 429.
3. CSV/BibTeX/RIS byte-equality on a fixed record set.
4. Full offline `scrape_scholar` extraction over scripted multi-page jobs.

The Python suite (`../src/tests`, 368 tests) remains the behavioral oracle;
the Rust port mirrors it 1:1 per module.

## Packaging

`cargo build --release` → `target/release/gss-app.exe` (~14.6 MB, cold start
<500 ms). `--packaging-smoke DIR` writes `packaging-smoke.xlsx`/`.csv` with the
exact 20-column export contract; CI (`​.github/workflows/rust.yml`) runs tests,
clippy, fmt, the smoke check, and attaches the exe + `SHA256SUMS.txt` on `v3.*`
tags.

<!-- ci-trigger: rust workflow watches rust/** -->
