//! Persistent Crossref lookup cache keyed by DOI.
//!
//! Both `enrich_articles` and `resolve_citations` keep a `HashMap` of answers
//! for the duration of one call, so a 200-record pass asks Crossref 200 times
//! and a second export asks all 200 again. This module stores the answers on
//! disk so a repeat run is instant, works offline, and is kinder to a provider
//! that rate-limits.
//!
//! Only DOI lookups are cached. Title-search answers are weaker evidence
//! (fuzzy matching) and are deliberately not persisted.

use crate::crossref::CrossrefRecord;
use std::collections::HashMap;
use std::path::Path;

/// Answers keyed by normalised DOI. `None` means "asked, nothing found", which
/// is worth remembering: a miss costs a request too.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LookupCache {
    /// Bumped on an incompatible layout change; a mismatch is discarded.
    pub version: u32,
    pub entries: HashMap<String, Option<CrossrefRecord>>,
}

/// Layout version for the cache file.
pub const CACHE_VERSION: u32 = 1;

/// Cap on stored answers. A review rarely exceeds a few thousand records, and
/// an unbounded file would grow without limit across sessions.
const MAX_ENTRIES: usize = 20_000;

impl LookupCache {
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            entries: HashMap::new(),
        }
    }

    /// Load from `path`; a missing, corrupt, or old-format file yields an empty
    /// cache rather than an error — the cache is an optimisation, never a
    /// correctness dependency.
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::new();
        };
        match serde_json::from_slice::<LookupCache>(&bytes) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            _ => Self::new(),
        }
    }

    /// Write atomically; errors are returned for the caller to log and ignore.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a DOI without performing a request, if it was seen before.
    pub fn get(&self, doi: &str) -> Option<Option<CrossrefRecord>> {
        self.entries.get(&cache_key(doi)).cloned()
    }

    /// Record an answer. Silently stops growing at `MAX_ENTRIES`.
    pub fn insert(&mut self, doi: &str, answer: Option<CrossrefRecord>) {
        let key = cache_key(doi);
        if key.is_empty() {
            return;
        }
        if self.entries.len() >= MAX_ENTRIES && !self.entries.contains_key(&key) {
            return;
        }
        self.entries.insert(key, answer);
    }
}

/// Cache key: the normalised DOI, lowercased and stripped of a resolver prefix.
pub fn cache_key(doi: &str) -> String {
    crate::text::normalize_doi(doi).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(doi: &str) -> CrossrefRecord {
        CrossrefRecord {
            doi: doi.to_string(),
            journal: "J".into(),
            ..CrossrefRecord::default()
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut cache = LookupCache::new();
        cache.insert("10.1/ABC", Some(record("10.1/abc")));
        cache.insert("10.2/miss", None);
        cache.save(&path).unwrap();

        let back = LookupCache::load(&path);
        assert_eq!(back.len(), 2);
        assert!(back.get("10.1/abc").unwrap().is_some());
        // A remembered miss stays a miss and is distinguishable from "not seen".
        assert!(back.get("10.2/miss").unwrap().is_none());
        assert!(back.get("10.3/never").is_none());
    }

    #[test]
    fn keys_are_case_insensitive() {
        let mut cache = LookupCache::new();
        cache.insert("10.1000/XYZ", Some(record("10.1000/xyz")));
        assert!(cache.get("10.1000/xyz").is_some());
        assert!(cache.get("10.1000/XyZ").is_some());
    }

    #[test]
    fn corrupt_or_old_cache_is_empty_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        std::fs::write(&path, b"{not json").unwrap();
        assert!(LookupCache::load(&path).is_empty());

        std::fs::write(&path, br#"{"version":99,"entries":{"a":null}}"#).unwrap();
        assert!(LookupCache::load(&path).is_empty());

        assert!(LookupCache::load(&dir.path().join("absent.json")).is_empty());
    }

    #[test]
    fn empty_doi_is_not_cached() {
        let mut cache = LookupCache::new();
        cache.insert("   ", None);
        assert!(cache.is_empty());
    }
}

#[cfg(test)]
mod lookup_seam_tests {
    use super::*;
    use crate::crossref::{ArticleLookup, CrossrefClient, CrossrefFetcher, TransportError};
    use std::cell::RefCell;

    /// Counts fetches so a cache hit can be proven to skip the network.
    struct CountingFetcher {
        calls: std::rc::Rc<RefCell<usize>>,
        body: String,
    }

    impl CrossrefFetcher for CountingFetcher {
        fn get(
            &mut self,
            _url: &str,
            _headers: &[(String, String)],
        ) -> Result<(Vec<u8>, u16), TransportError> {
            *self.calls.borrow_mut() += 1;
            Ok((self.body.clone().into_bytes(), 200))
        }
    }

    /// The adapter shape used by the app: consult the cache, else fetch.
    struct Cached<'a, 'b> {
        client: &'a mut CrossrefClient<'b>,
        cache: LookupCache,
        hits: usize,
    }

    impl ArticleLookup for Cached<'_, '_> {
        fn lookup(&mut self, doi: &str) -> Option<crate::crossref::CrossrefRecord> {
            if let Some(hit) = self.cache.get(doi) {
                self.hits += 1;
                return hit;
            }
            let answer = self.client.lookup(doi);
            self.cache.insert(doi, answer.clone());
            answer
        }

        fn lookup_title_citations(
            &mut self,
            title: &str,
            author: &str,
            year: Option<i32>,
            work_types: &'static [&'static str],
        ) -> Option<crate::crossref::CrossrefRecord> {
            self.client.lookup_title(title, author, year, 5, work_types)
        }
    }

    #[test]
    fn a_second_lookup_of_the_same_doi_costs_no_request() {
        // Minimal Crossref work payload which echoes the requested DOI.
        let body = r#"{"message":{"DOI":"10.1371/journal.pone.0185809","type":"journal-article","container-title":["PLOS ONE"],"ISSN":["1932-6203"]}}"#;
        let calls = std::rc::Rc::new(RefCell::new(0usize));
        let mut fetcher = CountingFetcher {
            calls: calls.clone(),
            body: body.to_string(),
        };
        let mut client = CrossrefClient::new(&mut fetcher);
        let mut lookup = Cached {
            client: &mut client,
            cache: LookupCache::new(),
            hits: 0,
        };

        let first = lookup.lookup("10.1371/journal.pone.0185809");
        assert!(first.is_some(), "first lookup must resolve");
        assert_eq!(*calls.borrow(), 1, "one request for the first lookup");

        let second = lookup.lookup("10.1371/journal.pone.0185809");
        assert!(second.is_some(), "second lookup must resolve from cache");
        assert_eq!(lookup.hits, 1, "second lookup must be served by the cache");
        assert_eq!(
            *calls.borrow(),
            1,
            "the cache must not issue a second request"
        );
    }
}
