//! The person's SSH keys for their place's account, from the app: the server is theirs, they may log in to it (as this
//! account, never root) and bring their own tools. Only on a server of the all-included offer (`cloud`): on someone's
//! own computer, the app never opens their account to a key.
//!
//!     GET    /v1/ssh                  {enabled, login?, keys: [{fingerprint, type, comment}]}
//!     POST   /v1/ssh/keys             {key: "ssh-ed25519 AAAA… comment"} -> the key added
//!     DELETE /v1/ssh/keys?fingerprint=SHA256:…
//!
//! A key is one public key line with nothing in front (no `command=` or other options); it goes to
//! ~/.ssh/authorized_keys (700/600), which the account's other keys share.
//!
//! The folder is the place's home from the config ([`Config::place_home`], only while `cloud`), never $HOME: without it
//! nothing is shown, added nor removed.
//!
//! A reopen (cloud_setup::reopen: a device removed from the person's account, a recovery) takes every key away
//! ([`forget_all`]): a key a removed device added must not outlive it (security review 2026-11-26, E3). The person adds
//! theirs again from the app.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::Config;
use crate::web::{self, check_max_len, check_min_len, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use crate::Dict;

pub const TYPES: [&str; 7] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];
pub const MAX_KEYS: usize = 20;
static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<type>[a-z0-9@.-]+) (?P<blob>[A-Za-z0-9+/]+={0,3})(?: (?P<comment>[^\r\n]{0,200}))?\z")
        .expect("valid regex")
});

/// Python's `SshKeyError` (a `ValueError`): safe to show.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct SshKeyError(pub String);

/// A parsed key line.
#[derive(Debug, Clone, PartialEq)]
pub struct SshKey {
    pub kind: String,
    pub blob: String,
    pub comment: String,
    pub fingerprint: String,
}

impl SshKey {
    fn public(&self) -> Dict {
        let mut out = Dict::new();
        out.insert("fingerprint".into(), json!(self.fingerprint));
        out.insert("type".into(), json!(self.kind));
        out.insert("comment".into(), json!(self.comment));
        out
    }
}

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

/// `base64.b64decode(blob, validate=True)` (strict: padding required, nothing after it).
fn b64decode_strict(blob: &str) -> Option<Vec<u8>> {
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireCanonical),
    );
    engine.decode(blob).ok()
}

/// One public key line, checked: its type, its blob (base64 of a key of that type), its comment.
pub fn parse(text: &str) -> Result<SshKey, SshKeyError> {
    let stripped = text.trim();
    let lines: Vec<&str> = splitlines(stripped).into_iter().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != 1 {
        return Err(SshKeyError("one public key at a time".into()));
    }
    let line = lines[0].split_whitespace().collect::<Vec<_>>().join(" ");
    let found = LINE.captures(&line).filter(|c| TYPES.contains(&&c["type"]));
    let Some(found) = found else {
        return Err(SshKeyError("not a public SSH key (ssh-ed25519 AAAA… name)".into()));
    };
    let Some(blob) = b64decode_strict(&found["blob"]) else {
        return Err(SshKeyError("not a public SSH key (its middle part)".into()));
    };
    let kind = &found["type"];
    let named = if blob.len() < 4 {
        None
    } else {
        let n = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
        let end = 4usize.saturating_add(n).min(blob.len());
        // `.decode("ascii", "replace")`: a byte over 127 is « � ».
        Some(blob[4..end].iter().map(|&b| if b < 128 { b as char } else { '\u{FFFD}' }).collect::<String>())
    };
    if named.as_deref() != Some(kind) {
        return Err(SshKeyError("not a public SSH key (its type and its content differ)".into()));
    }
    let comment = found.name("comment").map(|m| m.as_str().trim().to_string()).unwrap_or_default();
    Ok(SshKey { kind: kind.to_string(), blob: found["blob"].to_string(), comment, fingerprint: fingerprint(&blob) })
}

/// As `ssh-keygen -l` shows it: SHA256:… without padding.
pub fn fingerprint(blob: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(blob));
    format!("SHA256:{}", encoded.trim_end_matches('='))
}

fn file(home: &Path) -> PathBuf {
    home.join(".ssh").join("authorized_keys")
}

/// The account's keys (the ones it had before the app too); lines that aren't plain keys are left out.
pub fn keys(home: &Path) -> Vec<Dict> {
    let Ok(text) = std::fs::read_to_string(file(home)) else { return Vec::new() };
    splitlines(&text).into_iter().filter_map(|line| parse(line).ok()).map(|k| k.public()).collect()
}

fn write(home: &Path, lines: &[String]) -> std::io::Result<()> {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    write_named(home, "authorized_keys", text.as_bytes())
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
/// authorized_keys2): the ones added from the app, and any added by hand since over SSH (with options or not), which a
/// key from the app let in first: on a place of the offer the account has no other. Comments and blank lines stay; a
/// file that can't be read as one (a folder made in its place, say) goes whole. Nothing anywhere else: off a place of
/// the offer, or without its home in the config ([`folder`]), nothing is touched (0). How many lines went.
pub fn forget_all(config: &Config) -> std::io::Result<usize> {
    match folder(config) {
        Some(home) => forget_in(&home),
        None => Ok(0),
    }
}

/// Where a reopen notes that the keys could not all go (a write refused, a full disk): the bridge's data folder.
fn left_mark(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("ssh-keys-to-forget")
}

/// At a reopen: [`forget_all`]. A failure is never silent: logged as an error and marked, so each next start of the
/// bridge tries again ([`forget_left`]) and says so until the keys are gone. The reopen itself goes on. Whether the
/// keys are gone.
pub fn forget_at_reopen(config: &Config) -> bool {
    let mark = left_mark(config);
    match forget_all(config) {
        Ok(gone) => {
            if gone > 0 {
                tracing::info!(gone, "the account's SSH keys removed at a reopen");
            }
            if mark.exists() {
                let _ = std::fs::remove_file(&mark);
                tracing::info!("the SSH keys a reopen could not remove are gone now");
            }
            true
        }
        Err(e) => {
            tracing::error!(error = %e, "the account's SSH keys could not be removed at a reopen: tried at each start");
            if let Err(w) = crate::py::write_with_mode(&mark.to_string_lossy(), b"1\n", 0o600) {
                tracing::error!(error = %w, "and the mark to try again could not be written");
            }
            false
        }
    }
}

/// At the bridge's start: the SSH keys a reopen could not remove (its mark), tried again. Whether none are left to
/// remove.
pub fn forget_left(config: &Config) -> bool {
    if !left_mark(config).exists() {
        return true;
    }
    tracing::warn!("a reopen left SSH keys on this place: removing them now");
    forget_at_reopen(config)
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

fn read_lines(home: &Path) -> std::io::Result<Vec<String>> {
    match std::fs::read_to_string(file(home)) {
        Ok(text) => Ok(splitlines(&text).into_iter().map(str::to_string).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The key added (once: a key already there is answered as it is).
pub fn add(home: &Path, text: &str) -> anyhow::Result<Dict> {
    let key = parse(text)?;
    let lines = read_lines(home)?;
    let existing = keys(home);
    if existing.iter().any(|k| k.get("fingerprint").and_then(|f| f.as_str()) == Some(key.fingerprint.as_str())) {
        return Ok(key.public());
    }
    if existing.len() >= MAX_KEYS {
        return Err(SshKeyError(format!("{MAX_KEYS} keys at most")).into());
    }
    let comment = if key.comment.is_empty() { "added from BipAgents".to_string() } else { key.comment.clone() };
    let mut all = lines;
    all.push(format!("{} {} {comment}", key.kind, key.blob));
    write(home, &all)?;
    Ok(SshKey { comment, ..key }.public())
}

pub fn remove(home: &Path, fingerprint: &str) -> anyhow::Result<bool> {
    let lines = match std::fs::read_to_string(file(home)) {
        Ok(text) => splitlines(&text).into_iter().map(str::to_string).collect::<Vec<_>>(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let kept: Vec<String> = lines
        .iter()
        .filter(|line| parse(line).map(|k| k.fingerprint != fingerprint).unwrap_or(true))
        .cloned()
        .collect();
    if kept.len() == lines.len() {
        return Ok(false);
    }
    write(home, &kept)?;
    Ok(true)
}

/// This account's user name (`pwd.getpwuid(os.getuid()).pw_name`).
fn user_name() -> String {
    // SAFETY: getpwuid_r writes into our buffer; the result is read only when it returned an entry.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buffer = vec![0 as libc::c_char; 16384];
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let code = libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found);
        if code == 0 && !found.is_null() && !entry.pw_name.is_null() {
            return std::ffi::CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned();
        }
    }
    std::env::var("USER").unwrap_or_default()
}

/// `ssh bip@203.0.113.7`: this account at the server's address.
pub fn login(public_url: Option<&str>) -> Option<String> {
    let parsed = url::Url::parse(public_url.unwrap_or("")).ok()?;
    let host = match parsed.host()? {
        url::Host::Ipv6(address) => address.to_string(),
        other => other.to_string().to_lowercase(),
    };
    if host.is_empty() {
        return None;
    }
    Some(format!("ssh {}@{host}", user_name()))
}

#[derive(Debug, Deserialize)]
struct KeyRequest {
    key: String,
}

impl Validate for KeyRequest {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "key", Some(&self.key), 20);
        check_max_len(errors, "key", Some(&self.key), 16384);
    }
}

const NOT_CLOUD: &str = "only on a server of the all-included offer";

async fn ssh_status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let Some(home) = folder(&config) else {
        return Ok(web::ok(json!({"enabled": false, "keys": []})));
    };
    let listed = tokio::task::spawn_blocking(move || keys(&home)).await.map_err(anyhow::Error::from)?;
    Ok(web::ok(json!({"enabled": true, "login": login(config.public_url.as_deref()), "keys": listed})))
}

async fn ssh_add(State(state): State<AppState>, Json(body): Json<KeyRequest>) -> ApiResult<Response> {
    let Some(home) = folder(&state.config()) else {
        return Err(HttpError::new(403, NOT_CLOUD));
    };
    match tokio::task::spawn_blocking(move || add(&home, &body.key)).await.map_err(anyhow::Error::from)? {
        Ok(key) => Ok(web::ok(serde_json::Value::Object(key))),
        Err(exc) => match exc.downcast_ref::<SshKeyError>() {
            Some(e) => Err(HttpError::new(400, e.0.clone())),
            None => Err(exc.into()),
        },
    }
}

async fn ssh_remove(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let fingerprint = web::query_required(&q, "fingerprint")?;
    let Some(home) = folder(&state.config()) else {
        return Err(HttpError::new(403, NOT_CLOUD));
    };
    if !tokio::task::spawn_blocking(move || remove(&home, &fingerprint)).await.map_err(anyhow::Error::from)?? {
        return Err(HttpError::new(404, "no such key"));
    }
    Ok(web::ok(json!({"ok": true})))
}

/// `ssh_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/ssh", get(ssh_status)).route("/ssh/keys", post(ssh_add).delete(ssh_remove))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me";

    /// test_ssh_keys.py: only a plain public key goes in.
    #[test]
    fn only_a_plain_public_key_goes_in() {
        assert_eq!(parse(GOOD).unwrap().kind, "ssh-ed25519");
        for bad in [
            format!("command=\"rm -rf ~\" {GOOD}"),
            format!("from=\"*\" {GOOD}"),
            "ssh-dss AAAAB3NzaC1kc3MAAACBAP me".to_string(),
            "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQC me".to_string(), // an RSA blob called ed25519
            "ssh-ed25519 not-base64!".to_string(),
            "-----BEGIN OPENSSH PRIVATE KEY-----".to_string(),
            format!("{GOOD}\nssh-rsa AAAA second"),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keys_are_added_once_and_removed() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
        std::fs::write(home.path().join(".ssh").join("authorized_keys"), "# kept\n").unwrap();
        let added = add(home.path(), GOOD).unwrap();
        assert_eq!(added["comment"], "me");
        add(home.path(), GOOD).unwrap();
        let text = std::fs::read_to_string(home.path().join(".ssh/authorized_keys")).unwrap();
        assert!(text.starts_with("# kept\n") && text.matches('\n').count() == 2);
        let fp = added["fingerprint"].as_str().unwrap().to_string();
        assert_eq!(keys(home.path()).len(), 1);
        assert!(remove(home.path(), &fp).unwrap());
        assert!(!remove(home.path(), &fp).unwrap());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join(".ssh/authorized_keys")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// E3 (review 26/11): at a reopen every key goes, whatever was written in the files; nothing there stops it.
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
        assert!(keys(home.path()).is_empty());
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
        assert_eq!(keys(home.path()).len(), 1);
        config.place_home = Some(home.path().to_string_lossy().into_owned());
        assert_eq!(folder(&config).as_deref(), Some(home.path()));
        assert_eq!(forget_all(&config).unwrap(), 1);
        assert!(keys(home.path()).is_empty());
    }

    /// A reopen whose keys can't go is never silent: marked, and the next start tries again until they are gone.
    #[test]
    fn keys_a_reopen_could_not_remove_are_removed_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".ssh"), "a file where the folder should be").unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.cloud = true;
        config.place_home = Some(home.to_string_lossy().into_owned());
        config.outbox.db_path = dir.path().join("data").join("bridge.db").to_string_lossy().into_owned();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        let mark = dir.path().join("data").join("ssh-keys-to-forget");
        assert!(forget_left(&config) && !mark.exists(), "nothing left: nothing done");
        assert!(!forget_at_reopen(&config));
        assert!(mark.exists());
        assert!(!forget_left(&config) && mark.exists(), "still failing: still marked");
        std::fs::remove_file(home.join(".ssh")).unwrap();
        std::fs::create_dir(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/authorized_keys"), format!("{GOOD}\n")).unwrap();
        assert!(forget_left(&config));
        assert!(!mark.exists() && keys(&home).is_empty());
    }

    #[test]
    fn login_names_the_host() {
        assert!(login(Some("https://203.0.113.7")).unwrap().ends_with("@203.0.113.7"));
        assert_eq!(login(Some("box.ts.net")), None);
        assert_eq!(login(None), None);
    }
}
