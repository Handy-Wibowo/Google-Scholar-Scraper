import errno
import os
import tempfile
import unittest
from pathlib import Path
from threading import Event
from unittest import mock

import requests

from google_scholar_scraper.models import Article
from google_scholar_scraper.pdf_service import (
    MIN_PDF_BYTES,
    PDF_AVAILABLE,
    PDF_UNAVAILABLE,
    PDF_UNKNOWN,
    check_pdf_availability,
    download_pdfs,
    finalize_download,
    is_public_http_url,
    safe_pdf_filename,
)

PUBLIC_HOST = "https://repo.example.org"
PUBLIC_IP = "93.184.216.34"
OPENALEX_URL = "https://api.openalex.org/works/doi:10.1000/test"


def public_resolver(host: str) -> list[str]:
    return [PUBLIC_IP]


def pdf_bytes(size: int = 2048) -> bytes:
    return b"%PDF-1.4\n" + (b"x" * max(size - 9, 1))


class FakeResponse:
    def __init__(self, body: bytes = b"", status_code: int = 200, headers=None, chunks=None):
        self.status_code = status_code
        self.headers = headers if headers is not None else {}
        self._body = body
        self._chunks = chunks
        self.closed = False

    def iter_content(self, chunk_size: int = 65536):
        if self._chunks is not None:
            for item in self._chunks:
                if isinstance(item, Exception):
                    raise item
                yield item
            return
        if self._body:
            yield self._body

    def json(self):
        import json

        return json.loads(self._body.decode("utf-8"))

    def close(self) -> None:
        self.closed = True


class FakeSession:
    def __init__(self, routes=None):
        self.routes = dict(routes or {})
        self.calls: list[str] = []
        self.closed = False

    def get(self, url, **kwargs):
        self.calls.append(url)
        handler = self.routes.get(url)
        if handler is None:
            raise requests.ConnectionError(f"no fake route for {url}")
        if isinstance(handler, Exception):
            raise handler
        return handler

    def close(self) -> None:
        self.closed = True


class PublicUrlValidationTests(unittest.TestCase):
    def test_rejects_credentials_and_non_http_schemes(self) -> None:
        for url in (
            "file:///etc/passwd",
            "ftp://repo.example.org/a.pdf",
            "https://user:secret@repo.example.org/a.pdf",
            "//repo.example.org/a.pdf",
            "",
        ):
            with self.subTest(url=url):
                self.assertFalse(is_public_http_url(url, public_resolver))

    def test_accepts_bare_public_origin(self) -> None:
        # An origin with an empty path is a perfectly valid public HTTP URL.
        self.assertTrue(is_public_http_url("https://repo.example.org", public_resolver))
        self.assertTrue(is_public_http_url("https://repo.example.org/", public_resolver))

    def test_rejects_local_and_reserved_destinations(self) -> None:
        for url in (
            "http://127.0.0.1/a.pdf",
            "http://10.0.0.1/a.pdf",
            "http://192.168.1.5/a.pdf",
            "http://169.254.169.254/latest/meta-data/",
            "http://0.0.0.0/a.pdf",
            "http://224.0.0.1/a.pdf",
            "http://192.0.2.5/a.pdf",
            "http://100.64.0.1/a.pdf",
            "http://[::1]/a.pdf",
            "http://[fe80::1]/a.pdf",
            "http://[fc00::1]/a.pdf",
        ):
            with self.subTest(url=url):
                self.assertFalse(is_public_http_url(url, public_resolver))

    def test_accepts_public_http_destinations(self) -> None:
        self.assertTrue(is_public_http_url("https://repo.example.org/a.pdf", public_resolver))
        self.assertTrue(is_public_http_url(f"https://{PUBLIC_IP}/a.pdf", public_resolver))

    def test_host_resolving_to_private_address_is_rejected(self) -> None:
        self.assertFalse(is_public_http_url("https://evil.example.org/a.pdf", lambda host: ["10.1.2.3"]))

    def test_unsafe_link_makes_article_unavailable_without_any_request(self) -> None:
        session = FakeSession()
        article = Article(title="Paper", pdf_url="http://127.0.0.1/local.pdf")

        result = check_pdf_availability(article, session=session, resolver=public_resolver)

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertEqual(session.calls, [])


class PdfContentTests(unittest.TestCase):
    def test_real_pdf_content_is_available(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_AVAILABLE)
        self.assertEqual(result.pdf_source_url, url)
        self.assertIn("Validated PDF content", result.pdf_detail)

    def test_html_page_is_not_available(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(b"<html><body>login</body></html>", headers={"Content-Type": "text/html"})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertIn("HTML", result.pdf_detail)

    def test_html_masquerading_as_pdf_header_is_not_available(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        body = b"%PDF-1.4\n<html><body>Please sign in to download</body></html>"
        session = FakeSession({url: FakeResponse(body, headers={"Content-Type": "application/pdf"})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertIn("HTML", result.pdf_detail)

    def test_pdf_content_type_without_signature_is_not_available(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(b"not a pdf at all", headers={"Content-Type": "application/pdf"})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertIn("signature", result.pdf_detail)

    def test_empty_body_is_not_available(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(b"", headers={"Content-Type": "application/pdf"})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)

    def test_header_only_body_is_not_available(self) -> None:
        # A bare header is not a PDF; availability must not be claimed for it.
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession(
            {url: FakeResponse(b"%PDF-1.4", headers={"Content-Type": "application/pdf"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertIn("too small", result.pdf_detail)

    def test_truncated_header_is_not_available(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession(
            {url: FakeResponse(b"%PDF-", headers={"Content-Type": "application/pdf"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertIn("signature", result.pdf_detail)

    def test_pdf_split_across_small_chunks_is_available(self) -> None:
        # The signature and the body may arrive split over several tiny chunks;
        # the preview accumulates them instead of judging a single chunk.
        url = f"{PUBLIC_HOST}/a.pdf"
        body = pdf_bytes(MIN_PDF_BYTES + 32)
        chunks = [body[0:4], body[4:8], body[8:20], body[20:MIN_PDF_BYTES], body[MIN_PDF_BYTES:]]
        session = FakeSession(
            {
                url: FakeResponse(
                    chunks=chunks, headers={"Content-Type": "application/octet-stream"}
                )
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_AVAILABLE)
        self.assertEqual(result.pdf_source_url, url)

    def test_empty_chunks_between_real_content_are_ignored(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        body = pdf_bytes(MIN_PDF_BYTES + 16)
        session = FakeSession(
            {
                url: FakeResponse(
                    chunks=[b"", body[:4], b"", body[4:], b""],
                    headers={"Content-Type": "application/pdf"},
                )
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_AVAILABLE)

    def test_cancellation_during_preview_is_unknown(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        cancel_event = Event()

        def dribble():
            yield b"%PDF-1.4\n"
            cancel_event.set()
            yield b"x" * (MIN_PDF_BYTES * 2)

        session = FakeSession(
            {url: FakeResponse(chunks=dribble(), headers={"Content-Type": "application/pdf"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url),
            session=session,
            resolver=public_resolver,
            cancel_event=cancel_event,
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertIn("Cancelled", result.pdf_detail)

    def test_exact_minimum_size_is_available_in_check_and_download(self) -> None:
        # The availability preview and the download stream must agree on the
        # size boundary, so a check result can never contradict a later download.
        url = f"{PUBLIC_HOST}/boundary.pdf"
        body = b"%PDF-1.4\n" + b"x" * (MIN_PDF_BYTES - 9)
        self.assertEqual(len(body), MIN_PDF_BYTES)

        session = FakeSession({url: FakeResponse(body, headers={"Content-Type": "application/pdf"})})
        checked = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        with tempfile.TemporaryDirectory() as tmp:
            session2 = FakeSession(
                {url: FakeResponse(body, headers={"Content-Type": "application/pdf"})}
            )
            downloaded = download_pdfs(
                [Article(title="Paper", pdf_url=url)],
                tmp,
                session=session2,
                resolver=public_resolver,
            )[0]
            stored = Path(downloaded.pdf_local_path)
            self.assertEqual(stored.read_bytes(), body)

        self.assertEqual(checked.pdf_status, PDF_AVAILABLE)
        self.assertEqual(downloaded.pdf_status, PDF_AVAILABLE)

    def test_one_byte_below_minimum_is_unavailable_in_both_paths(self) -> None:
        url = f"{PUBLIC_HOST}/short.pdf"
        body = b"%PDF-1.4\n" + b"x" * (MIN_PDF_BYTES - 10)

        session = FakeSession({url: FakeResponse(body, headers={"Content-Type": "application/pdf"})})
        checked = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        with tempfile.TemporaryDirectory() as tmp:
            session2 = FakeSession(
                {url: FakeResponse(body, headers={"Content-Type": "application/pdf"})}
            )
            downloaded = download_pdfs(
                [Article(title="Paper", pdf_url=url)],
                tmp,
                session=session2,
                resolver=public_resolver,
            )[0]
            self.assertEqual(os.listdir(tmp), [])

        self.assertEqual(checked.pdf_status, PDF_UNAVAILABLE)
        self.assertEqual(downloaded.pdf_status, PDF_UNAVAILABLE)


class RedirectTests(unittest.TestCase):
    def test_public_redirect_chain_is_followed(self) -> None:
        start = f"{PUBLIC_HOST}/start"
        middle = f"{PUBLIC_HOST}/middle"
        final = f"{PUBLIC_HOST}/final.pdf"
        session = FakeSession(
            {
                start: FakeResponse(b"", 302, {"Location": middle}),
                middle: FakeResponse(b"", 301, {"Location": final}),
                final: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"}),
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=start), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_AVAILABLE)
        self.assertEqual(result.pdf_source_url, final)
        self.assertEqual(session.calls, [start, middle, final])

    def test_redirect_to_private_host_is_never_fetched(self) -> None:
        start = f"{PUBLIC_HOST}/start"
        session = FakeSession({start: FakeResponse(b"", 302, {"Location": "http://10.0.0.1/secret.pdf"})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=start), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertEqual(session.calls, [start])

    def test_redirect_to_link_local_metadata_endpoint_is_never_fetched(self) -> None:
        start = f"{PUBLIC_HOST}/start"
        session = FakeSession(
            {start: FakeResponse(b"", 302, {"Location": "http://169.254.169.254/latest/meta-data/"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=start), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertEqual(session.calls, [start])

    def test_redirect_loop_is_bounded(self) -> None:
        url = f"{PUBLIC_HOST}/loop"
        session = FakeSession({url: FakeResponse(b"", 302, {"Location": url})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertLessEqual(len(session.calls), 7)

    def test_redirect_without_location_is_unknown(self) -> None:
        url = f"{PUBLIC_HOST}/start"
        session = FakeSession({url: FakeResponse(b"", 302, {})})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)


class TransportTests(unittest.TestCase):
    def test_connection_error_is_unknown(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: requests.ConnectionError("boom")})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertIn("ConnectionError", result.pdf_detail)

    def test_timeout_is_unknown(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: requests.Timeout("slow")})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)

    def test_forbidden_and_server_errors_are_unknown(self) -> None:
        for status in (401, 403, 429, 500, 503):
            with self.subTest(status=status):
                url = f"{PUBLIC_HOST}/a{status}.pdf"
                session = FakeSession({url: FakeResponse(b"", status)})
                result = check_pdf_availability(
                    Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
                )
                self.assertEqual(result.pdf_status, PDF_UNKNOWN)

    def test_missing_and_gone_are_unavailable(self) -> None:
        for status in (404, 410):
            with self.subTest(status=status):
                url = f"{PUBLIC_HOST}/a{status}.pdf"
                session = FakeSession({url: FakeResponse(b"", status)})
                result = check_pdf_availability(
                    Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
                )
                self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
                self.assertIn(str(status), result.pdf_detail)

    def test_dns_failure_is_unknown(self) -> None:
        def failing_resolver(host: str) -> list[str]:
            raise OSError("dns down")

        session = FakeSession()
        result = check_pdf_availability(
            Article(title="Paper", pdf_url=f"{PUBLIC_HOST}/a.pdf"),
            session=session,
            resolver=failing_resolver,
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertEqual(session.calls, [])


class AbsentCandidateTests(unittest.TestCase):
    def test_record_without_link_or_identifier_is_unknown(self) -> None:
        # No candidate link and no DOI: nothing was consulted, so no source can
        # be said to have searched for and failed to find a public PDF.
        session = FakeSession()

        result = check_pdf_availability(Article(title="Paper"), session=session, resolver=public_resolver)

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertIn("candidate", result.pdf_detail)
        self.assertEqual(session.calls, [])

    def test_missing_link_without_identifier_is_unavailable(self) -> None:
        # A candidate link *was* consulted and rejected: that is a checked
        # absence, so the unknown branch must not swallow it.
        url = f"{PUBLIC_HOST}/gone.pdf"
        session = FakeSession({url: FakeResponse(b"", 404)})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertEqual(session.calls, [url])

    def test_unknown_candidate_is_not_reported_as_unavailable(self) -> None:
        good = f"{PUBLIC_HOST}/a.pdf"
        bad = f"{PUBLIC_HOST}/b.pdf"
        session = FakeSession({good: requests.ConnectionError("boom"), bad: FakeResponse(b"", 404)})

        result = check_pdf_availability(
            Article(title="Paper", pdf_url=good, pdf_source_url=bad),
            session=session,
            resolver=public_resolver,
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)


class FieldPreservationTests(unittest.TestCase):
    def test_check_preserves_unrelated_fields(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})
        article = Article(
            title="Original Title",
            authors="A. Author",
            link="https://scholar.google.com/x",
            relevance_score=0.75,
            doi="10.1000/test",
            journal="Journal of Tests",
            issns=("1234-5678",),
            publication_year=2021,
            sjr_quartile="Q2",
            sjr_year=2023,
            sjr_category="Education",
            sjr_source_url="https://scimagojr.example/x",
            sinta_rank="S3",
            sinta_year=2024,
            sinta_source_url="https://sinta.example/y",
            pdf_url=url,
            pdf_source_url="https://repo.example.org/landing",
            pdf_status="unknown",
            pdf_detail="",
            pdf_local_path="",
        )

        result = check_pdf_availability(article, session=session, resolver=public_resolver)

        self.assertIsNot(result, article)
        self.assertEqual(result.title, "Original Title")
        self.assertEqual(result.authors, "A. Author")
        self.assertEqual(result.link, "https://scholar.google.com/x")
        self.assertEqual(result.relevance_score, 0.75)
        self.assertEqual(result.doi, "10.1000/test")
        self.assertEqual(result.journal, "Journal of Tests")
        self.assertEqual(result.issns, ("1234-5678",))
        self.assertEqual(result.publication_year, 2021)
        self.assertEqual(result.sjr_quartile, "Q2")
        self.assertEqual(result.sjr_year, 2023)
        self.assertEqual(result.sjr_category, "Education")
        self.assertEqual(result.sjr_source_url, "https://scimagojr.example/x")
        self.assertEqual(result.sinta_rank, "S3")
        self.assertEqual(result.sinta_year, 2024)
        self.assertEqual(result.sinta_source_url, "https://sinta.example/y")
        # The candidate link is owned by the metadata module: never rewritten.
        self.assertEqual(result.pdf_url, url)
        self.assertEqual(result.pdf_local_path, "")
        self.assertEqual(result.pdf_status, PDF_AVAILABLE)

    def test_download_preserves_unrelated_fields_and_sets_local_path(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})
        article = Article(
            title="Preserved",
            authors="B. Author",
            link="https://scholar.google.com/y",
            relevance_score=0.5,
            doi="10.1000/keep",
            journal="Some Journal",
            pdf_url=url,
        )

        with tempfile.TemporaryDirectory() as tmp:
            results = download_pdfs(
                [article], tmp, session=session, resolver=public_resolver
            )

        self.assertEqual(len(results), 1)
        saved = results[0]
        self.assertEqual(saved.title, "Preserved")
        self.assertEqual(saved.authors, "B. Author")
        self.assertEqual(saved.link, "https://scholar.google.com/y")
        self.assertEqual(saved.relevance_score, 0.5)
        self.assertEqual(saved.doi, "10.1000/keep")
        self.assertEqual(saved.journal, "Some Journal")
        self.assertEqual(saved.pdf_url, url)
        self.assertEqual(saved.pdf_status, PDF_AVAILABLE)
        self.assertTrue(saved.pdf_local_path.endswith(".pdf"))


class FilenameTests(unittest.TestCase):
    def test_windows_unsafe_characters_are_removed(self) -> None:
        stem = safe_pdf_filename('A/B\\C:D*E?F"G<H>I|J')
        self.assertNotIn("/", stem)
        self.assertNotIn("\\", stem)
        self.assertNotIn(":", stem)
        self.assertNotIn("*", stem)
        self.assertNotIn("?", stem)
        self.assertNotIn('"', stem)
        self.assertNotIn("<", stem)
        self.assertNotIn(">", stem)
        self.assertNotIn("|", stem)

    def test_reserved_device_names_are_escaped(self) -> None:
        for name in ("CON", "con", "PRN", "AUX", "NUL", "COM1", "LPT9"):
            with self.subTest(name=name):
                self.assertNotEqual(safe_pdf_filename(name).upper(), name.upper())

    def test_reserved_device_names_with_extension_are_escaped(self) -> None:
        # Windows reserves the device name up to the first dot: CON.txt and
        # CON.tar.gz are just as unusable as CON.
        for name in ("CON.txt", "con.pdf", "PRN.data", "AUX.tar.gz", "COM1.notes", "LPT9.x.y"):
            with self.subTest(name=name):
                stem = safe_pdf_filename(name)
                self.assertTrue(stem.startswith("_"), stem)
                self.assertNotEqual(stem.split(".", 1)[0].upper(), name.split(".", 1)[0].upper())

    def test_empty_and_trailing_dot_titles_fall_back(self) -> None:
        self.assertEqual(safe_pdf_filename(""), "paper")
        self.assertEqual(safe_pdf_filename("   "), "paper")
        self.assertFalse(safe_pdf_filename("Title.").endswith("."))
        self.assertTrue(safe_pdf_filename("A" * 500))

    def test_colliding_titles_do_not_overwrite(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})
        articles = [Article(title="Same Title", pdf_url=url) for _ in range(3)]

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            existing = folder / "Same Title.pdf"
            existing.write_bytes(b"PRE-EXISTING")

            results = download_pdfs(articles, folder, session=session, resolver=public_resolver)

            self.assertEqual(existing.read_bytes(), b"PRE-EXISTING")
            paths = [item.pdf_local_path for item in results]
            self.assertEqual(len(set(paths)), 3)
            self.assertEqual(len([name for name in os.listdir(folder)]), 4)

    def test_same_name_different_article_in_one_batch_gets_unique_files(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            results = download_pdfs(
                [Article(title="Dup", pdf_url=url), Article(title="Dup", pdf_url=url)],
                folder,
                session=session,
                resolver=public_resolver,
            )

            names = sorted(Path(item.pdf_local_path).name for item in results)
            self.assertEqual(names, ["Dup (1).pdf", "Dup.pdf"])
            for item in results:
                self.assertTrue(Path(item.pdf_local_path).read_bytes().startswith(b"%PDF-"))


class SizeAndCleanupTests(unittest.TestCase):
    def test_oversize_download_is_unknown_and_leaves_no_file(self) -> None:
        url = f"{PUBLIC_HOST}/big.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(5000), headers={"Content-Type": "application/pdf"})})

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            results = download_pdfs(
                [Article(title="Big", pdf_url=url)],
                folder,
                session=session,
                resolver=public_resolver,
                max_bytes=1024,
            )

            self.assertEqual(results[0].pdf_status, PDF_UNKNOWN)
            self.assertIn("size limit", results[0].pdf_detail)
            self.assertEqual(results[0].pdf_local_path, "")
            self.assertEqual(os.listdir(folder), [])

    def test_transport_failure_mid_stream_cleans_partial_file(self) -> None:
        url = f"{PUBLIC_HOST}/broken.pdf"
        chunks = [pdf_bytes(256), requests.ConnectionError("reset")]
        session = FakeSession({url: FakeResponse(headers={"Content-Type": "application/pdf"}, chunks=chunks)})

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            results = download_pdfs(
                [Article(title="Broken", pdf_url=url)],
                folder,
                session=session,
                resolver=public_resolver,
            )

            self.assertEqual(results[0].pdf_status, PDF_UNKNOWN)
            self.assertEqual(os.listdir(folder), [])

    def test_cancellation_mid_stream_cleans_partial_file(self) -> None:
        url = f"{PUBLIC_HOST}/slow.pdf"
        cancel_event = Event()

        def chunks():
            yield pdf_bytes(256)
            cancel_event.set()
            yield pdf_bytes(256)

        session = FakeSession(
            {url: FakeResponse(headers={"Content-Type": "application/pdf"}, chunks=chunks())}
        )

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            results = download_pdfs(
                [Article(title="Slow", pdf_url=url)],
                folder,
                session=session,
                resolver=public_resolver,
                cancel_event=cancel_event,
            )

            self.assertEqual(results[0].pdf_status, PDF_UNKNOWN)
            self.assertIn("Cancelled", results[0].pdf_detail)
            self.assertEqual(os.listdir(folder), [])

    def test_cancel_before_start_keeps_every_entry_in_order(self) -> None:
        cancel_event = Event()
        cancel_event.set()
        session = FakeSession()
        articles = [Article(title=f"P{i}") for i in range(3)]

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            results = download_pdfs(
                articles, folder, session=session, resolver=public_resolver, cancel_event=cancel_event
            )

            self.assertEqual([item.title for item in results], ["P0", "P1", "P2"])
            self.assertTrue(all(item.pdf_status == PDF_UNKNOWN for item in results))
            self.assertEqual(session.calls, [])
            self.assertEqual(os.listdir(folder), [])

    def test_unwritable_folder_keeps_ordering_and_reports_failure(self) -> None:
        blocker = Path(tempfile.mkdtemp()) / "file-not-dir"
        blocker.write_bytes(b"x")

        results = download_pdfs(
            [Article(title="A"), Article(title="B")],
            blocker,
            session=FakeSession(),
            resolver=public_resolver,
        )

        self.assertEqual([item.title for item in results], ["A", "B"])
        # A local filesystem failure is not a statement about public
        # availability, so an unverified record must stay unknown.
        self.assertTrue(all(item.pdf_status == PDF_UNKNOWN for item in results))
        self.assertIn("could not be created", results[0].pdf_detail)


class DownloadFlowTests(unittest.TestCase):
    def test_mixed_batch_preserves_order_and_reports_progress(self) -> None:
        good = f"{PUBLIC_HOST}/good.pdf"
        missing = f"{PUBLIC_HOST}/missing.pdf"
        broken = f"{PUBLIC_HOST}/broken.pdf"
        session = FakeSession(
            {
                good: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"}),
                missing: FakeResponse(b"", 404),
                broken: requests.ConnectionError("boom"),
            }
        )
        articles = [
            Article(title="Good", pdf_url=good),
            Article(title="Missing", pdf_url=missing),
            Article(title="Broken", pdf_url=broken),
        ]
        events: list[tuple[int, int, str]] = []

        with tempfile.TemporaryDirectory() as tmp:
            results = download_pdfs(
                articles,
                tmp,
                session=session,
                resolver=public_resolver,
                progress_callback=lambda current, total, phase: events.append((current, total, phase)),
            )

        self.assertEqual([item.title for item in results], ["Good", "Missing", "Broken"])
        self.assertEqual(
            [item.pdf_status for item in results], [PDF_AVAILABLE, PDF_UNAVAILABLE, PDF_UNKNOWN]
        )
        # Two events per item: an opening "checking" phase and a terminal phase.
        # The per-item index never regresses, so the batch order is preserved.
        self.assertEqual([item[0] for item in events], [1, 1, 2, 2, 3, 3])
        self.assertTrue(all(item[1] == 3 for item in events))
        self.assertEqual([item[2] for item in events], ["checking", "saved", "checking", "unavailable", "checking", "unknown"])

    def test_download_creates_missing_folder(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "nested" / "pdfs"
            results = download_pdfs(
                [Article(title="Nested", pdf_url=url)],
                folder,
                session=session,
                resolver=public_resolver,
            )

            self.assertEqual(results[0].pdf_status, PDF_AVAILABLE)
            self.assertTrue(Path(results[0].pdf_local_path).is_file())

    def test_empty_input_returns_empty_list(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(download_pdfs([], tmp, session=FakeSession(), resolver=public_resolver), [])


class DoiDiscoveryTests(unittest.TestCase):
    def _openalex_payload(self, pdf_url: str, landing: str = "", *, is_oa: bool = True) -> bytes:
        import json

        return json.dumps(
            {
                "doi": "https://doi.org/10.1000/test",
                "open_access": {"oa_url": ""},
                "best_oa_location": {
                    "is_oa": is_oa,
                    "pdf_url": pdf_url,
                    "landing_page_url": landing,
                },
                "locations": [],
            }
        ).encode("utf-8")

    def test_doi_discovery_finds_public_pdf(self) -> None:
        pdf = f"{PUBLIC_HOST}/discovered.pdf"
        session = FakeSession(
            {
                OPENALEX_URL: FakeResponse(self._openalex_payload(pdf), 200, {"Content-Type": "application/json"}),
                pdf: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"}),
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_AVAILABLE)
        self.assertEqual(result.pdf_source_url, pdf)
        self.assertIn(OPENALEX_URL, session.calls)

    def test_doi_discovery_ignores_private_discovered_url(self) -> None:
        session = FakeSession(
            {
                OPENALEX_URL: FakeResponse(
                    self._openalex_payload("http://127.0.0.1/internal.pdf"),
                    200,
                    {"Content-Type": "application/json"},
                )
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertNotIn("http://127.0.0.1/internal.pdf", session.calls)

    def test_doi_discovery_provider_failure_stays_unknown(self) -> None:
        session = FakeSession({OPENALEX_URL: requests.ConnectionError("offline")})

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)

    def test_doi_discovery_without_result_is_unavailable(self) -> None:
        session = FakeSession(
            {
                OPENALEX_URL: FakeResponse(
                    b'{"open_access": {"oa_url": ""}, "locations": []}',
                    200,
                    {"Content-Type": "application/json"},
                )
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)

    def test_direct_candidate_is_used_before_doi_lookup(self) -> None:
        direct = f"{PUBLIC_HOST}/direct.pdf"
        session = FakeSession(
            {direct: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test", pdf_url=direct),
            session=session,
            resolver=public_resolver,
        )

        self.assertEqual(result.pdf_status, PDF_AVAILABLE)
        self.assertEqual(session.calls, [direct])

    def test_closed_location_is_not_treated_as_a_public_pdf(self) -> None:
        closed = f"{PUBLIC_HOST}/closed.pdf"
        session = FakeSession(
            {
                OPENALEX_URL: FakeResponse(
                    self._openalex_payload(closed, is_oa=False),
                    200,
                    {"Content-Type": "application/json"},
                )
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertNotIn(closed, session.calls)

    def test_landing_page_only_location_is_not_a_pdf_candidate(self) -> None:
        landing = f"{PUBLIC_HOST}/landing"
        session = FakeSession(
            {
                OPENALEX_URL: FakeResponse(
                    self._openalex_payload("", landing=landing),
                    200,
                    {"Content-Type": "application/json"},
                )
            }
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNAVAILABLE)
        self.assertNotIn(landing, session.calls)

    def test_index_redirect_to_private_host_is_never_followed(self) -> None:
        session = FakeSession(
            {OPENALEX_URL: FakeResponse(b"", 302, {"Location": "http://169.254.169.254/meta"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertEqual(session.calls, [OPENALEX_URL])

    def test_index_metadata_body_is_size_bounded(self) -> None:
        huge = b'{"locations": [' + (b" " * (2 * 1024 * 1024))
        session = FakeSession({OPENALEX_URL: FakeResponse(huge, 200, {"Content-Type": "application/json"})})

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)
        self.assertIn("index", result.pdf_detail)

    def test_index_malformed_body_stays_unknown(self) -> None:
        session = FakeSession(
            {OPENALEX_URL: FakeResponse(b"<html>nope</html>", 200, {"Content-Type": "text/html"})}
        )

        result = check_pdf_availability(
            Article(title="Paper", doi="10.1000/test"), session=session, resolver=public_resolver
        )

        self.assertEqual(result.pdf_status, PDF_UNKNOWN)


class SessionOwnershipTests(unittest.TestCase):
    def test_caller_session_is_never_closed(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})

        check_pdf_availability(Article(title="Paper", pdf_url=url), session=session, resolver=public_resolver)

        self.assertEqual(session.closed, False)

    def test_caller_session_is_never_closed_on_download(self) -> None:
        url = f"{PUBLIC_HOST}/a.pdf"
        session = FakeSession({url: FakeResponse(pdf_bytes(), headers={"Content-Type": "application/pdf"})})

        with tempfile.TemporaryDirectory() as tmp:
            download_pdfs([Article(title="A", pdf_url=url)], tmp, session=session, resolver=public_resolver)

        self.assertEqual(session.closed, False)


class LocalFailureStateTests(unittest.TestCase):
    def test_local_failure_does_not_downgrade_a_verified_status(self) -> None:
        blocker = Path(tempfile.mkdtemp()) / "file-not-dir"
        blocker.write_bytes(b"x")
        verified = Article(title="Known", pdf_status=PDF_AVAILABLE, pdf_detail="previously verified")

        results = download_pdfs([verified], blocker, session=FakeSession(), resolver=public_resolver)

        # The status stays verified; the detail carries the local operation failure.
        self.assertEqual(results[0].pdf_status, PDF_AVAILABLE)
        self.assertIn("could not be created", results[0].pdf_detail)

    def test_cancellation_does_not_downgrade_a_verified_status(self) -> None:
        cancel_event = Event()
        cancel_event.set()
        verified = Article(title="Known", pdf_status=PDF_AVAILABLE, pdf_detail="previously verified")

        with tempfile.TemporaryDirectory() as tmp:
            results = download_pdfs(
                [verified], tmp, session=FakeSession(), resolver=public_resolver, cancel_event=cancel_event
            )

        self.assertEqual(results[0].pdf_status, PDF_AVAILABLE)
        self.assertIn("Cancelled", results[0].pdf_detail)


class FinalizationRaceTests(unittest.TestCase):
    def test_existing_destination_is_never_overwritten_by_finalize(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            destination = folder / "Race.pdf"
            destination.write_bytes(b"WINNER")

            temp_path = folder / ".pdf-part-race.tmp"
            temp_path.write_bytes(b"%PDF-1.4\nloser")

            from google_scholar_scraper.pdf_service import finalize_download

            claimed = finalize_download(str(temp_path), folder, "Race", set())

            self.assertNotEqual(claimed, destination)
            self.assertEqual(destination.read_bytes(), b"WINNER")
            self.assertTrue(claimed.read_bytes().startswith(b"%PDF-"))

    def test_repeated_finalize_of_same_title_never_collides(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            from google_scholar_scraper.pdf_service import finalize_download

            taken: set[str] = set()
            written = []
            for index in range(3):
                temp_path = folder / f".pdf-part-{index}.tmp"
                temp_path.write_bytes(b"%PDF-1.4\n" + bytes([index]))
                written.append(finalize_download(str(temp_path), folder, "Dup", taken))

            self.assertEqual(len({str(path) for path in written}), 3)
            self.assertEqual(sorted(path.name for path in written), ["Dup (1).pdf", "Dup (2).pdf", "Dup.pdf"])
            for index, path in enumerate(written):
                self.assertEqual(path.read_bytes(), b"%PDF-1.4\n" + bytes([index]))


class FinalizationFallbackTests(unittest.TestCase):
    """The no-hardlink fallback must be exercised, not merely code-inspected."""

    @staticmethod
    def _no_hardlink():
        return mock.patch(
            "google_scholar_scraper.pdf_service.os.link",
            side_effect=OSError(errno.EPERM, "hard links not supported"),
        )

    def test_reservation_fallback_publishes_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            temp_path = folder / ".pdf-part-a.tmp"
            temp_path.write_bytes(b"%PDF-1.4\nbody")

            with self._no_hardlink():
                claimed = finalize_download(str(temp_path), folder, "Fallback", set())

            self.assertEqual(claimed.name, "Fallback.pdf")
            self.assertEqual(claimed.read_bytes(), b"%PDF-1.4\nbody")
            self.assertFalse(temp_path.exists())
            self.assertEqual(sorted(p.name for p in folder.iterdir()), ["Fallback.pdf"])

    def test_reservation_fallback_never_overwrites_existing_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            existing = folder / "Fallback.pdf"
            existing.write_bytes(b"WINNER")
            temp_path = folder / ".pdf-part-b.tmp"
            temp_path.write_bytes(b"%PDF-1.4\nloser")

            with self._no_hardlink():
                claimed = finalize_download(str(temp_path), folder, "Fallback", set())

            self.assertNotEqual(claimed, existing)
            self.assertEqual(existing.read_bytes(), b"WINNER")
            self.assertEqual(claimed.name, "Fallback (1).pdf")
            self.assertEqual(claimed.read_bytes(), b"%PDF-1.4\nloser")

    def test_failed_replace_removes_the_reserved_placeholder(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            temp_path = folder / ".pdf-part-c.tmp"
            temp_path.write_bytes(b"%PDF-1.4\nbody")

            with self._no_hardlink(), mock.patch(
                "google_scholar_scraper.pdf_service.os.replace",
                side_effect=OSError(errno.EACCES, "replace denied"),
            ):
                with self.assertRaises(OSError):
                    finalize_download(str(temp_path), folder, "Fallback", set())

            # The reservation placeholder must not be left behind as a fake PDF.
            self.assertEqual(sorted(p.name for p in folder.iterdir()), [".pdf-part-c.tmp"])


class DownloadStreamValidationTests(unittest.TestCase):
    def test_download_accepts_pdf_split_across_small_chunks(self) -> None:
        url = f"{PUBLIC_HOST}/split.pdf"
        body = pdf_bytes(MIN_PDF_BYTES + 64)
        chunks = [body[0:3], body[3:9], body[9:40], body[40:]]
        session = FakeSession(
            {
                url: FakeResponse(
                    chunks=chunks, headers={"Content-Type": "application/octet-stream"}
                )
            }
        )

        with tempfile.TemporaryDirectory() as tmp:
            results = download_pdfs(
                [Article(title="Split", pdf_url=url)],
                tmp,
                session=session,
                resolver=public_resolver,
            )

            self.assertEqual(results[0].pdf_status, PDF_AVAILABLE)
            stored = Path(results[0].pdf_local_path)
            self.assertEqual(stored.read_bytes(), body)
            self.assertEqual(sorted(p.name for p in Path(tmp).iterdir()), ["Split.pdf"])

    def test_download_rejects_header_only_body_and_cleans_up(self) -> None:
        url = f"{PUBLIC_HOST}/header-only.pdf"
        session = FakeSession(
            {url: FakeResponse(b"%PDF-1.4", headers={"Content-Type": "application/pdf"})}
        )

        with tempfile.TemporaryDirectory() as tmp:
            results = download_pdfs(
                [Article(title="Header", pdf_url=url)],
                tmp,
                session=session,
                resolver=public_resolver,
            )

            self.assertEqual(results[0].pdf_status, PDF_UNAVAILABLE)
            self.assertEqual(results[0].pdf_local_path, "")
            self.assertEqual(os.listdir(tmp), [])


if __name__ == "__main__":
    unittest.main()
