//! test_ssh_keys.py: the person adds, lists and removes their keys on a server of the offer. The keys' folder is the
//! place's home from the config (common::config_dict: the test's own folder). Its own test binary: each test points
//! HOME at a decoy holding a key, and checks that nothing ever touched it (review 26/11: a reopen in a test once
//! emptied the build server's own authorized_keys through HOME).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use base64::Engine;
use bipbridge::cloud_setup::{self, CLAIM_CONTEXT};
use common::{auth, TestApp, Urls, BRIDGE_KEY};
use serde_json::{json, Value};

/// HOME is the process's: one test at a time points it at its decoy, holding this to its end (each test on a runtime
/// of its own: the guard held across its awaits blocks no one but the next test).
static HOME: Mutex<()> = Mutex::new(());

const DECOY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl machine@home\n";

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

fn a_key(dir: &Path, name: &str, kind: &str) -> (String, String) {
    let path = dir.join(name);
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", kind, "-N", "", "-C", &format!("{name}@laptop"), "-f"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    let public = std::fs::read_to_string(dir.join(format!("{name}.pub"))).unwrap().trim().to_string();
    let listed = Command::new("ssh-keygen").arg("-l").arg("-f").arg(dir.join(format!("{name}.pub"))).output().unwrap();
    let shown = String::from_utf8_lossy(&listed.stdout).split_whitespace().nth(1).unwrap().to_string();
    (public, shown)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // HOME, on purpose (above)
async fn the_person_adds_lists_and_removes_their_keys_on_a_server_of_the_offer() {
    let _home = HOME.lock().unwrap_or_else(|e| e.into_inner());
    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        return; // no ssh-keygen here
    }
    let dir = tempfile::tempdir().unwrap();
    let decoy = decoy(dir.path());
    let home = dir.path().join("home"); // the place's home in the config (common::config_dict)
    std::fs::create_dir(&home).unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    assert_eq!(app.app.state.config().place_home.as_deref(), Some(&*home.to_string_lossy()));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(app.get("/v1/ssh", &h).await.json(), json!({"enabled": false, "keys": []})); // someone's own computer
    let (public, _) = a_key(dir.path(), "one", "ed25519");
    assert_eq!(app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": public})).await.status, 403);
    app.app.state.config.update(|c| {
        c.cloud = true;
        c.public_url = Some("https://203.0.113.7".into());
    });
    assert_eq!(app.get("/v1/ssh", &[]).await.status, 401);
    std::fs::create_dir(home.join(".ssh")).unwrap();
    std::fs::set_permissions(home.join(".ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.join(".ssh/authorized_keys"), "# kept\n").unwrap();
    let added = app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": public})).await.json();
    let (other, shown) = a_key(dir.path(), "two", "rsa");
    app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": format!("{other}\n")})).await;
    app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": public})).await; // once only
    let status = app.get("/v1/ssh", &h).await.json();
    assert_eq!(status["enabled"], true);
    assert!(status["login"].as_str().unwrap().ends_with("@203.0.113.7"));
    let listed: Vec<(String, String)> = status["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| (k["type"].as_str().unwrap().to_string(), k["comment"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        listed,
        [("ssh-ed25519".to_string(), "one@laptop".to_string()), ("ssh-rsa".to_string(), "two@laptop".to_string())]
    );
    assert_eq!(status["keys"][1]["fingerprint"], json!(shown));
    assert_eq!(added["fingerprint"], status["keys"][0]["fingerprint"]);
    let text = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    assert!(text.starts_with("# kept\n") && text.matches('\n').count() == 3);
    assert_eq!(mode(&home.join(".ssh")), 0o700);
    assert_eq!(mode(&home.join(".ssh/authorized_keys")), 0o600);
    let query =
        format!("/v1/ssh/keys?fingerprint={}", shown.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D"));
    let gone = app.request("DELETE", &query, &h, None).await;
    assert_eq!(gone.status, 200);
    assert!(!std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap().contains("two@laptop"));
    assert_eq!(app.request("DELETE", &query, &h, None).await.status, 404);
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

/// Security review 2026-11-26, E3: a stolen phone, while it still had its device key, added an SSH key; the person
/// removes the phone from their account and the place is reopened (a new pairing secret from the guichet). The keys
/// that open the account by SSH go with every device key: the one added from the app, one added by hand since (with
/// options), one in authorized_keys2 (sshd reads it too). Comments and blank lines stay.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // HOME, on purpose (above)
async fn a_reopen_removes_the_ssh_keys_a_removed_device_could_have_added() {
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
    std::fs::create_dir(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/authorized_keys"), "# kept\n\n").unwrap();
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl thief@laptop";
    let added = app.send_json("POST", "/v1/ssh/keys", &[("authorization", &phone)], &json!({"key": key})).await;
    assert_eq!(added.status, 200, "{}", added.text());
    let by_hand = "no-pty ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl again";
    let mut text = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    text.push_str(&format!("{by_hand}\n"));
    std::fs::write(home.join(".ssh/authorized_keys"), text).unwrap();
    std::fs::write(home.join(".ssh/authorized_keys2"), format!("{key}\n")).unwrap();

    // The phone is removed from the account: the guichet's order gives the place a new secret, another phone pairs.
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
    let mine: Value = again.json();
    assert_eq!(mine["reopened"], true);
    let left = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    assert_eq!(left, "# kept\n\n", "every key went, the comment stayed");
    assert!(!std::fs::read_to_string(home.join(".ssh/authorized_keys2")).unwrap_or_default().contains("ssh-ed25519"));
    assert_eq!(mode(&home.join(".ssh/authorized_keys")), 0o600);
    let tablet = format!("Bearer {}", mine["bridgeKey"].as_str().unwrap());
    let status = app.get("/v1/ssh", &[("authorization", &tablet)]).await.json();
    assert_eq!((status["enabled"].clone(), status["keys"].clone()), (json!(true), json!([])));
    assert_eq!(app.get("/v1/ssh", &[("authorization", &phone)]).await.status, 401); // the phone's key: gone too
    untouched(&decoy);
}

/// The same reopen on a place whose home the config doesn't give: no SSH keys shown, added nor removed anywhere, and
/// HOME's never read; the reopen itself goes through.
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
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl x@laptop";
    assert_eq!(app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": key})).await.status, 403);
    let fingerprint = "SHA256:AAAA";
    assert_eq!(app.request("DELETE", &format!("/v1/ssh/keys?fingerprint={fingerprint}"), &h, None).await.status, 403);
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
    assert!(!dir.path().join("home").exists());
    untouched(&decoy);
}
