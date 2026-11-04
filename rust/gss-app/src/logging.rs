//! File logging and crash capture.
//!
//! Deliberately dependency-free: the app needs a rotating text log and a crash
//! file, not a distributed-tracing stack. Every failure path in the worker layer
//! used to end in `let _ = ...` / `.catch(() => {})`, so a user report carried no
//! evidence at all.
//!
//! Layout under `%APPDATA%\<identifier>\`:
//! - `logs\gss-YYYYMMDD.log` — one line per event, kept for 5 days.
//! - `logs\crash-<unix>.txt` — panic payload, thread, and location.
//!
//! Both writers are best-effort: a read-only or absent profile must never take
//! the app down, so every error here is swallowed by design.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Set while a worker thread is unwinding inside `catch_unwind`. Read by the
/// latch guard so it can log the crash without re-querying the panic hook.
static PANICKING: AtomicBool = AtomicBool::new(false);

pub fn mark_panicking() {
    PANICKING.store(true, Ordering::SeqCst);
}

pub fn take_panicking() -> bool {
    PANICKING.swap(false, Ordering::SeqCst)
}

const LOG_DIR: &str = "logs";
const KEEP_DAYS: usize = 5;

static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

/// Directory holding the log and crash files, beside the settings JSON.
fn log_dir() -> Option<PathBuf> {
    let base = std::env::var("APPDATA").ok().filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(base)
            .join("com.googlescholarscraper.app")
            .join(LOG_DIR),
    )
}

fn today_log_path(dir: &Path) -> PathBuf {
    // Civil date without a chrono dependency: YYYYMMDD from the unix day count.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    dir.join(format!("gss-{y:04}{m:02}{d:02}Z.log"))
}

/// Howard Hinnant's days-from-civil inverse; avoids a date library.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Resolve (and create) the log path once per process.
fn path() -> Option<&'static PathBuf> {
    LOG_PATH
        .get_or_init(|| {
            let dir = log_dir()?;
            std::fs::create_dir_all(&dir).ok()?;
            prune(&dir);
            Some(today_log_path(&dir))
        })
        .as_ref()
}

/// Delete logs older than `KEEP_DAYS` so the directory cannot grow unbounded.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(86_400 * KEEP_DAYS as u64));
    let Some(cutoff) = cutoff else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn write_line(line: &str) {
    let Some(path) = path() else {
        return;
    };
    let Ok(_guard) = LOCK.lock() else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// One structured log event. `fields` is free text; callers pass `key=value`
/// pairs so the line stays greppable.
pub fn log(level: &str, event: &str, fields: &str) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = format_ts(secs);
    if fields.is_empty() {
        write_line(&format!("{stamp} {level:<5} {event}"));
    } else {
        write_line(&format!("{stamp} {level:<5} {event} {fields}"));
    }
}

/// `HH:MM:SS` in **UTC**. The filename carries the UTC date too, so a log is
/// self-consistent even when the machine is in another timezone (this one is
/// UTC+7, where the local date is a day ahead for seven hours each night).
fn format_ts(secs: u64) -> String {
    let rem = secs % 86_400;
    let mut out = String::with_capacity(8);
    let _ = write!(
        out,
        "{:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    );
    out
}

pub fn info(event: &str, fields: &str) {
    log("INFO", event, fields);
}

pub fn warn(event: &str, fields: &str) {
    log("WARN", event, fields);
}

pub fn error(event: &str, fields: &str) {
    log("ERROR", event, fields);
}

/// Write a crash report and log a pointer to it. Returns the file path so the
/// panic hook can mention it in its stderr message.
pub fn write_crash(payload: &str, thread: &str, location: &str) -> Option<PathBuf> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file = dir.join(format!("crash-{secs}.txt"));
    let mut body = String::new();
    let _ = writeln!(
        body,
        "Google Scholar Scraper {} crash",
        crate::strings::APP_VERSION
    );
    let _ = writeln!(body, "os: {}", std::env::consts::OS);
    let _ = writeln!(body, "thread: {thread}");
    let _ = writeln!(body, "location: {location}");
    let _ = writeln!(body, "payload: {payload}");
    std::fs::write(&file, body).ok()?;
    error("panic", &format!("file={}", file.display()));
    Some(file)
}

/// Install the panic hook. Called once from `main`.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let saved = write_crash(&payload, &thread, &location);
        if let Some(path) = &saved {
            eprintln!("gss-app crashed; report saved to {}", path.display());
        }
        // Keep the default behaviour (stderr) so CI and debug runs still see it.
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversion_matches_known_dates() {
        // 1970-01-01 is day 0.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2024-02-29 (leap day).
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        // 2026-11-05.
        assert_eq!(civil_from_days(20_762), (2026, 11, 5));
    }

    #[test]
    fn timestamp_formats_without_panicking() {
        assert_eq!(format_ts(0), "00:00:00");
        assert_eq!(format_ts(3_661), "01:01:01");
        assert_eq!(format_ts(86_399), "23:59:59");
    }

    #[test]
    fn logging_never_panics_without_appdata() {
        // The writer is best-effort; with no APPDATA it must simply do nothing.
        // (APPDATA exists on Windows targets, so this asserts "no panic" only.)
        info("test_event", "k=v");
        warn("test_warn", "");
        error("test_error", "code=X");
    }
}
