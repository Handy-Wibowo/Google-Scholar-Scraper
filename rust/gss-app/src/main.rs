//! gss-app: Windows desktop GUI shell (Tauri v2, plain static UI — no node).
//!
//! Two-mode state machine (IDLE/RUNNING(operation)) with worker threads that
//! stream progress events on per-operation channels; a shared AtomicBool
//! cancel; row identity = stored index (iid `article-{i}`) surviving filters;
//! synchronous Excel/CSV exports; background citation export. All user-facing
//! strings verbatim from the Python UI tables.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
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

    tauri::Builder::default()
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::search_scholar,
            commands::check_metadata_pdfs,
            commands::download_pdfs,
            commands::import_ranking,
            commands::export_citations,
            commands::export_excel,
            commands::export_csv,
            commands::cancel_operation,
            commands::pick_path,
            commands::load_settings,
            commands::save_settings,
            commands::reveal_path,
            commands::open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running gss-app");
}
