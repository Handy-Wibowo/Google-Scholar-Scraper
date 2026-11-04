//! gss-app: Windows desktop GUI shell (Tauri v2, plain static UI — no node).
//!
//! Two-mode state machine (IDLE/RUNNING(operation)) with worker threads that
//! stream progress events on per-operation channels; a shared AtomicBool
//! cancel; row identity = stored index (iid `article-{i}`) surviving filters;
//! synchronous Excel/CSV exports; background citation export. All user-facing
//! strings verbatim from the Python UI tables.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod errors;
mod logging;
mod settings;
mod state;
mod strings;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--packaging-smoke" {
        let dir = args.get(2).cloned().unwrap_or_default();
        if let Err(msg) = commands::run_packaging_smoke(std::path::Path::new(&dir)) {
            eprintln!("packaging smoke failed: {msg}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "--selfcheck-layout" {
        // Headless layout invariants: window sizing math mirrors the Python
        // probe (default 1080x700, min 860x560, clamp to screen-(20,80)).
        commands::selfcheck_layout();
        return;
    }

    logging::install_panic_hook();
    logging::info("app_start", &format!("version={}", strings::APP_VERSION));

    tauri::Builder::default()
        .manage({
            // Restore the imported catalog before the window opens so ranks are
            // applied to a restored session without re-importing (6.16).
            let st = state::AppState::new();
            if let Ok(mut guard) = st.catalog.lock() {
                *guard = settings::load_catalog();
            }
            st
        })
        .invoke_handler(tauri::generate_handler![
            commands::search_scholar,
            commands::check_metadata_pdfs,
            commands::download_pdfs,
            commands::import_ranking,
            commands::export_citations,
            commands::export_excel,
            commands::export_csv,
            commands::export_excel_rich,
            commands::export_tabular,
            commands::export_citations_extra,
            commands::format_citation_text,
            commands::read_text_file,
            commands::cancel_operation,
            commands::pick_path,
            commands::load_settings,
            commands::load_catalog_records,
            commands::journal_rank_history,
            commands::save_settings,
            commands::save_results,
            commands::load_results,
            commands::fetch_abstract,
            commands::reveal_path,
            commands::open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running gss-app");
}
