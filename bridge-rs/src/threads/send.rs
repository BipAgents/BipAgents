//! Sending through the bridge (docs/conversations.md §6.7): every message of the person goes to the bridge with the
//! app's own id (`client_id`), and the bridge decides what becomes of it, in one place:
//!
//! ```text
//! POST /v1/threads/{thread}/messages {agent?, client_id, text, attachments?, notes?, kinds?, after?, instructions?}
//!      thread: an id, `main`, or `new` (a new conversation)                     → {client_id, thread, state, run_id?}
//! POST /v1/threads/{thread}/stop {agent?}                                       → {stopped, run_id?}
//! ```
//!
//! - nothing runs in the conversation: a turn starts (`state: started`, its `run_id` followed as before through
//!   `/v1/runs/{id}/events`);
//! - a turn goes on: words only go into it (`steered`): a turn that did nothing yet starts again with them
//!   (turn_restart.rs, `restarted: true`), else Hermes' steer at its next step; with files, with `after` (« Après »), or when
//!   the turn takes no more words, the message waits in the agent's queue (msg_queue.rs, `queued`) and starts next;
//! - the same `client_id` again (a retry after a lost answer) changes nothing (`known`).
//!
//! The bridge keeps each message in `thread_sends` until Hermes has stored its row; the timeline then gives that item
//! the `client_id` (and, before, the message itself as an item `s_<client_id>` at the end), so an app replaces its
//! own bubble by id, never by words or time. Photos come inline (`data`, base64) and go to Hermes as image parts;
//! documents come stored by `POST /v1/files` (`path`) and go as one reference line each.
//!
//! A photo is also kept as a file with the agent's uploads at once (`keep_photos`, 6/11: every photo Alex sent showed
//! « 0 Ko », nothing to draw, and an empty row in « En attente »: the bridge kept no bytes, no size, no path for it).
//! Its `s_` item, its queue item and, once Hermes stores it, the stored row (matched by `client_id`) all carry that
//! file (`name`, `mime`, `size`, `path`): every app, the ones already installed too, draws it like any file.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{text, threads_of, timeline::Item, unavailable, Param, Thread, Threads};
use crate::app::AppState;
use crate::config::AgentConfig;
use crate::instant_reply::Mode;
use crate::msg_queue::{queues_of, Sent};
use crate::store::{iso, utcnow};
use crate::web::{check_max_len, ApiResult, FieldError, HttpError, Validate};

/// An app's id for a message (a UUID).
pub static CLIENT_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{8,64}\z").expect("valid regex"));
/// Files one message carries at most.
const ATTACHMENTS_MAX: usize = 10;
/// A text one message takes at most (characters): over msg_queue.rs `TEXT_MAX`, its words go whole as a text file
/// joined to the message (`long_text_as_file`). 200 000 (Alex 24/11): two of Hermes' `read_file` reads (100 000
/// characters each), about 50 000 tokens; over it refused as a text over `TEXT_MAX` was before (the apps say it first).
pub const LONG_TEXT_MAX: usize = 200_000;
/// The agent's note on a message whose words went as a text file (never shown: a `<!--bip:` note).
const LONG_TEXT_NOTE: &str = "<!--bip:long_text The person's message was too long to go as words: it is whole in the \
                              attached text file. Read the whole file before answering.-->";
/// A message Hermes never stored (its turn failed before) stops showing after this long.
const SENDS_KEPT_SECONDS: i64 = 2 * 86_400;
/// Hermes' clock and the bridge's may differ a little (an existing Hermes on another machine).
const CLOCK_SLACK: f64 = 120.0;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Attachment {
    /// `image` (a photo, `data`) or `document` (`path`, from `POST /v1/files`).
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default, skip_serializing)]
    pub data: Option<String>,
    /// A scan's text recognised on the phone (a PDF with no text of its own): kept beside the file in a shared
    /// agent's pool (shared_files.rs), never stored with the message.
    #[serde(default, skip_serializing)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewMessage {
    #[serde(default)]
    pub agent: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// The app's notes for the agent only (its 👍 / 👎 since the last message): before the words, never shown.
    #[serde(default)]
    pub notes: Option<String>,
    /// The message's kinds (`voice`, `image`, `pdf`…), for a feature to propose (feature_tips.rs, never kept).
    #[serde(default)]
    pub kinds: Vec<String>,
    /// « Après »: waits for the turn going on to end, even with words only.
    #[serde(default)]
    pub after: bool,
    /// This turn's instructions (a read-only share's, read_shares.rs).
    #[serde(default)]
    pub instructions: Option<String>,
    /// The phone it is sent from: « ios » or « android » (apps from 7/11 on). A reminder asked in its turn goes to
    /// that phone (reminders.rs `phone_for`); none said, the bridge's own rule.
    #[serde(default)]
    pub phone: Option<String>,
    /// The phone's decision for this turn's thinking (a decider on the phone): true thinks, false answers at once;
    /// none, the bridge's rule (turn_thinking.rs). Followed only for an agent whose level the person never chose.
    #[serde(default)]
    pub thinking: Option<bool>,
    /// The app draws a line said at once under a message that starts a turn (instant_reply.rs `first`): apps from
    /// 20/11 on. An older app (it would draw it out of place, with its own first word too) gets none.
    #[serde(default)]
    pub instant_first: bool,
    /// Who writes in a family agent's common conversation (family_thread.rs): a member's message relayed by her
    /// bridge; none, the creator's own. Never from an app.
    #[serde(skip)]
    pub by: Option<crate::family_thread::Author>,
}

impl Validate for NewMessage {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "text", Some(&self.text), LONG_TEXT_MAX);
        check_max_len(errors, "notes", self.notes.as_deref(), crate::msg_queue::TEXT_MAX);
        check_max_len(errors, "instructions", self.instructions.as_deref(), crate::msg_queue::TEXT_MAX);
        if self.phone.as_deref().is_some_and(|p| crate::reminders::phone_named(p).is_none()) {
            errors.push(field("phone", "« ios » or « android »"));
        }
        if !CLIENT_ID.is_match(&self.client_id) {
            errors.push(field("client_id", "8 to 64 letters, digits, - or _"));
        }
        if self.text.trim().is_empty() && self.attachments.is_empty() {
            errors.push(field("text", "words or a file"));
        }
        if self.attachments.len() > ATTACHMENTS_MAX
            || self.attachments.iter().any(|a| match a.kind.as_str() {
                "image" => a.data.is_none(),
                "document" => a.path.as_deref().is_none_or(str::is_empty),
                _ => true,
            })
        {
            errors.push(field("attachments", "photos with data, documents with a path, ten at most"));
        }
    }
}

fn field(name: &str, msg: &str) -> FieldError {
    FieldError::new("value_error", vec![json!("body"), json!(name)], msg, Value::Null)
}

#[derive(Debug, Default, Deserialize)]
pub struct Stop {
    #[serde(default)]
    pub agent: Option<String>,
}

impl Validate for Stop {}

/// What Hermes gets: the notes, the person's words and one reference line per document; photos as image parts of one
/// user message (`/v1/runs` reads a multimodal input as a list of messages).
pub fn input_of(notes: Option<&str>, words: &str, attachments: &[Attachment]) -> Value {
    let lines: Vec<String> = attachments
        .iter()
        .filter(|a| a.kind == "document")
        .map(|a| {
            format!(
                "[Pièce jointe : {} ({}, {}) → {}]",
                a.name.as_deref().unwrap_or("document"),
                a.mime.as_deref().unwrap_or("application/octet-stream"),
                crate::storage_watch::size_fr(a.size.unwrap_or(0)),
                a.path.as_deref().unwrap_or("")
            )
        })
        .collect();
    let said: Vec<&str> = [notes.unwrap_or("").trim(), words.trim()].into_iter().filter(|p| !p.is_empty()).collect();
    let text = [said.join("\n"), lines.join("\n")].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
    let images: Vec<Value> = attachments
        .iter()
        .filter_map(|a| {
            let data = a.data.as_deref().filter(|_| a.kind == "image")?;
            let url = format!("data:{};base64,{data}", a.mime.as_deref().unwrap_or("image/jpeg"));
            Some(json!({"type": "image_url", "image_url": {"url": url}}))
        })
        .collect();
    if images.is_empty() {
        return Value::String(text);
    }
    let mut parts = if text.is_empty() { Vec::new() } else { vec![json!({"type": "text", "text": text})] };
    parts.extend(images);
    json!([{"role": "user", "content": parts}])
}

impl Threads {
    /// The thread's messages sent through the bridge: the items Hermes stored get their `client_id` (and the ones just
    /// stored are matched once, in order: the first person's message since the send holding its words), the others
    /// come as items `s_<client_id>` at the end — except those waiting in the queue (`pending.queue` shows them).
    pub async fn with_sends(&self, agent: &AgentConfig, thread: &Thread, items: &mut Vec<Item>, queued: &[String]) -> anyhow::Result<()> {
        let rows = self
            .store
            .run(
                "SELECT client_id, text, attachments, message_id, created_at FROM thread_sends \
                 WHERE agent = ? AND thread_id = ? ORDER BY created_at",
                vec![text(&agent.name), text(&thread.id)],
            )
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let s = |row: &crate::store::Row, k: &str| row.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        // a stored row → the first message it holds (Hermes joins the steers held until its next step into one row:
        // the others it holds are noted `joined` on it)
        let mut claimed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut joined: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for r in rows.iter().filter(|r| !s(r, "message_id").is_empty()) {
            match claimed.entry(s(r, "message_id")) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(s(r, "client_id"));
                }
                std::collections::hash_map::Entry::Occupied(e) => joined.entry(e.key().clone()).or_default().push(s(r, "client_id")),
            }
        }
        let since = |row: &crate::store::Row| {
            crate::store::parse_since(&s(row, "created_at")).map(|d| d.timestamp_micros() as f64 / 1e6).unwrap_or(0.0)
        };
        let now = utcnow().timestamp() as f64;
        let mut from = 0;
        let mut waiting = Vec::new();
        for row in rows.iter().filter(|r| s(r, "message_id").is_empty()) {
            let (client_id, words, at) = (s(row, "client_id"), s(row, "text"), since(row));
            let words = words.trim();
            let sought = sought(words);
            let holds = |i: &Item| squeezed(i.text()).contains(&sought);
            let found = items.iter().enumerate().skip(from).find(|(_, i)| {
                i.kind() == "message"
                    && i.body.get("role") == Some(&json!("person"))
                    && !claimed.contains_key(&i.id)
                    && (i.at == 0.0 || i.at >= at - CLOCK_SLACK) // a row without its time (an older store): order only
                    // files alone: a row with files (every person's row has the key, most an empty list: a photo
                    // sent alone took the row of the words sent just before it, 6/11)
                    && (if words.is_empty() { has_files(i) } else { holds(i) })
            });
            if let Some((n, item)) = found {
                from = n + 1;
                claimed.insert(item.id.clone(), client_id.clone());
                self.store
                    .run(
                        "UPDATE thread_sends SET message_id = ? WHERE agent = ? AND client_id = ?",
                        vec![text(&item.id), text(&agent.name), text(&client_id)],
                    )
                    .await?;
            } else if let Some(row) = items.iter().find(|i| {
                // words steered with others the turn read at once: Hermes stored them in the same row (19/11, review
                // of instant-ack H1: the second message came back « Pas envoyé », and a tap sent it twice)
                !words.is_empty()
                    && i.kind() == "message"
                    && i.body.get("role") == Some(&json!("person"))
                    && i.body.get("added_during_run") == Some(&json!(true))
                    && claimed.contains_key(&i.id)
                    && (i.at == 0.0 || i.at >= at - CLOCK_SLACK)
                    && holds(i)
            }) {
                joined.entry(row.id.clone()).or_default().push(client_id.clone());
                self.store
                    .run(
                        "UPDATE thread_sends SET message_id = ? WHERE agent = ? AND client_id = ?",
                        vec![text(&row.id), text(&agent.name), text(&client_id)],
                    )
                    .await?;
            } else if now - at < SENDS_KEPT_SECONDS as f64 && !queued.contains(&client_id) {
                waiting.push((client_id, words.to_string(), at, s(row, "attachments")));
            }
        }
        // the photos kept as files when sent (`keep_photos`): the stored row shows them by their file, in order
        let photos: std::collections::HashMap<String, Vec<Value>> = rows
            .iter()
            .filter_map(|r| {
                let kept = serde_json::from_str::<Vec<Value>>(&s(r, "attachments")).ok()?;
                let photos: Vec<Value> =
                    kept.into_iter().filter(|a| a["kind"] == "image" && a["path"].is_string()).collect();
                (!photos.is_empty()).then(|| (s(r, "client_id"), photos))
            })
            .collect();
        for item in items.iter_mut() {
            if let Some(c) = claimed.get(&item.id) {
                item.body.insert("client_id".into(), json!(c));
                if let Some(others) = joined.get(&item.id) {
                    // the other messages this row holds: their `s_` items are replaced by it too (`Page::replaced`)
                    item.body.insert("joined".into(), json!(others));
                }
                if let Some(kept) = photos.get(c) {
                    with_kept_photos(item, kept);
                }
            }
        }
        let last = items.last().map(|i| i.pos).unwrap_or((0, 0, 0));
        for (n, (client_id, words, at, attachments)) in waiting.into_iter().enumerate() {
            let id = format!("s_{client_id}");
            let files: Vec<Value> = serde_json::from_str::<Vec<Value>>(&attachments)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(k, mut a)| {
                    a["id"] = json!(format!("{id}.a{k}"));
                    a
                })
                .collect();
            let mut body = serde_json::Map::new();
            body.insert("kind".into(), json!("message"));
            body.insert("role".into(), json!("person"));
            body.insert("text".into(), json!(words));
            body.insert("client_id".into(), json!(client_id));
            body.insert("sending".into(), json!(true));
            if !files.is_empty() {
                body.insert("attachments".into(), Value::Array(files));
            }
            // after every row there is now: what Hermes stores next reads after it (`after=` brings it)
            let pos = (last.0, last.1, last.2 + 1 + n as u32);
            items.push(Item { id, pos, at, turn: String::new(), segment: String::new(), row: 0, body });
        }
        Ok(())
    }
}

/// What a stored row is looked for by: the words sent as the timeline shows a person's row (`person_text`, then
/// `visible`: the machinery's lines out, blank lines in threes or more tidied to two), each run of spaces and line breaks
/// one space (`squeezed`, the row's text too). 23/11 (Alex): a blog article pasted from a web page, blank lines in threes
/// between its blocks, never matched its row (shown tidied): the message stayed « Pas envoyé » at the end of the
/// conversation although his agent had answered it. Words that are only machinery are looked for as they are.
fn sought(words: &str) -> String {
    let shown = super::timeline::person_text(words).map(|w| super::timeline::visible(&w, "s", 0).0).unwrap_or_default();
    squeezed(if shown.trim().is_empty() { words } else { &shown })
}

/// A text with each run of spaces and line breaks as one space (trimmed).
fn squeezed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_files(item: &Item) -> bool {
    item.body.get("attachments").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
}

/// A stored row's photos (Hermes' image parts, without a name, size or file) given the files the bridge kept for them
/// when they were sent, in order; a photo the row has more of stays as it was.
fn with_kept_photos(item: &mut Item, kept: &[Value]) {
    let Some(Value::Array(atts)) = item.body.get_mut("attachments") else {
        // a row read without its photos (none counted): they are the message's
        let files: Vec<Value> = kept
            .iter()
            .enumerate()
            .map(|(n, a)| {
                let mut a = a.clone();
                a["id"] = json!(format!("{}.a{n}", item.id));
                a
            })
            .collect();
        item.body.insert("attachments".into(), Value::Array(files));
        return;
    };
    let mut kept = kept.iter();
    for (n, a) in atts.iter_mut().enumerate() {
        if a["kind"] != "image" || a["path"].is_string() {
            continue;
        }
        let Some(file) = kept.next() else { break };
        let mut file = file.clone();
        file["id"] = json!(format!("{}.a{n}", item.id));
        *a = file;
    }
}

/// A message kept until Hermes stores it (`with_sends`): the person's words, its files' names (never their bytes).
pub async fn keep(
    threads: &Threads,
    agent: &str,
    thread: &str,
    client_id: &str,
    words: &str,
    attachments: &[Attachment],
) -> anyhow::Result<()> {
    threads
        .store
        .run(
            "INSERT OR IGNORE INTO thread_sends (agent, client_id, thread_id, text, attachments, message_id, created_at) \
             VALUES (?,?,?,?,?,NULL,?)",
            vec![
                text(agent),
                text(client_id),
                text(thread),
                text(words.trim()),
                match attachments.is_empty() {
                    true => Param::Null,
                    false => text(&serde_json::to_string(attachments).unwrap_or_default()),
                },
                text(&iso(utcnow())),
            ],
        )
        .await?;
    Ok(())
}

/// Seconds a new conversation's first message waits for a gateway the bridge is restarting (the tip is made then).
const RESTART_WAIT: u64 = 90;

/// Hermes not reached, or its door answering that it is not up, while the bridge restarts its gateway.
fn down_for_restart(agent: &AgentConfig, exc: &crate::hermes::HermesError) -> bool {
    (exc.is_transport() || matches!(exc.status(), Some(502..=504))) && crate::agent_restart::restarting(agent)
}

/// A message that waits in line (« En attente », msg_queue.rs) instead of failing: its agent's gateway just
/// restarted by the bridge, or its hub full past the bridge's tries (429: hermes.rs `BUSY_ATTEMPTS`; 24/11) — it
/// starts once Hermes takes it (each run's end and every few seconds).
fn waits_in_line(agent: &AgentConfig, exc: &crate::hermes::HermesError) -> bool {
    exc.status() == Some(429) || down_for_restart(agent, exc)
}

/// The conversation's tip, made when it has none yet (its first message): a Hermes session titled from the words.
async fn tip_of(state: &AppState, threads: &Threads, agent: &AgentConfig, thread: &Thread, words: &str) -> ApiResult<String> {
    if let Some(tip) = threads.line(agent, thread).await.map_err(unavailable)?.last() {
        // its shared memory off since (the person, an outside model): a new session first, unless a turn goes on there
        // (the words join it, or wait in the queue, which moves them; agent_shared_memory.rs `fresh_tip`, review 2 B2-a)
        if state.hub.session_run(&agent.name, &tip.session_id).is_none() {
            if let Some(fresh) =
                crate::agent_shared_memory::fresh_tip(&state.config(), &state.hermes, threads, agent, thread).await
            {
                return Ok(fresh);
            }
        }
        return Ok(tip.session_id.clone());
    }
    let first: String = words.chars().take(60).collect();
    // Hermes refuses a title another conversation holds (400 `invalid_title`): a new conversation opened with the same
    // words as an earlier one (« Bonjour », the console's « ＋ », a relative's) failed with « isn't answering » and left
    // an empty thread (release bench of 4/11). Numbered then, as the background turns' work conversations are.
    let (mut attempt, mut waited) = (1, 0);
    let session = loop {
        let title = if attempt == 1 { first.clone() } else { format!("{first} ({attempt})") };
        match state.hermes.create_session(agent, &title).await {
            Ok(session) => break session,
            Err(crate::hermes::HermesError::Http { status: 400 | 409, .. }) if attempt < 9 => attempt += 1,
            // its gateway restarting (the bridge's own restart): the conversation made once Hermes answers again
            Err(exc) if down_for_restart(agent, &exc) && waited < RESTART_WAIT => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                waited += 2;
            }
            Err(_) => return Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment")),
        }
    };
    threads.link(agent, thread, &session).await?;
    Ok(session)
}

/// A message sent again (its answer lost on the way, or the person's « Réessayer »): `known`, and where it is now, so
/// the phone does what the first answer would have had it do. Waiting in the queue: `queued` (« En attente » shows
/// it); its conversation's turn going on: that turn's `run_id` (followed again); else neither (its turn is over: the
/// timeline has it). Nothing is sent twice. An app from before reads `known` alone (thin apps, 5/11).
/// None: the message was lost (`lost`): sent again for real, under the same id.
async fn known_now(state: &AppState, threads: &Threads, agent: &AgentConfig, client_id: &str, thread: &str) -> Option<Value> {
    let queued = queues_of(state).get(&agent.name).items.iter().any(|i| i.client_id.as_deref() == Some(client_id));
    let found = threads.get(thread).await.ok().flatten();
    let segments = match &found {
        Some(t) => threads.line(agent, t).await.ok().unwrap_or_default(),
        None => Vec::new(),
    };
    let tip = segments.last().map(|s| s.session_id.clone());
    let run = tip.as_deref().filter(|_| !queued).and_then(|t| state.hub.session_run(&agent.name, t));
    if let (Some(t), false, None) = (&found, queued, &run) {
        // nothing runs it and it waits nowhere: stored since (matched by this read), or lost
        let items = threads.items(agent, t, &segments).await.ok().unwrap_or_default();
        if items.iter().any(|i| i.id == format!("s_{client_id}") && lost(i)) {
            return None;
        }
    }
    Some(json!({"client_id": client_id, "thread": thread, "state": "known", "run_id": run, "tip": tip, "queued": queued}))
}

/// A sent message no turn took and none will: Hermes never stored it, it waits in no queue, no turn runs in its
/// conversation, a while after it was sent (5/11, Alex: « Hello? » steered into a turn stuck on a card, then Stop —
/// the turn never took it; its `s_` item stayed « being sent » at the end of the conversation for two days). Shown
/// `not_sent` (« Pas envoyé · Toucher pour réessayer »), and the same id sent again goes for real.
pub fn lost(item: &Item) -> bool {
    item.body.contains_key("sending") && utcnow().timestamp() as f64 - item.at > LOST_AFTER
}

/// How long a sent message may wait unstored, nothing running, before it counts as lost (its run is registered by the
/// same request that sends it: a few seconds cover the clocks).
const LOST_AFTER: f64 = 10.0;

/// Words steered into a turn that ended before `LOST_AFTER` without taking them (Stop at once) become « Pas envoyé »
/// when that time passes, with nothing else happening: the phones showing the conversation are told then (a `thread`
/// hint, `lost`), and read it again (5/11-6/11, Android: « Pas envoyé » never showed while the conversation stayed
/// open; the bridge only knew it when a page was read). A turn that ends later says it with its own hint (`run`).
fn tell_if_lost(state: &AppState, agent: &AgentConfig, thread: &Thread, tip: &str, client_id: &str) {
    let (state, agent, thread, tip, client_id) =
        (state.clone(), agent.clone(), thread.clone(), tip.to_string(), client_id.to_string());
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs_f64(LOST_AFTER + 1.0)).await;
        if state.hub.session_run(&agent.name, &tip).is_some() {
            return; // still running: its end tells
        }
        let waiting = state
            .store
            .run(
                "SELECT 1 FROM thread_sends WHERE agent = ? AND client_id = ? AND message_id IS NULL",
                vec![text(&agent.name), text(&client_id)],
            )
            .await
            .unwrap_or_default();
        if !waiting.is_empty() {
            let _ = threads_of(&state).bump(&thread, "lost").await;
        }
    });
}

impl Attachment {
    /// What the apps are shown of it (never its bytes).
    pub fn shown(&self) -> Value {
        json!({"kind": self.kind, "name": self.name, "mime": self.mime, "size": self.size, "path": self.path})
    }
}

/// A message's photos as files with the agent's uploads (where `POST /v1/files` puts documents), their bytes still
/// going to Hermes inline: each gets its real `size` and, stored, its `path` and stored `name`. Undecodable data, or an
/// agent without uploads: the size alone (the photo still goes to Hermes).
pub async fn keep_photos(state: &AppState, agent: &AgentConfig, attachments: Vec<Attachment>) -> Vec<Attachment> {
    if !attachments.iter().any(|a| a.kind == "image" && a.data.is_some() && a.path.is_none()) {
        return attachments;
    }
    let config = state.config();
    let agent = agent.clone();
    let given = attachments.clone();
    let kept = tokio::task::spawn_blocking(move || {
        let mut stored = false;
        let out: Vec<Attachment> = attachments
            .into_iter()
            .map(|mut a| {
                if a.kind != "image" || a.path.is_some() {
                    return a;
                }
                let Some(bytes) = a.data.as_deref().and_then(photo_bytes) else { return a };
                a.size = Some(bytes.len() as u64);
                let name = a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "photo.jpg".into());
                let mut open = || Ok(std::io::Cursor::new(bytes.clone()));
                let limits = &config.limits;
                match crate::files::store_upload_from(&agent, &mut open, &name, a.mime.as_deref(), limits, None) {
                    Ok(file) => {
                        a.path = file["path"].as_str().map(str::to_string);
                        // erased after 30 days unless the Carnet holds it (files_kept.rs `purge_photos`)
                        if let Some(path) = &a.path {
                            if let Err(err) = crate::files_kept::note_photo(&config, &agent.name, path, crate::py::time_time()) {
                                tracing::warn!(agent = %agent.name, error = %err, "a sent photo not noted for its 30 days");
                            }
                        }
                        a.name = file["filename"].as_str().map(str::to_string).or(a.name);
                        stored = true;
                    }
                    Err(exc) => {
                        tracing::warn!(agent = %agent.name, status = %exc.status, "a sent photo not kept as a file")
                    }
                }
                a
            })
            .collect();
        if stored {
            // an isolated sandbox sees a copy of the uploads (sandbox.rs)
            if let Err(exc) = crate::sandbox::sync_agent(&config, &agent.name) {
                tracing::warn!(agent = %agent.name, error = %exc, "uploads not copied to the sandbox");
            }
        }
        out
    });
    kept.await.unwrap_or(given)
}

/// A photo's bytes from its base64 (a `data:` URL too).
pub fn photo_bytes(data: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let b64 = data.strip_prefix("data:").and_then(|d| d.split_once(',')).map(|(_, b)| b).unwrap_or(data);
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(clean.as_bytes()).ok().filter(|b| !b.is_empty())
}

/// A text over msg_queue.rs `TEXT_MAX` characters as a file (23/11, Alex: a blog article pasted): stored whole with the
/// agent's uploads (where `POST /v1/files` puts documents), named after its first line, to join the message in its
/// words' stead (its reference line goes to Hermes, every app draws the file, installed ones too). Measured on Hermes
/// v0.23.1: a 150 000-character message would go whole into the model's context, every turn until a compaction; read
/// from a file, the agent takes what it needs. None when the agent has no uploads (the text is then refused).
async fn long_text_as_file(state: &AppState, agent: &AgentConfig, text: &str) -> Option<Attachment> {
    let (config, agent, bytes, name) = (state.config(), agent.clone(), text.as_bytes().to_vec(), long_text_name(text));
    let stored = tokio::task::spawn_blocking(move || {
        let mut open = || Ok(std::io::Cursor::new(bytes.clone()));
        let file = crate::files::store_upload_from(&agent, &mut open, &name, Some("text/plain"), &config.limits, None);
        let file = match file {
            Ok(file) => file,
            Err(exc) => {
                tracing::warn!(agent = %agent.name, status = %exc.status, "a long text not kept as a file");
                return None;
            }
        };
        // an isolated sandbox sees a copy of the uploads (sandbox.rs)
        if let Err(exc) = crate::sandbox::sync_agent(&config, &agent.name) {
            tracing::warn!(agent = %agent.name, error = %exc, "uploads not copied to the sandbox");
        }
        Some(Attachment {
            kind: "document".into(),
            // shown with its spaces (Alex 24/11, lt-nom-du-fichier A); stored under its safe name (`path`)
            name: Some(name),
            mime: Some("text/plain".into()),
            size: file["size"].as_u64(),
            path: file["path"].as_str().map(str::to_string),
            data: None,
            text: None,
        })
    });
    stored.await.ok().flatten()
}

/// A long text's file name as shown (Alex 24/11, lt-nom-du-fichier A: with its spaces, « Pourquoi le vélo électrique a
/// gagné la ville.txt »): its first line with words (a title's `#` out), what no file name may hold out (`/`, `\`, `:`,
/// control and invisible characters: a space each, runs of spaces one), no leading dot, 60 characters at most, `.txt`.
/// Stored on the disk under its safe form (files.rs `sanitize_filename`: `<month>/<id12>-<name>`).
fn long_text_name(text: &str) -> String {
    let first =
        text.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| l.chars().any(char::is_alphanumeric));
    let cleaned: String =
        first
            .unwrap_or("message")
            .chars()
            .map(|c| {
                if matches!(c, '/' | '\\' | ':') || c.is_control() || crate::files::is_control_like(c) {
                    ' '
                } else {
                    c
                }
            })
            .collect();
    let spaced = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name: String = spaced.trim_start_matches('.').trim().chars().take(60).collect();
    name = name.trim_end_matches(|c: char| !c.is_alphanumeric()).to_string();
    if name.is_empty() {
        name = "message".into();
    }
    format!("{name}.txt")
}

/// What the line said at once restates (instant_reply.rs): the words, else what the message carries.
fn said_for_ack(words: &str, attachments: &[Attachment]) -> String {
    match words.trim().is_empty() {
        true => format!("({})", crate::msg_queue::files_said(&attachments.iter().map(Attachment::shown).collect::<Vec<_>>())),
        false => words.to_string(),
    }
}

/// `POST /v1/threads/{thread}/messages`.
pub async fn send(state: &AppState, agent: &AgentConfig, thread: Option<Thread>, body: NewMessage) -> ApiResult<Value> {
    let threads = threads_of(state);
    // the same message again (an answer lost on the way): nothing sent twice, no conversation made twice
    let known = state
        .store
        .run(
            "SELECT thread_id FROM thread_sends WHERE agent = ? AND client_id = ?",
            vec![text(&agent.name), text(&body.client_id)],
        )
        .await?;
    if let Some(row) = known.first() {
        let thread_id = row["thread_id"].as_str().unwrap_or("").to_string();
        match known_now(state, &threads, agent, &body.client_id, &thread_id).await {
            Some(answer) => return Ok(answer),
            None => {
                // lost: sent again for real, in its own conversation, under the same id
                for table in ["thread_sends", "thread_instant"] {
                    state
                        .store
                        .run(
                            &format!("DELETE FROM {table} WHERE agent = ? AND client_id = ?"),
                            vec![text(&agent.name), text(&body.client_id)],
                        )
                        .await?;
                }
                tracing::info!(agent = %agent.name, thread = %thread_id, "a lost message sent again");
            }
        }
    }
    let mut body = body;
    let mut long_text = None;
    if body.text.trim().chars().count() > crate::msg_queue::TEXT_MAX {
        // too long for one message's words (an article pasted, 23/11): whole as a text file, the agent told to read it
        let Some(file) = long_text_as_file(state, agent, body.text.trim()).await else {
            return Err(HttpError::new(
                422,
                format!(
                    "a message's words are {} characters at most: send a longer text as a file",
                    crate::msg_queue::TEXT_MAX
                ),
            ));
        };
        long_text = file.path.clone();
        body.attachments.push(file);
        body.text = String::new();
        body.notes = Some(match body.notes.take().filter(|n| !n.trim().is_empty()) {
            Some(notes) => format!("{LONG_TEXT_NOTE}\n{notes}"),
            None => LONG_TEXT_NOTE.to_string(),
        });
    }
    let thread = match thread {
        Some(t) => t,
        None => threads.create(&agent.name, "side", None, Value::Null, false).await?,
    };
    if let Some(path) = &long_text {
        // its file lives with this conversation (long_texts.rs, Alex 24/11)
        super::long_texts::keep(&state.store, agent, &thread.id, path).await?;
    }
    let words = body.text.trim().to_string();
    let phone = body.phone.as_deref().and_then(crate::reminders::phone_named);
    // a family agent's one conversation (family_thread.rs): each message framed with who writes, the creator's too
    let config = state.config();
    let creator = state.households.name();
    let author = crate::family_thread::is_common(&config, &agent.name)
        .then(|| body.by.clone().unwrap_or_else(|| crate::family_thread::Author::creator(&creator)));
    let said = match &author {
        Some(a) => crate::family_thread::framed(a, &words),
        None => words.clone(),
    };
    let client_id = body.client_id.clone();
    // the message taken (a turn started, queued, or steered): the others told (family_thread.rs)
    let told = |tip: &str| {
        if let Some(a) = &author {
            crate::family_thread::said(state, agent, a, &words, &client_id, tip);
        }
    };
    body.attachments = keep_photos(state, agent, std::mem::take(&mut body.attachments)).await;
    if !body.attachments.is_empty() {
        // an agent others use too (shared with relatives, the family's): its files also go into its pool, everyone's
        let (config, shared, files, person) = (state.config(), agent.clone(), body.attachments.clone(), state.households.name());
        let pooled = move || crate::shared_files::share_own_files(&config, &shared, &files, &person);
        let _ = tokio::task::spawn_blocking(pooled).await;
    }
    // an outside model the bridge has not seen yet (SSH, a restart, the update): protected before this turn (B2-a)
    crate::agent_shared_memory::guard_on_send(state.config(), agent).await;
    let tip = tip_of(state, &threads, agent, &thread, &words).await?;
    let tip = tip.as_str();
    let answer = |state: &str, run: Option<&str>| json!({"client_id": body.client_id, "thread": thread.id, "state": state, "run_id": run, "tip": tip});
    keep(&threads, &agent.name, &thread.id, &body.client_id, &said, &body.attachments).await?;
    let queues = queues_of(state);
    let running = state.hub.session_run(&agent.name, tip);
    let waits = queues.get(&agent.name).items.iter().any(|i| i.session_id == tip);
    // in the family conversation, words steer only their writer's own turn: else they wait behind it; the others'
    // messages waiting there don't hold hers back (only her own do: they keep her order)
    let own_turn = |run: &str| {
        author.as_ref().is_none_or(|a| crate::family_thread::author_of(&config, &agent.name, run, &creator).household == a.household)
    };
    let held = match &author {
        Some(a) => queues.get(&agent.name).items.iter().any(|i| {
            i.session_id == tip
                && i.by.as_ref().and_then(crate::family_thread::Author::of_value).is_none_or(|b| b.household == a.household)
        }),
        None => waits,
    };
    // what waited for this message (a Live call's recap, the time, the family's news, a feature to propose…)
    let mut waited = None;
    // this message gets the agent's line said at once (instant_reply.rs): reserved once, kept if the steer fails; one
    // per conversation every 20 s (in the family's, per writer: Paul's line never takes Marie's)
    let mut instant = false;
    let cap_key = match &author {
        Some(a) => format!("{}#{}", thread.id, a.household),
        None => thread.id.clone(),
    };
    // a message that starts a turn and calls for it (instant_reply::wants_line): its line asked at once, while the rest
    // of the send goes on; the turn starts at once, the line shown only if it comes before the turn's first words; off
    // for an agent whose lines keep coming too late (instant_reply.rs `Adapt`). Its own slot (`#first`): words steered
    // into that turn a moment later still get their line
    let first_key = format!("{cap_key}#first");
    let first_mode = (body.instant_first
        && running.is_none()
        && !waits
        && !body.after
        && author.is_none()
        && body.instructions.as_deref().is_none_or(|i| i.trim().is_empty())
        && crate::instant_reply::wants_line(&words, !body.attachments.is_empty(), crate::instant_reply::slow_here(&agent.name, &thread.id)))
    .then(|| crate::instant_reply::first_mode(&config, agent, crate::py::time_time()))
    .filter(|m| *m != crate::instant_reply::FirstMode::Off)
    .filter(|_| crate::instant_reply::reserve(&agent.name, &first_key, crate::py::time_time()));
    let first_line = first_mode
        .map(|_| crate::instant_reply::first(state, agent, &thread, &body.client_id, &said_for_ack(&words, &body.attachments)));
    // a turn about to start again with earlier words (runs.rs `restart` starting it): Hermes ended it, it takes no
    // words; these wait first in line, for the turn after
    let restarting = |r: &str| state.hub.subscription(&agent.name, r).is_some_and(|sub| sub.restarting());
    if let Some(run) = running
        .as_deref()
        .filter(|r| !body.after && !held && body.attachments.is_empty() && own_turn(r) && !restarting(r))
    {
        if author.is_none() && state.hub.subscription(&agent.name, run).is_some_and(|sub| !sub.acted()) {
            // the turn did nothing yet (it thinks, writes, reads): stopped and started again with these words too, for
            // one answer to both (turn_restart.rs); Hermes' steer would only reach it after a tool call
            waited = Some(crate::live_brief::message_notes(state, agent, tip, &words, &body.kinds).await);
            let notes = [waited.clone().flatten(), body.notes.clone()].into_iter().flatten().collect::<Vec<_>>();
            // the restarted turn thinks when the cut one did
            let thinking = if crate::turn_thinking::is_thinking_run(run) { Some(true) } else { body.thinking };
            let plan = crate::turn_restart::Plan {
                said: said.clone(),
                notes: notes.join("\n"),
                client_id: Some(body.client_id.clone()),
                phone: phone.map(str::to_string),
                instructions: body.instructions.clone().filter(|i| !i.trim().is_empty()),
                options: crate::turn_thinking::turn_options(&state.config(), agent, &words, thinking),
            };
            if state.hub.interrupt(&agent.name, run, plan) {
                match state.hermes.stop_run(agent, run).await {
                    // stopping, or over already: its end starts the words (runs.rs `restart`)
                    Ok(()) => {}
                    Err(exc) if matches!(exc.status(), Some(404 | 409)) => {}
                    Err(exc) => {
                        tracing::info!(agent = %agent.name, run_id = %run, error = %exc, "restart: stop refused, words steered");
                        state.hub.withdraw_restart(&agent.name, run);
                    }
                }
                if state.hub.subscription(&agent.name, run).is_some_and(|sub| sub.restarting()) {
                    threads.bump(&thread, "sent").await?;
                    tell_if_lost(state, agent, &thread, tip, &body.client_id);
                    let mut answer = answer("steered", Some(run));
                    answer["restarted"] = json!(true);
                    return Ok(answer);
                }
            }
        }
        // words into the turn going on (Hermes takes them at its next step); answered at once by a short line
        // (instant_reply.rs), the turn told so with them, so it does not acknowledge them again
        instant = instant || crate::instant_reply::reserve(&agent.name, &cap_key, crate::py::time_time());
        let steer = if instant { crate::instant_reply::with_note(&said, Mode::Steered) } else { said.clone() };
        let steered = match queues.steer(agent, tip, Some(run), &steer, Some(&body.client_id)).await {
            Ok(steered) => steered,
            Err(_) => {
                // nothing said: the conversation's slot for a line stays free
                if instant {
                    crate::instant_reply::release(&agent.name, &cap_key);
                }
                return Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment"));
            }
        };
        if let Some(run) = steered {
            // the latest words decide where a reminder asked in this turn goes
            state.hub.set_phone(&agent.name, &run, phone);
            threads.bump(&thread, "sent").await?;
            tell_if_lost(state, agent, &thread, tip, &body.client_id);
            told(tip);
            if instant {
                crate::instant_reply::answer(state, agent, &thread, &body.client_id, &said_for_ack(&words, &body.attachments), Mode::Steered, author.is_some());
            }
            crate::instant_reply::watch_taken(state, agent, &thread, &run, &body.client_id);
            return Ok(answer("steered", Some(&run)));
        }
    }
    let waited = match waited {
        Some(waited) => waited,
        None => crate::live_brief::message_notes(state, agent, tip, &words, &body.kinds).await,
    };
    let notes = [waited, body.notes.clone()].into_iter().flatten().collect::<Vec<_>>().join("\n");
    // a message that waits behind a turn going on: answered at once too (« right after »), its turn told so
    let instant = running.is_some()
        && (instant || crate::instant_reply::reserve(&agent.name, &cap_key, crate::py::time_time()));
    let input = match instant {
        true => input_of(Some(&notes), &crate::instant_reply::with_note(&said, Mode::Queued), &body.attachments),
        false => input_of(Some(&notes), &said, &body.attachments),
    };
    let by = crate::family_thread::by_value(author.as_ref());
    if running.is_some() || waits || body.after {
        // first in line when the turn just stopped taking words, else after what waits (someone else's turn too)
        let front = running.is_some() && !body.after && !waits;
        let sent = Sent {
            client_id: Some(body.client_id.clone()),
            input: Some(input),
            attachments: body.attachments.iter().map(Attachment::shown).collect(),
            voice: body.kinds.iter().any(|k| k == "voice"),
            phone: phone.map(str::to_string),
            thinking: body.thinking,
            by: by.clone(),
        };
        let shown = sent.attachments.clone();
        queues
            .add(agent, tip, &words, running.as_deref(), front, sent)
            .map_err(|_| HttpError::new(409, "too many messages waiting"))?;
        if let (Some(run), false, false) = (running.as_deref(), body.after, shown.is_empty()) {
            // files Hermes' steer cannot carry: the turn going on knows they wait for it (10/11: a background task
            // started without Alex's photo, and its answer came after the right one)
            let note = crate::msg_queue::files_steer(&shown, false);
            if let Err(exc) = state.hermes.steer_run(agent, run, &note).await {
                tracing::info!(agent = %agent.name, run_id = %run, error = %exc, "the turn not told of the files waiting");
            }
        }
        threads.bump(&thread, "sent").await?;
        told(tip);
        if instant {
            crate::instant_reply::answer(state, agent, &thread, &body.client_id, &said_for_ack(&words, &body.attachments), Mode::Queued, author.is_some());
        }
        return Ok(answer("queued", None));
    }
    crate::cloud_setup::READINESS.wait(&agent.name, crate::cloud_setup::READY_WAIT).await;
    crate::mcp_sessions::SESSIONS.first_turn(&agent.name, crate::mcp_sessions::FIRST_TURN_WAIT).await;
    let instructions = body.instructions.as_deref().filter(|i| !i.trim().is_empty());
    let options = crate::turn_thinking::turn_options(&state.config(), agent, &words, body.thinking);
    let thinks = crate::turn_thinking::thought(options.as_ref());
    // parallel: the turn starts now with a standing note, the line comes beside it (`line_beside`); no wait here (a wait
    // let a second message start a turn of its own first: Relecture 1, B1)
    let line_after = first_line;
    if first_mode.is_some() && line_after.is_none() {
        crate::instant_reply::release(&agent.name, &first_key);
    }
    let start_input = match &line_after {
        Some(_) => input_of(Some(&notes), &crate::instant_reply::with_standing_note(&said), &body.attachments),
        None => input.clone(),
    };
    let run = match state.hermes.start_run_input(agent, start_input, tip, instructions, options.clone()).await {
        Ok(run) => run,
        Err(exc) => match exc.status() {
            Some(status @ (400 | 413 | 415 | 422)) => {
                return Err(HttpError::new(status, "your agent refused this message"))
            }
            _ if waits_in_line(agent, &exc) => {
                // its gateway restarted by the bridge a moment ago (agent_restart.rs), or its hub full (429): the
                // message waits in line and starts when Hermes takes it (« En attente », msg_queue.rs), it never fails
                // for that
                let sent = Sent {
                    client_id: Some(body.client_id.clone()),
                    input: Some(input),
                    attachments: body.attachments.iter().map(Attachment::shown).collect(),
                    voice: body.kinds.iter().any(|k| k == "voice"),
                    phone: phone.map(str::to_string),
                    thinking: body.thinking,
                    by,
                };
                queues
                    .add(agent, tip, &words, None, false, sent)
                    .map_err(|_| HttpError::new(409, "too many messages waiting"))?;
                tracing::info!(agent = %agent.name, status = ?exc.status(), "message sent while its Hermes can't take it: it waits in line");
                threads.bump(&thread, "sent").await?;
                told(tip);
                return Ok(answer("queued", None));
            }
            _ => return Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment")),
        },
    };
    if thinks {
        crate::turn_thinking::thinking_run(&run);
    }
    crate::welcome::done(&state.config(), &agent.name, Some(tip));
    if let Some(a) = &author {
        // whose turn it is, noted before the hub reads it (a member's cards are hers, family_thread.rs)
        crate::family_thread::started(state, agent, &run, a, tip, false);
    }
    // followed by the hub whoever looks (the brake, « reply ready »), and the queue waits for it
    state.hub.watch(agent, &run, Some(tip), true);
    state.hub.keep(agent, &run, Some(tip));
    state.hub.set_phone(&agent.name, &run, phone);
    // its continuation, if it ends on a promise, keeps them (runs.rs `go_on`)
    state.hub.set_turn(&agent.name, &run, instructions, options);
    tracing::info!(agent = %agent.name, run_id = %run, "message sent: a turn started");
    threads.bump(&thread, "sent").await?;
    if body.instant_first && author.is_none() {
        // how fast this agent's turns here say their first words (rule 5 of instant_reply::wants_line)
        crate::instant_reply::watch_first_words(state, &agent.name, &thread.id, &run);
    }
    if let Some(asked) = line_after {
        line_beside(state, agent, &thread, &run, &body.client_id, &first_key, asked);
    }
    told(tip);
    Ok(answer("started", Some(&run)))
}

/// The parallel mode's line, once it comes: shown only while the turn has shown no words yet (and is not over), its
/// words then steered into the turn's next step (a new message after the tool results: the turn's prompt cache stays
/// whole); else dropped. Its fate tells `Adapt` whether lines come in time beside this agent's turns.
fn line_beside(state: &AppState, agent: &AgentConfig, thread: &Thread, run: &str, client_id: &str, cap_key: &str,
               asked: tokio::task::JoinHandle<Option<crate::instant_reply::FirstLine>>) {
    let (state, agent, thread, run, client_id, cap_key) =
        (state.clone(), agent.clone(), thread.clone(), run.to_string(), client_id.to_string(), cap_key.to_string());
    tokio::spawn(async move {
        let got = asked.await.ok().flatten();
        let config = state.config();
        // (a turn about to restart with a second message: the new run never gets the line, so none: Relecture 2)
        let silent = state.hub.subscription(&agent.name, &run)
            .is_some_and(|sub| !sub.finished() && sub.terminal().is_none() && !sub.restarting())
            && state.hub.glance(&agent.name, &run).is_some_and(|(_, words)| words.trim().is_empty());
        let mode = crate::instant_reply::FirstMode::Parallel;
        crate::instant_reply::first_outcome(&config, &agent, mode, got.is_some() && silent, crate::py::time_time());
        let Some(said) = got.and_then(|l| l.said).filter(|_| silent) else {
            crate::instant_reply::release(&agent.name, &cap_key);
            tracing::info!(agent = %agent.name, run_id = %run, "instant reply (first message): none or too late, not shown");
            return;
        };
        let threads = threads_of(&state);
        if let Err(exc) = threads.keep_first(&agent.name, &thread.id, &client_id, &said).await {
            tracing::warn!(agent = %agent.name, error = %format!("{exc:#}"), "instant reply (first message) not kept");
            return;
        }
        let _ = threads.bump(&thread, "instant").await;
        if let Err(exc) = state.hermes.steer_run(&agent, &run, &crate::instant_reply::first_note(&said)).await {
            tracing::info!(agent = %agent.name, run_id = %run, error = %exc, "the turn not told of its line");
        }
        tracing::info!(agent = %agent.name, run_id = %run, "instant reply (first message) said beside the turn");
    });
}

/// `POST /v1/threads/{thread}/stop`: the turn going on in the conversation stopped (Hermes then ends it cancelled).
pub async fn stop(state: &AppState, agent: &AgentConfig, thread: &Thread) -> ApiResult<Value> {
    // the person stopped it: their « Oui » for sending forms ends with it (form_send.rs, review of 18/11, F3)
    crate::form_send::end_windows(&state.config(), &agent.name);
    let threads = threads_of(state);
    let tip = threads.line(agent, thread).await.map_err(unavailable)?.last().map(|s| s.session_id.clone());
    let Some(run) = tip.as_deref().and_then(|t| state.hub.session_run(&agent.name, t)) else {
        return Ok(json!({"stopped": false}));
    };
    // stopped for new words a moment ago (turn_restart.rs): the person's stop wins, nothing starts again
    state.hub.withdraw_restart(&agent.name, &run);
    // noted first: a turn that just ended on a promise is not continued either (runs.rs `go_on`)
    state.hub.ask_stop(&agent.name, &run);
    match state.hermes.stop_run(agent, &run).await {
        Ok(()) => Ok(json!({"stopped": true, "run_id": run})),
        Err(exc) if matches!(exc.status(), Some(404 | 409)) => Ok(json!({"stopped": false, "run_id": run})),
        Err(_) => Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo() -> Attachment {
        Attachment { kind: "image".into(), name: None, mime: Some("image/png".into()), size: None, path: None, data: Some("AAAA".into()), text: None }
    }

    #[test]
    fn words_go_as_a_string_photos_as_one_user_message() {
        assert_eq!(input_of(None, " Bonjour ", &[]), json!("Bonjour"));
        assert_eq!(input_of(Some("<!--bip:now 9 h-->"), "Bonjour", &[]), json!("<!--bip:now 9 h-->\nBonjour"));
        let input = input_of(None, "Et ça ?", &[photo()]);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0], json!({"type": "text", "text": "Et ça ?"}));
        assert_eq!(input[0]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        // a photo alone: no empty text part
        assert_eq!(input_of(None, "", &[photo()])[0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_document_goes_as_its_reference_line() {
        let doc = Attachment { kind: "document".into(), name: Some("devis.pdf".into()), mime: Some("application/pdf".into()),
                               size: Some(12_000), path: Some("/uploads/2026-11/abc-devis.pdf".into()), data: None, text: None };
        let input = input_of(None, "Regarde", &[doc]);
        assert_eq!(input, json!("Regarde\n\n[Pièce jointe : devis.pdf (application/pdf, 12 Ko) → /uploads/2026-11/abc-devis.pdf]"));
        // the timeline reads it back as the message's attachment, the words alone shown
        let (words, files) = super::super::timeline::visible(input.as_str().unwrap(), "m_x", 0);
        assert_eq!(words.trim(), "Regarde");
        assert_eq!(files[0]["name"], "devis.pdf");
    }

    #[test]
    fn a_photo_reads_from_its_base64_or_its_data_url() {
        assert_eq!(photo_bytes("AAEC").unwrap(), vec![0, 1, 2]);
        assert_eq!(photo_bytes("data:image/png;base64,AAEC\n").unwrap(), vec![0, 1, 2]);
        assert!(photo_bytes("").is_none() && photo_bytes("%%").is_none());
    }

    #[test]
    fn a_stored_row_shows_the_photos_kept_when_sent() {
        // 6/11: Hermes' row of a photo has no name, size or file: the one the bridge kept when it was sent, in order
        let kept = vec![json!({"kind": "image", "name": "IMG_1.jpg", "mime": "image/jpeg", "size": 2048, "path": "/u/2026-11/a-IMG_1.jpg"})];
        let mut body = serde_json::Map::new();
        body.insert("attachments".into(), json!([
            {"id": "m_1.a0", "kind": "document", "name": "devis.pdf", "path": "/u/devis.pdf"},
            {"id": "m_1.a1", "kind": "image", "name": null, "mime": "image/jpeg", "size": null, "path": null},
            {"id": "m_1.a2", "kind": "image", "name": null, "mime": "image/jpeg", "size": null, "path": null},
        ]));
        let mut item = Item { id: "m_1".into(), pos: (0, 0, 0), at: 0.0, turn: String::new(), segment: String::new(), row: 0, body };
        with_kept_photos(&mut item, &kept);
        let atts = item.body["attachments"].as_array().unwrap();
        assert_eq!(atts[0]["name"], "devis.pdf");
        assert_eq!(atts[1], json!({"id": "m_1.a1", "kind": "image", "name": "IMG_1.jpg", "mime": "image/jpeg", "size": 2048,
                                   "path": "/u/2026-11/a-IMG_1.jpg"}));
        assert_eq!(atts[2]["path"], Value::Null);
        // a row read without its photos: the kept ones
        let mut bare = Item { id: "m_2".into(), pos: (0, 0, 0), at: 0.0, turn: String::new(), segment: String::new(), row: 0,
                              body: serde_json::Map::new() };
        with_kept_photos(&mut bare, &kept);
        assert_eq!(bare.body["attachments"][0]["id"], "m_2.a0");
        assert_eq!(bare.body["attachments"][0]["size"], 2048);
    }

    #[test]
    fn a_message_needs_words_or_a_file_and_a_client_id() {
        let mut errors = Vec::new();
        let body: NewMessage = serde_json::from_value(json!({"client_id": "x", "text": " "})).unwrap();
        body.validate(&mut errors);
        assert_eq!(errors.len(), 2);
        let mut errors = Vec::new();
        let body: NewMessage =
            serde_json::from_value(json!({"client_id": "0F0E-1A2B-3C4D", "attachments": [{"kind": "image", "data": "AA"}]})).unwrap();
        body.validate(&mut errors);
        assert!(errors.is_empty());
    }

    /// 23/11 (Alex): an article pasted from a web page (blank lines in threes) stayed « Pas envoyé » after its answer:
    /// its stored row, as the timeline shows it (blank lines tidied), never held the words as sent.
    #[test]
    fn a_stored_row_holds_the_words_sent_as_the_timeline_shows_them() {
        let article = "Pourquoi le vélo a gagné la ville\n\n\n\nLes pistes ont doublé.   \n\n\n- point 1 : voir \
                       https://exemple.org/a?b=c\n- point 2 : ça roule 🚲\r\n\n\nFin.";
        let stored = format!("<!--bip:now lundi 23 novembre, 19:00-->\n{article}");
        let words = super::super::timeline::person_text(&stored).unwrap();
        let (shown, _) = super::super::timeline::visible(&words, "m_1", 0);
        assert!(!shown.contains(article), "the row is shown tidied: {shown:?}");
        assert!(squeezed(&shown).contains(&sought(article)));
        // in a row holding other words too (Hermes joins steers held until its next step)
        assert!(squeezed(&format!("Et aussi ceci\n\n{shown}")).contains(&sought(article)));
        // never another message
        assert!(!squeezed("Pourquoi le vélo a gagné la ville").contains(&sought(article)));
        // a line the timeline takes out of a row ([image], a tool's markup) is looked for without it
        let marked = "Regarde ça\n[image]\nC'est beau";
        let (shown, _) =
            super::super::timeline::visible(&super::super::timeline::person_text(marked).unwrap(), "m_2", 0);
        assert!(squeezed(&shown).contains(&sought(marked)), "{shown:?}");
        // words that are only machinery: looked for as they are
        assert_eq!(sought("[image]"), "[image]");
    }

    #[test]
    fn a_long_text_is_named_after_its_first_line() {
        assert_eq!(
            long_text_name("\n\n# Pourquoi le vélo électrique a gagné la ville !\n\nTexte"),
            "Pourquoi le vélo électrique a gagné la ville.txt"
        );
        assert_eq!(long_text_name(&"mot ".repeat(40)).chars().count(), 59 + 4);
        assert_eq!(long_text_name("---\n\n  "), "message.txt");
    }

    /// Point 6: lt-nom-du-fichier A (Alex 24/11): shown with its spaces, never « _ »; what no file name may hold out (`/`,
    /// `\`, `:`, control and invisible characters), no leading dot, 60 characters at most. The attachment carries this
    /// name; the disk keeps its safe form (`<month>/<id12>-<name>`).
    #[test]
    fn a6_a_long_texts_name_keeps_its_spaces_and_stays_safe() {
        assert_eq!(long_text_name("Vélo : bilan/2026\\ville\tà\u{200B}vélo\n"), "Vélo bilan 2026 ville à vélo.txt");
        assert_eq!(long_text_name("...caché\n"), "caché.txt");
        assert_eq!(long_text_name("a/b"), "a b.txt");
        let long = long_text_name(&"Pourquoi le vélo électrique a gagné la ville ".repeat(3));
        assert!(
            long.chars().count() <= 64 && long.ends_with(".txt") && long.contains(' ') && !long.contains('_'),
            "{long}"
        );
        let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/threads/send.rs"))
            .unwrap();
        let made = &src[src.find("async fn long_text_as_file").unwrap()..];
        let made = &made[..made.find("\n}\n").unwrap()];
        assert!(made.contains("name: Some(name)"), "the attachment named after the stored file (« _ »): {made}");
    }

    #[test]
    fn a_message_takes_a_long_text_up_to_its_limit() {
        let check = |chars: usize| {
            let mut errors = Vec::new();
            let body: NewMessage =
                serde_json::from_value(json!({"client_id": "0F0E-1A2B-3C4D", "text": "é".repeat(chars)})).unwrap();
            body.validate(&mut errors);
            errors.len()
        };
        assert_eq!((check(150_000), check(200_000), check(200_001)), (0, 0, 1));
        assert_eq!(LONG_TEXT_MAX, 200_000);
    }

    /// 24/11: a hub full (Hermes' 429 past the bridge's tries, hermes.rs `BUSY_ATTEMPTS`) never fails the person's
    /// message: it waits in line (« En attente ») and starts once a run of the hub ends, as for a gateway the bridge
    /// just restarted. Hermes down otherwise stays the plain error.
    #[test]
    fn a_message_waits_in_line_when_the_hub_is_full() {
        // a gateway no other test restarts (agent_restart.rs keeps the restarts by space, the hub's « », for the
        // whole process)
        let agent = AgentConfig { name: "vie".into(), space: Some("hub-full-test".into()), ..AgentConfig::default() };
        assert!(waits_in_line(&agent, &crate::hermes::HermesError::http(429, "Too many concurrent runs (max 16)")));
        assert!(!waits_in_line(&agent, &crate::hermes::HermesError::http(502, "")));
        assert!(!waits_in_line(&agent, &crate::hermes::HermesError::http(409, "")));
    }

    /// The phone a message is sent from (7/11: where a reminder asked in its turn goes): « ios » or « android ».
    #[test]
    fn a_message_says_its_phone_or_nothing() {
        let check = |phone: Value| {
            let mut errors = Vec::new();
            let body: NewMessage = serde_json::from_value(json!({"client_id": "0F0E-1A2B-3C4D", "text": "Rappelle-moi",
                                                                  "phone": phone})).unwrap();
            body.validate(&mut errors);
            errors.len()
        };
        assert_eq!((check(json!("ios")), check(json!("android")), check(Value::Null)), (0, 0, 0));
        assert_eq!(check(json!("watch")), 1);
    }
}
