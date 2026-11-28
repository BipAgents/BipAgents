//! The web console is gone (BipAgents 1.1, Alex 27/11 « Retirer partout », src/console_removed.rs): no page, no
//! `/console/…` and no `/v1/console/…` route (an app of before 1.1 gets the plain 404 of any path no route serves,
//! which it reads as « this bridge has no web console »), and the browsers it let in are revoked at the first start:
//! `<data>/console.json` deleted, their cookie no longer standing for the bridge key. The phone's bridge key and the
//! devices' own keys keep working.
//!
//! Only HTTP and files here (no call into the new code): the same file runs against the bridge of before, where it
//! fails (the red run of 28/11).

mod common;

use common::{auth, config, TestApp, Urls};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The session cookie of a browser approved in the app (a 1.0 console).
const TOKEN: &str = "an-approved-browser-session-token-0123456789abcdef";

fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

/// console.json as the 1.0 bridge wrote it (console.rs): a browser logged in for 30 days, a one-time link and an
/// approved pairing still waiting for its browser.
fn console_file() -> Value {
    let now = now();
    let mut sessions = Map::new();
    sessions.insert(
        digest(TOKEN),
        json!({"id": "a1b2c3d4e5f6", "name": "PC du salon", "created": now, "seen": now,
               "expires": now + 30.0 * 86400.0}),
    );
    let mut links = Map::new();
    links.insert(digest("a-one-time-link-code"), json!(now + 600.0));
    let mut pairings = Map::new();
    pairings
        .insert("123456".into(), json!({"wait": digest("the-wait-secret"), "expires": now + 300.0, "approved": "Mac"}));
    json!({"sessions": sessions, "links": links, "pairings": pairings})
}

#[tokio::test]
async fn the_web_console_is_gone_and_its_browsers_are_revoked_at_the_first_start() {
    let dir = tempfile::tempdir().unwrap();
    let data = std::path::PathBuf::from(config(dir.path(), &Urls::default()).data_dir());
    let (k, v) = auth();

    // A bridge of before: the iPad got a key of its own from the phone.
    let first = TestApp::new(config(dir.path(), &Urls::default()));
    let minted = first.send_json("POST", "/v1/device-keys", &[(k, &v)], &json!({"name": "iPad"})).await;
    assert_eq!(minted.status, 200, "{}", minted.text());
    let device_key = format!("Bearer {}", minted.json()["key"].as_str().unwrap());
    first.shutdown().await;
    let device_keys = std::fs::read_to_string(data.join("device-keys.json")).unwrap();

    // What its web console left: a browser approved in the app, and a write cut short.
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("console.json"), serde_json::to_vec(&console_file()).unwrap()).unwrap();
    std::fs::write(data.join("console.json.tmp"), b"{\"sessions\": {").unwrap();

    // The new bridge starts.
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    assert!(!data.join("console.json").exists(), "the console's sessions are deleted at the first start");
    assert!(!data.join("console.json.tmp").exists());

    // The approved browser's cookie and header stand for nothing any more: the 401 of no key.
    let cookie = format!("bip_console={TOKEN}");
    let browser = [("x-bip-console", "1"), ("cookie", cookie.as_str())];
    for (method, path) in [("GET", "/v1/agents"), ("GET", "/v1/vault"), ("POST", "/v1/vault/abc123/reveal")] {
        let said = app.request(method, path, &browser, None).await;
        assert_eq!(said.status, 401, "{method} {path} with a console's cookie: {}", said.text());
    }
    let mint = app.send_json("POST", "/v1/device-keys", &browser, &json!({"name": "PC"})).await;
    assert_eq!(mint.status, 401, "a console's cookie mints no device key: {}", mint.text());
    // A browser left open keeps asking: never counted as a wrong key (its address is not blocked for the phone).
    for _ in 0..=app.app.state.guard.limit {
        assert_eq!(app.get("/v1/agents", &browser).await.status, 401);
    }

    // The phone's key and the iPad's own key are untouched.
    assert_eq!(std::fs::read_to_string(data.join("device-keys.json")).unwrap(), device_keys);
    assert_eq!(app.get("/v1/agents", &[(k, &v)]).await.status, 200);
    assert_eq!(app.get("/v1/agents", &[("authorization", &device_key)]).await.status, 200);
    let listed = app.get("/v1/device-keys", &[(k, &v)]).await.json();
    let names: Vec<&str> = listed["devices"].as_array().unwrap().iter().filter_map(|d| d["name"].as_str()).collect();
    assert_eq!(names, ["iPad"]);

    // No page and no login: the 404 of any path no route serves.
    let not_found = json!({"detail": "Not Found"});
    for (method, path) in [
        ("GET", "/console"),
        ("GET", "/console/"),
        ("GET", "/console/static/app.js"),
        ("GET", "/console/static/style.css"),
        ("GET", "/console/static/pixel.js"),
        ("GET", "/console/api/me"),
        ("POST", "/console/api/login"),
        ("POST", "/console/api/pairing"),
        ("POST", "/console/api/pairing/wait"),
        ("POST", "/console/api/logout"),
    ] {
        for headers in [&[][..], &browser[..]] {
            let said = app.request(method, path, headers, None).await;
            assert_eq!((said.status.as_u16(), said.json()), (404, not_found.clone()), "{method} {path}");
        }
    }

    // The app's console calls (approve a code, list the browsers, log one out): a plain 404, which the 1.0 apps read
    // as « no web console on this bridge » (iOS consoleDevices → nil, approveConsole → false; Android orNull404).
    for (method, path, body) in [
        ("POST", "/v1/console/pairings/123456", Some(json!({"name": "Mac"}))),
        ("GET", "/v1/console/devices", None),
        ("DELETE", "/v1/console/devices/a1b2c3d4e5f6", None),
    ] {
        for headers in [vec![(k, v.as_str())], vec![("authorization", device_key.as_str())], vec![]] {
            let said = match &body {
                Some(body) => app.send_json(method, path, &headers, body).await,
                None => app.request(method, path, &headers, None).await,
            };
            assert_eq!((said.status.as_u16(), said.json()), (404, not_found.clone()), "{method} {path}");
        }
    }
    app.shutdown().await;

    // Every later start: nothing left to do, the keys still the same (the iPad's « seen » just written included).
    let device_keys = std::fs::read_to_string(data.join("device-keys.json")).unwrap();
    let again = TestApp::new(config(dir.path(), &Urls::default()));
    assert!(!data.join("console.json").exists());
    assert_eq!(std::fs::read_to_string(data.join("device-keys.json")).unwrap(), device_keys);
    assert_eq!(again.get("/v1/agents", &[("authorization", &device_key)]).await.status, 200);
    again.shutdown().await;
}
