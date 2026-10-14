from pathlib import Path
import argparse

from google_scholar_scraper.exporters import save_to_csv, save_to_excel
from google_scholar_scraper.models import PDF_STATUS_UNKNOWN, Article
from google_scholar_scraper.ui.tkinter_app import run


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument(
        "--packaging-smoke",
        metavar="OUTPUT_DIR",
        help="write sample Excel and CSV exports, then exit",
    )
    args = parser.parse_args(argv)
    if args.packaging_smoke:
        run_packaging_smoke(Path(args.packaging_smoke))
        return

    run()


def run_packaging_smoke(output_dir: Path) -> None:
    """Write synthetic exports that exercise every export column, fully offline.

    The records below are fabricated smoke data, not scraped or looked-up results.
    No network request, ranking import, or PDF download happens here, so the PDF
    state stays ``unknown`` (never checked) rather than claiming availability.
    """
    output_dir.mkdir(parents=True, exist_ok=True)
    articles = [
        Article(
            title="Unicode Smoke: cafe, alpha + beta, 医学 AI",
            authors="Packaging Smoke",
            link="https://example.edu/smoke",
            relevance_score=88.8,
            doi="10.1000/smoke.2024.001",
            journal="Journal of Packaging Smoke",
            issns=("1234-5679", "1234-5687"),
            publication_year=2024,
            sjr_quartile="Q1",
            sjr_year=2024,
            sjr_category="Library and Information Sciences",
            sjr_source_url="https://www.scimagojr.com/journalsearch.php?q=12345678",
            sinta_rank="S2",
            sinta_year=2024,
            sinta_source_url="https://sinta.kemdikbud.go.id/journals/profile/1234",
            pdf_url="https://example.edu/smoke.pdf",
            pdf_source_url="https://example.edu/smoke",
            pdf_status=PDF_STATUS_UNKNOWN,
            pdf_detail="Synthetic fixture: no public-PDF check was performed.",
        ),
        Article(title="Unranked Smoke", authors="", link=""),
    ]
    save_to_excel(articles, output_dir / "packaging-smoke.xlsx")
    save_to_csv(articles, output_dir / "packaging-smoke.csv")
