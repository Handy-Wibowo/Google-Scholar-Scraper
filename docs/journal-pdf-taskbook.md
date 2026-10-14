# Journal ranks and public PDFs — approved scope and task book

## User decisions
- Journal rankings: SCImago **SJR** Q1–Q4 (not official Scopus CiteScore quartiles), plus SINTA S1–S6. Journal-level labels, never a paper-quality score or proof of current Scopus indexing.
- Default to latest available ranking records; show source, ranking year and category when supplied. Unverified matches and missing records stay unknown. An absent match is not proof that a journal is unindexed/unaccredited.
- Public source lookups only where reliably supported; CSV/XLSX ranking imports are an approved fallback. No manufactured rankings, data years, DOI or journal identities.
- Public PDFs may include legitimate publisher/repository manuscripts, with version/source when known. No login, paywall, CAPTCHA or access-control bypass.
- Filter by public-PDF status; download only user-selected papers to a chosen folder. Show progress/cancel and failures; preserve search results.
- Include the new metadata in Excel/CSV exports.
- After verification: back up and update the installed Windows app, push **feature/journal-ranks-public-pdfs** to the user's fork. Do not push/merge main.

## Plan
1. Finish bounded feasibility checks for ranking sources and legal PDF discovery; document limitations rather than promise unavailable automatic lookups.
2. Implement metadata/ranking, PDF handling, and desktop workflow in separate owned modules using the shared interfaces below.
3. Independently review integration, run the full offline suite, and run bounded live-source checks. Do not manufacture live coverage when a provider is blocked.
4. Build a clearly versioned Windows app; verify bundled modules, smoke exports, and actual GUI workflow with controlled fixtures.
5. Back up the currently installed patched app, replace only executable/runtime artifacts while retaining installer/legal files, verify installed launch/exports, commit and push the feature branch.

## Parallel ownership
- Metadata task: models.py, dedupe.py, ranking.py, scraper/parser.py, exporters.py, new journal_catalog.py and enrichment.py; related metadata/parser/dedupe/ranking/export tests. Preserve old constructors/export columns where compatible; preserve every new field through normalization/ranking/deduplication.
- PDF task: new pdf_service.py and its tests only. No edits to shared models or UI. Own public-URL validation, bounded PDF checks, atomic downloads, filenames, overwrite prevention and cancellation.
- Desktop task: ui/tkinter_app.py, tests/test_ui.py and new UI tests only. No edits to shared metadata/PDF implementations. Keep all network/file download work off Tk's main thread; use stable article IDs under filters, multiselection and cancellation.
- Dispatcher: task book, integration review, acceptance, release documentation and packaging hand-off. No parallel editing of delegated code.

## Shared interface contract (for implementers)
Retain the existing frozen Article fields/positional constructor order: title, authors, link, relevance_score. Add defaults for these fields:
- doi: str = ""; journal: str = ""; issns: tuple[str, ...] = (); publication_year: int | None = None
- sjr_quartile: str = ""; sjr_year: int | None = None; sjr_category: str = ""; sjr_source_url: str = ""
- sinta_rank: str = ""; sinta_year: int | None = None; sinta_source_url: str = ""
- pdf_url: str = ""; pdf_source_url: str = ""; pdf_status: str = "unknown"; pdf_detail: str = ""; pdf_local_path: str = ""
PDF states are exactly available / unavailable / unknown. `unavailable` means no public PDF was found by checked sources, not proof of universal absence; provider/transport errors and unverified candidate links remain unknown.

Metadata module exports:
- JournalCatalog(): local ranking store; import_file(path: str | Path) -> int adds validated CSV/XLSX records transactionally and returns loaded count; invalid imports retain existing data. Keep source/year/category from actual records, match ISSNs before conservative exact normalized titles, do not guess from authors/source text.
- enrich_articles(articles: list[Article], catalog: JournalCatalog | None = None, cancel_event: Event | None = None, progress_callback: Callable[[int, int, str], None] | None = None) -> list[Article]. Keep ordering and partial results. Verified bibliographic matching only; lookup failures leave original articles usable. Prefer injectable HTTP sessions/services for tests.

PDF module exports (does not require metadata module):
- check_pdf_availability(article: Article, cancel_event: Event | None = None) -> Article. Validate candidate public links and, where a DOI enables a documented supported OA lookup, discover candidates. Never mark HTML/login pages as available. Preserve all unrelated Article fields.
- download_pdfs(articles: list[Article], folder: str | Path, cancel_event: Event | None = None, progress_callback: Callable[[int, int, str], None] | None = None) -> list[Article]. Preserve input ordering including failures/cancelled entries; successful entries get pdf_local_path, per-item failures go in pdf_detail. No destructive overwrite or executable content; bounded streamed checks and temporary-file cleanup.

UI behavior:
- Retain normal Scholar search. Provide a background `Check metadata / PDFs` action and explain that unchecked records remain unknown. Imported rankings apply to known journal identities; rerun/check action can enrich missing identities.
- Show SJR / SINTA / PDF status plus metadata source/year detail, a ranking import button, and a PDF status filter (All / Public PDF available / No public PDF found / Unknown).
- Multi-select visible papers and download selected accessible PDFs after folder choice. Filtering changes the visible set, not stored results. Use stable row-to-article mappings, not visible indices. Export the visible filtered set consistently and label it.
- Disable conflicting operations while workers run; cancel applies to the current search/check/download. Report explicit status and retain collected/enriched records on failures.

## Feasibility and retry record
- Crossref REST is reachable from this host for bibliographic identity. Neither Crossref nor OpenAlex supplies SJR/SINTA ranking labels.
- SCImago programmatic ranking access returned HTTP 403 in a bounded probe; no reliable automatic ranking feed was verified. Use legitimate manually downloaded ranking exports, not a challenge bypass.
- Old SINTA domains failed DNS in the initial probe. A dispatcher follow-up to the current public domain `https://sinta.kemdiktisaintek.go.id/` returned HTTP 403 without redirects. These are host/domain-specific findings, not proof that SINTA is unavailable everywhere. No reliable live SINTA feed has been verified.
- Dispatcher follow-up: Crossref `works/10.1038/nature14539` and OpenAlex `works/doi:10.1038/nature14539` both returned HTTP 200 JSON without credentials. This verifies bibliographic/OA work endpoints on this host, not journal-ranking coverage or all PDF candidates.
- A Scholar `[PDF]` link is a candidate, not proof of PDF content or an open-access licence. PDF availability requires a successful bounded validation; provider/transport errors remain unknown.
- The initial implementation workers failed before editing code. A clean-tree check confirmed only this task book was new. Retried metadata, PDF and UI workers on `bandelbanget/kimi-k2.7-code`, keeping their separate file ownership and shared contracts.
- No new dependency or ranking data is assumed. Ranking imports and clear unknown states are the approved fallback, not fabricated live coverage.

## Acceptance gates
- Existing tests stay green; new tests cover missing/invalid rank records, ISSN/journal matching, ranking year selection, metadata preservation through ranking/deduplication, and export fields.
- PDF tests use controlled responses: real PDF bytes vs HTML masquerading as PDF, redirects, missing links, transport failure, unsafe URLs/filenames, duplicate names/no overwrite, size bounds, partial cleanup and cancellation.
- UI tests cover worker behavior, filter mapping, multiselection/export selection semantics, progress, failure and cancellation. No network in unit tests.
- Independent review does not treat agent self-reports as acceptance. Installed build and source version must match; no main-branch push.

## Independent review record (before packaging)
- The pinned release environment is `E:\ai_project\google-scholar-scraper\release-env\Scripts\python.exe`, using `constraints/release-2.1.0.txt`. The dependency set is unchanged from V2.0.1.
- Metadata corrective passes 1 and 2 were independently accepted: title lookup caching includes author/year constraints, cached answers remain usable after lookup budgets are exhausted, and real Scholar classification spans are not mistaken for title text. Scholar's `javascript:void(0)` PDF expanders are excluded from candidates. The original bounded SOMLAP search returned results; this does not establish exhaustive coverage.
- PDF corrective pass 1 was independently accepted. A real Scholar-discovered arXiv PDF (`2501.10956`) was validated and downloaded to the review output folder: 655696 bytes, `%PDF-1.5`, no partial files left. This is one live-source check, not a promise that every publisher will be accessible.
- Native desktop probes accepted stable filtered multiselection, visible-set CSV/XLSX exports with metadata, sequential SJR/SINTA catalogue accumulation, and truthful reporting when a failed retry retains a previously downloaded file.
- Desktop corrective passes 1 and 2 did not close the DPI layout gate; the second worker returned unfinished diagnostics with failing native bounds. Dispatcher fallback was restricted to that UI-layout task. It added the missing parent-grid weight, separated input/filter grids, compacted spacing without changing fonts, and removed the unnecessary resize-notice helper.
- Final source acceptance: all 308 pinned offline tests pass; all 68 desktop tests also pass in a fresh DPI-aware process. The standalone `python tools/ui_layout_probe.py` returns exit 0 at Tk scaling 1.6683: 860x560 has query/path widths 615/416 px and table height 100 px, 880x620 has 635/436 and 160 px, and 1080x700 has 835/636 and 240 px. Both scrollbars and essential controls remain inside the windows. Regenerated screenshots were visually reviewed, and native filter/multiselection/metadata export actions passed.
- Versioned documentation distinguishes unpublished release artifacts from locally produced builds. Current SINTA reachability differs with request headers; neither this nor the SCImago probe established a supported automated ranking interface. CSV/XLSX imports remain the approved ranking path.
- Final release acceptance: 311 offline tests pass. The initial native portable failed because PyInstaller's stock `pyi_rth__tkinter` required missing `_tcl_data`. After two incomplete packaging-worker corrective passes, dispatcher fallback was restricted to this release task. The compatibility hook now replaces the stock hook in `Analysis.scripts` (not `datas`/`pure`); regression tests exercise the actual script-table assignment and external-library overrides. No dependency upgrades or empty placeholder library directories were used.
- The rebuilt portable's real GUI and relocated export smoke pass. Both shipped DLLs contain ZIP script libraries; native Tcl/Tk 9.0.4 initialization verified `//zipfs:/lib/tcl/tcl_library` and `//zipfs:/lib/tk/tk_library`. The compiled archive contains the compatibility hook and excludes the failing stock hook. ZIP integrity, all required notices/docs/header-only templates, frozen feature modules, and both 20-column CSV/XLSX smoke exports pass independent checks.
- Complete pre-upgrade backup: `E:\ai_project\google-scholar-scraper\backup-installed-v2.0.1-before-v2.1.0-20261015-035525`, 119 files, every hash verified before replacement. Installed v2.1.0 payload hashes match the verified build; original uninstaller preserved; installed exports and actual responsive GUI pass. EXE SHA-256: `961b31abc219124dff608475a1f86fba73d35a1678edac466075e63b224b4ea7`. Portable SHA-256: `4e087469405627d5495e9ea2bf5a25be02a3ff9f1c29512c213eef4953be6465`.
- Only feature-branch publication is authorized. Remote fork main baseline is `d81ab1f0347b02d6d8d65aeeb4fd1fcad72f9b07`; compare it after pushing `feature/journal-ranks-public-pdfs`. No release tag or main-branch modification is authorized.
