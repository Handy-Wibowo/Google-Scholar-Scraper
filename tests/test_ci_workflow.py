import re
import unittest
from pathlib import Path

import google_scholar_scraper

PROJECT_ROOT = Path(__file__).resolve().parents[1]
CI_WORKFLOW = PROJECT_ROOT / ".github" / "workflows" / "ci.yml"


class CIWorkflowTests(unittest.TestCase):
    """Regression guard for the CI workflow.

    The workflow once asserted a hardcoded ``__version__ == "2.0.1"`` literal
    and pinned ``constraints/release-2.0.1.txt`` while the project had moved to
    a newer release. These tests tie the workflow's version references to the
    actual project version so a stale literal or constraint pin cannot silently
    reappear on the next release bump.
    """

    def setUp(self) -> None:
        self.workflow = CI_WORKFLOW.read_text(encoding="utf-8")

    def _current_version(self) -> str:
        # The CI "Import safety check" step validates the runtime __version__
        # against the installed distribution metadata. Use that same metadata as
        # the source of truth here, falling back to the runtime __version__ when
        # the distribution is not installed (e.g. a PYTHONPATH-only local run).
        try:
            from importlib.metadata import PackageNotFoundError, version

            return version("google-scholar-scraper")
        except PackageNotFoundError:
            return google_scholar_scraper.__version__

    def test_constraints_file_matches_current_release_version(self) -> None:
        version = self._current_version()
        self.assertIn(
            f"constraints/release-{version}.txt",
            self.workflow,
            f"ci.yml must install from constraints/release-{version}.txt to "
            f"match the current project version {version!r}",
        )

    def test_workflow_does_not_assert_a_hardcoded_version_literal(self) -> None:
        # A stale ``__version__ == "<literal>"`` drifts on every release bump.
        # The workflow must validate the version against installed metadata
        # (importlib.metadata) instead of comparing to a hardcoded string.
        stale = re.findall(r'__version__\s*==\s*["\'][^"\']+["\']', self.workflow)
        self.assertEqual(
            stale,
            [],
            f"ci.yml must not hardcode a version literal that can go stale: {stale}",
        )

    def test_version_is_validated_against_installed_metadata(self) -> None:
        self.assertIn("importlib.metadata", self.workflow)
        self.assertIn("metadata_version", self.workflow)

    def test_import_safety_checks_are_present(self) -> None:
        self.assertIn("import google_scholar_scraper", self.workflow)
        self.assertIn("from google_scholar_scraper.app import main", self.workflow)
        self.assertIn(
            "from google_scholar_scraper.ui.tkinter_app import MainWindow",
            self.workflow,
        )
        self.assertIn("assert callable(main)", self.workflow)
        self.assertIn('MainWindow.__name__ == "MainWindow"', self.workflow)


if __name__ == "__main__":
    unittest.main()
