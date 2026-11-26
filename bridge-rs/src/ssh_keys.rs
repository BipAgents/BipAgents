//! `/v1/ssh` on a place of the all-included offer, closed (Alex, 2026-11-26). Since 20/10 the app could put an SSH key
//! in the place's account, which gave the person a shell on a server shared with other places: where the attacks on
//! the neighbours started (security review 2026-11-26: C1, E1, E3, E7), and nobody on the offer needed it (whoever
//! wants SSH has a server of their own, where it stays theirs). The account has no login shell now and the server's
//! SSH refuses it (installer/cloud: user-data.yaml, add-seat.sh, seat-agent.py's `no_login`). On someone's own computer
//! the app never offered SSH. An app from before still asks, and gets what it already shows without an error screen:
//!
//!     GET    /v1/ssh                                  {enabled: false, keys: []}
//!     POST   /v1/ssh/keys                             403
//!     DELETE /v1/ssh/keys?fingerprint=SHA256:…        403
//!
//! What opened the account before goes: every key of its ~/.ssh/authorized_keys and authorized_keys2 ([`forget_all`]),
//! when a place of the offer starts ([`at_start`]) and at a reopen (cloud_setup::reopen: a device removed from the
//! person's account, a recovery; security review E3); a failure is logged as an error and tried again at each start.
//! Only in the place's home from the config ([`Config::place_home`], only while `cloud`), never $HOME: without it
//! nothing is touched.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;

use crate::app::AppState;
use crate::config::Config;
use crate::web::{self, ApiResult, HttpError, QueryMap};

/// The account whose keys these are: the place's home from the config, on a place of the offer only. Never $HOME.
pub fn folder(config: &Config) -> Option<PathBuf> {
    config.place_home.as_deref().filter(|h| config.cloud && Path::new(h).is_absolute()).map(PathBuf::from)
}

/// Python's `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let is_break =
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if is_break {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = chars.peek().copied() {
                    chars.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// ~/.ssh/<name> (700/600) replaced at once: a temporary file renamed over it.
fn write_named(home: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let folder = home.join(".ssh");
    crate::py::makedirs(&folder.to_string_lossy(), 0o700)?;
    crate::py::chmod(&folder.to_string_lossy(), 0o700)?;
    let tmp = folder.join(format!(".{name}.tmp"));
    crate::py::write_with_mode(&tmp.to_string_lossy(), bytes, 0o600)?;
    std::fs::rename(&tmp, folder.join(name))
}

/// Every key that opens the place's account by SSH goes, from both files sshd reads (~/.ssh/authorized_keys and
/// authorized_keys2): the ones added from the app before 26/11, and any added since by whatever ran in the account
/// (with options or not). Comments and blank lines stay; a file that can't be read as one (a folder made in its place, say)
/// goes whole; a file with no key is left as it is. Nothing anywhere else: off a place of the offer, or without its
/// home in the config ([`folder`]), nothing is touched (0). How many lines went.
pub fn forget_all(config: &Config) -> std::io::Result<usize> {
    match folder(config) {
        Some(home) => forget_in(&home),
        None => Ok(0),
    }
}

/// When a place of the offer starts ([`at_start`]) and at a reopen ([`forget_at_reopen`]): [`forget_all`], said in
/// the log. A failure is never silent (an error); keys a reopen could not remove go at the next start, since every
/// start of a place removes them anyway: one mechanism, no mark to keep. Whether none are left.
fn forget_logged(config: &Config, when: &str) -> bool {
    match forget_all(config) {
        Ok(gone) => {
            if gone > 0 {
                tracing::info!(gone, when, "the account's SSH keys removed (no SSH on a place of the offer)");
            }
            true
        }
        Err(e) => {
            tracing::error!(error = %e, when, "the account's SSH keys could not be removed: tried again at each start");
            false
        }
    }
}

/// At each start of a place of the offer (create_app): no key left to open its account, whatever an app, a reopen
/// that failed or anything else put there before.
pub fn at_start(config: &Config) -> bool {
    forget_logged(config, "start")
}

/// At a reopen (cloud_setup::reopen), with every device key: the same. The reopen goes on whatever happens.
pub fn forget_at_reopen(config: &Config) -> bool {
    forget_logged(config, "reopen")
}

fn forget_in(home: &Path) -> std::io::Result<usize> {
    let mut gone = 0;
    for name in ["authorized_keys", "authorized_keys2"] {
        let path = home.join(".ssh").join(name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(_) => std::fs::remove_dir_all(&path)?,
                }
                gone += 1;
                continue;
            }
        };
        let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop(); // after the last line's end
        }
        let kept: Vec<&[u8]> = lines
            .iter()
            .copied()
            .filter(|line| {
                let text = line.trim_ascii();
                text.is_empty() || text.starts_with(b"#")
            })
            .collect();
        if kept.len() == lines.len() {
            continue;
        }
        gone += lines.len() - kept.len();
        let mut out = Vec::new();
        for line in kept {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
        write_named(home, name, &out)?;
    }
    Ok(gone)
}

const NOT_CLOUD: &str = "only on a server of the all-included offer";
const NO_SSH: &str = "no SSH access on a server of the all-included offer";

/// Why a key is neither added nor removed: there is no SSH here, on a place of the offer as anywhere else.
fn refused(config: &Config) -> HttpError {
    HttpError::new(403, if config.cloud { NO_SSH } else { NOT_CLOUD })
}

async fn ssh_status() -> ApiResult<Response> {
    Ok(web::ok(json!({"enabled": false, "keys": []})))
}

async fn ssh_add(State(state): State<AppState>) -> ApiResult<Response> {
    Err(refused(&state.config()))
}

async fn ssh_remove(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    web::query_required(&q, "fingerprint")?;
    Err(refused(&state.config()))
}

/// `ssh_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/ssh", get(ssh_status)).route("/ssh/keys", post(ssh_add).delete(ssh_remove))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me";

    fn keys(home: &Path) -> usize {
        let text = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap_or_default();
        text.lines().filter(|l| l.contains("ssh-ed25519")).count()
    }

    /// E3 (review 26/11): at a start or a reopen every key goes, whatever was written in the files; nothing there
    /// stops it.
    #[test]
    fn every_key_goes_comments_stay() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(forget_in(home.path()).unwrap(), 0); // no ~/.ssh at all
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let mut odd = format!("# kept\n\n{GOOD}\r\n  # indented, kept\n").into_bytes();
        odd.extend_from_slice(b"no-pty \xff\xfe not even text\n");
        odd.extend_from_slice(GOOD.as_bytes()); // no line end
        std::fs::write(ssh.join("authorized_keys"), odd).unwrap();
        std::fs::create_dir(ssh.join("authorized_keys2")).unwrap(); // a folder in its place
        std::fs::write(ssh.join("authorized_keys2").join("x"), GOOD).unwrap();
        assert_eq!(forget_in(home.path()).unwrap(), 4);
        assert_eq!(std::fs::read_to_string(ssh.join("authorized_keys")).unwrap(), "# kept\n\n  # indented, kept\n");
        assert!(!ssh.join("authorized_keys2").exists());
        assert_eq!(keys(home.path()), 0);
        assert_eq!(forget_in(home.path()).unwrap(), 0); // nothing left to take: the file untouched
    }

    /// The folder is the place's home from the config, on a place of the offer only: off one, without it, or with a
    /// relative one, nothing is touched (and $HOME is never read: this test leaves it as it is).
    #[test]
    fn only_the_places_home_from_the_config() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
        std::fs::write(home.path().join(".ssh/authorized_keys"), format!("{GOOD}\n")).unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.place_home = Some(home.path().to_string_lossy().into_owned());
        assert_eq!(forget_all(&config).unwrap(), 0, "not a place of the offer");
        config.cloud = true;
        config.place_home = Some("relative/home".into());
        assert_eq!(forget_all(&config).unwrap(), 0, "a relative home");
        config.place_home = None;
        assert_eq!((folder(&config), forget_all(&config).unwrap()), (None, 0), "no home given");
        assert_eq!(keys(home.path()), 1);
        config.place_home = Some(home.path().to_string_lossy().into_owned());
        assert_eq!(folder(&config).as_deref(), Some(home.path()));
        assert_eq!(forget_all(&config).unwrap(), 1);
        assert_eq!(keys(home.path()), 0);
    }

    /// A reopen whose keys can't go is never silent, and they go at the next start of the place, which removes them
    /// at each start anyway (review of fix-device-removal: no reopen's failure left behind).
    #[test]
    fn keys_a_reopen_could_not_remove_go_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".ssh"), "a file where the folder should be").unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.cloud = true;
        config.place_home = Some(home.to_string_lossy().into_owned());
        assert!(!forget_at_reopen(&config));
        assert!(!at_start(&config), "still failing: tried again at each start");
        std::fs::remove_file(home.join(".ssh")).unwrap();
        std::fs::create_dir(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/authorized_keys"), format!("{GOOD}\n")).unwrap();
        assert!(at_start(&config));
        assert_eq!(keys(&home), 0);
        assert!(at_start(&config), "nothing left: nothing to do");
    }
}
