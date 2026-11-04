# Capability backlog — Google Scholar Scraper (Rust)

Generated from a six-agent parallel review of the Rust app (`rust/`), each with a
distinct focus: research workflow, data sources, UX craft, robustness, platform
integration, and export/data interchange. Every item was verified against the code
by the agent that proposed it; "exists" / "fetch" notes say whether the data is
already in the repo or must be requested.

**Baseline (already shipped, do not re-propose):** Scholar search, Crossref
metadata enrichment, public-PDF discovery/check/download, SJR+SINTA catalog import
and application, Excel/CSV/BibTeX/RIS export, TF-IDF relevance ranking, dedupe,
year and PDF filters, sortable/hideable columns, compact density, results
persistence, query history, retry, keyboard grid navigation, selection-scoped
exports, in-app abstract lookup, journal indexing column.

Effort: **S** ≤ ½ day · **M** 1–2 days · **L** 3+ days.

---

## Shipped in this cycle

These came from the direct request and are implemented and verified:

| Capability | Detail |
|---|---|
| **In-app abstract** | Context menu → *Abstract*, and a *Abstract* button in the details dialog. Crossref first (`abstract_for_doi`, JATS→text with DOI echo-check), then OpenAlex (`abstract_from_openalex_doi`, inverted-index reconstruction); records without a DOI use a title search guarded by normalized-title equality. Measured: 1,878 chars in 1.9 s; a genuine miss returns `null` in 0.9 s and the dialog says so. |
| **Index column** | Replaced the local relevance score with journal indexing labels: `SJR Q1`, `SINTA S2`, `SCOPUS`, `DOAJ`, or blank when unknown. Derived by `catalog::indexing_labels` (Rust) mirrored in `indexingLabels` (JS). Sortable; blank means *unknown*, never *unranked*. |

---

## 1. Citation management

| # | Capability | Why | Effort | Data |
|---|---|---|---|---|
| 1.1 | **CSL-JSON export** (`scholar_citations.json`) | The format Zotero/Mendeley/Paperpile/Pandoc import; one drag instead of re-parsing BibTeX. `CitationRecord` already holds every CSL field. | S | exists |
| 1.2 | **Persistent enrichment cache keyed by DOI** | Second export is instant and works offline; today every export re-resolves all records against Crossref, which is why 200-row sets are slow and hit rate limits. Key with existing `normalize_doi`. | M | exists |
| 1.3 | **DOI-aware dedupe + conflict report** | `deduplicate_articles` keys only on canonical link and exact normalized title, so publisher-page and repository copies of one paper both survive. Add DOI as a third key; report "N collapsed (k by DOI)". | M | exists |
| 1.4 | **Stable, persisted BibTeX keys** | `bibtex_key` rebuilds its key set per call, so adding one paper renumbers later keys and breaks `\cite{}` in a written manuscript. Persist key↔DOI. | S | exists |
| 1.5 | **PDF attachment paths in BibTeX** (`file = {...}`) | Downloaded PDFs arrive attached to the reference instead of as anonymous files. Must be a **separate writer**; editing the frozen path breaks gate4. | S | exists |
| 1.6 | **Manual metadata override** | Crossref misses leave a review note instead of authors. Let the user correct title/authors/year/journal/DOI; the override wins at export. | M | exists |
| 1.7 | **Add records by pasting DOIs or a .bib** | Scholar never returns everything; a reading list already exists. Resolve each DOI through `CrossrefClient::lookup` and merge via `dedupe` so re-adds don't duplicate. | M | exists (DOI) / new dep (.bib) |
| 1.8 | **Copy formatted citation (APA/IEEE/Vancouver)** | One right-click for a reference in an email or slide. Needs a small formatter — no CSL engine present. | S–M | exists |
| 1.9 | **"Cited by" count column** | The one metric researchers scan for, and Scholar's own count is a single already-fetched field. Crossref returns `is-referenced-by-count` with **no URL change** (verified: 3142 for `10.1371/…0185809`). | S | exists |
| 1.10 | **Retraction / correction badge** | Highest-consequence metadata check the app lacks. Crossref `updated-by[].type = "retraction"` verified live (Retraction Watch + publisher sources); OpenAlex `is_retracted` is free in the payload already downloaded. | M | fetch (free) |
| 1.11 | **Surface volume/issue/pages/publisher** | Already parsed into `CrossrefRecord` (`crossref.rs:50-55`), used only by `citation_from_record`, then dropped. Zero new network. | S | exists |
| 1.12 | **Funding / grant acknowledgements** | Crossref `funder` (verified: NWO / 840.11.001). Useful for compliance and "who paid for this". | S | fetch |
| 1.13 | **Structured authors + ORCID** | `Article.authors` is Scholar's raw display line and cannot be sorted or trusted; Crossref returns given/family/ORCID and `apply_record` discards it. | M | exists (add `ORCID` to select) |

## 2. Literature-review workflow

| # | Capability | Why | Effort | Data |
|---|---|---|---|---|
| 2.1 | **Screening decisions** (include/exclude/maybe + coded reason) | The single most-wanted systematic-review feature. Needs somewhere to put per-record state — `Article` has no user fields and `AppState` holds one flat `Vec`. | M | exists |
| 2.2 | **Notes and tags per record** | Holds the *thinking* about a paper, not just its metadata; tags double as filters. Same sidecar as 2.1. | M | exists |
| 2.3 | **PRISMA flow counts** | identified → duplicates removed → screened → excluded (by reason) → included, from `duplicates_removed` / `invalid_articles_removed` plus 2.1. | M | exists |
| 2.4 | **Re-score stored results against a new query** | `score_articles` runs once inside `scrape_scholar`; after screening, the useful question changes. Cheap: call it again synchronously. | S | exists |
| 2.5 | **Near-duplicate report** | Exact title dedupe misses "X" vs "X: a systematic review". `ranking::tokenize` + `cosine_similarity` are already in the repo; human confirms merges. | M | exists |
| 2.6 | **Search diff vs saved baseline** | "What's new since last month?" — compare by DOI/title key, badge and count the additions. | M | exists |
| 2.7 | **Append mode (union queries)** | A review needs synonym variants; today each search replaces the set. Branch on `state.set_articles(Vec::new())`, merge via dedupe, record per-record provenance. | M | exists |
| 2.8 | **Saved filter expressions** | "Q1 + PDF + year ≥ 2020 + tag=theory" as a named filter instead of re-clicking three controls. | M | exists |
| 2.9 | **Screening log export** | Audit trail: title, DOI, decision, reason, timestamp, source query, rank at decision time. New file type → no parity collision. | S | exists |
| 2.10 | **Metadata-only refresh** | `check_metadata_pdfs` always enriches *and* validates every PDF, so refreshing ranks on 200 records costs minutes of needless network. The enrich half is already isolated. | S | exists |
| 2.11 | **Excel export formatting** | Frozen header, autofilter, column widths, real hyperlinks, Q1/Q2 colouring. `save_to_excel` is the one export surface the parity gate does not touch. | S | exists |
| 2.12 | **Grouped worksheets / summary sheet** | Per-year, per-journal or per-status sheets plus a counts summary, instead of manual splitting. | M | exists |
| 2.13 | **Year distribution in the summary bar** | "1998–2024, 60 % since 2020" — cheapest idea here, high perceived value. | S | exists |
| 2.14 | **Journal-level aggregation export** | Group by journal with counts, ISSN, best quartile, mean year — "where does this field publish?" | M | exists |
| 2.15 | **Unranked-journal gap report** | Turns "unknown" into a worklist: every journal with no catalog match, with ISSN and hit count, so the user can source the missing rows. | S | exists |
| 2.16 | **Journal metric trend** | `JournalCatalog` retains every year; "Q1 2019 → Q2 2023" is a presentational hop, and it's the context promotion committees weigh. | M | exists |
| 2.17 | **Catalog match provenance** | `JournalRank.matched_by` distinguishes ISSN from exact-title fallback, but `apply_rank` drops it. A Q1 earned by title match looks identical to one earned by a validated ISSN. | S | exists |

## 3. Sharing and interchange

| # | Capability | Why | Effort | Data |
|---|---|---|---|---|
| 3.1 | **Project file (`.gssproj`)** | One file with results, decisions, notes, tags, query history, catalog snapshot — emailable to a co-author. Natural container for 2.1/2.2/2.7; state currently lives in per-machine `%APPDATA%`. | M | exists |
| 3.2 | **Self-contained HTML reading list** | One `.html` a co-author opens with no app: title, authors, year, rank, DOI link, PDF button. | S | exists |
| 3.3 | **Clipboard bibliography (Markdown/plain)** | Select 12 rows → paste into Slack/Confluence/email. `selection` and `copyText` already exist. | S | exists |
| 3.4 | **Round-trip import of own exports** | A co-author annotates the CSV and sends it back; merge by DOI/title. `read_csv`/`read_xlsx` machinery is directly reusable. | M | exists |
| 3.5 | **Grouped bibliography export** | One document with a heading per year/journal/tag — a related-work skeleton, not a flat list. | S | exists |

## 4. Discovery expansion

| # | Capability | Why | Effort | Data |
|---|---|---|---|---|
| 4.1 | **Snowballing (citation chasing)** | Seed a search from a paper's references and citing works — the standard complement to keyword search, impossible today. OpenAlex `referenced_works` / `filter=cites:` through the existing `PdfFetcher` + `require_public_url` seam. | L | fetch (free) |
| 4.2 | **arXiv / preprint linking** | Shows the free full text of a paywalled paper and whether versions differ. `10.48550/arXiv.*` DOIs match directly; else arXiv Atom API. | M | fetch (free) |
| 4.3 | **License / OA status column** | "Can I reuse this figure?" Crossref `license[]` + OpenAlex `open_access.oa_status` (gold/green/bronze/hybrid/closed). The license is parsed today only to admit PDF candidates. | S | exists |
| 4.4 | **PMID / PMCID links** | One click to PubMed and to free full text in PMC, from `ids.pmid`/`ids.pmcid` in the payload already fetched. | S | exists |
| 4.5 | **DOAJ / SCOPUS coverage** | Both are indexing signals; SCOPUS has no free bulk dataset, so label only when the record proves membership (source URL marker). Partly done by the new Index column. | M | fetch |
| 4.6 | **PubMed / Europe PMC / Semantic Scholar integration** | Additional metadata and citation graph; needs a POST seam for Semantic Scholar batch (the current trait is GET-only). | L | fetch |
| 4.7 | **Fuzzy title fallback for identity** | `lookup_title` requires an exact normalized match, so slightly-rewritten titles never resolve. `cosine_similarity` is already available for scoring candidates. | M | exists |

## 5. Interface and interaction

| # | Capability | Why | Effort |
|---|---|---|---|
| 5.1 | **Keep last good results until a replacement search succeeds** | `startSearch()` empties `storedArticles` immediately, so a failure destroys the working context. Keep it visible, marked "Searching…". | M |
| 5.2 | **One-click filter reset** | The no-match empty state *tells* users to clear filters but offers no button; add chips + "Clear result filters". | S |
| 5.3 | **Separate "results are for…" from "search setup"** | `#appliedYear` recomputes from editable fields, though editing them does not re-filter. Show an immutable "Results for … · years …" line. | M |
| 5.4 | **Save As / overwrite confirmation on export** | Fixed filenames with `File::create` silently replace earlier exports. Add a save dialog and remember the last destination. | M |
| 5.5 | **Explain score vs rank where they appear** | "Rel.", "Rank" and "Index" mean different things; the explanation is buried in the details disclaimer. | S |
| 5.6 | **Distinguish "Open PDF" from "Show in folder"** | The dialog button says "Open PDF" but `revealRecordPdf` only selects the file in Explorer. Needs an OS open-file command. | M |
| 5.7 | **Feedback for copy/open actions** | `copyText`, `openUrl` and reveal handlers swallow failures and confirm nothing. | M |
| 5.8 | **Keyboard-operable sorting + reset to Scholar order** | Headers are click-only and alternate asc/desc with no way back to source order. | S |
| 5.9 | **Preview panel / split view** | Read an abstract or details without a modal covering the table. | M |
| 5.10 | **Batch operations** | Multi-select already exists; hook it to bulk tag, bulk decide, bulk reveal. | M |
| 5.11 | **Theming / dark mode** | Single hard-coded light palette; no `prefers-color-scheme` handling. | M |

## 6. Reliability and operations

| # | Capability | Why it matters | Effort |
|---|---|---|---|
| 6.1 | **Logging** | None exists. `emit` discards errors (`let _ = app.emit`), `save_settings` is `.catch(() => {})`, and `get_json` collapses transport/HTTP/oversize/JSON failures into `None` — a user report gives nothing reproducible. | M |
| 6.2 | **Panic hook + RAII latch guard** | Only `search_scholar` wraps anything in `catch_unwind`, and not the client construction. A panic on any other worker returns **before** `clear_running`, leaving `is_running()` `Some(..)` forever — the app is bricked until restart. | M |
| 6.3 | **Diagnostics bundle** | `ExtractionResult.diagnostic` is populated (`skipped_missing_title=N`) but never shown or persisted. Export versions, settings, last result, log tail. | S/M |
| 6.4 | **Report partial provider failure** | `enrich_error` is hardcoded `""`, so the UI's "Check finished with errors" branch is dead code and a total Crossref outage reports as success. | S |
| 6.5 | **Typed backend↔UI errors** | The UI branches on message text (`startsWith("No file found")`, `replace("Export failed: ", "")`) and sentinel strings; any wording change silently breaks a branch. | M |
| 6.6 | **`--doctor` preflight** | Check write access, disk space, network reachability, WebView2 version before blaming the user. | S |
| 6.7 | **Operation audit trail** | No record of what ran when; useful for bug reports and for 2.9. | S |
| 6.8 | **HTTP timeouts on both clients** | `ReqwestCrossref` and `ReqwestPdfFetcher` build clients with no `.timeout(...)`; only `ReqwestFetcher` (Scholar) sets one. | S |
| 6.9 | **Per-host politeness budget** | PDF and Crossref requests fire back-to-back with no pacing. | S |
| 6.10 | **Honour `Retry-After`; retry 429** | Scholar 429 is never retried; `max_retries=1`, constant 2 s backoff, `Retry-After` ignored. | S |
| 6.11 | **Per-source retry policy** | Crossref/OpenAlex/PDF have no retry at all. | S |
| 6.12 | **Don't record transport failure as "no PDF"** | A network error currently produces a factual-looking `unavailable` verdict, which the user then trusts. | S |
| 6.13 | **Proxy / UA / cookie configuration** | No way to route through a proxy or set a UA; `no_proxy()` is hardcoded. | M |
| 6.14 | **SSRF guard is inert in the shipped app** | `ReqwestPdfFetcher` buffers with `bytes().unwrap_or_default()` and `redirect::none()`, bypassing the streaming/atomic contract `pdf.rs` carefully implements. | M |
| 6.15 | **Bound the page count** | No upper limit on `pages`; a typo requests thousands of pages. | S |
| 6.16 | **Persist the imported ranking catalog** | Re-import is required every launch; the catalog lives only in memory. | S |
| 6.17 | **Settings schema version + reset** | No migration marker; corrupt state is silently discarded with total loss and no notice. | S |
| 6.18 | **Read-only / roaming profile handling** | `%APPDATA%` is assumed writable; failures are silent. | S |
| 6.19 | **Single-instance guard** | Two launches fight over the same sidecar files. | S |
| 6.20 | **Atomic exports** | CSV/BibTeX/RIS use `File::create` — a crash mid-write leaves a truncated file that looks valid. | S |
| 6.21 | **Checkpoint and resume** | A crash or close loses the whole run; finished work is redone from scratch. | M |
| 6.22 | **Cancel for import; faster cancel elsewhere** | Cancel is a no-op for import and slow elsewhere. | S |
| 6.23 | **Watchdog on the running latch** | If a worker dies, the latch never clears (see 6.2). A timeout + forced reset beats a restart. | S |
| 6.24 | **Work queue instead of one-shot latch** | No concurrent or queued operations. | M |
| 6.25 | **Graceful shutdown + temp-file sweep** | No handling for in-flight work on close; `.tmp` files can be orphaned. | S |
| 6.26 | **Download-batch disk preflight** | Free-space and path checks before starting; IO errors currently surface with misleading names. | S |
| 6.27 | **On-disk HTTP cache** | Repeated identical metadata lookups re-hit the network. | M |
| 6.28 | **Fetch Scholar responses compressed** | No `Accept-Encoding`; bandwidth and latency left on the table. | S |

## 7. Platform integration

| # | Capability | Why | Effort |
|---|---|---|---|
| 7.1 | **Window geometry persistence** | Size/position/maximized/monitor restored on launch. Needs `tauri-plugin-window-state` or 4 `UiSettings` fields plus `core:window:*` capabilities. | S |
| 7.2 | **Native menu bar** | Current "File" menu is HTML; no OS menu, so no standard accelerators or Discoverability. | M |
| 7.3 | **System tray** | Keep a long check/download running when the window is closed. `tauri` currently has `features = []`, so `tray-icon` isn't even compiled. | M |
| 7.4 | **Single-instance enforcement** | Second launch focuses the first instead of opening a competing window. | S |
| 7.5 | **File drag-and-drop** | Drop a ranking CSV/`.bib` onto the window. Needs `onDragDropEvent` + capability. | S |
| 7.6 | **Paste-to-search** | Paste a DOI/title from the clipboard and search immediately. | S |
| 7.7 | **File associations** | Double-click a `.gssproj`/`.bib` to open in the app; needs `bundle.fileAssociations`. | M |
| 7.8 | **Auto-update** | No updater plugin, no manifest; users must find releases manually. | L |
| 7.9 | **Code signing** | Unsigned exe triggers SmartScreen; no signing step anywhere in CI. | M (ops) |
| 7.10 | **Ship the NSIS installer from CI** | The release job copies a bare `gss-app.exe` and never runs `tauri build`, so the configured bundle target is unused in releases. | S |
| 7.11 | **Installer options** | Per-user vs per-machine, install-dir choice, WebView2 bootstrapper mode. | M |
| 7.12 | **Uninstall cleanup** | `%APPDATA%` data (settings, results, logs) survives uninstall. | S |
| 7.13 | **Dark mode following the OS** | No `prefers-color-scheme` or `color-scheme`; single palette. | M |
| 7.14 | **High-DPI / multi-monitor correctness** | No DPI handling; geometry is monitor-unaware. | M |
| 7.15 | **Localization / i18n** | All strings are inline English literals. | L |
| 7.16 | **Screen-reader polish** | `role`/`aria` added in 3.2.0, but no live announcements for selection changes and the menus are `div`-based. | M |
| 7.17 | **Portable mode** | No "store settings next to the exe" option. | S |
| 7.18 | **Open .bib/.csv with the app** | Shell integration beyond the picker. | M |
| 7.19 | **Keyboard accelerator coverage** | Only Esc, Ctrl+E, Ctrl+A, Ctrl+Shift+A, Enter, arrows. No Ctrl+F (filter), Ctrl+S (export), Ctrl+O (import). | S |

## 8. Export and data interchange

| # | Capability | Why | Effort | Parity risk |
|---|---|---|---|---|
| 8.1 | **Excel: freeze header + autofilter** | Column names vanish when scrolling 20 columns; `rust_xlsxwriter` supports both. | S | none |
| 8.2 | **Excel: column widths + wrapped text** | Default-width worksheet is unreadable. | S | none |
| 8.3 | **Excel: real hyperlinks** | Click a DOI or PDF straight from the workbook (native hyperlinks, not `HYPERLINK()` formulas). | M | none |
| 8.4 | **Excel: rank and PDF-status colouring** | Scan Q1 vs Q4 and downloaded vs unknown at a glance. | S | none |
| 8.5 | **Additional formats** | JSON, Markdown table, HTML, TSV, EndNote `.enw`, Docx bibliography, plain-text APA/MLA/IEEE. | S–M | new writers only |
| 8.6 | **Citation style selection** | Pick the style at export instead of post-processing. | M | none |
| 8.7 | **Export only selected columns** | Column hiding in the UI is not honoured by the export. | S | CSV gated — needs a separate writer |
| 8.8 | **Export the details text** | The dialog's full record text as a companion file. | S | new writer |
| 8.9 | **Clipboard copy of citations** | See 3.3. | S | none |

---

## Recommended next slice

Highest value-to-effort, in dependency order — this sequence turns the tool from a
scraper into something a reviewer keeps open all day:

1. **1.9 cited-by count** + **4.3 license/OA status** + **1.11 venue fields** — three
   fields from payloads already downloaded. No new requests.
2. **2.11 Excel formatting** — the only export surface free of parity constraints.
3. **2.10 metadata-only refresh** — split the existing command; makes rank work iterative.
4. **6.1 logging** + **6.2 panic-safe latch** — without these, any failure is unreportable
   and can brick the app.
5. **2.1 screening decisions** + **2.2 notes/tags** — requires the per-record state
   container that 3.1 (project file) formalises.
6. **1.2 DOI cache** — makes everything network-bound faster and kinder to Crossref.
