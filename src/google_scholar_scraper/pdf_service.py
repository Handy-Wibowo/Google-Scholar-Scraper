"""Public-PDF availability checks and bounded, atomic downloads.

States are exactly three (shared contract):
- ``available``:   a reachable URL served validated PDF content; for
  :func:`download_pdfs` it is also stored at ``pdf_local_path``.
- ``unavailable``: every candidate offered by the checked sources was reachable
  and rejected as a non-PDF. This means "no public PDF was found by the checked
  sources" -- never proof of universal absence.
- ``unknown``:     a provider/transport problem or an uncertain candidate stopped
  verification (DNS failure, 401/403/429/5xx, timeout, oversize body, redirect to
  a non-public host, cancellation).

Security: http(s) only, no URL credentials, no local/private/link-local/multicast/
reserved destinations (re-checked on every redirect hop), TLS verification always
on, bounded redirect chain, timeouts and read size. No credentials, no
paywall/CAPTCHA/login bypass, no browser dependency.

Only ``pdf_status``, ``pdf_detail`` and ``pdf_source_url`` are written back (plus
``pdf_local_path`` on a successful download); candidate fields owned by the
metadata module (``pdf_url``, bibliographic fields) are left untouched.
"""

import ipaddress
import json
import os
import re
import socket
import tempfile
import unicodedata
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path
from threading import Event
from urllib.parse import quote, urlsplit

import requests

from google_scholar_scraper.models import Article

PDF_AVAILABLE = "available"
PDF_UNAVAILABLE = "unavailable"
PDF_UNKNOWN = "unknown"

DEFAULT_TIMEOUT = (5, 20)
DEFAULT_MAX_BYTES = 50 * 1024 * 1024
MAX_REDIRECTS = 5
CHUNK_SIZE = 64 * 1024
MIN_PDF_BYTES = 64
MAX_METADATA_BYTES = 1024 * 1024
MAX_NAME_ATTEMPTS = 10000

# Documented, credential-free public endpoint (OpenAlex works API, `doi:` filter).
# https://docs.openalex.org/api-entities/works/get-a-single-work
OPENALEX_WORK_URL = "https://api.openalex.org/works/doi:{}"

DEFAULT_HEADERS = {
    "User-Agent": (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
        "AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/126.0 Safari/537.36"
    ),
    "Accept": "application/pdf,*/*;q=0.8",
}

# 4xx that mean "there is nothing here", as opposed to "we could not check".
_REJECT_STATUSES = frozenset({400, 404, 405, 406, 410, 451})
_HTML_MARKERS = (b"<html", b"<!doctype html", b"<head", b"<body")
# A real PDF starts with a versioned header; "%PDF-" alone is not enough.
_PDF_SIGNATURE = re.compile(rb"^%PDF-\d+\.\d+")

_ILLEGAL_NAME_CHARS = re.compile(r'[<>:"/\\|?*\x00-\x1f]')
_WHITESPACE = re.compile(r"\s+")
_WINDOWS_RESERVED = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}
_MAX_STEM = 120

Resolver = Callable[[str], list[str]]


class _CandidateRejected(Exception):
    """The candidate is reachable but is not a public PDF."""


class _CandidateUnverified(Exception):
    """The candidate could not be verified: transport or provider problem."""


class _Cancelled(Exception):
    """Cancellation was requested while working on a candidate."""


@dataclass
class _Outcome:
    status: str
    detail: str
    source_url: str = ""
    local_path: str = ""
    # True when the reason for a non-available status is a local/operational
    # failure (disk, folder, cancellation) rather than a statement about public
    # availability. The caller must not let such a failure downgrade a status
    # that a previous check already verified.
    local_failure: bool = False


_STATUS_RANK = {PDF_UNAVAILABLE: 0, PDF_UNKNOWN: 1, PDF_AVAILABLE: 2}


def check_pdf_availability(
    article: Article,
    cancel_event: Event | None = None,
    *,
    session: requests.Session | None = None,
    timeout: tuple[int, int] = DEFAULT_TIMEOUT,
    resolver: Resolver | None = None,
) -> Article:
    """Validate public PDF candidates for ``article`` and label ``pdf_status``.

    Direct candidates come from the record itself (``pdf_url``); when a DOI is
    present, a documented credential-free OA lookup may add candidates. Never
    marks an HTML/login page as available. Unrelated fields are preserved.
    """
    active = session or _new_session()
    owned = session is None
    try:
        outcome = _evaluate(
            article,
            session=active,
            timeout=timeout,
            max_bytes=DEFAULT_MAX_BYTES,
            resolver=resolver,
            cancel_event=cancel_event,
            folder=None,
            taken=set(),
        )
    finally:
        # Only close what this call created; a caller-supplied session stays open.
        if owned:
            active.close()
    if outcome.local_failure:
        # A local problem (nothing to do with public availability) must not
        # overwrite a status an earlier check already verified.
        return replace(article, pdf_detail=outcome.detail)
    return replace(
        article,
        pdf_status=outcome.status,
        pdf_detail=outcome.detail,
        pdf_source_url=outcome.source_url or article.pdf_source_url,
    )


def download_pdfs(
    articles: list[Article],
    folder: str | Path,
    cancel_event: Event | None = None,
    progress_callback: Callable[[int, int, str], None] | None = None,
    *,
    session: requests.Session | None = None,
    timeout: tuple[int, int] = DEFAULT_TIMEOUT,
    max_bytes: int = DEFAULT_MAX_BYTES,
    resolver: Resolver | None = None,
) -> list[Article]:
    """Download public PDFs for ``articles`` into ``folder``.

    Input order is preserved, including failures and cancelled entries. Each item
    is streamed to a temporary sibling file and atomically finalised, so no
    existing file is ever overwritten and partial files are removed.
    """
    active = session or _new_session()
    owned = session is None
    target = Path(folder)
    try:
        target.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        detail = f"Download folder could not be created: {exc.__class__.__name__}"
        total = len(articles)
        # Nothing was checked, so no paper's public availability may be
        # re-labelled: keep each record's verified state and report the local
        # failure in the detail.
        results = [_preserve(article, detail) for article in articles]
        for index in range(total):
            _emit_progress(progress_callback, index + 1, total, "failed")
        if owned:
            active.close()
        return results

    taken: set[str] = set()
    results: list[Article] = []
    total = len(articles)

    try:
        for index, article in enumerate(articles):
            position = index + 1
            if _is_cancelled(cancel_event):
                results.append(_preserve(article, "Cancelled before this item was checked."))
                _emit_progress(progress_callback, position, total, "cancelled")
                continue

            _emit_progress(progress_callback, position, total, "checking")
            outcome = _evaluate(
                article,
                session=active,
                timeout=timeout,
                max_bytes=max_bytes,
                resolver=resolver,
                cancel_event=cancel_event,
                folder=target,
                taken=taken,
            )

            if outcome.status == PDF_AVAILABLE:
                _emit_progress(progress_callback, position, total, "saved")
                results.append(
                    replace(
                        article,
                        pdf_status=PDF_AVAILABLE,
                        pdf_detail=outcome.detail,
                        pdf_source_url=outcome.source_url,
                        pdf_local_path=outcome.local_path,
                    )
                )
            elif outcome.local_failure:
                # Disk/cancel failures say nothing about public availability.
                _emit_progress(
                    progress_callback,
                    position,
                    total,
                    "cancelled" if outcome.detail.startswith("Cancelled") else "failed",
                )
                results.append(_preserve(article, outcome.detail))
            else:
                _emit_progress(progress_callback, position, total, outcome.status)
                results.append(
                    replace(
                        article,
                        pdf_status=outcome.status,
                        pdf_detail=outcome.detail,
                        pdf_source_url=outcome.source_url or article.pdf_source_url,
                    )
                )
    finally:
        if owned:
            active.close()

    return results


def _evaluate(
    article: Article,
    *,
    session: requests.Session,
    timeout: tuple[int, int],
    max_bytes: int,
    resolver: Resolver | None,
    cancel_event: Event | None,
    folder: Path | None,
    taken: set[str],
) -> _Outcome:
    direct = _direct_candidates(article)
    doi = (article.doi or "").strip()
    if not direct and not doi:
        # Nothing was consulted: the record carries no candidate link and no
        # identifier, so no source could have been searched. That is an
        # unchecked record, not a checked absence.
        return _Outcome(
            PDF_UNKNOWN,
            "No PDF candidate link or DOI identifier on this record; no public PDF "
            "source could be consulted.",
        )
    outcome = _try_candidates(
        direct,
        session=session,
        timeout=timeout,
        max_bytes=max_bytes,
        resolver=resolver,
        cancel_event=cancel_event,
        folder=folder,
        taken=taken,
        title=article.title,
    )
    if outcome.status == PDF_AVAILABLE or _is_cancelled(cancel_event):
        return outcome

    if not doi:
        return outcome

    discovered = _discover_oa_candidates(
        doi, session=session, timeout=timeout, resolver=resolver, cancel_event=cancel_event
    )
    if discovered is None:
        # The index could not be consulted, so a clean rejection from the direct
        # candidates is not a complete search result.
        if outcome.status == PDF_UNAVAILABLE:
            return _Outcome(
                PDF_UNKNOWN,
                "No public PDF at the record's own link, and the public PDF index "
                "could not be consulted.",
            )
        return outcome
    if not discovered:
        return outcome

    second = _try_candidates(
        discovered,
        session=session,
        timeout=timeout,
        max_bytes=max_bytes,
        resolver=resolver,
        cancel_event=cancel_event,
        folder=folder,
        taken=taken,
        title=article.title,
    )
    # A confirmed PDF wins; otherwise the more informative of the two attempts
    # (an unverified transport problem outranks a clean rejection).
    if _STATUS_RANK.get(second.status, -1) > _STATUS_RANK.get(outcome.status, -1):
        return second
    return outcome


def _try_candidates(
    urls: list[str],
    *,
    session: requests.Session,
    timeout: tuple[int, int],
    max_bytes: int,
    resolver: Resolver | None,
    cancel_event: Event | None,
    folder: Path | None,
    taken: set[str],
    title: str,
) -> _Outcome:
    if not urls:
        return _Outcome(PDF_UNAVAILABLE, "No public PDF link found on this record.")

    rejection = ""
    unverified = ""

    for url in urls:
        if _is_cancelled(cancel_event):
            return _Outcome(PDF_UNKNOWN, "Cancelled while checking public PDF links.")

        response = None
        try:
            response = _open_candidate(url, session=session, timeout=timeout, resolver=resolver)
            resolved = getattr(response, "url", "") or url
            if folder is None:
                try:
                    preview = _read_preview(response, cancel_event)
                except requests.RequestException as exc:
                    raise _CandidateUnverified(
                        f"Transport error while reading the public PDF: {exc.__class__.__name__}"
                    ) from exc
                _validate_pdf_preview(preview)
                return _Outcome(
                    PDF_AVAILABLE,
                    f"Validated PDF content from {resolved}",
                    source_url=resolved,
                )

            temp_path = _stream_to_file(
                response, folder, max_bytes, cancel_event, title
            )
            try:
                final_path = finalize_download(temp_path, folder, title, taken)
            except BaseException:
                _discard(temp_path)
                raise
            return _Outcome(
                PDF_AVAILABLE,
                f"Downloaded PDF from {resolved}",
                source_url=resolved,
                local_path=str(final_path),
            )
        except _Cancelled:
            return _Outcome(
                PDF_UNKNOWN,
                "Cancelled while downloading a public PDF.",
                local_failure=True,
            )
        except OSError as exc:
            # A local disk/filesystem failure is not a statement about public
            # availability: keep any status already verified for this record.
            return _Outcome(
                PDF_UNKNOWN,
                f"Could not store the public PDF locally: {exc.__class__.__name__}",
                local_failure=True,
            )
        except _CandidateRejected as exc:
            rejection = str(exc)
        except _CandidateUnverified as exc:
            unverified = str(exc)
        finally:
            if response is not None:
                response.close()

    if unverified:
        return _Outcome(PDF_UNKNOWN, unverified)
    return _Outcome(PDF_UNAVAILABLE, rejection or "No public PDF found at the checked link(s).")


def _open_candidate(
    url: str,
    *,
    session: requests.Session,
    timeout: tuple[int, int],
    resolver: Resolver | None,
) -> requests.Response:
    """Return a streamed response for ``url``, following redirects by hand.

    Every hop is re-validated so an automatic redirect can never land on a
    local/private destination.
    """
    current = url
    for hop in range(MAX_REDIRECTS + 1):
        try:
            _require_public_url(current, resolver)
        except _CandidateRejected as exc:
            if hop == 0:
                raise
            # A redirect that leaves the public internet is inconclusive, not
            # evidence that no public PDF exists at the original candidate.
            raise _CandidateUnverified(f"Redirect left the public internet: {exc}") from exc

        try:
            response = session.get(
                current,
                timeout=timeout,
                allow_redirects=False,
                stream=True,
                headers=DEFAULT_HEADERS,
            )
        except requests.RequestException as exc:
            raise _CandidateUnverified(
                f"Transport error while fetching the public PDF link: {exc.__class__.__name__}"
            ) from exc

        status = getattr(response, "status_code", 0)
        if status in (301, 302, 303, 307, 308):
            location = response.headers.get("Location") or ""
            response.close()
            if not location:
                raise _CandidateUnverified("Redirect response carried no Location header.")
            current = requests.compat.urljoin(current, location)
            continue

        if status in _REJECT_STATUSES:
            response.close()
            raise _CandidateRejected(f"Public PDF link returned HTTP {status}.")

        if status >= 400 or status < 200:
            response.close()
            raise _CandidateUnverified(f"Public PDF link returned HTTP {status}.")

        content_type = (response.headers.get("Content-Type") or "").lower()
        if content_type.startswith("text/html"):
            response.close()
            raise _CandidateRejected("Public PDF link returned an HTML page, not a PDF.")

        # Record where the chain actually landed: the verified source URL is the
        # resolved one, not the candidate we started from.
        response.url = current
        return response

    raise _CandidateUnverified("Too many redirects while resolving the public PDF link.")


def _validate_pdf_chunk(chunk: bytes) -> None:
    """Lightweight header/content validation -- not a complete integrity check.

    Confirms a versioned PDF signature and the absence of HTML markers. It does
    not parse the document, so a corrupt body behind a valid header passes.
    """
    if not chunk:
        raise _CandidateRejected("Public PDF link returned an empty response.")
    if not _PDF_SIGNATURE.match(chunk):
        raise _CandidateRejected(
            "Response does not start with a PDF signature; not treated as a public PDF."
        )
    head = chunk[:1024].lower()
    if any(marker in head for marker in _HTML_MARKERS):
        raise _CandidateRejected("Response carries a PDF header but HTML content.")


def _validate_pdf_preview(preview: bytes) -> None:
    """Header/content validation over an accumulated preview.

    A PDF that arrives as several small chunks, or whose header is split across
    a chunk boundary, is judged on the accumulated bytes rather than on a single
    chunk. The preview must also carry enough real content to be a PDF, so a bare
    ``%PDF-1.4`` header alone is not enough. Lightweight: no document parsing.
    """
    _validate_pdf_chunk(preview)
    if len(preview) < MIN_PDF_BYTES:
        raise _CandidateRejected("Response is too small to be a valid PDF.")


def _read_preview(
    response: requests.Response,
    cancel_event: Event | None,
    needed: int = MIN_PDF_BYTES,
) -> bytes:
    """Accumulate a small bounded preview so split/short chunks still validate.

    Stops as soon as ``needed`` bytes are in hand; a provider that dribbles the
    header out over several tiny chunks is handled just like a single big one.
    """
    parts: list[bytes] = []
    total = 0
    for chunk in response.iter_content(CHUNK_SIZE):
        if _is_cancelled(cancel_event):
            raise _Cancelled()
        if not chunk:
            continue
        parts.append(chunk)
        total += len(chunk)
        if total >= needed:
            break
    return b"".join(parts)


def _stream_to_file(
    response: requests.Response,
    folder: Path,
    max_bytes: int,
    cancel_event: Event | None,
    title: str,
) -> str:
    handle = None
    temp_path = ""
    succeeded = False
    try:
        descriptor, temp_path = tempfile.mkstemp(dir=folder, prefix=".pdf-part-", suffix=".tmp")
        handle = os.fdopen(descriptor, "wb")
        total = 0
        head = bytearray()
        validated = False

        for chunk in response.iter_content(CHUNK_SIZE):
            if _is_cancelled(cancel_event):
                raise _Cancelled()
            if not chunk:
                continue
            total += len(chunk)
            if total > max_bytes:
                raise _CandidateUnverified(
                    f"Public PDF exceeds the {max_bytes // (1024 * 1024)} MB size limit; not stored."
                )
            if not validated:
                # Validate the accumulated head, not a single chunk: a provider
                # may split the signature or the first bytes across chunks.
                head.extend(chunk)
                if len(head) < MIN_PDF_BYTES:
                    continue
                _validate_pdf_preview(bytes(head))
                handle.write(head)
                head.clear()
                validated = True
                continue
            handle.write(chunk)

        if not validated:
            # Fewer than MIN_PDF_BYTES arrived in total: empty, header-only or
            # truncated content is rejected rather than stored as a PDF.
            _validate_pdf_preview(bytes(head))
            handle.write(head)

        handle.flush()
        handle.close()
        handle = None
        succeeded = True
        return temp_path
    except BaseException as exc:
        if isinstance(exc, requests.RequestException):
            raise _CandidateUnverified(
                f"Transport error while downloading the public PDF: {exc.__class__.__name__}"
            ) from exc
        raise
    finally:
        # Close first, then remove: Windows refuses to delete an open file, so an
        # unclosed handle would strand the partial download in the folder.
        if handle is not None:
            try:
                handle.close()
            except OSError:
                pass
            handle = None
        if not succeeded:
            _discard(temp_path)


def finalize_download(temp_path: str, folder: Path, title: str, taken: set[str]) -> Path:
    """Move a verified temp file to a safe name, never overwriting any file.

    The finalisation is atomic and exclusive: the name is claimed with an
    OS-level primitive that fails if the destination already exists, so a
    concurrent writer cannot be silently overwritten by the exists()-then-move
    race. ``taken`` only records names this batch already used.
    """
    stem = _safe_filename(title)
    counter = 0
    while True:
        name = f"{stem}.pdf" if counter == 0 else f"{stem} ({counter}).pdf"
        candidate = folder / name
        if name.casefold() in taken:
            counter += 1
            if counter > MAX_NAME_ATTEMPTS:
                raise OSError("Could not find a free file name for the downloaded PDF.")
            continue
        if _claim_path(temp_path, candidate):
            taken.add(name.casefold())
            return candidate
        counter += 1
        if counter > MAX_NAME_ATTEMPTS:
            raise OSError("Could not find a free file name for the downloaded PDF.")


def _claim_path(temp_path: str, candidate: Path) -> bool:
    """Atomically publish ``temp_path`` at ``candidate``; False if it exists."""
    try:
        # Hard link is exclusive: it fails with FileExistsError when the
        # destination is already there, and never replaces it.
        os.link(temp_path, candidate)
    except FileExistsError:
        return False
    except OSError:
        return _claim_path_by_reservation(temp_path, candidate)
    else:
        _discard(temp_path)
        return True


def _claim_path_by_reservation(temp_path: str, candidate: Path) -> bool:
    """Fallback for filesystems without hard links: reserve, then replace.

    ``O_CREAT | O_EXCL`` claims the name atomically; the reserved placeholder is
    then overwritten by our own temp file, so no other writer's file is touched.
    """
    try:
        descriptor = os.open(candidate, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        return False
    os.close(descriptor)
    try:
        os.replace(temp_path, candidate)
    except BaseException:
        _discard(str(candidate))
        raise
    return True


def _safe_filename(title: str) -> str:
    stem = unicodedata.normalize("NFKC", title or "")
    stem = _ILLEGAL_NAME_CHARS.sub(" ", stem)
    stem = _WHITESPACE.sub(" ", stem).strip().rstrip(".").strip()
    stem = stem[:_MAX_STEM].rstrip(" .")
    if not stem:
        stem = "paper"
    # Windows reserves the device name before the first dot: CON, CON.txt and
    # CON.tar.gz are all unusable.
    if stem.split(".", 1)[0].upper() in _WINDOWS_RESERVED:
        stem = f"_{stem}"
    return stem


def _discard(path: str) -> None:
    if not path:
        return
    try:
        os.unlink(path)
    except OSError:
        pass


def _direct_candidates(article: Article) -> list[str]:
    seen: set[str] = set()
    urls: list[str] = []
    for value in (article.pdf_url, article.pdf_source_url):
        url = (value or "").strip()
        if url and url not in seen:
            seen.add(url)
            urls.append(url)
    return urls


def _discover_oa_candidates(
    doi: str,
    *,
    session: requests.Session,
    timeout: tuple[int, int],
    resolver: Resolver | None,
    cancel_event: Event | None = None,
) -> list[str] | None:
    """Ask a documented, credential-free OA index for public PDF candidates.

    Returns ``None`` when the index itself could not be consulted -- the caller
    must then stay ``unknown`` instead of claiming a complete search. An empty
    list means the index answered and knows of no public PDF.

    Only locations the index itself flags as open access are considered, and only
    their ``pdf_url``: a closed publisher location or an arbitrary landing page is
    not a public PDF candidate. Every redirect hop is re-validated and the
    metadata body is size-bounded.
    """
    api_url = OPENALEX_WORK_URL.format(quote(doi, safe="/"))
    try:
        payload = _open_metadata(
            api_url,
            session=session,
            timeout=timeout,
            resolver=resolver,
            cancel_event=cancel_event,
        )
    except (_CandidateRejected, _CandidateUnverified):
        return None

    urls: list[str] = []
    locations = [payload.get("best_oa_location"), *(payload.get("locations") or [])]
    for location in locations:
        if not isinstance(location, dict):
            continue
        if location.get("is_oa") is not True:
            continue
        url = location.get("pdf_url")
        if isinstance(url, str) and url.strip() and url.strip() not in urls:
            urls.append(url.strip())
        if len(urls) >= 3:
            break

    return urls


def _open_metadata(
    url: str,
    *,
    session: requests.Session,
    timeout: tuple[int, int],
    resolver: Resolver | None,
    cancel_event: Event | None,
) -> dict:
    """Fetch a small JSON document with the same hop validation as a PDF fetch."""
    current = url
    for _hop in range(MAX_REDIRECTS + 1):
        _require_public_url(current, resolver)
        if _is_cancelled(cancel_event):
            raise _Cancelled()

        try:
            response = session.get(
                current,
                timeout=timeout,
                allow_redirects=False,
                stream=True,
                headers=DEFAULT_HEADERS,
            )
        except requests.RequestException as exc:
            raise _CandidateUnverified(
                f"Transport error while consulting the public PDF index: {exc.__class__.__name__}"
            ) from exc

        try:
            status = getattr(response, "status_code", 0)
            if status in (301, 302, 303, 307, 308):
                location = response.headers.get("Location") or ""
                if not location:
                    raise _CandidateUnverified("Index redirect carried no Location header.")
                current = requests.compat.urljoin(current, location)
                continue
            if status != 200:
                raise _CandidateUnverified(f"Public PDF index returned HTTP {status}.")
            body = _read_bounded(response, MAX_METADATA_BYTES, cancel_event)
        finally:
            response.close()

        try:
            payload = json.loads(body.decode("utf-8"))
        except (UnicodeDecodeError, ValueError) as exc:
            raise _CandidateUnverified("Public PDF index returned a malformed JSON body.") from exc
        if not isinstance(payload, dict):
            raise _CandidateUnverified("Public PDF index returned an unexpected payload.")
        return payload

    raise _CandidateUnverified("Too many redirects while consulting the public PDF index.")


def _read_bounded(
    response: requests.Response,
    limit: int,
    cancel_event: Event | None,
) -> bytes:
    """Read at most ``limit`` bytes from a streamed response."""
    parts: list[bytes] = []
    total = 0
    for chunk in response.iter_content(CHUNK_SIZE):
        if _is_cancelled(cancel_event):
            raise _Cancelled()
        if not chunk:
            continue
        total += len(chunk)
        if total > limit:
            raise _CandidateUnverified(
                f"Public PDF index response exceeded the {limit // 1024} KB metadata limit."
            )
        parts.append(chunk)
    return b"".join(parts)


def _require_public_url(url: str, resolver: Resolver | None) -> None:
    """Reject anything that is not a public http(s) destination."""
    candidate = (url or "").strip()
    if not candidate:
        raise _CandidateRejected("Empty PDF link.")

    parsed = urlsplit(candidate)
    if parsed.scheme.lower() not in {"http", "https"}:
        raise _CandidateRejected("PDF link is not an http(s) URL.")
    if parsed.username or parsed.password:
        raise _CandidateRejected("PDF link carries embedded credentials.")
    host = parsed.hostname or ""
    if not host:
        raise _CandidateRejected("PDF link has no host.")

    if _parse_address(host) is not None:
        _require_public_address(host)
        return

    try:
        addresses = (resolver or _resolve_addresses)(host)
    except OSError as exc:
        raise _CandidateUnverified(
            f"Could not resolve the PDF link host: {exc.__class__.__name__}"
        ) from exc
    if not addresses:
        raise _CandidateUnverified("PDF link host resolved to no addresses.")
    for address in addresses:
        _require_public_address(address)


def _resolve_addresses(host: str) -> list[str]:
    infos = socket.getaddrinfo(host, None, proto=socket.IPPROTO_TCP)
    return [info[4][0] for info in infos]


def _parse_address(host: str):
    try:
        return ipaddress.ip_address(host)
    except ValueError:
        return None


def _require_public_address(address: str) -> None:
    parsed = _parse_address(address)
    if parsed is None:
        raise _CandidateUnverified(f"Unrecognised address for the PDF link host: {address}")
    if not parsed.is_global or parsed.is_multicast:
        raise _CandidateRejected(
            "PDF link resolves to a local, private, link-local, multicast or reserved address."
        )


def _new_session() -> requests.Session:
    session = requests.Session()
    session.trust_env = False
    return session


def _is_cancelled(cancel_event: Event | None) -> bool:
    return bool(cancel_event and cancel_event.is_set())


def _preserve(article: Article, detail: str) -> Article:
    """Record a local/operational failure without touching verified status.

    ``pdf_status`` keeps whatever the record already carries (including a
    previously verified ``available``); the failure lives in ``pdf_detail``.
    """
    return replace(article, pdf_detail=detail)


def _emit_progress(
    progress_callback: Callable[[int, int, str], None] | None,
    current: int,
    total: int,
    phase: str,
) -> None:
    if progress_callback is not None:
        progress_callback(current, total, phase)


def safe_pdf_filename(title: str) -> str:
    """Public helper: Windows-safe, non-empty PDF file stem for a paper title."""
    return _safe_filename(title)


def is_public_http_url(url: str, resolver: Resolver | None = None) -> bool:
    """Public helper: True when ``url`` is a public http(s) destination."""
    try:
        _require_public_url(url, resolver)
    except (_CandidateRejected, _CandidateUnverified):
        return False
    return True


__all__ = [
    "MIN_PDF_BYTES",
    "PDF_AVAILABLE",
    "PDF_UNAVAILABLE",
    "PDF_UNKNOWN",
    "check_pdf_availability",
    "download_pdfs",
    "finalize_download",
    "is_public_http_url",
    "safe_pdf_filename",
]
