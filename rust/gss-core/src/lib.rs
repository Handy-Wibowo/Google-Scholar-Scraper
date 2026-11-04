//! gss-core: scraping, PDF, enrichment, catalog, export logic ported from the
//! Python reference implementation (v2.2.2) at ../src/.
//!
//! Contracts transcribed in docs/CONTRACTS.md (step 1).

pub mod abstracts;
pub mod catalog;
pub mod client;
pub mod crossref;
pub mod dedupe;
pub mod exporters;
pub mod models;
pub mod parser;
pub mod pdf;
pub mod ranking;
pub mod text;
pub mod url;
