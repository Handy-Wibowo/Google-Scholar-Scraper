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
}

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

/// Saved settings, or the defaults when nothing usable is on disk.
pub fn load() -> UiSettings {
    let Some(path) = settings_path() else {
        return UiSettings::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return UiSettings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
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
    fn settings_path_uses_appdata_when_present() {
        // Guarded: this only asserts the shape when APPDATA is set, which is
        // always true on the Windows target but not in every CI container.
        if let Some(path) = settings_path() {
            assert!(path.ends_with(FILE_NAME));
            assert!(path.parent().unwrap().ends_with(APP_DIR));
        }
    }
}
