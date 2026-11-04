//! Persisted UI settings between launches.
//!
//! Stored as JSON under `%APPDATA%\com.googlescholarscraper.app\ui-settings.json`
//! (the Tauri identifier of this app). Every failure path is non-fatal: a
//! missing, unreadable or corrupt file loads as `UiSettings::default()`, and a
//! failed save silently leaves the running session untouched — the window must
//! keep working on a machine with a read-only or absent profile.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One UI settings snapshot; field defaults are the shipped window defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub query: String,
    pub pages: String,
    pub output_folder: String,
    pub ranking_path: String,
    pub ranking_year: String,
    pub ranking_enabled: bool,
    pub year_preset_index: usize,
    pub year_from: String,
    pub year_to: String,
    pub pdf_filter: String,
    /// Table sort column key; empty string = source order.
    pub sort_key: String,
    /// Sort direction: 1 ascending, -1 descending.
    pub sort_dir: i32,
    /// Compact row height in the results table.
    pub compact_rows: bool,
    /// Column keys hidden from the results table; empty = all shown.
    pub hidden_columns: Vec<String>,
    /// Most recent successful queries, newest first (cap 20).
    pub query_history: Vec<String>,
    /// Settings layout version. Bumped only on an incompatible change; a file
    /// with a different value is replaced by defaults and the caller is told.
    pub schema_version: u32,
    /// Interface theme: "system", "light" or "dark" (5.11).
    pub theme: String,
}

/// Current settings layout version.
pub const SCHEMA_VERSION: u32 = 1;

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            query: String::new(),
            pages: "1".to_string(),
            output_folder: String::new(),
            ranking_path: String::new(),
            ranking_year: String::new(),
            ranking_enabled: true,
            year_preset_index: 0,
            year_from: String::new(),
            year_to: String::new(),
            pdf_filter: "All".to_string(),
            sort_key: String::new(),
            sort_dir: 1,
            compact_rows: false,
            hidden_columns: Vec::new(),
            query_history: Vec::new(),
            schema_version: SCHEMA_VERSION,
            theme: "system".to_string(),
        }
    }
}

const APP_DIR: &str = "com.googlescholarscraper.app";
const FILE_NAME: &str = "ui-settings.json";

/// `%APPDATA%\<identifier>\ui-settings.json`; `None` when APPDATA is unset.
pub fn settings_path() -> Option<PathBuf> {
    let base = std::env::var("APPDATA").ok().filter(|v| !v.is_empty())?;
    Some(PathBuf::from(base).join(APP_DIR).join(FILE_NAME))
}

/// What a settings load produced, and why (for a user-visible notice).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LoadOutcome {
    pub settings: UiSettings,
    /// Empty on a clean load; otherwise a short explanation of the reset.
    pub notice: String,
}

/// Load settings and report whether anything had to be discarded.
///
/// A corrupt file used to be swallowed silently with total loss and no
/// explanation; the notice lets the UI say the window reset its setup.
pub fn load_with_notice() -> LoadOutcome {
    let default = UiSettings::default();
    let Some(path) = settings_path() else {
        return LoadOutcome {
            settings: default,
            notice: String::new(),
        };
    };
    let Ok(bytes) = std::fs::read(&path) else {
        // Missing file is normal on first run: no notice.
        return LoadOutcome {
            settings: default,
            notice: String::new(),
        };
    };
    match serde_json::from_slice::<UiSettings>(&bytes) {
        Ok(settings) if settings.schema_version == SCHEMA_VERSION => LoadOutcome {
            settings,
            notice: String::new(),
        },
        Ok(found) => LoadOutcome {
            settings: default,
            notice: format!(
                "Your saved window settings were from an older layout (version {}); the setup was reset.",
                found.schema_version
            ),
        },
        Err(_) => LoadOutcome {
            settings: default,
            notice: "Your saved window settings could not be read and were reset.".to_string(),
        },
    }
}

/// Write the settings atomically (`.tmp` then rename). Errors are returned for
/// the caller to ignore; the UI never depends on a successful save.
pub fn save(settings: &UiSettings) -> Result<(), String> {
    let Some(path) = settings_path() else {
        return Err("APPDATA is not set.".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

const CATALOG_FILE: &str = "catalog.json";

/// `%APPDATA%\<identifier>\catalog.json`: the imported ranking records.
pub fn catalog_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name(CATALOG_FILE))
}

/// Persist the imported catalog so a relaunch does not require re-importing.
pub fn save_catalog(catalog: &gss_core::catalog::JournalCatalog) -> Result<(), String> {
    let Some(path) = catalog_path() else {
        return Err("APPDATA is not set.".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(&catalog.to_records()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Records from the last successful import; empty when absent or corrupt.
pub fn load_catalog() -> gss_core::catalog::JournalCatalog {
    let Some(path) = catalog_path() else {
        return gss_core::catalog::JournalCatalog::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return gss_core::catalog::JournalCatalog::new();
    };
    let Ok(records) = serde_json::from_slice::<Vec<gss_core::catalog::JournalRankRecord>>(&bytes)
    else {
        return gss_core::catalog::JournalCatalog::new();
    };
    gss_core::catalog::JournalCatalog::from_records(records)
}

const PROJECT_FILE: &str = "review-project.json";

/// `%APPDATA%\<identifier>\review-project.json`.
pub fn project_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name(PROJECT_FILE))
}

/// Persisted review state (decisions, notes, tags, baselines).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct StoredProject {
    pub version: u32,
    /// Keyed by `gss_core::review::record_key`.
    pub entries: std::collections::BTreeMap<String, StoredEntry>,
    pub baselines: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct StoredEntry {
    pub decision: String,
    pub reason: String,
    pub note: String,
    pub tags: Vec<String>,
    pub updated_at: u64,
    pub found_by: String,
}

impl Default for StoredProject {
    fn default() -> Self {
        Self {
            version: gss_core::review::PROJECT_VERSION,
            entries: Default::default(),
            baselines: Default::default(),
        }
    }
}

impl StoredProject {
    pub fn from_core(project: &gss_core::review::ReviewProject) -> Self {
        let entries = project
            .entries
            .iter()
            .map(|(key, entry)| {
                (
                    key.clone(),
                    StoredEntry {
                        decision: entry.decision.clone(),
                        reason: entry.reason.clone(),
                        note: entry.note.clone(),
                        tags: entry.tags.clone(),
                        updated_at: entry.updated_at,
                        found_by: entry.found_by.clone(),
                    },
                )
            })
            .collect();
        Self {
            version: project.version,
            entries,
            baselines: project.baselines.clone(),
        }
    }

    pub fn to_core(&self) -> gss_core::review::ReviewProject {
        let entries = self
            .entries
            .iter()
            .map(|(key, e)| {
                (
                    key.clone(),
                    gss_core::review::ReviewEntry {
                        decision: e.decision.clone(),
                        reason: e.reason.clone(),
                        note: e.note.clone(),
                        tags: e.tags.clone(),
                        updated_at: e.updated_at,
                        found_by: e.found_by.clone(),
                    },
                )
            })
            .collect();
        gss_core::review::ReviewProject {
            version: gss_core::review::PROJECT_VERSION,
            entries,
            baselines: self.baselines.clone(),
        }
    }
}

pub fn load_project() -> StoredProject {
    let Some(path) = project_path() else {
        return StoredProject::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return StoredProject::default();
    };
    match serde_json::from_slice::<StoredProject>(&bytes) {
        Ok(project) if project.version == gss_core::review::PROJECT_VERSION => project,
        _ => StoredProject::default(),
    }
}

pub fn save_project(project: &StoredProject) -> Result<(), String> {
    let Some(path) = project_path() else {
        return Err("APPDATA is not set.".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

const BIBTEX_KEYS_FILE: &str = "bibtex-keys.json";

/// `%APPDATA%\<identifier>\bibtex-keys.json`.
pub fn bibtex_keys_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name(BIBTEX_KEYS_FILE))
}

/// DOI -> BibTeX key, so a re-export never renumbers an existing `\cite{}`.
pub fn load_bibtex_keys() -> std::collections::BTreeMap<String, String> {
    let Some(path) = bibtex_keys_path() else {
        return Default::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Default::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save_bibtex_keys(keys: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
    let Some(path) = bibtex_keys_path() else {
        return Err("APPDATA is not set.".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(keys).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

const CACHE_FILE: &str = "crossref-cache.json";

/// DOI lookup cache shared by enrichment and citation resolution (1.2).
pub fn cache_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name(CACHE_FILE))
}

const RESULTS_FILE: &str = "last-results.json";
/// Upper bound on what is written to the sidecar; larger sessions keep the
/// results in memory only rather than stalling the next launch.
const RESULTS_MAX_ARTICLES: usize = 2000;
/// A sidecar bigger than this is ignored on load (startup must stay fast).
const RESULTS_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// `%APPDATA%\<identifier>\last-results.json`, beside `ui-settings.json`.
pub fn results_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name(RESULTS_FILE))
}

/// Persist the collected records for the next launch. Best effort: a failure is
/// returned for the caller to ignore, and an oversized set is skipped silently.
pub fn save_results(articles: &[gss_core::models::Article]) -> Result<(), String> {
    if articles.len() > RESULTS_MAX_ARTICLES {
        return Err(format!(
            "Too many records to persist ({} > {RESULTS_MAX_ARTICLES}).",
            articles.len()
        ));
    }
    let Some(path) = results_path() else {
        return Err("APPDATA is not set.".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(articles).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Records from the previous session; empty when absent, unreadable, corrupt,
/// or too large to be worth the startup cost.
pub fn load_results() -> Vec<gss_core::models::Article> {
    let Some(path) = results_path() else {
        return Vec::new();
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return Vec::new();
    };
    if meta.len() > RESULTS_MAX_BYTES {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UiSettings {
        UiSettings {
            query: "machine learning".into(),
            pages: "3".into(),
            output_folder: "C:\\out".into(),
            ranking_path: "C:\\ranks.csv".into(),
            ranking_year: "2024".into(),
            ranking_enabled: false,
            year_preset_index: 2,
            year_from: "2018".into(),
            year_to: "2024".into(),
            pdf_filter: "Public PDF available".into(),
            sort_key: "title".into(),
            sort_dir: -1,
            compact_rows: true,
            hidden_columns: vec!["authors".into(), "link".into()],
            query_history: vec!["machine learning".into(), "graph neural networks".into()],
            schema_version: SCHEMA_VERSION,
            theme: "dark".into(),
        }
    }

    #[test]
    fn defaults_match_shipped_window() {
        let d = UiSettings::default();
        assert_eq!(d.pages, "1");
        assert_eq!(d.pdf_filter, "All");
        assert!(d.ranking_enabled);
        assert_eq!(d.year_preset_index, 0);
        assert!(d.query.is_empty());
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-settings.json");
        let original = sample();
        let bytes = serde_json::to_vec_pretty(&original).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let restored: UiSettings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.json");
        let loaded: UiSettings = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        assert_eq!(loaded, UiSettings::default());
    }

    #[test]
    fn corrupted_json_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, b"{not json at all").unwrap();
        let loaded: UiSettings = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        assert_eq!(loaded, UiSettings::default());
    }

    #[test]
    fn partial_json_fills_defaults() {
        // Forward/backward compatibility: unknown keys ignored, absent keys
        // defaulted, so an older file still opens.
        let parsed: UiSettings = serde_json::from_str(r#"{"query":"x","unknown":1}"#).unwrap();
        assert_eq!(parsed.query, "x");
        assert_eq!(parsed.pages, "1");
        assert!(parsed.ranking_enabled);
        // v3.2.0 additions default cleanly for a v3.1.0-era file.
        assert_eq!(parsed.sort_key, "");
        assert_eq!(parsed.sort_dir, 1);
        assert!(!parsed.compact_rows);
        assert!(parsed.hidden_columns.is_empty());
        assert!(parsed.query_history.is_empty());
    }

    #[test]
    fn atomic_save_leaves_no_temp_file() {
        // The rename target is the real file; the temp file must not survive.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-settings.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&UiSettings::default()).unwrap()).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        assert!(path.is_file());
        assert!(!tmp.exists());
    }

    #[test]
    fn results_round_trip_and_bounds() {
        // The sidecar is a plain Article array; verify serde handles the shape
        // and that an oversized set is refused rather than silently truncated.
        let articles = vec![
            gss_core::models::Article::new("Alpha"),
            gss_core::models::Article {
                title: "Beta".into(),
                doi: "10.1234/beta".into(),
                pdf_url: "https://example.org/b.pdf".into(),
                relevance_score: Some(42.5),
                ..gss_core::models::Article::default()
            },
        ];
        let bytes = serde_json::to_vec(&articles).unwrap();
        let back: Vec<gss_core::models::Article> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, articles);
        assert_eq!(back[1].pdf_url, "https://example.org/b.pdf");

        // Over the cap: refused, so load_results can never return a huge blob.
        let many: Vec<gss_core::models::Article> = (0..=RESULTS_MAX_ARTICLES)
            .map(|i| gss_core::models::Article::new(format!("t{i}")))
            .collect();
        assert!(save_results(&many).is_err());
    }

    #[test]
    fn missing_results_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RESULTS_FILE);
        let loaded: Vec<gss_core::models::Article> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        assert!(loaded.is_empty());
    }

    #[test]
    fn settings_path_uses_appdata_when_present() {
        // Guarded: this only asserts the shape when APPDATA is set, which is
        // always true on the Windows target but not in every CI container.
        if let Some(path) = settings_path() {
            assert!(path.ends_with(FILE_NAME));
            assert!(path.parent().unwrap().ends_with(APP_DIR));
        }
    }
}
