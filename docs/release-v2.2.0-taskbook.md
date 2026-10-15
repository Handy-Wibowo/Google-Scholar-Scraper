# V2.2.0 publication taskbook

## Confirmed decisions

The user authorized a normal/latest v2.2.0 release on the
Handy-Wibowo/Google-Scholar-Scraper fork, main-branch CI/documentation fixes, and
publication only after fresh CI and installer verification pass at the exact
release commit. The upstream MahdiNavaei repository must not be modified. No
installed-app changes are required for this publication task.

The merged application baseline was 2.2.0 at
5da2bd4589ded8f942efc1ed64864384539028d5. Its CI incorrectly asserted 2.0.1;
the supplied downloaded logs match that commit and show AssertionError before
unit tests ran. The Windows build at that commit
passed installer install/export/uninstall smoke; that older artifact is not the
release candidate for the upcoming corrected commit.

## Ownership and acceptance

- CI worker owns .github/workflows/ci.yml and tests/test_ci_workflow.py: compare
  package version against installed metadata, retain import safety, use the
  current existing constraints, preserve Python3.10 compatibility and add a
  focused regression without dependencies.
- Documentation worker owns README.md, CHANGELOG.md,
  docs/RELEASE_NOTES_V2.2.0.md and tests/test_packaging.py: publishable fork release
  links, accurate retained feature/caveat documentation and corresponding tests;
  no fabricated checksums, signatures or verification claims.
- Dispatcher reviews changes and runs the full local suite, pushes only fork
  main, verifies fresh exact-commit GitHub CI/build jobs and installer smoke,
  downloads the matching artifact and verifies both checksums, inspects portable
  payload and native launch, then tags and publishes.

## Release assets and final safeguards

Tag: v2.2.0; normal release, latest, not prerelease.
Assets: Google-Scholar-Scraper-v2.2.0-Setup-Windows-x64.exe,
Google-Scholar-Scraper-v2.2.0-Portable-Windows-x64.zip, SHA256SUMS.txt.
The tag must resolve to the exact checked main commit. All uploads must match
fresh successful build hashes; verify published asset names/sizes and metadata.
Do not silently reuse the older build, bypass failures, overwrite an existing
tag, create a release in upstream, or claim green remote checks prematurely.
