//! Small Bips in a conversation (Alex's B2–B5 of 26/11, internal/next/conversation-pages-bips.md part B): what works
//! for the conversation beside its agent, drawn by the apps as one light line each where it was launched.
//!
//! - a background task (jobs.rs, kind `task`) started from one of the thread's sessions (`from_session`): after the
//!   `start_task` tool row that started it (the last one before it started), else (« Lancer » on a card, the app)
//!   after the last row older than its start;
//! - a Hermes helper (`delegate_task`, « Handing a task to a helper… »): after its tool row; done when the tool came
//!   back, unless it handed the work off asynchronously (its result names a `deleg_…` id): then done once Hermes'
//!   report for that id (`[ASYNC DELEGATION …]`, a row the timeline never shows) is in the conversation.
//!
//! One `bips` item per place: the lines launched at the same place are one item (`coalesce`), its id from its place
//! (the row before it), so a line joining it later keeps it. The apps overlay a task's live state from `/v1/jobs`,
//! which they follow already, by its id; and since a phone catches up only after its last item (`after=`), every
//! answer also gives the thread's `bips` items as they are now (`now`, review of 27/11): the phone patches those it
//! holds (a helper's report came back, a line joined). Its answer says `"bips": true` (timeline.rs `Page::answer`):
//! an app seeing it drops its « Tâche de fond » banner; with an older bridge it keeps it.
//!
//! The tasks are read from their folder once per change of it (`tasks_of`: jobs.rs writes each by a rename), not at
//! every read of the thread.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::source::Row;
use super::timeline::Item;
use crate::Dict;

pub const KIND: &str = "bips";
/// A helper still « working » after this long (its turn stopped, its report never came): stopped.
pub const HELPER_MAX_SECONDS: f64 = 3.0 * 3600.0;

type Cached = Option<(PathBuf, Option<SystemTime>, Arc<Vec<Dict>>)>;
static TASKS: LazyLock<Mutex<Cached>> = LazyLock::new(|| Mutex::new(None));

/// Every task (jobs.rs `tasks`), read again only when their folder changed (a task saved: a rename in it).
pub fn tasks_of(config: &crate::config::Config) -> Arc<Vec<Dict>> {
    let folder = crate::jobs::folder(config);
    let changed = std::fs::metadata(&folder).and_then(|m| m.modified()).ok();
    let mut cache = TASKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((path, at, tasks)) = cache.as_ref() {
        if *path == folder && *at == changed && changed.is_some() {
            return tasks.clone();
        }
    }
    let tasks = Arc::new(crate::jobs::tasks(config));
    *cache = Some((folder, changed, tasks.clone()));
    tasks
}

/// What a task's line says of its state: (state, what it waits for).
pub fn task_state(job: &Dict) -> (&'static str, Option<&'static str>) {
    let reason = job.get("reason").and_then(Value::as_str).unwrap_or("");
    match job.get("state").and_then(Value::as_str).unwrap_or("") {
        "done" => ("done", None),
        "failed" => ("failed", None),
        "paused" if reason == "budget" => ("waiting", Some("budget")),
        "paused" if reason == "stopped" => ("stopped", None),
        "paused" => ("paused", None),
        _ => ("working", None),
    }
}

fn str_of<'a>(job: &'a Dict, key: &str) -> &'a str {
    job.get(key).and_then(Value::as_str).unwrap_or("")
}

fn num_of(job: &Dict, key: &str) -> Option<f64> {
    job.get(key).and_then(Value::as_f64)
}

/// The step a task is at, as its stored list says (the first not done), for the line's second words.
fn step_of(job: &Dict) -> Option<String> {
    let items = job.get("steps")?.get("items")?.as_array()?;
    items
        .iter()
        .find(|s| s.get("status").and_then(Value::as_str) != Some("done"))
        .and_then(|s| s.get("text").and_then(Value::as_str))
        .map(str::to_string)
}

/// A task's line, with where it goes (its start), when it is a small Bip of these sessions.
pub fn task_line(job: &Dict, agent: &str, sessions: &HashSet<&str>) -> Option<(f64, Value)> {
    if str_of(job, "kind") != "task" || str_of(job, "agent") != agent || str_of(job, "state") == "proposed" {
        return None;
    }
    if !sessions.contains(str_of(job, "from_session")) {
        return None;
    }
    let at = num_of(job, "started").or_else(|| num_of(job, "created"))?;
    let (state, waiting) = task_state(job);
    let session = Some(str_of(job, "session_id")).filter(|s| !s.is_empty());
    Some((at, json!({"id": str_of(job, "id"), "source": "task", "title": str_of(job, "title"), "state": state,
                     "waiting_for": waiting, "step": step_of(job), "session": session, "started": num_of(job, "started"),
                     "finished": num_of(job, "finished")})))
}

static DELEGATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"deleg_[0-9A-Za-z]+").expect("valid regex"));

/// The asynchronous hand-offs whose report came back in these rows (`[ASYNC DELEGATION … — deleg_…]`).
pub fn reports(rows: &[Row]) -> HashSet<String> {
    rows.iter()
        .filter(|r| r.role == "user")
        .filter_map(|r| r.content.as_str().map(str::to_string).or_else(|| {
            r.content.as_array().map(|parts| {
                parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
            })
        }))
        .filter(|t| crate::cronwatch::REPORT_PREFIXES.iter().any(|p| t.trim_start().starts_with(p)))
        .flat_map(|t| DELEGATION.find_iter(&t).map(|m| m.as_str().to_string()).collect::<Vec<_>>())
        .collect()
}

fn is_delegate(name: &str) -> bool {
    name == "delegate_task" || name.ends_with("_delegate_task")
}

static GOAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""goal"\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("valid regex"));

/// A helper's goals, from its call's arguments (`goal`, or `tasks[].goal` for a batch); arguments cut by the timeline
/// (a long `context`: no longer JSON) read by their `"goal"` fields.
fn goals(arguments: &str) -> Vec<String> {
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return GOAL
            .captures_iter(arguments)
            .filter_map(|c| serde_json::from_str::<String>(&format!("\"{}\"", &c[1])).ok())
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect();
    };
    if let Some(tasks) = args.get("tasks").and_then(Value::as_array) {
        let found: Vec<String> = tasks
            .iter()
            .filter_map(|t| t.get("goal").and_then(Value::as_str))
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    args.get("goal").and_then(Value::as_str).map(|g| vec![g.trim().to_string()]).filter(|g| !g[0].is_empty()).unwrap_or_default()
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// One line per goal of each `delegate_task` call in `items`, placed right after its row (`at` just after it). `now`:
/// a helper still « working » after `HELPER_MAX_SECONDS` is stopped (its turn stopped, its report never came).
pub fn helper_lines(items: &[Item], reports: &HashSet<String>, now: f64) -> Vec<(f64, String, String, Value)> {
    let mut out = Vec::new();
    for item in items.iter().filter(|i| i.kind() == "tool") {
        let Some(tool) = item.body.get("tool") else { continue };
        if !tool.get("name").and_then(Value::as_str).is_some_and(is_delegate) {
            continue;
        }
        let arguments = tool.get("arguments").and_then(Value::as_str).unwrap_or("");
        let result = tool.get("result").and_then(Value::as_str).unwrap_or("");
        let handed = DELEGATION.find(result).map(|m| m.as_str().to_string());
        let state = match tool.get("status").and_then(Value::as_str).unwrap_or("") {
            "running" => "working",
            "failed" => "failed",
            _ => match &handed {
                Some(id) if !reports.contains(id) => "working",
                _ => "done",
            },
        };
        let state = if state == "working" && item.at > 0.0 && now - item.at > HELPER_MAX_SECONDS { "stopped" } else { state };
        for (n, goal) in goals(arguments).iter().enumerate() {
            let id = format!("{}/{n}", handed.clone().unwrap_or_else(|| item.id.clone()));
            out.push((item.at + 0.001, item.segment.clone(), item.turn.clone(),
                      json!({"id": id, "source": "helper", "title": one_line(goal), "state": state, "waiting_for": null,
                             "step": null, "session": null, "started": item.at, "finished": null})));
        }
    }
    out
}

/// Where a task's line goes: just after the `start_task` row that started it (its title, within the 10 minutes before
/// its start, or a minute after), else just after its start (« Lancer » on a card, the app, the rescue: never an older
/// task's row).
pub fn task_at(items: &[Item], started: f64, title: &str) -> f64 {
    items
        .iter()
        .filter(|i| i.kind() == "tool" && i.at <= started + 60.0 && i.at >= started - 600.0)
        .filter(|i| {
            let tool = i.body.get("tool");
            tool.and_then(|t| t.get("name")).and_then(Value::as_str).is_some_and(|n| n == "start_task" || n.ends_with("_start_task"))
                && tool.and_then(|t| t.get("arguments")).and_then(Value::as_str).is_some_and(|a| {
                    serde_json::from_str::<Value>(a).ok().and_then(|v| v.get("title").and_then(Value::as_str).map(str::trim).map(str::to_string))
                        .is_some_and(|t| !t.is_empty() && (t == title.trim() || title.trim().starts_with(&t)))
                })
        })
        .map(|i| i.at + 0.001)
        .next_back()
        .unwrap_or(started)
}

/// A `bips` item for the Boîte's way of merging (timeline::merge places it by time).
pub fn item(id: String, at: f64, segment: String, turn: String, line: Value) -> Item {
    let mut body = Map::new();
    body.insert("kind".into(), json!(KIND));
    body.insert("bips".into(), json!([line]));
    Item { id, pos: (0, 0, 0), at, turn, segment, row: 0, body }
}

/// Lines launched at one place are one item: each `bips` item right after another takes its lines. Its id comes from
/// its place (`<the row before it>.b`): the same when a line joins it later, so the phone patches the item it holds.
pub fn coalesce(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for mut item in items {
        if item.kind() == KIND {
            if let Some(last) = out.last_mut().filter(|l| l.kind() == KIND) {
                let more = item.body.get("bips").and_then(Value::as_array).cloned().unwrap_or_default();
                if let Some(Value::Array(lines)) = last.body.get_mut("bips") {
                    lines.extend(more);
                }
                continue;
            }
            if let Some(before) = out.last() {
                item.id = format!("{}.b", before.id);
            }
        }
        out.push(item);
    }
    out
}

/// The thread's `bips` items as they are now, by id (every answer: the phone patches those it holds).
pub fn now(items: &[Item]) -> Value {
    let map: Map<String, Value> = items
        .iter()
        .filter(|i| i.kind() == KIND)
        .map(|i| (i.id.clone(), i.body.get("bips").cloned().unwrap_or_else(|| json!([]))))
        .collect();
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(v: Value) -> Dict {
        v.as_object().cloned().unwrap()
    }

    fn tool(id: &str, at: f64, name: &str, args: Value, result: Option<&str>, status: &str) -> Item {
        let body = json!({"kind": "tool", "tool": {"name": name, "call_id": id, "arguments": args.to_string(),
                                                    "result": result, "status": status}});
        Item { id: id.into(), pos: (1, 1, 1), at, turn: "m_1".into(), segment: "s1".into(), row: 1,
               body: body.as_object().cloned().unwrap() }
    }

    fn message(id: &str, at: f64) -> Item {
        let body = json!({"kind": "message", "role": "agent", "text": "ok"});
        Item { id: id.into(), pos: (1, 2, 0), at, turn: "m_1".into(), segment: "s1".into(), row: 2,
               body: body.as_object().cloned().unwrap() }
    }

    #[test]
    fn a_task_of_this_conversation_is_a_small_bip_with_its_state() {
        let sessions: HashSet<&str> = ["s1"].into_iter().collect();
        let running = job(json!({"id": "t1", "kind": "task", "agent": "vie", "title": "Comparer les mutuelles",
                                 "state": "running", "from_session": "s1", "started": 100.0, "session_id": "task_s",
                                 "steps": {"items": [{"text": "Lire", "status": "done"}, {"text": "Comparer", "status": "doing"}]}}));
        let (at, line) = task_line(&running, "vie", &sessions).expect("a small Bip");
        assert_eq!(at, 100.0);
        assert_eq!(line["state"], "working");
        assert_eq!(line["step"], "Comparer");
        assert_eq!(line["session"], "task_s");
        // its budget reached: it waits for the person
        let budget = job(json!({"id": "t2", "kind": "task", "agent": "vie", "title": "x", "state": "paused", "reason": "budget",
                                "from_session": "s1", "created": 90.0}));
        let (at, line) = task_line(&budget, "vie", &sessions).unwrap();
        assert_eq!((at, line["state"].clone(), line["waiting_for"].clone()), (90.0, json!("waiting"), json!("budget")));
        assert_eq!(task_state(&job(json!({"state": "paused", "reason": "stopped"}))).0, "stopped");
        assert_eq!(task_state(&job(json!({"state": "paused", "reason": "time"}))).0, "paused");
        assert_eq!(task_state(&job(json!({"state": "done"}))).0, "done");
        // another conversation's, another agent's, a proposal: none
        for other in [json!({"kind": "task", "agent": "vie", "state": "running", "from_session": "s9", "started": 1.0}),
                      json!({"kind": "task", "agent": "coach", "state": "running", "from_session": "s1", "started": 1.0}),
                      json!({"kind": "task", "agent": "vie", "state": "proposed", "from_session": "s1", "created": 1.0}),
                      json!({"kind": "mail_scan", "agent": "vie", "state": "running", "from_session": "s1", "started": 1.0})] {
            assert!(task_line(&job(other.clone()), "vie", &sessions).is_none(), "{other}");
        }
    }

    #[test]
    fn a_helper_is_done_when_its_report_came_back() {
        let sync = tool("c1", 10.0, "delegate_task", json!({"goal": "Chercher trois vestes"}), Some("trois vestes"), "done");
        let batch = tool("c2", 20.0, "mcp_hermes_delegate_task", json!({"tasks": [{"goal": "Météo"}, {"goal": "Trains"}]}),
                         Some("Started in the background: deleg_ab12"), "done");
        let running = tool("c3", 30.0, "delegate_task", json!({"goal": "Long\nsecond line"}), None, "running");
        let other = tool("c4", 40.0, "web_search", json!({"query": "x"}), Some("y"), "done");
        let items = [sync, batch, running, other];
        let lines = helper_lines(&items, &HashSet::new(), 100.0);
        let states: Vec<(String, String)> =
            lines.iter().map(|l| (l.3["title"].as_str().unwrap().to_string(), l.3["state"].as_str().unwrap().to_string())).collect();
        assert_eq!(states, [("Chercher trois vestes".into(), "done".into()), ("Météo".into(), "working".into()),
                            ("Trains".into(), "working".into()), ("Long".into(), "working".into())]);
        assert_eq!(lines[1].3["id"], "deleg_ab12/0");
        assert!((lines[0].0 - 10.001).abs() < 1e-9, "right after its row");
        // its report in the conversation: done
        let back: HashSet<String> = ["deleg_ab12".to_string()].into_iter().collect();
        assert!(helper_lines(&items, &back, 100.0).iter().filter(|l| l.3["id"].as_str().unwrap().starts_with("deleg_ab12"))
            .all(|l| l.3["state"] == "done"));
    }

    /// Review of 27/11: a helper whose turn stopped or whose report never came is not « working » for ever; one whose
    /// arguments the timeline cut (a long context) still has its line.
    #[test]
    fn a_helper_is_stopped_after_hours_and_read_from_cut_arguments() {
        let running = tool("c1", 10.0, "delegate_task", json!({"goal": "Long"}), None, "running");
        assert_eq!(helper_lines(std::slice::from_ref(&running), &HashSet::new(), 10.0 + HELPER_MAX_SECONDS + 1.0)[0].3["state"], "stopped");
        assert_eq!(helper_lines(&[running], &HashSet::new(), 20.0)[0].3["state"], "working");
        let mut cut = tool("c2", 10.0, "delegate_task", json!({}), Some("ok"), "done");
        cut.body.get_mut("tool").unwrap()["arguments"] =
            json!(r#"{"tasks": [{"goal": "Météo \"demain\""}, {"goal": "Trains", "context": "très long…"#);
        let titles: Vec<String> = helper_lines(&[cut], &HashSet::new(), 20.0).iter()
            .map(|l| l.3["title"].as_str().unwrap().to_string()).collect();
        assert_eq!(titles, ["Météo \"demain\"", "Trains"]);
    }

    #[test]
    fn a_report_names_its_hand_off() {
        let row = |role: &str, content: Value| Row { role: role.into(), content, timestamp: 1.0, ..Row::default() };
        let rows = [row("user", json!("[ASYNC DELEGATION BATCH COMPLETE — deleg_ab12]\n\nTask 1 (completed): ok")),
                    row("user", json!("deleg_zz99 is mentioned by the person")),
                    row("assistant", json!("[ASYNC DELEGATION deleg_no]"))];
        let found = reports(&rows);
        assert_eq!(found, ["deleg_ab12".to_string()].into_iter().collect());
    }

    #[test]
    fn a_task_goes_after_the_start_task_row_and_lines_at_one_place_are_one_item() {
        let items = [message("m1", 5.0), tool("c1", 10.0, "start_task", json!({"title": "x"}), Some("Started"), "done"),
                     message("m2", 12.0)];
        assert!((task_at(&items, 10.5, "x") - 10.001).abs() < 1e-9);
        assert_eq!(task_at(&items, -100.0, "x"), -100.0, "started from a card before any start_task");
        // review of 27/11: another task's start_task row (another title, or days before) is not its place
        assert_eq!(task_at(&items, 10.5, "y"), 10.5);
        assert_eq!(task_at(&items, 10.0 + 86_400.0, "x"), 10.0 + 86_400.0);
        let a = item("b_1".into(), 10.001, "s1".into(), "m_1".into(), json!({"id": "t1"}));
        let b = item("b_2".into(), 10.002, "s1".into(), "m_1".into(), json!({"id": "deleg_x/0"}));
        let merged = coalesce(vec![message("m1", 5.0), a, b, message("m2", 12.0)]);
        assert_eq!(merged.iter().map(|i| i.kind().to_string()).collect::<Vec<_>>(), ["message", "bips", "message"]);
        assert_eq!(merged[1].body["bips"].as_array().unwrap().len(), 2);
        // its id from its place: the same when a line joins it later
        assert_eq!(merged[1].id, "m1.b");
        assert_eq!(now(&merged)["m1.b"].as_array().unwrap().len(), 2);
    }
}
