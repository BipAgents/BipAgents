//! The person's Obsidian vault (obsidian.rs, obsidian_ob.rs): the sign-in typed into `ob` on a terminal of the
//! bridge's (a fake `ob`, tests/fixtures/fake_ob.py, asks the same questions), the vault set up and synced, the
//! agents chosen, and their tools: reads straight away, every write after the person's card.

mod common;

use bipbridge::agent_tools;
use bipbridge::config::{parse_config, Config};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "obsidian-agent-token";

fn fake_ob() -> String {
    format!("{}/tests/fixtures/fake_ob.py", env!("CARGO_MANIFEST_DIR"))
}

fn with_agent(dir: &std::path::Path, obsidian: Value) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    dict["obsidian"] = obsidian;
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("coach".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    config
}

async fn rpc(app: &TestApp, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "method": method, "params": params, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()["result"].clone()
}

async fn tool(app: &TestApp, name: &str, arguments: Value) -> (bool, String) {
    let result = rpc(app, "tools/call", json!({"name": name, "arguments": arguments})).await;
    (result["isError"] == true, result["content"][0]["text"].as_str().unwrap_or("").to_string())
}

async fn tool_names(app: &TestApp) -> Vec<String> {
    rpc(app, "tools/list", json!({})).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn all_files_text(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in walk(dir) {
        if let Ok(bytes) = std::fs::read(&entry) {
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    out
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let meta = std::fs::symlink_metadata(entry.path()).unwrap();
        if meta.is_dir() {
            out.extend(walk(&entry.path()));
        } else if meta.is_file() {
            out.push(entry.path());
        }
    }
    out
}

/// A vault already on the machine (a self-install): no sign-in; the agents chosen read at once and write after a card.
#[tokio::test]
async fn a_local_vault_is_read_at_once_and_written_only_after_the_persons_yes() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("Coffre");
    std::fs::create_dir_all(vault.join("Projets")).unwrap();
    std::fs::write(vault.join("Projets/Maison.md"), "# Maison\n\n## Travaux\n- toiture\n").unwrap();
    let config = with_agent(dir.path(), json!({"enabled": true, "folder": vault.to_string_lossy()}));
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(app.get("/v1/obsidian", &[]).await.status, 401, "the bridge key only");
    let shown = app.get("/v1/obsidian", &h).await.json();
    assert_eq!((shown["state"].as_str(), shown["local"].as_bool(), shown["notes"].as_u64()), (Some("connected"), Some(true), Some(1)));
    assert_eq!(shown["vault"]["name"], "Coffre");
    assert!(!tool_names(&app).await.iter().any(|n| n.starts_with("obsidian_")), "not chosen: not listed");
    assert!(tool(&app, "obsidian_search", json!({"words": "toiture"})).await.0);
    assert_eq!(app.send_json("POST", "/v1/obsidian/login", &h, &json!({"email": "a@b.c", "password": "x"})).await.status, 409);

    let set = app.send_json("PUT", "/v1/obsidian/agents", &h, &json!({"agents": ["coach"]})).await;
    assert_eq!(set.status, 200, "{}", set.text());
    assert_eq!(set.json()["agents"], json!(["coach"]));
    let names = tool_names(&app).await;
    for name in ["obsidian_list", "obsidian_search", "obsidian_read", "obsidian_create", "obsidian_append", "obsidian_edit"] {
        assert!(names.contains(&name.to_string()), "{name}");
    }
    assert_eq!(app.send_json("PUT", "/v1/obsidian/agents", &h, &json!({"agents": ["nobody"]})).await.status, 422);

    let (failed, found) = tool(&app, "obsidian_search", json!({"words": "toiture"})).await;
    assert!(!failed && found.contains("Projets/Maison.md"), "{found}");
    let (_, read) = tool(&app, "obsidian_read", json!({"note": "Projets/Maison"})).await;
    assert!(read.contains("## Travaux"), "{read}");

    // a new note: a card first, « Oui », then written
    let ((failed, said), card) = tokio::join!(
        tool(&app, "obsidian_create", json!({"note": "Idées/Cadeaux", "text": "- un livre pour Chi"})),
        common::answer_card(&app, "bip-ok-", "once")
    );
    assert!(!failed, "{said}");
    assert_eq!(card["bip_summary"], "Coach wants to create the note « Cadeaux » in Obsidian");
    assert_eq!(card["bip_detail"], "Folder: Idées\nNew note:\n- un livre pour Chi");
    assert_eq!(card["bip_ok"]["kind"], "service_write");
    assert_eq!(card["bip_always"], "Always let Coach write in Obsidian");
    assert_eq!(std::fs::read_to_string(vault.join("Idées/Cadeaux.md")).unwrap(), "- un livre pour Chi");
    let (failed, said) = tool(&app, "obsidian_create", json!({"note": "Idées/Cadeaux", "text": "x"})).await;
    assert!(failed && said.contains("exists already"), "{said}");

    // « Non »: nothing written
    let ((failed, said), _) = tokio::join!(
        tool(&app, "obsidian_append", json!({"note": "Projets/Maison", "heading": "Travaux", "text": "- isolation"})),
        common::answer_card(&app, "bip-ok-", "deny")
    );
    assert!(failed && said.contains("said no"), "{said}");
    assert!(!std::fs::read_to_string(vault.join("Projets/Maison.md")).unwrap().contains("isolation"));

    // a change of text already there: critical, its before and after shown, a copy kept
    let ((failed, said), card) = tokio::join!(
        tool(&app, "obsidian_edit", json!({"note": "Projets/Maison", "find": "- toiture", "replace": "- toiture (devis reçu)"})),
        common::answer_card(&app, "bip-ok-", "once")
    );
    assert!(!failed, "{said}");
    assert_eq!(card["bip_ok"]["kind"], "notes_change");
    assert!(card.get("bip_always").is_none(), "never « Toujours » for a change: {card}");
    assert_eq!(card["bip_detail"], "Folder: Projets\nBefore:\n- toiture\nAfter:\n- toiture (devis reçu)");
    assert!(std::fs::read_to_string(vault.join("Projets/Maison.md")).unwrap().contains("(devis reçu)"));
    let backups = walk(&dir.path().join("data").join("obsidian").join("backups"));
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert!(!std::fs::read_to_string(&backups[0]).unwrap().contains("devis"), "the copy is the old version");

    // « Seulement le critique »: creating and adding go at once, a change is still asked
    let level = app.send_json("PUT", "/v1/approval-level", &h, &json!({"level": "critical"})).await;
    assert_eq!(level.status, 200, "{}", level.text());
    let (failed, said) =
        tool(&app, "obsidian_append", json!({"note": "Projets/Maison", "heading": "Travaux", "text": "- isolation"})).await;
    assert!(!failed, "{said}");
    assert_eq!(
        std::fs::read_to_string(vault.join("Projets/Maison.md")).unwrap(),
        "# Maison\n\n## Travaux\n- toiture (devis reçu)\n- isolation\n"
    );
    let auto = app.get("/v1/approvals/auto?agent=coach", &h).await.json();
    assert!(auto.to_string().contains("Coach wants to add to the note « Maison » in Obsidian"), "{auto}");
    let ((failed, _), card) = tokio::join!(
        tool(&app, "obsidian_edit", json!({"note": "Projets/Maison", "find": "- isolation", "replace": ""})),
        common::answer_card(&app, "bip-ok-", "deny")
    );
    assert!(failed && card["bip_ok"]["kind"] == "notes_change");

    // the daily note, made where the vault's settings say
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    std::fs::write(vault.join(".obsidian/daily-notes.json"), r#"{"folder": "Journal", "format": "YYYY-MM-DD"}"#).unwrap();
    let (failed, said) = tool(&app, "obsidian_append", json!({"daily": true, "text": "Appeler le garage"})).await;
    assert!(!failed, "{said}");
    let journal = walk(&vault.join("Journal"));
    assert_eq!(journal.len(), 1);
    assert_eq!(std::fs::read_to_string(&journal[0]).unwrap(), "Appeler le garage\n");

    // never outside the vault, never Obsidian's own folders
    assert!(tool(&app, "obsidian_create", json!({"note": "../dehors", "text": "x"})).await.1.contains("leaves the vault"));
    assert!(tool(&app, "obsidian_read", json!({"note": ".obsidian/daily-notes.json"})).await.0);

    // the agent no longer chosen: no tools
    assert_eq!(app.send_json("PUT", "/v1/obsidian/agents", &h, &json!({"agents": []})).await.status, 200);
    assert!(!tool_names(&app).await.iter().any(|n| n.starts_with("obsidian_")));
    assert!(tool(&app, "obsidian_read", json!({"note": "Projets/Maison"})).await.0);
}

/// Obsidian Sync: the sign-in with its 2FA code and the vault's end-to-end password typed into `ob`'s questions, never
/// on a command line nor kept; the copy synced both ways; a write reaches the « server »; disconnecting deletes the
/// place's copy.
#[tokio::test]
async fn signing_in_from_the_app_sets_up_the_vault_and_syncs_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let server = dir.path().join("server");
    std::fs::create_dir_all(server.join("remote/v2/Journal")).unwrap();
    std::fs::write(server.join("accounts.json"), r#"{"twofa": true}"#).unwrap();
    std::fs::write(server.join("remote/v2/Journal/Lundi.md"), "réunion avec le notaire\n").unwrap();
    let command = json!(["python3", fake_ob(), server.to_string_lossy()]);
    let config = with_agent(dir.path(), json!({"enabled": true, "command": command}));
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let shown = app.get("/v1/obsidian", &h).await.json();
    assert_eq!((shown["state"].as_str(), shown["tool"].as_str()), (Some("not_connected"), Some("ready")));

    // a wrong password: refused, plainly
    let wrong = app.send_json("POST", "/v1/obsidian/login", &h, &json!({"email": "alex@example.org", "password": "nope"})).await;
    assert_eq!(wrong.status, 401, "{}", wrong.text());
    assert_eq!(wrong.json()["detail"]["code"], "wrong_login");

    // the account has 2FA: the code is asked, then typed into the same `ob`
    let first = app.send_json("POST", "/v1/obsidian/login", &h, &json!({"email": "alex@example.org", "password": "s3cret"})).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(first.json()["step"], "code_needed");
    let flow = first.json()["flow"].as_str().unwrap().to_string();
    let bad = app.send_json("POST", "/v1/obsidian/login/code", &h, &json!({"flow": "nope", "code": "1"})).await;
    assert_eq!(bad.status, 404);
    let signed = app.send_json("POST", "/v1/obsidian/login/code", &h, &json!({"flow": flow, "code": "123456"})).await;
    assert_eq!(signed.status, 200, "{}", signed.text());
    let signed = signed.json();
    assert_eq!(signed["step"], "signed_in");
    assert_eq!(signed["vaults"], json!([{"id": "v1", "name": "Perso", "shared": false}, {"id": "v2", "name": "Secret", "shared": false}]));
    assert_eq!(signed["obsidian"]["state"], "choose_vault");
    assert_eq!(signed["obsidian"]["account"], json!({"name": "Alex", "email": "alex@example.org"}));

    // the end-to-end encrypted vault: its password asked, then typed
    let chosen = app.send_json("POST", "/v1/obsidian/vault", &h, &json!({"vault": "v2", "agents": ["coach"]})).await;
    assert_eq!(chosen.status, 200, "{}", chosen.text());
    assert_eq!(chosen.json()["step"], "password_needed");
    let flow = chosen.json()["flow"].as_str().unwrap().to_string();
    let wrong = app.send_json("POST", "/v1/obsidian/vault/password", &h, &json!({"flow": flow, "password": "x"})).await;
    assert_eq!(wrong.status, 401);
    assert_eq!(wrong.json()["detail"]["code"], "wrong_vault_password");
    let again = app.send_json("POST", "/v1/obsidian/vault", &h, &json!({"vault": "v2", "agents": ["coach"]})).await;
    let flow = again.json()["flow"].as_str().unwrap().to_string();
    let done = app.send_json("POST", "/v1/obsidian/vault/password", &h, &json!({"flow": flow, "password": "vaultpw"})).await;
    assert_eq!(done.status, 200, "{}", done.text());
    assert_eq!(done.json()["step"], "connected");
    assert_eq!(done.json()["obsidian"]["vault"], json!({"id": "v2", "name": "Secret"}));

    // the first sync brings the notes
    let vault = dir.path().join("data/obsidian/vault");
    for _ in 0..100 {
        if vault.join("Journal/Lundi.md").is_file() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(vault.join("Journal/Lundi.md").is_file(), "first sync");
    let shown = app.get("/v1/obsidian", &h).await.json();
    assert_eq!(shown["sync"], "ok", "{shown}");
    let (_, found) = tool(&app, "obsidian_search", json!({"words": "notaire"})).await;
    assert!(found.contains("Journal/Lundi.md"), "{found}");

    // a write goes out at the next sync (the scheduler's, a few seconds after it; here asked by the app's button)
    let ((failed, said), _) = tokio::join!(
        tool(&app, "obsidian_append", json!({"note": "Journal/Lundi", "text": "- apporter le RIB"})),
        common::answer_card(&app, "bip-ok-", "once")
    );
    assert!(!failed, "{said}");
    assert_eq!(app.send_json("POST", "/v1/obsidian/sync", &h, &json!({})).await.status, 200);
    let remote = server.join("remote/v2/Journal/Lundi.md");
    for _ in 0..150 {
        if std::fs::read_to_string(&remote).unwrap().contains("RIB") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(std::fs::read_to_string(&remote).unwrap().contains("apporter le RIB"), "synced out");

    // nothing typed is on a command line, nor kept anywhere on the place
    let calls = std::fs::read_to_string(server.join("calls.log")).unwrap();
    assert!(!calls.contains("s3cret") && !calls.contains("123456") && !calls.contains("vaultpw"), "{calls}");
    assert!(calls.contains(r#"["sync-config", "--path""#), "only the core plugins' settings: {calls}");
    let kept = all_files_text(&dir.path().join("data"));
    assert!(!kept.contains("s3cret") && !kept.contains("vaultpw"), "a password kept on the place");
    use std::os::unix::fs::PermissionsExt;
    let state = dir.path().join("data/obsidian/state");
    assert_eq!(std::fs::metadata(&state).unwrap().permissions().mode() & 0o777, 0o700);

    // disconnected: signed out, the place's copy gone, the notes still on the « server »
    let gone = app.send_json("DELETE", "/v1/obsidian", &h, &json!({})).await;
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert_eq!(gone.json()["state"], "not_connected");
    assert!(!vault.exists() && !state.exists());
    assert!(remote.is_file());
    assert!(!tool_names(&app).await.iter().any(|n| n.starts_with("obsidian_")));
}

#[tokio::test]
async fn off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_agent(dir.path(), json!({}));
    let app = TestApp::new(config);
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let answer = app.get("/v1/obsidian", &h).await;
    assert_eq!(answer.status, 404);
    assert_eq!(answer.json()["detail"]["code"], "off");
}
