"""Verified bibliographic enrichment via Crossref, plus local ranking lookup.

Everything here is conservative by construction. A journal identity is only ever taken
from Crossref metadata that the publisher deposited for that exact work — resolved by DOI,
or by an exact normalized title match that is unambiguous. A ranking label only ever comes
from a record the user imported. When a provider is unreachable, slow, ambiguous or simply
has no record, the original article is returned unchanged and the unknown stays unknown.
Nothing is inferred from a Scholar citation string, and no fuzzy score ever decides an
identity.
"""

from collections.abc import Callable, Sequence
from dataclasses import dataclass, replace
from threading import Event
from urllib.parse import quote, urlencode
import json
import re
import unicodedata

import requests

from google_scholar_scraper.journal_catalog import JournalCatalog, catalog_matches
from google_scholar_scraper.models import Article, normalize_issn_list


CROSSREF_BASE_URL = "https://api.crossref.org/works/"
DEFAULT_TIMEOUT = 10.0
DEFAULT_USER_AGENT = "google-scholar-scraper/2.1 (metadata enrichment; mailto:none)"

# A single Crossref bibliographic response is small; anything larger than this is not a
# record we asked for, so it is refused rather than parsed.
DEFAULT_MAX_RESPONSE_BYTES = 2_000_000
# A short page is not enough: an exact title can sit behind several same-titled book
# chapters. Live evidence for "Deep learning" (2015) — rows=5 returned only book chapters,
# rows=20 surfaced the Nature article among the exact-title alternatives, where the author
# and year checks disambiguated it.
DEFAULT_TITLE_ROWS = 20

ProgressCallback = Callable[[int, int, str], None]

_YEAR_RE = re.compile(r"^(1[5-9]\d\d|2[01]\d\d)$")
_NON_ALNUM_RE = re.compile(r"[^0-9a-z]+")

# Only these Crossref work types describe a journal article. A conference paper, a
# preprint, a dataset or a book chapter can carry an ISSN-bearing container, but it is not
# a journal article and must never receive a journal ranking label.
_JOURNAL_WORK_TYPES = frozenset({"journal-article"})

_TITLE_SELECT = (
    "DOI,title,container-title,short-container-title,ISSN,type,author,"
    "issued,published-print,published-online,link,license"
)


@dataclass(frozen=True)
class CrossrefRecord:
    """Normalized bibliographic facts for one work, as Crossref reported them."""

    doi: str = ""
    journal: str = ""
    issns: tuple[str, ...] = ()
    year: int | None = None
    work_type: str = ""
    pdf_candidate_url: str = ""
    pdf_candidate_source: str = ""


class CrossrefClient:
    """Minimal Crossref REST client: `/works/{doi}` plus a bounded title query.

    The client is deliberately narrow. It refuses redirects, ignores proxy/credential
    configuration from the environment, caps the response size it will parse, and turns
    every failure into None so an unreachable provider can never look like a broken
    pipeline.
    """

    def __init__(
        self,
        session: requests.Session | None = None,
        *,
        base_url: str = CROSSREF_BASE_URL,
        timeout: float = DEFAULT_TIMEOUT,
        user_agent: str = DEFAULT_USER_AGENT,
        max_response_bytes: int = DEFAULT_MAX_RESPONSE_BYTES,
        allow_redirects: bool = False,
    ) -> None:
        self._session = session if session is not None else requests.Session()
        self._owns_session = session is None
        self._base_url = base_url
        self._timeout = timeout
        self._user_agent = user_agent
        self._max_response_bytes = max_response_bytes
        self._allow_redirects = allow_redirects

        if self._owns_session:
            # The official API is reached directly: a proxy or netrc credential picked up
            # from the environment would silently redirect these requests elsewhere.
            self._session.trust_env = False

    def close(self) -> None:
        """Close the session only when this client created it."""
        if self._owns_session:
            self._session.close()

    def __enter__(self) -> "CrossrefClient":
        return self

    def __exit__(self, *_exc_info: object) -> None:
        self.close()

    def lookup(self, doi: str) -> CrossrefRecord | None:
        """Return the record for a DOI, or None when it cannot be verified.

        The payload must echo the DOI that was asked for. A redirect, a proxy or a
        provider-side mix-up that answers with a different work would otherwise have its
        identity silently attached to this article, so a mismatch is treated as no answer.
        """
        normalized = (doi or "").strip()
        if not normalized:
            return None

        message = self._get_message(f"{self._base_url}{quote(normalized, safe='')}")
        if message is None:
            return None

        echoed = str(message.get("DOI") or "").strip().casefold()
        if echoed != normalized.casefold():
            return None

        return record_from_crossref_message(message, fallback_doi=normalized)

    def lookup_title(
        self,
        title: str,
        *,
        author: str = "",
        year: int | None = None,
        rows: int = DEFAULT_TITLE_ROWS,
    ) -> CrossrefRecord | None:
        """Resolve a missing DOI by an exact normalized title match, or return None.

        Crossref is asked for a small page of *title* candidates — `query.bibliographic`
        matches on authors, venue and abstract text too, which drowns an exact title under
        unrelated works. When the caller knows a year it is also pushed into the API filter
        (padded by the same one-year drift `_year_consistent` tolerates, so online-first and
        print dates both survive); with no known year no filter is sent.

        A candidate is only accepted when its normalized title is *identical* to the
        requested one — no similarity score, no token overlap. If several distinct DOIs
        match that exact title, the title is ambiguous and nothing is returned. When the
        caller also knows an author or a year, a candidate contradicting either is rejected
        too.
        """
        wanted_key = _title_key(title)
        if not wanted_key:
            return None

        params: dict[str, object] = {
            "query.title": title.strip(),
            "rows": max(1, min(int(rows), 20)),
            "select": _TITLE_SELECT,
        }
        if year is not None:
            params["filter"] = (
                f"from-pub-date:{year - 1}-01-01,until-pub-date:{year + 1}-12-31"
            )
        payload = self._get_json(f"{self._base_url}?{urlencode(params)}")
        if payload is None:
            return None

        message = payload.get("message") if isinstance(payload, dict) else None
        items = message.get("items") if isinstance(message, dict) else None
        if not isinstance(items, Sequence) or isinstance(items, str):
            return None

        matches: list[dict] = []
        for item in items:
            if not isinstance(item, dict):
                continue
            # A title alone is a weak identity: a book chapter, a dataset or a preprint can
            # carry the very same words. Only a journal article may be resolved this way,
            # otherwise a wrong DOI would be stamped onto the article.
            if str(item.get("type") or "").strip().casefold() not in _JOURNAL_WORK_TYPES:
                continue
            if _title_key(_first_text(item.get("title"))) != wanted_key:
                continue
            if not _author_consistent(author, item):
                continue
            if not _year_consistent(year, item):
                continue
            matches.append(item)

        dois = {str(item.get("DOI") or "").strip().casefold() for item in matches}
        dois.discard("")
        if len(dois) != 1:
            # Zero matches means unknown; more than one distinct DOI behind the same exact
            # title means the title alone cannot identify the work.
            return None

        resolved_doi = dois.pop()
        winner = next(
            item
            for item in matches
            if str(item.get("DOI") or "").strip().casefold() == resolved_doi
        )
        return record_from_crossref_message(winner, fallback_doi=resolved_doi)

    def _get_message(self, url: str) -> dict | None:
        payload = self._get_json(url)
        if payload is None:
            return None
        message = payload.get("message") if isinstance(payload, dict) else None
        return message if isinstance(message, dict) else None

    def _get_json(self, url: str) -> dict | None:
        try:
            response = self._session.get(
                url,
                headers={"User-Agent": self._user_agent, "Accept": "application/json"},
                timeout=self._timeout,
                allow_redirects=self._allow_redirects,
                stream=True,
            )
        except requests.RequestException:
            return None

        try:
            if response.status_code != 200:
                return None
            return _bounded_json(response, self._max_response_bytes)
        finally:
            _close_quietly(response)


def record_from_crossref_message(message: dict, *, fallback_doi: str = "") -> CrossrefRecord:
    """Map a Crossref `message` object onto the fields we are willing to trust.

    Journal identity is only recorded for a journal article. A conference paper or a
    preprint may name a container with an ISSN, but reporting that as the article's
    journal would hand it a journal ranking label it has not earned.
    """
    work_type = str(message.get("type") or "").strip().casefold()

    journal = ""
    issns: tuple[str, ...] = ()
    if work_type in _JOURNAL_WORK_TYPES:
        journal = _first_text(message.get("container-title")) or _first_text(
            message.get("short-container-title")
        )
        issns = normalize_issn_list(_as_text_list(message.get("ISSN")))

    pdf_url, pdf_source = _public_pdf_candidate(message)

    return CrossrefRecord(
        doi=str(message.get("DOI") or fallback_doi).strip().casefold(),
        journal=journal,
        issns=issns,
        year=_crossref_year(message),
        work_type=work_type,
        pdf_candidate_url=pdf_url,
        pdf_candidate_source=pdf_source,
    )


def enrich_articles(
    articles: list[Article],
    catalog: JournalCatalog | None = None,
    cancel_event: Event | None = None,
    progress_callback: ProgressCallback | None = None,
    *,
    lookup: CrossrefClient | None = None,
    max_lookups: int | None = None,
    max_title_lookups: int | None = None,
    allow_title_lookup: bool = True,
) -> list[Article]:
    """Attach verified journal identity and imported ranking labels to articles.

    Input order is preserved exactly, and every article is returned — enriched or not.
    Cancellation returns the articles handled so far plus the untouched remainder, so a
    cancelled run never loses collected data.

    A DOI-bearing article is resolved by DOI. An article without a DOI can still be
    resolved by an exact normalized title match, which is how most Scholar results (which
    link the publisher or a repository, not doi.org) get any journal identity at all.

    `lookup` is injectable so tests never touch the network; `max_lookups` and
    `max_title_lookups` bound how many distinct queries one run will issue.
    """
    client = lookup if lookup is not None else CrossrefClient()
    owns_client = lookup is None
    total = len(articles)
    results: list[Article] = []
    doi_cache: dict[str, CrossrefRecord | None] = {}
    title_cache: dict[str, CrossrefRecord | None] = {}
    lookups_done = 0
    title_lookups_done = 0

    try:
        for index, article in enumerate(articles):
            if cancel_event is not None and cancel_event.is_set():
                results.extend(articles[index:])
                if progress_callback is not None:
                    progress_callback(index, total, "Cancelled")
                return results

            enriched = article
            needs_identity = not article.journal or not article.issns
            record: CrossrefRecord | None = None

            doi = article.doi.strip()
            if doi and (needs_identity or article.publication_year is None):
                # A cached answer is free, so it stays available after the budget is spent;
                # only a genuinely new query consumes budget.
                if doi in doi_cache:
                    record = doi_cache[doi]
                elif max_lookups is None or lookups_done < max_lookups:
                    doi_cache[doi] = client.lookup(doi)
                    lookups_done += 1
                    record = doi_cache[doi]

            # Title search is only for an article that never asserted a DOI. When a known
            # DOI lookup fails, falling back to a title guess could silently swap in a
            # different work's identity, so the record stays unknown instead.
            if record is None and not doi and needs_identity and allow_title_lookup and article.title.strip():
                key = _title_query_key(article.title, article.authors, article.publication_year)
                if key in title_cache:
                    record = title_cache[key]
                elif max_title_lookups is None or title_lookups_done < max_title_lookups:
                    title_cache[key] = client.lookup_title(
                        article.title,
                        author=article.authors,
                        year=article.publication_year,
                    )
                    title_lookups_done += 1
                    record = title_cache[key]

            if record is not None:
                enriched = _apply_record(article, record)

            if catalog is not None:
                sjr, sinta = catalog_matches(enriched, catalog)
                enriched = _apply_ranks(enriched, sjr, sinta)

            results.append(enriched)

            if progress_callback is not None:
                progress_callback(index + 1, total, enriched.title or article.title)
    finally:
        if owns_client:
            client.close()

    return results


def _apply_record(article: Article, record: CrossrefRecord) -> Article:
    """Fill only fields Crossref actually supplied; never overwrite a known value."""
    updates: dict[str, object] = {}

    if not article.journal and record.journal:
        updates["journal"] = record.journal
    if not article.issns and record.issns:
        updates["issns"] = record.issns
    if article.publication_year is None and record.year is not None:
        updates["publication_year"] = record.year
    if not article.doi and record.doi:
        updates["doi"] = record.doi

    # A Crossref licence/PDF link is an unverified candidate, so it is recorded as
    # provenance only. `pdf_status` stays untouched — verification is another module's
    # job, and an unchecked link is never "available".
    if not article.pdf_url and record.pdf_candidate_url:
        updates["pdf_url"] = record.pdf_candidate_url
        updates["pdf_source_url"] = record.pdf_candidate_source

    return replace(article, **updates) if updates else article


def _apply_ranks(article: Article, sjr, sinta) -> Article:
    from google_scholar_scraper.journal_catalog import apply_rank

    updated = apply_rank(article, sjr)
    return apply_rank(updated, sinta)


def _bounded_json(response, max_bytes: int) -> dict | None:
    """Parse a JSON body, refusing anything larger than `max_bytes`.

    A declared `Content-Length` over the bound is rejected before any body is read; when
    the transport exposes a stream, the read itself is capped so an oversized or
    open-ended body is abandoned instead of buffered.
    """
    headers = getattr(response, "headers", None) or {}
    declared = headers.get("Content-Length")
    if declared is not None:
        try:
            if int(declared) > max_bytes:
                return None
        except (TypeError, ValueError):
            pass

    raw = getattr(response, "raw", None)
    if raw is not None and hasattr(raw, "read"):
        chunks: list[bytes] = []
        size = 0
        while True:
            try:
                chunk = raw.read(max_bytes + 1 - size, decode_content=True)
            except Exception:  # noqa: BLE001 - any transport failure means "no usable body"
                return None
            if not isinstance(chunk, (bytes, bytearray)) or not chunk:
                break
            chunks.append(bytes(chunk))
            size += len(chunk)
            if size > max_bytes:
                return None
        try:
            payload = json.loads(b"".join(chunks).decode("utf-8"))
        except (UnicodeDecodeError, ValueError):
            return None
        return payload if isinstance(payload, dict) else None

    try:
        payload = response.json()
    except ValueError:
        return None
    return payload if isinstance(payload, dict) else None


def _close_quietly(response) -> None:
    close = getattr(response, "close", None)
    if callable(close):
        try:
            close()
        except Exception:  # noqa: BLE001 - closing must never mask the result
            pass


def _title_key(value: str) -> str:
    """Conservative comparison key for article titles: case, punctuation, diacritics."""
    normalized = unicodedata.normalize("NFKD", value or "")
    decomposed = "".join(char for char in normalized if not unicodedata.combining(char))
    return _NON_ALNUM_RE.sub("", decomposed.casefold())


def _title_query_key(title: str, author: str, year: int | None) -> str:
    """Cache key for a title query, covering every constraint the query is built from.

    `lookup_title` is called with the author hint and the year, and both change which
    candidate is accepted, so a key of the normalized title alone would hand the first
    same-titled record's answer to every later one — including a different work by a
    different author in a different year. Every constraint that can change the answer is
    therefore part of the key.

    The author hint is reduced to the same token set `_author_consistent` compares with, so
    records that differ only in an irrelevant part of the display string still share one
    query. Returns "" for a title with no comparable characters, which is not worth asking
    about at all.
    """
    title_key = _title_key(title)
    if not title_key:
        return ""
    author_key = " ".join(sorted(set(_title_tokens(author))))
    return f"{title_key}|{author_key}|{'' if year is None else year}"


def _author_consistent(author: str, item: dict) -> bool:
    """Reject a candidate whose authors contradict the ones the caller already knows.

    The check only narrows acceptance: when the caller supplies nothing usable, or the
    candidate lists no authors, nothing is concluded. A supplied surname that appears
    nowhere in the candidate's author list is a contradiction, not a near miss.
    """
    wanted = {token for token in _title_tokens(author)}
    if not wanted:
        return True

    families = set()
    for entry in item.get("author") or ():
        if isinstance(entry, dict):
            families.update(_title_tokens(str(entry.get("family") or "")))
            families.update(_title_tokens(str(entry.get("name") or "")))

    if not families:
        return True
    return bool(wanted & families)


def _year_consistent(year: int | None, item: dict) -> bool:
    """Reject a candidate whose year contradicts a known one, allowing one year of drift.

    An online-first deposit and its print issue legitimately differ by a year, so a
    one-year gap is accepted. A larger gap is a different work.
    """
    if year is None:
        return True

    candidate_year = _crossref_year(item)
    if candidate_year is None:
        return True
    return abs(candidate_year - year) <= 1


def _title_tokens(value: str) -> list[str]:
    """Split text into comparable word tokens, dropping short and purely numeric ones.

    Tokens must come from the *raw* text. Normalizing first (removing all punctuation and
    spaces) welds the whole string into a single token, so an author hint could never match
    anything and every candidate would be rejected.

    ponytail: Scholar's author field is a display string ("A Author - Venue, 2015"), so a
    venue word can land in the surname set. That only weakens this contradiction filter; it
    never creates a match on its own, because an exact title match and a unique DOI are
    still required.
    """
    normalized = unicodedata.normalize("NFKD", value or "")
    decomposed = "".join(char for char in normalized if not unicodedata.combining(char))
    return [
        token
        for token in _NON_ALNUM_RE.split(decomposed.casefold())
        if len(token) >= 3 and not token.isdigit()
    ]


def _first_text(value: object) -> str:
    values = _as_text_list(value)
    return values[0] if values else ""


def _as_text_list(value: object) -> list[str]:
    if isinstance(value, str):
        return [value.strip()] if value.strip() else []
    if isinstance(value, Sequence):
        return [str(item).strip() for item in value if str(item).strip()]
    return []


def _crossref_year(message: dict) -> int | None:
    """Prefer the publication year; fall back to issued, then created.

    Crossref's `issued` date is often the earliest known date and `published-print`
    can be later, so the most specific published date wins over a generic one. Only a
    plausible four-digit year is accepted; a date-parts fragment is never guessed into
    a year.
    """
    for key in ("published-print", "published-online", "published", "issued", "created"):
        year = _year_from_date(message.get(key))
        if year is not None:
            return year
    return None


def _year_from_date(value: object) -> int | None:
    if not isinstance(value, dict):
        return None
    parts = value.get("date-parts")
    if not isinstance(parts, Sequence) or isinstance(parts, str) or not parts:
        return None
    first = parts[0]
    if not isinstance(first, Sequence) or isinstance(first, str) or not first:
        return None
    return _parse_year(first[0])


def _parse_year(value: object) -> int | None:
    text = str(value).strip()
    return int(text) if _YEAR_RE.match(text) else None


def _public_pdf_candidate(message: dict) -> tuple[str, str]:
    """Return an (url, provenance) public-PDF candidate, or ("", "").

    A candidate requires two independent facts from the publisher's own deposit: an
    explicit public licence URL, and a link the publisher labels as a PDF. A link
    without a licence is a text-mining endpoint, not a public document, so it is
    ignored. Nothing is downloaded or fetched here.
    """
    licence_url = ""
    for licence in message.get("license") or ():
        if isinstance(licence, dict) and str(licence.get("URL") or "").strip():
            licence_url = str(licence["URL"]).strip()
            break

    if not licence_url:
        return "", ""

    for link in message.get("link") or ():
        if not isinstance(link, dict):
            continue
        content_type = str(link.get("content-type") or "").casefold()
        url = str(link.get("URL") or "").strip()
        if url.startswith("https://") and "pdf" in content_type:
            return url, "Crossref licence deposit"

    return "", ""
