//! Text identity + normalization functions ported from `models.py` and
//! `pdf_service.py::_safe_filename` verbatim.

use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

static NON_ALNUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^0-9a-z]+").unwrap());
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static ISSN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{4})-?(\d{3}[\dX])$").unwrap());
static ILLEGAL_NAME_CHARS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[<>:"/\\|?*\x00-\x1f]"#).unwrap());

/// Python `str.casefold()` semantics for the mapping classes this app folds.
pub trait CaseFold {
    fn casefold(&self) -> String;
}

impl CaseFold for str {
    fn casefold(&self) -> String {
        self.chars()
            .flat_map(|c| match c {
                'ß' | 'ẞ' => "ss".chars().collect::<Vec<_>>(),
                'ﬀ' => "ff".chars().collect(),
                'ﬁ' => "fi".chars().collect(),
                'ﬂ' => "fl".chars().collect(),
                'ﬃ' => "ffi".chars().collect(),
                'ﬄ' => "ffl".chars().collect(),
                'ﬆ' | 'ﬅ' => "st".chars().collect(),
                'ς' => "σ".chars().collect(),
                c => c.to_lowercase().collect::<Vec<_>>(),
            })
            .collect()
    }
}

impl CaseFold for String {
    fn casefold(&self) -> String {
        self.as_str().casefold()
    }
}

/// Public re-export of the combining-mark test for other modules.
pub fn is_combining_pub(c: char) -> bool {
    is_combining(c)
}

/// Split on runs of non-`[0-9a-z]` (Python `re.split(r"[^0-9a-z]+", ...)`).
pub fn split_non_alnum(value: &str) -> Vec<String> {
    NON_ALNUM_RE
        .split(value)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Conservative comparison key for an article title (`normalize_title_key`).
/// NFKD → drop combining marks → casefold → remove non `[0-9a-z]` runs.
pub fn normalize_title_key(value: &str) -> String {
    let value = if value.is_empty() { "" } else { value };
    let normalized: String = value.nfkd().collect();
    let decomposed: String = normalized.chars().filter(|c| !is_combining(*c)).collect();
    NON_ALNUM_RE
        .replace_all(&decomposed.casefold(), "")
        .into_owned()
}

/// Approximation of Python's `unicodedata.combining`: nonzero canonical
/// combining class. Covers Mn/Mc/Me combining marks.
fn is_combining(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}' // combining diacritical marks
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05BD}'
            | '\u{05BF}' | '\u{05C1}' | '\u{05C2}'
            | '\u{0610}'..='\u{061A}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{06D6}'..='\u{06DC}'
            | '\u{06DF}'..='\u{06E4}'
            | '\u{0711}'
            | '\u{0730}'..='\u{074A}'
            | '\u{07A6}'..='\u{07B0}'
            | '\u{0816}'..='\u{082D}'
            | '\u{0900}'..='\u{0903}'
            | '\u{093A}'..='\u{094F}'
            | '\u{0951}'..='\u{0957}'
            | '\u{0962}' | '\u{0963}'
            | '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20F0}'
            | '\u{FE00}'..='\u{FE0F}' // variation selectors
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// Canonical `1234-567X` ISSN or empty (`normalize_issn`), checksum validated.
pub fn normalize_issn(value: &str) -> String {
    let text = WHITESPACE_RE.replace_all(value, "").to_uppercase();
    let Some(caps) = ISSN_RE.captures(&text) else {
        return String::new();
    };
    let candidate = format!("{}-{}", &caps[1], &caps[2]);
    let digits: String = candidate.chars().filter(|c| *c != '-').collect();
    let total: u32 = digits[..7]
        .chars()
        .enumerate()
        .map(|(index, d)| d.to_digit(10).unwrap() * (8 - index as u32))
        .sum();
    let check = (11 - total % 11) % 11;
    let expected = if check == 10 { "X" } else { &check.to_string() };
    let last = &digits[7..8];
    if last == expected {
        candidate
    } else {
        String::new()
    }
}

/// Normalize, validate, de-duplicate and order ISSNs (`normalize_issn_list`).
pub fn normalize_issn_list<'a, I: IntoIterator<Item = &'a str>>(values: I) -> Vec<String> {
    let mut normalized: Vec<String> = Vec::new();
    for value in values {
        let issn = normalize_issn(value);
        if !issn.is_empty() && !normalized.contains(&issn) {
            normalized.push(issn);
        }
    }
    normalized.sort();
    normalized
}

const DOI_PREFIXES: [&str; 5] = [
    "https://doi.org/",
    "http://doi.org/",
    "https://dx.doi.org/",
    "http://dx.doi.org/",
    "doi:",
];

/// Bare case-folded DOI, or empty when nothing usable is left (`normalize_doi`).
/// Only a wrapper prefix is stripped; the suffix is never guessed or repaired.
pub fn normalize_doi(value: &str) -> String {
    let doi = value.trim();
    if doi.is_empty() {
        return String::new();
    }
    let lowered = doi.casefold();
    let mut doi = doi.to_string();
    for prefix in DOI_PREFIXES {
        if lowered.starts_with(prefix) {
            doi = doi[prefix.len()..].to_string();
            break;
        }
    }
    doi.trim().casefold()
}

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const MAX_STEM: usize = 120;

/// Windows-safe filename stem from a title (`pdf_service._safe_filename`).
pub fn safe_filename(title: &str) -> String {
    let title = if title.is_empty() { "" } else { title };
    let stem: String = title.nfkc().collect();
    let stem = ILLEGAL_NAME_CHARS_RE.replace_all(&stem, " ").into_owned();
    let stem = WHITESPACE_RE.replace_all(&stem, " ").into_owned();
    let mut stem = stem.trim().trim_end_matches('.').trim().to_string();
    if stem.len() > MAX_STEM {
        stem = stem[..MAX_STEM].to_string();
        stem = stem.trim_end_matches([' ', '.']).to_string();
    }
    if stem.is_empty() {
        stem = "paper".to_string();
    }
    let head = stem.split('.').next().unwrap_or("").to_uppercase();
    if WINDOWS_RESERVED.contains(&head.as_str()) {
        stem = format!("_{stem}");
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_key_strips_case_punct_and_accents() {
        assert_eq!(normalize_title_key("Héllo—Wörld!"), "helloworld");
        assert_eq!(normalize_title_key("  Deep   Learning  "), "deeplearning");
        assert_eq!(normalize_title_key(""), "");
        // Python's `[^0-9a-z]+` class drops ALL non-ASCII-alnum runs,
        // including Arabic — a conservative Latin-only key.
        assert_eq!(normalize_title_key("یادگیری عمیق"), "");
    }

    #[test]
    fn issn_checksum_gate() {
        assert_eq!(normalize_issn("0028-0836"), "0028-0836");
        assert_eq!(normalize_issn("00280836"), "0028-0836");
        assert_eq!(normalize_issn("0028-0837"), ""); // checksum fail
        assert_eq!(normalize_issn("1234-5679"), "1234-5679");
        assert_eq!(normalize_issn("1234-5687"), "1234-5687");
        assert_eq!(normalize_issn("1234-567X"), ""); // checksum fail (expected 9)
        assert_eq!(normalize_issn("abc"), "");
        assert_eq!(normalize_issn(""), "");
    }

    #[test]
    fn issn_list_sorted_deduped() {
        let v = normalize_issn_list(["1476-4687", "0028-0836", "00280836", "bogus"]);
        assert_eq!(v, vec!["0028-0836", "1476-4687"]);
    }

    #[test]
    fn doi_prefixes_stripped_casefolded() {
        assert_eq!(
            normalize_doi("https://doi.org/10.1038/Nature"),
            "10.1038/nature"
        );
        assert_eq!(normalize_doi("doi:10.1038/x"), "10.1038/x");
        assert_eq!(normalize_doi("10.1038/x"), "10.1038/x");
        assert_eq!(normalize_doi("  "), "");
        // No fabrication: no prefix → left as casefolded text.
        assert_eq!(normalize_doi("not a doi"), "not a doi");
    }

    #[test]
    fn safe_filename_rules() {
        assert_eq!(safe_filename("Deep: Learning?"), "Deep Learning");
        assert_eq!(safe_filename("  "), "paper");
        assert_eq!(safe_filename("CON"), "_CON");
        assert_eq!(safe_filename("CON.txt"), "_CON.txt");
        assert_eq!(safe_filename("trailing..."), "trailing");
        assert_eq!(safe_filename(&"x".repeat(200)).len(), 120);
    }
}
