# Google Scholar Scraper

A Windows desktop app for extracting, ranking, reviewing, and exporting Google
Scholar search results.

Google Scholar Scraper V2 is a local research utility for collecting Scholar
result pages, removing duplicates, applying optional lexical relevance ranking,
attaching journal ranking and public-PDF metadata, and exporting reviewed results
to Excel or CSV. It is source-available for noncommercial use under the PolyForm
Noncommercial License 1.0.0, with separate commercial licensing available.

![Version](https://img.shields.io/badge/version-2.2.0-blue)
![Latest Release](https://img.shields.io/badge/latest%20release-2.0.1-green)
![Python](https://img.shields.io/badge/python-3.10%2B-3776AB)
[![CI](https://github.com/MahdiNavaei/Google-Scholar-Scraper/actions/workflows/ci.yml/badge.svg)](https://github.com/MahdiNavaei/Google-Scholar-Scraper/actions/workflows/ci.yml)
[![Windows Build](https://github.com/MahdiNavaei/Google-Scholar-Scraper/actions/workflows/build-windows.yml/badge.svg)](https://github.com/MahdiNavaei/Google-Scholar-Scraper/actions/workflows/build-windows.yml)
![License](https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-lightgrey)

> **V2.2.0 is an unpublished feature-branch preview.** The version badges above
> report the source tree, not a shipped build. No V2.2.0 release or tag has been
> published and no V2.2.0 binary is available for download — you can only run it
> from a source checkout, or from a build you produce locally yourself. The
> year-filter, journal-ranking, and public-PDF work lives on the feature branch
> [`feature/scholar-year-filters`](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/tree/feature/scholar-year-filters)
> of the fork [Handy-Wibowo/Google-Scholar-Scraper](https://github.com/Handy-Wibowo/Google-Scholar-Scraper).
> The newest published release is still **V2.0.1** from the upstream repository —
> use the links under [Installation Options](#installation-options) for that, or
> [build V2.2.0 from source](#building-windows-artifacts) yourself. See
> [Release And Documentation](#release-and-documentation) for the current status.

![Google Scholar Scraper V2 desktop interface](docs/assets/google-scholar-scraper-v2.png)

Google Scholar Scraper is not affiliated with Google. It does not solve CAPTCHA,
bypass access controls, rotate proxies, use account credentials, or use stealth
browser automation.

## Why V2?

The original goal is simple: make small-scale academic result collection easier
without turning the project into an unsafe crawler or a heavyweight data
platform. V2 focuses on a dependable desktop workflow, explicit failure states,
local-only ranking, deterministic exports, and Windows packaging.

What matters in practice:

- No API key is required.
- No LLM or external AI service is used.
- Smart Relevance Ranking runs locally.
- Journal ranking tables are imported from files you supply, not bundled or fetched.
- Public PDF checks report an explicit state instead of guessing.
- Rate limits, blocked pages, parser errors, and cancellations are explicit.
- Partial results are preserved when extraction stops early.
- Reviewed results export to Excel and CSV, including the ranking and PDF columns.
- Windows distribution supports both portable and installer-based workflows.

## Core Features

- Search Google Scholar by query and page count.
- Review collected articles in a responsive Tkinter/ttk desktop table.
- Open result links directly from the table.
- Enable or disable local relevance ranking.
- Import SCImago SJR and SINTA ranking tables from CSV or XLSX files.
- Attach journal ranking metadata (SJR quartile, SINTA rank) and public-PDF state to results.
- Filter by public-PDF status and download only the papers you select.
- Preserve usable partial results when later pages fail or the user cancels.
- Export reviewed results to Excel or CSV.
- Run from source, portable Windows ZIP, or Windows installer artifacts.

## Smart Relevance Ranking

Smart Relevance Ranking is optional and runs locally. It compares the query
with article title and author/source metadata using deterministic token
weighting and TF-IDF-style cosine scoring.

The score is shown as `Relevance` in the UI and `Relevance Score` in exports.
It is a review aid, not a semantic understanding score, confidence value,
citation-quality metric, or scientific assessment of a paper.

Ranking does not use:

- LLMs.
- External AI APIs.
- Embedding services.
- Model downloads.
- GPU acceleration.

## Publication-Year Filters

Choose **Any time**, **Since the current year**, **Since the previous year**,
**Last five years**, or **Custom range** before searching. The Since labels are
generated from the local calendar year; Last five years starts four years before
that year. For Custom range, enter inclusive four-digit From/To years (1000–9999).
A blank endpoint is unbounded and two blank endpoints mean Any time. Future years
are allowed; From must not be later than To.

The range is sent to Google Scholar as `as_ylo`/`as_yhi` on every page and retry.
It is not a local filter on enriched publication years: articles with unknown
metadata are retained. The **Years** label beside the PDF filter identifies the
range applied to the displayed search; changing the setup applies only to the
next search. Any time remains the default. Scholar blocking and incomplete
results remain possible; filtering does not prove corpus completeness.

## Journal Rankings And Public PDFs

V2.2.0 can attach two kinds of extra metadata to each reviewed result: a journal
ranking label, and the state of a public PDF copy. Both are additive review aids;
neither changes what was extracted from Google Scholar.

### Importing Ranking Tables

The application does **not** bundle a ranking dataset, and it does not download
ranking tables for you. You import them yourself:

1. Download the SCImago **SJR** journal ranking export and/or a **SINTA** journal
   ranking export from their own sites.
2. Import the file through the ranking import in the app (CSV or XLSX).
3. Records are matched to results by ISSN first, then by a conservative exact
   normalized journal title. Nothing is inferred from author names or free text.

Import details, expected columns, and the validation rules are documented in
[docs/ranking-imports.md](docs/ranking-imports.md).

What the labels mean:

- `SJR Quartile` is the SCImago SJR quartile (`Q1`–`Q4`). It is **not** an
  official Scopus CiteScore quartile.
- `SINTA Rank` is the SINTA accreditation rank (`S1`–`S6`).
- Both are **journal-level labels**. They are not a paper-quality score, not a
  citation metric for the paper, and not proof of current Scopus indexing or
  accreditation.
- The ranking year and category are kept from the imported record, so an old
  label is never presented as current.
- An unmatched or missing record stays **unknown**. An absent match is not proof
  that a journal is unindexed, unaccredited, or low quality.

Live ranking lookups are **not** verified from this application, and no supported,
reliable automated SJR or SINTA ranking interface has been verified. Probes from
this host produced **HTTP 403** from the SCImago site and from the current SINTA
domain `https://sinta.kemdiktisaintek.go.id/` (no redirects followed), while the
older SINTA domains did not resolve at all. A separate observation reached SINTA
canonical HTML with HTTP 200, so the outcomes differ by host, request, and time —
neither result is proof that these services are always blocked or always
available, and neither makes an automated feed dependable. The approved path is
the file import above.

### Metadata Lookup

Crossref is used only for conservative bibliographic identity (such as DOI,
journal title, and ISSNs). Crossref does not supply SJR or SINTA ranking labels.

### Public PDF Status

`[PDF]` links shown by Google Scholar are **candidates**. A candidate link is not
proof that a downloadable PDF exists, and it is not proof of an open-access or
reuse licence. Always check the version and licence of a downloaded file against
its source before redistributing it.

Each result carries one of three PDF states:

| State | Meaning |
| --- | --- |
| `available` | A public PDF was found and its bytes were confirmed. |
| `unavailable` | The checked sources returned no public PDF. |
| `unknown` | Not checked, or the check failed (provider or transport error). |

`unavailable` means the checked sources found nothing — it is **not** proof that
no copy exists anywhere. Errors stay `unknown` rather than being reported as
absence.

This project does **not** bypass paywalls, logins, CAPTCHA challenges, or any
other access control, and it does not promise that a public PDF exists for every
paper.

### Selecting And Downloading

Filter the results by public-PDF status, select the papers you want, and download
only those to a folder you choose. Downloads report progress and can be
cancelled. Selecting a row never starts a download by itself.

## Reliability Model

The scraper uses a reusable HTTP session, explicit timeouts, conservative
request headers, bounded retries, and request pacing. Google Scholar can still
rate-limit, block, challenge, or change markup without notice.

V2 reports these states instead of treating every response as a successful
empty result:

| Status | Meaning |
| --- | --- |
| `SUCCESS` | Requested pages completed and results were collected. |
| `PARTIAL_SUCCESS` | Some pages completed, then extraction stopped early. |
| `NO_RESULTS` | Google Scholar returned no results for the query. |
| `RATE_LIMITED` | Google Scholar temporarily limited the request. |
| `BLOCKED` | Google Scholar returned a block, consent, or challenge page. |
| `NETWORK_ERROR` | Timeout, connection issue, or bad HTTP response. |
| `PARSING_ERROR` | The response could not be parsed safely. |
| `CANCELLED` | The user cancelled extraction. |

## Desktop Workflow

1. Enter a search query.
2. Choose the number of Google Scholar result pages to scan.
3. Choose an export folder.
4. Enable or disable `Rank by relevance`.
5. Click `Search Scholar`.
6. Review results in the table.
7. Import SCImago SJR / SINTA ranking tables to attach journal ranking labels.
8. Run `Check metadata / PDFs` to attach bibliographic metadata and public-PDF state.
9. Filter the results by public-PDF status if you only want a subset.
10. Select the rows you want, then download those public PDFs to a folder.
11. Open individual links as needed.
12. Export to Excel or CSV — the export covers the visible, filtered rows.

The UI remains responsive while extraction, metadata checks, and downloads run.

## Installation Options

**No published V2.2.0 binary exists.** The journal-ranking and public-PDF
features are on an unpublished feature branch, so this section covers the two
things you can install today: the published **V2.0.1** release, and **V2.2.0 built
from source**. Building V2.2.0 locally does produce a portable ZIP and installer,
but those are your own local artifacts — they are not published releases, and no
published checksum file exists for them.

### Published Release: V2.0.1

The newest published build is V2.0.1 from the upstream repository
[MahdiNavaei/Google-Scholar-Scraper](https://github.com/MahdiNavaei/Google-Scholar-Scraper).
It does **not** include the V2.2.0 journal-ranking or public-PDF features.

- [Windows Installer](https://github.com/MahdiNavaei/Google-Scholar-Scraper/releases/download/v2.0.1/Google-Scholar-Scraper-v2.0.1-Setup-Windows-x64.exe)
- [Portable Windows ZIP](https://github.com/MahdiNavaei/Google-Scholar-Scraper/releases/download/v2.0.1/Google-Scholar-Scraper-v2.0.1-Portable-Windows-x64.zip)
- [SHA-256 checksums](https://github.com/MahdiNavaei/Google-Scholar-Scraper/releases/download/v2.0.1/SHA256SUMS.txt)
- [V2.0.1 release page](https://github.com/MahdiNavaei/Google-Scholar-Scraper/releases/tag/v2.0.1)

### Unpublished Preview: V2.2.0 From Source

V2.2.0 has no published installer and no published portable ZIP. Run it from the
feature branch:

```powershell
git clone --branch feature/scholar-year-filters https://github.com/Handy-Wibowo/Google-Scholar-Scraper.git
cd Google-Scholar-Scraper
python -m pip install --upgrade pip
python -m pip install .
python -m google_scholar_scraper
```

To produce the portable ZIP and installer yourself, see
[Building Windows Artifacts](#building-windows-artifacts). Those builds are your
own local artifacts of this branch; they are not published releases and carry no
published checksum file.

### Portable Windows ZIP (V2.0.1)

1. Download the portable ZIP from the V2.0.1 release.
2. Verify the ZIP against `SHA256SUMS.txt`.
3. Extract the ZIP.
4. Run `GoogleScholarScraper.exe` from the extracted folder.

The portable build does not require a source checkout.

### Windows Installer (V2.0.1)

1. Download the installer from the V2.0.1 release.
2. Verify the installer against `SHA256SUMS.txt`.
3. Run the installer.
4. Launch it from the Start Menu or optional desktop shortcut.

The installer is built with Inno Setup and installs per user.

### Run From Source

Requirements:

- Python 3.10 or newer.
- Tkinter available in the Python installation.

This works for any checkout, including the V2.2.0 feature branch:

```powershell
python -m pip install --upgrade pip
python -m pip install .
python -m google_scholar_scraper
```

## Export Formats

| Format | Default filename | Notes |
| --- | --- | --- |
| Excel | `scholar_articles.xlsx` | Written with `openpyxl`. |
| CSV | `scholar_articles.csv` | UTF-8 with BOM for Windows Excel. |

Exported columns:

- `Title`
- `Authors`
- `Link`
- `Relevance Score`
- `DOI`
- `Journal`
- `ISSNs`
- `Publication Year`
- `SJR Quartile`
- `SJR Year`
- `SJR Category`
- `SJR Source`
- `SINTA Rank`
- `SINTA Year`
- `SINTA Source`
- `PDF Status`
- `PDF URL`
- `PDF Source`
- `PDF Detail`
- `PDF Local Path`

The column list is derived from the article model's own export order, so a field
added to the model cannot silently drop out of an export.

When ranking is disabled, `Relevance Score` is left blank instead of inventing
a score. When a journal or PDF value was never looked up, its cell is left blank
(or `unknown` for `PDF Status`) instead of being filled with a guess.

## Google Scholar Limitations

Google Scholar is an external service and may change behavior without notice.
Automated access can be rate-limited, blocked, challenged, or affected by markup
changes.

Use conservative page counts and respect applicable terms, policies, and laws.
This project does not guarantee uninterrupted access or complete coverage of
Google Scholar results.

## Privacy And Local Processing

- Ranking is performed locally.
- Ranking table imports are read from local files you choose; no ranking dataset
  is bundled with the application.
- Exports are written to the selected local folder.
- No telemetry or analytics collection is implemented.
- No external AI service receives query or result data.
- Normal operation makes HTTP requests to Google Scholar for the query entered
  by the user.
- When you run `Check metadata / PDFs`, the app additionally contacts Crossref
  for bibliographic identity and the candidate PDF hosts to validate a public
  copy. When you start a download, the selected PDFs are fetched from their
  source hosts. Nothing else is contacted.

## Development Setup

```powershell
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
python -m pip install -e .
```

Optional build dependency:

```powershell
python -m pip install -c constraints\release-2.2.0.txt pyinstaller
```

Run the deterministic test suite:

```powershell
python -m unittest discover -s tests
```

The tests use saved fixtures and mocked responses. They do not require live
Google Scholar access.

## Building Windows Artifacts

Local portable build:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build_windows.ps1
```

The script removes only known generated outputs (`build/` and `dist/`) before
building, then creates:

- PyInstaller `onedir` application under `dist/Google-Scholar-Scraper-v2.2.0/`
- Portable ZIP under `dist/release/`
- `SHA256SUMS.txt`

If Inno Setup is installed locally and `-SkipInstaller` is not used, the same
script also compiles the installer. The Windows GitHub Actions build workflow is
configured to install or locate Inno Setup and fail if the installer cannot be
built.

Portable and installer distributions include `LICENSE`, `NOTICE`,
`COMMERCIAL_LICENSE.md`, `THIRD_PARTY_NOTICES.txt`, and a generated
`third_party_licenses/` bundle collected from the exact constrained release-build
environment.

They also ship the user documentation needed to prepare a ranking import:

- `README.md`
- `docs/ranking-imports.md`
- `docs/ranking_templates/*.csv` — header-only CSV templates with no ranking data
  in them, so you can see the accepted columns and fill in your own records.

## Project Structure

```text
src/google_scholar_scraper/
  app.py                  CLI/application entrypoint
  models.py               Article and extraction result models
  dedupe.py               Normalization, validation, duplicate removal
  ranking.py              Local lexical relevance ranking
  exporters.py            CSV and Excel export
  enrichment.py           Bibliographic metadata and PDF-state attachment
  journal_catalog.py      Imported SJR/SINTA ranking store and journal matching
  pdf_service.py          Bounded public-PDF checks and downloads
  scraper/client.py       Request flow, retries, progress, cancellation
  scraper/parser.py       Google Scholar HTML parsing and page classification
  ui/tkinter_app.py       Desktop UI

tests/
  fixtures/               Saved deterministic HTML fixtures
  test_*.py               Parser, client, ranking, export, UI, packaging tests

packaging/pyinstaller/    PyInstaller configuration
installer/                Inno Setup definition
scripts/                  Windows build and release-compliance helpers
constraints/              Exact release-build dependency constraints
docs/                     Release and audit documentation
```

`constraints/release-2.2.0.txt` reuses the V2.0.1 runtime and build pins verbatim:
V2.2.0 adds no dependency and upgrades no version. The V2.0.1 constraints file is
retained unchanged as the historical record for that release.

## Release And Documentation

Release status:

- **V2.2.0 — unpublished feature-branch preview.** No release, tag, installer, or
  portable ZIP has been published for this version. The work lives on
  [`feature/scholar-year-filters`](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/tree/feature/scholar-year-filters)
  in the fork [Handy-Wibowo/Google-Scholar-Scraper](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
  and the version number identifies the source tree only.
- **V2.0.1 — newest published release**, from the upstream repository
  [MahdiNavaei/Google-Scholar-Scraper](https://github.com/MahdiNavaei/Google-Scholar-Scraper).
- **V2.0.0 — previous published release.**

Documents:

- [V2.2.0 release notes](docs/RELEASE_NOTES_V2.2.0.md) — notes for the unpublished
  feature-branch preview, not a published release announcement.
- [V2.0.1 release notes](docs/RELEASE_NOTES_V2.0.1.md)
- [V2.0.0 release notes](docs/RELEASE_NOTES_V2.0.0.md)
- [V2.0.0 release verification record](docs/V2_RELEASE_CHECKLIST.md)
- [Changelog](CHANGELOG.md)
- [Ranking table import reference](docs/ranking-imports.md)
- [Journal ranks and public PDFs task book](docs/journal-pdf-taskbook.md)
- [Third-party notices](THIRD_PARTY_NOTICES.txt)
- [Third-party license bundle workflow](third_party_licenses/README.md)
- [PyInstaller spec](packaging/pyinstaller/GoogleScholarScraper.spec)
- [Inno Setup definition](installer/GoogleScholarScraper.iss)
- [Windows build script](scripts/build_windows.ps1)

## License

Google Scholar Scraper is source-available for noncommercial use under the
[PolyForm Noncommercial License 1.0.0](LICENSE).

This is not an OSI-approved open-source license. See [NOTICE](NOTICE) for
attribution and distribution notice information. See
[COMMERCIAL_LICENSE.md](COMMERCIAL_LICENSE.md) for the commercial licensing
summary.

## Commercial Licensing

The public repository is available under the PolyForm Noncommercial License
1.0.0. Commercial use requires a separate written commercial license from Mahdi
Navaei.

Commercial licensing inquiries are welcome. Pricing is not published publicly,
and each commercial arrangement may be evaluated separately.

For commercial licensing inquiries, contact
[Mahdi Navaei](mailto:M.Navaei1367@gmail.com).

## Sponsorship & Integrations

Project sponsorship, relevant commercial collaborations, carefully scoped
integrations, and ecosystem partnership discussions are welcome when they fit
the project direction.

For sponsorship or integration inquiries, contact
[Mahdi Navaei](mailto:M.Navaei1367@gmail.com).

Bug reports and focused issue reports are welcome. Larger code contribution
policy is intentionally conservative until a licensing and assignment process is
defined.

## Attribution

Google Scholar Scraper was created by
[Mahdi Navaei](https://github.com/MahdiNavaei) and is developed upstream at
[MahdiNavaei/Google-Scholar-Scraper](https://github.com/MahdiNavaei/Google-Scholar-Scraper).
The upstream author and project are unchanged; the journal-ranking and public-PDF
work described above is an independent fork feature.

This preview is developed in the fork
[Handy-Wibowo/Google-Scholar-Scraper](https://github.com/Handy-Wibowo/Google-Scholar-Scraper)
on the branch
[`feature/scholar-year-filters`](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/tree/feature/scholar-year-filters).
Fork changes have not been proposed to, reviewed by, or endorsed by the upstream
author, and the fork is not the source of any published release.

See [NOTICE](NOTICE) for attribution and distribution notice information.
