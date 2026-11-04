# Changelog

## 3.3.1

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v3.3.1 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v3.3.1).

Phase 0 of the [roadmap](docs/ROADMAP.md): foundations that make every later
phase debuggable and stop the app from wedging itself.

### Fixed

- **A worker panic no longer bricks the app.** The running latch was cleared by
  hand on each terminal path; a panic (or a later early `return`) skipped that
  call, leaving every subsequent command answering "An operation is already
  running." until restart. A RAII guard now clears the latch unconditionally —
  panic, early return, or normal completion.
- **A Crossref outage no longer reports as success.** `enrich_error` was
  hardcoded empty, so a total provider failure looked identical to a run where
  nothing matched, and the UI's "Check finished with errors" branch was dead
  code. Transport, 5xx, 429 and unparseable responses are now counted and
  reported (`N metadata lookup(s) could not be completed`).
- Both outbound HTTP clients gained connect and total timeouts; a stalled mirror
  could previously hold a worker thread indefinitely.

### Added

- **Logging.** A dependency-free file logger under
  `%APPDATA%\com.googlescholarscraper.app\logs\gss-YYYYMMDDZ.log` records every
  operation start/end, provider failures, and crashes, keeping 5 days. A panic
  hook writes `crash-<unix>.txt` with the payload, thread and location.
- **Typed command errors.** Commands now reject with `{code, message}` and the UI
  switches on the code, so changing a message can no longer silently disable a
  branch.
- **The imported ranking catalog persists** to `catalog.json` and is restored at
  startup — ranks no longer require re-importing every launch.
- **Settings carry a schema version.** A corrupt or older-layout file is replaced
  by defaults and the window says so instead of losing everything silently.
- **A persistent Crossref cache keyed by DOI** (`crossref-cache.json`), shared by
  metadata enrichment and citation resolution; a repeat run costs no requests.
- **DOI is now a dedupe key**, so a publisher page and a repository copy of the
  same paper collapse even when their titles and links differ. The summary
  reports how many merges were DOI-matched.

## 3.3.0

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v3.3.0 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v3.3.0).

### Added

- **Read the abstract without leaving the app.** A new `Abstract` action in the
  row context menu and in the details dialog fetches the paper's abstract and
  shows it in an in-app dialog. Crossref's JATS abstract is flattened to text
  first; OpenAlex's inverted-index abstract is reconstructed if Crossref has
  none. Records without a DOI fall back to an OpenAlex title search that only
  accepts an exact normalized-title match, so a fuzzy hit can never attach the
  wrong abstract. A record with no abstract anywhere says so explicitly instead
  of failing silently.
- **A journal indexing column replaces the relevance score.** The table now
  shows which abstracting service covers the journal — `SJR Q1`, `SINTA S2`,
  `SCOPUS`, `DOAJ`, or blank — derived from the imported ranking catalog plus
  source markers on the record. Local relevance still determines the default
  ordering but is no longer displayed as a column. A blank cell means the journal
  is unknown, never that it is unranked; index membership describes the journal,
  not the individual paper.
- `docs/CAPABILITY-BACKLOG.md` records a six-agent review of the app: 192 raw
  proposals consolidated into 109 verified capability candidates with effort,
  data availability, and export-parity risk, plus a dependency-ordered next
  slice.

## 3.2.0

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v3.2.0 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v3.2.0).

### Added

- **Keyboard navigation over the results grid.** The table is a focusable grid
  (`role="grid"`, `aria-selected` rows): Up/Down move, Shift+Up/Down extend the
  selection from an anchor, Space toggles, Home/End jump, Enter opens Details.
  Rows were previously unreachable without a mouse.
- **Selection feedback.** The summary reports `N selected`, `Esc` clears the
  selection when nothing else is dismissable, and `Ctrl+Shift+A` deselects all.
- **Export scope.** The spreadsheet buttons say `Excel (visible)` / `CSV
  (visible)` and a new scope selector adds **Selected rows**, so the two scopes
  are explicit instead of implicit.
- **Retry.** A failed search (rate limit, network, parsing, block) reveals a
  `Retry search` button that restores the request and runs it again.
- **Collapsible Filters panel.** After a successful search the setup panel
  collapses automatically, growing the table from 5 to ~8 visible rows; the
  toggle keeps it manual afterwards.
- **Results survive a restart.** Collected records are written to
  `%APPDATA%\com.googlescholarscraper.app\last-results.json` and restored on
  launch (`N results restored from the last session`). Capped at 2000 records and
  ignored above 5 MB so startup stays fast.
- **Clickable PDF and link cells.** A downloaded record reveals its file in
  Explorer; an available public PDF opens its URL. `pdf_url` was fetched but
  previously never shown.
- **Query history.** Recent successful queries feed a datalist and arrow-key
  recall from an empty query field (newest 20 kept).
- **Columns menu** to hide Authors/Relevance/Rank/PDF/Link, and a **Compact**
  density toggle that drops rows from ~48 px to ~21 px.
- **Details dialog actions:** Open link, Open PDF, Copy DOI, Copy title.
- Accessibility: `scope="col"`, live `aria-sort`, `role="status"` +
  `aria-live="polite"` on the status panel, and `:focus-visible` rings.
- Long cells now ellipsize with the full value in a tooltip, and sort order plus
  all new preferences persist between launches.

## 3.1.0

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v3.1.0 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v3.1.0).
Windows installer, portable exe, and `SHA256SUMS.txt` are the release assets.

### Fixed

- **The window opened with every button dead.** `init()` recursed through
  `setIdleState() → onYearPresetChanged() → renderResults() → setIdleState()`
  until the stack overflowed, so no event listener was ever registered. Rendering
  and control-state are now a strict one-way edge (`renderResults()` →
  `setIdleState()`), proven by a DOM-depth assertion and by repeated live runs.
- **Only one operation ever ran per launch.** The running latch was set by all
  five worker commands and never cleared, so the second search failed with "An
  operation is already running." Every terminal message (result, task result,
  task error) now clears it before reaching the UI.
- Seven unparenthesised `??` / `||` expressions were hard `SyntaxError`s in the
  webview's engine; they are parenthesised.
- A `task_error` left every control disabled; the terminal transition now always
  restores the idle state.
- `Ctrl+A` is select-all for the result table and no longer hijacks carets in
  text fields; `Enter` starts a search from the query and year fields only.

### Fixed (UI string parity)

Every user-visible status string in the Rust window now matches the Python
oracle verbatim, as the port has always claimed:

- **Status lookups never matched.** `ExtractionStatus` serialized as
  `"Success"` rather than `"SUCCESS"`, so every heading and tone lookup fell
  through to the fallback — the window always read "Ready" after a search. The
  enum now serializes as `SCREAMING_SNAKE_CASE`.
- Search headings use the oracle's wording (`Search stopped early`,
  `No results found`, `Google Scholar temporarily limited requests`,
  `Google Scholar returned a challenge page`, `Network request failed`,
  `Results could not be parsed safely`) and the result detail carries the
  `Run 'Check metadata / PDFs' ...` hint.
- Task failures report `Ranking import failed` / `Download failed` /
  `Check failed` / `Citation export failed` (with the "existing ranking records
  were kept" note) instead of the running labels.
- Citation exports report `Citations exported` /
  `Citation export cancelled` with the resolved-via-Crossref sentence.
- Checks report the full `Updated metadata and public-PDF status for N records.`
  sentence, and the cancelled/enrich-error variants compose the same detail.
- Downloads report `Download finished` and distinguish newly saved, retained,
  and unsaved records; imports name the file and always print the gained-label
  count; the ranking-year range has its own message.
- The details dialog prints ranking years inline, the raw `PDF status:` value,
  and `unknown` for a missing ISSN; `Link:` shows `none`.
- List summaries and `result_summary` use the oracle's singular/plural forms.

### Added

- **Worker events are permitted.** The window label `main` and a capability
  granting `core:event:listen`/`unlisten` were missing, so progress and result
  messages could never reach the page.
- **Real metadata enrichment.** `Check metadata / PDFs` now calls Crossref
  (`enrich_articles`) through a `reqwest` transport, keeping the Python
  two-phase progress contract (`total*2`) and its failure semantics.
- **Real citation resolution.** `Export citations` resolves records against
  Crossref with live per-record progress, so the status line reports an actual
  `N of M verified` count.
- **Imported rankings now apply.** The journal catalog is committed after an
  import and re-labels the records already on screen, reported as
  "N displayed record(s) gained a journal label"; searches also label new
  results from the loaded catalog. Unidentified records stay unknown rather than
  being guessed at.
- Cancellation now reaches metadata enrichment, PDF checks, and downloads
  (`AppCancelBridge`); a cancelled citation export still writes no file.
- Settings persist between launches to
  `%APPDATA%\com.googlescholarscraper.app\ui-settings.json` (atomic write,
  defaults on any read failure).
- `File → Export citations…` menu and `Ctrl+E`; sortable columns; select-all; a
  session-only results filter; an `Open folder` button after exports; a row
  context menu (open link, reveal PDF in Explorer, copy title, copy DOI,
  details); the progress bar resets when an operation ends; error dialogs dismiss
  with Escape.

## 2.2.2

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v2.2.2 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.2)
after fresh CI and Windows installer checks pass on the release commit. Windows
installer, portable ZIP, and `SHA256SUMS.txt` are the release assets.

### Added

- BibTeX (`.bib`) and RIS (`.ris`) citation export for one paper, the selected rows, the
  visible (filtered) rows, or all collected records, importable into Zotero and Mendeley.
- Citation records are resolved against Crossref where possible; unresolved records are
  still exported with the fields already known and an explicit review note. The Google
  Scholar citation line is never parsed into authors.
- `Export citations` sits grouped with the Excel and CSV export buttons in the results
  filter row (a `File → Export citations…` item and `Ctrl+E` remain as secondary paths).

### Fixed

- The export button cluster is relabelled (`Excel`, `CSV`, `Citations`) so all three fit
  the minimum supported window without clipping the applied-year range.

## 2.2.1

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v2.2.1 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.1)
after fresh CI and Windows installer checks pass on the release commit. Windows
installer, portable ZIP, and `SHA256SUMS.txt` are the release assets.

### Added

- Added an exact normalized-title OpenAlex fallback for public-PDF discovery when a result has no DOI.
- After a search, the UI now reminds users to run `Check metadata / PDFs`.

### Fixed

- Bot-challenge HTML returned by public-PDF links now keeps PDF availability `unknown` instead of incorrectly reporting `unavailable`.

## 2.2.0

Normal release of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
developed on `feature/scholar-year-filters` and published at the
[v2.2.0 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.0)
after fresh CI and Windows installer checks pass on the release commit. Windows
installer, portable ZIP, and `SHA256SUMS.txt` are the release assets.

- Added dynamic publication-year presets and inclusive optional From/To bounds.
- Forwarded Scholar `as_ylo`/`as_yhi` across pagination and retries; default searches omit both.
- Validate four-digit years and reversed ranges before starting a search; allow future years.
- Display the completed search's year range separately from editable setup controls.
- Preserve unknown article years, local ranking, metadata/PDF workflows and all export columns.
- Reuse the exact existing dependencies and corrected Tcl/Tk packaging hook.
- Added offline regressions and native-DPI year-control/layout acceptance checks.

## 2.1.0

Unpublished feature-branch preview: version `2.1.0` identifies the source tree on
`feature/journal-ranks-public-pdfs` in the fork
[Handy-Wibowo/Google-Scholar-Scraper](https://github.com/Handy-Wibowo/Google-Scholar-Scraper).
No release or tag has been published for it. The newest published release is
V2.0.1 from the upstream repository
[MahdiNavaei/Google-Scholar-Scraper](https://github.com/MahdiNavaei/Google-Scholar-Scraper),
whose attribution is unchanged.

### Added

- SCImago **SJR** and **SINTA** journal ranking metadata, imported from user-supplied CSV or XLSX ranking tables and matched by ISSN before conservative exact normalized journal titles.
- Journal ranking fields on every article: kind, label, ranking year, category, and source URL, with unmatched records left unknown rather than guessed.
- Bibliographic identity metadata (DOI, journal, ISSNs, publication year) attached through a `Check metadata / PDFs` action.
- Public-PDF state per result with exactly three values — `available`, `unavailable`, `unknown`. `unavailable` means the checked sources found no public PDF; errors stay `unknown` and are never reported as absence.
- PDF status filter and multi-selection download of the selected papers to a chosen folder, with progress reporting and cancellation.
- New Excel/CSV export columns: `DOI`, `Journal`, `ISSNs`, `Publication Year`, `SJR Quartile`, `SJR Year`, `SJR Category`, `SJR Source`, `SINTA Rank`, `SINTA Year`, `SINTA Source`, `PDF Status`, `PDF URL`, `PDF Source`, `PDF Detail`, `PDF Local Path`.
- Export column order derived from the article model's own export order, so a model field cannot silently drop out of an export.
- `README.md`, `docs/ranking-imports.md`, and header-only `docs/ranking_templates/*.csv` templates shipped inside the portable and installer distributions, so a packaged-app user can prepare a ranking import without a source checkout.
- `docs/RELEASE_NOTES_V2.1.0.md`.

### Changed

- Raised the application version to `2.1.0` in the package `__version__`, project metadata, PyInstaller output directory, Inno Setup definition, Windows build script, and Windows CI workflow.
- Added `constraints/release-2.1.0.txt`, reusing the V2.0.1 runtime and build pins verbatim. No dependency was added and no version was upgraded; `constraints/release-2.0.1.txt` is retained unchanged.
- Extended the packaged Excel/CSV smoke fixture to cover the new export columns from an explicitly synthetic record, with no network access, no ranking import, and no PDF download.

### Notes

- No ranking dataset is bundled and no automatic ranking download is offered. No supported, reliable automated SJR or SINTA ranking interface has been verified. Probes from this host returned HTTP 403 from SCImago and from the current SINTA domain `https://sinta.kemdiktisaintek.go.id/`, while the older SINTA domains did not resolve; a separate observation reached SINTA canonical HTML with HTTP 200. The outcomes differ by host, request, and time, and neither result is proof that these services are always blocked or always available.
- Crossref is used for bibliographic identity only; it does not supply SJR or SINTA labels.
- Journal ranking labels are journal-level review aids. They are not paper-quality scores, not citation metrics for the paper, and not proof of current Scopus indexing or accreditation.
- A Scholar `[PDF]` link is a candidate, not proof of a downloadable PDF or of a reuse licence. No paywall, login, CAPTCHA, or access-control bypass is implemented, and full PDF availability is not promised.
- License, attribution, and legal files are unchanged in this release.

## 2.0.1

### Fixed

- Corrected the declared minimum Python version to 3.10, matching the codebase's use of modern type syntax.
- Reworded partial-success extraction messages so they no longer imply an export occurred automatically.
- Added UI error handling for Excel and CSV export filesystem failures.
- Aligned local and CI checksum file entries so installer hashes use the plain artifact filename.

### Changed

- Added exact V2.0.1 release-build dependency constraints.
- Updated constrained HTTP dependencies to `requests 2.33.0`, `urllib3 2.7.0`, and `idna 3.15` following the final dependency security review.
- Raised the declared Requests runtime floor to `requests>=2.33,<3`.
- Added deterministic collection of exact third-party legal files from the constrained release-build environment.
- Included `LICENSE`, `NOTICE`, `COMMERCIAL_LICENSE.md`, `THIRD_PARTY_NOTICES.txt`, and a populated `third_party_licenses/` bundle in portable and installer layouts.
- Removed duplicate project notice files from the PyInstaller `_internal` directory.
- Added remote Windows installer install-launch-export-uninstall smoke coverage.
- Added verification of the installed third-party license bundle.
- Improved project metadata and release documentation.

## 2.0.0

### Added

- Import-safe Python package layout under `src/google_scholar_scraper`.
- Safer Google Scholar extraction with reusable sessions, explicit timeouts, bounded retries, conservative request pacing, and explicit failure states.
- Deterministic parser fixtures for success, missing fields, empty results, rate limits, blocked/challenge pages, parser drift, and temporary Google server errors.
- Article normalization, validation, and deterministic duplicate removal.
- Local lexical relevance ranking with no LLM, no external AI API, no model download, and no GPU requirement.
- Responsive Tkinter/ttk desktop UI with background worker execution, progress reporting, cancellation, result review, and English-only interface text.
- CSV and Excel export from reviewed results.
- Windows PyInstaller `onedir` packaging, portable ZIP generation, Inno Setup installer definition, and SHA-256 checksum generation.
- Final UI screenshot for README use.
- PolyForm Noncommercial License 1.0.0, attribution notice, commercial licensing document, release notes, release checklist, and CI workflows.

### Changed

- Replaced the legacy single-script implementation with separated modules for scraping, parsing, ranking, deduplication, export, and UI.
- Replaced synchronous GUI scraping with a background worker and queue-based UI updates.
- Replaced pandas-based export with direct `openpyxl` Excel export and standard-library CSV export.
- Updated documentation to describe the project as source-available for noncommercial use rather than OSI-approved open source.

### Fixed

- Invalid page-count and empty-query inputs now produce user-facing validation messages.
- Network failures, rate limits, blocked/challenge pages, no-result pages, parser incompatibility, partial success, and cancellation are classified explicitly.
- Missing optional Scholar fields no longer crash parsing.
- Partial results are preserved when later pages fail or extraction is cancelled.
