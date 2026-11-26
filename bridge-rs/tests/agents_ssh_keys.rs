//! A place of the all-included offer has no SSH (Alex, 2026-11-26): its account has no login shell and the server's
//! SSH refuses it (installer/cloud), and the bridge no longer opens it to a key. An app from before still asks
//! `/v1/ssh`: the bridge answers that there is none, as the apps already show it, and touches no key. A place that
//! starts, and a reopen, take away any key still in its account. The keys' folder is the place's home from the config
//! (common::config_dict: the test's own folder). Its own test binary: each test points HOME at a decoy holding a key,
//! and checks that nothing ever touched it (review 26/11: a reopen in a test once emptied the build server's own
//! authorized_keys through HOME).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;

use base64::Engine;
use bipbridge::cloud_setup::{self, CLAIM_CONTEXT};
use common::{auth, TestApp, Urls, BRIDGE_KEY};
use serde_json::{json, Value};

/// HOME is the process's: one test at a time points it at its decoy, holding this to its end (each test on a runtime
/// of its own: the guard held across its awaits blocks no one but the next test).
static HOME: Mutex<()> = Mutex::new(());

const DECOY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl machine@home\n";
const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl thief@laptop";
const NO_SSH: &str = "no SSH access on a server of the all-included offer";

/// HOME pointed at `<dir>/decoy`, whose authorized_keys holds a key: the test checks it is never touched
/// ([`untouched`]).
fn decoy(dir: &Path) -> std::path::PathBuf {
    let decoy = dir.join("decoy");
    std::fs::create_dir_all(decoy.join(".ssh")).unwrap();
    std::fs::write(decoy.join(".ssh/authorized_keys"), DECOY).unwrap();
    std::env::set_var("HOME", &decoy);
    decoy
}

fn untouched(decoy: &Path) {
    assert_eq!(std::fs::read_to_string(decoy.join(".ssh/authorized_keys")).unwrap(), DECOY, "HOME's keys were touched");
    assert_eq!(std::fs::read_dir(decoy.join(".ssh")).unwrap().count(), 1, "something was written in HOME's .ssh");
}

/// The keys an account of the offer may hold from before: one added from the app, one added by hand since (with
/// options), one in authorized_keys2 (sshd reads it too), around a comment and a blank line.
fn keys_from_before(home: &Path) {
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/authorized_keys"), format!("# kept\n{KEY}\n\nno-pty {KEY} again\n")).unwrap();
    std::fs::write(home.join(".ssh/authorized_keys2"), format!("{KEY}\n")).unwrap();
}

fn no_key_left(home: &Path) {
    assert_eq!(std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap(), "# kept\n\n", "the comment stays");
    assert!(!std::fs::read_to_string(home.join(".ssh/authorized_keys2")).unwrap_or_default().contains("ssh-ed25519"));
    assert_eq!(std::fs::metadata(home.join(".ssh/authorized_keys")).unwrap().permissions().mode() & 0o777, 0o600);
}

/// What an app from before sends: `/v1/ssh` says there is no SSH (`enabled: false`, which it shows as « not here »,
/// no error screen), adding or removing a key is refused with words of its own, and nothing is written. On someone's
/// own computer, the same answers as ever.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // HOME, on purpose (above)
async fn on_a_server_of_the_offer_the_ssh_routes_say_there_is_none_and_add_no_key() {
    let _home = HOME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let decoy = decoy(dir.path());
    let home = dir.path().join("home"); // the place's home in the config (common::config_dict)
    std::fs::create_dir(&home).unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    assert_eq!(app.app.state.config().place_home.as_deref(), Some(&*home.to_string_lossy()));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let delete = "/v1/ssh/keys?fingerprint=SHA256%3AAAAA";
    // someone's own computer: as before
    assert_eq!(app.get("/v1/ssh", &h).await.json(), json!({"enabled": false, "keys": []}));
    let refused = app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": KEY})).await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.json(), json!({"detail": "only on a server of the all-included offer"}));
    assert_eq!(app.request("DELETE", delete, &h, None).await.status, 403);
    // a place of the offer
    app.app.state.config.update(|c| {
        c.cloud = true;
        c.public_url = Some("https://203.0.113.7".into());
    });
    assert_eq!(app.get("/v1/ssh", &[]).await.status, 401);
    assert_eq!(app.get("/v1/ssh", &h).await.json(), json!({"enabled": false, "keys": []}));
    let refused = app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": KEY})).await;
    assert_eq!((refused.status.as_u16(), refused.json()), (403, json!({"detail": NO_SSH})));
    let refused = app.request("DELETE", delete, &h, None).await;
    assert_eq!((refused.status.as_u16(), refused.json()), (403, json!({"detail": NO_SSH})));
    assert!(!home.join(".ssh").exists(), "no key written");
    untouched(&decoy);
}

/// A place of the offer that starts takes away every key that opens its account (added from the app before 26/11,
/// or by hand since); on someone's own computer, nothing is touched.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // HOME, on purpose (above)
async fn a_place_of_the_offer_that_starts_takes_away_the_keys_left_in_its_account() {
    let _home = HOME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let decoy = decoy(dir.path());
    let own = dir.path().join("own");
    std::fs::create_dir(&own).unwrap();
    keys_from_before(&own.join("home"));
    let before = std::fs::read_to_string(own.join("home/.ssh/authorized_keys")).unwrap();
    drop(TestApp::new(common::config(&own, &Urls::default())));
    assert_eq!(std::fs::read_to_string(own.join("home/.ssh/authorized_keys")).unwrap(), before, "own computer");
    let offer = dir.path().join("offer");
    std::fs::create_dir(&offer).unwrap();
    keys_from_before(&offer.join("home"));
    let app = TestApp::new(place(&offer));
    no_key_left(&offer.join("home"));
    let (k, v) = auth();
    assert_eq!(app.get("/v1/ssh", &[(k, v.as_str())]).await.json(), json!({"enabled": false, "keys": []}));
    untouched(&decoy);
}

fn encoded(secret: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
}

/// A place of the all-included offer, its certificate made, its first pairing secret given (as the cloud setup
/// tests' fixture).
fn place(dir: &Path) -> bipbridge::config::Config {
    let setup = dir.join("setup.json");
    std::fs::write(&setup, json!({"pair": encoded(&[7u8; 32])}).to_string()).unwrap();
    let path = dir.join("bridge.toml");
    std::fs::write(&path, "# bridge config\n").unwrap();
    let mut d = common::config_dict(dir, &Urls::default());
    let map = d.as_object_mut().unwrap();
    map.insert("cloud".into(), json!(true));
    map.insert("public_url".into(), json!("https://203.0.113.7"));
    map.insert(
        "lan".into(),
        json!({"enabled": true, "address": "203.0.113.7", "port": 443, "cert_dir": dir.join("lan").to_string_lossy()}),
    );
    map.insert(
        "setup".into(),
        json!({"file": setup.to_string_lossy(), "progress": dir.join("progress.json").to_string_lossy()}),
    );
    let mut config = bipbridge::config::parse_config(&d, Some(&path.to_string_lossy())).unwrap();
    config.place_spare_agents = Some(0);
    bipbridge::lan::ensure_certificate(&config).unwrap();
    config
}

/// The guichet's signed order put a new pairing secret in the place (a device removed, a recovery): the tablet proves
/// it. Its answer.
async fn reopen(app: &TestApp, config: &bipbridge::config::Config) -> Value {
    let secret: Vec<u8> = (32u8..64).collect();
    std::fs::write(config.setup_file.clone().unwrap(), json!({"pair": encoded(&secret), "reopen": true}).to_string())
        .unwrap();
    let progress = config.setup_progress.clone().unwrap();
    std::fs::write(progress, json!({"step": 6, "total": 6, "label": "Checks", "done": true}).to_string()).unwrap();
    let setup = app.get("/v1/setup?nonce=abababababababababababababababab", &[]).await.json();
    let challenge = setup["challenge"].as_str().unwrap().to_string();
    let request = json!({"reopen": {"name": "Tablette"}}).to_string();
    let mac = cloud_setup::proof(&secret, &[CLAIM_CONTEXT, &challenge, &request]);
    let body = json!({"challenge": challenge, "mac": mac, "request": request});
    let again = app.send_json("POST", "/v1/setup/claim", &[], &body).await;
    assert_eq!(again.status, 200, "{}", again.text());
    again.json()
}

/// Security review 2026-11-26, E3: a key put in the account while the bridge ran (by hand, by whatever ran in it) goes
/// at the next reopen too, with every device key.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // HOME, on purpose (above)
async fn a_reopen_removes_the_ssh_keys_put_in_the_account_meanwhile() {
    let _home = HOME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let decoy = decoy(dir.path());
    let home = dir.path().join("home"); // the place's home in the config (common::config_dict)
    std::fs::create_dir(&home).unwrap();
    let config = place(dir.path());
    assert_eq!(config.place_home.as_deref(), Some(&*home.to_string_lossy()));
    let app = TestApp::new(config.clone());
    let owner = format!("Bearer {BRIDGE_KEY}");
    let phone = app.send_json("POST", "/v1/device-keys", &[("authorization", &owner)], &json!({"name": "Pixel"})).await;
    assert_eq!(phone.status, 200, "{}", phone.text());
    let phone = format!("Bearer {}", phone.json()["key"].as_str().unwrap());
    keys_from_before(&home);
    let mine = reopen(&app, &config).await;
    assert_eq!(mine["reopened"], true);
    no_key_left(&home);
    let tablet = format!("Bearer {}", mine["bridgeKey"].as_str().unwrap());
    let status = app.get("/v1/ssh", &[("authorization", &tablet)]).await.json();
    assert_eq!(status, json!({"enabled": false, "keys": []}));
    assert_eq!(app.get("/v1/ssh", &[("authorization", &phone)]).await.status, 401); // the phone's key: gone too
    untouched(&decoy);
}

/// The same on a place whose home the config doesn't give: no key added nor removed anywhere, and HOME's never read;
/// the start and the reopen go through.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // HOME, on purpose (above)
async fn without_the_places_home_in_the_config_no_key_is_touched_anywhere() {
    let _home = HOME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let decoy = decoy(dir.path());
    let mut config = place(dir.path());
    config.place_home = None;
    let app = TestApp::new(config.clone());
    let owner = format!("Bearer {BRIDGE_KEY}");
    let h = [("authorization", owner.as_str())];
    assert_eq!(app.get("/v1/ssh", &h).await.json(), json!({"enabled": false, "keys": []}));
    assert_eq!(app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": KEY})).await.status, 403);
    assert_eq!(app.request("DELETE", "/v1/ssh/keys?fingerprint=SHA256:AAAA", &h, None).await.status, 403);
    reopen(&app, &config).await;
    assert!(!dir.path().join("home").exists());
    untouched(&decoy);
}
