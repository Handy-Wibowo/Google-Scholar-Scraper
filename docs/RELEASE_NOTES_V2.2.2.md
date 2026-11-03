# Google Scholar Scraper V2.2.2 Release Notes

Status: **current normal release** of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v2.2.2 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.2)
after fresh CI and Windows installer checks pass on the release commit. The
latest published upstream release remains V2.0.1. Attribution to Mahdi Navaei is
unchanged; the fork is not endorsed by the upstream author.

## Added in V2.2.2

BibTeX (`.bib`) and RIS (`.ris`) citation export. One paper, the selected rows, the
visible (filtered) rows, or all collected records can be written to a citation file
and imported into Zotero (File → Import) or Mendeley (+ Add new → Import library).
No browser extension, no reference-manager account connection, and no new Scholar
crawling are involved.

Citation records are resolved against Crossref where a DOI or an exact normalized
title identifies the work. Unresolved records are still exported with the fields
already known and an explicit review note; the Google Scholar citation line is
preserved verbatim in that note and is never parsed into structured authors. Nothing
is fabricated: a record without verified authors carries no author field at all.

`Export citations` is grouped with the Excel and CSV export buttons in the results
filter row. `File → Export citations…` and `Ctrl+E` remain as secondary paths.

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
exhaustive collection. Challenge pages and candidate hosts that block a check
remain `unknown`. Citation export is independent of PDF availability.

## Release assets and local builds

The published release ships the Windows installer, portable ZIP, and
`SHA256SUMS.txt` at the
[v2.2.2 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.2).
To reproduce the artifacts locally, use the pinned `constraints/release-2.2.2.txt`
and `scripts/build_windows.ps1`. The portable package includes README,
ranking-import guidance, header-only ranking templates, and license notices. An
installer also requires Inno Setup; `-SkipInstaller` builds only the portable
package. A local build is your own artifact; verify published downloads against
the release's `SHA256SUMS.txt`.
