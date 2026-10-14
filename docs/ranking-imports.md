# Ranking imports (SJR quartiles and SINTA ranks)

Journal-level ranks are **not** fetched from a live ranking service. They are imported
from a file the user supplies, because the official sources cannot be automated
reliably from this application (see *Why there is no live lookup* below).

Two header-only templates ship with the application:

| Template | Source | Columns |
| --- | --- | --- |
| `docs/ranking_templates/scimago.csv` | SCImago Journal Rank (SJR) | `Title,Issn,SJR Best Quartile,Year,Categories,Source` |
| `docs/ranking_templates/sinta.csv` | SINTA (Indonesian accreditation) | `journal,issn,sinta_rank,year,source_url` |

They contain **no rows** — only the header line. Export the real data yourself and fill
the file in; nothing is bundled or fabricated.

## Importing

```python
from google_scholar_scraper.journal_catalog import JournalCatalog

catalog = JournalCatalog()
catalog.import_file("scimago-journal-rank.csv")          # SJR quartiles
catalog.import_file("sinta-journals.csv")                # SINTA ranks
catalog.import_file("scimago-2019.csv", default_year=2019)
```

`import_file(path, default_year=None) -> int` returns the number of records added.

- **Imports accumulate.** Importing SJR and then SINTA leaves both sets in place, and
  importing the same source for another year adds that year alongside the old one.
- **Imports are transactional.** Every row is parsed and validated before any table is
  rebuilt. A malformed file raises `ValueError` and leaves the existing catalogue
  completely untouched.
- `.csv`, `.tsv`, `.txt`, `.xlsx` and `.xlsm` are accepted. CSV delimiter (`;` or `,`) is
  detected from the file, so a semicolon SCImago export works as-is. `utf-8-sig` is used,
  so a BOM from Excel is harmless.

## Accepted column headers

Headers are matched case- and punctuation-insensitively against aliases, so the real
SCImago and SINTA exports work without editing:

| Meaning | Accepted headers |
| --- | --- |
| Journal title | `Title`, `Journal`, `Journal Title`, `Journal Name`, `Name`, `Source Title` |
| ISSN(s) | `Issn`, `ISSNs`, `ISSN (print)`, `ISSN (online)`, `pISSN`, `eISSN`, `ISSN numbers` |
| Rank | `SJR Best Quartile`, `Best Quartile`, `Quartile`, `SJR Quartile`, `SINTA Rank`, `Rank`, `Accreditation`, `Accreditation Rank` |
| Year | `Year`, `Ranking Year`, `SJR Year`, `SINTA Year`, `Accreditation Year`, `Date` |
| Category | `Categories`, `Category`, `Subject Area(s)`, `Subject Category`, `Field` |
| Source URL | `Source URL`, `URL`, `Link`, `Source`, `Source Link` |

Rank values are validated strictly: only `Q1`–`Q4` (SJR) and `S1`–`S6` (SINTA) are
accepted. A row with any other rank value — including a numeric `Rank` column, an empty
cell, or a stray `Q5` — is dropped, not guessed at. The SCImago `Rank` column is a
numeric position and is deliberately **not** used as a quartile; `SJR Best Quartile` wins.

A journal can carry several ISSNs in one cell (comma- or semicolon-separated); each is
normalized and checksum-validated, and invalid ones are discarded.

## Years

`Year` is taken from the record. **A missing year is never invented.** If your export
has no year column at all, either pass the year you know explicitly:

```python
catalog.import_file("scimago-latest.csv", default_year=2023)
```

…or import it without a year, in which case the record's year stays unknown and the
export shows an unknown year rather than a plausible-looking guess.

When several records exist for the same journal (multiple years, or SJR and SINTA
together), the **latest actual year** wins. An undated record never displaces a dated
one. Each system resolves independently: a journal can hold an SJR quartile and a SINTA
rank at the same time, and the year, category and source URL reported are the ones from
the record that actually supplied that rank.

## Matching articles to ranks

Two identity keys are tried, in this order:

1. **Exact ISSN** — all of the article's ISSNs are checked, so an SJR quartile on the
   e-ISSN and a SINTA rank on the p-ISSN both resolve.
2. **Exact normalized journal title** — only if no ISSN matched. Normalization is
   deliberately conservative: case, diacritics, punctuation and a leading `The` are
   ignored, and nothing else. No abbreviations, no stemming, no fuzzy scoring.

If a title maps to more than one distinct rank, the match is **dropped** rather than
guessed. Nothing is ever inferred from an article's authors or from Google Scholar's
citation line — that text mixes authors, venue and year, and reading a journal out of it
would fabricate an identity the source never asserted.

Consequently a conference paper, a preprint, or any article whose journal is not in the
catalogue keeps an empty rank. **Unknown is the correct answer there, not a defect.**
Missing records and unmatched journals are reported as unknown; they are never filled
with a default.

An SJR quartile is a journal-level label. It is not a statement about the quality of an
individual paper, and it is not proof that a journal is currently indexed in Scopus.

## Filling in bibliographic identity (Crossref)

`enrich_articles` fills the *bibliographic* fields — DOI, journal, ISSNs, publication
year — from Crossref, then applies whatever ranks the catalogue holds:

```python
from google_scholar_scraper.enrichment import enrich_articles

articles = enrich_articles(articles, catalog, cancel_event=stop, progress_callback=report)
```

- **Ordering and count are preserved.** Failures leave the original article in place, so
  a lookup that cannot answer degrades to an unknown field rather than a dropped row.
- **An explicit DOI is verified by echo.** Crossref must return the DOI that was asked
  for; a payload naming a different work (a redirect, a proxy, a provider mix-up) is
  treated as no answer. A failed DOI lookup is **never** retried by title — that would
  trade a known identifier for a guess.
- **A missing DOI is resolved by exact title only.** The query uses `query.title` (not
  `query.bibliographic`, which also matches authors, venue and abstract and buries an
  exact title under unrelated works) and a known year is pushed into the API date filter;
  with no known year no filter is sent. The candidate must have an identical normalized
  title and a journal-article type, and must not contradict a known author or year. Two
  distinct DOIs behind one exact title is ambiguous, so nothing is returned.
- **`rows` defaults to 20.** Measured for `Deep learning` (2015): a five-row page returned
  only same-titled book chapters, while a twenty-row page surfaced the Nature article
  among the exact-title alternatives, where the author and year checks disambiguated it.
- Only a *journal* gets journal identity. A conference or book record keeps an empty
  journal and no rank, even when its title matches something in the catalogue.
- `lookup=`, `max_lookups=`, `max_title_lookups=` and `allow_title_lookup=False` are
  injectable for tests and for bounding network work.

## Why there is no live lookup

- `journalrank.php` on scimagojr.com answers a direct programmatic request with
  **HTTP 403**; no supported public ranking API is known.
- SINTA's current canonical domain `sinta.kemdiktisaintek.go.id` **does** resolve, but its
  reachability varies with the request. Two independent observations on the same machine,
  same day, no proxy configured (`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` all
  unset), requests 2.34.2:

  | Request | Result |
  | --- | --- |
  | default `requests` User-Agent, `allow_redirects=False`, `stream=True`, timeout 15 | **403** `text/html`, `Server: awselb/2.0`, body `<h1>403 Forbidden</h1>` |
  | `requests.get` with the default User-Agent | **403** `text/html`, `Server: awselb/2.0` |
  | same call with a browser `User-Agent` | **200** `text/html; charset=UTF-8`, `Server: Apache/2.4.58 (Ubuntu)`, 29822 bytes |

  The difference is the `User-Agent` header, not the host, network, proxy or TLS: the AWS
  ELB edge answers 403 to a scripted client and forwards a browser-like one to the Apache
  origin. So neither "SINTA is unavailable" nor "SINTA is open" is accurate — reachability
  depends on the request, and it can change without notice.

  An earlier check also found SINTA's older domains (`sinta.ristekbrin.go.id`,
  `sinta2.ristekdikti.go.id`) unresolvable (NXDOMAIN / `gaierror 11001`) from this machine;
  that finding was about *those* hosts, not SINTA as a whole.

  Reachable HTML is still not a supported endpoint: the pages are markup meant for a
  browser, with no documented query interface and no published schema, so reading ranks out
  of them would mean inventing a scraping contract — and passing that edge block would mean
  sending a browser `User-Agent` specifically to get past it. **No supported automatic
  ranking interface was verified for SJR or SINTA.** SINTA ranks therefore still come from a
  file you export.

  Scope of that decision: it applies to *this* ranking lookup, which was never added. It is
  not a claim about the rest of the project — the existing Scholar client
  (`scraper/client.py`) has always sent a browser `User-Agent` as part of its normal request
  headers, and that behaviour is unchanged.
- OpenAlex and Crossref publish no SJR or SINTA quartiles at all.

So ranking data comes from a file you supply. Crossref *is* used, without a key, but
only for bibliographic identity — DOI, journal title, ISSNs, publication year — never for
a quartile. The application does not scrape ranking sites and does not invent an API where
none exists.

## What is not automated

- Downloading SCImago or SINTA exports.
- Any live quartile/rank lookup, including parsing SINTA's HTML pages (reachable only behind
  a User-Agent gate, and with no supported or documented endpoint — see above).
- Verifying that a journal is currently Scopus- or SINTA-accredited.

PDF availability is a separate concern owned by the PDF module; a Crossref licence/PDF
link is recorded as an **unverified candidate** and leaves `pdf_status` as `unknown`.
