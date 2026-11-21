//! The agents group's Python tests: test_spaces.py, test_own_skills.py, test_services.py, test_shares.py,
//! test_peers.py, test_households.py, test_ssh_keys.py (parsing; the routes are in agents_ssh_keys.rs) and the files
//! part of test_http_routes.py.

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::routing::any;
use axum::Router;
use bipbridge::agents_admin::{self, pyyaml, AgentSettings, NewAgent, Run};
use bipbridge::config::{parse_config, Config, SharedConfig};
use bipbridge::households::{HouseholdError, Households};
use bipbridge::peers::{self, Links};
use bipbridge::rules;
use bipbridge::{files, own_skills, services, shares, ssh_keys};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

type Commands = Arc<Mutex<Vec<(Vec<String>, BTreeMap<String, String>)>>>;

fn recorder() -> (Run, Commands) {
    recorder_with(|_| String::new())
}

fn recorder_with(answer: impl Fn(&[String]) -> String + Send + Sync + 'static) -> (Run, Commands) {
    let commands: Commands = Arc::default();
    let seen = commands.clone();
    let answer = Arc::new(answer);
    let run: Run = Arc::new(move |args: Vec<String>, env: BTreeMap<String, String>| {
        let out = answer(&args);
        seen.lock().unwrap().push((args, env));
        Box::pin(async move { Ok(out) })
    });
    (run, commands)
}

fn args(commands: &Commands) -> Vec<Vec<String>> {
    commands.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A fake Hermes answering every request with `{}` (200; 201 for a POST).
async fn any_gateway() -> String {
    common::spawn_fake(Router::new().fallback(any(|request: Request| async move {
        let status = if request.method() == "POST" { 201 } else { 200 };
        if request.uri().path().ends_with("/v1/toolsets") {
            return (axum::http::StatusCode::OK, axum::Json(json!({"data": []})));
        }
        (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(json!({})))
    })))
    .await
}

// -- spaces -----------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_agent_moves_into_its_own_space_and_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let hub = tmp.join(".hermes");
    let folder = hub.join("profiles/budget");
    std::fs::create_dir_all(folder.join("memories")).unwrap();
    std::fs::write(folder.join("memories/MEMORY.md"), "Aime les tableaux.").unwrap();
    std::fs::write(folder.join("SOUL.md"), "Tu es Budget.").unwrap();
    std::fs::write(folder.join("gateway.pid"), "123").unwrap(); // runtime files stay behind
    {
        let db = rusqlite::Connection::open(folder.join("state.db")).unwrap();
        db.execute_batch("create table messages (content text); insert into messages values ('bonjour');").unwrap();
    }
    std::fs::write(hub.join("config.yaml"), "model:\n  default: glm\n").unwrap();
    std::fs::write(hub.join(".env"), "OPENROUTER_API_KEY=or-key\nAPI_SERVER_PORT=8642\nAPI_SERVER_KEY=hub-key\n")
        .unwrap();
    let config_path = tmp.join("bridge.toml");
    std::fs::write(&config_path, "# bridge\n").unwrap();
    let mut d = config_dict(tmp, &Urls::default());
    d["public_url"] = json!("https://box.ts.net:8643");
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&hub), "hub_url": "http://127.0.0.1:8642",
                         "python": "/opt/py -I", "root": "/opt/hermes-agent", "isolated": true});
    d["spaces"] = json!({"root": s(&tmp.join("spaces")), "first_port": 18660, "sandbox": "none",
                         "start": "start-space {agent} {home}", "stop": "stop-space {agent}",
                         "container_home": "/home/hermes/.hermes"});
    d["agents"]["Budget"] = json!({"hermes_url": "http://127.0.0.1:8642/p/budget", "hermes_key": "b".repeat(32)});
    let config = SharedConfig::new(parse_config(&d, Some(&s(&config_path))).unwrap());
    let (run, commands) = recorder();
    // The new gateway, reached through a proxy: it checks the agent keeps its own key.
    let keys: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = keys.clone();
    let gateway = common::spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().push(request.headers().get("authorization").unwrap().to_str().unwrap().to_string());
            axum::Json(json!({}))
        }
    })))
    .await;
    let http = reqwest::Client::builder().proxy(reqwest::Proxy::http(&gateway).unwrap()).build().unwrap();
    let c = config.get();
    let moved = bipbridge::spaces::Spaces::new(run.clone())
        .move_agent(&c, c.agent(Some("budget")).unwrap(), &http)
        .await
        .unwrap();
    assert!(keys.lock().unwrap().iter().all(|k| *k == format!("Bearer {}", "b".repeat(32))));
    let space = tmp.join("spaces/budget");
    let port = moved.hermes_url.split(':').nth(2).unwrap().split('/').next().unwrap().parse::<u16>().unwrap();
    assert!(port >= 18660);
    assert!(
        moved.hermes_url == format!("http://127.0.0.1:{port}/p/budget")
            && moved.isolated
            && moved.space.as_deref() == Some(&*s(&space))
    );
    assert_eq!(moved.public_url.as_deref(), Some("https://box.ts.net:8643/hermes/budget")); // through the bridge
    assert_eq!(moved.python, ["/opt/py", "-I"]);
    assert_eq!(moved.hermes_home.as_deref(), Some(&*s(&space.join("profiles/budget"))));
    assert_eq!(moved.upload_dir_host.as_deref(), Some(&*s(&space.join("profiles/budget/uploads"))));
    assert_eq!(moved.upload_dir_container.as_deref(), Some("/home/hermes/.hermes/profiles/budget/uploads"));
    let env = std::fs::read_to_string(space.join(".env")).unwrap();
    assert!(
        env.contains("OPENROUTER_API_KEY=or-key")
            && env.contains(&format!("API_SERVER_PORT={port}"))
            && !env.contains("hub-key")
    );
    assert_eq!(
        std::fs::read_to_string(space.join("profiles/budget/memories/MEMORY.md")).unwrap(),
        "Aime les tableaux."
    );
    assert!(!space.join("profiles/budget/gateway.pid").exists());
    {
        let db = rusqlite::Connection::open(space.join("profiles/budget/state.db")).unwrap();
        let content: String = db.query_row("select content from messages", [], |r| r.get(0)).unwrap();
        assert_eq!(content, "bonjour");
    }
    let ran = commands.lock().unwrap().clone();
    let settings = std::fs::read_to_string(space.join("profiles/budget/config.yaml")).unwrap(); // no `hermes config set`
    assert!(settings.contains("gateway:\n  strict: true\n") && settings.contains("browser:\n  backend: 'off'\n"), "{settings}");
    assert_eq!(ran[0].0[..2], ["sh", "-c"]);
    assert!(ran[0].0[2].contains("start-space budget"));
    assert_eq!(
        ran.last().unwrap(),
        &(
            strs(&["/opt/hermes", "profile", "delete", "budget", "--yes"]),
            BTreeMap::from([("HERMES_HOME".into(), s(&hub))])
        )
    );

    agents_admin::save_agent(&config, &moved).unwrap();
    assert!(moved.sandboxed); // its own start command: a container
    let c = config.get();
    assert_eq!(bipbridge::vault::holder(&c, c.agent(Some("budget")).unwrap()), None); // may now hold credentials
    let mut unsandboxed = moved.clone();
    unsandboxed.sandboxed = false;
    assert_eq!(bipbridge::vault::holder(&c, &unsandboxed).as_deref(), Some("needs_sandbox"));
    assert_eq!(agents_admin::managed(&c, "budget").unwrap(), moved);
    assert_eq!(agents_admin::profile_folder(&c, &moved), space.join("profiles/budget"));
    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(text.contains("space = ") && text.contains("isolated = true"));
    let mut data = bipbridge::config::toml_to_json(text.parse::<toml::Value>().unwrap());
    data["bridge_key"] = json!(common::BRIDGE_KEY);
    data["pocket"] = json!({"url": "http://p"});
    let reread = parse_config(&data, None).unwrap();
    assert!(
        reread.agent(Some("budget")).unwrap().space.as_deref() == Some(&*s(&space))
            && reread.agent(Some("budget")).unwrap().isolated
    );

    commands.lock().unwrap().clear();
    agents_admin::delete_agent(&config, &moved, &run).await.unwrap();
    assert!(args(&commands)[0][2].contains("stop-space budget"));
    assert!(!space.exists() && config.get().agent(Some("budget")).is_none());
}

/// own_space_back.rs: an agent that left the hub only because other people talk to it comes back (Alex 21/11). Its
/// space as the way out left it (its own .env, its profile with its conversations, memory and Hermes vault, runtime
/// files); the hub's tombstone of it from that move.
#[tokio::test]
async fn an_agent_comes_back_into_the_hub_and_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let hub = tmp.join(".hermes");
    std::fs::create_dir_all(hub.join("profiles/.deleted")).unwrap();
    std::fs::write(hub.join("profiles/.deleted/budget"), "deleted\n").unwrap(); // Hermes' mark from the move out
    let space = tmp.join("spaces/budget");
    let folder = space.join("profiles/budget");
    for sub in ["memories", "vault", "sandboxes/s1", "uploads"] {
        std::fs::create_dir_all(folder.join(sub)).unwrap();
    }
    std::fs::write(space.join(".env"), "API_SERVER_PORT=18660\nAPI_SERVER_KEY=space-key\n").unwrap();
    std::fs::write(folder.join("memories/MEMORY.md"), "Aime les tableaux. Appris dans son espace.").unwrap();
    std::fs::write(folder.join("SOUL.md"), "Tu es Budget.").unwrap();
    std::fs::write(folder.join("config.yaml"), "terminal:\n  backend: docker\ngateway:\n  strict: true\n").unwrap();
    std::fs::write(folder.join("vault/vault.json.enc"), "its Hermes vault").unwrap();
    std::fs::write(folder.join("gateway.pid"), "123").unwrap(); // its space's running gateway: never copied
    std::fs::write(folder.join("uploads/releve.pdf"), "%PDF").unwrap();
    let space_id = std::fs::canonicalize(&folder).unwrap().to_string_lossy().into_owned();
    {
        let db = rusqlite::Connection::open(folder.join("state.db")).unwrap();
        db.execute_batch(
            "create table messages (content text); insert into messages values ('bonjour'), ('vu depuis mon espace');
             create table state_meta (key text primary key, value text);",
        )
        .unwrap();
        let receipt = json!({"profile_id": space_id, "session_id": "api_1", "route": "agent:main:api_server:dm:api_1",
                             "entry": {"session_id": "api_1"}});
        db.execute(
            "insert into state_meta values (?1, ?2)",
            rusqlite::params![format!("{}api_1", bipbridge::spaces::API_BINDING_PREFIX), receipt.to_string()],
        )
        .unwrap();
    }
    let config_path = tmp.join("bridge.toml");
    std::fs::write(&config_path, "# bridge\n").unwrap();
    let mut d = config_dict(tmp, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&hub), "hub_url": "http://127.0.0.1:8642",
                         "python": "/opt/py -I", "root": "/opt/hermes-agent", "isolated": true});
    d["spaces"] = json!({"root": s(&tmp.join("spaces")), "first_port": 18660, "sandbox": "none",
                         "start": "start-space {agent} {home}", "stop": "stop-space {agent}"});
    d["agents"]["Budget"] = json!({"hermes_url": "http://127.0.0.1:18660/p/budget", "hermes_key": "b".repeat(32),
                                   "isolated": true, "sandboxed": true, "space": s(&space), "python": ["/opt/py", "-I"],
                                   "hermes_home": s(&folder), "upload_dir_host": s(&folder.join("uploads")),
                                   "upload_dir_container": s(&folder.join("uploads"))});
    let config = SharedConfig::new(parse_config(&d, Some(&s(&config_path))).unwrap());
    let before = config.get().agent(Some("budget")).unwrap().clone();
    // `hermes profile delete` as Hermes does it: its folder gone, its tombstone written
    let deleted = hub.clone();
    let (run, commands) = recorder_with(move |args| {
        if args.get(1..3) == Some(&["profile".to_string(), "delete".to_string()][..]) {
            let _ = std::fs::remove_dir_all(deleted.join("profiles").join(&args[3]));
            std::fs::write(deleted.join("profiles/.deleted").join(&args[3]), "deleted\n").unwrap();
        }
        String::new()
    });
    // the hub, reached through a proxy: it serves the agent with its own key
    let keys: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = keys.clone();
    let gateway = common::spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = seen.clone();
        async move {
            let key = request.headers().get("authorization").unwrap().to_str().unwrap().to_string();
            seen.lock().unwrap().push(format!("{} {key}", request.uri()));
            axum::Json(json!({}))
        }
    })))
    .await;
    let http = reqwest::Client::builder().proxy(reqwest::Proxy::http(&gateway).unwrap()).build().unwrap();
    let spaces = bipbridge::spaces::Spaces::new(run.clone());
    let back = spaces.copy_back(&config.get(), &before, &http).await.unwrap();
    assert_eq!(
        keys.lock().unwrap().last().unwrap(),
        &format!("http://127.0.0.1:8642/p/budget/v1/capabilities Bearer {}", "b".repeat(32))
    );
    // its section as the hub's agents have it, the rest kept
    assert_eq!(back.hermes_url, "http://127.0.0.1:8642/p/budget");
    assert!(!back.isolated && !back.sandboxed && back.space.is_none() && back.python.is_empty());
    assert!(back.hermes_home.is_none() && back.hermes_root.is_none());
    let copied = hub.join("profiles/budget");
    assert_eq!(back.upload_dir_host.as_deref(), Some(&*s(&copied.join("uploads"))));
    assert_eq!(back.upload_dir_container.as_deref(), Some(&*s(&copied.join("uploads"))));
    assert_eq!((back.hermes_key.as_str(), back.display_name.as_str()), (before.hermes_key.as_str(), before.display_name.as_str()));
    // its memory, personality, files and Hermes vault (not a place whose copies are in memory) with it
    assert_eq!(
        std::fs::read_to_string(copied.join("memories/MEMORY.md")).unwrap(),
        "Aime les tableaux. Appris dans son espace."
    );
    assert_eq!(std::fs::read_to_string(copied.join("SOUL.md")).unwrap(), "Tu es Budget.");
    assert_eq!(std::fs::read_to_string(copied.join("vault/vault.json.enc")).unwrap(), "its Hermes vault");
    assert_eq!(std::fs::read_to_string(copied.join("uploads/releve.pdf")).unwrap(), "%PDF");
    assert!(!copied.join("gateway.pid").exists() && !copied.join("sandboxes").exists());
    // its conversations, the API ones the hub's again
    {
        let db = rusqlite::Connection::open(copied.join("state.db")).unwrap();
        let rows: Vec<String> = db
            .prepare("select content from messages")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, ["bonjour", "vu depuis mon espace"]);
        let value: String = db.query_row("select value from state_meta", [], |r| r.get(0)).unwrap();
        let receipt: Value = serde_json::from_str(&value).unwrap();
        let hub_id = std::fs::canonicalize(&copied).unwrap().to_string_lossy().into_owned();
        assert_eq!(receipt["profile_id"], json!(hub_id));
        assert_eq!(receipt["route"], json!("agent:main:api_server:dm:api_1"));
    }
    // Hermes serves the name again: its tombstone gone, nothing half copied left beside it
    assert!(!hub.join("profiles/.deleted/budget").exists());
    let mut left: Vec<String> = std::fs::read_dir(hub.join("profiles"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, [".deleted", "budget"]);
    // its space still serves it until the bridge switches: nothing stopped, nothing deleted
    assert!(space.join("profiles/budget/SOUL.md").exists());
    assert!(args(&commands).is_empty(), "{:?}", args(&commands));

    // Again, as after a restart that cut a way back off: the hub's copy deleted through Hermes first, then the space's
    // copy taken again (what it learned since included).
    std::fs::write(folder.join("memories/MEMORY.md"), "Aime les tableaux. Et les graphiques.").unwrap();
    std::fs::write(copied.join("memories/MEMORY.md"), "a copy the hub had").unwrap();
    let again = spaces.copy_back(&config.get(), &before, &http).await.unwrap();
    assert_eq!(again, back);
    assert_eq!(args(&commands), [strs(&["/opt/hermes", "profile", "delete", "budget", "--yes"])]);
    assert_eq!(commands.lock().unwrap()[0].1, BTreeMap::from([("HERMES_HOME".into(), s(&hub))]));
    assert_eq!(
        std::fs::read_to_string(copied.join("memories/MEMORY.md")).unwrap(),
        "Aime les tableaux. Et les graphiques."
    );
    assert!(!hub.join("profiles/.deleted/budget").exists());

    // The bridge switches: its section rewritten as the hub's agents have it, then its space stopped and removed.
    agents_admin::save_agent(&config, &back).unwrap();
    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(!text.contains("space = ") && !text.contains("isolated = true") && text.contains("hermes_url = \"http://127.0.0.1:8642/p/budget\""), "{text}");
    let c = config.get();
    assert!(bipbridge::vault::in_hub(&c, c.agent(Some("budget")).unwrap()));
    assert_eq!(agents_admin::profile_folder(&c, &back), copied);
    commands.lock().unwrap().clear();
    let from = bipbridge::config::AgentConfig { name: "budget".into(), space: Some(s(&space)), ..Default::default() };
    spaces.remove(&c, &from).await.unwrap();
    assert!(args(&commands)[0][2].contains("stop-space budget"));
    assert!(!space.exists() && copied.join("SOUL.md").exists());
}

// -- own skills -------------------------------------------------------------------------------------------------------

fn skill(folder: &Path, name: &str, description: &str, frontmatter: bool) {
    std::fs::create_dir_all(folder).unwrap();
    let head =
        if frontmatter { format!("---\nname: {name}\ndescription: {description}\n---\n") } else { String::new() };
    std::fs::write(folder.join("SKILL.md"), head + "# Steps\n").unwrap();
}

fn make_profile(tmp: &Path) -> PathBuf {
    let profile = tmp.join("profiles/budget");
    let skills = profile.join("skills");
    skill(&skills.join("productivity/arxiv"), "arxiv", "Search papers", true); // shipped with Hermes
    skill(&skills.join("hub-thing"), "hub-thing", "Does a thing", true); // from the hub
    skill(&skills.join("shared-data"), "shared-data", "Does a thing", true); // the bridge's
    skill(&skills.join("finance/monthly-budget"), "monthly-budget", "Sums  the\n month's spending", true);
    skill(&skills.join("plant-care"), "plant-care", "Waters the ficus", true);
    skill(&skills.join("plant-care/templates/nested"), "nested", "Does a thing", true); // a file inside a skill
    skill(&skills.join(".archive/old"), "old", "Does a thing", true); // archived
    skill(&skills.join("no-front"), "x", "Does a thing", false); // name from its folder
    std::fs::write(skills.join(".bundled_manifest"), "arxiv:abc123\n").unwrap();
    std::fs::create_dir(skills.join(".hub")).unwrap();
    std::fs::write(skills.join(".hub/lock.json"), json!({"installed": {"hub-thing": {"source": "hub"}}}).to_string())
        .unwrap();
    std::fs::write(
        profile.join("config.yaml"),
        "model:\n  provider: custom\nskills:\n  disabled: [plant-care, arxiv]\n",
    )
    .unwrap();
    profile
}

#[test]
fn only_the_agents_own_skills_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let found = own_skills::listing(&make_profile(dir.path()));
    let names: Vec<&str> = found.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["monthly-budget", "no-front", "plant-care"]);
    assert_eq!(
        Value::Object(found[0].clone()),
        json!({"name": "monthly-budget", "description": "Sums the month's spending", "enabled": true})
    );
    assert_eq!(found[2]["enabled"], false);
    assert!(own_skills::listing(&dir.path().join("nowhere")).is_empty());
}

#[test]
fn turning_skills_on_and_off_keeps_the_rest_of_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let profile = make_profile(dir.path());
    let changes: IndexMap<String, bool> =
        [("plant-care".to_string(), true), ("monthly-budget".to_string(), false)].into_iter().collect();
    own_skills::set_enabled(&profile, &changes).unwrap();
    let data = pyyaml::to_json(&pyyaml::load(&std::fs::read_to_string(profile.join("config.yaml")).unwrap()).unwrap());
    assert_eq!(data["model"], json!({"provider": "custom"}));
    assert_eq!(data["skills"]["disabled"], json!(["arxiv", "monthly-budget"])); // Hermes' own choice stays
    let states: BTreeMap<String, bool> = own_skills::listing(&profile)
        .iter()
        .map(|s| (s["name"].as_str().unwrap().to_string(), s["enabled"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        states,
        BTreeMap::from([("monthly-budget".into(), false), ("no-front".into(), true), ("plant-care".into(), true)])
    );
    let arxiv: IndexMap<String, bool> = [("arxiv".to_string(), true)].into_iter().collect();
    let err = own_skills::set_enabled(&profile, &arxiv).unwrap_err(); // not the agent's: not this switch's
    assert!(err.downcast_ref::<agents_admin::LookupError>().is_some());
}

#[tokio::test]
async fn settings_show_and_change_the_agents_skills() {
    let hub = any_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    std::fs::write(&path, "# bridge config\n").unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&dir.path().join(".hermes")), "hub_url": hub, "public_hub_url": "https://mini.ts.net:8642"});
    let config = SharedConfig::new(parse_config(&d, Some(&s(&path))).unwrap());
    let (run, _) = recorder();
    let http = reqwest::Client::new();
    let body: NewAgent = serde_json::from_value(json!({"name": "Budget", "kind": "finance"})).unwrap();
    let agent = agents_admin::create_agent(&config, &body, &http, &run).await.unwrap();
    skill(&dir.path().join(".hermes/profiles/budget/skills/receipts"), "receipts", "Files receipts", true);
    let before = agents_admin::read_settings(&config.get(), &agent, &http).await.unwrap();
    let off: AgentSettings = serde_json::from_value(json!({"skills": {"receipts": false}})).unwrap();
    agents_admin::write_settings(&config, &agent, &off, &run).await.unwrap();
    let after = agents_admin::read_settings(&config.get(), &agent, &http).await.unwrap();
    let bad: AgentSettings = serde_json::from_value(json!({"skills": {"../etc": false}})).unwrap();
    let err = agents_admin::write_settings(&config, &agent, &bad, &run).await.unwrap_err();
    assert!(err
        .downcast_ref::<agents_admin::AgentError>()
        .unwrap()
        .0
        .starts_with("not one of the agent's own skills: "));
    assert_eq!(before["skills"], json!([{"name": "receipts", "description": "Files receipts", "enabled": true}]));
    assert_eq!(before["browser"], Value::Null); // this install has no browser for its agents: Hermes' own tool shows
    assert_eq!(after["skills"][0]["enabled"], false);
}

// -- services ---------------------------------------------------------------------------------------------------------

fn services_setup(dir: &Path) -> (Config, PathBuf) {
    let hub = dir.join(".hermes");
    std::fs::create_dir_all(hub.join("profiles/maison")).unwrap();
    let mut d = config_dict(dir, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&hub), "hub_url": "http://127.0.0.1:8642", "isolated": true});
    d["agents"]["Maison"] = json!({"hermes_url": "http://127.0.0.1:8642/p/maison", "hermes_key": "m".repeat(32)});
    (parse_config(&d, None).unwrap(), hub.join("profiles/maison"))
}

fn ha_runner() -> (Run, Commands) {
    recorder_with(|args| {
        if args.last().map(String::as_str) == Some("ha") && args.iter().any(|a| a == "test") {
            "  Transport: HTTP\n  ✓ Connected (318ms)\n  ✓ Tools discovered: 7\n".to_string()
        } else {
            String::new()
        }
    })
}

fn dict(value: Value) -> bipbridge::Dict {
    value.as_object().unwrap().clone()
}

#[tokio::test]
async fn a_service_with_a_token_needs_the_agents_own_space() {
    let dir = tempfile::tempdir().unwrap();
    let (config, folder) = services_setup(dir.path());
    let agent = config.agent(Some("maison")).unwrap().clone();
    let (run, commands) = ha_runner();
    let body = dict(json!({"catalog": "home_assistant", "address": "homeassistant.local:8123", "token": "ha-secret"}));
    assert_eq!(services::plan(&body).unwrap()["url"], "https://homeassistant.local:8123/api/mcp");
    let err = services::add(&config, &agent, &body, &run).await.unwrap_err();
    assert!(matches!(
        err.downcast_ref::<bipbridge::vault::VaultError>(),
        Some(bipbridge::vault::VaultError::CannotHold(_))
    ));
    assert!(commands.lock().unwrap().is_empty() && !folder.join(".env").exists());
    // Without a token and not personal: fine in the shared hub.
    services::add(
        &config,
        &agent,
        &dict(json!({"catalog": "custom", "name": "Météo", "address": "https://meteo.example/mcp"})),
        &run,
    )
    .await
    .unwrap();
    // written into its profile, its tools that write asking the person first
    let profile = std::fs::read_to_string(folder.join("config.yaml")).unwrap();
    assert!(profile.contains("meteo:\n    url: https://meteo.example/mcp\n    trust: untrusted\n"), "{profile}");
}

#[tokio::test]
async fn the_token_goes_to_the_env_file_never_on_a_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let (config, folder) = services_setup(dir.path());
    let mut agent = config.agent(Some("maison")).unwrap().clone();
    agent.isolated = true;
    agent.python = vec!["/opt/py".into()];
    agent.hermes_home = Some(s(&folder));
    let (run, commands) = ha_runner();
    let result = services::add(
        &config,
        &agent,
        &dict(
            json!({"catalog": "home_assistant", "name": "ha", "address": "http://ha.local:8123", "token": "ha-secret"}),
        ),
        &run,
    )
    .await
    .unwrap();
    assert_eq!(result["catalog"], "home_assistant");
    assert!(args(&commands).iter().all(|c| !c.join(" ").contains("ha-secret")));
    assert!(std::fs::read_to_string(folder.join(".env")).unwrap().contains("MCP_HOME_ASSISTANT_API_KEY=ha-secret"));
    assert_eq!(mode(&folder.join(".env")), 0o600);
    let profile = std::fs::read_to_string(folder.join("config.yaml")).unwrap();
    assert!(profile.contains("Authorization: Bearer ${MCP_HOME_ASSISTANT_API_KEY}"), "{profile}");
    std::fs::write(
        folder.join("config.yaml"),
        "mcp_servers:\n  ha:\n    url: http://ha.local:8123/api/mcp\n  garmin:\n    command: uvx\n",
    )
    .unwrap();
    let listed: Vec<(Value, Value)> =
        services::listing(&config, &agent).iter().map(|s| (s["name"].clone(), s["catalog"].clone())).collect();
    assert_eq!(listed, vec![(json!("ha"), json!("home_assistant")), (json!("garmin"), Value::Null)]);
    assert_eq!(
        Value::Object(services::test(&config, &agent, "ha", &run).await.unwrap()),
        json!({"ok": true, "message": null, "tools": 7})
    );
    services::remove(&config, &agent, "garmin", &run).await.unwrap();
    assert!(!std::fs::read_to_string(folder.join("config.yaml")).unwrap().contains("garmin"));
    let err = services::remove(&config, &agent, "nope", &run).await.unwrap_err();
    assert!(err.downcast_ref::<services::UnknownService>().is_some());
}

#[test]
fn what_the_form_must_hold() {
    assert!(services::plan(&dict(json!({"catalog": "github"}))).is_err());
    assert!(services::plan(&dict(json!({"catalog": "custom", "name": "x", "address": "ftp://x"}))).is_err());
    assert!(services::plan(&dict(json!({"catalog": "nope"}))).is_err());
    assert_eq!(
        services::plan(&dict(json!({"catalog": "github", "token": "t"}))).unwrap()["url"],
        "https://api.githubcopilot.com/mcp"
    );
    assert_eq!(services::catalog_of("https://api.githubcopilot.com/mcp").as_deref(), Some("github"));
    assert_eq!(services::catalog_of("https://x.org/mcp").as_deref(), Some("custom"));
}

// -- shares -----------------------------------------------------------------------------------------------------------

fn shares_setup(tmp: &Path) -> (SharedConfig, PathBuf) {
    let hub = tmp.join(".hermes");
    for (name, terminal) in [("vie", "docker"), ("wellness", "docker"), ("budget", "local")] {
        let folder = hub.join("profiles").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let volumes = if name == "vie" { "\n  docker_volumes:\n    - \"/home/x/photos:/photos:ro\"" } else { "" };
        std::fs::write(folder.join("config.yaml"), format!("terminal:\n  backend: {terminal}{volumes}\n")).unwrap();
    }
    let mut d = config_dict(tmp, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&hub), "hub_url": "http://127.0.0.1:8642", "isolated": true});
    d["outbox"] = json!({"db_path": s(&tmp.join("data/bridge.db"))});
    for name in ["vie", "wellness", "budget"] {
        let uploads = s(&hub.join("profiles").join(name).join("uploads"));
        let title = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
        d["agents"][title] = json!({"hermes_url": format!("http://127.0.0.1:8642/p/{name}"), "hermes_key": "k".repeat(32),
                                    "upload_dir_host": uploads, "upload_dir_container": uploads});
    }
    (SharedConfig::new(parse_config(&d, None).unwrap()), hub)
}

fn volumes_set(commands: &Commands, agent: &str) -> Value {
    let ran = args(commands);
    let found = ran
        .iter()
        .rev()
        .find(|a| a.len() > 6 && a[1..6] == strs(&["-p", agent, "config", "set", "terminal.docker_volumes"]))
        .unwrap();
    serde_json::from_str(&found[6]).unwrap()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

#[tokio::test]
async fn an_agent_reads_what_another_shares_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let (config, hub) = shares_setup(dir.path());
    let c = config.get();
    let (run, commands) = recorder();
    let root = dir.path().join("data/shares");
    shares::apply(&c, &pairs(&[("vie", "wellness")]), &run).await.unwrap();
    // Wellness writes in /partage; Vie reads it, read-only; the person's own mount is kept.
    assert_eq!(volumes_set(&commands, "wellness"), json!([format!("{}/wellness:/partage:z", s(&root))]));
    assert_eq!(
        volumes_set(&commands, "vie"),
        json!(["/home/x/photos:/photos:ro", format!("{}/wellness:/lecture/wellness:ro,z", s(&root))])
    );
    assert!(root.join("wellness").is_dir() && mode(&root.join("wellness")) == 0o777);
    assert!(hub.join("profiles/vie/skills/shared-data/SKILL.md").exists());
    assert_eq!(shares::load(&c), pairs(&[("vie", "wellness")]));
    // Taken back: the mounts go, the person's stays.
    commands.lock().unwrap().clear();
    shares::apply(&c, &[], &run).await.unwrap();
    assert_eq!(volumes_set(&commands, "vie"), json!(["/home/x/photos:/photos:ro"]));
    assert_eq!(volumes_set(&commands, "wellness"), json!([]));
    assert!(!hub.join("profiles/vie/skills/shared-data").exists());
}

#[tokio::test]
async fn only_agents_in_a_sandbox_take_part() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = shares_setup(dir.path());
    let c = config.get();
    let (run, commands) = recorder();
    assert_eq!(shares::why_not(&c, c.agent(Some("budget")).unwrap()).as_deref(), Some("no_sandbox"));
    for wanted in [pairs(&[("budget", "wellness")]), pairs(&[("vie", "vie")])] {
        let err = shares::apply(&c, &wanted, &run).await.unwrap_err();
        assert!(err.downcast_ref::<shares::InvalidShare>().is_some());
    }
    assert!(commands.lock().unwrap().is_empty() && shares::load(&c).is_empty());
}

#[tokio::test]
async fn deleting_an_agent_stops_the_others_reading_it() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = shares_setup(dir.path());
    let (run, commands) = recorder();
    shares::apply(&config.get(), &pairs(&[("vie", "wellness")]), &run).await.unwrap();
    config.remove_agent("wellness");
    commands.lock().unwrap().clear();
    shares::forget(&config.get(), "wellness", &run).await.unwrap();
    assert_eq!(volumes_set(&commands, "vie"), json!(["/home/x/photos:/photos:ro"]));
    assert!(shares::load(&config.get()).is_empty() && !dir.path().join("data/shares/wellness").exists());
}

#[tokio::test]
async fn shares_routes() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let listing = app.get("/v1/agent-shares", &[(k, &v)]).await.json();
    assert_eq!(
        listing,
        json!({"agents": [{"id": "wellness", "name": "Wellness", "why_not": "not_managed", "external": false}], "reads": []})
    );
    let bad = app
        .send_json("PUT", "/v1/agent-shares", &[(k, &v)], &json!({"reads": [{"reader": "wellness", "source": "x"}]}))
        .await;
    assert_eq!(bad.status, 400);
}

#[tokio::test]
async fn an_agent_set_up_by_hand_mounts_its_folders_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = shares_setup(dir.path());
    let mut vie = config.get().agent(Some("vie")).unwrap().clone();
    vie.hermes_url = "http://127.0.0.1:8644".into();
    vie.shares = "external".into();
    vie.shares_apply = Some("restart-when-idle {agent}".into());
    config.put_agent(vie.clone());
    let c = config.get();
    let (run, commands) = recorder();
    assert_eq!(shares::why_not(&c, &vie), None);
    shares::apply(&c, &pairs(&[("vie", "wellness")]), &run).await.unwrap();
    let root = dir.path().join("data/shares");
    // and its outbox, for send_files_to_agent (agent_files.rs)
    assert_eq!(
        shares::mounts(&c, "vie", None),
        [
            format!("{}/wellness:/lecture/wellness:ro,z", s(&root)),
            format!("{}/envoi/vie:/envoi:z", s(&dir.path().join("data")))
        ]
    );
    assert_eq!(shares::mounts(&c, "wellness", None), [format!("{}/wellness:/partage:z", s(&root))]);
    assert!(root.join("wellness").is_dir());
    let restart = (strs(&["sh", "-c", "restart-when-idle vie"]), BTreeMap::new());
    assert!(commands.lock().unwrap().contains(&restart));
    assert!(!args(&commands).iter().any(|a| a[1..3] == strs(&["-p", "vie"]))); // nothing written in its config
    commands.lock().unwrap().clear();
    shares::apply(&c, &pairs(&[("vie", "wellness")]), &run).await.unwrap(); // unchanged: not restarted
    assert!(!commands.lock().unwrap().contains(&restart));
}

#[test]
fn a_profile_without_its_own_terminal_setting_runs_in_the_hubs_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let (config, hub) = shares_setup(dir.path());
    let c = config.get();
    let budget = c.agent(Some("budget")).unwrap();
    std::fs::write(hub.join("profiles/budget/config.yaml"), "model:\n  provider: custom\n").unwrap();
    assert_eq!(shares::why_not(&c, budget).as_deref(), Some("no_sandbox"));
    std::fs::write(hub.join(".env"), "API_SERVER_ENABLED=true\nTERMINAL_ENV=docker\n").unwrap();
    assert_eq!(shares::why_not(&c, budget), None);
    std::fs::write(hub.join("profiles/budget/.env"), "TERMINAL_ENV=local\n").unwrap();
    assert_eq!(shares::why_not(&c, budget).as_deref(), Some("no_sandbox")); // the profile's own word wins
}

#[tokio::test]
async fn a_sandboxed_agent_opens_the_files_it_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (config, hub) = shares_setup(dir.path());
    let (run, commands) = recorder();
    let uploads = hub.join("profiles/vie/uploads");
    std::fs::create_dir_all(uploads.join("2026-10")).unwrap();
    let old = uploads.join("2026-10/0123456789ab-avant.pdf");
    std::fs::write(&old, b"%PDF").unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o640)).unwrap();
    agents_admin::mount_all_uploads(config.clone(), run.clone()).await;
    // Its sandbox sees nothing of this machine but its mounts: the uploads join the person's own, read-only.
    assert_eq!(
        volumes_set(&commands, "vie"),
        json!(["/home/x/photos:/photos:ro", format!("{}:/fichiers:ro,z", s(&uploads))])
    );
    assert_eq!(config.get().agent(Some("vie")).unwrap().upload_dir_container.as_deref(), Some("/fichiers"));
    assert_eq!(mode(&uploads), 0o755);
    assert_eq!(mode(&old), 0o644);
    // Tools that run here: nothing to mount, the path stays the machine's.
    assert_eq!(
        config.get().agent(Some("budget")).unwrap().upload_dir_container.as_deref(),
        Some(&*s(&hub.join("profiles/budget/uploads")))
    );
    // Hermes' `config set` commands only: where SELinux enforces (the build server) the sandbox's own folders are
    // labelled too (`stat`, `chcon`: sandbox::label_hermes_folders), a host step of its own
    let hermes = |commands: &Commands| -> Vec<Vec<String>> {
        args(commands).into_iter().filter(|a| a.len() > 6 && a[1] == "-p" && a[3..5] == strs(&["config", "set"])).collect()
    };
    assert!(!hermes(&commands).iter().any(|a| a[2] == "budget"));
    // Again (each start), once Hermes holds them: nothing changes.
    for a in hermes(&commands) {
        std::fs::write(
            hub.join("profiles").join(&a[2]).join("config.yaml"),
            format!("terminal:\n  backend: docker\n  docker_volumes: {}\n", a[6]),
        )
        .unwrap();
    }
    commands.lock().unwrap().clear();
    agents_admin::mount_all_uploads(config.clone(), run.clone()).await;
    assert!(hermes(&commands).is_empty());
    // A file sent now: named as the sandbox sees it, readable by the sandbox's user.
    let source = dir.path().join("spooled");
    std::fs::write(&source, b"%PDF-1.7").unwrap();
    let c = config.get();
    let stored =
        files::store_upload(c.agent(Some("vie")).unwrap(), &source, "statuts.pdf", None, &c.limits, None).unwrap();
    let path = stored["path"].as_str().unwrap();
    assert!(path.starts_with("/fichiers/") && path.ends_with("-statuts.pdf"));
    let on_disk = uploads.join(path.trim_start_matches("/fichiers/"));
    assert_eq!(std::fs::read(&on_disk).unwrap(), b"%PDF-1.7");
    assert_eq!(mode(&on_disk), 0o644);
}

// -- peers ------------------------------------------------------------------------------------------------------------

struct PeersSetup {
    config: Config,
    links: Links,
    links_path: PathBuf,
    hub: PathBuf,
    space: PathBuf,
}

fn peers_setup(tmp: &Path) -> PeersSetup {
    let hub = tmp.join(".hermes");
    let space = tmp.join("spaces/jardin");
    for folder in [hub.join("profiles/budget"), hub.join("profiles/maison"), space.join("profiles/jardin")] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let mut d = config_dict(tmp, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": s(&hub), "hub_url": "http://127.0.0.1:8642"});
    d["agents"]["Budget"] = json!({"hermes_url": "http://127.0.0.1:8642/p/budget", "hermes_key": "b".repeat(32)});
    d["agents"]["Maison"] = json!({"hermes_url": "http://127.0.0.1:8642/p/maison", "hermes_key": "m".repeat(32)});
    d["agents"]["Jardin"] = json!({"hermes_url": "http://127.0.0.1:8660/p/jardin", "hermes_key": "j".repeat(32),
                                   "isolated": true, "sandboxed": true, "space": s(&space)});
    let links_path = tmp.join("peers.json");
    PeersSetup { config: parse_config(&d, None).unwrap(), links: Links::new(&s(&links_path)), links_path, hub, space }
}

fn env_token(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.split("HERMES_PEER_BIP_KEY=").nth(1).map(|t| t.lines().next().unwrap_or("").trim().to_string())
}

/// Whether a profile has Hermes' Bot Mode mark (`ui_meta.hermes-bots`).
fn marked(profile: &Path) -> bool {
    std::fs::read_to_string(profile.join("profile.yaml")).unwrap_or_default().contains("hermes-bots")
}

/// Léa's household linked to this one (households.json as `join` leaves it).
fn lea(tmp: &Path) -> Households {
    let households = Households::new(&s(&tmp.join("households.json")));
    let mut data = households.load();
    data.insert("households".into(), json!({"lea": {"name": "Léa", "url": "https://b.ts.net", "key_out": "o".repeat(40)}}));
    households.save(&data).unwrap();
    households
}

#[test]
fn bot_mode_is_off_even_towards_another_household() {
    let dir = tempfile::tempdir().unwrap();
    let p = peers_setup(dir.path());
    let households = lea(&PathBuf::from(p.config.data_dir()));
    let (budget, maison, jardin) =
        (p.hub.join("profiles/budget"), p.hub.join("profiles/maison"), p.space.join("profiles/jardin"));
    // a place made before 21/11: every profile marked, the hub and the space with the bridge as their peer and a key
    for profile in [&budget, &maison, &jardin] {
        std::fs::write(profile.join("profile.yaml"), "description: x\nui_meta:\n  hermes-bots: {}\n").unwrap();
    }
    for gateway in [&p.hub, &p.space] {
        std::fs::write(gateway.join("config.yaml"), "model:\n  default: glm\nbot_peers:\n  bip:\n    url: http://127.0.0.1:1/peer\n").unwrap();
        std::fs::write(gateway.join(".env"), "HERMES_PEER_BIP_KEY=old\nOTHER=1\n").unwrap();
    }
    let digest = |t: &str| {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(t.as_bytes()))
    };
    let frozen = json!([["jardin", "cuisine__lea"]]);
    std::fs::write(&p.links_path, json!({"links": frozen, "tokens": {s(&p.hub): digest("old"), s(&p.space): digest("old"),
        "manual:wellness": digest("w")}}).to_string()).unwrap();
    // At start: Bot Mode off everywhere, the pairs of before kept as they were
    peers::drop_bot_mode(&p.config, &p.links);
    let off = || {
        !marked(&budget) && !marked(&maison) && !marked(&jardin)
            && env_token(&p.hub.join(".env")).is_none() && env_token(&p.space.join(".env")).is_none()
            && ![&p.hub, &p.space].iter().any(|g| std::fs::read_to_string(g.join("config.yaml")).unwrap().contains("bip:"))
    };
    assert!(off());
    assert!(std::fs::read_to_string(p.hub.join(".env")).unwrap().contains("OTHER=1"));
    assert!(std::fs::read_to_string(p.hub.join("config.yaml")).unwrap().contains("glm"));
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&p.links_path).unwrap()).unwrap();
    assert_eq!((kept["tokens"].clone(), kept["links"].clone()), (json!({}), frozen));
    // Your agents write to Léa's Cuisine and she agrees: still no Bot Mode (the bridge's message_agent goes there)
    let both = pairs(&[("budget", "jardin"), ("jardin", "cuisine__lea")]);
    let result = peers::apply(&p.config, &p.links, &both, Some(&households)).unwrap();
    assert_eq!((result["manual"].clone(), result["changed"].clone()), (json!({}), json!(["lea"])));
    rules::change(&p.config, |r| {
        rules::heard(r, "lea", &rules::Wire { from_you: rules::Rule::only(["cuisine"]), ..Default::default() });
    })
    .unwrap();
    peers::refresh(&p.config, &p.links, Some(&households)).unwrap();
    assert!(off());
    assert!(rules::allows(&p.config, "maison", "cuisine__lea"));
    let contacts = std::fs::read_to_string(p.hub.join("profiles/maison/skills/agent-contacts/SKILL.md")).unwrap();
    assert!(contacts.contains("`bip/cuisine__lea`: cuisine, an agent of Léa's household"), "{contacts}");
    let err = peers::apply(&p.config, &p.links, &pairs(&[("budget", "budget")]), None).unwrap_err();
    assert!(err.downcast_ref::<peers::InvalidLink>().is_some());
}

#[test]
fn at_start_a_place_loses_bot_mode_between_its_own_agents() {
    let dir = tempfile::tempdir().unwrap();
    let p = peers_setup(dir.path());
    let budget = p.hub.join("profiles/budget");
    std::fs::write(budget.join("profile.yaml"), "ui_meta:\n  hermes-bots: {}\n").unwrap();
    std::fs::write(p.hub.join("config.yaml"), "bot_peers:\n  bip:\n    url: http://127.0.0.1:1/peer\n").unwrap();
    std::fs::write(p.hub.join(".env"), "HERMES_PEER_BIP_KEY=old\n").unwrap();
    let digest = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(b"old"))
    };
    std::fs::write(&p.links_path, json!({"links": [["budget", "maison"]], "tokens": {s(&p.hub): digest}}).to_string())
        .unwrap();
    peers::drop_bot_mode(&p.config, &p.links);
    assert!(!marked(&budget) && env_token(&p.hub.join(".env")).is_none());
    assert!(!std::fs::read_to_string(p.hub.join("config.yaml")).unwrap().contains("bip"));
    // that file is not the data folder's: no pairs there, several agents: nothing allowed (review 1, B2)
    assert!(!rules::allows(&p.config, "budget", "maison"));
}

#[tokio::test]
async fn the_bot_modes_doors_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    for path in ["/peer/p/wellness/api/sessions", "/family/p/wellness/v1/runs"] {
        assert_eq!(app.get(path, &[("authorization", "Bearer x")]).await.status, 404, "{path}");
    }
    let (k, v) = auth();
    assert_eq!(
        app.get("/v1/agent-links", &[(k, &v)]).await.json()["agents"][0],
        json!({"id": "wellness", "name": "Wellness", "managed": false})
    );
}

#[test]
fn each_sender_learns_whom_it_may_write_to() {
    let dir = tempfile::tempdir().unwrap();
    let p = peers_setup(dir.path());
    peers::apply(&p.config, &p.links, &pairs(&[("budget", "jardin"), ("budget", "maison")]), None).unwrap();
    let contacts = std::fs::read_to_string(p.hub.join("profiles/budget/skills/agent-contacts/SKILL.md")).unwrap();
    assert!(contacts.contains("`bip/jardin`: Jardin") && contacts.contains("`bip/maison`: Maison"));
    assert!(!p.space.join("profiles/jardin/skills/agent-contacts").exists()); // writes to nobody
    peers::apply(&p.config, &p.links, &[], None).unwrap();
    assert!(!p.hub.join("profiles/budget/skills/agent-contacts/SKILL.md").exists());
}

/// Alex 21/11: the family's agent (« Budget » here) said « mon carnet de contacts indique que je peux écrire à Vie,
/// mais l'outil … n'est pas disponible ». Its contacts list only the agents of this place it may ask, with
/// `message_agent` (no files from it), and nothing of another household's: such a link is dropped, its Bot Mode never
/// set (another household's agents stay out of its reach).
#[test]
fn the_familys_agent_learns_only_this_places_agents_it_may_ask() {
    let dir = tempfile::tempdir().unwrap();
    let p = peers_setup(dir.path());
    let households = lea(&PathBuf::from(p.config.data_dir()));
    let data = std::path::PathBuf::from(p.config.data_dir());
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("family-agent.json"), r#"{"agent": "budget", "agents": ["budget"]}"#).unwrap();
    let wanted = pairs(&[("budget", "maison"), ("budget", "cuisine__lea"), ("maison", "cuisine__lea")]);
    peers::apply(&p.config, &p.links, &wanted, Some(&households)).unwrap();
    // the family's agent → Léa's agent: not kept (rules.rs: its « Peut demander à » is this place's agents); Maison's
    // way to Léa's Cuisine is (it works once Léa agrees)
    assert!(rules::allows(&p.config, "budget", "maison") && !rules::allows(&p.config, "budget", "cuisine__lea"));
    assert_eq!(rules::load(&p.config).households["lea"].out, rules::Rule::only(["cuisine"]));
    assert!(!marked(&p.hub.join("profiles/budget")) && !marked(&p.hub.join("profiles/maison")), "no Bot Mode at all");
    let contacts = std::fs::read_to_string(p.hub.join("profiles/budget/skills/agent-contacts/SKILL.md")).unwrap();
    assert!(contacts.contains("`bip/maison`: Maison"), "{contacts}");
    assert!(!contacts.contains("cuisine__lea") && !contacts.contains("send_files_to_agent"), "{contacts}");
    assert!(contacts.contains("every member of the family reads"), "{contacts}");
    // a link kept from before (made before the rules): still no Bot Mode at the start, still not listed
    std::fs::write(&p.links_path, json!({"links": [["budget", "cuisine__lea"]], "tokens": {}}).to_string()).unwrap();
    peers::drop_bot_mode(&p.config, &p.links);
    assert!(!marked(&p.hub.join("profiles/budget")));
    peers::refresh(&p.config, &p.links, Some(&households)).unwrap();
    let contacts = std::fs::read_to_string(p.hub.join("profiles/budget/skills/agent-contacts/SKILL.md")).unwrap();
    assert!(!contacts.contains("cuisine__lea"), "{contacts}");
}

// -- households -------------------------------------------------------------------------------------------------------

#[test]
fn invitations_are_one_time_and_expire() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("households.json");
    let households = Households::new(&s(&path));
    let err = households.invite().unwrap_err(); // name the household first
    assert!(err.downcast_ref::<HouseholdError>().is_some());
    households.set_name("Alex").unwrap();
    let code = households.invite().unwrap();
    let key = "k".repeat(40);
    let answer = households.accept(&code, "Léa", "https://b.ts.net", &key, None, None).unwrap();
    assert_eq!(households.caller(&key).as_deref(), Some("lea"));
    assert!(answer["key"].as_str().unwrap().len() >= 32 && answer["name"] == "Alex");
    assert!(!std::fs::read_to_string(&path).unwrap().contains(&key)); // only a digest of their key
    assert!(households.accept(&code, "Paul", "https://b.ts.net", &key, None, None).is_err()); // used once
    let late = households.invite().unwrap();
    let mut data = households.load();
    let past = bipbridge::py::time_time() - 1.0;
    let invites: bipbridge::Dict =
        data["invites"].as_object().unwrap().keys().map(|k| (k.clone(), json!(past))).collect();
    data.insert("invites".into(), Value::Object(invites));
    households.save(&data).unwrap();
    assert!(households.accept(&late, "Paul", "https://c.ts.net", &"p".repeat(40), None, None).is_err());
}

// -- ssh keys ---------------------------------------------------------------------------------------------------------

#[test]
fn only_a_plain_public_key_goes_in() {
    let good = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me";
    assert_eq!(ssh_keys::parse(good).unwrap().kind, "ssh-ed25519");
    for bad in [
        format!("command=\"rm -rf ~\" {good}"),
        format!("from=\"*\" {good}"),
        "ssh-dss AAAAB3NzaC1kc3MAAACBAP me".into(),
        "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQC me".into(),
        "ssh-ed25519 not-base64!".into(),
        "-----BEGIN OPENSSH PRIVATE KEY-----".into(),
        format!("{good}\nssh-rsa AAAA second"),
    ] {
        assert!(ssh_keys::parse(&bad).is_err(), "{bad}");
    }
}

// -- files (test_http_routes.py) --------------------------------------------------------------------------------------

fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "b0undaryXYZ";
    let mut body = Vec::new();
    for (name, filename, data) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(f) => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(app: &TestApp, parts: &[(&str, Option<&str>, &[u8])]) -> common::TestResponse {
    let (content_type, body) = multipart(parts);
    let (k, v) = auth();
    app.request("POST", "/v1/files", &[(k, &v), ("content-type", &content_type)], Some(axum::body::Body::from(body)))
        .await
}

#[tokio::test]
async fn upload_maps_path_into_container() {
    let dir = tempfile::tempdir().unwrap();
    let config = common::config(dir.path(), &Urls::default());
    let host = config.agents["wellness"].upload_dir_host.clone().unwrap();
    let app = TestApp::new(config);
    let resp =
        upload(&app, &[("agent", None, b"wellness"), ("file", Some("Relevé de compte (oct).pdf"), b"%PDF-1.4 data")])
            .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    let month = chrono::Utc::now().format("%Y-%m").to_string();
    assert_eq!(body["filename"], "Relevé_de_compte_(oct).pdf");
    assert_eq!(body["size"], 13);
    let prefix = format!("/home/hermes/.hermes/profiles/wellness/uploads/{month}/");
    let path = body["path"].as_str().unwrap();
    assert!(path.starts_with(&prefix));
    let stored = &path[prefix.len()..];
    assert!(stored.ends_with("-Relevé_de_compte_(oct).pdf") && stored.split('-').next().unwrap().len() == 12);
    let host_path = Path::new(&host).join(&month).join(stored);
    assert_eq!(std::fs::read(&host_path).unwrap(), b"%PDF-1.4 data");
    assert_eq!(mode(&host_path), 0o640);
}

#[tokio::test]
async fn upload_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let config = common::config(dir.path(), &Urls::default());
    let too_big = vec![b'x'; config.limits.upload_max_bytes as usize + 1];
    let host = config.agents["wellness"].upload_dir_host.clone().unwrap();
    let app = TestApp::new(config);
    assert_eq!(upload(&app, &[("agent", None, b"wellness"), ("file", Some("big.bin"), &too_big)]).await.status, 413);
    let month_dir = Path::new(&host).join(chrono::Utc::now().format("%Y-%m").to_string());
    assert!(!month_dir.is_dir() || std::fs::read_dir(&month_dir).unwrap().next().is_none());
}

#[tokio::test]
async fn upload_size_limit_without_content_length() {
    let dir = tempfile::tempdir().unwrap();
    let config = common::config(dir.path(), &Urls::default());
    let half = config.limits.upload_max_bytes as usize / 2;
    let app = TestApp::new(config);
    let mut chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![
        Ok(b"--b0undary\r\nContent-Disposition: form-data; name=\"agent\"\r\n\r\nwellness\r\n".to_vec()),
        Ok(b"--b0undary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\n".to_vec()),
    ];
    for _ in 0..3 {
        chunks.push(Ok(vec![b'y'; half]));
    }
    chunks.push(Ok(b"\r\n--b0undary--\r\n".to_vec()));
    let body = axum::body::Body::from_stream(futures::stream::iter(chunks));
    let (k, v) = auth();
    let resp = app
        .request(
            "POST",
            "/v1/files",
            &[(k, &v), ("content-type", "multipart/form-data; boundary=b0undary")],
            Some(body),
        )
        .await;
    assert_eq!(resp.status, 413);
}

#[tokio::test]
async fn upload_errors() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    assert_eq!(upload(&app, &[("agent", None, b"inconnu"), ("file", Some("a.txt"), b"x")]).await.status, 404);
    assert_eq!(upload(&app, &[("agent", None, b"wellness")]).await.status, 400);
}

#[test]
fn sanitize_filename() {
    let long = "é".repeat(200) + ".pdf";
    let cut = "é".repeat(116) + ".pdf";
    for (raw, expected) in [
        ("../../etc/passwd", "passwd"),
        ("C:\\Users\\alex\\notes.txt", "notes.txt"),
        (".bashrc", "bashrc"),
        ("rapport final\n2026.xlsx", "rapport_final2026.xlsx"),
        ("a;b|c$(rm).sh", "a_b_c_(rm).sh"),
        ("", "fichier"),
        ("..", "fichier"),
        (long.as_str(), cut.as_str()),
    ] {
        assert_eq!(files::sanitize_filename(raw), expected, "{raw}");
    }
}

#[test]
fn purge_only_old_bridge_files() {
    let dir = tempfile::tempdir().unwrap();
    let month = dir.path().join("2026-01");
    std::fs::create_dir(&month).unwrap();
    let (old, new, foreign) =
        (month.join("0123456789ab-old.pdf"), month.join("0123456789ac-new.pdf"), month.join("notes.txt"));
    for f in [&old, &new, &foreign] {
        std::fs::write(f, b"x").unwrap();
    }
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86400);
    for f in [&old, &foreign] {
        std::fs::File::options().write(true).open(f).unwrap().set_modified(long_ago).unwrap();
    }
    assert_eq!(files::purge_uploads(&s(dir.path()), 30, None), 1);
    assert!(!old.exists() && new.exists() && foreign.exists());
}
