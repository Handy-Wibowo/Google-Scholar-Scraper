from collections.abc import Iterable
from dataclasses import dataclass, replace
from urllib.parse import urlsplit, urlunsplit
import re
import unicodedata

from google_scholar_scraper.models import (
    PDF_STATUS_AVAILABLE,
    PDF_STATUS_UNKNOWN,
    PDF_STATUSES,
    Article,
    normalize_doi,
    normalize_issn_list,
)


_WHITESPACE_RE = re.compile(r"\s+")


@dataclass(frozen=True)
class DeduplicationResult:
    articles: list[Article]
    duplicates_removed: int = 0
    invalid_removed: int = 0


def normalize_article(article: Article) -> Article:
    pdf_url = normalize_link(article.pdf_url)
    pdf_source_url = normalize_link(article.pdf_source_url)
    pdf_status = normalize_pdf_status(article.pdf_status)
    if pdf_status == PDF_STATUS_AVAILABLE and not (pdf_url or pdf_source_url):
        # `available` is a positive claim: something was fetched and verified. With no
        # candidate and no verified location the record names nothing it checked, so the
        # claim is downgraded rather than exported as a verdict about nothing.
        pdf_status = PDF_STATUS_UNKNOWN
    return replace(
        article,
        title=normalize_display_text(article.title),
        authors=normalize_display_text(article.authors),
        link=normalize_link(article.link),
        doi=normalize_doi(article.doi),
        journal=normalize_display_text(article.journal),
        issns=normalize_issns(article.issns),
        sjr_quartile=normalize_display_text(article.sjr_quartile).upper(),
        sjr_category=normalize_display_text(article.sjr_category),
        sjr_source_url=normalize_link(article.sjr_source_url),
        sinta_rank=normalize_display_text(article.sinta_rank).upper(),
        sinta_source_url=normalize_link(article.sinta_source_url),
        pdf_url=pdf_url,
        pdf_source_url=pdf_source_url,
        pdf_status=pdf_status,
        pdf_detail=normalize_display_text(article.pdf_detail),
        pdf_local_path=article.pdf_local_path.strip(),
    )


def normalize_pdf_status(value: str) -> str:
    status = normalize_display_text(value).lower()
    return status if status in PDF_STATUSES else PDF_STATUS_UNKNOWN


def normalize_issns(values: Iterable[str]) -> tuple[str, ...]:
    return normalize_issn_list(values)


def normalize_display_text(value: str) -> str:
    return _WHITESPACE_RE.sub(" ", value).strip()


def normalize_link(value: str) -> str:
    link = value.strip()
    if not link:
        return ""

    parsed = urlsplit(link)
    if not parsed.scheme or not parsed.netloc:
        return link

    return urlunsplit((parsed.scheme.lower(), parsed.netloc.lower(), parsed.path, parsed.query, ""))


def is_valid_article(article: Article) -> bool:
    return bool(normalize_display_text(article.title))


def title_key(article: Article) -> str:
    return _comparison_text_key(article.title)


def canonical_link(article: Article) -> str:
    link = normalize_link(article.link)
    parsed = urlsplit(link)
    if parsed.scheme in {"http", "https"} and parsed.netloc:
        return link
    return ""


def deduplicate_articles(articles: Iterable[Article]) -> DeduplicationResult:
    deduped: list[Article] = []
    link_to_index: dict[str, int] = {}
    title_to_index: dict[str, int] = {}
    duplicates_removed = 0
    invalid_removed = 0

    for article in articles:
        normalized = normalize_article(article)
        if not is_valid_article(normalized):
            invalid_removed += 1
            continue

        link = canonical_link(normalized)
        title = title_key(normalized)
        existing_index = _find_existing_index(link, title, link_to_index, title_to_index)

        if existing_index is None:
            index = len(deduped)
            deduped.append(normalized)
            if link:
                link_to_index[link] = index
            if title:
                title_to_index[title] = index
            continue

        duplicates_removed += 1
        merged = _merge_articles(deduped[existing_index], normalized)
        deduped[existing_index] = merged

        merged_link = canonical_link(merged)
        merged_title = title_key(merged)
        if merged_link:
            link_to_index[merged_link] = existing_index
        if merged_title:
            title_to_index[merged_title] = existing_index

    return DeduplicationResult(
        articles=deduped,
        duplicates_removed=duplicates_removed,
        invalid_removed=invalid_removed,
    )


def _comparison_text_key(value: str) -> str:
    normalized = unicodedata.normalize("NFKC", value)
    collapsed = normalize_display_text(normalized)
    return collapsed.casefold()


def _find_existing_index(
    link: str,
    title: str,
    link_to_index: dict[str, int],
    title_to_index: dict[str, int],
) -> int | None:
    if link and link in link_to_index:
        return link_to_index[link]
    if title and title in title_to_index:
        return title_to_index[title]
    return None


def _merge_articles(existing: Article, incoming: Article) -> Article:
    """Merge a duplicate record without ever mixing two different claims.

    Scalar identity fields keep the first non-empty value. Fields whose empty state is
    meaningful (`publication_year` None, PDF status unknown, empty rank) are absences
    rather than values, so they never overwrite a present value.

    The ranking and PDF fields are merged as *bundles*, not field by field. A quartile,
    its year, its category and its source URL are one claim about one journal at one
    time; a PDF URL, its source and its verified status are one claim about one
    candidate file. Splicing fields across two claims would attribute a year or an
    "available" verdict to evidence that never supported it.
    """
    sjr_quartile, sjr_year, sjr_category, sjr_source_url = _merge_rank_bundle(
        (existing.sjr_quartile, existing.sjr_year, existing.sjr_category, existing.sjr_source_url),
        (incoming.sjr_quartile, incoming.sjr_year, incoming.sjr_category, incoming.sjr_source_url),
    )
    sinta_rank, sinta_year, sinta_source_url = _merge_rank_bundle(
        (existing.sinta_rank, existing.sinta_year, existing.sinta_source_url),
        (incoming.sinta_rank, incoming.sinta_year, incoming.sinta_source_url),
    )
    pdf_url, pdf_source_url, pdf_status, pdf_detail = _merge_pdf_bundle(existing, incoming)
    return replace(
        existing,
        title=existing.title or incoming.title,
        authors=existing.authors or incoming.authors,
        link=existing.link or incoming.link,
        relevance_score=existing.relevance_score,
        doi=existing.doi or incoming.doi,
        journal=existing.journal or incoming.journal,
        issns=normalize_issn_list([*existing.issns, *incoming.issns]),
        publication_year=(
            existing.publication_year if existing.publication_year is not None else incoming.publication_year
        ),
        sjr_quartile=sjr_quartile,
        sjr_year=sjr_year,
        sjr_category=sjr_category,
        sjr_source_url=sjr_source_url,
        sinta_rank=sinta_rank,
        sinta_year=sinta_year,
        sinta_source_url=sinta_source_url,
        pdf_url=pdf_url,
        pdf_source_url=pdf_source_url,
        pdf_status=pdf_status,
        pdf_detail=pdf_detail,
        pdf_local_path=existing.pdf_local_path or incoming.pdf_local_path,
    )


def _merge_rank_bundle(
    existing: tuple[str, int | None, ...],
    incoming: tuple[str, int | None, ...],
) -> tuple:
    """Pick one coherent ranking claim: the latest genuinely dated one, else the first.

    A dated record always beats an undated one, because the undated record cannot be
    shown to be newer. Two undated records are indistinguishable, so the first wins and
    nothing is overwritten. An incoming record only replaces the existing one when it
    is dated and the existing claim is either absent or undated.
    """
    existing_rank = existing[0]
    incoming_rank = incoming[0]

    if not incoming_rank:
        return existing
    if not existing_rank:
        return incoming

    existing_year = existing[1]
    incoming_year = incoming[1]

    if existing_year is None:
        return incoming
    if incoming_year is None:
        return existing
    return incoming if incoming_year > existing_year else existing


def _merge_pdf_bundle(existing: Article, incoming: Article) -> tuple[str, str, str, str]:
    """Pick one coherent PDF claim so a status never outlives the candidate it describes.

    The URL, its source, the verified status and the detail text are taken together from
    a single record; splicing them across two claims would attribute an "available"
    verdict to a candidate nobody verified.

    The anchor of a claim is either `pdf_url` (a candidate link) or `pdf_source_url` (the
    location actually verified). Both are checked because the PDF module deliberately
    leaves `pdf_url` alone and records a verified location in `pdf_source_url`, so a
    legitimate `available` verdict can carry an empty `pdf_url`.
    """
    existing_checked = _pdf_claim_strength(existing)
    incoming_checked = _pdf_claim_strength(incoming)
    if incoming_checked > existing_checked:
        take = incoming
    elif existing_checked > incoming_checked:
        take = existing
    else:
        take = existing if _pdf_anchor(existing) else incoming
    return take.pdf_url, take.pdf_source_url, take.pdf_status, take.pdf_detail


def _pdf_anchor(article: Article) -> str:
    return article.pdf_url or article.pdf_source_url


def _pdf_claim_strength(article: Article) -> int:
    """How much a record actually establishes about public availability.

    A status is only meaningful when the record also names what was consulted, so an
    anchored claim outranks an unanchored one, and among anchored claims a verified
    result outranks a clean rejection, which outranks an unchecked record.
    """
    if not _pdf_anchor(article):
        return -1
    return {"available": 2, "unavailable": 1}.get(article.pdf_status, 0)
