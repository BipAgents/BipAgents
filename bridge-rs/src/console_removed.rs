//! The web console is gone (BipAgents 1.1, Alex 27/11: « Retirer partout »). It was the agents in a computer's
//! browser (`/console/`): the browser showed a 6-digit code, the person approved it in the app, and a session cookie
//! stood for the bridge key for 30 days.
//!
//! - No page, no `/console/…` and no `/v1/console/…` route any more: an app of before 1.1 gets the 404 of any path no
//!   route serves (`{"detail": "Not Found"}`), which it already read as « this bridge has no web console » (iOS
//!   `consoleDevices` → nil, `approveConsole` → false; Android `orNull404`).
//! - No browser session stands for the bridge key any more (auth.rs): a console's cookie and `X-Bip-Console` header
//!   get the 401 of no key.
//! - At each start ([`at_start`]), what the console kept is deleted: `<data>/console.json` (its sessions, one-time
//!   links and pairings, as digests) and a `console.json.tmp` a write cut short left. The browsers it let in are
//!   thus revoked for good, even if an older bridge were put back on this data. The phone's bridge key and the
//!   devices' own keys (device-keys.json) are not touched: a console never had one (device_keys.rs refused it).

use crate::config::Config;
use crate::py;

/// The console's store, in the data folder (console.py, then console.rs until 1.1).
pub const FILE: &str = "console.json";

/// How many browser sessions a console file still held (unreadable: 0, it goes all the same).
fn sessions_in(text: &str) -> usize {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|data| data.get("sessions").and_then(|s| s.as_object()).map(|s| s.len()))
        .unwrap_or(0)
}

/// Deletes `path` (a file, or whatever stands there). Whether something was there.
fn remove(path: &std::path::Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path).map(|_| true),
        Ok(_) => std::fs::remove_file(path).map(|_| true),
    }
}

/// At each start (create_app): the web console's file deleted, said in the log. Nothing there: nothing to do (every
/// start after the first). A failure is logged as an error and tried again at the next start; meanwhile the file
/// opens nothing (no code reads it). Whether none is left.
pub fn at_start(config: &Config) -> bool {
    let dir = config.data_dir();
    let path = std::path::PathBuf::from(py::join(&dir, FILE));
    let sessions = std::fs::read_to_string(&path).map(|text| sessions_in(&text)).unwrap_or(0);
    let mut left = false;
    for target in [path.clone(), path.with_file_name(format!("{FILE}.tmp"))] {
        match remove(&target) {
            Ok(true) => tracing::info!(
                file = %target.display(),
                sessions,
                "the web console is gone (1.1): its file deleted, every browser it let in revoked"
            ),
            Ok(false) => {}
            Err(e) => {
                left = true;
                tracing::error!(file = %target.display(), error = %e,
                    "the web console's file could not be deleted: tried again at the next start (it opens nothing)");
            }
        }
    }
    !left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_counted_from_the_file() {
        assert_eq!(sessions_in(r#"{"sessions": {"a": {}, "b": {}}, "links": {}}"#), 2);
        assert_eq!(sessions_in(r#"{"links": {"x": 1}}"#), 0);
        assert_eq!(sessions_in("not json"), 0);
    }

    #[test]
    fn removes_a_file_a_folder_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        assert!(remove(&file).unwrap() && !file.exists());
        assert!(!remove(&file).unwrap());
        let folder = dir.path().join("d");
        std::fs::create_dir_all(folder.join("inner")).unwrap();
        assert!(remove(&folder).unwrap() && !folder.exists());
    }
}
