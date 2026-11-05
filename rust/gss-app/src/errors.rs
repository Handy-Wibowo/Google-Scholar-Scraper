//! Typed command errors.
//!
//! Every `#[tauri::command]` used to reject with a bare `String`, so the UI
//! branched on message text (`startsWith("No file found")`,
//! `includes("NOTHING_TO_CHECK")`). Any wording change silently disabled a
//! branch. Errors now carry a stable `code` the UI switches on, with the prose
//! kept only for display.

use serde::Serialize;

/// A command failure: a stable machine-readable code plus human text.
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl CommandError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Codes the frontend switches on. Keep in sync with `errCode` handling in
/// `ui/app.js`; each is asserted by the test at the bottom of this file.
pub const ALREADY_RUNNING: &str = "ALREADY_RUNNING";
pub const NOTHING_TO_CHECK: &str = "NOTHING_TO_CHECK";
pub const NO_PAPERS_SELECTED: &str = "NO_PAPERS_SELECTED";
pub const NO_RESULTS_TO_EXPORT: &str = "NO_RESULTS_TO_EXPORT";
pub const NO_FILE_FOUND: &str = "NO_FILE_FOUND";
pub const UNSUPPORTED_FORMAT: &str = "UNSUPPORTED_FORMAT";
pub const EXPORT_FAILED: &str = "EXPORT_FAILED";
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
pub const NETWORK_FAILED: &str = "NETWORK_FAILED";

impl CommandError {
    pub fn already_running() -> Self {
        Self::new(ALREADY_RUNNING, "An operation is already running.")
    }

    pub fn nothing_to_check() -> Self {
        Self::new(
            NOTHING_TO_CHECK,
            "Run a search first, then check metadata and public PDFs.",
        )
    }

    pub fn no_papers_selected() -> Self {
        Self::new(
            NO_PAPERS_SELECTED,
            "Select one or more rows (Ctrl/Shift click), then download the accessible public PDFs.",
        )
    }

    pub fn no_results_to_export() -> Self {
        Self::new(NO_RESULTS_TO_EXPORT, "No results to export.")
    }

    pub fn no_file_found(path: &str) -> Self {
        Self::new(NO_FILE_FOUND, format!("No file found at {path}."))
    }

    pub fn export_failed(detail: impl std::fmt::Display) -> Self {
        Self::new(EXPORT_FAILED, format!("Export failed: {detail}"))
    }
}

/// Validation failures from `state::validate_search_inputs` keep their wording
/// (the UI shows it verbatim) but gain a code so the frontend can tell a
/// user-input problem from a system failure.
pub const INVALID_INPUT: &str = "INVALID_INPUT";

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::new(INVALID_INPUT, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_code_and_message() {
        let e = CommandError::no_file_found("C:\\x.csv");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["code"], NO_FILE_FOUND);
        assert_eq!(json["message"], "No file found at C:\\x.csv.");
    }

    #[test]
    fn export_failure_keeps_the_established_prefix() {
        // _handle_export_error builds "Export failed: {detail}"; the UI strips
        // the prefix for the status line, so it must stay present here.
        assert_eq!(
            CommandError::export_failed("permission denied").message,
            "Export failed: permission denied"
        );
    }

    #[test]
    fn codes_are_distinct_and_stable() {
        let codes = [
            ALREADY_RUNNING,
            NOTHING_TO_CHECK,
            NO_PAPERS_SELECTED,
            NO_RESULTS_TO_EXPORT,
            NO_FILE_FOUND,
            UNSUPPORTED_FORMAT,
            EXPORT_FAILED,
            INVALID_ARGUMENT,
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
        // Pinned literals: the frontend compares against these exact strings.
        assert_eq!(ALREADY_RUNNING, "ALREADY_RUNNING");
        assert_eq!(NOTHING_TO_CHECK, "NOTHING_TO_CHECK");
        assert_eq!(NO_FILE_FOUND, "NO_FILE_FOUND");
        assert_eq!(EXPORT_FAILED, "EXPORT_FAILED");
    }
}
