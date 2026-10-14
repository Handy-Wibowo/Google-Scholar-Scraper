import unittest

from google_scholar_scraper.dedupe import (
    canonical_link,
    deduplicate_articles,
    is_valid_article,
    normalize_article,
    normalize_display_text,
    normalize_link,
    title_key,
)
from google_scholar_scraper.models import Article


class NormalizationTests(unittest.TestCase):
    def test_display_text_collapses_whitespace_without_changing_punctuation_or_unicode(self) -> None:
        text = "  یادگیری\t عمیق\nبرای   پزشکی: α + β!  "

        self.assertEqual(normalize_display_text(text), "یادگیری عمیق برای پزشکی: α + β!")

    def test_article_normalization_preserves_readable_case(self) -> None:
        article = normalize_article(Article(title="  Deep   Learning  ", authors="\nA\tAuthor  ", link=" https://Example.edu/paper#part "))

        self.assertEqual(article.title, "Deep Learning")
        self.assertEqual(article.authors, "A Author")
        self.assertEqual(article.link, "https://example.edu/paper")

    def test_title_key_is_case_insensitive_and_whitespace_insensitive(self) -> None:
        first = title_key(Article(title="  Deep   Learning for Medical Imaging  "))
        second = title_key(Article(title="DEEP LEARNING FOR MEDICAL IMAGING"))

        self.assertEqual(first, second)

    def test_link_normalization_preserves_empty_and_relative_values(self) -> None:
        self.assertEqual(normalize_link("   "), "")
        self.assertEqual(normalize_link(" /scholar?cluster=1#frag "), "/scholar?cluster=1#frag")

    def test_canonical_link_requires_http_url(self) -> None:
        self.assertEqual(canonical_link(Article(title="Title", link="https://Example.edu/a#b")), "https://example.edu/a")
        self.assertEqual(canonical_link(Article(title="Title", link="/scholar?cluster=1")), "")


class ValidationTests(unittest.TestCase):
    def test_normal_article_is_valid(self) -> None:
        self.assertTrue(is_valid_article(Article(title="Valid title", authors="Author", link="")))

    def test_missing_link_remains_valid(self) -> None:
        self.assertTrue(is_valid_article(Article(title="Citation only result", authors="", link="")))

    def test_missing_authors_remains_valid(self) -> None:
        self.assertTrue(is_valid_article(Article(title="Title", authors="", link="https://example.edu")))

    def test_empty_title_is_invalid(self) -> None:
        self.assertFalse(is_valid_article(Article(title=" \n\t ", authors="Author", link="https://example.edu")))


class DeduplicationTests(unittest.TestCase):
    def test_exact_duplicate_is_removed(self) -> None:
        result = deduplicate_articles(
            [
                Article(title="Title", authors="Author", link="https://example.edu/a"),
                Article(title="Title", authors="Author", link="https://example.edu/a"),
            ]
        )

        self.assertEqual(len(result.articles), 1)
        self.assertEqual(result.duplicates_removed, 1)

    def test_duplicate_title_with_whitespace_and_capitalization_is_removed(self) -> None:
        result = deduplicate_articles(
            [
                Article(title="  Deep   Learning for Medical Imaging  ", authors="A", link=""),
                Article(title="DEEP LEARNING FOR MEDICAL IMAGING", authors="A", link=""),
            ]
        )

        self.assertEqual(len(result.articles), 1)
        self.assertEqual(result.duplicates_removed, 1)
        self.assertEqual(result.articles[0].title, "Deep Learning for Medical Imaging")

    def test_same_canonical_link_is_duplicate(self) -> None:
        result = deduplicate_articles(
            [
                Article(title="First title", authors="A", link="https://example.edu/paper#section"),
                Article(title="Second title", authors="B", link="https://EXAMPLE.edu/paper"),
            ]
        )

        self.assertEqual(len(result.articles), 1)
        self.assertEqual(result.articles[0].title, "First title")

    def test_same_title_merges_missing_link_from_later_record(self) -> None:
        result = deduplicate_articles(
            [
                Article(title="Shared title", authors="Author", link=""),
                Article(title=" shared   title ", authors="", link="https://example.edu/shared"),
            ]
        )

        self.assertEqual(len(result.articles), 1)
        self.assertEqual(result.articles[0].link, "https://example.edu/shared")
        self.assertEqual(result.articles[0].authors, "Author")

    def test_different_titles_remain_distinct(self) -> None:
        result = deduplicate_articles(
            [
                Article(title="Deep Learning for Medical Imaging", authors="", link=""),
                Article(title="Deep Learning for Medical Image Segmentation", authors="", link=""),
            ]
        )

        self.assertEqual(len(result.articles), 2)
        self.assertEqual(result.duplicates_removed, 0)

    def test_stable_first_seen_ordering(self) -> None:
        result = deduplicate_articles(
            [
                Article(title="First", authors="", link=""),
                Article(title="Second", authors="", link=""),
                Article(title="first", authors="", link="https://example.edu/first"),
                Article(title="Third", authors="", link=""),
            ]
        )

        self.assertEqual([article.title for article in result.articles], ["First", "Second", "Third"])
        self.assertEqual(result.articles[0].link, "https://example.edu/first")

    def test_invalid_empty_title_is_removed(self) -> None:
        result = deduplicate_articles([Article(title="  ", authors="Author", link="https://example.edu")])

        self.assertEqual(result.articles, [])
        self.assertEqual(result.invalid_removed, 1)


class MetadataMergeTests(unittest.TestCase):
    def test_merge_preserves_every_new_field_from_a_single_record(self) -> None:
        first = Article(
            title="Deep Learning",
            authors="A Author",
            link="",
            doi="10.1038/nature14539",
            journal="Nature",
            issns=("0028-0836",),
            publication_year=2015,
            sjr_quartile="Q1",
            sjr_year=2023,
            sjr_category="Multidisciplinary",
            sjr_source_url="https://scimagojr.example/nature",
            sinta_rank="S2",
            sinta_year=2024,
            sinta_source_url="https://sinta.example/nature",
            pdf_url="https://repo.example/a.pdf",
            pdf_source_url="https://api.crossref.org/works/10.1038/nature14539",
            pdf_status="available",
            pdf_detail="verified",
            pdf_local_path="C:/pdfs/a.pdf",
        )
        second = Article(title=" deep   learning ", authors="", link="https://example.edu/a")

        result = deduplicate_articles([first, second])

        self.assertEqual(len(result.articles), 1)
        merged = result.articles[0]
        self.assertEqual(merged.doi, "10.1038/nature14539")
        self.assertEqual(merged.journal, "Nature")
        self.assertEqual(merged.issns, ("0028-0836",))
        self.assertEqual(merged.publication_year, 2015)
        self.assertEqual(
            (merged.sjr_quartile, merged.sjr_year, merged.sjr_category, merged.sjr_source_url),
            ("Q1", 2023, "Multidisciplinary", "https://scimagojr.example/nature"),
        )
        self.assertEqual(
            (merged.sinta_rank, merged.sinta_year, merged.sinta_source_url),
            ("S2", 2024, "https://sinta.example/nature"),
        )
        self.assertEqual(
            (merged.pdf_url, merged.pdf_source_url, merged.pdf_status, merged.pdf_detail),
            (
                "https://repo.example/a.pdf",
                "https://api.crossref.org/works/10.1038/nature14539",
                "available",
                "verified",
            ),
        )
        self.assertEqual(merged.pdf_local_path, "C:/pdfs/a.pdf")
        self.assertEqual(merged.link, "https://example.edu/a")

    def test_ranking_is_merged_as_one_claim_not_field_by_field(self) -> None:
        existing = Article(
            title="Shared",
            authors="",
            link="",
            sjr_quartile="Q1",
            sjr_year=None,
            sjr_category="Old category",
        )
        incoming = Article(
            title="shared",
            authors="",
            link="",
            sjr_quartile="Q4",
            sjr_year=2024,
            sjr_category="New category",
        )

        merged = deduplicate_articles([existing, incoming]).articles[0]

        # A dated record beats an undated one whole: never Q1 with 2024's year.
        self.assertEqual(
            (merged.sjr_quartile, merged.sjr_year, merged.sjr_category),
            ("Q4", 2024, "New category"),
        )

    def test_latest_dated_ranking_wins_over_an_older_one(self) -> None:
        older = Article(title="Shared", authors="", link="", sjr_quartile="Q2", sjr_year=2019)
        newer = Article(title="shared", authors="", link="", sjr_quartile="Q1", sjr_year=2023)

        merged = deduplicate_articles([older, newer]).articles[0]

        self.assertEqual((merged.sjr_quartile, merged.sjr_year), ("Q1", 2023))

    def test_available_pdf_is_never_attached_to_a_different_candidate(self) -> None:
        verified = Article(
            title="Shared",
            authors="",
            link="",
            pdf_url="https://repo.example/verified.pdf",
            pdf_source_url="https://repo.example/verified.pdf",
            pdf_status="available",
            pdf_detail="verified",
        )
        candidate = Article(
            title="shared",
            authors="",
            link="",
            pdf_url="https://other.example/candidate.pdf",
            pdf_source_url="https://other.example/candidate.pdf",
            pdf_status="unknown",
        )

        merged = deduplicate_articles([candidate, verified]).articles[0]

        self.assertEqual(merged.pdf_url, "https://repo.example/verified.pdf")
        self.assertEqual(merged.pdf_status, "available")

    def test_available_status_with_no_anchor_degrades_to_unknown(self) -> None:
        # No candidate URL and no verified source: the verdict names nothing consulted.
        broken = Article(
            title="Shared",
            authors="",
            link="",
            pdf_url="",
            pdf_source_url="",
            pdf_status="available",
        )

        merged = deduplicate_articles([broken]).articles[0]

        self.assertEqual(merged.pdf_status, "unknown")

    def test_verified_status_with_only_a_source_url_is_kept(self) -> None:
        # The PDF module leaves `pdf_url` untouched and records the verified location in
        # `pdf_source_url`, so an empty `pdf_url` must not discard a real verdict.
        verified = Article(
            title="Shared",
            authors="",
            link="",
            pdf_url="",
            pdf_source_url="https://repo.example/a.pdf",
            pdf_status="available",
        )

        merged = deduplicate_articles([verified]).articles[0]

        self.assertEqual(merged.pdf_status, "available")
        self.assertEqual(merged.pdf_source_url, "https://repo.example/a.pdf")

    def test_issns_from_duplicates_are_unioned_and_normalized(self) -> None:
        first = Article(title="Shared", authors="", link="", issns=("0028-0836",))
        second = Article(title="shared", authors="", link="", issns=("1476-4687", "0028-0836"))

        merged = deduplicate_articles([first, second]).articles[0]

        self.assertEqual(merged.issns, ("0028-0836", "1476-4687"))


if __name__ == "__main__":
    unittest.main()
