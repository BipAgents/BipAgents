//! The Carnet's dated tasks on the person's phones, from the outside (the app's routes and an agent's tool): an iPhone
//! puts them in its Reminders, an Android phone in its calendar (« Dans l'agenda », 7/11), the phone they were asked
//! from first, never both. Ticked or removed in the Carnet, the Android phone takes its event off (the « cochée »
//! request: `complete`, by the request that made it).

mod common;

use bipbridge::config::{parse_config, Config};
use bipbridge::notebook;
use bipbridge::Dict;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

fn with_coach(dir: &std::path::Path) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    parse_config(&dict, None).unwrap()
}

fn queue(value: Value) -> Vec<Value> {
    value["queue"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_dated_task_from_the_android_app_goes_to_its_calendar_and_leaves_it_when_ticked() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_coach(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    app.send_json("PUT", "/v1/reminders/agents", &h, &json!({"agents": ["coach"]})).await;
    // the Android 1.1 app says it may write to its calendar and takes the Carnet's tasks (no iPhone shares here)
    let said =
        app.send_json("PUT", "/v1/reminders/android", &h, &json!({"calendar": true, "carnet": true, "items": []})).await;
    assert_eq!(said.json(), json!({"items": 0}));
    let added = app
        .send_json("POST", "/v1/notebook/tasks", &h, &json!({"title": "Appeler le dentiste", "due": "2026-11-28T10:30", "phone": "android"}))
        .await
        .json();
    assert_eq!(added["reminder"], true, "{added}");
    let task = added["task"]["id"].as_str().unwrap().to_string();
    // the Android phone's request, not the iPhone's: one, never both
    let android = queue(app.get("/v1/reminders/queue?phone=android", &h).await.json());
    assert_eq!(android.len(), 1, "{android:?}");
    assert!(queue(app.get("/v1/reminders/queue", &h).await.json()).is_empty());
    let add = &android[0];
    assert_eq!((&add["op"], &add["phone"]), (&json!("add"), &json!("android")));
    assert_eq!((&add["args"]["title"], &add["args"]["due"]), (&json!("Appeler le dentiste"), &json!("2026-11-28T10:30")));
    assert!(add["args"]["notes"].as_str().unwrap().ends_with(&format!("^{task}")));
    // the person's own task: no agent's name (the app writes « Rappel de BipAgents », never « de notebook »)
    assert_eq!(add["agent"], "");
    let made = add["id"].as_str().unwrap().to_string();
    app.send_json("POST", &format!("/v1/reminders/queue/{made}"), &h, &json!({"done": true})).await;
    app.send_json(
        "PUT",
        "/v1/reminders/android",
        &h,
        &json!({"calendar": true, "carnet": true,
                "items": [{"id": made, "title": "Appeler le dentiste", "due": "2026-11-28T10:30:00+01:00"}]}),
    )
    .await;
    // ticked in the Carnet: the « cochée » request, for the Android phone, naming the request that made the event
    let ticked = app.send_json("POST", &format!("/v1/notebook/tasks/{task}"), &h, &json!({"done": true})).await;
    assert_eq!(ticked.status, 200);
    let android = queue(app.get("/v1/reminders/queue?phone=android", &h).await.json());
    assert_eq!(android.len(), 1, "{android:?}");
    assert_eq!((&android[0]["op"], &android[0]["args"]), (&json!("complete"), &json!({"id": made})));
    assert!(queue(app.get("/v1/reminders/queue", &h).await.json()).is_empty());
}

#[tokio::test]
async fn with_both_phones_the_app_asking_decides_and_an_older_app_keeps_the_iphone_first() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_coach(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    app.send_json("PUT", "/v1/reminders/agents", &h, &json!({"agents": ["coach"]})).await;
    app.send_json("PUT", "/v1/reminders", &h, &json!({"lists": [], "items": []})).await; // the iPhone shares
    app.send_json("PUT", "/v1/reminders/android", &h, &json!({"calendar": true, "carnet": true, "items": []})).await;
    let add = |title: &str, phone: Option<&str>| {
        let mut body = json!({"title": title, "due": "2026-11-28"});
        if let Some(phone) = phone {
            body["phone"] = json!(phone);
        }
        body
    };
    for (title, phone) in [("Payer la cantine", Some("android")), ("Arroser les plantes", Some("ios")), ("Réserver", None)] {
        let added = app.send_json("POST", "/v1/notebook/tasks", &h, &add(title, phone)).await.json();
        assert_eq!(added["reminder"], true, "{title}: {added}");
    }
    let titles = |ops: Vec<Value>| ops.iter().map(|o| o["args"]["title"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(titles(queue(app.get("/v1/reminders/queue?phone=android", &h).await.json())), ["Payer la cantine"]);
    assert_eq!(titles(queue(app.get("/v1/reminders/queue", &h).await.json())), ["Arroser les plantes", "Réserver"]);
    // the same task again, from the other phone: already there, no second reminder anywhere
    let again = app.send_json("POST", "/v1/notebook/tasks", &h, &add("Payer la cantine", Some("ios"))).await.json();
    assert_eq!((&again["existing"], &again["reminder"]), (&json!(true), &json!(false)));
    assert_eq!(queue(app.get("/v1/reminders/queue?phone=android", &h).await.json()).len(), 1);
    assert_eq!(queue(app.get("/v1/reminders/queue", &h).await.json()).len(), 2);
    // a phone the bridge doesn't know: refused as any wrong field, as a message's (threads/send.rs), long or short
    for phone in ["x".repeat(30), "windows".to_string()] {
        let wrong = app.send_json("POST", "/v1/notebook/tasks", &h, &add("Vaccin", Some(phone.as_str()))).await;
        assert_eq!(wrong.status, 422, "{phone}");
        let answer = json!({"accept": true, "phone": phone});
        assert_eq!(app.send_json("POST", "/v1/notebook/proposals/p0", &h, &answer).await.status, 422, "{phone}");
    }
    // a proposal said yes to on the Android phone: its task's reminder in that phone's calendar
    let proposal = notebook::propose(&config, "coach", "deadline", "Renouveler le passeport", "2026-12-15").unwrap();
    let yes = app
        .send_json("POST", &format!("/v1/notebook/proposals/{proposal}"), &h, &json!({"accept": true, "phone": "android"}))
        .await
        .json();
    assert_eq!(yes["reminder"], true, "{yes}");
    let android = titles(queue(app.get("/v1/reminders/queue?phone=android", &h).await.json()));
    assert_eq!(android, ["Payer la cantine", "Renouveler le passeport"]);
}

#[test]
fn an_agents_dated_task_in_a_turn_from_android_is_in_the_calendar_and_the_agent_is_told_so() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_coach(dir.path());
    let mut data = Dict::new();
    data.insert("agents".into(), json!(["coach"]));
    data.insert("at".into(), json!(bipbridge::py::time_time())); // an iPhone shares its Reminders too
    data.insert("android".into(), json!({"at": bipbridge::py::time_time(), "calendar": true, "carnet": true, "items": []}));
    bipbridge::reminders::write(&config, &data).unwrap();
    let arguments: Dict =
        serde_json::from_value(json!({"action": "add", "title": "Appeler Paul", "due": "2026-11-28T14:00"})).unwrap();
    let turn = notebook::reads::Turn { phone: Some("android".into()), ..Default::default() };
    let said = notebook::tools::call(&config, "coach", notebook::tools::TASK, &arguments, &turn).unwrap();
    let text = said["content"][0]["text"].as_str().unwrap().to_string();
    // « Phone reminder too » wakes the phone (agent_tools.rs); the agent says « dans ton agenda », not « Rappels »
    assert!(text.contains("Phone reminder too") && text.contains("calendar") && !text.contains("iPhone"), "{text}");
    let op = bipbridge::reminders::read(&config)["queue"][0].clone();
    assert_eq!((&op["phone"], &op["agent"]), (&json!("android"), &json!("coach")));
    // the same in a turn from the iPhone: its Reminders, and the agent told nothing of a calendar
    let arguments: Dict =
        serde_json::from_value(json!({"action": "add", "title": "Appeler Marie", "due": "2026-11-28T14:00"})).unwrap();
    let turn = notebook::reads::Turn { phone: Some("ios".into()), ..Default::default() };
    let said = notebook::tools::call(&config, "coach", notebook::tools::TASK, &arguments, &turn).unwrap();
    let text = said["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("Phone reminder too") && !text.contains("calendar"), "{text}");
    assert_eq!(bipbridge::reminders::read(&config)["queue"][1].get("phone"), None);
}

/// Relecture 1, B1 (Alex, 21/11: « A, porte en 1.1 »): an Android app of 1.0 says nothing of the Carnet in its copy, so
/// it never gets the Carnet's dated tasks — not even those added from it — and an iPhone keeps them as before.
#[tokio::test]
async fn an_android_app_of_1_0_keeps_exactly_what_it_did() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_coach(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    app.send_json("PUT", "/v1/reminders/agents", &h, &json!({"agents": ["coach"]})).await;
    app.send_json("PUT", "/v1/reminders/android", &h, &json!({"calendar": true, "items": []})).await;
    let task = json!({"title": "Appeler le dentiste", "due": "2026-11-28T10:30", "phone": "android"});
    let added = app.send_json("POST", "/v1/notebook/tasks", &h, &task).await.json();
    assert_eq!(added["reminder"], false, "{added}");
    assert!(queue(app.get("/v1/reminders/queue?phone=android", &h).await.json()).is_empty());
    // an iPhone sharing too: the task goes to its Reminders, as before 1.1
    app.send_json("PUT", "/v1/reminders", &h, &json!({"lists": [], "items": []})).await;
    let task = json!({"title": "Payer la cantine", "due": "2026-11-28", "phone": "android"});
    assert_eq!(app.send_json("POST", "/v1/notebook/tasks", &h, &task).await.json()["reminder"], true);
    assert!(queue(app.get("/v1/reminders/queue?phone=android", &h).await.json()).is_empty());
    let iphone = queue(app.get("/v1/reminders/queue", &h).await.json());
    assert_eq!(iphone.iter().map(|o| o["args"]["title"].clone()).collect::<Vec<_>>(), [json!("Payer la cantine")]);
    // the stored word: only what the app said (a 1.1 app updated says it at its next sync)
    let stored = bipbridge::reminders::read(&config);
    assert_eq!(stored["android"].get("carnet"), Some(&json!(false)));
}
