# Year-filter implementation and acceptance

## Approved scope

Add only Google Scholar publication-year presets and custom inclusive bounds.
Any time is the default. Presets use the local calendar year; Last five years
starts at current year minus four. A blank endpoint is unbounded. Custom input
must be four ASCII digits in 1000–9999 and From must not exceed To; future years
are allowed. SearchRequest snapshots the bounds before the worker starts.

Filtering is sent to Scholar on every page/retry, not applied locally to Article
publication_year. Existing relevance ranking, metadata/imports, PDF workflows,
exports, partial results, and cancellation are retained. No new dependencies.

## Implementation decisions

- Reuse the current parameter builder and shared bounds validator; encode the URL
  helper using the standard library to preserve quoted/Unicode queries.
- Append defaulted fields to SearchRequest and keyword-only optional API bounds.
  The worker omits year keyword arguments for unrestricted legacy callbacks.
- Reuse the options row for presets and From/To; shorten the ranking notice rather
  than adding a row or reducing fonts. Display applied bounds in the existing PDF
  toolbar's spare space so the full range remains readable at minimum sizes.
- Reuse the exact pinned release packages and the verified Tcl/Tk hook. Version2.2.0
  is a local build/source preview, not a published release.

## Source acceptance

326 offline tests passed with the pinned Python, including year validation,
encoding, worker forwarding, pagination/retries, cancellation, partial retention,
and prior metadata/PDF regressions. The native-DPI probe covers year controls and
an applied 2018–2024 range at860x560,880x620,1080x700. Actual Tk multi-selection
and filtered CSV/XLSX export checks passed. One bounded live SOMLAP request
returned HTTP200 with5 article entries and verified as_ylo=2018/as_yhi=2026.
This does not prove the result set is exhaustive.

## Delivery gates

Build and verify frozen2.2.0 year parameters/UI, embedded Tcl/Tk hook, compliance, relocated
portable launch and CSV/XLSX smoke. Then make a new full, hash-verified backup of
the installed2.1.0 payload before replacement; preserve uninstaller/shortcuts and
verify the installed payload/launch. Publish only feature/scholar-year-filters
from83af705a3b4ac9196252a06611b05d90cd873933 to the Handy-Wibowo fork. Verify remote
main remains d81ab1f0347b02d6d8d65aeeb4fd1fcad72f9b07. Do not merge or create a tag.

## Packaging and installed-app acceptance

The local portable passed all106 payload/ZIP-byte comparisons, checksum and
license/template checks. Frozen code includes both year parameters and controls;
the Tcl9 DLLs and custom embedded-data hook are present and the incompatible
stock runtime hook is absent. Relocated and installed native2.2.0 windows were
responsive; both export smokes passed with20 columns and unknown PDF values intact.

The complete previous2.1.0 installation was backed up to
`E:\ai_project\google-scholar-scraper\backup-installed-v2.1.0-before-v2.2.0-20261015-044746`.
All108 backup file hashes matched before replacement. All106 installed payload
hashes match the verified build, and both uninstaller files retain their original
hashes. Existing shortcut paths were not modified.

Portable SHA256: `2e7ab09023f1d83decf24594a22703a41fcc42042593faea464e3034e0ce4319`.
EXE SHA256: `19fbde9c8e408616e6ec257fa7568a1b12756999afcf4daf5ad8278f775bfa9e`.
Feature-only publication is the remaining gate at this source snapshot; the
external release-review record stores the final remote commit/main comparison.
