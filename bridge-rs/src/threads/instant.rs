//! The agent's sentences said at once to a message sent while it worked (instant_reply.rs), as timeline items: an
//! agent message `i_<client_id>` with `instant: true`, right after the person's message it answers (matched by
//! `client_id`: its `s_` item, then Hermes' stored row), or at its time while that message waits in the queue. Dated
//! no earlier than its message (Hermes dates a steered row when the turn reads it, maybe a minute after the line was
//! said: an app ordering a turn's rows by time keeps the line under its message). A line whose message is in no row and
//! in no queue (its `s_` item expired, the queued message cancelled) is not shown.

use serde_json::{json, Map, Value};

use super::timeline::Item;
use super::{text, Param, Thread, Threads};
use crate::config::AgentConfig;
use crate::store::{iso, utcnow};

/// One kept sentence: the message it answers, its words, when it was said (Unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub client_id: String,
    pub text: String,
    pub at: f64,
}

impl Threads {
    pub async fn keep_instant(&self, agent: &str, thread: &str, client_id: &str, said: &str, by_model: bool) -> anyhow::Result<()> {
        self.store
            .run(
                "INSERT OR IGNORE INTO thread_instant (agent, client_id, thread_id, text, by_model, created_at) \
                 VALUES (?,?,?,?,?,?)",
                vec![text(agent), text(client_id), text(thread), text(said), Param::Integer(by_model as i64), text(&iso(utcnow()))],
            )
            .await?;
        Ok(())
    }

    /// The thread's sentences placed among its items.
    pub(super) async fn with_instant(&self, agent: &AgentConfig, thread: &Thread, items: &mut Vec<Item>, queued: &[String]) -> anyhow::Result<()> {
        let rows = self
            .store
            .run(
                "SELECT client_id, text, created_at FROM thread_instant WHERE agent = ? AND thread_id = ? ORDER BY created_at",
                vec![text(&agent.name), text(&thread.id)],
            )
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let said: Vec<Said> = rows
            .iter()
            .map(|r| Said {
                client_id: r["client_id"].as_str().unwrap_or("").to_string(),
                text: r["text"].as_str().unwrap_or("").to_string(),
                at: r["created_at"]
                    .as_str()
                    .and_then(|c| crate::store::parse_since(c).ok())
                    .map(|d| d.timestamp_micros() as f64 / 1e6)
                    .unwrap_or(0.0),
            })
            .collect();
        place(items, &said, queued);
        Ok(())
    }
}

fn body_of(said: &Said) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("kind".into(), json!("message"));
    body.insert("role".into(), json!("agent"));
    body.insert("text".into(), json!(said.text));
    body.insert("instant".into(), json!(true));
    body.insert("answers".into(), json!(said.client_id));
    body
}

/// Each sentence after the message it answers (dated no earlier than it), else, while that message waits in the queue
/// (`queued`: client ids), at its time (after the last item not later than it, at the end of that item's row); else
/// not at all. Positions kept strictly increasing (the bridge's own items at the end are numbered on).
pub fn place(items: &mut Vec<Item>, said: &[Said], queued: &[String]) {
    for s in said {
        if s.text.trim().is_empty() || items.iter().any(|i| i.id == format!("i_{}", s.client_id)) {
            continue;
        }
        let anchor = items
            .iter()
            .position(|i| i.kind() == "message" && i.body.get("client_id").and_then(Value::as_str) == Some(s.client_id.as_str()));
        let mut when = s.at;
        let (at, pos, turn, segment) = match anchor {
            Some(n) => {
                let a = &items[n];
                // a person's row draws one line (sub 0); the Boîte's messages after it are at 100 and on
                let sub = if a.id.starts_with("s_") { a.pos.2 + 1 } else { 50 };
                when = when.max(a.at);
                (n + 1, (a.pos.0, a.pos.1, sub), a.turn.clone(), a.segment.clone())
            }
            None if !queued.contains(&s.client_id) => continue,
            None => {
                // its message waits in the queue (`pending.queue` shows it): at its time, after that row's lines
                let mut n = items.iter().position(|i| i.at > s.at).unwrap_or(items.len());
                if n == 0 {
                    n = 1.min(items.len());
                }
                while n > 0 && n < items.len() && (items[n].pos.0, items[n].pos.1) == (items[n - 1].pos.0, items[n - 1].pos.1) {
                    n += 1;
                }
                match n.checked_sub(1).and_then(|p| items.get(p)) {
                    Some(p) => (n, (p.pos.0, p.pos.1, p.pos.2 + 1), p.turn.clone(), p.segment.clone()),
                    None => (0, (0, 0, 0), String::new(), String::new()),
                }
            }
        };
        let id = format!("i_{}", s.client_id);
        let turn = if turn.is_empty() { id.clone() } else { turn };
        items.insert(at, Item { id, pos, at: when, turn, segment, row: 0, body: body_of(s) });
    }
    // the bridge's own items at the end (`s_`, `i_`) follow each other
    for n in 1..items.len() {
        let own = items[n].id.starts_with("s_") || items[n].id.starts_with("i_");
        if own && items[n].pos <= items[n - 1].pos {
            let p = items[n - 1].pos;
            items[n].pos = (p.0, p.1, p.2 + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, pos: (u32, i64, u32), at: f64, body: Value) -> Item {
        Item { id: id.into(), pos, at, turn: "m_1".into(), segment: "s".into(), row: 0, body: body.as_object().cloned().unwrap() }
    }

    fn person(id: &str, pos: (u32, i64, u32), at: f64, client: &str) -> Item {
        item(id, pos, at, json!({"kind": "message", "role": "person", "text": "x", "client_id": client}))
    }

    fn said(client: &str, at: f64) -> Said {
        Said { client_id: client.into(), text: "Noté, j'en tiens compte.".into(), at }
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn after_the_stored_message_it_answers() {
        let mut items = vec![
            person("m_1", (0, 1, 0), 10.0, "c1"),
            item("m_2.t0", (0, 2, 1), 11.0, json!({"kind": "tool"})),
            person("m_3", (0, 3, 0), 20.0, "c2"),
            item("o_9", (0, 3, 100), 21.0, json!({"kind": "boite"})),
            item("m_4", (0, 4, 0), 30.0, json!({"kind": "message", "role": "agent", "text": "Voilà"})),
        ];
        place(&mut items, &[said("c2", 20.5)], &[]);
        assert_eq!(ids(&items), ["m_1", "m_2.t0", "m_3", "i_c2", "o_9", "m_4"]);
        assert_eq!(items[3].pos, (0, 3, 50));
        assert_eq!((items[3].body["instant"].clone(), items[3].body["role"].clone()), (json!(true), json!("agent")));
        assert!(items.windows(2).all(|w| w[0].pos < w[1].pos));
        // read again: once
        place(&mut items, &[said("c2", 20.5)], &[]);
        assert_eq!(items.len(), 6);
    }

    #[test]
    fn after_its_sent_item_at_the_end_the_others_numbered_on() {
        let mut items = vec![
            person("m_1", (0, 1, 0), 10.0, "c1"),
            person("s_c2", (0, 1, 1), 20.0, "c2"),
            person("s_c3", (0, 1, 2), 21.0, "c3"),
        ];
        place(&mut items, &[said("c2", 20.5)], &[]);
        assert_eq!(ids(&items), ["m_1", "s_c2", "i_c2", "s_c3"]);
        assert!(items.windows(2).all(|w| w[0].pos < w[1].pos), "{:?}", items.iter().map(|i| i.pos).collect::<Vec<_>>());
    }

    #[test]
    fn its_message_in_the_queue_at_its_time_after_that_rows_lines() {
        let mut items = vec![
            person("m_1", (0, 1, 0), 10.0, "c1"),
            item("m_2", (0, 2, 0), 15.0, json!({"kind": "message", "role": "agent", "text": "Je cherche"})),
            item("m_2.t0", (0, 2, 1), 15.0, json!({"kind": "tool"})),
            item("m_3", (0, 3, 0), 40.0, json!({"kind": "message", "role": "agent", "text": "Voilà"})),
        ];
        place(&mut items, &[said("c9", 20.0)], &["c9".to_string()]);
        assert_eq!(ids(&items), ["m_1", "m_2", "m_2.t0", "i_c9", "m_3"]);
        assert_eq!(items[3].pos, (0, 2, 2));
        assert!(items.windows(2).all(|w| w[0].pos < w[1].pos));
        // its message no longer waits (cancelled) nor stands in any row (its `s_` item expired): no line alone
        let mut alone = vec![person("m_1", (0, 1, 0), 10.0, "c1")];
        place(&mut alone, &[said("c9", 20.0)], &[]);
        assert_eq!(ids(&alone), ["m_1"]);
    }

    #[test]
    fn never_dated_before_the_message_it_answers() {
        // Hermes stored the steered words when the turn read them, 40 s after the line was said
        let mut items = vec![person("m_1", (0, 1, 0), 10.0, "c1"), person("m_3", (0, 3, 0), 61.0, "c2")];
        place(&mut items, &[said("c2", 21.0)], &[]);
        assert_eq!(items[2].id, "i_c2");
        assert_eq!(items[2].at, 61.0);
    }
}
