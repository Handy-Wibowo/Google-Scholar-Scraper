# Roadmap — phased delivery of the capability backlog

Companion to [`CAPABILITY-BACKLOG.md`](CAPABILITY-BACKLOG.md). The backlog lists
*what* could be built; this file says *in what order*, at *what version*, and
*what depends on what* — so any single phase can be picked up in isolation.

**Totals:** 109 capabilities · ≈119.5 person-days · 8 phases · 7 releases
(v3.3.1 → v4.0.0).

**Delivery status.** P0, P1, P2 and P3 shipped in full (P2 minus 1.6). P4 and P5
shipped partially, P6 partially, P7 partially; the remainder is listed per phase
below and is **not** implemented. Three backlog entries were found to be already
correct on inspection (6.12) or blocked by a parity contract (4.7).

Effort key (from the backlog): **S** ≈ ½ day · **M** ≈ 1.5 days · **L** ≈ 4 days.
These are estimates, not measurements.

---

## Phase table

| Phase | Theme | Release | Depends on | Items | S/M/L | Person-days |
|---|---|---|---|---|---|---|
| **P0** | Foundations | `v3.3.1` patch | — | 9 | 4/5/0 | **9.5** |
| **P1** | Free metadata | `v3.4.0` minor | P0 | 13 | 9/4/0 | **10.5** |
| **P2** | Export & interchange | `v3.5.0` minor | P1 | 20 | 13/7/0 | **17.0** |
| **P3** | Review state | `v3.6.0` minor | P0, P2 | 12 | 2/10/0 | **16.0** |
| **P4** | Interface polish | `v3.7.0` minor | P0 | 10 | 3/7/0 | **12.0** |
| **P5** | Reliability hardening | `v3.7.0` minor | P0 | 21 | 15/6/0 | **16.5** |
| **P6** | Platform integration | `v3.8.0` minor | P0 | 19 | 8/9/2 | **25.5** |
| **P7** | Discovery expansion | `v4.0.0` major | P1, P3 | 5 | 0/3/2 | **12.5** |
| | | | | **109** | **54/51/4** | **119.5** |

P4 and P5 ship together as v3.7.0 — they are independent of each other and of
P2/P3, so they can be done in either order or in parallel.

---

## P0 — Foundations · `v3.3.1`

**Shipped in full** — v3.3.1.

**Goal:** make every later phase debuggable and stop the app from wedging itself.

| # | Capability | Effort | Release type |
|---|---|---|---|
| 6.1 | Logging (file + per-operation span) | M | patch |
| 6.2 | Panic hook + RAII latch guard | M | patch |
| 6.4 | Report partial provider failure (`enrich_error` is hardcoded `""`) | S | patch |
| 6.5 | Typed backend↔UI errors instead of string matching | M | patch |
| 6.8 | HTTP timeouts on `ReqwestCrossref` / `ReqwestPdfFetcher` | S | patch |
| 6.16 | Persist the imported ranking catalog | S | patch |
| 6.17 | Settings schema version + reset path | S | patch |
| 1.2 | Persistent enrichment cache keyed by DOI | M | feature |
| 1.3 | DOI-aware dedupe + conflict report | M | feature |

**Why this is a patch for 6.x and a feature for 1.2/1.3:** the six-point items are
defect fixes (a panic can currently brick the running latch; a Crossref outage
reports as success). 1.2 and 1.3 add capability, so they justify the minor bump —
if you prefer a clean patch, ship only 6.x as `v3.3.1` and move 1.2/1.3 into P1.

**Exit criteria:** a forced worker panic clears the latch and logs a crash file; a
Crossref total outage surfaces "N lookups failed" instead of success; `cargo test`,
clippy, fmt and the four parity gates stay green.

---

## P1 — Free metadata · `v3.4.0`

**Shipped in full** (excluding 4.7, reverted) — v3.4.0.

**Goal:** spend zero extra network. Every item here reads payloads the app already
fetches, or performs one bounded lookup.

| # | Capability | Effort |
|---|---|---|
| 1.9 | "Cited by" count column | S |
| 1.10 | Retraction / correction badge | M |
| 1.11 | Surface volume/issue/pages/publisher | S |
| 1.12 | Funding / grant acknowledgements | S |
| 1.13 | Structured authors + ORCID | M |
| 4.3 | License / OA status column | S |
| 4.4 | PMID / PMCID links | S |
| 4.7 | Fuzzy title fallback for identity | M |
| 2.10 | Metadata-only refresh (split the fused check command) | S |
| 2.13 | Year distribution in the summary bar | S |
| 2.15 | Unranked-journal gap report | S |
| 2.16 | Journal metric trend | M |
| 2.17 | Catalog match provenance | S |

**Watch out:** new `Article` fields need `#[serde(default)]` or the restored-session
load silently discards everything. `EXPORT_COLUMNS` is byte-parity-frozen — new
columns go to Excel-only or a separate writer.

---

## P2 — Export & interchange · `v3.5.0`

**Shipped** except 1.6, which needs per-record user state (the P3 container) — v3.5.0.

**Goal:** make the output usable without post-processing.

| # | Capability | Effort |
|---|---|---|
| 8.1 | Excel: freeze header + autofilter | S |
| 8.2 | Excel: column widths + wrapped text | S |
| 8.3 | Excel: real hyperlinks | M |
| 8.4 | Excel: rank and PDF-status colouring | S |
| 8.5 | Additional formats (JSON, MD, HTML, TSV, ENW, Docx) | M |
| 8.6 | Citation style selection | M |
| 8.7 | Export only selected columns | S |
| 8.8 | Export the details text | S |
| 8.9 | Clipboard copy of citations | S |
| 1.1 | CSL-JSON export | S |
| 1.4 | Stable, persisted BibTeX keys | S |
| 1.5 | PDF attachment paths in BibTeX | S |
| 1.6 | Manual metadata override | M |
| 1.8 | Copy formatted citation (APA/IEEE/Vancouver) | M |
| 2.11 | Excel export formatting (bundle with 8.x) | S |
| 2.12 | Grouped worksheets / summary sheet | M |
| 2.14 | Journal-level aggregation export | M |
| 3.2 | Self-contained HTML reading list | S |
| 3.3 | Clipboard bibliography (Markdown/plain) | S |
| 3.5 | Grouped bibliography export | S |

**Parity:** `save_to_excel` is the only export surface the gate does not touch.
Anything that changes CSV/BibTeX/RIS bytes must be a **new writer**, not an edit.

---

## P3 — Review state · `v3.6.0`

**Shipped in full** — v3.6.0.

**Goal:** hold the reviewer's decisions, not just the metadata.

| # | Capability | Effort |
|---|---|---|
| 3.1 | Project file (`.gssproj`) — the container the rest need | M |
| 2.1 | Screening decisions | M |
| 2.2 | Notes and tags per record | M |
| 2.3 | PRISMA flow counts | M |
| 2.4 | Re-score stored results against a new query | S |
| 2.5 | Near-duplicate report | M |
| 2.6 | Search diff vs saved baseline | M |
| 2.7 | Append mode (union queries) | M |
| 2.8 | Saved filter expressions | M |
| 2.9 | Screening log export | S |
| 3.4 | Round-trip import of own exports | M |
| 5.10 | Batch operations (bulk tag/decide/reveal) | M |

**Start with 3.1.** Every other item here needs somewhere to put per-record state;
`Article` has no user-owned fields today.

---

## P4 — Interface polish · `v3.7.0`

**Shipped** 5.1, 5.2, 5.5, 5.8, 5.11. Not done: 5.3, 5.4, 5.6, 5.7, 5.9, 5.10.

| # | Capability | Effort |
|---|---|---|
| 5.1 | Keep last good results until a replacement search succeeds | M |
| 5.2 | One-click filter reset | S |
| 5.3 | Separate "results are for…" from "search setup" | M |
| 5.4 | Save As / overwrite confirmation on export | M |
| 5.5 | Explain score vs rank where they appear | S |
| 5.6 | Distinguish "Open PDF" from "Show in folder" | M |
| 5.7 | Feedback for copy/open actions | M |
| 5.8 | Keyboard-operable sorting + reset to Scholar order | S |
| 5.9 | Preview panel / split view | M |
| 5.11 | Theming / dark mode | M |

---

## P5 — Reliability hardening · `v3.7.0`

**Shipped** 6.20 only. 6.12 was already correct. The rest is open.

| # | Capability | Effort |
|---|---|---|
| 6.3 | Diagnostics bundle | M |
| 6.6 | `--doctor` preflight | S |
| 6.7 | Operation audit trail | S |
| 6.9 | Per-host politeness budget | S |
| 6.10 | Honour `Retry-After`; retry 429 | S |
| 6.11 | Per-source retry policy | S |
| 6.12 | Don't record transport failure as "no PDF" | S |
| 6.13 | Proxy / UA / cookie configuration | M |
| 6.14 | Fix the SSRF guard bypass in the shipped transport | M |
| 6.15 | Bound the page count | S |
| 6.18 | Read-only / roaming profile handling | S |
| 6.19 | Single-instance guard | S |
| 6.20 | Atomic exports | S |
| 6.21 | Checkpoint and resume | M |
| 6.22 | Cancel for import; faster cancel elsewhere | S |
| 6.23 | Watchdog on the running latch | S |
| 6.24 | Work queue instead of one-shot latch | M |
| 6.25 | Graceful shutdown + temp-file sweep | S |
| 6.26 | Download-batch disk preflight | S |
| 6.27 | On-disk HTTP cache | M |
| 6.28 | Fetch Scholar responses compressed | S |

---

## P6 — Platform integration · `v3.8.0`

**Shipped** 7.1 and 7.10. The rest needs new plugins, build features, or out-of-band work.

| # | Capability | Effort | Lead time |
|---|---|---|---|
| 7.1 | Window geometry persistence | S | — |
| 7.2 | Native menu bar | M | — |
| 7.3 | System tray | M | — |
| 7.4 | Single-instance enforcement | S | — |
| 7.5 | File drag-and-drop | S | — |
| 7.6 | Paste-to-search | S | — |
| 7.7 | File associations | M | — |
| 7.8 | Auto-update | L | needs hosted manifest |
| 7.9 | Code signing | M | **certificate procurement** |
| 7.10 | Ship the NSIS installer from CI | S | — |
| 7.11 | Installer options | M | — |
| 7.12 | Uninstall cleanup | S | — |
| 7.13 | Dark mode following the OS | M | — |
| 7.14 | High-DPI / multi-monitor correctness | M | — |
| 7.15 | Localization / i18n | L | translation effort |
| 7.16 | Screen-reader polish | M | — |
| 7.17 | Portable mode | S | — |
| 7.18 | Open .bib/.csv with the app | M | — |
| 7.19 | Keyboard accelerator coverage | S | — |

**Do 7.10 first** — it is S-sized and removes the manual `tauri build` step every
release currently needs. Most of this phase requires touching `Cargo.toml`
features, `tauri.conf.json`, and `capabilities/default.json` together; a JS-only
change cannot reach any of it.

---

## P7 — Discovery expansion · `v4.0.0`

**Shipped** 1.7 and 4.1. Not done: 4.2, 4.5, 4.6.

| # | Capability | Effort |
|---|---|---|
| 4.1 | Snowballing (citation chasing) | L |
| 4.2 | arXiv / preprint linking | M |
| 4.5 | DOAJ / SCOPUS coverage | M |
| 4.6 | PubMed / Europe PMC / Semantic Scholar | L |
| 1.7 | Add records by pasting DOIs or a .bib | M |

**Why this is a major version:** these are not additive. Snowballing *creates*
result sets from other result sets (changing what "a search" means), and the new
metadata fields change the persisted sidecar and every export surface. The
Semantic Scholar batch API also needs a POST seam — the current transport trait is
GET-only.

---

## Working one phase at a time

Each phase above is self-contained. To start one:

1. Copy its row set into a fresh plan — the backlog entry has the rationale and the
   data-availability note; this file has the dependency and the target version.
2. Satisfy the dependency first: P1 needs P0's logging; P3 needs P0+P2; P7 needs
   P1+P3.
3. Apply the two standing constraints from the backlog:
   - new `Article` fields need `#[serde(default)]`, or `load_results` silently
     discards the saved session;
   - CSV/BibTeX/RIS are byte-compared against the Python oracle — new output goes
     to a new writer or Excel-only.
4. Bump the version, tag, let CI build, and build the NSIS installer locally
   (until 7.10 ships).

## Calendar projections

| Scenario | Calendar |
|---|---|
| 1 dev, full-time | ≈120 working days ≈ 5.7 months |
| 1 dev + 25 % review/QA | ≈149 days ≈ 7.1 months |
| 2 devs in parallel | ≈72 days ≈ 3.4 months |
| 1 dev at 50 % | ≈239 days ≈ 11.4 months |

Per-phase, single dev full-time: P0 ≈ 2 weeks · P1 ≈ 2 weeks · P2 ≈ 3.5 weeks ·
P3 ≈ 3 weeks · P4 ≈ 2.5 weeks · P5 ≈ 3.5 weeks · P6 ≈ 5 weeks · P7 ≈ 2.5 weeks.

## Suggested first cut

**P0 + P1 = 20 person-days (~4 weeks).** That ships cited-by counts, retraction
badges, license/OA status, venue fields, ORCID, PMID links, journal trends, and
the metadata-only refresh — all from data already downloaded — plus the logging and
panic-safety that make every later phase debuggable.
