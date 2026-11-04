# Google Scholar Scraper — Rust (v3.0.0)

Native Rust rewrite of the Python desktop app (`../src`, frozen at v2.2.2 as the
behavioral oracle). Same core logic — Scholar scraping, PDF
availability/download, Crossref enrichment, citations, ranking, dedupe, journal
catalog, exports — with a Tauri v2 GUI (plain static `ui/`, no node).

## Layout

| Crate | Role |
|---|---|
| `gss-core` | All logic: `client`, `parser`, `pdf`, `crossref`, `catalog`, `exporters`, `dedupe`, `ranking`, `models`, `text`, `url`. Contracts in `gss-core/docs/CONTRACTS.md`. |
| `gss-app` | Tauri GUI shell + `--packaging-smoke` / `--selfcheck-layout` headless modes. |
| `gss-parity` | JSON-dump harness driven by `tools/parity_check.py`. |

## Build

Use `tools/build.cmd` — windres's inner `cmd /c` preprocessor needs
`CONDA_SHLVL=1` (silences the machine's cmd AutoRun conda hook) and a
backslash-format mingw64 PATH:

```bat
tools\build.cmd build --release
```

Requirements: Rust `stable-x86_64-pc-windows-gnu` + mingw-w64 binutils/gcc
(no MSVC Build Tools on this workstation — recorded deviation).

## Tests

```bat
tools\build.cmd test --workspace        :: 265 tests
tools\build.cmd clippy --workspace -- -D warnings
tools\build.cmd fmt --all --check
```

## Parity vs the Python oracle

```bat
python tools\parity_check.py --python ..\src --rust .
```

Gates (all must be zero-diff):
1. `normalize_title_key` over fixtures + adversarial edge cases.
2. `parse_scholar_page` over all 9 HTML fixtures + a 429.
3. CSV/BibTeX/RIS byte-equality on a fixed record set.
4. Full offline `scrape_scholar` extraction over scripted multi-page jobs.

The Python suite (`../src/tests`, 368 tests) remains the behavioral oracle;
the Rust port mirrors it 1:1 per module.

## Packaging

`cargo build --release` → `target/release/gss-app.exe` (~14.6 MB, cold start
<500 ms). `--packaging-smoke DIR` writes `packaging-smoke.xlsx`/`.csv` with the
exact 20-column export contract; CI (`​.github/workflows/rust.yml`) runs tests,
clippy, fmt, the smoke check, and attaches the exe + `SHA256SUMS.txt` on `v3.*`
tags.

<!-- ci-trigger: rust workflow watches rust/** -->
