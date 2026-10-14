# Google Scholar Scraper V2.2.0 Release Notes

Status: **unpublished feature-branch preview**. This source version and its local
builds live on [`feature/scholar-year-filters`](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/tree/feature/scholar-year-filters)
in the Handy-Wibowo fork. No release tag or downloadable release is published.
The latest published upstream release remains V2.0.1. Attribution to Mahdi Navaei
is unchanged; the fork is not endorsed by the upstream author.

## Added in V2.2.0

Publication-year filtering is applied at Google Scholar, using `as_ylo` and
`as_yhi` consistently on all pages and retries. Any time omits both parameters.
Presets include Since the current year, Since the previous year, Last five years
(starting current year minus four), and Custom range; labels follow the local
calendar year. Custom bounds are inclusive four-digit years from 1000–9999.
Blank endpoints are unbounded; both blank mean Any time. Reversed or malformed
ranges are rejected before a worker starts. Future years are allowed.

The applied year range is displayed beside the PDF filter independently of the
next search's editable controls. Changing those controls does not relabel earlier
results. Articles are not discarded based on missing or enriched publication years.

Existing local relevance ranking, deduplication, journal metadata/imports,
public-PDF checking/downloads, partial results, cancellation, and CSV/XLSX exports
remain available. No export column or runtime dependency is added. The pinned
build dependencies are unchanged, including the verified Tcl/Tk packaging hook.

## Retained limitations and safeguards

No bundled ranking dataset is included. Crossref supplies bibliographic metadata
but does not supply SJR or SINTA ranking labels. Users supply ranking CSV/XLSX
files with actual source/year provenance; unmatched values remain unknown.
An SJR quartile is not an official Scopus CiteScore quartile and is not proof of
current Scopus indexing or paper quality.

Candidate PDF metadata does not prove a reuse licence. Public PDF checks preserve
available / unavailable / unknown states; unavailable means no PDF was found
through the checked sources, not proof that none exists anywhere. No login,
paywall, CAPTCHA, or other access-control bypass is provided. Scholar may block,
rate-limit, or return incomplete results; year filtering does not guarantee
exhaustive collection.

## Local Windows build

Use the pinned `constraints/release-2.2.0.txt` and `scripts/build_windows.ps1`.
The portable package includes README, ranking-import guidance, header-only
ranking templates, and license notices. An installer also requires Inno Setup;
`-SkipInstaller` builds only the portable package. Local builds and checksums are
not a published GitHub release.
