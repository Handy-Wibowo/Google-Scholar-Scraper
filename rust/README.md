# Google Scholar Scraper — Rust (v4.0.2)

Native Rust rewrite of the Python desktop app (`../src`, frozen at v2.2.2 as the
behavioral oracle). Same core logic — Scholar scraping, PDF availability and
download, Crossref/OpenAlex enrichment, citations, ranking, dedupe, journal
catalog, exports — with a Tauri v2 GUI (plain static `ui/`, no node).

## Layout

| Crate | Role |
|---|---|
| `gss-core` | All logic. Contracts in `gss-core/docs/CONTRACTS.md`. |
| `gss-app` | Tauri GUI shell + `--packaging-smoke` / `--selfcheck-layout` headless modes. |
| `gss-parity` | JSON-dump harness driven by `tools/parity_check.py`. |

`gss-core` modules: `client`, `parser`, `pdf`, `crossref`, `catalog`, `abstracts`,
`cache`, `review`, `discovery`, `exporters`, `exports_extra`, `dedupe`, `ranking`,
`models`, `text`, `url`.

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
tools\build.cmd test --workspace              :: 345 tests, 12 suites
tools\build.cmd clippy --workspace -- -D warnings
tools\build.cmd fmt --all --check
python tools\parity_check.py --python .. --rust .   :: 4 gates (1,2,4,5), zero-diff
```

## GUI

The window is a plain static page (`gss-app/ui/`) over Tauri IPC — 40 commands.
Beyond the search table it carries:

- **Search** — year preset, pages, ranking file/year, output folder; a progress
  bar that resets when an operation ends; `Retry search` after a transport failure;
  results kept on screen when a re-search fails.
- **Table** — click-to-sort headers (`▲`/`▼`, `aria-sort` maintained), keyboard
  grid navigation (arrows, `Space`, `Shift`+arrow, `Home`/`End`, `Enter`),
  `Ctrl+A` select-all, `Ctrl+Shift+A` clear, selection count in the summary,
  ellipsis truncation with full-text tooltips, column visibility, compact row
  density, dark/light/system theme, and a column legend.
- **Filters** — text query, PDF state, screening decision, tag; a collapsible
  Filters panel; Clear filters; named saved filter expressions.
- **Records** — row context menu (open link, reveal downloaded PDF, copy title,
  copy DOI, details), details dialog with the same actions, clickable PDF/link
  cells, abstracts on demand, and a bounded metadata-only refresh.
- **Per-record state** — in/out/maybe decisions with reasons, notes, and tags,
  applied individually or in bulk across a selection.
- **Persistence** — settings in `%APPDATA%\com.googlescholarscraper.app\ui-settings.json`
  (query, pages, folders, ranking, year preset, PDF/decision/tag filters, saved
  filters, theme, sort order, density, hidden columns, query history, window
  geometry); results restored across launches from `last-results.json`.

## Review workflow

`gss-core/src/review.rs` holds the systematic-review container:

- Decisions, notes, and tags keyed by `record_key` — DOI when present, else a
  normalized title — so screening survives re-search, re-sort, and append.
- PRISMA counts from live state, and a screening-log CSV with decisions and
  timestamps.
- Near-duplicate report, named baselines with search-diff against a later run.
- Append mode, re-score of stored results, and round-trip import of the app's own
  CSV/JSON export.

## Exports

| Format | Path |
|---|---|
| CSV, BibTeX, RIS | Byte-identical to the Python oracle (parity gate 4). Do not edit. |
| Excel, JSON, Markdown, TSV, ENW, CSL-JSON, details, reading list | `exports_extra.rs` |

`exports_extra.rs` exists precisely so new formats never perturb the gated
writers. Excel adds freeze panes, autofilter, hyperlinks, and grouped sheets.
Citations render as APA, IEEE, Vancouver, or CSL-JSON, with stable BibTeX keys.
Excel and CSV writes are atomic (`.tmp` + rename).

## Discovery

`gss-core/src/discovery.rs` creates result sets from sources other than a keyword
search: paste DOIs (bare, `doi:`, or `https://doi.org/…` forms, resolved through
Crossref) and citation chasing both directions through OpenAlex — works citing a
DOI, or works it cites. Additions merge into the current set and persist, so they
screen and export like any other record.

## Parity vs the Python oracle

```bat
python tools\parity_check.py --python ..\src --rust .
```

Gates (all must be zero-diff). Numbering is historic — there is no gate 3; the
export comparison it once covered lives inside gate 4:

1. `normalize_title_key` over fixtures + adversarial edge cases.
2. `parse_scholar_page` over the HTML fixtures + a 429.
4. CSV/BibTeX/RIS byte-equality on a fixed record set.
5. Scholar extraction including `fetch_with_retries` behaviour.

The Python suite (`../src/tests`, 368 tests) remains the behavioral oracle; the
Rust port mirrors it per module. Two roadmap items were refused rather than
implemented because they contradict it: fuzzy title matching (4.7, reverted) and
a change to the PDF-availability failure state (6.12, already correct).

## Packaging

`cargo build --release` → `target/release/gss-app.exe` (15.4 MiB locally; the
CI-built asset is 17.6 MiB). Measured startup on this workstation: 2.07 s on the
first launch after boot, ~0.03 s warm.

`--packaging-smoke DIR` writes `packaging-smoke.xlsx`/`.csv` against the frozen
20-column export contract.

CI (`.github/workflows/rust.yml`) runs tests, clippy, fmt, and the smoke check on
`rust/**` changes, then on any `v[0-9]+.*` tag builds the NSIS installer with
`cargo tauri build --bundles nsis` and attaches the installer, the portable exe,
and `SHA256SUMS.txt` to the release.

## Releases

| Tag | Content |
|---|---|
| v3.3.1 | Logging, typed command errors, Crossref failure counting, catalog and settings persistence, DOI cache |
| v3.4.0 | 14 metadata fields, ORCID/retraction/funder parsing, OA metadata, year histogram, gap report, journal trend |
| v3.5.0 | Extra export formats, rich Excel, CSL-JSON, stable BibTeX keys, citation styles |
| v3.6.0 | Review state: decisions, notes, tags, PRISMA, screening log, baselines, near-duplicates |
| v3.7.0 | Interface polish; atomic Excel/CSV export |
| v3.8.0 | Window geometry persistence; CI builds the installer |
| v4.0.0 | Discovery: paste DOIs, citation chasing |
| v4.0.1 | Decision/tag filters, saved filter expressions |
| v4.0.2 | Fix discovery data loss and stale titles |

See `docs/ROADMAP.md` for per-phase delivery status. P0–P3 are complete; P4–P7 are
partial, with the open items listed per phase.

<!-- ci-trigger: rust workflow watches rust/** -->
