//! Port of bridge/tests/test_jobs.py: background tasks run in their own conversation of a fake Hermes, stopped at
//! their budget, paused, resumed, forgotten, proposed by an agent, followed again after a restart; mail scans listed
//! with them; the routes.

mod common;

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use bipbridge::config::{parse_config, AgentConfig, Config, SharedConfig};
use bipbridge::hermes::HermesClient;
use bipbridge::jobs::{self, Jobs, TellJob};
use bipbridge::mail_scan::{self, Scans};
use bipbridge::runs::RunHub;
use bipbridge::Dict;
use common::{config_dict, spawn_fake, Urls};
use serde_json::{json, Map, Value};

/// A gateway with one conversation at a time: each poll of its counters adds `step` input tokens while a run goes
/// on; a run ends after `rounds` polls of its status, or when stopped.
#[derive(Default)]
struct Gateway {
    rounds: u32,
    step: i64,
    output: String,
    requests: Vec<(String, String, Option<Value>)>,
    runs: Map<String, Value>,
    counters: [i64; 3],
    messages: Vec<Value>,
    forget_runs: bool,
    /// What each run streams on `/events` (`model.usage`, `tool.started`…), then `run.cancelled` once stopped.
    events: Vec<Value>,
}

type Fake = Arc<Mutex<Gateway>>;

async fn handle(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().to_string();
    let full = request.uri().path().to_string();
    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Option<Value> = if bytes.is_empty() { None } else { serde_json::from_slice(&bytes).ok() };
    let mut g = fake.lock().unwrap();
    g.requests.push((method.clone(), full.clone(), body.clone()));
    let path = full.strip_prefix("/p/coach").unwrap_or(&full).to_string();
    let parts: Vec<&str> = path.split('/').collect();
    if method == "POST" && path == "/api/sessions" {
        return Json(json!({"session": {"id": "s1", "title": body.unwrap()["title"]}})).into_response();
    }
    if method == "GET" && path == "/api/sessions/s1" {
        if g.runs.values().any(|r| r["status"] == "running") {
            g.counters[0] += g.step;
            g.counters[1] += g.step / 20;
        }
        let c = g.counters;
        return Json(
            json!({"session": {"id": "s1", "input_tokens": c[0], "output_tokens": c[1], "cache_read_tokens": c[2]}}),
        )
        .into_response();
    }
    if method == "GET" && path == "/api/sessions/s1/messages" {
        return Json(json!({"messages": g.messages})).into_response();
    }
    if method == "POST" && path == "/v1/runs" {
        let run_id = format!("r{}", g.runs.len() + 1);
        g.runs.insert(run_id.clone(), json!({"status": "running", "polls": 0, "input": body.unwrap()["input"]}));
        return (StatusCode::ACCEPTED, Json(json!({"run_id": run_id, "status": "started"}))).into_response();
    }
    if method == "GET" && path.ends_with("/events") {
        let (id, events) = (parts[3].to_string(), g.events.clone());
        drop(g);
        let fake = fake.clone();
        let stream = async_stream::stream! {
            for event in events {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                yield Ok::<_, std::io::Error>(Bytes::from(format!("data: {event}\n\n")));
            }
            for _ in 0..300 {
                if fake.lock().unwrap().runs[&id]["status"] != "running" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            yield Ok(Bytes::from(format!("data: {}\n\n", json!({"event": "run.cancelled", "run_id": id}))));
        };
        return Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap();
    }
    if method == "POST" && path.ends_with("/stop") {
        g.runs[parts[3]]["status"] = json!("cancelled");
        return Json(json!({"status": "stopping"})).into_response();
    }
    if method == "GET" && path.starts_with("/v1/runs/") {
        let (rounds, forget, output) = (g.rounds, g.forget_runs, g.output.clone());
        let Some(run) = g.runs.get_mut(parts[3]).filter(|_| !forget) else {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response();
        };
        let polls = run["polls"].as_u64().unwrap() + 1;
        run["polls"] = json!(polls);
        if run["status"] == "running" && polls >= rounds as u64 {
            run["status"] = json!("completed");
            return Json(json!({"status": "completed", "output": output, "usage": {"input_tokens": 100_000, "output_tokens": 5_000}}))
                .into_response();
        }
        return Json(json!({"status": run["status"]})).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn gateway(rounds: u32, step: i64) -> (Fake, String) {
    let fake: Fake = Arc::new(Mutex::new(Gateway {
        rounds,
        step,
        output: "Rapport : trois écoles comparées, page « Écoles ».".into(),
        ..Default::default()
    }));
    let url = spawn_fake(Router::new().fallback(any(handle)).with_state(fake.clone())).await;
    (fake, url)
}

type Told = Arc<Mutex<Vec<(String, Option<String>, String, String)>>>;

fn setup(dir: &std::path::Path, url: &str) -> (Config, AgentConfig) {
    let mut dict = config_dict(dir, &Urls::default());
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": format!("{url}/p/coach"), "hermes_key": "k"});
    let mut config = parse_config(&dict, None).unwrap();
    config.browser.language = "fr-FR".into(); // a French place: its pushes in French (names.rs)
    let agent = config.agent(Some("coach")).unwrap().clone();
    (config, agent)
}

fn board(config: &Config, told: Option<&Told>) -> Arc<Jobs> {
    let mut jobs =
        Jobs::new(SharedConfig::new(config.clone()), HermesClient::new(reqwest::Client::new()), None, None, None);
    jobs.poll = 0.01;
    if let Some(told) = told {
        let told = told.clone();
        let tell: TellJob = Arc::new(move |agent, session, line, _job, body| {
            told.lock().unwrap().push((agent, session, line, body));
            Box::pin(async {})
        });
        jobs.tell = Some(tell);
    }
    Arc::new(jobs)
}

/// A task run's words, without its time line and its rules' note (every run of a task carries them).
fn words(input: &Value) -> String {
    let text = common::without_now(input.as_str().unwrap());
    let (words, rules) = text.split_once("\n\n<!--bip:task-rules\n").expect("the task's rules in its message");
    assert!(rules.ends_with("\n-->"));
    words.to_string()
}

fn id_of(job: &Dict) -> String {
    job["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_task_runs_in_its_own_conversation_and_ends_with_its_report() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(3, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = board(&config, Some(&told));
    let job = board
        .create(&agent, "Écoles du quartier", "Compare les trois écoles près de chez nous.", 0.50, Some("research"))
        .await
        .unwrap();
    assert_eq!((job["state"].as_str(), job["session_id"].as_str()), (Some("running"), Some("s1")));
    board.wait(&id_of(&job)).await;
    let done = board.view(&id_of(&job)).unwrap();
    assert_eq!(done["state"], "done");
    assert!(done["report"].as_str().unwrap().starts_with("Rapport"));
    // The run's own usage at the end, not the counters' guess.
    assert_eq!(done["tokens"], json!({"input": 100_000, "output": 5_000, "cached": 0}));
    let posted: Vec<Value> = fake
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|(m, _, _)| m == "POST")
        .map(|(_, _, b)| b.clone().unwrap_or(Value::Null))
        .collect();
    assert_eq!(posted[0]["title"], "🧩 Écoles du quartier");
    assert_eq!(posted[1]["session_id"], "s1");
    // the rules in the message, the system prompt the conversations' own (background_turn.rs, cache audit #2)
    assert!(posted[1].get("instructions").is_none(), "{}", posted[1]);
    let input = common::without_now(posted[1]["input"].as_str().unwrap());
    assert!(input.starts_with("Écoles du quartier\n\nCompare"));
    assert!(input.ends_with(&format!("<!--bip:task-rules\n{}\n-->", jobs::INSTRUCTIONS)), "{input}");
    assert_eq!(
        told.lock().unwrap().clone(),
        vec![(
            "coach".into(),
            Some("s1".into()),
            "Tâche de fond terminée : Écoles du quartier".into(),
            "Tâche de fond terminée".into()
        )]
    );
    let listed = board.list(None);
    assert_eq!(listed[0]["id"], json!(id_of(&job)));
    assert!(!listed[0].contains_key("base") && !listed[0].contains_key("run_id"));
}

#[tokio::test]
async fn a_task_stops_at_its_budget_and_goes_on_with_a_bigger_one() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(50, 60_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = board(&config, Some(&told));
    let job = board.create(&agent, "Gros tri", "Range tout.", 0.05, None).await.unwrap();
    let id = id_of(&job);
    board.wait(&id).await;
    let paused = board.view(&id).unwrap();
    assert_eq!((paused["state"].as_str(), paused["reason"].as_str()), (Some("paused"), Some("budget")));
    assert!(fake
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, p, b)| m == "POST" && p == "/p/coach/v1/runs/r1/stop" && b.is_none()));
    let before = paused["tokens"]["input"].as_i64().unwrap();
    assert!(before >= 100_000 * 20 / 21); // counted from the conversation's counters
    assert_eq!(told.lock().unwrap().last().unwrap().3, "Tâche de fond en pause");
    // Resuming without more budget is refused; with more, the same conversation goes on.
    assert!(board.resume(&id, None, None).await.unwrap_err().downcast_ref::<jobs::JobError>().is_some());
    fake.lock().unwrap().rounds = 2;
    board.resume(&id, Some(3.0), Some("Continue, et termine par ton rapport.")).await.unwrap();
    board.wait(&id).await;
    let done = board.view(&id).unwrap();
    assert_eq!((done["state"].as_str(), done["runs"].as_i64()), (Some("done"), Some(2)));
    assert_eq!(done["tokens"]["input"].as_i64().unwrap(), before + 100_000); // the second run's own usage added
    let g = fake.lock().unwrap();
    assert_eq!(words(&g.runs["r2"]["input"]), "Continue, et termine par ton rapport.");
    assert_eq!(g.requests.iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/api/sessions")).count(), 1);
}

#[tokio::test]
async fn the_person_pauses_resumes_and_forgets_a_task() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let board = board(&config, None);
    let job = board.create(&agent, "Long", "Fais-le.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    // (several at once since 19/11: `several_tasks_of_one_agent_run_at_once`)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(board.pause(&id).await.unwrap()["state"], "stopping");
    board.wait(&id).await;
    let view = board.view(&id).unwrap();
    assert_eq!((view["state"].as_str(), view["reason"].as_str()), (Some("paused"), Some("stopped")));
    board.resume(&id, None, None).await.unwrap();
    assert_eq!(words(&fake.lock().unwrap().runs["r2"]["input"]), jobs::CONTINUE);
    board.forget(&id).await.unwrap();
    assert!(jobs::load(&config, &id).is_none());
    assert_eq!(fake.lock().unwrap().runs["r2"]["status"], "cancelled");
}

#[tokio::test]
async fn an_agent_proposes_a_task_the_person_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = board(&config, Some(&told));
    let arguments: Dict = serde_json::from_value(
        json!({"title": "Dossier location", "prompt": "Prépare le dossier.", "budget": "small"}),
    )
    .unwrap();
    let result = jobs::propose_tool(Some(&board), &agent, &arguments).await.unwrap();
    assert_eq!(result["isError"], false);
    assert!(result["content"][0]["text"].as_str().unwrap().contains("Tâches de fond"));
    assert!(result["content"][0]["text"].as_str().unwrap().contains("1,818,181 tokens"), "small: 5 × 0.20 € at home");
    let proposed = board.list(None)[0].clone();
    assert_eq!((proposed["state"].as_str(), proposed["proposed_by"].as_str()), (Some("proposed"), Some("agent")));
    assert_eq!((proposed["budget_eur"].as_f64(), proposed["max_tokens"].as_i64()), (Some(1.0), Some(1_818_181)));
    assert!(!fake.lock().unwrap().requests.iter().any(|(_, p, _)| p.ends_with("/v1/runs"))); // nothing spent before the go
    let first = told.lock().unwrap()[0].clone();
    assert_eq!((first.3.as_str(), first.1), ("Tâche de fond proposée", None));
    let id = id_of(&proposed);
    board.start(&id, Some(0.30)).await.unwrap();
    board.wait(&id).await;
    assert_eq!(board.view(&id).unwrap()["state"], "done");
}

#[tokio::test]
async fn after_a_restart_a_finished_run_is_read_from_its_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let first = board(&config, None);
    let job = first.create(&agent, "Recherche", "Cherche.", 50.0, None).await.unwrap();
    let id = id_of(&job);
    first.stop_followers(); // the bridge stops
    {
        let mut g = fake.lock().unwrap();
        g.forget_runs = true; // Hermes restarted meanwhile: the run is gone, its turns stay
        g.messages = vec![
            json!({"role": "user", "content": "Recherche\n\nCherche."}),
            json!({"role": "assistant", "content": "Voici le rapport."}),
        ];
    }
    let again = board(&config, None);
    assert_eq!(again.clone().resume_following().await, 1);
    again.wait(&id).await;
    let view = again.view(&id).unwrap();
    assert_eq!((view["state"].as_str(), view["report"].as_str()), (Some("done"), Some("Voici le rapport.")));
}

#[tokio::test]
async fn mail_scans_are_listed_and_controlled_with_the_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = setup(dir.path(), "http://127.0.0.1:9");
    let scan: Dict = serde_json::from_value(json!({"agent": "coach", "account": "a1", "label": "Gmail", "state": "paused", "error": "budget",
        "created": 1.0, "ranges": [], "tokens": {"input": 9000, "output": 900, "cached": 0},
        "processed": 30, "pages": 0, "step": {"since": "2026-07-21", "until": "2026-10-21", "max_tokens": 10_000, "mails": 80, "processed": 30}}))
    .unwrap();
    mail_scan::save(&config, &scan).unwrap();
    let scans = Arc::new(Scans::new(None, None, None));
    let board = Jobs::new(
        SharedConfig::new(config.clone()),
        HermesClient::new(reqwest::Client::new()),
        None,
        None,
        Some(scans),
    );
    let listed: Vec<Value> = board.list(None).into_iter().map(Value::Object).collect();
    assert_eq!(
        Value::Array(listed),
        json!([{"id": "mail-scan:coach", "kind": "mail_scan", "agent": "coach", "title": "Gmail",
                "state": "paused", "reason": "budget", "error": null, "created": 1.0, "finished": null,
                "tokens": {"input": 9000, "output": 900, "cached": 0}, "max_tokens": 10_000,
                "progress": {"done": 30, "total": 80}, "processed": 30, "pages": 0}])
    );
    // Forgotten from the jobs' list: its file goes.
    board.forget("mail-scan:coach").await.unwrap();
    assert!(mail_scan::load(&config, "coach").is_none());
    let unknown = board.pause("mail-scan:coach").await.unwrap_err();
    assert_eq!(unknown.to_string(), "unknown job");
}

#[tokio::test]
async fn the_routes_need_the_bridge_key_and_list_the_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(common::config(dir.path(), &Urls::default()));
    assert_eq!(app.get("/v1/jobs", &[]).await.status, 401);
    let (k, v) = common::auth();
    let answer = app.get("/v1/jobs", &[(k, &v)]).await;
    assert_eq!(answer.status, 200);
    // a home install (the test's HOME has no place token): five times the offer's budgets (jobs.rs HOME_BUDGET_SCALE)
    assert_eq!(
        answer.json(),
        json!({"jobs": [], "budgets_eur": {"small": 1.0, "medium": 2.5, "large": 7.5}, "budgets_tokens": {"small": 1_818_181, "medium": 4_545_454, "large": 13_636_363}})
    );
    // a place of the offer keeps its own: 0.20 / 0.50 / 1.50 €, cache reads at the input price
    assert_eq!(
        jobs::budgets_on(&common::config(dir.path(), &Urls::default()), true),
        (json!({"small": 0.20, "medium": 0.50, "large": 1.50}), json!({"small": 363_636, "medium": 909_090, "large": 2_727_272}))
    );
    let missing = app.request("POST", "/v1/jobs/t000000000000/pause", &[(k, &v)], None).await;
    assert_eq!((missing.status.as_u16(), missing.json()), (404, json!({"detail": "unknown job"})));
}

fn with_handback(config: &Config, told: &Told) -> Arc<Jobs> {
    let board = board(config, Some(told));
    let mut jobs = Arc::try_unwrap(board).ok().unwrap();
    let mut handback = bipbridge::background_turn::Pipeline::new(
        SharedConfig::new(config.clone()),
        HermesClient::new(reqwest::Client::new()),
    );
    handback.poll = 0.01;
    jobs.handback = Some(Arc::new(handback));
    Arc::new(jobs)
}

fn posted_runs(fake: &Fake) -> Vec<Value> {
    fake.lock()
        .unwrap()
        .requests
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/p/coach/v1/runs")
        .map(|(_, _, b)| b.clone().unwrap_or(Value::Null))
        .collect()
}

/// 2026-10-25: a task started from a conversation ends with the agent's own message there (fed with the report, the
/// full report kept as a page and linked), not only a push towards the task's conversation.
#[tokio::test]
async fn a_finished_task_is_handed_back_to_the_conversation_it_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(3, 40_000).await;
    fake.lock().unwrap().output =
        "J'ai comparé les écoles.\n\n- **Jules Ferry** : la plus proche\n- Pasteur : cantine bio\n\
- Curie : classes de 22"
            .into();
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = with_handback(&config, &told);
    let job =
        board.create_from(&agent, "Écoles", "Compare les écoles.", 0.50, None, "asked", Some("api_c")).await.unwrap();
    let id = id_of(&job);
    assert_eq!(job["from_session"], "api_c");
    board.wait(&id).await;
    let runs = posted_runs(&fake);
    assert_eq!(runs.len(), 2);
    let turn = &runs[1];
    assert_eq!(turn["session_id"], "api_c");
    // its rules in the note, the system prompt (and the conversation's prompt cache) untouched
    assert!(turn.get("instructions").is_none(), "{turn}");
    let note = &common::without_now(turn["input"].as_str().unwrap());
    // the report's page holds the points: the message introduces it
    assert!(note.contains(bipbridge::handback::PAGE_INSTRUCTIONS));
    let page = format!("/bipagents/pages/coach/rapport-{id}.html");
    assert!(note.starts_with("<!--bip:task\nYour background work « Écoles » is finished."), "{note}");
    assert!(
        note.contains("- Pasteur : cantine bio") && note.contains(&format!("MEDIA:{page}")) && note.ends_with("-->")
    );
    // the report page, in the agent's pages
    let kept = dir.path().join(format!("data/agent-pages/coach/rapport-{id}.html"));
    let html = std::fs::read_to_string(kept).unwrap();
    assert!(html.contains("<li><strong>Jules Ferry</strong> : la plus proche</li>"));
    let done = jobs::load(&config, &id).unwrap();
    assert_eq!(done["state"], "done");
    assert_eq!(done["handed_back"]["by_agent"], true);
    assert_eq!(done["handed_back"]["links"], json!([page]));
    // the turn's tokens are the task's too (its usage: 105,000 more)
    assert_eq!(done["tokens"]["input"], 205_000);
    // no « Tâche de fond terminée » push towards the task's own conversation: the message is the news
    assert!(told.lock().unwrap().is_empty());
}

/// 2026-10-31 (Alex's « Vérifier restaurants étoilés près du Pyla »): a task that made its own page delivers that page
/// alone — no « Rapport — » page beside it — and the agent's message only introduces it (one message, one version).
#[tokio::test]
async fn a_task_that_made_its_page_is_handed_back_with_that_page_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(3, 40_000).await;
    fake.lock().unwrap().output = "Vérification terminée : 3 tables étoilées.\n\n- Le Patio, Arcachon\n- Nacre, Arès\n\
MEDIA:/bipagents/pages/coach/tables-etoilees.html"
        .into();
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = with_handback(&config, &told);
    let job = board
        .create_from(&agent, "Vérifier les tables", "Vérifie.", 0.50, None, "asked", Some("api_c"))
        .await
        .unwrap();
    let id = id_of(&job);
    board.wait(&id).await;
    let runs = posted_runs(&fake);
    let note = &common::without_now(runs[1]["input"].as_str().unwrap());
    assert!(note.contains(bipbridge::handback::PAGE_INSTRUCTIONS), "{note}");
    assert!(note.contains("End your message with these lines:\nMEDIA:/bipagents/pages/coach/tables-etoilees.html\n-->"));
    assert!(!note.contains("rapport-"), "{note}");
    assert!(!dir.path().join(format!("data/agent-pages/coach/rapport-{id}.html")).exists());
    let done = jobs::load(&config, &id).unwrap();
    assert_eq!(done["handed_back"]["links"], json!(["/bipagents/pages/coach/tables-etoilees.html"]));
}

/// 2026-10-26: a task that ended with its budget (nearly) spent still gets the agent's own message (it used to get the
/// template, ids and all); only one far past it (`OVERRUN_MAX`) does not.
#[tokio::test]
async fn a_task_ending_at_its_budget_is_still_handed_back_by_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(3, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = with_handback(&config, &told);
    let job = board.create_from(&agent, "Écoles", "Compare.", 0.0605, None, "asked", Some("api_c")).await.unwrap();
    let id = id_of(&job);
    board.wait(&id).await;
    assert_eq!(posted_runs(&fake).len(), 2, "the agent's turn, past the budget");
    let done = jobs::load(&config, &id).unwrap();
    assert_eq!((done["state"].as_str(), done["handed_back"]["by_agent"].as_bool()), (Some("done"), Some(true)));
    assert_eq!(done["handed_back"]["why"], Value::Null);
    // without a conversation to go back to: the push as before
    let other = board.create(&agent, "Autre", "Encore.", 0.50, None).await.unwrap();
    board.wait(&id_of(&other)).await;
    assert_eq!(told.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_month_s_credit_used_stops_a_task_after_its_step_refuses_new_ones_and_resumes_it_when_back() {
    // credit.rs (Alex 2026-10-26): no wall for conversations, but background tasks pause while the credit is used.
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let credit = dir.path().join("credit.json");
    let mut jobs =
        Jobs::new(SharedConfig::new(config.clone()), HermesClient::new(reqwest::Client::new()), None, None, None);
    jobs.poll = 0.01;
    jobs.credit_file = Some(credit.clone());
    let board = Arc::new(jobs);
    let job = board.create(&agent, "Long", "Fais-le.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    std::fs::write(&credit, json!({"state": "full", "resumes": 4.0e9}).to_string()).unwrap();
    board.wait(&id).await;
    let view = board.view(&id).unwrap();
    assert_eq!((view["state"].as_str(), view["reason"].as_str()), (Some("paused"), Some("credit")));
    assert_eq!(fake.lock().unwrap().runs["r1"]["status"], "cancelled");
    // no new task meanwhile, said plainly
    let refused = board.create(&agent, "Autre", "Encore.", 0.50, None).await.unwrap_err();
    assert!(refused.to_string().contains("credit is used up"), "{refused}");
    // a stale file (its new month come) pauses nothing; the credit back: the task goes on by itself
    std::fs::write(&credit, json!({"state": "full", "resumes": 1.0}).to_string()).unwrap();
    assert!(!board.credit_used());
    std::fs::write(&credit, json!({"state": "ok"}).to_string()).unwrap();
    board.resume_after_credit(&config).await;
    assert_eq!(words(&fake.lock().unwrap().runs["r2"]["input"]), jobs::CONTINUE);
    assert_eq!(board.view(&id).unwrap()["state"], "running");
    board.forget(&id).await.unwrap();
}

/// 2026-10-31, Alex's home install: a task spent about twice its budget (« 0,071 € dépensés · au plus 0,037 € »).
/// Hermes writes a conversation's counters only at a run's end (its token counts are queued and drained when the turn
/// is saved), so the bridge, reading them every few seconds, saw nothing move and the run went on to its end. With
/// Hermes patch 0020 each model call's real tokens come on the run's stream: the hub stops the run before a call that
/// would pass what is left of the budget, and the task pauses at its budget with those tokens.
#[tokio::test]
async fn a_task_is_stopped_on_its_real_tokens_before_a_call_past_its_budget() {
    let usage = |call: u32, input: i64, total_in: i64, total_out: i64| {
        json!({"event": "model.usage", "call": call, "input_tokens": input, "output_tokens": 300,
               "totals": {"input_tokens": total_in, "output_tokens": total_out, "cache_read_tokens": 1_000}})
    };
    let step = |file: &str| json!({"event": "tool.started", "tool": "read_file", "preview": file});
    let dir = tempfile::tempdir().unwrap();
    // counters that never move while the run goes on, as Hermes' own; the run would go on for ever
    let (fake, url) = gateway(10_000, 0).await;
    fake.lock().unwrap().events = vec![
        usage(1, 20_000, 20_000, 300),
        step("a.md"),
        usage(2, 35_000, 55_000, 600),
        step("b.md"),
        usage(3, 40_000, 95_000, 900),
        step("c.md"),
    ];
    let (config, agent) = setup(dir.path(), &url);
    let hub = Arc::new(RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None));
    let mut board =
        Jobs::new(SharedConfig::new(config.clone()), HermesClient::new(reqwest::Client::new()), Some(hub), None, None);
    board.poll = 0.05;
    let board = Arc::new(board);
    let job = board.create(&agent, "Analyse interview", "Résume l'interview.", 0.05, None).await.unwrap();
    let id = id_of(&job);
    tokio::time::timeout(std::time::Duration::from_secs(20), board.wait(&id)).await.expect("the task stopped");
    let paused = board.view(&id).unwrap();
    assert_eq!((paused["state"].as_str(), paused["reason"].as_str()), (Some("paused"), Some("budget")), "{paused:?}");
    // after its 2nd call (55.6k, the next reading 35k at least) it goes on; after its 3rd (95.9k, the next 40k) a 4th
    // would pass 100k: stopped as it asks for tools, within its budget
    // at home (the test's HOME) its screens count a cache read a tenth of a fresh token; an offer place shows Hermes' own
    assert_eq!(paused["tokens"], json!({"input": 94_100, "output": 900, "cached": 100, "cache_reads": 1_000}));
    let raw = jobs::load(&config, &id).unwrap();
    assert_eq!(jobs::task_view_on(&raw, true)["tokens"], json!({"input": 95_000, "output": 900, "cached": 1_000}));
    assert!(jobs::spent(&paused) <= 100_000);
    let g = fake.lock().unwrap();
    assert!(g.requests.iter().any(|(m, p, _)| m == "POST" && p == "/p/coach/v1/runs/r1/stop"));
    assert_eq!(g.runs.len(), 1);
}

/// Without patch 0020 (no `model.usage`) and with Hermes' counters still until the run's end, nothing tells the bridge
/// a run's tokens before it ends: it is counted at its end, with its own usage, past its budget. Kept to say why the
/// stop needs Hermes 0020 (the installer's patches).
#[tokio::test]
async fn without_real_tokens_a_run_is_counted_at_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(4, 0).await;
    let (config, agent) = setup(dir.path(), &url);
    let board = board(&config, None);
    let job = board.create(&agent, "Analyse interview", "Résume l'interview.", 0.025, None).await.unwrap();
    board.wait(&id_of(&job)).await;
    let done = board.view(&id_of(&job)).unwrap();
    assert_eq!(done["state"], "done");
    assert_eq!(jobs::spent(&done), 105_000, "its usage at the end, twice its budget");
    assert!(!fake.lock().unwrap().requests.iter().any(|(_, p, _)| p.ends_with("/stop")));
}

/// Cache audit #2 (2026-11-03): a turn taken without the person gets the same system prompt as the agent's
/// conversations, so its request carries no run instructions, whatever starts it (background_turn.rs); what the turn
/// must know is in its message. Same keys as a conversation's run (`input`, `session_id`), a task's thinking level
/// aside (`model_options`, not part of the prompt).
#[tokio::test]
async fn every_background_turn_keeps_the_conversations_system_prompt() {
    use bipbridge::background_turn::{Kind, Pipeline, Turn};
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let mut pipeline = Pipeline::new(SharedConfig::new(config.clone()), HermesClient::new(reqwest::Client::new()));
    pipeline.poll = 0.01;
    for kind in [Kind::Handback, Kind::Trigger, Kind::Watcher, Kind::Task] {
        let mut turn = Turn::new(&agent, kind, "api_c", format!("<!--bip:{} note -->", kind.as_str()));
        turn.max_seconds = 5.0;
        let ran = pipeline.run(&turn).await.unwrap();
        assert_eq!(ran.session, "api_c", "{kind:?}");
    }
    let runs = posted_runs(&fake);
    assert_eq!(runs.len(), 4);
    for run in &runs {
        let mut keys: Vec<&str> =
            run.as_object().unwrap().keys().map(String::as_str).filter(|k| *k != "model_options").collect();
        keys.sort_unstable();
        assert_eq!(keys, ["input", "session_id"], "{run}");
    }
    // the same request a background task sends through its board
    let board = board(&config, None);
    let job = board.create(&agent, "Écoles", "Compare.", 0.50, None).await.unwrap();
    board.wait(&id_of(&job)).await;
    let task = posted_runs(&fake).pop().unwrap();
    assert!(task.get("instructions").is_none(), "{task}");
}

/// Alex, 2026-11-04 (offer place): a task stopped at « 5 % of the month », he gave it more, and his credit still
/// showed about 97 % left. Its budget counted every prompt token at the full input price, while the router bills the
/// cache reads (nearly all of a task's prompt: its conversation read again at each step) at a fifth of it. Its euros
/// now are the router's bill for its calls: fresh input, cache reads and output at the model's prices, the numbers
/// the month's credit is metered with.
#[tokio::test]
async fn a_task_counts_its_euros_as_the_month_s_credit_does_cache_reads_included() {
    let usage = |call: u32, input: i64, total_in: i64, total_cached: i64, total_out: i64| {
        json!({"event": "model.usage", "call": call, "input_tokens": input, "output_tokens": 300,
               "totals": {"input_tokens": total_in, "output_tokens": total_out, "cache_read_tokens": total_cached}})
    };
    let step = |file: &str| json!({"event": "tool.started", "tool": "read_file", "preview": file});
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(10_000, 0).await;
    fake.lock().unwrap().events = vec![
        usage(1, 95_000, 95_000, 90_000, 900),
        step("a.md"),
        usage(2, 300_000, 400_000, 380_000, 3_000),
        step("b.md"),
    ];
    let (mut config, agent) = setup(dir.path(), &url);
    // GLM-5.3 Flash at EUrouter, as the guichet lists it: 0.10 € fresh, 0.02 € from the cache, 0.35 € out
    (config.jobs.price_in, config.jobs.price_cached, config.jobs.price_out) = (0.10, 0.02, 0.35);
    let hub = Arc::new(RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None));
    let mut board =
        Jobs::new(SharedConfig::new(config.clone()), HermesClient::new(reqwest::Client::new()), Some(hub), None, None);
    board.poll = 0.05;
    let board = Arc::new(board);
    let job = board.create(&agent, "Connecteur", "Installe le connecteur.", 0.015, None).await.unwrap();
    let id = id_of(&job);
    tokio::time::timeout(std::time::Duration::from_secs(20), board.wait(&id)).await.expect("the task stopped");
    let paused = board.view(&id).unwrap();
    // its first call (95k read, 90k of it from the cache) cost 0.0026 €: the old count said 0.048 €, three times
    // this budget, and stopped it there; it went on to its second call (0.0107 €), whose next one would pass 0.015 €
    assert_eq!((paused["state"].as_str(), paused["reason"].as_str()), (Some("paused"), Some("budget")), "{paused:?}");
    // a place of the offer shows Hermes' own counts; the test's HOME is a home install, whose screens weigh the cache
    // reads at their price (0.02 / 0.10), its euros the same
    let raw = jobs::load(&config, &id).unwrap();
    assert_eq!(jobs::task_view_on(&raw, true)["tokens"], json!({"input": 400_000, "output": 3_000, "cached": 380_000}));
    assert_eq!(paused["tokens"], json!({"input": 96_000, "output": 3_000, "cached": 76_000, "cache_reads": 380_000}));
    let bill = (20_000.0 * 0.10 + 380_000.0 * 0.02 + 3_000.0 * 0.35) / 1e6;
    assert!((paused["eur"].as_f64().unwrap() - bill).abs() < 1e-6, "{} vs {bill}", paused["eur"]);
    assert_eq!(paused["budget_eur"], json!(0.015));
    assert!(!paused.contains_key("prices"), "its prices stay the bridge's");
    // « Continuer (+0,20 €) »: its new total, at once
    fake.lock().unwrap().events = vec![];
    fake.lock().unwrap().rounds = 2;
    let resumed = board.resume(&id, Some(0.215), None).await.unwrap();
    assert_eq!((resumed["state"].as_str(), resumed["budget_eur"].as_f64()), (Some("running"), Some(0.215)));
    board.wait(&id).await;
    assert_eq!(board.view(&id).unwrap()["state"], "done");
}

type Hinted = Arc<Mutex<Vec<(String, String, String)>>>;

fn hinted(board: Arc<Jobs>, hints: &Hinted) -> Arc<Jobs> {
    let mut jobs = Arc::try_unwrap(board).ok().unwrap();
    let hints = hints.clone();
    let hint: jobs::HintJob = Arc::new(move |agent, job, why| {
        hints.lock().unwrap().push((agent, job, why));
        Box::pin(async {})
    });
    jobs.hint = Some(hint);
    Arc::new(jobs)
}

/// Sub-tasks (Alex 19/11): one agent runs up to three tasks at once (the install's three too), the fourth refused.
#[tokio::test]
async fn several_tasks_of_one_agent_run_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(1000, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let board = board(&config, None);
    let mut ids = Vec::new();
    for title in ["Mails", "Assurances", "Vacances"] {
        let job = board.create(&agent, title, "Fais-le en entier.", 0.50, None).await.unwrap();
        assert_eq!(job["state"], "running", "{title}");
        ids.push(id_of(&job));
    }
    let fourth = board.create(&agent, "Encore", "Encore une.", 0.50, None).await.unwrap_err();
    assert!(fourth.to_string().contains("three background tasks"), "{fourth}");
    assert_eq!(jobs::AGENT_RUNNING_MAX, 3);
    for id in ids {
        board.forget(&id).await.unwrap();
    }
}

/// The apps are told (a silent « jobs » hint, no polling) when a task starts, ends and is seen; a finished task stays
/// « not seen » for every device until one of them opened its result, and the home's list (`/v1/status`) says so.
#[tokio::test]
async fn a_finished_task_waits_to_be_seen_and_the_apps_are_hinted() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(3, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let hints: Hinted = Default::default();
    let board = hinted(board(&config, None), &hints);
    let job = board.create(&agent, "Plan 10 km", "Prépare mon plan pour un 10 km.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    let home = jobs::home_jobs(&config);
    assert_eq!((home.len(), home[0]["state"].as_str()), (1, Some("running")));
    board.wait(&id).await;
    let done = board.view(&id).unwrap();
    assert_eq!((done["state"].as_str(), done["seen"].as_bool()), (Some("done"), Some(false)));
    let home = jobs::home_jobs(&config);
    assert_eq!((home.len(), home[0]["id"].as_str(), home[0]["seen"].as_bool()), (1, Some(id.as_str()), Some(false)));
    assert!(!home[0].contains_key("report") && !home[0].contains_key("prompt"), "the home's list stays small");
    board.seen(&id).await.unwrap();
    assert_eq!(board.view(&id).unwrap()["seen"], true);
    assert!(jobs::home_jobs(&config).is_empty());
    let whys: Vec<String> = hints.lock().unwrap().iter().filter(|(a, j, _)| a == "coach" && *j == id).map(|h| h.2.clone()).collect();
    assert_eq!(whys, vec!["started", "ended", "seen"]);
    // unseen for more than a week (an app that never says « seen »): not « new » any more
    let mut stale = jobs::load(&config, &id).unwrap();
    stale.insert("seen".into(), json!(false));
    stale.insert("finished".into(), json!(bipbridge::py::time_time() - 8.0 * 86_400.0));
    jobs::save(&config, &stale).unwrap();
    assert!(jobs::home_jobs(&config).is_empty(), "a week-old unseen task");
    // a task finished before 19/11 (no `seen`) is not « new » on the home
    let mut old = jobs::load(&config, &id).unwrap();
    old.remove("seen");
    jobs::save(&config, &old).unwrap();
    assert!(jobs::home_jobs(&config).is_empty());
}

/// Review 1 (subtasks-redesign): the per-agent cap holds when the starts come at once (a model's parallel `start_task`
/// calls, two devices): `may_run` reads the running tasks, then `launch` awaits Hermes (heavy_work's check, the session,
/// the run) before the task is saved « running »; every start in between passes the same check.
#[tokio::test]
async fn starts_at_once_never_pass_the_agent_s_cap() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(1000, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let board = board(&config, None);
    let starts = (0..5).map(|n| {
        let (board, agent) = (board.clone(), agent.clone());
        async move { board.create(&agent, &format!("Tâche {n}"), "Fais-le en entier.", 0.50, None).await }
    });
    let outcomes = futures::future::join_all(starts).await;
    let running: Vec<String> = outcomes.iter().filter_map(|o| o.as_ref().ok()).map(id_of).collect();
    let on_disk = jobs::tasks(&config).into_iter().filter(|j| j["state"] == "running").count();
    for id in &running {
        board.forget(id).await.unwrap();
    }
    assert!(running.len() <= jobs::AGENT_RUNNING_MAX, "{} tasks of one agent started at once", running.len());
    assert!(on_disk <= jobs::AGENT_RUNNING_MAX, "{on_disk} running on disk");
}

/// One browser per running task (Alex 19/11, « vraiment en parallèle »): an agent with its own browser gets, for each
/// task it starts, a browser of the task's own (its container, its ports, a copy of the agent's profile with its
/// logins, its folder), announced to the agent's browser tools by the task's Hermes session (`sessions.json` beside
/// the agent's `busy`, patch 0028), and removed when the task ends. The person's conversations keep the agent's.
#[tokio::test]
async fn a_task_of_an_agent_with_a_browser_gets_a_browser_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(3, 10).await;
    let (mut config, agent) = setup(dir.path(), &url);
    config.browser.root = dir.path().join("browsers").to_string_lossy().into_owned();
    let place = bipbridge::browsers::folder(&config, "coach");
    std::fs::create_dir_all(place.join("profile").join("Default")).unwrap();
    std::fs::write(place.join("profile").join("Default").join("Cookies"), "logged in").unwrap();
    std::fs::create_dir_all(place.join("profile/Default/Cache")).unwrap();
    std::fs::write(place.join("profile/Default/Cache/data_1"), "x".repeat(4096)).unwrap();
    std::fs::write(place.join("profile/Local State"), "{\"os_crypt\": {}}").unwrap();
    std::fs::write(place.join("ports.json"), r#"{"cdp": 19600, "vnc": 19601}"#).unwrap();
    std::fs::write(place.join("mcp.json"), r#"{"server": ["/usr/bin/podman", "exec", "-i", "bipagents-browser-coach", "/opt/stealth/mcp.sh"]}"#).unwrap();
    let board = board(&config, None);
    let job = board.create(&agent, "Comparer", "Compare trois offres sur leurs sites.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    let own = bipbridge::browsers::task_browser("coach", &id);
    // its agent read back from its name (the egress proxy's policy gives it the agent's rules, send_guard.rs)
    assert_eq!(bipbridge::browsers::task_browser_agent(&own), Some("coach"));
    assert_eq!(bipbridge::browsers::task_browser_agent("coach"), None);
    let theirs = bipbridge::browsers::folder(&config, &own);
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(theirs.join("mcp.json")).unwrap()).unwrap();
    assert_eq!(settings["server"][3], json!(bipbridge::browsers::container(&own)), "{settings}");
    assert_eq!(std::fs::read_to_string(theirs.join("profile/Default/Cookies")).unwrap(), "logged in", "the agent's logins");
    // only what keeps the logins (its cookies' key in « Local State »), never the agent's caches (disk)
    assert!(theirs.join("profile/Local State").exists());
    assert!(!theirs.join("profile/Default/Cache").exists(), "the agent's cache copied");
    let ports = bipbridge::browsers::ports_of(&config, &own).unwrap();
    assert!(ports["cdp"] != 19600 && ports["vnc"] != 19601, "ports of its own: {ports:?}");
    let sessions: Value = serde_json::from_str(&std::fs::read_to_string(place.join("sessions.json")).unwrap()).unwrap();
    assert_eq!(sessions["s1"], json!(theirs.join("mcp.json").to_string_lossy()));
    // Hermes' own browser tools (its vault fill, on the agent's Chrome) closed in its turns (patch 0024)
    let run = posted_runs(&fake).last().cloned().unwrap();
    assert_eq!(run["model_options"]["closed_tools"], json!(["browser_*"]), "{run}");
    // the task's own calls are told from the person's (patch 0028): a payment never happens in its browser
    let task_call = bipbridge::runs::CALL_SESSION
        .sync_scope(Some("s1".into()), || bipbridge::browsers::task_of_call(&config, "coach"));
    assert_eq!(task_call, Some(theirs.join("mcp.json")));
    let person_call = bipbridge::runs::CALL_SESSION
        .sync_scope(Some("api_person".into()), || bipbridge::browsers::task_of_call(&config, "coach"));
    assert_eq!(person_call, None);
    board.wait(&id).await;
    assert!(!theirs.exists(), "its browser removed with the task's end");
    let sessions: Value = serde_json::from_str(&std::fs::read_to_string(place.join("sessions.json")).unwrap()).unwrap();
    assert_eq!(sessions, json!({}));
    // an agent without a browser: nothing of the kind
    let other = bipbridge::browsers::folder(&config, &bipbridge::browsers::task_browser("vie", "t0"));
    assert!(!other.exists());
}

/// Alex 20/11: a task never pays nor logs in inside its own browser: it hands over to the agent's main conversation,
/// which gets the request (what, which site, why) in the agent's own turn there and does it in the agent's browser
/// with the person's usual approval. The task is told it was handed over.
#[tokio::test]
async fn a_tasks_payment_is_handed_over_to_the_agents_main_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = with_handback(&config, &told);
    let job = board.create(&agent, "Courses de la semaine", "Remplis le panier Leclerc Drive.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    let ask = jobs::Ask { kind: jobs::AskKind::Pay, what: "le panier de la semaine, 38,40 €".into(),
                          site: "leclercdrive.fr".into(), why: "payer la commande avant 18 h".into() };
    assert!(jobs::HANDED_OVER.starts_with("Handed over"));
    let outcome = board.hand_over(&agent, &id, &ask, "api_main").await;
    assert!(outcome.is_some(), "nothing delivered");
    let run = posted_runs(&fake).into_iter().find(|r| r["session_id"] == "api_main").expect("a turn in the main conversation");
    let input = run["input"].as_str().unwrap();
    for part in ["Courses de la semaine", "38,40 €", "leclercdrive.fr", "payer la commande avant 18 h"] {
        assert!(input.contains(part), "{part} missing: {input}");
    }
    assert!(input.contains("approval"), "the person's usual approval: {input}");
    board.forget(&id).await.unwrap();
}

/// Alex 20/11, « no blocking for capacity »: a task that would browse in a browser of its own waits its turn when the
/// place's free memory (MemAvailable) is under `[jobs] browser_free_mb` (a browsing task measured 0.5–0.8 GB): queued,
/// « memory », never refused; it starts by itself once there is room again.
#[tokio::test]
async fn a_browsing_task_waits_for_free_memory_and_starts_once_there_is() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 10).await;
    let (mut config, agent) = setup(dir.path(), &url);
    config.browser.root = dir.path().join("browsers").to_string_lossy().into_owned();
    assert_eq!(config.jobs.browser_free_mb, 1024, "the default threshold");
    let place = bipbridge::browsers::folder(&config, "coach");
    std::fs::create_dir_all(place.join("profile")).unwrap();
    std::fs::write(place.join("ports.json"), r#"{"cdp": 19600, "vnc": 19601}"#).unwrap();
    std::fs::write(place.join("mcp.json"), r#"{"server": ["/usr/bin/podman"]}"#).unwrap();
    let free = Arc::new(Mutex::new(Some(600u64)));
    let mut jobs = Arc::try_unwrap(board(&config, None)).ok().unwrap();
    let probe = free.clone();
    jobs.memory = Arc::new(move || *probe.lock().unwrap());
    let board = Arc::new(jobs);
    let job = board.create(&agent, "Comparer", "Compare trois offres sur leurs sites.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    assert_eq!((job["state"].as_str(), job["reason"].as_str()), (Some("paused"), Some("memory")), "{job:?}");
    assert!(posted_runs(&fake).is_empty(), "nothing started while memory is short");
    assert_eq!(board.resume_after_memory().await, 0, "still short: it keeps waiting");
    *free.lock().unwrap() = Some(4096);
    assert_eq!(board.resume_after_memory().await, 1);
    assert_eq!(board.view(&id).unwrap()["state"], "running");
    assert!(words(&posted_runs(&fake)[0]["input"]).starts_with("Comparer\n\nCompare trois offres"), "its own words");
    board.forget(&id).await.unwrap();
}

// -- Review 2 (subtasks-redesign, 20/11, head 347bdad19): one browser per task ------------------------------------------

/// An agent with a browser of its own, as `a_task_of_an_agent_with_a_browser_gets_a_browser_of_its_own` makes it
/// (`big`: a site storage of that many bytes in its profile, what makes its copy take a while).
fn agent_browser(config: &mut Config, dir: &std::path::Path, big: usize) -> std::path::PathBuf {
    config.browser.root = dir.join("browsers").to_string_lossy().into_owned();
    let place = bipbridge::browsers::folder(config, "coach");
    std::fs::create_dir_all(place.join("profile/Default/Local Storage")).unwrap();
    std::fs::write(place.join("profile/Default/Cookies"), "logged in").unwrap();
    std::fs::write(place.join("profile/Default/Local Storage/big.ldb"), vec![b'x'; big]).unwrap();
    std::fs::write(place.join("profile/Local State"), "{\"os_crypt\": {}}").unwrap();
    std::fs::write(place.join("ports.json"), r#"{"cdp": 19600, "vnc": 19601}"#).unwrap();
    std::fs::write(
        place.join("mcp.json"),
        r#"{"server": ["/usr/bin/podman", "exec", "-i", "bipagents-browser-coach", "/opt/stealth/mcp.sh"]}"#,
    )
    .unwrap();
    place
}

/// Review 2: the person's « Oui » to a task's « Envoyer sur ce site ? » (form_send.rs `call` → `grant_for_now`, the
/// task's session) and the sites allowed for good are written to the AGENT's `send-sites.json`; the task's own
/// browser's proxy reads the copy made in its own folder at its start (browsers.rs `setup_task`, `Proxy::send_sites_path`
/// beside its `busy`). The task asks, the person says yes, « do the same step again », refused again: a task with a
/// browser of its own never sends a form to a new site.
#[tokio::test]
async fn a_persons_yes_to_a_tasks_sending_reaches_the_tasks_own_browser() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(1000, 10).await;
    let (mut config, agent) = setup(dir.path(), &url);
    agent_browser(&mut config, dir.path(), 16);
    let board = board(&config, None);
    let job = board.create(&agent, "Écrire au service client", "Envoie ma réclamation.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    let own = bipbridge::browsers::folder(&config, &bipbridge::browsers::task_browser("coach", &id));
    assert!(own.join("mcp.json").is_file(), "no browser of its own");
    // « Oui » to the task's question (its Hermes session is the fake's s1)
    bipbridge::form_send::grant_for_now(&config, "coach", "contact.example", Some("s1")).unwrap();
    // what the task's own browser's proxy reads before it lets a form through
    let settings: Dict = serde_json::from_str(&std::fs::read_to_string(own.join("mcp.json")).unwrap()).unwrap();
    let proxy = bipbridge::browser_mcp::Proxy::new(settings, Box::new(std::io::sink()));
    let now = bipbridge::py::time_time();
    let allowed = bipbridge::browser_mcp::send_allowed(&proxy.send_sites_path, "contact.example", &Default::default(), now, Some("s1"));
    board.forget(&id).await.unwrap();
    assert_eq!(allowed, Some(None), "the task's own browser still refuses after the « Oui » ({})", proxy.send_sites_path.display());
}

/// Review 2: what a task's own browser downloads lands in ITS profile's Downloads (`downloads_dir(<agent>--<task>)`)
/// and is moved to the agent's only at the task's end (`remove_task`); meanwhile the task's `give_file` /
/// `send_file_to_service` of « /telechargements/… » (sent_files.rs `places`, service_files.rs `recent_downloads`) read
/// the agent's: an invoice fetched by a task can't be handed over by that task (it was, when tasks used the agent's
/// browser).
#[tokio::test]
async fn a_tasks_download_can_be_given_while_the_task_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(1000, 10).await;
    let (mut config, agent) = setup(dir.path(), &url);
    agent_browser(&mut config, dir.path(), 16);
    let board = board(&config, None);
    let job = board.create(&agent, "Facture EDF", "Télécharge ma dernière facture et donne-la moi.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    let downloads = bipbridge::browsers::downloads_dir(&config, &bipbridge::browsers::task_browser("coach", &id));
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("facture-2026-11.pdf"), "%PDF-1.4 facture").unwrap();
    let given = bipbridge::runs::CALL_SESSION.sync_scope(Some("s1".into()), || {
        bipbridge::sent_files::capture(&config, &agent, "/telechargements/facture-2026-11.pdf")
    });
    board.forget(&id).await.unwrap();
    assert!(given.is_ok(), "the task's own download, given by the task: {given:?}");
}

/// Review 2: two tasks of one agent set up at once (a model's parallel start_task, two devices; `may_run` lets both
/// through) read the ports taken (`pick_ports`) before either writes its `ports.json`, the profile's copy in between:
/// the same two ports for both browsers (the second can't start, its DevTools secret shadows the first's in
/// `SECRETS`).
#[test]
fn two_tasks_set_up_at_once_get_ports_of_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let (mut config, _agent) = setup(dir.path(), "http://127.0.0.1:9");
    agent_browser(&mut config, dir.path(), 64 * 1024 * 1024);
    let gate = Arc::new(std::sync::Barrier::new(2));
    let starts: Vec<_> = ["t1", "t2"]
        .into_iter()
        .map(|job| {
            let (config, gate) = (config.clone(), gate.clone());
            std::thread::spawn(move || {
                gate.wait();
                bipbridge::browsers::setup_task(&config, "coach", job, &format!("s_{job}")).unwrap()
            })
        })
        .collect();
    for start in starts {
        assert!(start.join().unwrap().is_some());
    }
    let ports = |job: &str| bipbridge::browsers::ports_of(&config, &bipbridge::browsers::task_browser("coach", job)).unwrap();
    let (a, b) = (ports("t1"), ports("t2"));
    assert!(a["cdp"] != b["cdp"] && a["vnc"] != b["vnc"], "the same ports for two tasks' browsers: {a:?} {b:?}");
}

/// Review 2: an agent deleted while its task runs: the task fails (« this agent isn't on this install any more »,
/// `watch`) but its own browser (a copy of the agent's logins, its unit) is never removed — neither there nor by
/// `delete_agent` (browsers.rs `remove` takes the agent's folder only).
#[tokio::test]
async fn a_deleted_agents_task_leaves_no_copy_of_its_logins() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(1000, 10).await;
    let (mut config, agent) = setup(dir.path(), &url);
    agent_browser(&mut config, dir.path(), 16);
    let shared = SharedConfig::new(config.clone());
    let mut jobs = Jobs::new(shared.clone(), HermesClient::new(reqwest::Client::new()), None, None, None);
    jobs.poll = 0.01;
    let board = Arc::new(jobs);
    let job = board.create(&agent, "Comparer", "Compare trois offres.", 0.50, None).await.unwrap();
    let id = id_of(&job);
    let own = bipbridge::browsers::folder(&config, &bipbridge::browsers::task_browser("coach", &id));
    assert!(own.join("profile/Default/Cookies").is_file());
    shared.remove_agent("coach");
    for _ in 0..200 {
        if jobs::tasks(&config).iter().any(|j| j["id"] == json!(id) && j["state"] == "failed") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(jobs::tasks(&config).iter().any(|j| j["id"] == json!(id) && j["state"] == "failed"), "the task didn't end");
    assert!(!own.exists(), "the deleted agent's logins copied for its task are still there: {}", own.display());
}

/// Review 2 (focus 4: measured 0.5–0.8 GB per browsing task, 20/11): the only room check is at a task's start,
/// « pressed » at 90 % of the place's memory.high (heavy_work.rs `task_gate`); the browser of the task starts later,
/// on its first call, with no check. A place at 75 % of a 1.6 GB memory.high (a VPS-2 shared by four people: 80 % of
/// 8 GB ÷ 4, MemoryMax 1.5×) starts a task of an agent with a browser, and three of them at once: each browser takes
/// 0.5–0.8 GB more, past memory.high (throttled) and MemoryMax (the kernel kills in the place). A task whose browser
/// can't fit must wait for room (queued, never refused).
#[test]
fn a_task_whose_browser_cant_fit_in_the_room_waits_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut config, _agent) = setup(dir.path(), "http://127.0.0.1:9");
    agent_browser(&mut config, dir.path(), 16);
    let gb = 1_000_000_000u64;
    let room = bipbridge::heavy_work::Room {
        disk_total: Some(50 * gb),
        disk_free: Some(40 * gb),
        memory: bipbridge::heavy_work::Memory {
            current: Some(1_200_000_000),
            high: Some(1_600_000_000),
            stalled: Some(0.0),
            high_events: Some(0),
            anon: Some(1_000_000_000),
        },
        ..Default::default()
    };
    assert!(!room.memory.pressed(), "not pressed: the case is the browser's room, not the pressure");
    let gate = bipbridge::heavy_work::task_gate(&config, "coach", &room, bipbridge::py::time_time());
    assert!(gate.is_some(), "0.4 GB left under memory.high, a browsing task takes 0.5–0.8 GB: started anyway");
}

/// Review 2: the bridge's MCP servers run each call in a spawned task (card_pay.rs `serve_call`: `let mut work =
/// tokio::spawn(… (server.call)(…) …)`, used by bipagents_pay, bipagents_vault — vault_login, vault_create_login,
/// ask_to_send —, connectors, handover, link_wallet; agent_tools.rs `mcp_call`: `let work = tokio::spawn(… call_tool
/// …)`). The task-local `runs::CALL_SESSION` set around the handler (patch 0028) is gone inside: `browsers::task_of_call`
/// is always None there, so pay_with_card / vault_login / vault_create_login from a task with a browser of its own are
/// never handed over to the main conversation (Alex 20/11) but done in the AGENT's browser, the « Oui » of
/// ask_to_send is never the task's own, and `from_background` / heavy_work's pause fall back on guessing. (The unit
/// tests call `task_of_call` in `sync_scope` directly; `a_tasks_payment_is_handed_over…` calls `hand_over` directly.)
#[tokio::test]
async fn a_call_to_the_bridges_pay_and_vault_servers_keeps_its_session() {
    static SEEN: Mutex<Option<Option<String>>> = Mutex::new(None);
    fn probe<'a>(
        _state: &'a bipbridge::AppState,
        _agent: &'a AgentConfig,
        _name: &'a str,
        _arguments: &'a Dict,
        _asker: &'a bipbridge::card_pay::Asker,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Dict>> {
        Box::pin(async move {
            *SEEN.lock().unwrap() = Some(bipbridge::runs::CALL_SESSION.try_with(|s| s.clone()).ok().flatten());
            Ok(Dict::new())
        })
    }
    static PROBE: bipbridge::card_pay::ToolServer =
        bipbridge::card_pay::ToolServer { name: "probe", instructions: "", tools: Vec::new, call: probe };
    let dir = tempfile::tempdir().unwrap();
    let (config, _agent) = setup(dir.path(), "http://127.0.0.1:9");
    let mut tokens = indexmap::IndexMap::new();
    tokens.insert("coach".to_string(), bipbridge::agent_tools::digest("tok-coach-0123456789abcdef"));
    bipbridge::agent_tools::save_tokens(&config, &tokens).unwrap();
    let app = common::TestApp::new(config);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "vault_login", "arguments": {"site": "shop.example"},
        "_meta": {bipbridge::agent_tools::CALL_SESSION_META: "api_task"}}});
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer tok-coach-0123456789abcdef".parse().unwrap());
    headers.insert("accept", "application/json".parse().unwrap());
    let _ = bipbridge::card_pay::serve(app.app.state.clone(), headers, Bytes::from(body.to_string()), &PROBE).await;
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(seen, Some(Some("api_task".to_string())), "the call's Hermes session inside the server's call");
}

// -- Review 3 (subtasks-redesign, 20/11, head 357a87a0c) ---------------------------------------------------------------

/// Alex 20/11: short of memory a task « attend de la place », never refused, and starts again by itself. The queue
/// (`resume_after_memory`) only ever tries its OLDEST task: when that one can't be started (its agent deleted while it
/// waited, its agent at its cap of running tasks), the call returns 0 every minute and every task behind it waits
/// for ever, memory free or not.
#[tokio::test]
async fn review3_a_waiting_task_that_cant_start_never_holds_the_others_back() {
    let dir = tempfile::tempdir().unwrap();
    let (_fake, url) = gateway(1000, 10).await;
    let (mut config, agent) = setup(dir.path(), &url);
    config.browser.root = dir.path().join("browsers").to_string_lossy().into_owned();
    let place = bipbridge::browsers::folder(&config, "coach");
    std::fs::create_dir_all(place.join("profile")).unwrap();
    std::fs::write(place.join("ports.json"), r#"{"cdp": 19600, "vnc": 19601}"#).unwrap();
    std::fs::write(place.join("mcp.json"), r#"{"server": ["/usr/bin/podman"]}"#).unwrap();
    let free = Arc::new(Mutex::new(Some(600u64)));
    let mut jobs = Arc::try_unwrap(board(&config, None)).ok().unwrap();
    let probe = free.clone();
    jobs.memory = Arc::new(move || *probe.lock().unwrap());
    let board = Arc::new(jobs);
    let first = id_of(&board.create(&agent, "Comparer", "Compare trois offres.", 0.50, None).await.unwrap());
    // the oldest waiting task's agent deleted meanwhile (its task files stay: agents_admin.rs deletes no task)
    let mut orphan = bipbridge::jobs::load(&config, &first).unwrap();
    orphan.insert("agent".into(), json!("ancien"));
    let created = orphan.get("created").and_then(|v| v.as_f64()).unwrap_or(0.0);
    orphan.insert("created".into(), json!(created - 100.0));
    bipbridge::jobs::save(&config, &orphan).unwrap();
    let second = id_of(&board.create(&agent, "Réserver", "Trouve un créneau chez le dentiste.", 0.50, None).await.unwrap());
    assert_eq!(bipbridge::jobs::load(&config, &second).unwrap()["reason"], json!("memory"));
    *free.lock().unwrap() = Some(4096);
    for _ in 0..3 {
        board.resume_after_memory().await; // three minutes
    }
    assert_eq!(board.view(&second).unwrap()["state"], "running", "memory free again, the second task still waits behind the first");
    let _ = board.forget(&second).await;
}
