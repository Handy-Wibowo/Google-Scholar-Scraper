//! Abstract retrieval helpers.
//!
//! Two providers already wired into the app expose abstracts, in two different
//! shapes:
//!
//! - Crossref returns `abstract` as a JATS XML fragment
//!   (`<jats:p>…</jats:p>` with inline tags), which must be flattened to text.
//! - OpenAlex returns `abstract_inverted_index`, a map of word → positions that
//!   must be reassembled into a sentence.
//!
//! The pure functions live here so they can be tested offline; the network
//! calls stay in `crossref` and `pdf`, next to the transports they reuse.

use serde_json::Value;
use std::collections::BTreeMap;

/// Flatten a JATS abstract fragment to plain text.
///
/// Crossref wraps abstracts in `<jats:p>` and sprinkles inline markup
/// (`<jats:italic>`, `<jats:sub>`, and the occasional bare `<p>`). Tags are
/// dropped, entities decoded, and runs of whitespace collapsed so the result
/// reads as a paragraph rather than markup.
pub fn strip_jats(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                // A closing block tag is a paragraph break, not a word joiner.
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = decode_entities(&out);
    collapse_ws(&decoded)
}

/// Collapse whitespace runs to single spaces and trim.
fn collapse_ws(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = true;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// Decode the XML entities Crossref actually emits in abstracts.
fn decode_entities(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#x2013;", "\u{2013}")
        .replace("&#x2014;", "\u{2014}")
        .replace("&#8211;", "\u{2013}")
        .replace("&#8212;", "\u{2014}")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Rebuild plain text from OpenAlex's `abstract_inverted_index`.
///
/// The index maps each word to the positions it occupies, so the inverse is a
/// position → word map rendered in order. Unknown or malformed entries are
/// skipped rather than failing the whole abstract.
pub fn reconstruct_inverted_index(index: &Value) -> Option<String> {
    let map = index.as_object()?;
    if map.is_empty() {
        return None;
    }
    let mut slots: BTreeMap<i64, &str> = BTreeMap::new();
    for (word, positions) in map {
        let Some(list) = positions.as_array() else {
            continue;
        };
        for pos in list {
            if let Some(p) = pos.as_i64() {
                // First writer wins; a duplicate position is a malformed payload.
                slots.entry(p).or_insert(word.as_str());
            }
        }
    }
    if slots.is_empty() {
        return None;
    }
    let text = slots.values().copied().collect::<Vec<_>>().join(" ");
    let text = collapse_ws(&text);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Pull a usable abstract out of a Crossref work message.
pub fn abstract_from_crossref(message: &Value) -> Option<String> {
    let raw = message.get("abstract")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let text = strip_jats(raw);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Pull a usable abstract out of an OpenAlex work payload.
pub fn abstract_from_openalex(payload: &Value) -> Option<String> {
    let index = payload.get("abstract_inverted_index")?;
    reconstruct_inverted_index(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_jats_tags_and_collapses_whitespace() {
        let raw = "<jats:p>We study   neural\n networks &amp; their uses.</jats:p>";
        assert_eq!(strip_jats(raw), "We study neural networks & their uses.");
    }

    #[test]
    fn keeps_inline_markup_text_and_decodes_entities() {
        let raw = "<jats:p>Effect of <jats:italic>in vivo</jats:italic> dosing &lt;5 mg.</jats:p>";
        assert_eq!(strip_jats(raw), "Effect of in vivo dosing <5 mg.");
    }

    #[test]
    fn separates_closing_tags_into_words() {
        // Two adjacent paragraphs must not run together.
        let raw = "<jats:p>First.</jats:p><jats:p>Second.</jats:p>";
        assert_eq!(strip_jats(raw), "First. Second.");
    }

    #[test]
    fn empty_or_tag_only_abstract_is_none() {
        assert_eq!(abstract_from_crossref(&json!({})), None);
        assert_eq!(abstract_from_crossref(&json!({"abstract": "   "})), None);
        assert_eq!(
            abstract_from_crossref(&json!({"abstract": "<jats:p></jats:p>"})),
            None
        );
    }

    #[test]
    fn crossref_abstract_is_extracted() {
        let message = json!({"abstract": "<jats:p>A study of learning.</jats:p>"});
        assert_eq!(
            abstract_from_crossref(&message).as_deref(),
            Some("A study of learning.")
        );
    }

    #[test]
    fn rebuilds_openalex_inverted_index_in_position_order() {
        let index = json!({
            "We": [0],
            "study": [1],
            "graphs.": [2]
        });
        let payload = json!({ "abstract_inverted_index": index });
        assert_eq!(
            abstract_from_openalex(&payload).as_deref(),
            Some("We study graphs.")
        );
    }

    #[test]
    fn inverted_index_handles_repeated_words() {
        // 'the' appears at positions 0 and 2.
        let index = json!({ "the": [0, 2], "cat": [1], "sat": [3] });
        let payload = json!({ "abstract_inverted_index": index });
        assert_eq!(
            abstract_from_openalex(&payload).as_deref(),
            Some("the cat the sat")
        );
    }

    #[test]
    fn inverted_index_missing_or_empty_is_none() {
        assert_eq!(abstract_from_openalex(&json!({})), None);
        assert_eq!(
            abstract_from_openalex(&json!({"abstract_inverted_index": {}})),
            None
        );
        // Wrong shape (array instead of object) must not panic.
        assert_eq!(
            abstract_from_openalex(&json!({"abstract_inverted_index": [1, 2]})),
            None
        );
    }
}
