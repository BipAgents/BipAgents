//! The family's agent asks one of its creator's agents (Alex 21/11: « Famille » → Vie ticked in « Qui peut écrire à
//! qui », and the family agent answered « l'outil de messagerie entre agents n'est pas disponible »). The link the
//! person ticks IS the consent (« si la personne donne le droit à écrire, même pas besoin de carte »): during a family
//! member's turn the family's agent calls `message_agent`, the target answers in the pair conversation « Famille ⇄
//! Vie », told who asks and for whom, and the answer comes back in the same call, for the family agent's reply in the
//! family's common conversation. An agent the links don't allow, and another household's, are refused, nothing sent.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bipbridge::agent_tools;
use bipbridge::config::parse_config;
use common::{auth, config_dict, spawn_fake, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

#[derive(Clone, Default)]
struct Fake {
    /// (method, path, body) of each call.
    calls: Arc<Mutex<Vec<(String, String, Value)>>>,
}

/// The agents' Hermes: Vie's turn answers the planning; every other agent's would answer something else.
async fn hermes(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let n = {
        let mut calls = fake.calls.lock().unwrap();
        calls.push((method.clone(), path.clone(), body));
        calls.len()
    };
    match (method.as_str(), path.as_str()) {
        ("POST", p) if p.ends_with("/api/sessions") => {
            (axum::http::StatusCode::CREATED, axum::Json(json!({"session": {"id": format!("s{n}")}}))).into_response()
        }
        ("PATCH", _) => axum::Json(json!({})).into_response(),
        ("POST", p) if p.ends_with("/v1/runs") => {
            (axum::http::StatusCode::ACCEPTED, axum::Json(json!({"run_id": format!("r{n}")}))).into_response()
        }
        ("GET", p) if p.starts_with("/p/vie/v1/runs/") => {
            axum::Json(json!({"status": "completed", "output": "Lundi : piscine à 18 h. Mercredi : dentiste."}))
                .into_response()
        }
        ("GET", p) if p.contains("/v1/runs/") => {
            axum::Json(json!({"status": "completed", "output": "Autre chose."})).into_response()
        }
        _ => (axum::http::StatusCode::NOT_FOUND, "no").into_response(),
    }
}

fn runs_of(fake: &Fake, agent: &str) -> Vec<Value> {
    let path = format!("/p/{agent}/v1/runs");
    fake.calls.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && *p == path).map(|(_, _, b)| b.clone()).collect()
}

async fn mcp(app: &TestApp, token: &str, method: &str, params: Value) -> Value {
    let bearer = format!("Bearer {token}");
    let answer = app
        .send_json(
            "POST",
            "/v1/agent-tools/mcp",
            &[("authorization", &bearer)],
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()["result"].clone()
}

fn tool_names(result: &Value) -> Vec<String> {
    result["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn a_member_asks_the_family_agent_and_it_asks_the_creators_agent_the_links_allow() {
    let fake = Fake::default();
    let url = spawn_fake(Router::new().fallback(hermes).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["agents"] = json!({
        "Famille": {"hermes_url": format!("{url}/p/famille"), "hermes_key": "f".repeat(32)},
        "Vie": {"hermes_url": format!("{url}/p/vie"), "hermes_key": "v".repeat(32)},
        "Coach": {"hermes_url": format!("{url}/p/coach"), "hermes_key": "c".repeat(32)},
    });
    let config = parse_config(&d, None).unwrap();
    let data = std::path::PathBuf::from(config.data_dir());
    std::fs::create_dir_all(&data).unwrap();
    // « Famille » is this place's family agent (Alex made it)
    std::fs::write(data.join("family-agent.json"), r#"{"agent": "famille", "agents": ["famille"]}"#).unwrap();
    let app = TestApp::new(config.clone());
    let mut tokens: IndexMap<String, String> = agent_tools::load_tokens(&config);
    tokens.insert("famille".into(), agent_tools::digest("famille-token"));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let asks = bipbridge::agent_messages::TOOL_NAME;

    // nobody it may write to: no message_agent, and called anyway it is closed like the family's other closed tools
    let listed = tool_names(&mcp(&app, "famille-token", "tools/list", json!({})).await);
    assert!(!listed.iter().any(|n| n == asks), "{listed:?}");
    let closed = mcp(&app, "famille-token", "tools/call", json!({"name": asks, "arguments": {"target": "vie", "message": "x"}})).await;
    assert_eq!(closed["isError"], json!(true));
    assert!(runs_of(&fake, "vie").is_empty());

    // Alex ticks Famille → Vie in « Qui peut écrire à qui »; the links screen knows Famille is the family's agent
    let set = app.send_json("PUT", "/v1/agent-links", &h, &json!({"links": [{"from": "famille", "to": "vie"}]})).await;
    assert_eq!(set.status, 200, "{}", set.text());
    let page = app.get("/v1/agent-links", &h).await.json();
    let famille = page["agents"].as_array().unwrap().iter().find(|a| a["id"] == json!("famille")).unwrap().clone();
    assert_eq!(famille["family"], json!(true), "{page}");
    let vie = page["agents"].as_array().unwrap().iter().find(|a| a["id"] == json!("vie")).unwrap().clone();
    assert!(vie.get("family").is_none(), "{vie}");
    let listed = tool_names(&mcp(&app, "famille-token", "tools/list", json!({})).await);
    assert!(listed.iter().any(|n| n == asks), "{listed:?}");

    // Marie, a member of the family, writes in the family's common conversation: her turn goes on (family_thread.rs
    // `started` notes it in her household's entry)
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    std::fs::write(
        data.join("agent-shares.json"),
        json!({"conversations": {"f-famille-marie": {"agent": "famille", "household": "marie", "person": "Marie",
                                                      "family": true, "run": {"id": "run-marie", "started": started}}}})
        .to_string(),
    )
    .unwrap();

    // the family's agent asks Vie: answered in the same call
    let message = "Marie demande : quel est le planning des enfants cette semaine ?";
    let answered =
        mcp(&app, "famille-token", "tools/call", json!({"name": asks, "arguments": {"target": "bip/vie", "message": message}}))
            .await;
    assert_eq!(answered["isError"], json!(false), "{answered}");
    assert_eq!(text_of(&answered), "Vie (bip/vie) answered:\nLundi : piscine à 18 h. Mercredi : dentiste.");

    // Vie read it in its own conversation of the pair, told it is the family's agent, for Marie, read by the family
    let runs = runs_of(&fake, "vie");
    assert_eq!(runs.len(), 1);
    let input = runs[0]["input"].as_str().unwrap();
    assert!(input.contains("This is your own conversation with Famille (@famille), the family's agent"), "{input}");
    assert!(input.contains("for Marie, a member of the family"), "{input}");
    assert!(input.contains("every member of the family reads"), "{input}");
    assert!(input.ends_with(&format!("Message from Famille (@famille): {message}")), "{input}");
    let id = bipbridge::agent_pairs::pair_id("famille", "vie");
    let pair = bipbridge::agent_pairs::get(&config, &id).unwrap();
    assert_eq!(pair.sessions.get("vie").map(String::as_str), Some("s1"));
    let kinds: Vec<_> = pair.messages.iter().map(|m| (m.from.as_str(), m.kind.as_str())).collect();
    assert_eq!(kinds, [("famille", "ask"), ("vie", "answer")]);
    // the person (Alex) reads « Famille ⇄ Vie » in the app
    let pairs = app.get("/v1/agent-pairs?agent=vie", &h).await.json();
    assert_eq!(pairs["pairs"][0]["messages"], json!(2), "{pairs}");

    // an agent the links don't allow: refused, the member told who can allow it, nothing sent
    let refused =
        mcp(&app, "famille-token", "tools/call", json!({"name": asks, "arguments": {"target": "coach", "message": "Et toi ?"}}))
            .await;
    assert_eq!(refused["isError"], json!(true));
    assert!(text_of(&refused).contains("hasn't let you write to Coach"), "{}", text_of(&refused));
    assert!(runs_of(&fake, "coach").is_empty());

    // another household's agent: never from here
    let other = mcp(
        &app,
        "famille-token",
        "tools/call",
        json!({"name": asks, "arguments": {"target": "bip/cuisine__marie", "message": "On mange quoi ?"}}),
    )
    .await;
    assert_eq!(other["isError"], json!(true));
    assert!(text_of(&other).contains("Another household's agents aren't reached from here"), "{}", text_of(&other));
    let posted: Vec<_> = fake.calls.lock().unwrap().iter().filter(|(m, _, _)| m == "POST").map(|(_, p, _)| p.clone()).collect();
    assert!(posted.iter().all(|p| p.starts_with("/p/vie/")), "{posted:?}");
}
