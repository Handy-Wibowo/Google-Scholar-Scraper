# gss-core CONTRACTS — binding behavior ported from the Python oracle (v2.2.2)

Source of truth: `E:/ai_project/google-scholar-scraper/src/src/google_scholar_scraper/*`.
Each section lists the invariant + Python anchor. Parity gates in `rust/tools/parity_check.py`
diff both implementations; zero diffs required.

## Data model (`models.py`)

- `Article`: frozen, 20 fields, `pdf_status` default `"unknown"`.
  Order: title, authors, link, relevance_score (float|None), doi, journal, issns (tuple),
  publication_year, sjr_quartile, sjr_year, sjr_category, sjr_source_url, sinta_rank,
  sinta_year, sinta_source_url, pdf_url, pdf_source_url, pdf_status, pdf_detail, pdf_local_path.
- `to_export_row()` → dict in EXACT order = `EXPORT_COLUMNS` (20 cols):
  Title, Authors, Link, Relevance Score, DOI, Journal, ISSNs, Publication Year,
  SJR Quartile, SJR Year, SJR Category, SJR Source, SINTA Rank, SINTA Year,
  SINTA Source, PDF Status, PDF URL, PDF Source, PDF Detail, PDF Local Path.
  None → "" (Relevance Score, Publication Year, SJR Year, SINTA Year); ISSNs joined `"; "`.
- PDF statuses exactly: `"available" | "unavailable" | "unknown"`.
  "unavailable" = checked sources found nothing; errors/unverified stay "unknown".
- `ExtractionStatus` (str-enum, 8): SUCCESS, PARTIAL_SUCCESS, NO_RESULTS, RATE_LIMITED,
  BLOCKED, NETWORK_ERROR, PARSING_ERROR, CANCELLED.
- `ExtractionResult`: status, articles, requested_pages, successful_pages=0,
  failure_page=None, message="", diagnostic="", duplicates_removed=0,
  invalid_articles_removed=0; `has_articles` property.
- `CitationRecord` (export-only): title, authors (verified structured names,
  "Given Family" deposit order, never parsed from the Scholar line), year, container,
  entry_type, doi, issns, volume, issue, pages, publisher, url, source, verified,
  fallback_authors_line (verbatim Scholar display line), warnings. No PDF fields.

## Text identity (`models.py` + `dedupe.py` + `pdf_service.py`)

- `normalize_title_key`: NFKD → drop combining marks (unicodedata.combining != 0) →
  casefold → remove runs of `[^0-9a-z]+`. NOTE: non-ASCII letters (Arabic, CJK) are
  REMOVED — the key is Latin-only by construction. Gate 1 parity: zero mismatches.
- `normalize_issn`: strip whitespace, uppercase, `^(\d{4})-?(\d{3}[\dX])$`; canonical
  `1234-567X`; checksum `sum(digit*(8-i) for i,d in enumerate(first7))`, check =
  (11 - total%11) % 11, 10→X; mismatch → "".
- `normalize_issn_list`: normalize each, drop invalid, dedupe, sort ascending.
- `normalize_doi`: strip; strip exactly one of prefixes (case-insensitive match on
  lowercased value, slice from original): https://doi.org/, http://doi.org/,
  https://dx.doi.org/, http://dx.doi.org/, doi:; then trim + casefold. No fabrication:
  bare text without prefix is casefolded as-is (test pins "not a doi" → "not a doi").
- `_safe_filename` (pdf_service.py:708): NFKC → replace `[<>:"/\\|?*\x00-\x1f]` with " "
  → collapse `\s+` → strip → rstrip(".") → strip → cut to 120 chars → rstrip(" .") →
  empty → "paper" → if head before first dot in WINDOWS_RESERVED (CON PRN AUX NUL
  COM1-9 LPT1-9) → prefix "_".
- Python `casefold()` semantics needed beyond `to_lowercase()`: ß/ẞ→ss, ﬀ→ff, ﬁ→fi,
  ﬂ→fl, ﬃ→ffi, ﬄ→ffl, ﬆ/ﬅ→st, ς→σ (implemented as `text::CaseFold`).

## Dedupe (`dedupe.py`) — ported 1:1, all 21 Python tests green in Rust

- `normalize_display_text`: collapse `\s+` → single space, strip. Unicode untouched.
- `normalize_link`: trim; empty → ""; urlsplit; missing scheme OR netloc → original
  trimmed link; else rebuild with lowercased scheme+netloc, fragment dropped.
- `normalize_article`: whitespace-collapse title/authors/journal/sjr_category/pdf_detail;
  link/sjr_source_url/sinta_source_url/pdf_url/pdf_source_url via normalize_link;
  doi via normalize_doi; issns via normalize_issn_list; sjr_quartile/sinta_rank
  uppercased; pdf_status via normalize_pdf_status (invalid → unknown);
  `available` with NO pdf_url AND NO pdf_source_url → downgraded to "unknown";
  pdf_local_path stripped; relevance_score/years passed through.
- `canonical_link`: normalize_link then require scheme ∈ {http, https} + netloc, else "".
- `is_valid_article`: normalize_display_text(title) non-empty.
- `deduplicate_articles`: normalize each; invalid (empty title) → invalid_removed++;
  duplicate lookup by canonical link first, then title_key (`_comparison_text_key` =
  NFKC → normalize_display_text → casefold); first occurrence keeps position; merge
  into existing; re-index merged link+title after merge.
- `_merge_articles`: scalar fields keep first non-empty (title/authors/link/doi/journal/
  pdf_local_path); issns = normalize_issn_list(union); publication_year keeps existing
  unless None; relevance_score ALWAYS existing (first). Ranking fields merged as
  BUNDLES (`_merge_rank_bundle`): incoming empty rank → keep existing; existing empty →
  take incoming; existing undated (year None) → take incoming; incoming undated → keep
  existing; both dated → higher year wins, tie → existing. PDF fields merged as ONE
  bundle (`_merge_pdf_bundle`): claim strength = unanchored −1, unknown 0,
  unavailable 1, available 2 (anchor = pdf_url or pdf_source_url); stronger wins;
  tie → the one WITH anchor, else incoming.

## Ranking (`ranking.py`) — ported 1:1, all 17 Python tests green in Rust

- `tokenize`: NFKC → casefold → `\w+` (unicode) tokens.
- `article_tokens`: title×3 + authors (title tokens first, repeated 3×).
- IDF over corpus [query, *docs]: `ln((N+1)/(df+1)) + 1`.
- TF-IDF vector: `(count/total) * idf[t]`.
- Cosine similarity; score = clamp(cos*100, 0, 100), rounded to 1 decimal
  (Python `round(x,1)`; round-half-even at exact binary edges unreachable here —
  pinned by parity in later gates).
- Empty query → all 0.0; empty docs → [].
- `rank_articles`: disabled → passthrough untouched; else stable sort by
  (-score, original index).

## UI string tables (`ui/tkinter_app.py`) — verbatim, incl. en-dash and ellipsis

- APP_TITLE "Google Scholar Scraper"; window title f"{APP_TITLE} v{__version__}" (v2.2.2 → Rust v3.0.0).
- OPERATION_HEADINGS: search="Searching Google Scholar", check="Checking metadata and
  public PDFs", import="Importing journal ranking records",
  download="Downloading selected public PDFs", citations="Exporting citations".
- DEFAULT_EXPORT_NAMES: scholar_articles.xlsx/.csv/.bib/.ris.
- PDF_FILTERS: "All", "Public PDF available", "No public PDF found", "Unknown".
- Year options (current year Y): "Any time"(None,None), f"Since {Y}"(Y,None),
  f"Since {Y-1}"(Y-1,None), "Last five years"(Y-4,None), "Custom range"(None,None).
- `year_range_label`: both → f"Years: {f}–{t}" (EN-DASH), from → f"Years: since {f}",
  to → f"Years: through {t}", none → "Years: any time".
- DETAIL_DISCLAIMER: 5-line journal/PDF caveat (tkinter_app.py:88-94), embedded verbatim
  in detail text.
- Window: default 1080x700, min 860x560, clamp to screen − (20,80); probe sizes
  860x560 / 880x620 / 1080x700, FIELD_MIN_WIDTH 300; dialogs: detail 820x520
  (min 520x340), citations 520x340 (min), both singleton-reuse.
- Status tones (5): ready/running/success/warning/error with full color tables
  (tkinter_app.py:99-107).
- Two messagebox paths ONLY: task_error, export error. Worker exceptions never surface
  tracebacks: search → synthetic NETWORK_ERROR result
  (message=f"Search failed: {ExcClass}.", diagnostic=ExcClass); tasks → task_error
  payload f"{ExcClass}: {exc}".
- Fresh queue + cancel_event per operation; running predicate = worker thread liveness;
  row identity = stored index iid `article-{i}` surviving filters; check/download write
  back by index; Excel/CSV exports synchronous on UI thread; citations = background
  task; cancelled citation export writes NO file.

## Port status (updated per step)

- Step 1 done: models/text/dedupe/ranking/url + Gate 1 parity zero-diff.
- Step 2 done: client.rs + parser.rs; Gate 2 parity: 10 parse jobs zero diffs.
- Step 3 done: pdf.rs + 73 tests (all classes of test_pdf_service.py ported 1:1).
- Step 4 done: crossref.rs, catalog.rs, exporters.rs; Gate 4 parity: csv+bib+ris byte-equal.
- Step 5 done: Tauri v2 shell (Gate B PASSED: window opens, WebView2 156.0.4314.58
  present, 100-row table handles filter+multi-select via iid identity; no iced
  fallback). Commands stream WorkerMessage events; shared AtomicBool cancel;
  `--packaging-smoke` + `--selfcheck-layout` headless modes.
- Step 6 done: parity harness gates 1/2/4/5 all zero-diff; live scholar.google.com
  page parses IDENTICALLY in both implementations (10/10 articles, titles+links
  byte-equal); offline E2E chain (extract→export→citations→GUI) verified.
- Step 7 done: release build (14.6 MB exe, cold start 19 ms), CI workflow
  (.github/workflows/rust.yml: test+clippy+fmt+smoke; release job on v3.* tags
  uploads exe + SHA256SUMS.txt), README + Python-README successor line.
- FINAL: 265 Rust tests green (mirroring 368 Python oracle tests), clippy -D
  warnings clean, fmt clean, all parity gates zero-diff.

## Client contract (`scraper/client.py`)

- BASE_URL https://scholar.google.com/scholar; headers: UA Chrome/126 (exact string in
  client.rs), Accept `text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8`,
  Accept-Language `en-US,en;q=0.9`. Python sets `trust_env=False` on its own session →
  Rust reqwest built WITHOUT system-proxy support.
- Params order: start=page*10, q, hl=en, as_sdt=0,5, then optional as_ylo/as_yhi;
  urlencode quote_plus (space→+).
- validate_year_range: each bound 1000–9999 else ValueError "Year must be a whole
  four-digit number (1000–9999)."; from>to → "From year must not be later than To year."
- Retry: attempts = max_retries+1 (default 2). Timeout → retry w/ backoff (2s default),
  final → NETWORK_ERROR "Network timeout while contacting Google Scholar." diagnostic
  exc class. ConnectionError/other RequestException → retry only if ConnectionError/
  Timeout; final → NETWORK_ERROR "Network error while contacting Google Scholar.".
  429 → return response (never retried). 502/503/504 → retry; after final attempt
  return response (page classifier handles it). Cancel during backoff → CANCELLED.
- scrape loop: per page — cancel check → progress "requesting" → fetch → progress
  "parsing" → parse → non-success → partial-or-status result; success → dedupe page →
  extend → dedupe ALL (counters accumulate) → successful_pages++ → progress
  "completed" → cancel check → pacing wait (1.5s default, cancel during wait →
  CANCELLED with preserved articles). No articles → NO_RESULTS "Google Scholar
  returned no results for this query.". Success message: "Extraction complete.
  Collected N articles." + " Removed D duplicate articles." then rank.
- PARTIAL_SUCCESS: articles non-empty → re-dedupe + rank; message "Extraction stopped
  early on page {failure_page}: {orig_message} Preserved {n} collected articles for
  review and export." + dup note; diagnostic = failed status value.
- CANCELLED: dedupe+rank preserved; "Extraction cancelled." + " Preserved {n} collected
  articles."; failure_page None.

