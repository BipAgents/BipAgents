//! The doorbell (internal/next/seat-nudge.md, 28/11): a place of the offer the guichet just reopened (or will) is told
//! « go and look now » instead of waiting for its seat agent's next round (2 minutes, bipagents-seats.timer).
//!
//!     POST /v1/seat/nudge   {server, seat, at, signature}   (no bridge key: the guichet's signature instead)
//!
//! - `signature`: base64 of the guichet's Ed25519 signature (the key that signs the seat agent's orders,
//!   /etc/bipagents/guichet.pub) over `bipagents-nudge-v1\n<server>\n<seat>\n<at>`.
//! - Refused (403, nothing rung) when it is not the guichet's, when `at` is more than a minute away from now, or when it
//!   is for another server or another place than this one; 429 within 10 s of the last one rung; 404 on a place that
//!   has no doorbell (a self-install, or a seat agent of before 28/11).
//! - Rung: the place's own file in /var/lib/bipagents/nudge (`<seat>`, its account's, 0600, made by the seat agent,
//!   seat-agent.py `doorbells`) gets the time written in it; bipagents-seats.path (PathChanged on the folder) starts
//!   the seat agent's round at once, which takes the guichet's orders as always (their own signatures, checked there).
//!   The bell carries nothing: at worst, one round more.
//! - This place's seat is the file of that folder its own account owns; the server's id, `server` in it (root's).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};

/// The seat agent's folder (seat-agent.py `NUDGE_DIR`).
pub const NUDGE_DIR: &str = "/var/lib/bipagents/nudge";
/// The guichet's public key, as the seat agent checks its orders with (seat-agent.py `PUBLIC_KEY`).
pub const GUICHET_PUB: &str = "/etc/bipagents/guichet.pub";
/// What the guichet signs before the server, the place and the time (guichet-rs/src/nudge.rs).
pub const CONTEXT: &str = "bipagents-nudge-v1";
/// A bell older (or further ahead) than this is refused.
pub const MAX_AGE: i64 = 60;
/// One bell rung every 10 s at most.
pub const SPACING: f64 = 10.0;
/// Places one server holds at most (seat-agent.py `MAX_SEATS`).
pub const MAX_SEATS: i64 = 40;

#[derive(Debug, Clone, Deserialize)]
pub struct Nudge {
    pub server: String,
    pub seat: i64,
    pub at: i64,
    pub signature: String,
}

impl Validate for Nudge {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "server", Some(&self.server), 200);
        check_max_len(errors, "signature", Some(&self.signature), 200);
    }
}

/// The text the guichet signs.
pub fn message(server: &str, seat: i64, at: i64) -> String {
    format!("{CONTEXT}\n{server}\n{seat}\n{at}")
}

/// When the last bell was rung (the 10 s between two).
#[derive(Default)]
pub struct Bell {
    last: Mutex<Option<f64>>,
}

/// This place's doorbell in `dir`: the seat whose file its own account owns (a regular file, never a link), and the
/// server's id beside it. None: no doorbell here (no folder, no file of this account's, or more than one).
pub fn own_doorbell(dir: &Path) -> Option<(String, i64)> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut seats = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(seat) = name.parse::<i64>() else { continue };
        if !(1..=MAX_SEATS).contains(&seat) || seat.to_string() != name {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
        if meta.file_type().is_file() && meta.uid() == uid {
            seats.push(seat);
        }
    }
    let [seat] = seats.as_slice() else { return None };
    let server = std::fs::read_to_string(dir.join("server")).ok()?.trim().to_string();
    (!server.is_empty()).then_some((server, *seat))
}

/// The raw Ed25519 key of a PEM public key (SubjectPublicKeyInfo, what openssl and the guichet write).
fn ed25519_key(pem: &str) -> Option<[u8; 32]> {
    use base64::Engine as _;
    const SPKI: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).map(str::trim).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(body).ok()?;
    if der.len() != 44 || der[..12] != SPKI {
        return None;
    }
    der[12..].try_into().ok()
}

/// The guichet's signature over the bell's words, with its key at `public_key`.
fn signed_by_the_guichet(public_key: &Path, nudge: &Nudge) -> bool {
    use base64::Engine as _;
    let Some(key) = std::fs::read_to_string(public_key).ok().as_deref().and_then(ed25519_key) else { return false };
    let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(nudge.signature.trim()) else { return false };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(message(&nudge.server, nudge.seat, nudge.at).as_bytes(), &signature)
        .is_ok()
}

/// `nudge` checked against the doorbell folder `dir` and the guichet's key at `public_key`, then rung: the seat rung.
/// Only a bell that passed every check counts for the 10 s (refusals never hold back the guichet's next one).
pub fn ring_at(dir: &Path, public_key: &Path, nudge: &Nudge, now: f64, bell: &Bell) -> Result<i64, HttpError> {
    let Some((server, seat)) = own_doorbell(dir) else {
        return Err(HttpError::new(404, "no doorbell on this place"));
    };
    if nudge.server != server || nudge.seat != seat {
        return Err(HttpError::new(403, "not this place's bell"));
    }
    if (nudge.at as f64 - now).abs() > MAX_AGE as f64 {
        return Err(HttpError::new(403, "too old a bell"));
    }
    if !signed_by_the_guichet(public_key, nudge) {
        return Err(HttpError::new(403, "not the guichet's bell"));
    }
    let mut last = bell.last.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|at| now - at < SPACING) {
        return Err(HttpError::new(429, "rung a moment ago: the seat agent is on its way"));
    }
    ring_file(&dir.join(seat.to_string()), now)
        .map_err(|e| HttpError::new(503, format!("the doorbell could not be rung: {e}")))?;
    *last = Some(now);
    Ok(seat)
}

/// The time written in the place's own file (never made here, never through a link): its change starts the round.
fn ring_file(path: &Path, now: f64) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).truncate(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    file.write_all(format!("{}\n", now as i64).as_bytes())?;
    file.sync_all()
}

async fn nudged(State(state): State<AppState>, Json(nudge): Json<Nudge>) -> ApiResult<Response> {
    static BELL: std::sync::LazyLock<Bell> = std::sync::LazyLock::new(Bell::default);
    if !state.config().cloud {
        return Err(HttpError::new(404, "Not Found"));
    }
    let now = crate::py::time_time();
    let seat = ring_at(&PathBuf::from(NUDGE_DIR), &PathBuf::from(GUICHET_PUB), &nudge, now, &BELL)?;
    Ok(web::ok(json!({"ok": true, "seat": seat})))
}

/// `/seat/nudge`, under /v1 without the bridge key (app.rs, beside `/v1/setup`).
pub fn router() -> Router<AppState> {
    Router::new().route("/seat/nudge", post(nudged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    const SERVER: &str = "vps-test-1";

    /// A guichet's key: its pair, and its public key as /etc/bipagents/guichet.pub holds it (PEM, SubjectPublicKeyInfo).
    fn guichet(dir: &Path) -> (Ed25519KeyPair, PathBuf) {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let mut der = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
        der.extend_from_slice(pair.public_key().as_ref());
        let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
        let path = dir.join("guichet.pub");
        std::fs::write(&path, format!("-----BEGIN PUBLIC KEY-----\n{b64}\n-----END PUBLIC KEY-----\n")).unwrap();
        (pair, path)
    }

    fn signed(pair: &Ed25519KeyPair, server: &str, seat: i64, at: i64) -> Nudge {
        let signature =
            base64::engine::general_purpose::STANDARD.encode(pair.sign(message(server, seat, at).as_bytes()).as_ref());
        Nudge { server: server.into(), seat, at, signature }
    }

    /// The seat agent's folder for place `seat` of SERVER: `server`, and the place's own file (this test's account's).
    fn doorbells(dir: &Path, seat: i64) -> PathBuf {
        let folder = dir.join("nudge");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("server"), format!("{SERVER}\n")).unwrap();
        std::fs::write(folder.join(seat.to_string()), "").unwrap();
        folder
    }

    fn rung(folder: &Path, seat: i64) -> String {
        std::fs::read_to_string(folder.join(seat.to_string())).unwrap()
    }

    #[test]
    fn a_signed_nudge_touches_the_seat_file() {
        let dir = tempfile::tempdir().unwrap();
        let (pair, public_key) = guichet(dir.path());
        let folder = doorbells(dir.path(), 3);
        let now = 1_800_000_000.0;
        let bell = Bell::default();
        let seat = ring_at(&folder, &public_key, &signed(&pair, SERVER, 3, now as i64 - 5), now, &bell);
        assert_eq!(seat.map_err(|e| e.detail), Ok(3));
        assert_eq!(rung(&folder, 3), format!("{}\n", now as i64), "the time written in the place's own file");
        // the server's file is never written by the bridge
        assert_eq!(std::fs::read_to_string(folder.join("server")).unwrap(), format!("{SERVER}\n"));
    }

    #[test]
    fn a_nudge_unsigned_old_or_for_another_seat_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (pair, public_key) = guichet(dir.path());
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let (other, _) = guichet(&elsewhere);
        let folder = doorbells(dir.path(), 3);
        let now = 1_800_000_000.0;
        let at = now as i64;
        let bell = Bell::default();
        let mut forged = signed(&pair, SERVER, 3, at);
        forged.signature = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
        let mut garbled = signed(&pair, SERVER, 3, at);
        garbled.signature = "not base64!".into();
        let refused = [
            ("not the guichet's key", signed(&other, SERVER, 3, at)),
            ("a signature of nothing", forged),
            ("no signature at all", garbled),
            ("a minute and more ago", signed(&pair, SERVER, 3, at - MAX_AGE - 1)),
            ("a minute and more ahead", signed(&pair, SERVER, 3, at + MAX_AGE + 1)),
            ("for another place", signed(&pair, SERVER, 4, at)),
            ("for another server", signed(&pair, "vps-test-2", 3, at)),
        ];
        for (why, nudge) in refused {
            let said = ring_at(&folder, &public_key, &nudge, now, &bell).unwrap_err();
            assert_eq!(said.status.as_u16(), 403, "{why}: {:?}", said.detail);
            assert_eq!(rung(&folder, 3), "", "{why}: nothing rung");
        }
        // signed for this place but the signature over other words (the time changed after signing)
        let mut moved = signed(&pair, SERVER, 3, at);
        moved.at = at - 1;
        assert_eq!(ring_at(&folder, &public_key, &moved, now, &bell).unwrap_err().status.as_u16(), 403);
        // refusals never count as a bell: the right one rings at once
        assert_eq!(
            ring_at(&folder, &public_key, &signed(&pair, SERVER, 3, at), now, &bell).map_err(|e| e.detail),
            Ok(3)
        );
        // no doorbell (a seat agent of before, a self-install): 404, never 403
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let none = ring_at(&empty, &public_key, &signed(&pair, SERVER, 3, at), now, &Bell::default()).unwrap_err();
        assert_eq!(none.status.as_u16(), 404);
        let missing =
            ring_at(&dir.path().join("missing"), &public_key, &signed(&pair, SERVER, 3, at), now, &Bell::default());
        assert_eq!(missing.unwrap_err().status.as_u16(), 404);
    }

    #[test]
    fn nudges_are_ten_seconds_apart() {
        let dir = tempfile::tempdir().unwrap();
        let (pair, public_key) = guichet(dir.path());
        let folder = doorbells(dir.path(), 1);
        let bell = Bell::default();
        let now = 1_800_000_000.0;
        let ring = |at: f64| ring_at(&folder, &public_key, &signed(&pair, SERVER, 1, at as i64), at, &bell);
        assert_eq!(ring(now).map_err(|e| e.detail), Ok(1));
        let soon = ring(now + 9.0).unwrap_err();
        assert_eq!(soon.status.as_u16(), 429, "{:?}", soon.detail);
        assert_eq!(rung(&folder, 1), format!("{}\n", now as i64), "the second bell rang nothing");
        assert_eq!(ring(now + SPACING).map_err(|e| e.detail), Ok(1), "10 s after: rung again");
        assert_eq!(rung(&folder, 1), format!("{}\n", (now + SPACING) as i64));
    }
}
