//! New mail of an Outlook mailbox (outlook.rs) for the routines that start on one (triggers.rs, kind `mail`, its
//! mailbox `outlook-<n>`) and for its standing rules (outlook_rules.rs) — what triggers_mail.rs does for an IMAP box by
//! IDLE, done here by Graph (Alex 24/11, « comme pour les autres boîtes »).
//!
//! How (the simplest that holds, chosen over Graph's two other ways): each mailbox some active routine or rule needs is
//! looked at every `[triggers] poll_minutes` (2 minutes at most): ONE `GET /me/mailFolders/inbox/messages` of the mails
//! received since where it was left (`receivedDateTime ge`, the oldest first, `PER_LOOK` at most), the ids seen in the
//! last minutes kept so that a mail is offered once even when two share a second. No web hook (Graph's subscriptions):
//! they need a public address that Graph reaches (a home has none), a validation round, a renewal every 3 days at most
//! for mail, and still a look to catch what was missed while the place was down. No delta query either: its first
//! round walks the whole inbox (tens of thousands of mails), and it tells every change of every mail (read, flagged,
//! moved), most of them of no use here. The first look starts from the newest mail: old mail is never replayed.
//!
//! Each new mail is offered to the mailbox's routines (triggers_mail.rs `offer`: the same filters, queue, caps, cards
//! and budget as an IMAP box's; its text, the first `FETCH_BYTES`, only when a routine reads it whole, else its headers;
//! never marked read), then its row goes to the mailbox's rules. What a mail says stays data: triggers.rs puts it in
//! the routine's hidden note as content written by someone else.
//!
//!     <data>/outlook/watch.json (0600)   {account: {after, seen}}   where each inbox was left
//!
//! Not a Python module.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::AppState;
use crate::config::Config;
use crate::drives::Api;
use crate::outlook::{self, AccountId, PREFER_IMMUTABLE};
use crate::outlook_mail;
use crate::triggers::{self, Event, MAIL};

/// New mails read in one look (a mailbox that received hundreds at once: the next ones wait the next look).
pub const PER_LOOK: usize = 50;
/// A look at most this often, whatever `[triggers] poll_minutes` says.
pub const MAX_MINUTES: f64 = 2.0;
/// How far back each look reads again (the ids seen meanwhile are left out): a mail stamped a little before the last
/// one seen is never lost.
const OVERLAP_SECONDS: i64 = 120;
const SEEN_MAX: usize = 500;
const SELECT: &str = "id,subject,from,receivedDateTime,internetMessageId,isRead,flag,isDraft,hasAttachments";

/// Where an inbox was left: the newest `receivedDateTime` read, the ids seen since `after - OVERLAP_SECONDS`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cursor {
    pub after: String,
    pub seen: Vec<String>,
}

static FILE: Mutex<()> = Mutex::new(());
/// When each mailbox (place, account) was last looked at.
static LOOKED: LazyLock<Mutex<HashMap<(String, AccountId), Instant>>> = LazyLock::new(Default::default);

fn cursors(config: &Config) -> BTreeMap<String, Cursor> {
    outlook::read_file(config, "watch.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(config: &Config, account: AccountId, cursor: Option<&Cursor>) {
    let _held = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = cursors(config);
    match cursor {
        Some(c) => all.insert(account.to_string(), c.clone()),
        None => all.remove(&account.to_string()),
    };
    if let Err(e) = serde_json::to_vec(&all).map_err(anyhow::Error::from).and_then(|b| outlook::write_file(config, "watch.json", &b)) {
        tracing::warn!(error = %e, "outlook: where the inbox was left not kept");
    }
}

/// An account disconnected: where its inbox was left forgotten.
pub fn forget_account(config: &Config, account: AccountId) {
    save(config, account, None);
    LOOKED.lock().unwrap_or_else(|e| e.into_inner()).remove(&(config.data_dir(), account));
}

/// The accounts some active e-mail routine or standing rule needs, each only for an agent that reads its mail.
pub fn watched(config: &Config) -> Vec<AccountId> {
    if !config.outlook.enabled {
        return Vec::new();
    }
    let wanted = triggers::load(config)
        .triggers
        .into_iter()
        .filter(|t| t.enabled && t.kind == MAIL)
        .filter_map(|t| t.mail.map(|m| (m.account, t.agent)))
        .chain(crate::mail_rules::watched(config));
    let mut out: Vec<AccountId> = Vec::new();
    for (key, agent) in wanted {
        let Some(id) = outlook::key_account(&key) else { continue };
        if !out.contains(&id) && outlook::parts_of(config, &agent, id).is_some_and(|p| p.mail) {
            out.push(id);
        }
    }
    out
}

fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A new mail as a routine's event: its text (the first `FETCH_BYTES`, Graph's text body, the attachments named) only
/// when `whole`, else its headers.
async fn event_of(config: &Config, account: AccountId, api: &Api, v: &Value, whole: bool) -> Event {
    let id = text(v, "id");
    let mut body = String::new();
    if whole {
        let url = format!("{}/me/messages/{id}", outlook::base(api));
        let prefer = [PREFER_IMMUTABLE, "outlook.body-content-type=\"text\""];
        if let Ok(m) = outlook::fetch(api, &url, &[("$select", "body".to_string())], &prefer).await {
            let content = m.pointer("/body/content").and_then(Value::as_str).unwrap_or("");
            body = if m.pointer("/body/contentType").and_then(Value::as_str) == Some("html") {
                crate::mailbox::html_text(content)
            } else {
                content.to_string()
            };
            body = crate::tools_knowledge::head(&body, crate::triggers_mail::FETCH_BYTES).to_string();
        }
        if v.get("hasAttachments").and_then(Value::as_bool).unwrap_or(false) {
            let url = format!("{}/me/messages/{id}/attachments", outlook::base(api));
            if let Ok((files, _)) = outlook::pages(api, &url, &[("$select", "name,isInline".to_string())], &[PREFER_IMMUTABLE], 50).await {
                let names: Vec<String> = files
                    .iter()
                    .filter(|a| !a.get("isInline").and_then(Value::as_bool).unwrap_or(false))
                    .map(|a| text(a, "name"))
                    .collect();
                if !names.is_empty() {
                    body.push_str(&format!("\n\n[attachments: {}]", names.join(", ")));
                }
            }
        }
    }
    Event {
        kind: MAIL.into(),
        at: crate::py::time_time(),
        from: outlook_mail::person_of(v.get("from").unwrap_or(&Value::Null)),
        subject: text(v, "subject"),
        date: text(v, "receivedDateTime"),
        message_id: text(v, "internetMessageId").trim().to_string(),
        mail_ref: outlook_mail::mail_ref(config, account, "inbox", &id),
        text: body,
        ..Event::default()
    }
}

/// One look at account `account`'s inbox: its new mails offered to its routines, then sorted by its rules. The number
/// of new mails.
pub async fn look_at(config: &Config, account: AccountId) -> Result<usize, String> {
    let api = outlook::signed_in(config, account).await.map_err(|f| outlook::words(&f))?;
    let inbox = outlook_mail::inbox(&api).await.map_err(|f| outlook::words(&f))?;
    let url = format!("{}/me/mailFolders/{}/messages", outlook::base(&api), inbox.id);
    let Some(mut cursor) = cursors(config).remove(&account.to_string()) else {
        // the first look: from the newest mail on
        let newest = [("$top", "1".to_string()), ("$orderby", "receivedDateTime desc".to_string()), ("$select", "id,receivedDateTime".to_string())];
        let page = outlook::fetch(&api, &url, &newest, &[PREFER_IMMUTABLE]).await.map_err(|f| outlook::words(&f))?;
        let first = page.get("value").and_then(Value::as_array).and_then(|a| a.first()).cloned();
        let cursor = match first {
            Some(m) => Cursor { after: text(&m, "receivedDateTime"), seen: vec![text(&m, "id")] },
            None => Cursor { after: stamp(chrono::Utc::now()), seen: Vec::new() },
        };
        save(config, account, Some(&cursor));
        tracing::info!(account, "outlook: inbox watched from its newest mail");
        return Ok(0);
    };
    let after = chrono::DateTime::parse_from_rfc3339(&cursor.after).map(|d| d.with_timezone(&chrono::Utc)).unwrap_or_else(|_| chrono::Utc::now());
    let since = stamp(after - chrono::Duration::seconds(OVERLAP_SECONDS));
    let query = [
        ("$filter", format!("receivedDateTime ge {since}")),
        ("$orderby", "receivedDateTime asc".to_string()),
        ("$top", PER_LOOK.to_string()),
        ("$select", SELECT.to_string()),
    ];
    let page = outlook::fetch(&api, &url, &query, &[PREFER_IMMUTABLE]).await.map_err(|f| outlook::words(&f))?;
    let items = page.get("value").and_then(Value::as_array).cloned().unwrap_or_default();
    let new: Vec<Value> = items.iter().filter(|m| !cursor.seen.contains(&text(m, "id"))).cloned().collect();
    // where it is left: the newest stamp read, the ids of the last minutes
    if let Some(newest) = items.iter().map(|m| text(m, "receivedDateTime")).max() {
        if newest > cursor.after {
            cursor.after = newest;
        }
    }
    let floor = chrono::DateTime::parse_from_rfc3339(&cursor.after)
        .map(|d| stamp(d.with_timezone(&chrono::Utc) - chrono::Duration::seconds(OVERLAP_SECONDS)))
        .unwrap_or_default();
    let mut seen: Vec<String> = items.iter().filter(|m| text(m, "receivedDateTime") >= floor).map(|m| text(m, "id")).collect();
    let older: Vec<String> = cursor.seen.iter().filter(|id| !seen.contains(id)).cloned().collect();
    seen.extend(older);
    seen.truncate(SEEN_MAX);
    cursor.seen = seen;
    save(config, account, Some(&cursor));
    if new.is_empty() {
        return Ok(0);
    }
    let key = outlook::mailbox_key(account);
    // its routines first (a mailbox watched only for its rules: no mail read whole), then its rules
    let (c, k) = (config.clone(), key.clone());
    let (routines, whole) = tokio::task::spawn_blocking(move || {
        (crate::triggers_mail::has_routines(&c, &k), crate::triggers_mail::reads_text(&c, &k))
    })
    .await
    .unwrap_or((false, false));
    if routines {
        for m in &new {
            let event = event_of(config, account, &api, m, whole).await;
            let (c, k) = (config.clone(), key.clone());
            let _ = tokio::task::spawn_blocking(move || crate::triggers_mail::offer(&c, &k, &event)).await;
        }
    }
    let rows = outlook_mail::rows_of(config, account, &new);
    crate::outlook_rules::sort_new(config, account, &api, &inbox.id, &rows).await;
    tracing::info!(account, new = new.len(), "outlook: new mail looked at");
    Ok(new.len())
}

/// One look at every mailbox watched now, whatever the time since the last one (the tests; `run_forever` waits its
/// time). The number of new mails.
pub async fn look(state: &AppState) -> usize {
    let config = state.config();
    let mut seen = 0;
    for account in watched(&config) {
        match look_at(&config, account).await {
            Ok(n) => seen += n,
            Err(why) => tracing::warn!(account, why = %why, "outlook: inbox not looked at"),
        }
        LOOKED.lock().unwrap_or_else(|e| e.into_inner()).insert((config.data_dir(), account), Instant::now());
    }
    seen
}

/// Looks at each mailbox watched every `poll_minutes` (`MAX_MINUTES` at most), as long as the bridge runs.
pub async fn run_forever(state: AppState) {
    loop {
        let config = state.config();
        let every = Duration::from_secs_f64((config.triggers.poll_minutes.min(MAX_MINUTES) * 60.0).max(1.0));
        if config.triggers.mail && config.outlook.enabled {
            for account in watched(&config) {
                let key = (config.data_dir(), account);
                let due = LOOKED.lock().unwrap_or_else(|e| e.into_inner()).get(&key).is_none_or(|at| at.elapsed() >= every);
                if !due {
                    continue;
                }
                LOOKED.lock().unwrap_or_else(|e| e.into_inner()).insert(key, Instant::now());
                if let Err(why) = look_at(&config, account).await {
                    tracing::warn!(account, why = %why, "outlook: inbox not looked at");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs_f64(config.triggers.tick_seconds.clamp(0.2, 10.0))).await;
    }
}
