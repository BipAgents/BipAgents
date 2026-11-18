//! drives.rs against a fake Google Drive, Graph and Dropbox (one local server answering all three).

use super::*;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Uri};
use std::sync::{Arc, Mutex};

/// What the fake server saw: (method, path, authorization).
type Seen = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

fn graph_item(id: &str) -> Option<Value> {
    Some(match id {
        "ROOT" => json!({"id": "ROOT", "name": "root", "folder": {}}),
        "D" => json!({"id": "D", "name": "Docs", "folder": {}, "parentReference": {"id": "ROOT"}}),
        "P" => json!({"id": "P", "name": "Privé", "folder": {}, "parentReference": {"id": "ROOT"}}),
        "S" => json!({"id": "S", "name": "Factures", "folder": {}, "parentReference": {"id": "D"}}),
        "F1" => json!({"id": "F1", "name": "notes.txt", "size": 11, "file": {"mimeType": "text/plain"},
                       "lastModifiedDateTime": "2026-11-02T10:00:00Z", "parentReference": {"id": "D"}}),
        "F2" => {
            json!({"id": "F2", "name": "deep.txt", "size": 9, "file": {"mimeType": "text/plain"}, "parentReference": {"id": "S"}})
        }
        "X" => {
            json!({"id": "X", "name": "secret.txt", "size": 6, "file": {"mimeType": "text/plain"}, "parentReference": {"id": "P"}})
        }
        _ => return None,
    })
}

fn dropbox_item(id: &str) -> Option<Value> {
    Some(match id {
        "id:w" => json!({".tag": "folder", "id": "id:w", "name": "Work", "path_lower": "/work"}),
        "id:f" => json!({".tag": "file", "id": "id:f", "name": "plan.txt", "size": 4, "path_lower": "/work/plan.txt"}),
        // same start, another folder: never inside /work
        "id:z" => json!({".tag": "file", "id": "id:z", "name": "x.txt", "size": 4, "path_lower": "/workshop/x.txt"}),
        _ => return None,
    })
}

fn google_item(id: &str) -> Option<Value> {
    Some(match id {
        "gf" => {
            json!({"id": "gf", "name": "Maison", "mimeType": "application/vnd.google-apps.folder", "parents": ["groot"]})
        }
        "gd" => {
            json!({"id": "gd", "name": "Bail", "mimeType": "application/vnd.google-apps.document", "parents": ["gf"]})
        }
        _ => return None,
    })
}

fn reply(status: u16, body: Value) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn raw(body: &'static str) -> axum::response::Response {
    axum::response::Response::builder().body(Body::from(body)).unwrap()
}

async fn fake(seen: Seen, method: Method, uri: Uri, headers: HeaderMap, body: String) -> axum::response::Response {
    let path = uri.path().to_string();
    let query: HashMap<String, String> = uri.query().map(connections::callback_query).unwrap_or_default();
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
    seen.lock().unwrap().push((method.to_string(), path.clone(), auth.clone()));
    if path == "/token" {
        let form = connections::callback_query(&body);
        return match (form.get("grant_type").map(String::as_str), form.get("code"), form.get("refresh_token")) {
            (Some("authorization_code"), Some(code), _) if code == "good" && form.contains_key("code_verifier") => {
                reply(200, json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600}))
            }
            (Some("refresh_token"), _, Some(r)) if r == "rt-1" => {
                reply(200, json!({"access_token": "at-2", "expires_in": 3600}))
            }
            _ => reply(400, json!({"error": "invalid_grant"})),
        };
    }
    if !matches!(auth.as_deref(), Some("Bearer at-1") | Some("Bearer at-2")) {
        return reply(401, json!({"error": "no token"}));
    }
    // Graph
    if path == "/me" {
        return reply(200, json!({"mail": "a@example.org"}));
    }
    if path == "/me/drive/root/children" {
        return reply(200, json!({"value": [graph_item("D"), graph_item("P"), graph_item("F1")]}));
    }
    if let Some(rest) = path.strip_prefix("/me/drive/items/") {
        let mut parts = rest.splitn(2, '/');
        let id = parts.next().unwrap_or_default();
        return match parts.next() {
            None => graph_item(id).map(|v| reply(200, v)).unwrap_or_else(|| reply(404, json!({}))),
            Some("children") => {
                let kids: Vec<Value> = ["F1", "F2", "S", "X", "D", "P"]
                    .iter()
                    .filter_map(|k| graph_item(k))
                    .filter(|v| v.pointer("/parentReference/id").and_then(Value::as_str) == Some(id))
                    .collect();
                reply(200, json!({"value": kids}))
            }
            Some("content") => match id {
                "F1" => raw("hello notes"),
                "F2" => raw("deep text"),
                "X" => raw("secret"),
                _ => reply(404, json!({})),
            },
            // Graph's search may answer items from elsewhere in the drive: the bridge checks them again
            Some(s) if s.starts_with("search(") => reply(200, json!({"value": [graph_item("F2"), graph_item("X")]})),
            _ => reply(404, json!({})),
        };
    }
    // Dropbox
    if path == "/dbx/files/get_metadata" {
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        return dropbox_item(v["path"].as_str().unwrap_or_default())
            .map(|v| reply(200, v))
            .unwrap_or_else(|| reply(409, json!({"error_summary": "path/not_found"})));
    }
    if path == "/dbx/files/download" {
        let arg: Value =
            serde_json::from_str(headers.get("dropbox-api-arg").and_then(|v| v.to_str().ok()).unwrap_or("{}")).unwrap();
        return if arg["path"] == "id:f" { raw("plan") } else { reply(409, json!({})) };
    }
    if path == "/dbx/users/get_current_account" {
        return reply(200, json!({"email": "d@example.org"}));
    }
    // Google
    if path == "/g/about" {
        return reply(200, json!({"user": {"emailAddress": "g@example.org"}}));
    }
    if path == "/g/files/gd/export" {
        return if query.get("mimeType").map(String::as_str) == Some("text/plain") {
            raw("Le bail commence le 1er mars.")
        } else {
            reply(400, json!({}))
        };
    }
    if let Some(id) = path.strip_prefix("/g/files/") {
        return google_item(id).map(|v| reply(200, v)).unwrap_or_else(|| reply(404, json!({})));
    }
    reply(404, json!({"path": path}))
}

async fn serve() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: String| {
        let s = s.clone();
        async move { fake(s, method, uri, headers, body).await }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), seen)
}

fn setup(base: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::with_key("k");
    config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
    for name in ["vie", "kerfy"] {
        let uploads = dir.path().join(format!("uploads/{name}"));
        let agent = AgentConfig {
            name: name.into(),
            upload_dir_host: Some(uploads.to_string_lossy().into_owned()),
            upload_dir_container: Some("/fichiers".into()),
            ..AgentConfig::default()
        };
        config.agents.insert(name.into(), agent);
    }
    let table = json!({
        "enabled": true,
        "onedrive": {"client_id": "ms-1", "token_url": format!("{base}/token"), "api_url": base, "auth_url": "https://login.example/authorize"},
        "dropbox": {"client_id": "db-1", "token_url": format!("{base}/token"), "api_url": format!("{base}/dbx")},
        "google_drive": {"client_id": "g-1", "redirect_uri": "com.googleusercontent.apps.g-1:/oauth2redirect",
                         "token_url": format!("{base}/token"), "api_url": format!("{base}/g")},
    });
    config.drives = Settings::from_table(&table);
    (dir, config)
}

fn args(v: Value) -> Dict {
    serde_json::from_value(v).unwrap()
}

fn text(d: &Dict) -> String {
    d["content"][0]["text"].as_str().unwrap().to_string()
}

/// Connected through the real start + callback, with the app's callback address.
async fn connect(config: &Config, service: &str, agents: &[&str], extra: &str) -> Value {
    let agents: Vec<String> = agents.iter().map(|a| a.to_string()).collect();
    let started = start(config, service, &agents).unwrap();
    let state = started["state"].as_str().unwrap();
    finish(config, &format!("bipagents://oauth/callback?code=good&state={state}{extra}")).await.unwrap()
}

#[test]
fn off_by_default_and_a_client_needs_its_id() {
    assert_eq!(Settings::from_table(&json!({})), Settings::default());
    let s = Settings::from_table(
        &json!({"enabled": true, "onedrive": {"client_id": " ms "}, "dropbox": {"redirect_uri": "x"}}),
    );
    assert!(s.enabled);
    assert_eq!(s.clients.keys().collect::<Vec<_>>(), ["onedrive"]);
    assert_eq!(s.clients["onedrive"].client_id, "ms");
    assert_eq!(s.clients["onedrive"].redirect_uri, connections::APP_REDIRECT);
    assert!(s.clients["onedrive"].client_secret.is_none());
}

#[test]
fn the_sign_in_addresses() {
    let (_dir, config) = setup("http://127.0.0.1:9");
    let google = endpoints(&config, Provider::Google).unwrap();
    let q = connections::callback_query(&authorize_url(&google, "st", "verifier-verifier-verifier-verifier-verifier"));
    // the Picker in the sign-in, drive.file alone, PKCE, a renewal
    assert_eq!(q["scope"], "https://www.googleapis.com/auth/drive.file");
    assert_eq!(q["trigger_onepick"], "true");
    assert_eq!(q["allow_folder_selection"], "true");
    assert_eq!(q["prompt"], "consent");
    assert_eq!(q["access_type"], "offline");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["redirect_uri"], "com.googleusercontent.apps.g-1:/oauth2redirect");
    let ms = endpoints(&config, Provider::Microsoft).unwrap();
    let url = authorize_url(&ms, "st", "v");
    assert!(url.starts_with("https://login.example/authorize?"));
    assert_eq!(connections::callback_query(&url)["scope"], "Files.Read offline_access User.Read");
    assert_eq!(ms.client.auth_method, "none");
    let dbx = endpoints(&config, Provider::Dropbox).unwrap();
    assert_eq!(connections::callback_query(&authorize_url(&dbx, "s", "v"))["token_access_type"], "offline");
    // Dropbox's downloads go to its content address (here the same fake)
    assert!(dbx.content.ends_with("/dbx"));
}

#[test]
fn references_and_pages() {
    assert_eq!(parse_reference("dropbox:id:abc"), Some((Provider::Dropbox, "id:abc".into())));
    assert_eq!(parse_reference(&reference(Provider::Microsoft, "01AB")), Some((Provider::Microsoft, "01AB".into())));
    assert_eq!(parse_reference("icloud:x"), None);
    assert_eq!(parse_reference("onedrive:"), None);
    let long = "é".repeat(PAGE_CHARS + 10);
    assert_eq!(page_of(&long, 1).1, 2);
    assert_eq!(page_of(&long, 2).0.chars().count(), 10);
    assert_eq!(page_of(&long, 9).0.chars().count(), 10, "past the end: the last page");
    assert_eq!(page_of("", 1), (String::new(), 1));
}

#[test]
fn the_items_of_the_three_services() {
    let g = item_of(Provider::Google, &google_item("gf").unwrap());
    assert!(g.folder && g.parents == ["groot"]);
    let m = item_of(Provider::Microsoft, &graph_item("F1").unwrap());
    assert!(!m.folder && m.size == Some(11) && m.parents == ["D"] && m.mime == "text/plain");
    let d = item_of(Provider::Dropbox, &dropbox_item("id:w").unwrap());
    assert!(d.folder && d.path == "/work");
    assert_eq!(google_export("application/vnd.google-apps.spreadsheet", true), Some(("text/csv", "csv")));
    assert_eq!(google_export("application/vnd.google-apps.document", false).unwrap().1, "docx");
    assert_eq!(google_export("application/pdf", true), None);
}

#[tokio::test]
async fn onedrive_folder_by_folder() {
    let (base, seen) = serve().await;
    let (_dir, config) = setup(&base);
    // nothing offered before a folder is given
    assert!(!offered(&config, "vie"));
    let shown = connect(&config, "onedrive", &["vie"], "").await;
    assert_eq!(shown["state"], "connected");
    assert_eq!(shown["account"], "a@example.org");
    assert!(!offered(&config, "vie"), "connected, no folder yet");
    // the app browses the root, then chooses Docs for vie
    let root = browse(&config, "onedrive", None).await.unwrap();
    let names: Vec<&str> = root["folders"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Docs", "Privé"], "folders only");
    let wrong = set_folders(&config, "onedrive", &[Chosen { id: "F1".into(), agents: vec!["vie".into()] }]).await;
    assert_eq!(wrong.unwrap_err().status.as_u16(), 422, "a file is not a folder");
    let unknown = set_folders(&config, "onedrive", &[Chosen { id: "D".into(), agents: vec!["nobody".into()] }]).await;
    assert_eq!(unknown.unwrap_err().status.as_u16(), 422);
    let shown =
        set_folders(&config, "onedrive", &[Chosen { id: "D".into(), agents: vec!["vie".into()] }]).await.unwrap();
    assert_eq!(shown["folders"][0]["name"], "Docs");
    assert!(offered(&config, "vie") && !offered(&config, "kerfy"));

    let vie = config.agents["vie"].clone();
    let folders = text(&call(&config, &vie, "drive_folders", &Dict::new()).await);
    assert!(folders.contains("Docs (OneDrive) — onedrive:D") && !folders.contains("Privé"), "{folders}");
    let listed = text(&call(&config, &vie, "drive_list", &args(json!({"folder": "onedrive:D"}))).await);
    assert!(
        listed.contains("notes.txt (1 kB, 2026-11-02) — onedrive:F1") && listed.contains("[folder] Factures"),
        "{listed}"
    );
    // a sub-folder is inside; the private folder is not
    let deep = call(&config, &vie, "drive_list", &args(json!({"folder": "onedrive:S"}))).await;
    assert!(text(&deep).contains("deep.txt"), "{}", text(&deep));
    let private = call(&config, &vie, "drive_list", &args(json!({"folder": "onedrive:P"}))).await;
    assert_eq!(private["isError"], true);
    // search: the hit outside Docs is dropped
    let found = text(&call(&config, &vie, "drive_search", &args(json!({"words": "it's"}))).await);
    assert!(found.contains("deep.txt") && !found.contains("secret"), "{found}");
    // read: text between the markers; outside: refused, and never downloaded
    let read = text(&call(&config, &vie, "drive_read", &args(json!({"file": "onedrive:F2"}))).await);
    assert!(read.starts_with(UNTRUSTED) && read.contains("deep text") && read.contains("page 1/1"), "{read}");
    let refused = call(&config, &vie, "drive_read", &args(json!({"file": "onedrive:X"}))).await;
    assert_eq!(refused["isError"], true);
    assert!(!seen.lock().unwrap().iter().any(|(_, p, _)| p == "/me/drive/items/X/content"));
    // save: a copy in its files, as an upload
    let saved = text(&call(&config, &vie, "drive_save", &args(json!({"file": "onedrive:F1"}))).await);
    let path = saved.strip_prefix("Copied: ").unwrap_or_else(|| panic!("{saved}"));
    assert!(path.starts_with("/fichiers/") && path.ends_with("-notes.txt"), "{path}");
    let host = std::path::Path::new(config.agents["vie"].upload_dir_host.as_deref().unwrap());
    let month = path.trim_start_matches("/fichiers/");
    assert_eq!(std::fs::read_to_string(host.join(month)).unwrap(), "hello notes");
    // the other agent: nothing
    let kerfy = config.agents["kerfy"].clone();
    let none = call(&config, &kerfy, "drive_read", &args(json!({"file": "onedrive:F1"}))).await;
    assert_eq!(none["isError"], true);
    // the token stays on the bridge: never in a tool's answer, never in the listing
    let listing = listing(&config).unwrap().to_string();
    assert!(!listing.contains("at-1") && !listing.contains("rt-1"));
    assert!(!read.contains("at-1") && !saved.contains("at-1"));
    // disconnected: forgotten, no tools
    let gone = disconnect(&config, "onedrive").await.unwrap();
    assert_eq!(gone["revoked"], false, "Microsoft has no revocation for a delegated grant");
    assert!(!offered(&config, "vie"));
}

#[tokio::test]
async fn dropbox_by_path_never_a_look_alike_folder() {
    let (base, _seen) = serve().await;
    let (_dir, config) = setup(&base);
    let shown = connect(&config, "dropbox", &[], "").await;
    assert_eq!(shown["account"], "d@example.org");
    set_folders(&config, "dropbox", &[Chosen { id: "id:w".into(), agents: vec!["vie".into()] }]).await.unwrap();
    let vie = config.agents["vie"].clone();
    let read = text(&call(&config, &vie, "drive_read", &args(json!({"file": "dropbox:id:f"}))).await);
    assert!(read.contains("plan"), "{read}");
    let other = call(&config, &vie, "drive_read", &args(json!({"file": "dropbox:id:z"}))).await;
    assert_eq!(other["isError"], true, "/workshop is not inside /work: {}", text(&other));
}

#[tokio::test]
async fn google_keeps_what_the_picker_gave_and_reads_its_documents_as_text() {
    let (base, _seen) = serve().await;
    let (_dir, config) = setup(&base);
    let shown = connect(&config, "google_drive", &["vie"], "&picked_file_ids=gf,unknown").await;
    assert_eq!(shown["account"], "g@example.org");
    let folders = shown["folders"].as_array().unwrap();
    assert_eq!(folders.len(), 1, "an item Google doesn't show is left out: {folders:?}");
    assert_eq!(folders[0]["agents"], json!(["vie"]));
    assert_eq!(folders[0]["file"], false);
    let vie = config.agents["vie"].clone();
    let read = text(&call(&config, &vie, "drive_read", &args(json!({"file": "google_drive:gd"}))).await);
    assert!(read.contains("Le bail commence le 1er mars."), "{read}");
    // only what was picked can be chosen; Google has no folder browsing here
    let not_picked = set_folders(&config, "google_drive", &[Chosen { id: "gd".into(), agents: vec![] }]).await;
    assert_eq!(not_picked.unwrap_err().status.as_u16(), 422);
    assert_eq!(browse(&config, "google_drive", None).await.unwrap_err().status.as_u16(), 422);
}

#[tokio::test]
async fn an_ending_token_is_renewed_and_a_refused_renewal_ends_the_connection() {
    let (base, seen) = serve().await;
    let (_dir, config) = setup(&base);
    connect(&config, "onedrive", &[], "").await;
    set_folders(&config, "onedrive", &[Chosen { id: "D".into(), agents: vec!["vie".into()] }]).await.unwrap();
    let age = |refresh: &str| {
        let store = connections::store_of(&config);
        let mut stored = store.read().unwrap();
        let link = stored.drives.get_mut("onedrive").unwrap();
        link.tokens.expires_at = Some(py::time_time() + 10.0);
        link.tokens.refresh_token = Some(refresh.into());
        store.write(&stored).unwrap();
    };
    age("rt-1");
    let vie = config.agents["vie"].clone();
    let listed = call(&config, &vie, "drive_list", &args(json!({"folder": "onedrive:D"}))).await;
    assert_eq!(listed["isError"], false, "{}", text(&listed));
    assert!(seen
        .lock()
        .unwrap()
        .iter()
        .any(|(_, p, a)| p == "/me/drive/items/D/children" && a.as_deref() == Some("Bearer at-2")));
    age("revoked");
    let failed = call(&config, &vie, "drive_list", &args(json!({"folder": "onedrive:D"}))).await;
    assert_eq!(failed["isError"], true);
    assert_eq!(listing(&config).unwrap()["drives"][1]["state"], "expired");
    assert!(!offered(&config, "vie"), "an ended connection gives no tools");
}

#[tokio::test]
async fn off_means_no_routes_no_tools() {
    let (base, _seen) = serve().await;
    let (_dir, mut config) = setup(&base);
    connect(&config, "onedrive", &[], "").await;
    set_folders(&config, "onedrive", &[Chosen { id: "D".into(), agents: vec!["vie".into()] }]).await.unwrap();
    config.drives.enabled = false;
    assert!(!offered(&config, "vie"));
    assert_eq!(start(&config, "onedrive", &[]).unwrap_err().status.as_u16(), 404);
    assert_eq!(finish(&config, "bipagents://oauth/callback?code=x&state=y").await.unwrap_err().status.as_u16(), 404);
    config.drives.enabled = true;
    // a denied or replayed sign-in
    assert_eq!(
        finish(&config, "bipagents://oauth/callback?error=access_denied").await.unwrap_err().status.as_u16(),
        400
    );
    assert_eq!(
        finish(&config, "bipagents://oauth/callback?code=good&state=never").await.unwrap_err().status.as_u16(),
        400
    );
    // no client for a service: 409; an unknown service: 404
    config.drives.clients.shift_remove("dropbox");
    assert_eq!(start(&config, "dropbox", &[]).unwrap_err().status.as_u16(), 409);
    assert_eq!(start(&config, "icloud", &[]).unwrap_err().status.as_u16(), 404);
}
