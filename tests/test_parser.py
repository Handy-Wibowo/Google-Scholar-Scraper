import unittest
from pathlib import Path

from google_scholar_scraper.models import ExtractionStatus
from google_scholar_scraper.scraper.parser import classify_scholar_page, parse_scholar_page


FIXTURES = Path(__file__).parent / "fixtures"


def fixture(name: str) -> str:
    return (FIXTURES / name).read_text(encoding="utf-8")


class ParserTests(unittest.TestCase):
    def test_successful_multiple_result_parsing(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_basic.html"))

        self.assertEqual(result.status, ExtractionStatus.SUCCESS)
        self.assertEqual(len(result.articles), 2)
        self.assertEqual(result.articles[0].title, "Example Paper One")
        self.assertEqual(result.articles[0].authors, "A Author - Journal, 2024")
        self.assertEqual(result.articles[0].link, "https://example.edu/paper-one")

    def test_missing_optional_metadata_and_link_do_not_crash(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_missing_optional.html"))

        self.assertEqual(result.status, ExtractionStatus.SUCCESS)
        self.assertEqual(len(result.articles), 1)
        self.assertEqual(result.articles[0].title, "Citation only result without external link")
        self.assertEqual(result.articles[0].authors, "")
        self.assertEqual(result.articles[0].link, "")

    def test_scholar_classification_labels_are_stripped_from_titles(self) -> None:
        """Real Scholar markup puts `[PDF]`/`[HTML]` spans in the heading, ahead of the title.

        Captured shape (review-scholar-somlap.html):
        `<h3 class="gs_rt"><span class="gs_ctc"><span class="gs_ct1">[PDF]</span>
         <span class="gs_ct2">[PDF]</span></span> <a href=...>Real title</a></h3>`
        Leaving those labels in made the title unmatchable against any bibliographic record.
        """
        result = parse_scholar_page(fixture("scholar_results_labels.html"))

        self.assertEqual(
            result.articles[0].title, "A Machine-Learning-Powered Real-Time SIEM Framework"
        )
        self.assertEqual(result.articles[1].title, "The malware detection challenge of accuracy")
        for article in result.articles:
            with self.subTest(title=article.title):
                self.assertFalse(article.title.startswith("["))
                self.assertNotIn("[PDF]", article.title)
                self.assertNotIn("[HTML]", article.title)
                self.assertNotIn("[CITATION]", article.title)

    def test_citation_only_result_drops_only_the_structural_label(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_labels.html"))

        self.assertEqual(
            result.articles[2].title, "Citation only result with a Scholar label"
        )
        self.assertEqual(result.articles[2].link, "")

    def test_genuine_brackets_and_emphasis_inside_a_title_are_preserved(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_labels.html"))

        self.assertEqual(
            result.articles[3].title, "Structural analysis of [RNA] complexes in solution"
        )

    def test_ui_expander_is_never_mistaken_for_a_pdf_candidate(self) -> None:
        """`<a class="gs_or_ggsm_exp" href="javascript:void(0)"><span>[PDF]</span></a>` is a control."""
        result = parse_scholar_page(fixture("scholar_results_labels.html"))

        self.assertEqual(result.articles[0].pdf_url, "https://repo.example.org/machine-learning.pdf")
        self.assertEqual(result.articles[0].pdf_status, "unknown")
        for article in result.articles:
            with self.subTest(title=article.title):
                self.assertFalse(article.pdf_url.startswith("javascript:"))

    def test_declared_pdf_link_is_captured_but_left_unverified(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_metadata.html"))

        paper_one = result.articles[0]
        self.assertEqual(paper_one.pdf_url, "https://repo.example.org/paper-one.pdf")
        self.assertEqual(paper_one.pdf_status, "unknown")
        self.assertIn("not verified", paper_one.pdf_detail)

    def test_html_and_unbracketed_labels_are_never_treated_as_pdf(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_metadata.html"))

        # Paper Two declares only [HTML]; Paper Four has a bare "PDF" with no brackets.
        self.assertEqual(result.articles[1].pdf_url, "")
        self.assertEqual(result.articles[3].pdf_url, "")

    def test_explicit_doi_link_is_captured_and_others_are_not(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_metadata.html"))

        self.assertEqual(result.articles[0].doi, "10.1234/example.2024.001")
        self.assertEqual(result.articles[1].doi, "")

    def test_publication_year_requires_an_unambiguous_citation_year(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_metadata.html"))

        self.assertEqual(result.articles[0].publication_year, 2024)
        self.assertEqual(result.articles[1].publication_year, 2023)
        # Paper Three's citation spans "2020 - 2022": two years, so neither is claimed.
        self.assertIsNone(result.articles[2].publication_year)

    def test_journal_is_never_fabricated_from_the_citation_line(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_metadata.html"))

        for article in result.articles:
            with self.subTest(title=article.title):
                self.assertEqual(article.journal, "")

    def test_parser_preserves_raw_article_text_for_data_quality_layer(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_duplicates.html"))

        self.assertEqual(result.status, ExtractionStatus.SUCCESS)
        self.assertEqual(len(result.articles), 2)
        self.assertIn("\n", result.articles[0].title)

    def test_genuine_no_results_detection(self) -> None:
        result = parse_scholar_page(fixture("scholar_results_empty.html"))

        self.assertEqual(result.status, ExtractionStatus.NO_RESULTS)
        self.assertEqual(result.articles, [])

    def test_rate_limit_text_detection(self) -> None:
        result = parse_scholar_page(fixture("scholar_rate_limited.html"))

        self.assertEqual(result.status, ExtractionStatus.RATE_LIMITED)

    def test_http_429_detection(self) -> None:
        status = classify_scholar_page("<html><body>anything</body></html>", status_code=429)

        self.assertEqual(status, ExtractionStatus.RATE_LIMITED)

    def test_blocked_challenge_detection(self) -> None:
        result = parse_scholar_page(fixture("scholar_blocked.html"))

        self.assertEqual(result.status, ExtractionStatus.BLOCKED)

    def test_incompatible_page_is_parsing_error(self) -> None:
        result = parse_scholar_page(fixture("scholar_incompatible.html"))

        self.assertEqual(result.status, ExtractionStatus.PARSING_ERROR)

    def test_results_page_carrying_hidden_error_template_is_still_success(self) -> None:
        # Live Scholar result pages always embed this hidden modal template, which
        # used to be substring-matched against the whole document and mask real results.
        html = (
            "<html><head><title>Google Scholar</title></head><body>"
            '<div id="gs_md_err" style="display:none">'
            "The system can&#39;t perform the operation now. Try again later.</div>"
            '<div class="gs_r gs_or gs_scl"><div class="gs_ri">'
            '<h3 class="gs_rt"><a href="https://example.edu/paper">Real result title</a></h3>'
            '<div class="gs_a">A Author - Journal, 2024</div>'
            "</div></div></body></html>"
        )

        result = parse_scholar_page(html)

        self.assertEqual(result.status, ExtractionStatus.SUCCESS)
        self.assertEqual([article.title for article in result.articles], ["Real result title"])

    def test_empty_results_page_carrying_hidden_error_template_is_no_results(self) -> None:
        html = (
            "<html><head><title>Google Scholar</title></head><body>"
            '<div id="gs_md_err" style="display:none">'
            "The system can&#39;t perform the operation now. Try again later.</div>"
            "<div>Your search did not match any articles.</div>"
            "</body></html>"
        )

        result = parse_scholar_page(html)

        self.assertEqual(result.status, ExtractionStatus.NO_RESULTS)

    def test_live_style_empty_results_page_is_no_results_not_network_error(self) -> None:
        # Observed on live pages past the end of a result set, and on pages Google serves
        # empty under load: HTTP 200, full Scholar shell with the result-count header, zero
        # gs_ri entries, plus the hidden error template every Scholar page embeds.
        html = (
            "<html><head><title>Google Scholar</title></head><body>"
            '<div id="gs_md_err" style="display:none">'
            "The system can&#39;t perform the operation now. Try again later.</div>"
            '<div id="gs_ab_md">Page 2 of 7 results (0,06 sec)</div>'
            '<div id="gs_res_ccl"><div id="gs_res_ccl_mid"></div></div>'
            "</body></html>"
        )

        result = parse_scholar_page(html)

        self.assertEqual(result.status, ExtractionStatus.NO_RESULTS)
        self.assertEqual(result.articles, [])

    def test_google_internal_server_error_page_is_network_error(self) -> None:
        html = """
        <html><head><title>Google Scholar</title></head>
        <body>The system can't perform the operation now. Try again later.
        Server Error We're sorry but it appears that there has been an internal server error.</body></html>
        """

        result = parse_scholar_page(html)

        self.assertEqual(result.status, ExtractionStatus.NETWORK_ERROR)


if __name__ == "__main__":
    unittest.main()
