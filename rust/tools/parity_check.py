#!/usr/bin/env python3
"""Parity harness: diff the Rust port against the Python oracle, offline.

Usage:
    python tools/parity_check.py --python src --rust rust [--parity BIN]

Gates:
    gate1  normalize_title_key over every fixture title + packaging-smoke
           titles + a fixed adversarial edge set → zero mismatches.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path


class _TitleCollector(HTMLParser):
    """Collect gs_rt title anchor text from fixture HTML the way the parser does."""

    def __init__(self) -> None:
        super().__init__()
        self.in_gs_ri = 0
        self.in_gs_rt = 0
        self.in_anchor = 0
        self.depth_stack: list[str] = []
        self.current_text: list[str] = []
        self.titles: list[str] = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        cls = a.get("class", "")
        if tag == "div" and "gs_ri" in cls.split():
            self.in_gs_ri += 1
        elif self.in_gs_ri and tag == "h3" and "gs_rt" in cls.split():
            self.in_gs_rt += 1
        elif self.in_gs_rt and tag == "a":
            self.in_anchor += 1

    def handle_endtag(self, tag):
        if tag == "a" and self.in_anchor:
            self.in_anchor -= 1
        elif tag == "h3" and self.in_gs_rt:
            self.in_gs_rt -= 1
            if self.in_anchor == 0:
                text = "".join(self.current_text).strip()
                if text:
                    self.titles.append(text)
                self.current_text = []
        elif tag == "div" and self.in_gs_ri:
            self.in_gs_ri -= 1

    def handle_data(self, data):
        if self.in_anchor:
            self.current_text.append(data)


def collect_fixture_titles(src: Path) -> list[str]:
    titles: list[str] = []
    fixtures = src / "tests" / "fixtures"
    for html_file in sorted(fixtures.glob("*.html")):
        parser = _TitleCollector()
        parser.feed(html_file.read_text(encoding="utf-8"))
        titles.extend(parser.titles)
    return titles


def edge_case_titles() -> list[str]:
    return [
        "",
        "   ",
        "Deep Learning",
        "DEEP   learning",
        "Héllo—Wörld! Ünïcode Ça_se",
        "学習機械 Learning 医学 AI",
        "یادگیری عمیق برای پزشکی",
        "A very long title " + "word " * 60,
        "ω] foo (bar) [baz] {qux}",
        "foo bar",
        "10.1038/Nature-Genetics_Review!",
        "quote \"nested\" 'single' `tick`",
        "emojis 🚀🎉 and symbols → ≤ ≥",
        "ß sharp Flenﬂix ﬁ",
    ]


def run_parity(parity_bin: Path, titles: list[str]) -> dict[str, str]:
    lines = "\n".join(json.dumps({"title": t}) for t in titles)
    proc = subprocess.run(
        [str(parity_bin), "title-keys"],
        input=lines,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if proc.returncode != 0:
        raise RuntimeError(f"gss-parity failed: {proc.stderr}")
    out: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        record = json.loads(line)
        out[record["title"]] = record["key"]
    return out


def python_title_keys(src: Path, titles: list[str]) -> dict[str, str]:
    sys.path.insert(0, str(src / "src"))
    from google_scholar_scraper.models import normalize_title_key

    return {t: normalize_title_key(t) for t in titles}


def gate1(src: Path, parity_bin: Path) -> int:
    titles = collect_fixture_titles(src)
    titles.extend(edge_case_titles())
    # Packaging-smoke fixture titles (from app.py run_packaging_smoke).
    titles.append("Unicode Smoke: cafe, alpha + beta, 医学 AI")
    titles.append("Unranked Smoke")

    rust_keys = run_parity(parity_bin, titles)
    py_keys = python_title_keys(src, titles)

    mismatches = [(t, py_keys[t], rust_keys.get(t)) for t in titles if py_keys[t] != rust_keys.get(t)]
    if mismatches:
        print(f"gate1 FAIL: {len(mismatches)} mismatch(es)")
        for t, py, rs in mismatches[:10]:
            print(f"  title={t!r}\n    python={py!r}\n    rust  ={rs!r}")
        return 1
    print(f"gate1 PASS: {len(titles)} titles, zero mismatches")
    return 0


def rust_parse_pages(parity_bin: Path, jobs: list[dict]) -> list[dict]:
    lines = "\n".join(json.dumps(j) for j in jobs)
    proc = subprocess.run(
        [str(parity_bin), "parse-page"],
        input=lines,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if proc.returncode != 0:
        raise RuntimeError(f"gss-parity parse-page failed: {proc.stderr}")
    return [json.loads(line) for line in proc.stdout.splitlines()]


def python_parse_pages(src: Path, jobs: list[dict]) -> list[dict]:
    sys.path.insert(0, str(src / "src"))
    from google_scholar_scraper.scraper.parser import parse_scholar_page

    out = []
    for job in jobs:
        html = Path(job["html_path"]).read_text(encoding="utf-8")
        result = parse_scholar_page(html, status_code=job.get("status_code", 200))
        articles = [
            {
                "title": a.title,
                "authors": a.authors,
                "link": a.link,
                "relevance_score": a.relevance_score,
                "doi": a.doi,
                "journal": a.journal,
                "issns": list(a.issns),
                "publication_year": a.publication_year,
                "sjr_quartile": a.sjr_quartile,
                "sjr_year": a.sjr_year,
                "sjr_category": a.sjr_category,
                "sjr_source_url": a.sjr_source_url,
                "sinta_rank": a.sinta_rank,
                "sinta_year": a.sinta_year,
                "sinta_source_url": a.sinta_source_url,
                "pdf_url": a.pdf_url,
                "pdf_source_url": a.pdf_source_url,
                "pdf_status": a.pdf_status,
                "pdf_detail": a.pdf_detail,
                "pdf_local_path": a.pdf_local_path,
            }
            for a in result.articles
        ]
        out.append(
            {
                "html_path": job["html_path"],
                "status": result.status.value,
                "message": result.message,
                "diagnostic": result.diagnostic,
                "articles": articles,
            }
        )
    return out


def gate2(src: Path, parity_bin: Path) -> int:
    fixtures = sorted((src / "tests" / "fixtures").glob("*.html"))
    jobs = [{"html_path": str(f), "status_code": 200} for f in fixtures]
    jobs.append(
        {
            "html_path": str(src / "tests" / "fixtures" / "scholar_results_basic.html"),
            "status_code": 429,
        }
    )
    py = python_parse_pages(src, jobs)
    rs = rust_parse_pages(parity_bin, jobs)
    mismatches = 0
    for p, r in zip(py, rs):
        if p != r:
            mismatches += 1
            print(f"gate2 MISMATCH for {p['html_path']} (status {p['status']} vs {r['status']})")
            for field in ("status", "message", "diagnostic"):
                if p[field] != r[field]:
                    print(f"  {field}: python={p[field]!r} rust={r[field]!r}")
            pa, ra = p["articles"], r["articles"]
            if len(pa) != len(ra):
                print(f"  article count: python={len(pa)} rust={len(ra)}")
            for i, (a, b) in enumerate(zip(pa, ra)):
                if a != b:
                    for k in a:
                        if a[k] != b.get(k):
                            print(f"  article[{i}].{k}: python={a[k]!r} rust={b.get(k)!r}")
    if mismatches:
        print(f"gate2 FAIL: {mismatches} mismatched fixture(s) of {len(jobs)}")
        return 1
    print(f"gate2 PASS: {len(jobs)} parse jobs, zero diffs")
    return 0


def run_parity_lines(parity_bin: Path, subcommand: str, jobs: list[dict]) -> list[dict]:
    lines = "\n".join(json.dumps(j) for j in jobs)
    proc = subprocess.run(
        [str(parity_bin), subcommand],
        input=lines,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if proc.returncode != 0:
        raise RuntimeError(f"gss-parity {subcommand} failed: {proc.stderr}")
    return [json.loads(line) for line in proc.stdout.splitlines()]


def article_json(a) -> dict:
    from dataclasses import asdict

    d = asdict(a)
    d["issns"] = list(d["issns"])
    return d


def citation_json(r) -> dict:
    return {
        "title": r.title,
        "authors": list(r.authors),
        "year": r.year,
        "container": r.container,
        "entry_type": r.entry_type,
        "doi": r.doi,
        "issns": list(r.issns),
        "volume": r.volume,
        "issue": r.issue,
        "pages": r.pages,
        "publisher": r.publisher,
        "url": r.url,
        "source": r.source,
        "verified": r.verified,
        "fallback_authors_line": r.fallback_authors_line,
        "warnings": list(r.warnings),
    }


def gate4(src: Path, parity_bin: Path) -> int:
    import base64
    import tempfile

    sys.path.insert(0, str(src / "src"))
    from google_scholar_scraper.exporters import save_to_bibtex, save_to_csv, save_to_ris
    from google_scholar_scraper.enrichment import UNRESOLVED_WARNING, SCHOLAR_AUTHORS_WARNING
    from google_scholar_scraper.models import Article, CitationRecord

    articles = [
        Article(
            title="Unicode Smoke: cafe, alpha + beta, 医学 AI",
            authors='A Author, "B" Author - Journal, 2024',
            link="https://example.edu/a?x=1,y=2",
            relevance_score=88.8,
            doi="10.1234/example.2024.001",
            journal="Journal of Examples",
            issns=("1234-5679", "1234-5687"),
            publication_year=2024,
            sjr_quartile="Q1",
            sjr_year=2023,
            sjr_category="Multidisciplinary",
            sjr_source_url="https://scimagojr.example/a",
            sinta_rank="S2",
            sinta_year=2024,
            sinta_source_url="https://sinta.example/a",
            pdf_url="https://repo.example/a.pdf",
            pdf_source_url="https://api.crossref.org/works/10.1234/example.2024.001",
            pdf_status="unknown",
            pdf_detail="Synthetic fixture: no public-PDF check was performed.",
        ),
        Article(title="=formula title", authors="-author starts with dash", link=""),
        Article(title="Unranked Smoke"),
    ]
    records = [
        CitationRecord(
            title="Deep learning",
            authors=("Yann LeCun", "Yoshua Bengio"),
            year=2015,
            container="Nature",
            entry_type="journal-article",
            doi="10.1038/nature14539",
            issns=("0028-0836", "1476-4687"),
            volume="521",
            issue="7553",
            pages="436-444",
            publisher="Springer",
            url="https://doi.org/10.1038/nature14539",
            source="crossref",
            verified=True,
        ),
        CitationRecord(
            title="A Conference Paper",
            container="Conference on Examples",
            entry_type="proceedings-article",
            year=2023,
            pages="1007",
            source="crossref",
            verified=True,
        ),
        CitationRecord(
            title="An Unresolved Paper",
            year=2024,
            source="scholar",
            fallback_authors_line="A Author, B Author - Journal of Examples, 2024 - publisher.example",
            warnings=(UNRESOLVED_WARNING, SCHOLAR_AUTHORS_WARNING),
        ),
    ]

    with tempfile.TemporaryDirectory() as temp:
        py_csv = Path(temp) / "py.csv"
        save_to_csv(articles, py_csv)
        py_csv_bytes = py_csv.read_bytes()
        py_bib = Path(temp) / "py.bib"
        save_to_bibtex(records, py_bib)
        py_bib_text = py_bib.read_text(encoding="utf-8")
        py_ris = Path(temp) / "py.ris"
        save_to_ris(records, py_ris)
        # newline="" preserves CRLF exactly (read_text default translates).
        py_ris_text = py_ris.read_text(encoding="utf-8", newline="")

    jobs = [{"articles": [article_json(a) for a in articles]}]
    rs = run_parity_lines(parity_bin, "export-rows", jobs)
    rs_csv = base64.b64decode(rs[0]["csv_base64"])

    fail = 0
    if rs_csv != py_csv_bytes:
        fail += 1
        print("gate4 MISMATCH: csv bytes differ")
        py_lines = py_csv_bytes.decode("utf-8-sig").split("\r\n")
        rs_lines = rs_csv.decode("utf-8-sig").split("\r\n")
        for i, (a, b) in enumerate(zip(py_lines, rs_lines)):
            if a != b:
                print(f"  line {i}:\n    python={a!r}\n    rust  ={b!r}")
                break

    cit_jobs = [
        {"records": [citation_json(r) for r in records], "format": fmt}
        for fmt in ("bibtex", "ris")
    ]
    rs_cit = run_parity_lines(parity_bin, "citations", cit_jobs)
    rs_bib = base64.b64decode(rs_cit[0]["text_base64"]).decode("utf-8")
    rs_ris = base64.b64decode(rs_cit[1]["text_base64"]).decode("utf-8")

    if rs_bib != py_bib_text:
        fail += 1
        print("gate4 MISMATCH: bibtex text differs")
        for i, (a, b) in enumerate(zip(py_bib_text.split("\n"), rs_bib.split("\n"))):
            if a != b:
                print(f"  line {i}:\n    python={a!r}\n    rust  ={b!r}")
                break
    if rs_ris != py_ris_text:
        fail += 1
        print("gate4 MISMATCH: ris text differs")
        py_parts = py_ris_text.split("\r\n")
        rs_parts = rs_ris.split("\r\n")
        for i in range(max(len(py_parts), len(rs_parts))):
            a = py_parts[i] if i < len(py_parts) else "<missing>"
            b = rs_parts[i] if i < len(rs_parts) else "<missing>"
            if a != b:
                print(f"  line {i}:\n    python={a!r}\n    rust  ={b!r}")
                break

    if fail:
        print(f"gate4 FAIL: {fail} export mismatch(es)")
        return 1
    print("gate4 PASS: csv + bibtex + ris byte-equal")
    return 0


def gate5(src: Path, parity_bin: Path) -> int:
    """Full offline extraction parity: scripted multi-page fixtures through
    scrape_scholar on both sides; ExtractionResult JSON must match exactly."""

    def basic_html(n: int) -> str:
        return f"""
        <html><body>
          <div class="gs_ri">
            <h3 class="gs_rt"><a href="https://example.edu/p{n}">Paper {n}</a></h3>
            <div class="gs_a">A Author - Journal, 2024</div>
          </div>
        </body></html>
        """

    def dup_html() -> str:
        return """
        <html><body>
          <div class="gs_ri">
            <h3 class="gs_rt"><a href="https://example.edu/dup#frag">  Shared
              Title  </a></h3>
            <div class="gs_a"> Author   One </div>
          </div>
          <div class="gs_ri">
            <h3 class="gs_rt"><a href="https://example.edu/unique">Unique Title</a></h3>
            <div class="gs_a">Author Two</div>
          </div>
        </body></html>
        """

    def dup2_html() -> str:
        return """
        <html><body>
          <div class="gs_ri">
            <h3 class="gs_rt"><a href="https://example.edu/dup">SHARED TITLE</a></h3>
            <div class="gs_a">Author One</div>
          </div>
        </body></html>
        """

    def rate_html() -> str:
        return "<html><body><h1>Too many requests</h1></body></html>"

    jobs = [
        {"pages": [basic_html(1), basic_html(2)], "query": "query", "ranking": True},
        {"pages": [dup_html(), dup2_html()], "query": "query", "ranking": True},
        {"pages": [dup_html(), dup2_html()], "query": "shared title", "ranking": False},
        {"pages": [basic_html(1), rate_html()], "query": "query", "ranking": True},
        {"pages": [dup_html(), rate_html()], "query": "query", "ranking": True},
        {"pages": [basic_html(1)], "query": "quantum", "ranking": True, "year_from": 2018, "year_to": 2024},
    ]

    # Python side: scrape_scholar with a FakeSession-shaped script.
    py_lines = []
    for job in jobs:
        responses = []
        for page_html in job["pages"]:
            resp = {"text": page_html, "status_code": 200}

            class R:
                pass

            r = R()
            r.text = page_html
            r.status_code = 200
            responses.append(r)

        class FakeSession:
            def __init__(self):
                self._responses = list(responses)
                self.headers = {}

            def get(self, url, params=None, timeout=None):
                return self._responses.pop(0)

        from google_scholar_scraper.scraper.client import ScholarClient, scrape_scholar

        client = ScholarClient(session=FakeSession(), page_delay_seconds=0, backoff_seconds=0, max_retries=1)
        result = scrape_scholar(
            job["query"], len(job["pages"]), client=client,
            ranking_enabled=job["ranking"],
            year_from=job.get("year_from"), year_to=job.get("year_to"),
        )
        from dataclasses import asdict

        d = asdict(result)
        d["status"] = result.status.value
        d["articles"] = [article_json(a) for a in result.articles]
        py_lines.append(d)

    rs = run_parity_lines(parity_bin, "extract", jobs)
    mismatches = 0
    for p, r in zip(py_lines, rs):
        if p != r:
            mismatches += 1
            print(f"gate5 MISMATCH: status {p['status']} vs {r['status']}")
            for field in ("status", "message", "diagnostic", "requested_pages",
                          "successful_pages", "failure_page", "duplicates_removed",
                          "invalid_articles_removed"):
                if p[field] != r[field]:
                    print(f"  {field}: python={p[field]!r} rust={r[field]!r}")
            pa, ra = p["articles"], r["articles"]
            if len(pa) != len(ra):
                print(f"  article count: python={len(pa)} rust={len(ra)}")
            for i, (a, b) in enumerate(zip(pa, ra)):
                if a != b:
                    for k in a:
                        if a[k] != b.get(k):
                            print(f"  article[{i}].{k}: python={a[k]!r} rust={b.get(k)!r}")
    if mismatches:
        print(f"gate5 FAIL: {mismatches} extraction job(s) differ")
        return 1
    print(f"gate5 PASS: {len(jobs)} extraction jobs, zero diffs")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True, help="path to the python project root (src/)")
    ap.add_argument("--rust", required=True, help="path to the rust workspace root")
    ap.add_argument("--parity", default=None, help="path to gss-parity binary (default: rust/target/debug/gss-parity[.exe])")
    args = ap.parse_args()

    src = Path(args.python).resolve()
    rust = Path(args.rust).resolve()
    parity_bin = Path(args.parity) if args.parity else rust / "target" / "debug" / (
        "gss-parity.exe" if sys.platform == "win32" else "gss-parity"
    )
    if not parity_bin.exists():
        print(f"parity binary not found: {parity_bin}; build with cargo build -p gss-parity", file=sys.stderr)
        return 2

    failures = 0
    failures += gate1(src, parity_bin)
    failures += gate2(src, parity_bin)
    failures += gate4(src, parity_bin)
    failures += gate5(src, parity_bin)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
