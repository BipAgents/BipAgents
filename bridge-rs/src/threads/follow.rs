//! A sent message followed until its reply (thin apps, brick 6): the quick talk (« Dictée rapide », the Action
//! button, the Android panel) asks the bridge where the person's words are, instead of reading the timeline and the
//! run's events with its own rules (until 6/11 the same rules twice, iOS `QuickTalkFollower.swift` and Android
//! `QuickTalkFollower.kt`).
//!
//! ```text
//! GET /v1/threads/{thread}/sends/{client_id}?agent=
//!     → {client_id, thread, state, message, run, needs, reply, next}
//! ```
//!
//! - `state`: `unknown` (no such message here, or not yet), `queued` (it waits in the agent's queue), `sending`
//!   (kept by the bridge, its turn about to start), `running` (its turn goes on), `needs` (the turn needs the person: `needs` says why), `reply` (the turn is over,
//!   `reply` is what to show and say), `not_sent` (no turn took it and none runs: the conversation offers it again);
//! - `message`: the person's item (where the app opens);
//! - `run`: while it runs, `{run_id, tool, answer}`: the tool going on (its raw name, null: it thinks or writes) and
//!   the answer's words so far, as the bridge follows the run;
//! - `needs`: `approval`, `payment`, `question`, `pick`, `task` (a card of this turn, a tool whose card is to act on);
//! - `reply`: `{id, text, needs}`: the agent's messages after the person's joined (the last one's id), `needs` `media`
//!   or `pick` when it holds what a voice can't say (a file, a page, a card);
//! - `next`: when to ask again (milliseconds; null: nothing more will change for this message). The thread's hint
//!   (`GET /v1/threads/events?thread=`) is a reason to ask sooner.
//!
//! Only the cards asked since the person's words count: one asked before them is not the quick talk's to hand over
//! (5/11 and before: any waiting card made every quick talk open the app), even when it waits at the end of the turn
//! the words were added to (a card goes after its turn's last words, timeline::place_waiting): a quick dictation sent
//! into a turn is never held by a card it isn't about.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::timeline::Item;

/// How often the app asks while the turn goes on (the island's working line and the answer's first words).
pub const ASK_RUNNING_MS: u64 = 1000;
/// How often while the words wait for a turn (or are not there yet).
pub const ASK_WAITING_MS: u64 = 1500;

static PAYMENT_HANDOVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^Payment hand-over (hov_[0-9a-f]{12}): ([0-9]+\.[0-9]{2}) ([A-Z]{3}) on (\S+) for "([^"]*)" at <(https://[^\s<>"]+)>\."#)
        .expect("valid regex")
});
static LINK_PAYMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^Link payment (lnk_[0-9a-f]{12}): ([0-9]+\.[0-9]{2}) ([A-Z]{3}) on (\S+) for "([^"]*)"\."#).expect("valid regex")
});

/// The run going on, as the bridge follows it (runs.rs `glance`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub run_id: String,
    pub tool: Option<String>,
    pub answer: String,
}

/// A tool's name without its MCP server (`mcp__<server>__<tool>`, earlier `mcp_<server>_<tool>`), lower case.
fn plain(tool: &str) -> String {
    let tool = tool.trim();
    if let Some(rest) = tool.strip_prefix("mcp__") {
        return rest.split_once("__").map_or(rest, |(_, name)| name).to_lowercase();
    }
    for server in ["bipagents_files", "bipagents_pay", "bipagents", "stealth"] {
        if let Some(name) = tool.strip_prefix(&format!("mcp_{server}_")) {
            return name.to_lowercase();
        }
    }
    tool.to_lowercase()
}

/// A tool whose result the person acts on (its card is in the conversation, not on the island): why.
pub fn tool_reason(tool: &str) -> Option<&'static str> {
    match plain(tool).as_str() {
        "list_cards" | "pay_with_card" | "pay_with_link" | "confirm_payment" | "cancel_payment" => Some("payment"),
        "propose_task" | "start_task" | "notebook_fill" => Some("task"),
        "propose_agent" => Some("pick"),
        "clarify" => Some("question"),
        "ask_secret" => Some("approval"),
        _ => None,
    }
}

/// An approval's command: a payment (a cart handed over, a Link payment) or any other approval.
pub fn approval_reason(command: Option<&str>) -> &'static str {
    match command {
        Some(c) if PAYMENT_HANDOVER.is_match(c) || LINK_PAYMENT.is_match(c) => "payment",
        _ => "approval",
    }
}

/// A reply that holds more than words: a card to pick from, or a file, a page, a map.
pub fn files_reason(paths: &[String]) -> Option<&'static str> {
    if paths.iter().any(|p| p.starts_with("/bipagents/cards/") || p.starts_with("/demo/card/")) {
        Some("pick")
    } else if paths.is_empty() {
        None
    } else {
        Some("media")
    }
}

fn role(item: &Item) -> &str {
    if item.kind() == "message" {
        item.body.get("role").and_then(Value::as_str).unwrap_or("")
    } else {
        ""
    }
}

fn files(item: &Item) -> Vec<String> {
    item.body
        .get("attachments")
        .and_then(Value::as_array)
        .map(|atts| {
            atts.iter()
                .filter_map(|a| a.get("path").and_then(Value::as_str).or_else(|| a.get("id").and_then(Value::as_str)))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a waiting row is about the person's message at `at`: placed at or after it (its `after`,
/// timeline::place_waiting) and, with a time, asked at or after it. A card waits at the end of the turn it was asked in
/// (22/11), so one asked before a message added to that turn is placed after it too: it is not that message's, and a
/// quick talk sent into the turn is never held by it (22/11 relecture 2, R1).
fn of_this_turn(row: &Value, items: &[Item], at: usize) -> bool {
    let placed = match row.get("after").and_then(Value::as_str) {
        Some(after) => items.iter().position(|i| i.id == after).is_some_and(|k| k >= at),
        None => false,
    };
    let said = items[at].at;
    let asked = match row.get("created_at") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => crate::store::parse_since(s).ok().map(|d| d.timestamp_micros() as f64 / 1e6),
        _ => None,
    };
    placed && (said <= 0.0 || asked.is_none_or(|asked| asked >= said))
}

/// Where the message `client_id` is, decided from the thread's items (with its kept sends), its waiting cards (placed:
/// `after`), the run going on in it, and whether it waits in the agent's queue (`queued`).
pub fn state_of(
    items: &[Item],
    approvals: &[Value],
    questions: &[Value],
    live: Option<&Live>,
    queued: bool,
    client_id: &str,
    thread: &str,
) -> Value {
    let mut out = json!({"client_id": client_id, "thread": thread, "state": "unknown", "message": null, "run": null,
                         "needs": null, "reply": null, "next": ASK_WAITING_MS});
    let found = items.iter().position(|i| {
        role(i) == "person" && i.body.get("client_id").and_then(Value::as_str).is_some_and(|c| c.eq_ignore_ascii_case(client_id))
    });
    let Some(at) = found else {
        // waiting in the agent's queue (msg_queue.rs: not an item until its turn starts)
        if queued {
            out["state"] = json!("queued");
        }
        return out;
    };
    let person = &items[at];
    out["message"] = json!(person.id);
    // the person's words kept by the bridge (send.rs) and nothing runs: their turn is about to start, or none took them
    if person.body.contains_key("sending") && live.is_none() {
        if super::send::lost(person) {
            out["state"] = json!("not_sent");
            out["next"] = Value::Null;
        } else {
            out["state"] = json!("sending");
        }
        return out;
    }
    let needs = |reason: &str, mut out: Value| {
        out["state"] = json!("needs");
        out["needs"] = json!(reason);
        out["next"] = Value::Null;
        out
    };
    if let Some(card) = approvals.iter().find(|a| of_this_turn(a, items, at)) {
        return needs(approval_reason(card.get("command").and_then(Value::as_str)), out);
    }
    if questions.iter().any(|q| of_this_turn(q, items, at)) {
        return needs("question", out);
    }
    // a message the person sent meanwhile (another device) starts another turn: the reply is what came before it
    let turn: Vec<&Item> = items[at + 1..].iter().take_while(|i| role(i) != "person").collect();
    let tool = turn.iter().filter(|i| i.kind() == "tool").find_map(|i| {
        i.body.get("tool").and_then(|t| t.get("name")).and_then(Value::as_str).and_then(tool_reason)
    });
    if let Some(reason) = tool {
        return needs(reason, out);
    }
    if let Some(live) = live {
        // a card-bearing tool started (not stored yet): handed over now, as the run's events said it before
        if let Some(reason) = live.tool.as_deref().and_then(tool_reason) {
            return needs(reason, out);
        }
        out["state"] = json!("running");
        out["run"] = json!({"run_id": live.run_id, "tool": live.tool, "answer": live.answer});
        out["next"] = json!(ASK_RUNNING_MS);
        return out;
    }
    let answer: Vec<&&Item> =
        // never the line said at once to a message added meanwhile (instant_reply.rs): not the turn's reply
        turn.iter()
            .filter(|i| role(i) == "agent" && !crate::instant_reply::is_instant(&i.body))
            .filter(|i| !i.text().is_empty() || !files(i).is_empty())
            .collect();
    let Some(last) = answer.last() else {
        // its turn over with no words yet (stored a moment later): asked again soon
        out["state"] = json!("running");
        return out;
    };
    let text: Vec<&str> = answer.iter().map(|i| i.text()).filter(|t| !t.is_empty()).collect();
    let paths: Vec<String> = answer.iter().flat_map(|i| files(i)).collect();
    out["state"] = json!("reply");
    out["reply"] = json!({"id": last.id, "text": text.join("\n\n"), "needs": files_reason(&paths)});
    // what the voice says: each reply's spoken version where it has one (spoken.rs), its words elsewhere
    if answer.iter().any(|i| i.body.contains_key("spoken")) {
        let said: Vec<&str> = answer.iter().map(|i| i.body.get("spoken").and_then(Value::as_str).unwrap_or(i.text())).collect();
        out["reply"]["spoken"] = json!(said.into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n"));
    }
    out["next"] = Value::Null;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn item(id: &str, n: i64, at: f64, body: Value) -> Item {
        let body: Map<String, Value> = body.as_object().cloned().unwrap_or_default();
        Item { id: id.into(), pos: (0, n, 0), at, turn: String::new(), segment: "s".into(), row: n, body }
    }

    fn thread() -> Vec<Item> {
        vec![
            item("m_1", 1, 100.0, json!({"kind": "message", "role": "person", "text": "avant"})),
            item("m_2", 2, 110.0, json!({"kind": "message", "role": "agent", "text": "ok"})),
            item("m_3", 3, 200.0, json!({"kind": "message", "role": "person", "text": "Quel temps ?", "client_id": "QT-1"})),
        ]
    }

    #[test]
    fn a_reply_is_the_agents_words_after_the_persons_once_the_turn_is_over() {
        let mut items = thread();
        assert_eq!(state_of(&items, &[], &[], None, false, "QT-1", "t")["state"], "running");
        items.push(item("m_4", 4, 201.0, json!({"kind": "message", "role": "agent", "text": "Demain : soleil."})));
        items.push(item("m_5", 5, 202.0, json!({"kind": "message", "role": "agent", "text": "Prends une veste."})));
        let live = Live { run_id: "r1".into(), tool: None, answer: "Demain : so".into() };
        let running = state_of(&items, &[], &[], Some(&live), false, "qt-1", "t");
        assert_eq!(running["state"], "running");
        assert_eq!(running["run"]["answer"], "Demain : so");
        assert_eq!(running["message"], "m_3");
        let done = state_of(&items, &[], &[], None, false, "QT-1", "t");
        assert_eq!(done["state"], "reply");
        assert_eq!(done["reply"], json!({"id": "m_5", "text": "Demain : soleil.\n\nPrends une veste.", "needs": null}));
        assert_eq!(done["next"], Value::Null);
        // a reply's spoken version (spoken.rs): what the voice says, its words elsewhere
        let mut spoken = items.clone();
        spoken[4].body.insert("spoken".into(), json!("Mets une veste."));
        let said = state_of(&spoken, &[], &[], None, false, "QT-1", "t");
        assert_eq!(said["reply"]["spoken"], "Demain : soleil.\n\nMets une veste.");
        assert_eq!(said["reply"]["text"], "Demain : soleil.\n\nPrends une veste.");
        // a message sent meanwhile starts another turn: the reply stops before it
        items.insert(4, item("m_6", 6, 201.5, json!({"kind": "message", "role": "person", "text": "et lundi ?"})));
        assert_eq!(state_of(&items, &[], &[], None, false, "QT-1", "t")["reply"]["text"], "Demain : soleil.");
    }

    #[test]
    fn only_this_turns_cards_hand_the_quick_talk_over() {
        let mut items = thread();
        let old = json!({"command": "rm -rf /tmp/x", "after": "m_2"});
        assert_eq!(state_of(&items, std::slice::from_ref(&old), &[], None, false, "QT-1", "t")["state"], "running");
        let pay = json!({"command": "Payment hand-over hov_0123456789ab: 34.90 EUR on exemple.fr for \"Robe\" at <https://exemple.fr/c>. Choose…",
                         "after": "m_3"});
        let got = state_of(&items, &[old, pay], &[], None, false, "QT-1", "t");
        assert_eq!((got["state"].as_str(), got["needs"].as_str()), (Some("needs"), Some("payment")));
        assert_eq!(state_of(&items, &[], &[json!({"after": "m_3"})], None, false, "QT-1", "t")["needs"], "question");
        items.push(item("m_4.t0", 4, 201.0, json!({"kind": "tool", "tool": {"name": "mcp__bipagents__propose_task"}})));
        assert_eq!(state_of(&items, &[], &[], None, false, "QT-1", "t")["needs"], "task");
        let live = Live { run_id: "r1".into(), tool: Some("propose_agent".into()), answer: String::new() };
        assert_eq!(state_of(&thread(), &[], &[], Some(&live), false, "QT-1", "t")["needs"], "pick");
    }

    /// 22/11 relecture 2, R1: a card waits at the end of the turn it was asked in, so one asked before a message the
    /// person added to that turn (a quick dictation into it) is placed after that message too. It is not that
    /// message's: the quick talk is never held by it. A card asked since, or with no time, is.
    #[test]
    fn a_card_asked_before_words_added_to_its_turn_never_holds_their_quick_talk() {
        let mut items = thread();
        items[2].body.insert("added_during_run".into(), json!(true));
        items.push(item("m_4", 4, 210.0, json!({"kind": "message", "role": "agent", "text": "J'attends ta réponse."})));
        let earlier = json!({"command": "rm -rf build/", "after": "m_4", "created_at": 150.0});
        assert_eq!(state_of(&items, std::slice::from_ref(&earlier), &[], None, false, "QT-1", "t")["state"], "reply");
        let since = json!({"command": "rm -rf build/", "after": "m_4", "created_at": 205.0});
        assert_eq!(state_of(&items, &[earlier.clone(), since], &[], None, false, "QT-1", "t")["needs"], "approval");
        let untimed = json!({"command": "rm -rf build/", "after": "m_4"});
        assert_eq!(state_of(&items, &[untimed], &[], None, false, "QT-1", "t")["needs"], "approval");
        let question = json!({"after": "m_4", "created_at": "1970-01-01T00:02:30Z"});
        assert_eq!(state_of(&items, &[], &[question], None, false, "QT-1", "t")["state"], "reply");
    }

    #[test]
    fn a_reply_with_a_file_or_a_card_says_so() {
        let mut items = thread();
        items.push(item("m_4", 4, 201.0, json!({"kind": "message", "role": "agent", "text": "Voilà",
                                                "attachments": [{"id": "a1", "path": "/bipagents/cards/c1.html"}]})));
        assert_eq!(state_of(&items, &[], &[], None, false, "QT-1", "t")["reply"]["needs"], "pick");
        assert_eq!(files_reason(&["/tmp/plan.pdf".into()]), Some("media"));
        assert_eq!(approval_reason(Some("Link payment lnk_0123456789ab: 34.90 USD on shop.example for \"Shoes\". Approve")), "payment");
        assert_eq!(tool_reason("mcp__bipagents_pay__pay_with_card"), Some("payment"));
        assert_eq!(tool_reason("mcp_bipagents_pay_confirm_payment"), Some("payment"));
        assert_eq!(tool_reason("web_search"), None);
    }

    #[test]
    fn words_not_there_yet_or_kept_by_the_bridge() {
        assert_eq!(state_of(&thread(), &[], &[], None, false, "QT-2", "t")["state"], "unknown");
        let mut items = thread();
        items.push(item("s_QT-2", 4, crate::store::utcnow().timestamp() as f64,
                        json!({"kind": "message", "role": "person", "text": "x", "client_id": "QT-2", "sending": true})));
        assert_eq!(state_of(&items, &[], &[], None, false, "QT-2", "t")["state"], "sending");
        items[3].at -= 60.0;
        let lost = state_of(&items, &[], &[], None, false, "QT-2", "t");
        assert_eq!((lost["state"].as_str(), lost["next"].is_null()), (Some("not_sent"), true));
        // its turn runs before Hermes stores the words: the turn's card (placed after them) hands over
        let live = Live { run_id: "r1".into(), ..Default::default() };
        assert_eq!(state_of(&items, &[], &[], Some(&live), false, "QT-2", "t")["state"], "running");
        let card = json!({"command": "rm -rf build/", "after": "s_QT-2"});
        assert_eq!(state_of(&items, &[card], &[], Some(&live), false, "QT-2", "t")["needs"], "approval");
        assert_eq!(state_of(&thread(), &[], &[], None, true, "QT-3", "t")["state"], "queued");
    }
}
