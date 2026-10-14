# Google Scholar Scraper V2.1.0 Release Notes

> **Status: unpublished feature-branch preview — not a published release.**
> V2.1.0 has not been released or tagged, and no installer or portable ZIP has
> been published for it. The version number identifies this unpublished feature
> branch and its local builds only.
> The work lives on the branch
> [`feature/journal-ranks-public-pdfs`](https://github.com/Handy-Wibowo/Google-Scholar-Scraper/tree/feature/journal-ranks-public-pdfs)
> in the fork [Handy-Wibowo/Google-Scholar-Scraper](https://github.com/Handy-Wibowo/Google-Scholar-Scraper).
> The newest published release remains **V2.0.1** from the upstream repository
> [MahdiNavaei/Google-Scholar-Scraper](https://github.com/MahdiNavaei/Google-Scholar-Scraper).
> Upstream attribution is unchanged; these fork changes have not been reviewed by
> or endorsed by the upstream author.

## Overview

V2.1.0 is a feature release that adds journal ranking metadata and public-PDF
handling to the existing V2 desktop workflow. Searching, local relevance
ranking, duplicate removal, and Excel/CSV export keep working as before; the new
metadata is additive and is shown and exported next to the extracted results.

Nothing in this release bypasses an access control, and no ranking data is
bundled or fabricated.

Because there is no published V2.1.0 binary, the ways to run this version are a
source checkout of the feature branch (`python -m pip install .`), or a Windows
build produced locally with `scripts/build_windows.ps1`. A local build always
produces the portable ZIP; it produces the installer as well only when Inno Setup
is available and the installer step is not skipped with `-SkipInstaller`. Either
way it is your own artifact of the branch — not a published release, and with no
published checksum file.

## Added

### Journal ranking metadata (SCImago SJR and SINTA)

- Import SCImago **SJR** and **SINTA** journal ranking tables from CSV or XLSX
  files you supply yourself.
- Records are matched to results by ISSN first, then by a conservative exact
  normalized journal title. Nothing is guessed from author names, free text, or
  source strings.
- Each matched journal carries its ranking kind, label, ranking year, category,
  and source URL, so an old label is never presented as current.
- Unmatched or missing records stay **unknown**. An absent match is not proof
  that a journal is unindexed, unaccredited, or low quality.

### Public PDF state

- A `Check metadata / PDFs` action attaches bibliographic metadata and the state
  of a public PDF copy to the reviewed results.
- Each result reports exactly one of three PDF states:
  - `available` — a public PDF was found and its bytes were confirmed.
  - `unavailable` — the checked sources returned no public PDF.
  - `unknown` — not checked, or the check failed.
- `unavailable` means the checked sources found nothing. It is not proof that no
  copy exists anywhere. Provider and transport errors stay `unknown` rather than
  being reported as absence.
- A Google Scholar `[PDF]` link is treated as a **candidate**. A candidate link is
  not proof that a downloadable PDF exists, and it is not proof of an
  open-access or reuse licence.

### Downloading selected PDFs

- Filter the results by public-PDF status, select the rows you want, and download
  only those to a folder you choose.
- Downloads run in the background with progress reporting and cancellation.
- Selecting a row never starts a download by itself.

### Export columns

Excel and CSV exports gained the new metadata columns: `DOI`, `Journal`,
`ISSNs`, `Publication Year`, `SJR Quartile`, `SJR Year`, `SJR Category`,
`SJR Source`, `SINTA Rank`, `SINTA Year`, `SINTA Source`, `PDF Status`,
`PDF URL`, `PDF Source`, `PDF Detail`, and `PDF Local Path`.

The export column list is derived from the article model's own export order, so a
field added to the model cannot silently drop out of an export. The export covers
the visible, filtered set and is labelled accordingly.

### Distribution documentation

- The portable and installer distributions now ship `README.md`,
  `docs/ranking-imports.md`, and header-only `docs/ranking_templates/*.csv`
  templates, so a user of the packaged application can see the accepted import
  columns and prepare an import without a source checkout.
- The templates contain headers only. **No ranking data is bundled.**

## Changed

- Application version raised to `2.1.0` across the package `__version__`, project
  metadata, PyInstaller output directory, Inno Setup definition, Windows build
  script, and the Windows CI workflow naming.
- `constraints/release-2.1.0.txt` reuses the V2.0.1 runtime and build pins
  verbatim. V2.1.0 adds no dependency and upgrades no version. The V2.0.1
  constraints file is retained unchanged as the historical record for that
  release.

## Limitations And Caveats

These are stated plainly because the feature is easy to over-trust.

- **No bundled ranking dataset, and no automatic ranking download.** No supported,
  reliable automated SJR or SINTA ranking interface has been verified. Probes from
  this host produced **HTTP 403** from the SCImago site and from the current SINTA
  domain `https://sinta.kemdiktisaintek.go.id/` (no redirects followed), while the
  older SINTA domains did not resolve at all. A separate observation reached SINTA
  canonical HTML with HTTP 200, so the outcomes differ by host, request, and time —
  neither result is proof that these services are always blocked or always
  available, and neither makes an automated feed dependable. The approved path is
  the file import described above.
- **Crossref supplies identity metadata only** (DOI, journal title, ISSNs). It
  does not supply SJR or SINTA ranking labels.
- **No Scopus-indexing or paper-quality inference.** An SJR quartile is a
  journal-level SCImago label and is not an official Scopus CiteScore quartile.
  Neither label is a paper-quality score or a citation metric for the paper.
- **No paywall, login, CAPTCHA, or access-control bypass**, and no promise that a
  public PDF exists for every paper.
- **Candidate PDF metadata does not prove a reuse licence.** Check the version
  and licence of any downloaded file against its source before redistributing it.

## Verification Notes

- The full deterministic test suite is offline: it uses saved fixtures and mocked
  responses and requires no live Google Scholar, Crossref, or PDF host access.
- The packaged application's Excel/CSV smoke check was extended to exercise the
  new export columns from an explicitly synthetic record, with no network access,
  no ranking import, and no PDF download. Its PDF state is deliberately `unknown`
  because nothing was checked.

## Licensing

The license, proprietary attribution, and legal files are unchanged in this
release. Google Scholar Scraper remains source-available for noncommercial use
under the PolyForm Noncommercial License 1.0.0, with separate commercial
licensing available. See `LICENSE`, `NOTICE`, and `COMMERCIAL_LICENSE.md`.
