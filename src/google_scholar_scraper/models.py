from collections.abc import Iterable
from dataclasses import dataclass
from enum import Enum
import re


# PDF states are exactly these three values. "unavailable" means no public PDF was
# found by the sources that were actually checked; it is not proof of universal
# absence. Provider/transport errors and unverified candidate links stay "unknown".
PDF_STATUS_AVAILABLE = "available"
PDF_STATUS_UNAVAILABLE = "unavailable"
PDF_STATUS_UNKNOWN = "unknown"

PDF_STATUSES = (PDF_STATUS_AVAILABLE, PDF_STATUS_UNAVAILABLE, PDF_STATUS_UNKNOWN)

_ISSN_RE = re.compile(r"^(\d{4})-?(\d{3}[\dX])$")


def normalize_issn(value: str) -> str:
    """Return a canonical `1234-567X` ISSN, or "" when the value is not a valid ISSN."""
    text = re.sub(r"\s+", "", str(value or "")).upper()
    match = _ISSN_RE.match(text)
    if not match:
        return ""

    candidate = f"{match.group(1)}-{match.group(2)}"
    digits = candidate.replace("-", "")
    total = sum(int(digit) * (8 - index) for index, digit in enumerate(digits[:7]))
    check = (11 - total % 11) % 11
    expected = "X" if check == 10 else str(check)
    return candidate if digits[7] == expected else ""


def normalize_issn_list(values: Iterable[str]) -> tuple[str, ...]:
    """Normalize, validate, de-duplicate and order ISSNs deterministically."""
    normalized = []
    for value in values or ():
        issn = normalize_issn(value)
        if issn and issn not in normalized:
            normalized.append(issn)
    return tuple(sorted(normalized))


_DOI_PREFIXES = (
    "https://doi.org/",
    "http://doi.org/",
    "https://dx.doi.org/",
    "http://dx.doi.org/",
    "doi:",
)


def normalize_doi(value: str) -> str:
    """Return a bare, case-folded DOI, or "" when nothing usable is left.

    Only a wrapper prefix is stripped. The suffix is never guessed or repaired: a DOI
    that was not carried by a credible DOI URL stays empty rather than becoming a
    plausible-looking fabricated identifier.
    """
    doi = str(value or "").strip()
    if not doi:
        return ""

    lowered = doi.casefold()
    for prefix in _DOI_PREFIXES:
        if lowered.startswith(prefix):
            doi = doi[len(prefix) :]
            break

    return doi.strip().casefold()


@dataclass(frozen=True)
class Article:
    title: str
    authors: str = ""
    link: str = ""
    relevance_score: float | None = None
    doi: str = ""
    journal: str = ""
    issns: tuple[str, ...] = ()
    publication_year: int | None = None
    sjr_quartile: str = ""
    sjr_year: int | None = None
    sjr_category: str = ""
    sjr_source_url: str = ""
    sinta_rank: str = ""
    sinta_year: int | None = None
    sinta_source_url: str = ""
    pdf_url: str = ""
    pdf_source_url: str = ""
    pdf_status: str = PDF_STATUS_UNKNOWN
    pdf_detail: str = ""
    pdf_local_path: str = ""

    def to_export_row(self) -> dict[str, object]:
        return {
            "Title": self.title,
            "Authors": self.authors,
            "Link": self.link,
            "Relevance Score": "" if self.relevance_score is None else self.relevance_score,
            "DOI": self.doi,
            "Journal": self.journal,
            "ISSNs": "; ".join(self.issns),
            "Publication Year": "" if self.publication_year is None else self.publication_year,
            "SJR Quartile": self.sjr_quartile,
            "SJR Year": "" if self.sjr_year is None else self.sjr_year,
            "SJR Category": self.sjr_category,
            "SJR Source": self.sjr_source_url,
            "SINTA Rank": self.sinta_rank,
            "SINTA Year": "" if self.sinta_year is None else self.sinta_year,
            "SINTA Source": self.sinta_source_url,
            "PDF Status": self.pdf_status,
            "PDF URL": self.pdf_url,
            "PDF Source": self.pdf_source_url,
            "PDF Detail": self.pdf_detail,
            "PDF Local Path": self.pdf_local_path,
        }


class ExtractionStatus(str, Enum):
    SUCCESS = "SUCCESS"
    PARTIAL_SUCCESS = "PARTIAL_SUCCESS"
    NO_RESULTS = "NO_RESULTS"
    RATE_LIMITED = "RATE_LIMITED"
    BLOCKED = "BLOCKED"
    NETWORK_ERROR = "NETWORK_ERROR"
    PARSING_ERROR = "PARSING_ERROR"
    CANCELLED = "CANCELLED"


@dataclass(frozen=True)
class ExtractionResult:
    status: ExtractionStatus
    articles: list[Article]
    requested_pages: int
    successful_pages: int = 0
    failure_page: int | None = None
    message: str = ""
    diagnostic: str = ""
    duplicates_removed: int = 0
    invalid_articles_removed: int = 0

    @property
    def has_articles(self) -> bool:
        return bool(self.articles)
