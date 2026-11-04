//! URL split/unsplit mirroring Python's `urllib.parse.urlsplit`/`urlunsplit`
//! for the exact subset of cases the app produces.

#[derive(Debug, Clone, PartialEq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// Split a URL into (scheme, netloc, path, query, fragment) following the
/// RFC 3986 generic syntax that `urlsplit` implements.
pub fn split(url: &str) -> Option<SplitUrl> {
    let mut rest = url;

    // Fragment first: it binds loosest.
    let mut fragment = String::new();
    if let Some(pos) = rest.find('#') {
        fragment = rest[pos + 1..].to_string();
        rest = &rest[..pos];
    }

    // Scheme: `[^:/?#]+:` at the start.
    let mut scheme = String::new();
    if let Some(pos) = rest.find(':') {
        let candidate = &rest[..pos];
        if !candidate.is_empty()
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
            && candidate
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic())
                .unwrap_or(false)
        {
            scheme = candidate.to_string();
            rest = &rest[pos + 1..];
        }
    }

    let mut netloc = String::new();
    if rest.starts_with("//") {
        let after = &rest[2..];
        let end = after.find(['/', '?']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        rest = &rest[2 + end..];
    }

    let mut query = String::new();
    if let Some(pos) = rest.find('?') {
        query = rest[pos + 1..].to_string();
        rest = &rest[..pos];
    }

    Some(SplitUrl {
        scheme,
        netloc,
        path: rest.to_string(),
        query,
        fragment,
    })
}

/// Reassemble components (`urlunsplit`): empty netloc drops `//` unless the
/// path starts with `//`; empty query/fragment are omitted.
pub fn unsplit(scheme: &str, netloc: &str, path: &str, query: &str, fragment: &str) -> String {
    let mut url = String::new();
    if !scheme.is_empty() {
        url.push_str(scheme);
        url.push(':');
    }
    if (!netloc.is_empty() || (!path.is_empty() && path.starts_with("//")))
        && (!scheme.is_empty() || !netloc.is_empty() || !path.is_empty())
    {
        url.push_str("//");
    }
    url.push_str(netloc);
    url.push_str(path);
    if !query.is_empty() {
        url.push('?');
        url.push_str(query);
    }
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(fragment);
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_reassembles() {
        let u = split("https://Example.edu:8080/a/b?x=1&y=2#frag").unwrap();
        assert_eq!(u.scheme, "https");
        assert_eq!(u.netloc, "Example.edu:8080");
        assert_eq!(u.path, "/a/b");
        assert_eq!(u.query, "x=1&y=2");
        assert_eq!(u.fragment, "frag");
        assert_eq!(
            unsplit(&u.scheme, &u.netloc, &u.path, &u.query, ""),
            "https://Example.edu:8080/a/b?x=1&y=2"
        );
    }

    #[test]
    fn relative_urls_have_no_scheme() {
        let u = split("/scholar?cluster=1#frag").unwrap();
        assert_eq!(u.scheme, "");
        assert_eq!(u.netloc, "");
        assert_eq!(u.path, "/scholar");
        assert_eq!(u.query, "cluster=1");
        assert_eq!(u.fragment, "frag");
        assert_eq!(
            unsplit("", "", "/scholar", "cluster=1", ""),
            "/scholar?cluster=1"
        );
    }

    #[test]
    fn scheme_only_when_valid() {
        // `1abc:` is not a valid scheme → treated as path.
        let u = split("1abc://host/x").unwrap();
        assert_eq!(u.scheme, "");
        assert_eq!(u.netloc, "");
        assert_eq!(u.path, "1abc://host/x");
    }
}
