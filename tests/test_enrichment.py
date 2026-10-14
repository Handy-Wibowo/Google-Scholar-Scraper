import json
import unittest
from threading import Event

from google_scholar_scraper.enrichment import (
    CrossrefClient,
    CrossrefRecord,
    enrich_articles,
    record_from_crossref_message,
)
from google_scholar_scraper.journal_catalog import JournalCatalog
from google_scholar_scraper.models import Article, PDF_STATUS_AVAILABLE, PDF_STATUS_UNKNOWN


JOURNAL_MESSAGE = {
    "DOI": "10.1234/example.2024.001",
    "type": "journal-article",
    "title": ["Deep Learning for Medical Imaging"],
    "container-title": ["Journal of Examples"],
    "ISSN": ["1234-5679"],
    "published-print": {"date-parts": [[2024, 3]]},
    "author": [{"family": "Author", "given": "A"}],
}

CONFERENCE_MESSAGE = {
    "DOI": "10.1234/conf.2023.001",
    "type": "proceedings-article",
    "title": ["A Conference Paper"],
    "container-title": ["Conference on Examples"],
    "ISSN": ["1234-5679"],
    "published-print": {"date-parts": [[2023]]},
}


class FakeResponse:
    def __init__(self, payload, status_code: int = 200, content_length=None) -> None:
        self.status_code = status_code
        self.headers = {} if content_length is None else {"Content-Length": str(content_length)}
        self._body = json.dumps(payload).encode("utf-8")
        self._offset = 0
        self.closed = False
        self.raw = self

    def read(self, amount=-1, decode_content=True):
        """Mimic a real stream: honour the bound and signal EOF with b""."""
        if amount is None or amount < 0:
            chunk = self._body[self._offset :]
        else:
            chunk = self._body[self._offset : self._offset + amount]
        self._offset += len(chunk)
        return chunk

    def json(self):
        return json.loads(self._body.decode("utf-8"))

    def close(self):
        self.closed = True


class FakeSession:
    """A transport stub: no socket is ever opened."""

    def __init__(self, responses) -> None:
        self._responses = list(responses)
        self.requests: list[tuple[str, dict]] = []
        self.closed = False
        self.trust_env = True

    def get(self, url, **kwargs):
        self.requests.append((url, kwargs))
        if not self._responses:
            raise AssertionError(f"Unexpected request to {url}")
        result = self._responses.pop(0)
        if isinstance(result, Exception):
            raise result
        return result

    def close(self):
        self.closed = True


class RecordingLookup:
    """A title/DOI lookup stub that answers per *constraints*, not per title alone.

    It models the real contract: the answer depends on the title, the author hint and the
    year, so a caller that ignores the constraints cannot get the right record.
    """

    def __init__(self, by_constraints=None, by_doi=None) -> None:
        self._by_constraints = by_constraints or {}
        self._by_doi = by_doi or {}
        self.title_calls: list[tuple[str, str, int | None]] = []
        self.doi_calls: list[str] = []
        self.closed = False

    def lookup(self, doi: str):
        self.doi_calls.append(doi)
        return self._by_doi.get(doi)

    def lookup_title(self, title: str, *, author: str = "", year: int | None = None, rows: int = 20):
        self.title_calls.append((title, author, year))
        return self._by_constraints.get((title, author, year))

    def close(self) -> None:
        self.closed = True


class CrossrefMappingTests(unittest.TestCase):
    def test_journal_article_carries_container_identity(self) -> None:
        record = record_from_crossref_message(JOURNAL_MESSAGE)

        self.assertEqual(record.journal, "Journal of Examples")
        self.assertEqual(record.issns, ("1234-5679",))
        self.assertEqual(record.year, 2024)
        self.assertEqual(record.work_type, "journal-article")

    def test_conference_paper_gets_no_journal_identity(self) -> None:
        record = record_from_crossref_message(CONFERENCE_MESSAGE)

        self.assertEqual(record.journal, "")
        self.assertEqual(record.issns, ())
        self.assertEqual(record.work_type, "proceedings-article")

    def test_public_pdf_candidate_requires_licence_and_pdf_link(self) -> None:
        without_licence = dict(JOURNAL_MESSAGE)
        without_licence["link"] = [{"URL": "https://repo.example/a.pdf", "content-type": "application/pdf"}]
        with_licence = dict(without_licence)
        with_licence["license"] = [{"URL": "https://creativecommons.org/licenses/by/4.0/"}]

        self.assertEqual(record_from_crossref_message(without_licence).pdf_candidate_url, "")
        self.assertEqual(
            record_from_crossref_message(with_licence).pdf_candidate_url, "https://repo.example/a.pdf"
        )


class ClientBehaviourTests(unittest.TestCase):
    def test_lookup_returns_none_on_transport_error(self) -> None:
        import requests

        client = CrossrefClient(FakeSession([requests.ConnectionError("down")]))

        self.assertIsNone(client.lookup("10.1234/example"))

    def test_lookup_returns_none_on_non_200(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({}, status_code=404)]))

        self.assertIsNone(client.lookup("10.1234/example"))

    def test_lookup_returns_none_on_malformed_json(self) -> None:
        response = FakeResponse({})
        response._body = b"{not json"
        client = CrossrefClient(FakeSession([response]))

        self.assertIsNone(client.lookup("10.1234/example"))

    def test_lookup_refuses_an_oversized_body(self) -> None:
        client = CrossrefClient(
            FakeSession([FakeResponse({"message": JOURNAL_MESSAGE}, content_length=10_000_000)]),
            max_response_bytes=1024,
        )

        self.assertIsNone(client.lookup("10.1234/example"))

    def test_doi_is_url_encoded_and_redirects_are_not_followed(self) -> None:
        session = FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})])
        client = CrossrefClient(session)

        client.lookup("10.1234/example 2024")

        url, kwargs = session.requests[0]
        self.assertNotIn(" ", url)
        self.assertFalse(kwargs["allow_redirects"])

    def test_own_session_disables_environment_trust(self) -> None:
        client = CrossrefClient()
        try:
            self.assertFalse(client._session.trust_env)
        finally:
            client.close()

    def test_injected_session_is_not_closed_by_the_client(self) -> None:
        session = FakeSession([])
        client = CrossrefClient(session)

        client.close()

        self.assertFalse(session.closed)

    def test_lookup_accepts_a_payload_echoing_the_requested_doi(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})]))

        record = client.lookup("10.1234/example.2024.001")

        self.assertEqual(record.doi, "10.1234/example.2024.001")

    def test_lookup_rejects_a_payload_for_a_different_doi(self) -> None:
        """A redirect or provider mix-up must not attach another work's identity."""
        client = CrossrefClient(FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})]))

        self.assertIsNone(client.lookup("10.9999/some.other.work"))

    def test_lookup_rejects_a_payload_with_no_doi_at_all(self) -> None:
        without_doi = {key: value for key, value in JOURNAL_MESSAGE.items() if key != "DOI"}
        client = CrossrefClient(FakeSession([FakeResponse({"message": without_doi})]))

        self.assertIsNone(client.lookup("10.1234/example.2024.001"))

    def test_lookup_is_case_insensitive_about_the_echoed_doi(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})]))

        self.assertIsNotNone(client.lookup("10.1234/EXAMPLE.2024.001"))

    def test_title_lookup_uses_query_title_and_accepts_one_exact_match(self) -> None:
        session = FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})])
        client = CrossrefClient(session)

        record = client.lookup_title("Deep Learning for Medical Imaging")

        self.assertEqual(record.doi, "10.1234/example.2024.001")
        self.assertIn("query.title", session.requests[0][0])
        # `query.bibliographic` also matches authors/venue/abstract, which buries an exact
        # title under unrelated works, so it must not be used for title resolution.
        self.assertNotIn("query.bibliographic", session.requests[0][0])

    def test_title_lookup_pushes_a_known_year_into_the_api_filter(self) -> None:
        session = FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})])
        client = CrossrefClient(session)

        client.lookup_title("Deep Learning for Medical Imaging", year=2024)

        url = session.requests[0][0].replace("%2D", "-").replace("%3A", ":")
        # One year of drift is tolerated, matching `_year_consistent`.
        self.assertIn("from-pub-date:2023-01-01", url)
        self.assertIn("until-pub-date:2025-12-31", url)

    def test_title_lookup_sends_no_date_filter_when_the_year_is_unknown(self) -> None:
        session = FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})])
        client = CrossrefClient(session)

        client.lookup_title("Deep Learning for Medical Imaging")

        self.assertNotIn("filter=", session.requests[0][0])

    def test_title_lookup_disambiguates_among_same_titled_works(self) -> None:
        """Live evidence for "Deep learning" (2015): the Nature article wins on author+year."""
        nature = {
            "DOI": "10.1038/nature14539",
            "type": "journal-article",
            "title": ["Deep learning"],
            "container-title": ["Nature"],
            "ISSN": ["0028-0836", "1476-4687"],
            "published-print": {"date-parts": [[2015, 5]]},
            "author": [{"family": "LeCun", "given": "Y"}],
        }
        scholarpedia = {
            "DOI": "10.4249/scholarpedia.32832",
            "type": "journal-article",
            "title": ["Deep learning"],
            "author": [{"family": "Schmidhuber", "given": "J"}],
        }
        chapter = {
            "DOI": "10.1007/978-1-4302-5990-9_9",
            "type": "book-chapter",
            "title": ["Deep Learning"],
            "author": [{"family": "LeCun", "given": "Y"}],
        }
        client = CrossrefClient(
            FakeSession([FakeResponse({"message": {"items": [chapter, scholarpedia, nature]}})])
        )

        record = client.lookup_title("Deep learning", author="Y LeCun - Nature, 2015", year=2015)

        self.assertEqual(record.doi, "10.1038/nature14539")
        self.assertEqual(record.journal, "Nature")

    def test_default_title_rows_covers_same_titled_alternatives(self) -> None:
        """A short page hides the real article behind same-titled book chapters."""
        session = FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})])
        client = CrossrefClient(session)

        client.lookup_title("Deep Learning for Medical Imaging")

        self.assertIn("rows=20", session.requests[0][0])

    def test_title_lookup_rejects_a_near_miss_title(self) -> None:
        near_miss = dict(JOURNAL_MESSAGE)
        near_miss["title"] = ["Deep Learning for Medical Imaging Systems"]
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [near_miss]}})]))

        self.assertIsNone(client.lookup_title("Deep Learning for Medical Imaging"))

    def test_title_lookup_rejects_a_non_journal_work_with_an_identical_title(self) -> None:
        """Live evidence: a book chapter titled exactly "Deep learning" is not this paper."""
        book_chapter = dict(JOURNAL_MESSAGE)
        book_chapter["type"] = "book-chapter"
        book_chapter["DOI"] = "10.1201/b17103-3"
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [book_chapter]}})]))

        self.assertIsNone(client.lookup_title("Deep Learning for Medical Imaging"))

    def test_title_lookup_ignores_a_non_journal_duplicate_of_a_valid_article(self) -> None:
        book_chapter = dict(JOURNAL_MESSAGE)
        book_chapter["type"] = "book-chapter"
        book_chapter["DOI"] = "10.1201/b17103-3"
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [book_chapter, JOURNAL_MESSAGE]}})]))

        record = client.lookup_title("Deep Learning for Medical Imaging")

        self.assertEqual(record.doi, "10.1234/example.2024.001")

    def test_title_lookup_rejects_two_distinct_dois_behind_one_title(self) -> None:
        duplicate = dict(JOURNAL_MESSAGE)
        duplicate["DOI"] = "10.1234/example.2024.002"
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE, duplicate]}})]))

        self.assertIsNone(client.lookup_title("Deep Learning for Medical Imaging"))

    def test_title_lookup_rejects_a_contradicting_author(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})]))

        self.assertIsNone(
            client.lookup_title("Deep Learning for Medical Imaging", author="Z Zhang - Journal, 2024")
        )

    def test_title_lookup_rejects_a_contradicting_year(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})]))

        self.assertIsNone(client.lookup_title("Deep Learning for Medical Imaging", year=2010))


class EnrichmentTests(unittest.TestCase):
    def _catalog(self) -> JournalCatalog:
        import tempfile
        from pathlib import Path

        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        path = Path(temp_dir.name) / "ranks.csv"
        path.write_text(
            "Title,Issn,SJR Best Quartile,Year,Categories\n"
            "Journal of Examples,1234-5679,Q1,2023,Multidisciplinary\n",
            encoding="utf-8",
        )
        catalog = JournalCatalog()
        catalog.import_file(path)
        return catalog

    def test_doi_article_gains_identity_and_rank(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})]))
        article = Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001")

        result = enrich_articles([article], self._catalog(), lookup=client)

        self.assertEqual(result[0].journal, "Journal of Examples")
        self.assertEqual(result[0].issns, ("1234-5679",))
        self.assertEqual(result[0].sjr_quartile, "Q1")
        self.assertEqual(result[0].sjr_year, 2023)
        self.assertEqual(result[0].sjr_category, "Multidisciplinary")

    def test_missing_doi_article_is_resolved_by_exact_title(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": {"items": [JOURNAL_MESSAGE]}})]))
        article = Article(title="Deep Learning for Medical Imaging", link="https://publisher.example/paper")

        result = enrich_articles([article], self._catalog(), lookup=client)

        self.assertEqual(result[0].doi, "10.1234/example.2024.001")
        self.assertEqual(result[0].journal, "Journal of Examples")
        self.assertEqual(result[0].sjr_quartile, "Q1")

    def test_conference_article_never_receives_a_journal_rank(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": CONFERENCE_MESSAGE})]))
        article = Article(title="A Conference Paper", doi="10.1234/conf.2023.001")

        result = enrich_articles([article], self._catalog(), lookup=client)

        self.assertEqual(result[0].journal, "")
        self.assertEqual(result[0].sjr_quartile, "")

    def test_unreachable_provider_leaves_the_article_unchanged(self) -> None:
        import requests

        session = FakeSession([requests.ConnectionError("down")])
        client = CrossrefClient(session)
        article = Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001")

        result = enrich_articles([article], self._catalog(), lookup=client)

        self.assertIs(result[0], article)
        # Only the DOI request was made: a failed DOI lookup must not be retried as a
        # title guess that could substitute a different work's identity.
        self.assertEqual(len(session.requests), 1)
        self.assertIn("10.1234%2Fexample.2024.001", session.requests[0][0])

    def test_failed_doi_lookup_is_never_retried_by_title(self) -> None:
        import requests

        session = FakeSession([requests.ConnectionError("down")])
        client = CrossrefClient(session)
        article = Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001")

        result = enrich_articles([article], self._catalog(), lookup=client)

        self.assertEqual(result[0].journal, "")
        self.assertEqual(result[0].doi, "10.1234/example.2024.001")
        self.assertFalse(any("query.bibliographic" in url for url, _ in session.requests))

    def test_known_values_are_never_overwritten(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})]))
        article = Article(
            title="Deep Learning for Medical Imaging",
            doi="10.1234/example.2024.001",
            journal="Already Known Journal",
            issns=("0028-0836",),
            publication_year=1999,
        )

        result = enrich_articles([article], None, lookup=client)

        self.assertEqual(result[0].journal, "Already Known Journal")
        self.assertEqual(result[0].issns, ("0028-0836",))
        self.assertEqual(result[0].publication_year, 1999)

    def test_unverified_pdf_candidate_keeps_status_unknown(self) -> None:
        message = dict(JOURNAL_MESSAGE)
        message["license"] = [{"URL": "https://creativecommons.org/licenses/by/4.0/"}]
        message["link"] = [{"URL": "https://repo.example/a.pdf", "content-type": "application/pdf"}]
        client = CrossrefClient(FakeSession([FakeResponse({"message": message})]))
        article = Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001")

        result = enrich_articles([article], None, lookup=client)

        self.assertEqual(result[0].pdf_url, "https://repo.example/a.pdf")
        self.assertEqual(result[0].pdf_status, PDF_STATUS_UNKNOWN)
        self.assertNotEqual(result[0].pdf_status, PDF_STATUS_AVAILABLE)

    def test_cancellation_keeps_handled_articles_and_returns_the_remainder(self) -> None:
        cancel_event = Event()
        cancel_event.set()
        client = CrossrefClient(FakeSession([]))
        articles = [
            Article(title="First Paper"),
            Article(title="Second Paper"),
        ]

        result = enrich_articles(articles, None, cancel_event=cancel_event, lookup=client)

        self.assertEqual(result, articles)

    def test_progress_callback_reports_each_article(self) -> None:
        client = CrossrefClient(FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})]))
        seen: list[tuple[int, int, str]] = []
        articles = [Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001")]

        enrich_articles(articles, None, progress_callback=lambda *args: seen.append(args), lookup=client)

        self.assertEqual(seen, [(1, 1, "Deep Learning for Medical Imaging")])

    def test_lookup_budget_bounds_network_work(self) -> None:
        client = CrossrefClient(FakeSession([]))
        articles = [
            Article(title="First Paper", doi="10.1234/a"),
            Article(title="Second Paper", doi="10.1234/b"),
        ]

        result = enrich_articles(articles, None, lookup=client, max_lookups=0, allow_title_lookup=False)

        self.assertEqual(result, articles)

    def test_same_doi_is_only_looked_up_once(self) -> None:
        session = FakeSession([FakeResponse({"message": JOURNAL_MESSAGE})])
        client = CrossrefClient(session)
        articles = [
            Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001"),
            Article(title="Deep Learning for Medical Imaging (reprint)", doi="10.1234/example.2024.001"),
        ]

        enrich_articles(articles, None, lookup=client)

        self.assertEqual(len(session.requests), 1)

    def test_same_title_with_different_authors_and_years_gets_distinct_records(self) -> None:
        """The reported defect: one title, two works — the cache must not conflate them."""
        first = CrossrefRecord(
            doi="10.1000/first", journal="Journal of First", issns=("1234-5679",), year=2015
        )
        second = CrossrefRecord(
            doi="10.1000/second", journal="Journal of Second", issns=("1234-5687",), year=2024
        )
        client = RecordingLookup(
            by_constraints={
                ("Same exact title", "First Author", 2015): first,
                ("Same exact title", "Second Author", 2024): second,
            }
        )
        articles = [
            Article(title="Same exact title", authors="First Author", publication_year=2015),
            Article(title="Same exact title", authors="Second Author", publication_year=2024),
        ]

        result = enrich_articles(articles, None, lookup=client)

        self.assertEqual(result[0].doi, "10.1000/first")
        self.assertEqual(result[0].journal, "Journal of First")
        self.assertEqual(result[1].doi, "10.1000/second")
        self.assertEqual(result[1].journal, "Journal of Second")
        self.assertEqual(len(client.title_calls), 2)

    def test_identical_constraints_reuse_one_query(self) -> None:
        record = CrossrefRecord(doi="10.1000/only", journal="Journal of Examples", issns=("1234-5679",))
        client = RecordingLookup(by_constraints={("Shared title", "A Author", 2020): record})
        articles = [
            Article(title="Shared title", authors="A Author", publication_year=2020),
            Article(title="shared   TITLE", authors="A Author", publication_year=2020),
        ]

        result = enrich_articles(articles, None, lookup=client)

        self.assertEqual([article.doi for article in result], ["10.1000/only", "10.1000/only"])
        self.assertEqual(len(client.title_calls), 1)

    def test_cached_title_answer_survives_an_exhausted_budget(self) -> None:
        record = CrossrefRecord(doi="10.1000/only", journal="Journal of Examples", issns=("1234-5679",))
        client = RecordingLookup(by_constraints={("Shared title", "A Author", 2020): record})
        articles = [
            Article(title="Shared title", authors="A Author", publication_year=2020),
            Article(title="shared title", authors="A Author", publication_year=2020),
        ]

        result = enrich_articles(articles, None, lookup=client, max_title_lookups=1)

        # The second record needs no new query, so the budget must not blank it out.
        self.assertEqual([article.doi for article in result], ["10.1000/only", "10.1000/only"])
        self.assertEqual(len(client.title_calls), 1)

    def test_exhausted_budget_blocks_only_a_genuinely_new_query(self) -> None:
        client = RecordingLookup(
            by_constraints={
                ("Shared title", "A Author", 2020): CrossrefRecord(doi="10.1000/a"),
                ("Other title", "B Author", 2021): CrossrefRecord(doi="10.1000/b"),
            }
        )
        articles = [
            Article(title="Shared title", authors="A Author", publication_year=2020),
            Article(title="Other title", authors="B Author", publication_year=2021),
        ]

        result = enrich_articles(articles, None, lookup=client, max_title_lookups=1)

        self.assertEqual(result[0].doi, "10.1000/a")
        self.assertEqual(result[1].doi, "")
        self.assertEqual(len(client.title_calls), 1)

    def test_cached_doi_answer_survives_an_exhausted_budget(self) -> None:
        session = FakeSession([FakeResponse({"message": dict(JOURNAL_MESSAGE)})])
        client = CrossrefClient(session)
        articles = [
            Article(title="Deep Learning for Medical Imaging", doi="10.1234/example.2024.001"),
            Article(title="Deep Learning for Medical Imaging (reprint)", doi="10.1234/example.2024.001"),
        ]

        result = enrich_articles(articles, None, lookup=client, max_lookups=1)

        self.assertEqual(result[0].journal, "Journal of Examples")
        self.assertEqual(result[1].journal, "Journal of Examples")
        self.assertEqual(len(session.requests), 1)

    def test_title_lookup_can_be_disabled(self) -> None:
        session = FakeSession([])
        client = CrossrefClient(session)
        articles = [Article(title="Deep Learning for Medical Imaging")]

        result = enrich_articles(articles, None, lookup=client, allow_title_lookup=False)

        self.assertEqual(result, articles)
        self.assertEqual(session.requests, [])


if __name__ == "__main__":
    unittest.main()
