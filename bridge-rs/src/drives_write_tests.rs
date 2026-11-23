//! drives_write.rs (and the writers, the whole drive and the write right in drives.rs) against a fake Dropbox, Google
//! Drive and Graph that keep what is written. The person's card and level: bridge/tests_blackbox/test_drives.py.

use super::*;
use crate::connections::{self, Tokens};
use crate::drives::{Link, Pick};
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Uri};
use axum::Router;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct World {
    dropbox: HashMap<String, Value>,
    google: HashMap<String, Value>,
    graph: HashMap<String, Value>,
    bytes: HashMap<String, Vec<u8>>,
    /// Google's upload sessions: (the file it updates, or the metadata of a new one).
    sessions: HashMap<String, (Option<String>, Value)>,
    /// (method, path, the Dropbox-API-Arg as sent)
    seen: Vec<(String, String, Option<String>)>,
    kept_forever: Vec<String>,
    made: u32,
    /// OneDrive's upload sessions: (the folder, the name).
    ms_sessions: HashMap<String, (String, String)>,
    /// OneDrive answers 200 (an update) where a new file was asked.
    upload_200: bool,
    /// Dropbox's shared links, by `path_lower`.
    links: Vec<String>,
}

type W = Arc<Mutex<World>>;

fn world() -> World {
    let mut w = World::default();
    for v in [
        json!({".tag": "folder", "id": "id:w", "name": "Work", "path_lower": "/work", "path_display": "/Work"}),
        json!({".tag": "file", "id": "id:f", "name": "plan.txt", "size": 4, "path_lower": "/work/plan.txt",
               "path_display": "/Work/plan.txt", "rev": "r1"}),
        // a look-alike path: never inside /work
        json!({".tag": "folder", "id": "id:ws", "name": "Workshop", "path_lower": "/workshop", "path_display": "/Workshop"}),
        json!({".tag": "file", "id": "id:r", "name": "racine.txt", "size": 4, "path_lower": "/racine.txt", "rev": "r1"}),
        // someone else's folder added to this Dropbox, and its content
        json!({".tag": "folder", "id": "id:s", "name": "Famille", "path_lower": "/work/famille", "path_display": "/Work/Famille",
               "sharing_info": {"read_only": false, "shared_folder_id": "9"}}),
        json!({".tag": "file", "id": "id:sf", "name": "x.txt", "path_lower": "/work/famille/x.txt", "rev": "r1",
               "sharing_info": {"read_only": false, "parent_shared_folder_id": "9"}}),
        // a shared folder never added: no path in the person's space
        json!({".tag": "folder", "id": "id:away", "name": "Ailleurs", "sharing_info": {"shared_folder_id": "7"}}),
    ] {
        w.dropbox.insert(v["id"].as_str().unwrap().into(), v);
    }
    let folder = drives::GOOGLE_FOLDER_MIME;
    for v in [
        json!({"id": "gf", "name": "Maison", "mimeType": folder, "parents": ["groot"]}),
        json!({"id": "gp", "name": "Facture.pdf", "mimeType": "application/pdf", "parents": ["gx"], "headRevisionId": "h1"}),
        json!({"id": "gd", "name": "Bail", "mimeType": "application/vnd.google-apps.document", "parents": ["gf"]}),
        json!({"id": "gin", "name": "notes.txt", "mimeType": "text/plain", "parents": ["gf"], "headRevisionId": "h1"}),
        // only inside a picked folder: Google may not let the app change it (drive.file)
        json!({"id": "gnp", "name": "pas choisi.txt", "mimeType": "text/plain", "parents": ["gf"], "headRevisionId": "h1"}),
        json!({"id": "gsh", "name": "Famille", "mimeType": folder, "parents": ["groot"], "shared": true}),
        json!({"id": "gsub", "name": "Photos", "mimeType": folder, "parents": ["gsh"]}),
        json!({"id": "gdr", "name": "Équipe", "mimeType": folder, "parents": ["0Ateam"], "driveId": "0Ateam"}),
        json!({"id": "gcut", "name": "Raccourci", "mimeType": "application/vnd.google-apps.shortcut", "parents": ["gf"]}),
    ] {
        w.google.insert(v["id"].as_str().unwrap().into(), v);
    }
    for v in [
        json!({"id": "ROOT", "name": "root", "folder": {}, "root": {}}),
        json!({"id": "D", "name": "Docs", "folder": {}, "parentReference": {"id": "ROOT"}}),
        json!({"id": "F1", "name": "notes.txt", "file": {"mimeType": "text/plain"}, "parentReference": {"id": "D"},
               "eTag": "e1", "cTag": "c1"}),
        json!({"id": "TOP", "name": "top.txt", "file": {"mimeType": "text/plain"}, "parentReference": {"id": "ROOT"},
               "eTag": "e1", "cTag": "c1"}),
        json!({"id": "SH", "name": "Partagé", "folder": {}, "parentReference": {"id": "ROOT"}, "shared": {"scope": "users"}}),
        json!({"id": "SHF", "name": "in.txt", "file": {}, "parentReference": {"id": "SH"}, "eTag": "e1", "cTag": "c1"}),
        json!({"id": "R", "name": "D'un autre", "remoteItem": {"id": "ELSE", "folder": {}}, "parentReference": {"id": "ROOT"}}),
    ] {
        w.graph.insert(v["id"].as_str().unwrap().into(), v);
    }
    w
}

fn reply(status: u16, body: Value) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn conflict() -> axum::response::Response {
    reply(409, json!({"error_summary": "path/conflict/file/.."}))
}

async fn fake(w: W, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> axum::response::Response {
    let path = uri.path().to_string();
    let query: HashMap<String, String> = uri.query().map(connections::callback_query).unwrap_or_default();
    let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let auth = header("authorization").unwrap_or_default();
    let mut w = w.lock().unwrap();
    w.seen.push((method.to_string(), path.clone(), header("dropbox-api-arg")));
    // OneDrive's upload session address: never with the bearer (Microsoft's rule)
    if let Some(session) = path.strip_prefix("/ms-upload/") {
        assert!(auth.is_empty(), "the bearer sent to the upload address");
        let n = body.len();
        assert_eq!(header("content-range"), Some(format!("bytes 0-{}/{n}", n.saturating_sub(1))));
        let Some((parent, name)) = w.ms_sessions.remove(session) else { return reply(404, json!({})) };
        w.made += 1;
        let id = format!("M{}", w.made);
        let v =
            json!({"id": id, "name": name, "file": {}, "parentReference": {"id": parent}, "eTag": "e1", "cTag": "c1"});
        w.graph.insert(id.clone(), v.clone());
        w.bytes.insert(id, body.to_vec());
        return reply(if w.upload_200 { 200 } else { 201 }, v);
    }
    if path == "/token-aad" {
        // Microsoft refusing a public client's code without a secret (the Entra setting, 23/11)
        return reply(
            401,
            json!({"error": "invalid_client", "error_description":
            "AADSTS7000218: The request body must contain the following parameter: 'client_assertion' or 'client_secret'."}),
        );
    }
    if !auth.starts_with("Bearer at") {
        return reply(401, json!({"error": "no token"}));
    }
    let json_body = || serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let host = header("host").unwrap_or_default();
    // -- Dropbox
    if path == "/dbx/sharing/list_shared_links" {
        if auth == "Bearer at-nolinks" {
            return reply(
                401,
                json!({"error_summary": "missing_scope/", "error": {".tag": "missing_scope", "required_scope": "sharing.read"}}),
            );
        }
        let v = json_body();
        assert_eq!(v["direct_only"], false, "the links above it too");
        let asked = v["path"].as_str().unwrap_or_default().to_lowercase();
        let hits: Vec<Value> = w
            .links
            .iter()
            .filter(|l| asked == **l || asked.starts_with(&format!("{l}/")))
            .map(|l| json!({".tag": "folder", "url": format!("https://www.dropbox.com/s/x{l}"), "path_lower": l}))
            .collect();
        return reply(200, json!({"links": hits, "has_more": false}));
    }
    if path == "/dbx/files/get_metadata" {
        let asked = json_body()["path"].as_str().unwrap_or_default().to_lowercase();
        let found =
            w.dropbox.values().find(|v| v["id"] == asked.as_str() || v["path_lower"] == asked.as_str()).cloned();
        return found.map(|v| reply(200, v)).unwrap_or_else(|| reply(409, json!({"error_summary": "path/not_found/"})));
    }
    if path == "/dbx/files/upload" {
        if auth == "Bearer at-read" {
            return reply(
                401,
                json!({"error_summary": "missing_scope/", "error": {".tag": "missing_scope", "required_scope": "files.content.write"}}),
            );
        }
        let raw = header("dropbox-api-arg").unwrap_or_default();
        assert!(raw.is_ascii(), "Dropbox-API-Arg in ASCII: {raw}");
        let arg: Value = serde_json::from_str(&raw).unwrap();
        let at = arg["path"].as_str().unwrap().to_string();
        let existing = w.dropbox.values().find(|v| v["path_lower"] == at.to_lowercase().as_str()).cloned();
        let id = match (&arg["mode"], existing) {
            (Value::String(m), None) if m == "add" => {
                w.made += 1;
                let id = format!("id:new{}", w.made);
                let name = at.rsplit('/').next().unwrap().to_string();
                let v = json!({".tag": "file", "id": id, "name": name, "path_lower": at.to_lowercase(), "path_display": at,
                               "rev": "r1", "size": body.len()});
                w.dropbox.insert(id.clone(), v);
                id
            }
            (Value::Object(m), Some(old)) if m[".tag"] == "update" && m["update"] == old["rev"] => {
                let id = old["id"].as_str().unwrap().to_string();
                let rev = format!("{}+", old["rev"].as_str().unwrap());
                w.dropbox.get_mut(&id).unwrap()["rev"] = json!(rev);
                id
            }
            _ => return conflict(),
        };
        w.bytes.insert(id.clone(), body.to_vec());
        return reply(200, w.dropbox[&id].clone());
    }
    if path == "/dbx/files/delete_v2" {
        let v = json_body();
        let id = v["path"].as_str().unwrap_or_default().to_string();
        return match w.dropbox.get(&id).cloned() {
            Some(old) if old["rev"] == v["parent_rev"] => {
                w.dropbox.remove(&id);
                reply(200, json!({"metadata": old}))
            }
            Some(_) => conflict(),
            None => reply(409, json!({"error_summary": "path_lookup/not_found/"})),
        };
    }
    if path == "/dbx/files/list_folder" {
        let folder = json_body()["path"].as_str().unwrap_or_default().to_string();
        let kids: Vec<Value> = w
            .dropbox
            .values()
            .filter(|v| {
                v["path_lower"].as_str().is_some_and(|p| p.rsplit_once('/').map(|(f, _)| f) == Some(folder.as_str()))
            })
            .cloned()
            .collect();
        return reply(200, json!({"entries": kids, "has_more": false}));
    }
    // -- Google
    if path == "/g/files" && method == Method::GET {
        let q = query.get("q").cloned().unwrap_or_default();
        let hits: Vec<Value> = w
            .google
            .values()
            .filter(|v| {
                let parent = v["parents"][0].as_str().unwrap_or_default();
                let name = v["name"].as_str().unwrap_or_default();
                q.contains(&format!("'{parent}' in parents"))
                    && q.contains(&format!("name = '{name}'"))
                    && v["trashed"] != true
            })
            .cloned()
            .collect();
        return reply(200, json!({"files": hits}));
    }
    if (path == "/g/files" && method == Method::POST)
        || (path.starts_with("/g/files/") && method == Method::PATCH && query.contains_key("uploadType"))
    {
        assert_eq!(query.get("uploadType").map(String::as_str), Some("resumable"));
        let file = path.strip_prefix("/g/files/").map(str::to_string);
        if file.as_deref() == Some("gnp") {
            return reply(403, json!({"error": {"errors": [{"reason": "appNotAuthorizedToFile"}], "code": 403}}));
        }
        w.made += 1;
        let session = format!("s{}", w.made);
        w.sessions.insert(session.clone(), (file, json_body()));
        return axum::response::Response::builder()
            .status(200)
            .header("location", format!("http://{host}/g/upload/{session}"))
            .body(Body::empty())
            .unwrap();
    }
    if let Some(session) = path.strip_prefix("/g/upload/") {
        let Some((file, metadata)) = w.sessions.remove(session) else { return reply(404, json!({})) };
        let id = match file {
            Some(id) => {
                let old = w.google[&id]["headRevisionId"].as_str().unwrap_or("h0").to_string();
                w.google.get_mut(&id).unwrap()["headRevisionId"] = json!(format!("{old}+"));
                id
            }
            None => {
                w.made += 1;
                let id = format!("gnew{}", w.made);
                let v = json!({"id": id, "name": metadata["name"], "mimeType": metadata["mimeType"], "parents": metadata["parents"],
                               "headRevisionId": "h1"});
                w.google.insert(id.clone(), v);
                id
            }
        };
        w.bytes.insert(id.clone(), body.to_vec());
        return reply(200, w.google[&id].clone());
    }
    if let Some(rest) = path.strip_prefix("/g/files/") {
        if let Some((id, rev)) = rest.split_once("/revisions/") {
            assert_eq!(json_body(), json!({"keepForever": true}));
            w.kept_forever.push(format!("{id}@{rev}"));
            return reply(200, json!({"id": rev, "keepForever": true}));
        }
        if method == Method::PATCH {
            let changes = json_body();
            let Some(v) = w.google.get_mut(rest) else { return reply(404, json!({})) };
            v["trashed"] = changes["trashed"].clone();
            return reply(200, v.clone());
        }
        return w.google.get(rest).cloned().map(|v| reply(200, v)).unwrap_or_else(|| reply(404, json!({})));
    }
    // -- Graph
    if let Some(rest) = path.strip_prefix("/ms/me/drive/items/") {
        let rest = rest.to_string();
        // `{parent}:/{name}:` and `{parent}:/{name}:/createUploadSession`
        if let Some((parent, tail)) = rest.split_once(":/") {
            let (name, content) = match tail.strip_suffix(":/createUploadSession") {
                Some(n) => (n, true),
                None => (tail.trim_end_matches(':'), false),
            };
            let name = url::form_urlencoded::parse(format!("n={name}").as_bytes()).next().unwrap().1.to_string();
            let found = w
                .graph
                .values()
                .find(|v| v.pointer("/parentReference/id") == Some(&json!(parent)) && v["name"] == name.as_str())
                .cloned();
            if !content {
                return found
                    .map(|v| reply(200, v))
                    .unwrap_or_else(|| reply(404, json!({"error": {"code": "itemNotFound"}})));
            }
            assert_eq!(method, Method::POST);
            assert_eq!(json_body()["item"]["@microsoft.graph.conflictBehavior"], "fail", "documented: never replaces");
            if found.is_some() {
                return reply(409, json!({"error": {"code": "nameAlreadyExists"}}));
            }
            w.made += 1;
            let session = format!("u{}", w.made);
            w.ms_sessions.insert(session.clone(), (parent.to_string(), name));
            return reply(200, json!({"uploadUrl": format!("http://{host}/ms-upload/{session}")}));
        }
        let (id, content) = match rest.strip_suffix("/content") {
            Some(id) => (id.to_string(), true),
            None => (rest.clone(), false),
        };
        let id = if id == "root" { "ROOT".to_string() } else { id };
        let Some(item) = w.graph.get(&id).cloned() else {
            return reply(404, json!({"error": {"code": "itemNotFound"}}));
        };
        let matches = header("if-match").is_none_or(|tag| item["eTag"] == tag.as_str());
        if content && method == Method::PUT {
            if !matches {
                return reply(412, json!({"error": {"code": "resourceModified"}}));
            }
            let next = |t: &Value, p: &str| json!(format!("{}+", t.as_str().unwrap_or(p)));
            let (e, c) = (next(&item["eTag"], "e"), next(&item["cTag"], "c"));
            let v = w.graph.get_mut(&id).unwrap();
            v["eTag"] = e;
            v["cTag"] = c;
            let v = v.clone();
            w.bytes.insert(id, body.to_vec());
            return reply(200, v);
        }
        if method == Method::DELETE {
            if !matches {
                return reply(412, json!({"error": {"code": "resourceModified"}}));
            }
            w.graph.remove(&id);
            return axum::response::Response::builder().status(204).body(Body::empty()).unwrap();
        }
        return reply(200, item);
    }
    reply(404, json!({"path": path}))
}

async fn serve() -> (String, W) {
    let w: W = Arc::new(Mutex::new(world()));
    let shared = w.clone();
    let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
        let w = shared.clone();
        async move { fake(w, method, uri, headers, body).await }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), w)
}

fn setup(base: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::with_key("k");
    config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
    for name in ["vie", "kerfy"] {
        let uploads = dir.path().join(format!("uploads/{name}"));
        std::fs::create_dir_all(&uploads).unwrap();
        let agent = AgentConfig {
            name: name.into(),
            upload_dir_host: Some(uploads.to_string_lossy().into_owned()),
            upload_dir_container: Some("/fichiers".into()),
            ..AgentConfig::default()
        };
        config.agents.insert(name.into(), agent);
    }
    config.drives = drives::Settings::from_table(&json!({
        "enabled": true,
        "onedrive": {"client_id": "ms-1", "token_url": format!("{base}/token"), "api_url": format!("{base}/ms")},
        "dropbox": {"client_id": "db-1", "token_url": format!("{base}/token"), "api_url": format!("{base}/dbx")},
        "google_drive": {"client_id": "g-1", "token_url": format!("{base}/token"), "api_url": format!("{base}/g")},
    }));
    (dir, config)
}

fn folder(id: &str, name: &str, path: &str, agents: &[&str], writers: &[&str]) -> Folder {
    Folder {
        id: id.into(),
        name: name.into(),
        path: path.into(),
        agents: agents.iter().map(|a| a.to_string()).collect(),
        writers: writers.iter().map(|a| a.to_string()).collect(),
        root: id == ROOT_ID,
        ..Default::default()
    }
}

/// A connection kept as a sign-in would keep it (`token` "at…" signs the fake's calls; `scope` as granted).
fn keep(config: &Config, provider: Provider, token: &str, scope: &str, folders: Vec<Folder>) {
    let store = connections::store_of(config);
    let mut stored = store.read().unwrap();
    stored.drives.insert(
        provider.id().into(),
        Link {
            tokens: Tokens {
                access_token: token.into(),
                refresh_token: Some("rt".into()),
                scope: Some(scope.into()),
                expires_at: Some(py::time_time() + 3600.0),
            },
            account: Some("a@example.org".into()),
            connected_at: 1.0,
            state: "connected".into(),
            folders,
            ..Default::default()
        },
    );
    store.write(&stored).unwrap();
}

const DBX_WRITE: &str = "account_info.read files.metadata.read files.content.read files.content.write sharing.read";

fn vie(config: &Config) -> AgentConfig {
    config.agents["vie"].clone()
}

fn text_content(text: &str) -> Content {
    Content {
        bytes: text.as_bytes().to_vec(),
        mime: String::new(),
        text: Some(text.into()),
        own_name: None,
        from_page: false,
    }
}

fn said(d: &Dict) -> String {
    d["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

async fn planned(config: &Config, asked: &str, replace: bool, name: &str) -> Result<Plan, String> {
    plan(config, &vie(config), asked, replace, name).await.map_err(|d| said(&d))
}

#[test]
fn a_name_is_one_plain_part_never_a_path() {
    assert_eq!(clean_name("  Compte rendu.md "), Ok("Compte rendu.md".to_string()));
    assert_eq!(clean_name("Récap été 2026"), Ok("Récap été 2026".to_string()));
    for bad in [
        "",
        "   ",
        ".",
        "..",
        "../x.md",
        "a/b.md",
        "a\\b.md",
        ".cache",
        "x.md.",
        "a:b",
        "a*b",
        "a?b",
        "a|b",
        "a\"b",
        "a<b>",
        "x\u{0}y",
        "x\ny",
        "rapport\u{202e}dm.exe",
        &"x".repeat(121),
    ] {
        assert!(clean_name(bad).is_err(), "{bad:?}");
    }
    assert_eq!(name_with_extension("Compte rendu", None), "Compte rendu.md", "a text: markdown");
    assert_eq!(name_with_extension("Devis", Some("devis-v2.pdf")), "Devis.pdf");
    assert_eq!(name_with_extension("Devis.pdf", Some("x.html")), "Devis.pdf");
    assert_eq!(name_with_extension("Sans", Some("README")), "Sans");
}

#[test]
fn the_words_of_the_card_and_of_the_line() {
    assert_eq!(
        card_sentence("fr", false, "Compte rendu.md", "Dropbox › Work"),
        "Enregistrer « Compte rendu.md » dans Dropbox › Work ?"
    );
    assert_eq!(card_sentence("en", true, "a.md", "OneDrive › Docs"), "New version of « a.md » in OneDrive › Docs?");
    assert_eq!(card_sentence("nl", false, "a.md", "Dropbox"), "« a.md » opslaan in Dropbox?");
    assert_eq!(card_sentence("de", false, "a.md", "Dropbox"), "« a.md » in Dropbox speichern?");
    assert_eq!(card_sentence("es", false, "a.md", "Dropbox"), "¿Guardar « a.md » en Dropbox?");
    let file = Content {
        bytes: vec![0; 12 * 1024],
        mime: String::new(),
        text: None,
        own_name: Some("x.pdf".into()),
        from_page: false,
    };
    assert_eq!(card_detail("fr", false, "Dropbox", &file), "Nouveau fichier · 12 ko");
    assert_eq!(
        card_detail("en", true, "Dropbox", &file),
        "The old version stays in Dropbox's history (at least 30 days).\n12 KB"
    );
    let long = text_content(&"mot ".repeat(400));
    assert!(
        card_detail("fr", false, "Dropbox", &long).ends_with("… (2 ko en tout)"),
        "{}",
        card_detail("fr", false, "Dropbox", &long)
    );
    assert_eq!(line_sentence("fr", "a.md", "Dropbox › Work"), "« a.md » enregistré dans Dropbox › Work");
    assert_eq!(size_text("fr", 3 * 1024 * 1024 / 2), "1,5 Mo");
    assert_eq!(size_text("en", 3 * 1024 * 1024 / 2), "1.5 MB");
    assert_eq!(
        ascii_json(&json!({"path": "/\u{c9}t\u{e9}/\u{2713} \u{1f600}.md"})),
        "{\"path\":\"/\\u00c9t\\u00e9/\\u2713 \\ud83d\\ude00.md\"}"
    );
    assert!(same_origin("http://127.0.0.1:9/g/upload/s1", "http://127.0.0.1:9/g"));
    assert!(!same_origin("https://evil.example/upload", "https://www.googleapis.com/upload/drive/v3"));
    assert!(!same_origin("http://127.0.0.1:8/x", "http://127.0.0.1:9/g"));
}

#[test]
fn the_kinds_of_the_card_new_file_not_critical_with_undo_new_version_critical() {
    let create = crate::approval_level::rule_of(KIND_CREATE);
    assert!(!create.critical && create.undo);
    assert!(crate::approval_level::rule_of(KIND_REPLACE).critical);
    let item = |kind: &str| json!({"run_id": "bip-ok-x", "bip_ok": {"kind": kind}}).as_object().cloned().unwrap();
    assert_eq!(crate::approval_level::classify_card(&item(KIND_CREATE)), KIND_CREATE);
    assert_eq!(crate::approval_level::classify_card(&item(KIND_REPLACE)), KIND_REPLACE);
    assert!(crate::person_ok::HOLDS_THE_CALL.contains(&TOOL));
    assert!(!crate::approval_level::offers_always(KIND_CREATE), "no « Toujours »");
}

/// Dropbox asks its write right only once a folder has a writer (or the connection had it); OneDrive asks
/// Files.ReadWrite (Kerfy's Entra app, 23/11); Google stays on drive.file.
#[tokio::test]
async fn the_write_right_is_asked_only_when_a_folder_has_a_writer() {
    let (base, _w) = serve().await;
    let (_dir, config) = setup(&base);
    let scope_of = |config: &Config, service: &str| {
        let started = drives::start(config, service, &[], Pick::Folders, "ios").unwrap();
        connections::callback_query(started["authorize_url"].as_str().unwrap())["scope"].clone()
    };
    assert_eq!(scope_of(&config, "dropbox"), "account_info.read files.metadata.read files.content.read");
    keep(
        &config,
        Provider::Dropbox,
        "at-read",
        "files.content.read",
        vec![folder("id:w", "Work", "/work", &["vie"], &[])],
    );
    assert!(!scope_of(&config, "dropbox").contains("write"), "a reader only");
    keep(
        &config,
        Provider::Dropbox,
        "at-read",
        "files.content.read",
        vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])],
    );
    assert!(scope_of(&config, "dropbox").ends_with(" files.content.write sharing.read"), "a writer: asked, links too");
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie"], &[])]);
    assert!(!scope_of(&config, "dropbox").contains("write"), "no writer any more: « Reconnecter » drops it");
    // OneDrive (review C6): reading until a folder has a writer, as Dropbox
    assert_eq!(scope_of(&config, "onedrive"), "Files.Read offline_access User.Read");
    keep(&config, Provider::Microsoft, "at", "Files.Read", vec![folder("D", "Docs", "", &["vie"], &["vie"])]);
    assert_eq!(scope_of(&config, "onedrive"), "Files.ReadWrite offline_access User.Read");
    let shown = drives::view(Provider::Microsoft, true, drives::read_stored(&config).unwrap().drives.get("onedrive"));
    assert_eq!(shown["writable"], false, "a reading connection: « Reconnecter OneDrive »");
    assert_eq!(scope_of(&config, "google_drive"), "https://www.googleapis.com/auth/drive.file", "never wider");
    // Kerfy's Microsoft client by default (public: code + PKCE, the app's redirect)
    let kerfy = drives::Settings::from_table(&json!({"enabled": true}));
    assert_eq!(kerfy.clients["onedrive"].client_id, "7a6cdda3-8fba-493e-9bf6-dc2c3506365a");
    assert_eq!(kerfy.clients["onedrive"].redirect_uri, "bipagents://oauth/callback");
    assert!(kerfy.clients["onedrive"].client_secret.is_none());
}

#[test]
fn the_listing_says_what_can_be_written_and_keeps_1_0_shapes() {
    let link = |scope: &str, folders: Vec<Folder>| Link {
        tokens: Tokens { access_token: "at".into(), scope: Some(scope.into()), ..Default::default() },
        state: "connected".into(),
        folders,
        ..Default::default()
    };
    let read = link("files.content.read", vec![folder("id:w", "Work", "/work", &["vie"], &[])]);
    let v = drives::view(Provider::Dropbox, true, Some(&read));
    assert_eq!((v["writable"].as_bool(), v["whole_drive"].as_bool()), (Some(false), Some(true)));
    assert_eq!(v["folders"][0], json!({"id": "id:w", "name": "Work", "file": false, "agents": ["vie"]}), "1.0's shape");
    let write = link(DBX_WRITE, vec![drives::Folder::whole(Provider::Dropbox, vec!["vie".into()], vec!["vie".into()])]);
    let v = drives::view(Provider::Dropbox, true, Some(&write));
    assert_eq!(v["writable"], true);
    assert_eq!(
        v["folders"][0],
        json!({"id": "root", "name": "Dropbox", "file": false, "agents": ["vie"], "writers": ["vie"], "root": true})
    );
    let v = drives::view(Provider::Google, true, None);
    assert_eq!((v["writable"].as_bool(), v["whole_drive"].as_bool()), (Some(true), Some(false)));
    let ms = link("https://graph.microsoft.com/Files.ReadWrite https://graph.microsoft.com/User.Read", vec![]);
    assert_eq!(drives::view(Provider::Microsoft, true, Some(&ms))["writable"], true);
    let ms_read = link("https://graph.microsoft.com/Files.Read", vec![]);
    assert_eq!(
        drives::view(Provider::Microsoft, true, Some(&ms_read))["writable"],
        false,
        "a connection made with Files.Read"
    );
    // a missing scope is a right to ask, never the connection's end
    let missing = json!({"error_summary": "missing_scope/", "error": {".tag": "missing_scope", "required_scope": "files.content.write"}});
    assert_eq!(drives::fault_of(401, None, &missing), Fault::MissingScope);
    assert_eq!(drives::fault_of(401, None, &json!({"error_summary": "expired_access_token/"})), Fault::Expired);
}

/// The app's switch: kept when a 1.0 app sends no `writers`, always a reader, the whole drive (Dropbox, OneDrive),
/// never Google's; an agent deleted loses it.
#[tokio::test]
async fn the_writers_and_the_whole_drive_in_the_folders_list() {
    let (base, _w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie", "kerfy"], &[])]);
    let chosen = |v: Value| -> Vec<drives::Chosen> { serde_json::from_value(v).unwrap() };
    let set = drives::set_folders(
        &config,
        "dropbox",
        &chosen(json!([{"id": "id:w", "agents": ["vie", "kerfy"], "writers": ["vie", "nobody"]}])),
    )
    .await
    .unwrap();
    assert_eq!(set["folders"][0]["writers"], json!(["vie"]), "only readers write");
    // a 1.0 app: no `writers`: kept; a reader taken off: its writing too
    let set = drives::set_folders(&config, "dropbox", &chosen(json!([{"id": "id:w", "agents": ["vie", "kerfy"]}])))
        .await
        .unwrap();
    assert_eq!(set["folders"][0]["writers"], json!(["vie"]));
    let set =
        drives::set_folders(&config, "dropbox", &chosen(json!([{"id": "id:w", "agents": ["kerfy"]}]))).await.unwrap();
    assert!(set["folders"][0].get("writers").is_none());
    // the whole Dropbox
    let set = drives::set_folders(
        &config,
        "dropbox",
        &chosen(json!([{"id": "root", "agents": ["vie"], "writers": ["vie"]}])),
    )
    .await
    .unwrap();
    assert_eq!(
        set["folders"],
        json!([{"id": "root", "name": "Dropbox", "file": false, "agents": ["vie"], "writers": ["vie"], "root": true}])
    );
    // never Google's whole Drive
    keep(&config, Provider::Google, "at", "", vec![folder("gf", "Maison", "", &["vie"], &[])]);
    let refused = drives::set_folders(&config, "google_drive", &chosen(json!([{"id": "root", "agents": ["vie"]}])))
        .await
        .unwrap_err();
    assert_eq!(refused.status.as_u16(), 422);
    assert!(format!("{:?}", refused.detail).contains("whole_drive"), "{:?}", refused.detail);
    let set = drives::set_folders(
        &config,
        "google_drive",
        &chosen(json!([{"id": "gf", "agents": ["vie"], "writers": ["vie"]}])),
    )
    .await
    .unwrap();
    assert_eq!(set["folders"][0]["writers"], json!(["vie"]));
    // OneDrive writes too now
    keep(&config, Provider::Microsoft, "at", "Files.ReadWrite", vec![folder("D", "Docs", "", &["vie"], &[])]);
    let set =
        drives::set_folders(&config, "onedrive", &chosen(json!([{"id": "D", "agents": ["vie"], "writers": ["vie"]}])))
            .await
            .unwrap();
    assert_eq!(set["folders"][0]["writers"], json!(["vie"]));
    drives::forget_agent(&config, "vie").await;
    let stored = drives::read_stored(&config).unwrap();
    assert!(stored
        .drives
        .values()
        .flat_map(|l| &l.folders)
        .all(|f| f.writers.is_empty() && f.agents.iter().all(|a| a != "vie")));
}

/// Dropbox: a new file in a folder given for writing, never over one, never outside, never in a shared folder; a
/// new version on the revision read; the whole Dropbox (its root's files too); the write right missing said plainly.
#[tokio::test]
async fn dropbox_writes_a_new_file_and_a_new_version_inside_the_folders_given() {
    let (base, w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])]);
    let plan = planned(&config, "dropbox:id:w", false, "Compte rendu é.md").await.unwrap();
    assert_eq!(plan.place, "Dropbox › Work");
    let made = write(&plan, &text_content("# Réunion")).await.unwrap();
    assert_eq!(made.path, "/work/compte rendu é.md");
    assert_eq!(w.lock().unwrap().bytes[&made.id], "# Réunion".as_bytes());
    // the same name again: refused before any card, never written over
    let again = planned(&config, "dropbox:id:w", false, "Compte rendu é.md").await.unwrap_err();
    assert!(again.contains("exists already") && again.contains(&format!("dropbox:{}", made.id)), "{again}");
    // a new version: on the revision read; a change meanwhile: refused by Dropbox
    let version = planned(&config, "dropbox:id:f", true, "").await.unwrap();
    assert_eq!((version.name.as_str(), version.place.as_str()), ("plan.txt", "Dropbox › Work"));
    let updated = write(&version, &text_content("plan v2")).await.unwrap();
    assert_eq!(updated.rev, "r1+");
    // its revision is no more: Dropbox's `path/conflict`, said as a change (review A-conflit)
    assert_eq!(write(&version, &text_content("plan v3")).await.unwrap_err(), Fault::Changed);
    // outside: the look-alike, a shared folder (even inside), a shared folder never added
    for (asked, replace, why) in [
        ("dropbox:id:ws", false, "not in a folder your person let you read"),
        ("dropbox:id:s", false, "shared with other people"),
        ("dropbox:id:sf", true, "shared with other people"),
        ("dropbox:id:away", false, "not in a folder your person let you read"),
        ("dropbox:id:f", false, "is a file"),
        ("dropbox:id:w", true, "is a folder"),
    ] {
        let said = planned(&config, asked, replace, "x.md").await.unwrap_err();
        assert!(said.contains(why), "{asked}: {said}");
    }
    // the path from the folder given, on the card (review A-carte)
    w.lock().unwrap().dropbox.insert(
        "id:w19".into(),
        json!({".tag": "folder", "id": "id:w19", "name": "2019", "path_lower": "/work/2019", "path_display": "/Work/2019"}),
    );
    assert_eq!(planned(&config, "dropbox:id:w19", false, "a.md").await.unwrap().place, "Dropbox › Work › 2019");
    // a shared link on the folder or on one above it (review C2): anyone with the link would read it
    w.lock().unwrap().links.push("/work".into());
    for (asked, replace) in [("dropbox:id:w", false), ("dropbox:id:w19", false), ("dropbox:id:f", true)] {
        let said = planned(&config, asked, replace, "x.md").await.unwrap_err();
        assert!(said.contains("shared with other people"), "{asked}: {said}");
    }
    w.lock().unwrap().links.clear();
    // the write right without the links' reading (a connection made before): asked again, never written blind
    keep(
        &config,
        Provider::Dropbox,
        "at-nolinks",
        "files.content.read files.content.write",
        vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])],
    );
    assert!(planned(&config, "dropbox:id:w", false, "x.md").await.unwrap_err().contains("right to write yet"));
    keep(
        &config,
        Provider::Dropbox,
        "at-nolinks",
        DBX_WRITE,
        vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])],
    );
    assert!(planned(&config, "dropbox:id:w", false, "x.md").await.unwrap_err().contains("right to write yet"));
    // the switch off: said with the way to turn it on
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie"], &[])]);
    let off = planned(&config, "dropbox:id:w", false, "x.md").await.unwrap_err();
    assert!(off.contains("peut y écrire") && off.contains("Documents en ligne"), "{off}");
    // the whole Dropbox: a file at its root
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder(ROOT_ID, "Dropbox", "", &["vie"], &["vie"])]);
    let root = planned(&config, "dropbox:root", false, "Au début.md").await.unwrap();
    assert_eq!(root.place, "Dropbox");
    assert_eq!(write(&root, &text_content("x")).await.unwrap().path, "/au début.md");
    assert!(planned(&config, "dropbox:id:r", true, "").await.is_ok(), "the root's own files");
    assert!(planned(&config, "dropbox:id:away", false, "x.md").await.is_err(), "never what has no path here");
    // a read-only connection: the right is asked of the person, the connection goes on
    keep(
        &config,
        Provider::Dropbox,
        "at-read",
        "files.content.read",
        vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])],
    );
    let refused = planned(&config, "dropbox:id:w", false, "Nouveau.md").await.unwrap_err();
    assert!(refused.contains("hasn't given BipAgents the right to write yet"), "before any card: {refused}");
    // (and if the service says it itself: the same words, the connection goes on)
    keep(&config, Provider::Dropbox, "at-read", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])]);
    let plan = planned(&config, "dropbox:id:w", false, "Nouveau.md").await.unwrap();
    assert_eq!(write(&plan, &text_content("x")).await.unwrap_err(), Fault::MissingScope);
    assert_eq!(drives::read_stored(&config).unwrap().drives["dropbox"].state, "connected");
    // the folder taken back: nothing (what the re-check after the card sees)
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![]);
    assert!(planned(&config, "dropbox:id:w", false, "x.md").await.unwrap_err().contains("no folder of Dropbox"));
}

/// Google Drive (drive.file): a new file in a picked folder (resumable, same host), a new version of a picked file
/// (the one before kept for good); never a Google document, a shared folder, a shared drive or a shortcut; a file
/// Google doesn't let the app change: refused there.
#[tokio::test]
async fn google_writes_inside_what_was_picked() {
    let (base, w) = serve().await;
    let (_dir, config) = setup(&base);
    let picked = vec![
        folder("gf", "Maison", "", &["vie"], &["vie"]),
        Folder { file: true, ..folder("gp", "Facture.pdf", "", &["vie"], &["vie"]) },
        folder("gsh", "Famille", "", &["vie"], &["vie"]),
        folder("gdr", "Équipe", "", &["vie"], &["vie"]),
    ];
    keep(&config, Provider::Google, "at", "https://www.googleapis.com/auth/drive.file", picked);
    let plan = planned(&config, "google_drive:gf", false, "Liste.md").await.unwrap();
    assert_eq!(plan.place, "Google Drive › Maison");
    let made = write(&plan, &text_content("- pain")).await.unwrap();
    {
        let w = w.lock().unwrap();
        assert_eq!(w.google[&made.id]["parents"], json!(["gf"]));
        assert_eq!(w.google[&made.id]["mimeType"], "text/markdown");
        assert_eq!(w.bytes[&made.id], b"- pain");
    }
    assert!(planned(&config, "google_drive:gf", false, "Liste.md").await.unwrap_err().contains("exists already"));
    let version = planned(&config, "google_drive:gp", true, "").await.unwrap();
    let updated = write(
        &version,
        &Content {
            bytes: b"%PDF".to_vec(),
            mime: String::new(),
            text: None,
            own_name: Some("f.pdf".into()),
            from_page: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.rev, "h1+");
    assert_eq!(w.lock().unwrap().kept_forever, ["gp@h1"], "the version before kept for good");
    for (asked, replace, why) in [
        ("google_drive:gd", true, "Google Docs"),
        ("google_drive:gsh", false, "shared with other people"),
        ("google_drive:gsub", false, "shared with other people"),
        ("google_drive:gdr", false, "shared with other people"),
        ("google_drive:gcut", false, "is a file"),
    ] {
        let said = planned(&config, asked, replace, "x.md").await.unwrap_err();
        assert!(said.contains(why), "{asked}: {said}");
    }
    // inside a picked folder but not picked itself: Google's refusal, said by the call (drive.file, never wider)
    let not_picked = planned(&config, "google_drive:gnp", true, "").await.unwrap();
    assert_eq!(write(&not_picked, &text_content("x")).await.unwrap_err(), Fault::Refused);
}

/// OneDrive (Files.ReadWrite, 23/11): a new file (`conflictBehavior=fail`), a new version with `If-Match`, the
/// whole OneDrive by its root read again; never someone else's item nor a shared folder.
#[tokio::test]
async fn onedrive_writes_with_if_match_and_its_whole_drive_by_the_parents() {
    let (base, w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Microsoft, "at", "Files.ReadWrite", vec![folder("D", "Docs", "", &["vie"], &["vie"])]);
    let plan = planned(&config, "onedrive:D", false, "Été.md").await.unwrap();
    let made = write(&plan, &text_content("x")).await.unwrap();
    assert_eq!(w.lock().unwrap().graph[&made.id].pointer("/parentReference/id"), Some(&json!("D")));
    assert!(planned(&config, "onedrive:D", false, "Été.md").await.unwrap_err().contains("exists already"));
    let version = planned(&config, "onedrive:F1", true, "").await.unwrap();
    w.lock().unwrap().graph.get_mut("F1").unwrap()["eTag"] = json!("e9"); // changed meanwhile
    assert_eq!(write(&version, &text_content("v2")).await.unwrap_err(), Fault::Changed);
    let version = planned(&config, "onedrive:F1", true, "").await.unwrap();
    assert!(write(&version, &text_content("v2")).await.is_ok());
    assert!(planned(&config, "onedrive:TOP", true, "").await.unwrap_err().contains("not in a folder"));
    // the whole OneDrive
    keep(
        &config,
        Provider::Microsoft,
        "at",
        "Files.ReadWrite",
        vec![folder(ROOT_ID, "OneDrive", "", &["vie"], &["vie"])],
    );
    assert_eq!(planned(&config, "onedrive:root", false, "Racine.md").await.unwrap().place, "OneDrive");
    assert!(planned(&config, "onedrive:TOP", true, "").await.is_ok(), "the root's own files");
    assert!(planned(&config, "onedrive:F1", true, "").await.is_ok());
    assert!(planned(&config, "onedrive:SHF", true, "").await.unwrap_err().contains("shared with other people"));
    assert!(planned(&config, "onedrive:R", false, "x.md").await.is_err(), "someone else's");
}

/// « Annuler » of a new file made at once: in the service's bin while unchanged, never once changed; gone already:
/// nothing to do.
#[tokio::test]
async fn annuler_puts_the_new_file_in_the_bin_only_while_unchanged() {
    let (base, w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])]);
    let made =
        write(&planned(&config, "dropbox:id:w", false, "a.md").await.unwrap(), &text_content("a")).await.unwrap();
    let changed =
        write(&planned(&config, "dropbox:id:w", false, "b.md").await.unwrap(), &text_content("b")).await.unwrap();
    w.lock().unwrap().dropbox.get_mut(&changed.id).unwrap()["rev"] = json!("r7");
    assert!(undo_file(&config, "dropbox", &changed.id, &changed.rev).await.unwrap_err().contains("changed"));
    assert!(w.lock().unwrap().dropbox.contains_key(&changed.id), "left as it is");
    undo_file(&config, "dropbox", &made.id, &made.rev).await.unwrap();
    assert!(!w.lock().unwrap().dropbox.contains_key(&made.id));
    undo_file(&config, "dropbox", &made.id, &made.rev).await.unwrap();
    keep(&config, Provider::Google, "at", "", vec![folder("gf", "Maison", "", &["vie"], &["vie"])]);
    let made =
        write(&planned(&config, "google_drive:gf", false, "c.md").await.unwrap(), &text_content("c")).await.unwrap();
    undo_file(&config, "google_drive", &made.id, &made.rev).await.unwrap();
    assert_eq!(w.lock().unwrap().google[&made.id]["trashed"], true, "Google's bin, not deleted");
    keep(&config, Provider::Microsoft, "at", "Files.ReadWrite", vec![folder("D", "Docs", "", &["vie"], &["vie"])]);
    let made = write(&planned(&config, "onedrive:D", false, "d.md").await.unwrap(), &text_content("d")).await.unwrap();
    undo_file(&config, "onedrive", &made.id, &made.rev).await.unwrap();
    assert!(!w.lock().unwrap().graph.contains_key(&made.id), "OneDrive's recycle bin");
}

/// The content: a text, one of the agent's files (never through a link), one of its own pages; too large: never.
#[test]
fn the_content_from_a_text_a_file_of_its_own_or_one_of_its_pages() {
    let (dir, config) = setup("http://127.0.0.1:9");
    let agent = vie(&config);
    let uploads = dir.path().join("uploads/vie");
    std::fs::write(uploads.join("devis.pdf"), b"%PDF-1").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", uploads.join("lien.txt")).unwrap();
    let got = content_of(&config, &agent, "", "/fichiers/devis.pdf").unwrap();
    assert_eq!((got.bytes.as_slice(), got.own_name.as_deref()), (b"%PDF-1".as_slice(), Some("devis.pdf")));
    assert!(content_of(&config, &agent, "", "/fichiers/lien.txt").is_err(), "never through a link");
    assert!(content_of(&config, &agent, "", "/fichiers/../../etc/passwd").is_err());
    assert!(content_of(&config, &agent, "x", "/fichiers/devis.pdf").is_err(), "one or the other");
    assert!(content_of(&config, &agent, "", "").is_err());
    let pages = crate::pages::root(&config);
    std::fs::create_dir_all(pages.join("vie")).unwrap();
    std::fs::create_dir_all(pages.join("kerfy")).unwrap();
    std::fs::write(pages.join("vie/budget.html"), b"<html>").unwrap();
    std::fs::write(pages.join("kerfy/autre.html"), b"<html>").unwrap();
    assert_eq!(content_of(&config, &agent, "", "/bipagents/pages/vie/budget.html").unwrap().bytes, b"<html>");
    assert!(content_of(&config, &agent, "", "/bipagents/pages/kerfy/autre.html").is_err(), "another agent's page");
    assert!(content_of(&config, &agent, "", "/bipagents/pages/vie/../kerfy/autre.html").is_err());
    let big = "x".repeat(WRITE_MAX_BYTES as usize + 1);
    assert!(content_of(&config, &agent, &big, "").unwrap_err().contains("too large"));
    std::fs::File::create(uploads.join("gros.bin")).unwrap().set_len(WRITE_MAX_BYTES + 1).unwrap();
    assert!(content_of(&config, &agent, "", "/fichiers/gros.bin").unwrap_err().contains("too large"));
}

/// The agent's own reads of the whole drive: Dropbox's root listed, its files read; the tool offered with the rest.
#[tokio::test]
async fn the_whole_dropbox_is_read_from_its_root() {
    let (base, _w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder(ROOT_ID, "Dropbox", "", &["vie"], &["vie"])]);
    let agent = vie(&config);
    let call = |name: &'static str, a: Value| {
        let (config, agent) = (config.clone(), agent.clone());
        async move { said(&drives::call(&config, &agent, name, &serde_json::from_value(a).unwrap()).await) }
    };
    let folders = call("drive_folders", json!({})).await;
    assert!(
        folders.contains("your person's whole Dropbox")
            && folders.contains("you may write in it")
            && folders.ends_with("dropbox:root\n>>>"),
        "{folders}"
    );
    let listed = call("drive_list", json!({"folder": "dropbox:root"})).await;
    assert!(listed.contains("racine.txt") && listed.contains("[folder] Work"), "{listed}");
    assert!(!call("drive_list", json!({"folder": "dropbox:id:away"})).await.contains("Ailleurs:"));
    assert!(drives::tools().iter().any(|t| t["name"] == TOOL));
}

/// Microsoft asking a secret of Kerfy's public client (« Autoriser les flux clients publics », 23/11): its code in the
/// logs and the app's error, to tell Alex which setting.
#[tokio::test]
async fn a_refused_microsoft_sign_in_says_its_aadsts_code() {
    let (base, _w) = serve().await;
    let server = connections::Server { token_endpoint: format!("{base}/token-aad"), ..Default::default() };
    let client = connections::Client { client_id: "ms".into(), auth_method: "none".into(), ..Default::default() };
    let refused = connections::token_request(
        &reqwest::Client::new(),
        &server,
        &client,
        vec![("grant_type", "authorization_code".into())],
    )
    .await
    .unwrap_err();
    assert_eq!(refused, (401, "invalid_client AADSTS7000218".to_string()));
}

// -- review-drive-write (23/11): red tests of the independent review (briefs/review-drive-write.md) ---------------------

/// Review R1: a new file never lands on the person's computers (Dropbox and OneDrive sync it there, without the
/// « downloaded from the internet » mark) as something that runs or reaches elsewhere when clicked or merely shown
/// (a `.bat`, a `.url` whose icon is fetched from another machine…), nor with a name that only looks like another
/// one (an invisible character). Under « Seulement le critique » it goes through with no card at all.
#[test]
fn review_a_name_never_runs_nor_links_nor_hides_a_character() {
    for bad in [
        "Facture.pdf.bat",
        "Lis-moi.cmd",
        "setup.exe",
        "photo.scr",
        "x.vbs",
        "x.ps1",
        "x.hta",
        "x.msi",
        "Raccourci.lnk",
        "Site.url",
        "x.scf",
        "x.library-ms",
        "Ouvre-moi.command",
        "desktop.ini",
        "Budget\u{200b}.xlsx",
        "Budget\u{2060}.xlsx",
        "Budget\u{feff}.xlsx",
        "Bud\u{ad}get.xlsx",
        "\u{61c}rapport.md",
    ] {
        assert!(clean_name(bad).is_err(), "{bad:?} accepted");
    }
}

/// Review R2: OneDrive, a folder given for writing that sits inside a folder shared with other people (the facet on
/// the shared folder, not on what is inside it): never written into, as the module promises (« a folder or file
/// shared with other people »). Today the parents are looked at only up to the folder given.
#[tokio::test]
async fn review_onedrive_never_writes_inside_a_folder_shared_above_the_one_given() {
    let (base, w) = serve().await;
    let (_dir, config) = setup(&base);
    {
        let mut w = w.lock().unwrap();
        w.graph.insert(
            "FAM".into(),
            json!({"id": "FAM", "name": "Famille", "folder": {}, "parentReference": {"id": "ROOT"},
                   "shared": {"scope": "users"}}),
        );
        w.graph.insert(
            "FAC".into(),
            json!({"id": "FAC", "name": "Factures", "folder": {}, "parentReference": {"id": "FAM"}}),
        );
    }
    keep(
        &config,
        Provider::Microsoft,
        "at",
        "Files.ReadWrite",
        vec![folder("FAC", "Factures", "", &["vie"], &["vie"])],
    );
    let said = planned(&config, "onedrive:FAC", false, "Relevé.md").await.err().unwrap_or_default();
    assert!(said.contains("shared with other people"), "written inside « Famille », shared: {said:?}");
}

/// Review R3: a bridge put back to the version before (a rollback) still reads approval-level.json after a new file
/// was made at once: today its `Undo` has no `drive_file`, the whole file fails to parse, and the person's level,
/// her agents' levels and her « Toujours » are read as the defaults, then written over at the next change.
#[test]
fn review_an_older_bridge_still_reads_the_levels_after_a_new_file_made_at_once() {
    #[derive(serde::Deserialize)]
    #[serde(tag = "what", rename_all = "snake_case")]
    #[allow(dead_code)]
    enum OldUndo {
        MailJob { job: String },
        MailRule { rule: String },
        Task { job: String },
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OldLine {
        #[serde(default)]
        undo: Option<OldUndo>,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OldBook {
        #[serde(default)]
        level: Option<String>,
        #[serde(default)]
        lines: Vec<OldLine>,
    }
    let (_dir, config) = setup("http://127.0.0.1:9");
    crate::approval_level::set_level(&config, "critical").unwrap();
    // (adapted to the fix: what undoes the new file is kept apart, drive-undo.json; the line, its « Annuler » shown)
    let line = record_made(
        &config,
        crate::approval_level::Line {
            agent: "vie".into(),
            kind: KIND_CREATE.into(),
            summary: "« a.md » enregistré dans Dropbox › Work".into(),
            ..Default::default()
        },
        Made { service: "dropbox".into(), id: "id:new1".into(), rev: "r1".into(), account: None, at: py::time_time() },
    );
    let shown = crate::approval_level::lines(&config, Some("vie"), 10);
    assert_eq!(crate::approval_level::line_view(&shown[0])["undo"], true, "« Annuler » still shown");
    assert_eq!(shown[0].id, line.id);
    let text = std::fs::read_to_string(std::path::Path::new(&config.data_dir()).join("approval-level.json")).unwrap();
    let old = serde_json::from_str::<OldBook>(&text);
    assert!(old.is_ok(), "the bridge before drive-write can't read it: {:?}", old.err());
    assert_eq!(old.unwrap().level.as_deref(), Some("critical"));
}

/// Review B1 with the fix: the types made at once without a card are documents and pictures only (an HTML file only
/// from the agent's own pages); the ones that run never, card or not; a new version of one neither.
#[test]
fn only_documents_go_through_at_once_and_nothing_that_runs_ever() {
    for ok in ["a.md", "a.TXT", "Relevé.pdf", "b.docx", "c.xlsx", "photo.JPG", "d.csv", "e.json", "f.odt"] {
        assert!(at_once_kind(ok, false), "{ok}");
        assert!(never_written(ok).is_none(), "{ok}");
    }
    for asked in ["page.html", "dessin.svg", "archive.zip", "sans-extension"] {
        assert!(!at_once_kind(asked, false) && never_written(asked).is_none(), "{asked}: with a card only");
    }
    assert!(at_once_kind("budget.html", true), "one of its own pages");
    for bad in [
        "NUL.txt",
        "con",
        "Lpt1.md",
        "autorun.inf",
        "Thumbs.db",
        "x.docm",
        "x.xlsm",
        "x.sh",
        "x.dmg",
        "x.webloc",
        "x.desktop",
        "x.app",
        "x.pkg",
        "x.apk",
        "x.iso",
        "x.jar",
        "x.reg",
        "x.js",
        "x.py",
    ] {
        assert!(clean_name(bad).is_err(), "{bad:?} accepted");
    }
    assert!(crate::loop_brake::write_tool("drive_write"), "counted by the brake (review C8)");
    assert!(crate::loop_brake::write_tool("mcp_bipagents_drive_write"));
}

/// Review C5: OneDrive's new file through an upload session (`conflictBehavior: fail`), the bytes to its address
/// without the bearer; a 200 (an update) is a failure said, never a new file.
#[tokio::test]
async fn onedrive_s_new_file_is_a_201_or_nothing() {
    let (base, w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Microsoft, "at", "Files.ReadWrite", vec![folder("D", "Docs", "", &["vie"], &["vie"])]);
    let made =
        write(&planned(&config, "onedrive:D", false, "Neuf.md").await.unwrap(), &text_content("x")).await.unwrap();
    assert!(made.id.starts_with('M'));
    w.lock().unwrap().upload_200 = true;
    let plan = planned(&config, "onedrive:D", false, "Autre.md").await.unwrap();
    match write(&plan, &text_content("x")).await {
        Err(Fault::Other(why)) => assert!(why.contains("not counted as a new file"), "{why}"),
        other => panic!("a 200 taken for a new file: {other:?}"),
    }
}

/// Review remark A-annuler: signed in with another account since, « Annuler » says so and leaves the file (never
/// « Annulé » for a file still there).
#[tokio::test]
async fn annuler_after_another_account_says_so() {
    let (base, _w) = serve().await;
    let (_dir, config) = setup(&base);
    keep(&config, Provider::Dropbox, "at", DBX_WRITE, vec![folder("id:w", "Work", "/work", &["vie"], &["vie"])]);
    let made =
        write(&planned(&config, "dropbox:id:w", false, "a.md").await.unwrap(), &text_content("a")).await.unwrap();
    let line = record_made(
        &config,
        crate::approval_level::Line { agent: "vie".into(), kind: KIND_CREATE.into(), ..Default::default() },
        Made {
            service: "dropbox".into(),
            id: made.id.clone(),
            rev: made.rev.clone(),
            account: Some("b@example.org".into()),
            at: py::time_time(),
        },
    );
    let refused = undo_line(&config, &line.id).await.unwrap().unwrap_err();
    assert!(refused.contains("another account"), "{refused}");
    assert!(undoable(&config).contains(&line.id), "still undoable from the right account");
    assert!(undo_line(&config, "not-a-file-line").await.is_none());
}
