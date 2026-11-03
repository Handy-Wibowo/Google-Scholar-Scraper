# Google Scholar Scraper V2.2.1 Release Notes

Status: **current normal release** of the
[Handy-Wibowo fork](https://github.com/Handy-Wibowo/Google-Scholar-Scraper),
published at the
[v2.2.1 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.1)
after fresh CI and Windows installer checks pass on the release commit. The
latest published upstream release remains V2.0.1. Attribution to Mahdi Navaei is
unchanged; the fork is not endorsed by the upstream author.

## Added in V2.2.1

Public-PDF verification now recognizes bot-challenge HTML returned through a
candidate PDF link and preserves the result as `unknown` rather than incorrectly
reporting it as `unavailable`. Challenge pages are not bypassed and do not prove
that a public PDF is absent.

For DOI-less records, public-PDF discovery can now fall back to an OpenAlex
search only when the returned record is an exact normalized-title match. A
non-matching or ambiguous title is not accepted as the article.

After a Scholar search completes, the interface now reminds users to run
`Check metadata / PDFs` before expecting enriched metadata or PDF states.

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
remain `unknown`.

## Release assets and local builds

The published release ships the Windows installer, portable ZIP, and
`SHA256SUMS.txt` at the
[v2.2.1 release page](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/releases/tag/v2.2.1).
To reproduce the artifacts locally, use the pinned `constraints/release-2.2.1.txt`
and `scripts/build_windows.ps1`. The portable package includes README,
ranking-import guidance, header-only ranking templates, and license notices. An
installer also requires Inno Setup; `-SkipInstaller` builds only the portable
package. A local build is your own artifact; verify published downloads against
the release's `SHA256SUMS.txt`.
