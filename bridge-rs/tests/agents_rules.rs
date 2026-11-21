//! « Entre tes agents » as rules (rules.rs, rules_live.rs; Alex 21/11, piste A): the apps' screen and its changes, the
//! gates that obey them (message_agent, send_files_to_agent, the shared folders' mounts, the households' relay), the
//! migration at start, and two households agreeing way by way with two bridges on their own sockets.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::routing::any;
use axum::Router;
use bipbridge::agents_admin::Run;
use bipbridge::config::{parse_config, SharedConfig};
use bipbridge::rules::{self, Mode};
use bipbridge::shares;
use common::{auth, config_dict, spawn_fake, TestApp, Urls};
use serde_json::{json, Value};

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn links(screen: &Value) -> Vec<(String, String)> {
    screen["effective"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["from"].as_str().unwrap().to_string(), l["to"].as_str().unwrap().to_string()))
        .collect()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// A place with Vie, Budget and Wellness (Hermes unreachable: only the rules are looked at).
fn three(dir: &Path) -> TestApp {
    let mut d = config_dict(dir, &Urls::default());
    for name in ["Vie", "Budget"] {
        d["agents"][name] = json!({"hermes_url": "http://127.0.0.1:9", "hermes_key": "k".repeat(32)});
    }
    TestApp::new(parse_config(&d, None).unwrap())
}

#[tokio::test]
async fn the_screen_and_its_small_changes() {
    let dir = tempfile::tempdir().unwrap();
    let app = three(dir.path());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    // a new account: every agent writes to every other, none reads another's folder
    let screen = app.get("/v1/agent-rules", &h).await.json();
    assert_eq!((screen["write"].clone(), screen["read"].clone()), (json!("all"), json!("none")));
    assert_eq!(screen["default"], json!({"write": "all", "read": "none"}));
    assert_eq!(links(&screen).len(), 6);
    let mut ids: Vec<&str> = screen["agents"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, ["budget", "vie", "wellness"]);
    // « Certains »: only the « liens à part »
    let put = |path: &'static str, body: Value| {
        let app = &app;
        async move { app.send_json("PUT", path, &h, &body).await }
    };
    let screen = put("/v1/agent-rules/write", json!({"mode": "some"})).await.json();
    assert!(links(&screen).is_empty());
    let screen = put("/v1/agent-rules/pairs", json!({"from": "vie", "to": "budget", "on": true})).await.json();
    assert_eq!(links(&screen), pairs(&[("vie", "budget")]));
    assert_eq!(screen["pairs"], json!([{"from": "vie", "to": "budget"}]));
    // an older app sees the same
    assert_eq!(app.get("/v1/agent-links", &h).await.json()["links"], json!([{"from": "vie", "to": "budget"}]));
    // « Tous » again, then Budget « à l'écart »: out of everything
    put("/v1/agent-rules/write", json!({"mode": "all"})).await;
    let screen = put("/v1/agent-rules/aside", json!({"agent": "budget", "on": true})).await.json();
    assert_eq!(links(&screen), pairs(&[("vie", "wellness"), ("wellness", "vie")]));
    assert_eq!(screen["aside"], json!(["budget"]));
    assert!(screen["agents"].as_array().unwrap().iter().any(|a| a["id"] == "budget" && a["aside"] == true));
    // what is refused
    for (path, body) in [
        ("/v1/agent-rules/write", json!({"mode": "most"})),
        ("/v1/agent-rules/pairs", json!({"from": "vie", "to": "nobody", "on": true})),
        ("/v1/agent-rules/pairs", json!({"from": "vie", "to": "vie", "on": true})),
        ("/v1/agent-rules/aside", json!({"agent": "vie"})),
        ("/v1/agent-rules/reads", json!({"reader": "vie", "source": "budget", "on": true})), // no sandbox
        ("/v1/agent-rules/family/vie", json!({"target": "budget", "on": true})), // not the family's agent
    ] {
        let said = put(path, body.clone()).await;
        assert_eq!(said.status, 400, "{path} {body}: {}", said.text());
    }
    let unknown = put("/v1/agent-rules/households/lea", json!({"way": "in", "mode": "all"})).await;
    assert_eq!(unknown.status, 404);
    // the file: 0600, the rules as the screen says them
    use std::os::unix::fs::PermissionsExt;
    let file = rules::path(&app.app.state.config());
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(rules::load(&app.app.state.config()).write, Mode::All);
}

#[tokio::test]
async fn the_familys_agent_asks_only_those_opened_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    for name in ["Vie", "Famille"] {
        d["agents"][name] = json!({"hermes_url": "http://127.0.0.1:9", "hermes_key": "k".repeat(32)});
    }
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/family-agent.json"), r#"{"agent": "famille"}"#).unwrap();
    let app = TestApp::new(parse_config(&d, None).unwrap());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let config = app.app.state.config();
    // « Tous » is not the family's agent: it asks no one, no one writes to it
    assert!(!rules::allows(&config, "famille", "vie") && !rules::allows(&config, "vie", "famille"));
    assert!(!bipbridge::agent_messages::offered(&config, "famille"));
    let said = app.send_json("PUT", "/v1/agent-rules/family/famille", &h, &json!({"target": "vie", "on": true})).await;
    assert_eq!(said.status, 200, "{}", said.text());
    assert_eq!(said.json()["family"], json!([{"id": "famille", "name": "Famille", "targets": ["vie"]}]));
    assert!(rules::allows(&config, "famille", "vie") && !rules::allows(&config, "famille", "wellness"));
    // the one gate (agent_files.rs `targets`) the family's agent's message_agent follows
    assert!(bipbridge::agent_messages::offered(&config, "famille"));
    let said = app.send_json("PUT", "/v1/agent-rules/pairs", &h, &json!({"from": "famille", "to": "vie", "on": true})).await;
    assert_eq!(said.status, 400);
    // Vie « à l'écart »: the family's agent can't ask it either
    app.send_json("PUT", "/v1/agent-rules/aside", &h, &json!({"agent": "vie", "on": true})).await;
    assert!(!rules::allows(&config, "famille", "vie"));
}

#[derive(Clone, Default)]
struct Calls(Arc<Mutex<Vec<(String, String)>>>);

/// A Hermes that answers every ask « Sept heures. ».
async fn hermes(calls: Calls) -> String {
    spawn_fake(Router::new().fallback(any(move |request: Request| {
        let calls = calls.clone();
        async move {
            let (method, path) = (request.method().as_str().to_string(), request.uri().path().to_string());
            calls.0.lock().unwrap().push((method.clone(), path.clone()));
            match (method.as_str(), path.as_str()) {
                ("POST", p) if p.ends_with("/api/sessions") => {
                    (axum::http::StatusCode::CREATED, axum::Json(json!({"session": {"id": "s1"}})))
                }
                ("POST", p) if p.ends_with("/v1/runs") => {
                    (axum::http::StatusCode::ACCEPTED, axum::Json(json!({"run_id": "r1"})))
                }
                ("GET", p) if p.contains("/v1/runs/") => {
                    (axum::http::StatusCode::OK, axum::Json(json!({"status": "completed", "output": "Sept heures."})))
                }
                _ => (axum::http::StatusCode::OK, axum::Json(json!({}))),
            }
        }
    })))
    .await
}

#[tokio::test]
async fn message_agent_and_send_files_obey_the_rules() {
    let calls = Calls::default();
    let url = hermes(calls.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["agents"] = json!({
        "Quotidien": {"hermes_url": format!("{url}/p/quotidien"), "hermes_key": "q".repeat(32)},
        "Coach": {"hermes_url": format!("{url}/p/coach"), "hermes_key": "c".repeat(32)},
    });
    let mut config = parse_config(&d, None).unwrap();
    let http = reqwest::Client::new();
    let ask = |text: &str| {
        let mut args = bipbridge::Dict::new();
        args.insert("target".into(), json!("bip/coach"));
        args.insert("message".into(), json!(text));
        args
    };
    let said = |result: &bipbridge::Dict| result["content"][0]["text"].as_str().unwrap_or_default().to_string();
    let give = |config: &bipbridge::config::Config| {
        let mut args = bipbridge::Dict::new();
        args.insert("agent".into(), json!("coach"));
        args.insert("text".into(), json!("la liste"));
        args.insert("text_name".into(), json!("liste.md"));
        said(&bipbridge::agent_files::call(config, config.agent(Some("quotidien")).unwrap(), &args))
    };
    let quotidien = config.agent(Some("quotidien")).unwrap().clone();
    // « Aucun »: refused, nothing reaches Coach
    rules::change(&config, |r| r.write = Mode::None).unwrap();
    assert!(!bipbridge::agent_messages::offered(&config, "quotidien"));
    let called = bipbridge::agent_messages::call(&http, None, &config, &quotidien, &ask("Combien ?")).await;
    assert!(said(&called.result).contains("hasn't let you write to Coach"), "{}", said(&called.result));
    assert!(give(&config).contains("can't pass things on to Coach"), "{}", give(&config));
    assert!(calls.0.lock().unwrap().is_empty());
    // « Tous »: asked, answered
    rules::change(&config, |r| r.write = Mode::All).unwrap();
    assert!(bipbridge::agent_messages::offered(&config, "quotidien"));
    let called = bipbridge::agent_messages::call(&http, None, &config, &quotidien, &ask("Combien ?")).await;
    assert!(said(&called.result).contains("Sept heures."), "{}", said(&called.result));
    assert!(!give(&config).contains("can't pass things on"), "{}", give(&config));
    // Coach « à l'écart »: refused again
    rules::change(&config, |r| r.aside = vec!["coach".into()]).unwrap();
    let called = bipbridge::agent_messages::call(&http, None, &config, &quotidien, &ask("Et demain ?")).await;
    assert!(said(&called.result).contains("hasn't let you write to Coach"), "{}", said(&called.result));
    // a new agent is covered at once
    rules::change(&config, |r| r.aside.clear()).unwrap();
    let mut jardin = config.agent(Some("coach")).unwrap().clone();
    jardin.name = "jardin".into();
    jardin.display_name = "Jardin".into();
    config.agents.insert("jardin".into(), jardin);
    let targets = rules::local_targets(&config, "quotidien");
    assert_eq!(targets, ["coach", "jardin"]);
}

type Commands = Arc<Mutex<Vec<Vec<String>>>>;

fn recorder() -> (Run, Commands) {
    let commands: Commands = Arc::default();
    let seen = commands.clone();
    let run: Run = Arc::new(move |args: Vec<String>, _env: BTreeMap<String, String>| {
        seen.lock().unwrap().push(args);
        Box::pin(async move { Ok(String::new()) })
    });
    (run, commands)
}

/// The volumes set for `agent` by the last commands, if any.
fn volumes(commands: &Commands, agent: &str) -> Option<Value> {
    commands
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|a| a.len() > 6 && a[1] == "-p" && a[2] == agent && a[5] == "terminal.docker_volumes")
        .map(|a| serde_json::from_str(&a[6]).unwrap())
}

#[tokio::test]
async fn the_shared_folders_are_mounted_again_once_per_change() {
    let dir = tempfile::tempdir().unwrap();
    let hub = dir.path().join(".hermes");
    for name in ["vie", "wellness"] {
        let folder = hub.join("profiles").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("config.yaml"), "terminal:\n  backend: docker\n").unwrap();
    }
    let mut d = config_dict(dir.path(), &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&hub), "hub_url": "http://127.0.0.1:8642"});
    d["agents"] = json!({});
    for name in ["Vie", "Wellness"] {
        d["agents"][name] = json!({"hermes_url": format!("http://127.0.0.1:8642/p/{}", name.to_lowercase()),
                                   "hermes_key": "k".repeat(32)});
    }
    let config = SharedConfig::new(parse_config(&d, None).unwrap());
    let c = config.get();
    let (run, commands) = recorder();
    let root = dir.path().join("data/shares");
    // « lisent les dossiers partagés »: Tous → both mounted, once
    rules::change(&c, |r| r.read = Mode::All).unwrap();
    assert_eq!(shares::sync(&c, &run).await.unwrap(), pairs(&[("vie", "wellness"), ("wellness", "vie")]));
    assert_eq!(
        volumes(&commands, "vie"),
        Some(json!([format!("{}/vie:/partage:z", s(&root)), format!("{}/wellness:/lecture/wellness:ro,z", s(&root))]))
    );
    assert!(volumes(&commands, "wellness").is_some());
    // nothing changed (another tap, a change of who writes): nothing mounted again
    commands.lock().unwrap().clear();
    shares::sync(&c, &run).await.unwrap();
    rules::change(&c, |r| r.write = Mode::None).unwrap();
    shares::sync(&c, &run).await.unwrap();
    assert!(commands.lock().unwrap().is_empty(), "{:?}", commands.lock().unwrap());
    // Wellness « à l'écart »: out of the mounts
    rules::change(&c, |r| r.aside = vec!["wellness".into()]).unwrap();
    assert!(shares::sync(&c, &run).await.unwrap().is_empty());
    assert_eq!(volumes(&commands, "vie"), Some(json!([])));
}

#[tokio::test]
async fn the_pairs_of_before_become_rules_once_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let peers = json!({"links": [["vie", "wellness"], ["wellness", "budget__lea"]], "tokens": {}});
    std::fs::write(data.join("peers.json"), peers.to_string()).unwrap();
    std::fs::write(data.join("shares.json"), r#"{"reads": [["vie", "wellness"]]}"#).unwrap();
    std::fs::write(data.join("households.json"), r#"{"households": {"lea": {"name": "Léa", "url": "http://127.0.0.1:9", "key_out": "o"}}}"#).unwrap();
    let app = three(dir.path());
    let config = app.app.state.config();
    let made = rules::load(&config);
    assert_eq!((made.write, made.read), (Mode::Some, Mode::Some));
    assert_eq!(made.pairs, pairs(&[("vie", "wellness")]));
    assert_eq!(made.reads, pairs(&[("vie", "wellness")]));
    assert_eq!(made.households["lea"].out, rules::Rule::only(["budget"]));
    let notes = made.migrated.as_ref().unwrap()["notes"].as_array().unwrap().clone();
    assert!(notes.iter().any(|n| n.as_str().unwrap().contains("write to budget too now")), "{notes:?}");
    // the files of before stay as they were (a bridge rolled back finds them)
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(data.join("peers.json")).unwrap()).unwrap();
    assert_eq!(kept["links"], peers["links"]);
    // a second start changes nothing
    let text = std::fs::read_to_string(rules::path(&config)).unwrap();
    app.shutdown().await;
    let again = three(dir.path());
    assert_eq!(std::fs::read_to_string(rules::path(&again.app.state.config())).unwrap(), text);
}

// -- two households -------------------------------------------------------------------------------------------------

type Received = Arc<Mutex<Vec<(String, Value)>>>;

/// A bridge served on a socket of its own, its agents in a fake Hermes (`<hermes>/<name>/p/<agent>`).
async fn serve_bridge(dir: &Path, name: &str, agents: &[&str], hermes: &str) -> TestApp {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut d = config_dict(dir, &Urls::default());
    d["public_url"] = json!(url);
    d["outbox"] = json!({"db_path": s(&dir.join(name).join("bridge.db"))});
    let mut table = serde_json::Map::new();
    for a in agents {
        let title = format!("{}{}", a[..1].to_uppercase(), &a[1..]);
        table.insert(title, json!({"hermes_url": format!("{hermes}/{name}/p/{a}"), "hermes_key": format!("{a}-key-{}", "x".repeat(24))}));
    }
    d["agents"] = Value::Object(table);
    let app = bipbridge::app::create_app(parse_config(&d, None).unwrap(), bipbridge::app::AppOptions { start_background: false })
        .unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    TestApp { app, peer: "198.51.100.7:50000".parse().unwrap() }
}

fn way<'a>(screen: &'a Value, which: &str) -> &'a Value {
    &screen["households"][0][which]
}

fn digest(token: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

/// A gateway token for `agent` of this bridge (set up by hand: `manual:<agent>`).
fn gateway_token(app: &TestApp, agent: &str) -> String {
    let token = format!("{agent}-gateway-token-{}", "z".repeat(20));
    let links = &app.app.state.links;
    let mut data = links.load();
    data["tokens"].as_object_mut().unwrap().insert(format!("manual:{agent}"), json!(digest(&token)));
    links.save(&data).unwrap();
    token
}

#[tokio::test]
async fn two_households_agree_way_by_way() {
    let received: Received = Arc::default();
    let seen = received.clone();
    let hermes = spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = seen.clone();
        async move {
            let (parts, body) = request.into_parts();
            let path = parts.uri.path().to_string();
            if parts.method == "GET" && path.ends_with("/api/sessions") {
                return axum::Json(json!({"data": [{"id": "bot-1", "title": "Bot Chat"}]}));
            }
            if parts.method == "POST" && path.ends_with("/api/sessions") {
                return axum::Json(json!({"session": {"id": "pair-1"}}));
            }
            let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            seen.lock().unwrap().push((path, body));
            axum::Json(json!({"ok": true}))
        }
    })))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let a = serve_bridge(dir.path(), "a", &["vie", "budget"], &hermes).await;
    let b = serve_bridge(dir.path(), "b", &["cuisine", "jardin"], &hermes).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(a.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Alex"})).await.status, 200);
    assert_eq!(b.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Léa"})).await.status, 200);
    let invitation = a.request("POST", "/v1/family/invite", &h, None).await.json();
    let joined = b
        .send_json("POST", "/v1/family/join", &h, &json!({"url": invitation["url"], "code": invitation["code"]}))
        .await;
    assert_eq!(joined.status, 200, "{}", joined.text());
    // both sides know nothing of each other's rules yet: nothing works between them
    let screen = a.get("/v1/agent-rules", &h).await.json();
    assert_eq!(screen["households"][0]["id"], "lea");
    assert_eq!((way(&screen, "in")["state"].clone(), way(&screen, "out")["state"].clone()), (json!("off"), json!("off")));
    let names: Vec<&str> = screen["households"][0]["agents"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(names, ["cuisine", "jardin"], "her agents, as her bridge lists them");

    // Alex opens Vie to Léa's agents: « en attente de Léa »
    let set = a.send_json("PUT", "/v1/agent-rules/households/lea", &h, &json!({"way": "in", "mode": "only", "agents": ["vie"]})).await;
    assert_eq!(set.status, 200, "{}", set.text());
    assert_eq!(way(&set.json(), "in")["state"], "waiting");
    // Léa's Boîte: « Alex ouvre son agent Vie à tes agents »
    let screen = b.get("/v1/agent-rules", &h).await.json();
    assert_eq!(way(&screen, "out")["state"], "proposed");
    let items = b.get("/v1/approvals", &h).await.json()["items"].clone();
    let card = items.as_array().unwrap().iter().find(|i| i["run_id"].as_str().unwrap().starts_with("bip-rules-")).unwrap().clone();
    assert_eq!(card["description"], "Alex ouvre son agent Vie à tes agents");
    assert_eq!(card["bip_rules"]["agents"], json!([{"id": "vie", "name": "Vie"}]));
    assert_eq!(card["choices"], json!(["once", "deny"]));
    // what crossed was signed with the link's own key: an unsigned side is refused
    let key = b.app.state.households.get("alex").unwrap()["key_out"].as_str().unwrap().to_string();
    let forged = a
        .send_json("POST", "/family/rules", &[("authorization", &format!("Bearer {key}"))],
                   &json!({"rules": {"v": 1, "from_you": {"mode": "all"}, "to_you": {"mode": "all"}}}))
        .await;
    assert_eq!(forged.status, 403, "{}", forged.text());
    // « D'accord »: both sides agree, the card goes
    let answered = b.send_json("POST", "/v1/approve", &h, &json!({"agent": card["agent"], "run_id": card["run_id"], "choice": "once"})).await;
    assert_eq!(answered.status, 200, "{}", answered.text());
    let items = b.get("/v1/approvals", &h).await.json()["items"].clone();
    assert!(!items.as_array().unwrap().iter().any(|i| i["run_id"].as_str().unwrap().starts_with("bip-rules-")));
    let screen = a.get("/v1/agent-rules", &h).await.json();
    assert_eq!(way(&screen, "in")["state"], "agreed");
    assert!(links(&screen).contains(&("cuisine__lea".into(), "vie".into())));
    assert_eq!(way(&b.get("/v1/agent-rules", &h).await.json(), "out")["state"], "agreed");

    // Cuisine writes to Vie (Alex): delivered; to Budget, refused on Léa's side already
    let token = gateway_token(&b, "cuisine");
    let bearer = format!("Bearer {token}");
    let message = json!({"session_id": "bot-1", "message": "Message from Cuisine (@cuisine): on mange quoi ?"});
    let sent = b.send_json("POST", "/peer/p/vie__alex/v1/runs", &[("authorization", &bearer)], &message).await;
    assert_eq!(sent.status, 200, "{}", sent.text());
    assert!(received.lock().unwrap().iter().any(|(p, _)| p == "/a/p/vie/v1/runs"));
    let refused = b.send_json("POST", "/peer/p/budget__alex/v1/runs", &[("authorization", &bearer)], &message).await;
    assert_eq!(refused.status, 403);
    // and Alex's bridge checks again: Vie taken back on his side, Léa's call is refused there
    let set = a.send_json("PUT", "/v1/agent-rules/households/lea", &h, &json!({"way": "in", "mode": "none"})).await;
    assert_eq!(way(&set.json(), "in")["state"], "off");
    let direct = a
        .send_json("POST", "/family/p/vie/v1/runs",
                   &[("authorization", &format!("Bearer {key}")), ("x-bip-sender", "cuisine")], &message)
        .await;
    assert_eq!(direct.status, 403, "{}", direct.text());

    // Alex proposes his agents write to Léa's Cuisine; she says « Non merci »: he sees it, nothing works
    a.send_json("PUT", "/v1/agent-rules/households/lea", &h, &json!({"way": "out", "mode": "only", "agents": ["cuisine"]})).await;
    let card = common::answer_card(&b, "bip-rules-in-", "deny").await;
    assert_eq!(card["description"], "Alex propose que ses agents écrivent à ton agent Cuisine");
    let screen = a.get("/v1/agent-rules", &h).await.json();
    assert_eq!(way(&screen, "out")["state"], "declined");
    assert!(!links(&screen).iter().any(|(_, to)| to == "cuisine__lea"));
    assert!(b.get("/v1/approvals", &h).await.json()["items"].as_array().unwrap().iter()
        .all(|i| !i["run_id"].as_str().unwrap().starts_with("bip-rules-")));

    // unlinked: the rules go
    assert_eq!(b.request("DELETE", "/v1/family/alex", &h, None).await.status, 204);
    assert!(rules::load(&b.app.state.config()).households.is_empty());
}
