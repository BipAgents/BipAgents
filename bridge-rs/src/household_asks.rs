//! An agent asking another household's agent through the two bridges (Alex 21/11: never Hermes' Bot Mode, so no
//! agent ever leaves the shared hub for it).
//!
//! - **The asker's side** (`ask`, called by agent_messages.rs once its rules allow it, rules.rs `may_write`): the ask
//!   goes to the other household's bridge over the household link, as every household call (its key, and through the
//!   home relay or Kerfy Studio's relay when that is how they reach each other, family_relay.rs), signed with the
//!   link's own key when the link has one (`POST /family/ask`). Both sides keep the pair's thread: here
//!   « Vie ⇄ Cuisine (Léa) », the person reads it as any pair (agent_pairs.rs).
//! - **The other side** (`ask_route`): its own rules checked again (both households must agree), Hermes patch 0018
//!   checked (the turn is a relative's: without the patch it would run with the person's own tools), then its agent
//!   asked in their pair's conversation (agent_pairs.rs `ask_household`: signed « Message from Vie (Alex)
//!   (@vie__alex) », the bridge tools of `agent_shares::RELATIVE_TOOLS` only), on its person's credit, its approvals
//!   her cards as for any turn the hub follows. Its answer goes back in the same call, within
//!   `agent_messages::ANSWER_SECONDS`.
//! - **Later than that** (its turn waiting for its person, or queued behind another exchange): the asker is told so,
//!   and the answer is posted back when it comes (`POST /family/answer`), kept in the asker's thread of the pair.
//! - Never files (agent_files.rs: another household's agents get none), and the asker's hop cap (agent_hops.rs) counts
//!   the message as any other; a relative's turn has no `message_agent`, so nothing goes further from there.
//!
//! No Python counterpart.

use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent_pairs::Answer;
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::households::{split, Households, SEPARATOR};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// An ask's body, at most.
const BODY_MAX: usize = 256 * 1024;
/// How long the asker's bridge waits for the other's answer: the target's own time, and the relay's margin.
const CALL_SECONDS: f64 = crate::agent_messages::ANSWER_SECONDS + 60.0;
/// A late answer waits this long at most (a turn waiting for its person).
const LATE_SECONDS: f64 = crate::agent_pairs::APPROVAL_WAIT + crate::agent_messages::ANSWER_SECONDS;

/// What crosses for an ask: who asks (its id and shown name there), whom, the words.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Ask {
    #[serde(default)]
    pub v: u32,
    pub sender: String,
    #[serde(default)]
    pub sender_name: String,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub at: f64,
}

/// A late answer: from their agent to ours.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Late {
    #[serde(default)]
    pub v: u32,
    pub from: String,
    pub to: String,
    /// "answer" or "notice" (agent_pairs.rs `Message::kind`).
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub at: f64,
}

/// What an ask gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The target's answer, or why there is none (agent_pairs.rs).
    Answered(Answer),
    /// The other household's rules don't let this agent write to that one (theirs checked there).
    Refused,
    /// Their Hermes would run another household's turn with its person's tools (patch 0018 missing): refused there.
    Closed,
}

fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `{body, sig}`: `sig` the link's own key's HMAC for `purpose`, when the link has one.
fn signed<T: Serialize>(entry: &Dict, purpose: &[u8], body: &T) -> Value {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let mut out = json!({"body": serde_json::to_value(body).unwrap_or(Value::Null)});
    if let Some(key) = entry.get("seal").and_then(Value::as_str).and_then(|s| crate::rules::link_key(s, purpose)) {
        out["sig"] = json!(crate::rules::mac(&key, &bytes));
    }
    out
}

/// `body` from `{body, sig}`, signed with the link's own key when the link has one. None: not theirs.
fn opened<T: Serialize + for<'de> Deserialize<'de>>(entry: &Dict, purpose: &[u8], wire: &Value) -> Option<T> {
    let body: T = serde_json::from_value(wire.get("body")?.clone()).ok()?;
    if let Some(key) = entry.get("seal").and_then(Value::as_str).and_then(|s| crate::rules::link_key(s, purpose)) {
        let bytes = serde_json::to_vec(&body).ok()?;
        if !crate::rules::mac_ok(&key, &bytes, wire.get("sig").and_then(Value::as_str)?) {
            return None;
        }
    }
    Some(body)
}

fn households(config: &Config) -> Households {
    Households::new(&crate::households::path_for(config))
}

/// « Cuisine (Léa) »: their agent as their bridge last listed it (rules.rs), with its household's name.
pub fn shown_remote(config: &Config, remote: &str) -> String {
    let Some((agent, household)) = split(remote) else { return remote.to_string() };
    let rules = crate::rules::load(config);
    let name = rules
        .households
        .get(&household)
        .and_then(|h| h.agents.iter().find(|(id, _)| *id == agent).map(|(_, n)| n.clone()))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            let mut c = agent.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        });
    let household_name = households(config)
        .get(&household)
        .and_then(|h| h.get("name").map(py::py_str))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(household);
    format!("{name} ({household_name})")
}

fn kept(config: &Config, from: &str, to: &str, kind: &str, text: &str) {
    if let Err(exc) = crate::agent_pairs::record(config, from, to, kind, text, py::time_time()) {
        tracing::warn!(from, to, error = %exc, "household pair: not kept");
    }
}

/// Our `agent` asks `remote` (`cuisine__lea`) through the two bridges (its rules checked by the caller). The answer
/// within `agent_messages::ANSWER_SECONDS`, or why not.
pub async fn ask(http: &reqwest::Client, config: &Config, agent: &AgentConfig, remote: &str, message: &str) -> Outcome {
    let Some((target, household)) = split(remote) else { return Outcome::Answered(Answer::NotDelivered) };
    let Some(entry) = households(config).get(&household) else { return Outcome::Answered(Answer::Unreachable) };
    let shown = shown_remote(config, remote);
    if let Err(exc) = crate::agent_pairs::household_pair(config, remote, &shown, &agent.name) {
        tracing::warn!(agent = %agent.name, target = %remote, error = %exc, "household pair: not kept");
    }
    kept(config, &agent.name, remote, "ask", message);
    let body = Ask {
        v: 1,
        sender: agent.name.clone(),
        sender_name: crate::agent_files::shown(agent).to_string(),
        target,
        message: message.trim().to_string(),
        at: py::time_time(),
    };
    let url = format!("{}/family/ask", crate::family_relay::reach_url(&entry));
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let sent = crate::door_tls::household_client(http, &entry)
        .post(url)
        .timeout(Duration::from_secs_f64(CALL_SECONDS))
        .bearer_auth(&key)
        .json(&signed(&entry, b"ask", &body))
        .send()
        .await;
    let outcome = match sent {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let answer: Value = resp.json().await.unwrap_or(Value::Null);
            let state = answer.get("state").and_then(Value::as_str).unwrap_or_default().to_string();
            match (status, state.as_str()) {
                (200, "answer") => {
                    let text = answer.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                    Outcome::Answered(if text.is_empty() { Answer::Empty } else { Answer::Text(text) })
                }
                (200, "empty") => Outcome::Answered(Answer::Empty),
                (200, "waiting") => Outcome::Answered(Answer::Waiting),
                (200, "queued") => Outcome::Answered(Answer::Queued),
                (200, "not_delivered") => Outcome::Answered(Answer::NotDelivered),
                (403, _) => Outcome::Refused,
                (503, _) => Outcome::Closed,
                _ => Outcome::Answered(Answer::Unreachable),
            }
        }
        Err(exc) => {
            tracing::info!(agent = %agent.name, target = %remote, error = %exc, "household ask: their bridge does not answer");
            Outcome::Answered(Answer::Unreachable)
        }
    };
    match &outcome {
        Outcome::Answered(Answer::Text(text)) => kept(config, remote, &agent.name, "answer", text),
        Outcome::Answered(Answer::Empty) => kept(config, remote, &agent.name, "notice", "answered nothing"),
        Outcome::Answered(Answer::Unreachable) => kept(config, remote, &agent.name, "notice", "couldn't answer"),
        Outcome::Answered(Answer::NotDelivered) => kept(config, remote, &agent.name, "notice", "message not delivered"),
        Outcome::Refused => kept(config, remote, &agent.name, "notice", "their person has not allowed it"),
        Outcome::Closed => kept(config, remote, &agent.name, "notice", "message not delivered"),
        Outcome::Answered(_) => {}
    }
    tracing::info!(agent = %agent.name, target = %remote, refused = matches!(outcome, Outcome::Refused), "household ask");
    outcome
}

fn state_of(answer: &Answer) -> Value {
    match answer {
        Answer::Text(text) => json!({"state": "answer", "text": text}),
        Answer::Empty => json!({"state": "empty"}),
        Answer::Waiting => json!({"state": "waiting"}),
        Answer::Queued => json!({"state": "queued"}),
        Answer::NotDelivered => json!({"state": "not_delivered"}),
        // a relative's turn never writes to an agent (no message_agent there): no chain crosses
        Answer::Unreachable | Answer::Cycle => json!({"state": "unreachable"}),
    }
}

/// `POST /family/ask`: a linked household's agent asks one of ours (its key; signed with the link's own key when the
/// link has one). Our rules again, patch 0018, then our agent in their pair's conversation; its answer in the reply,
/// or later through `POST /family/answer`.
async fn ask_route(State(state): State<AppState>, headers: HeaderMap, request: Request) -> ApiResult<Response> {
    let household = crate::peers::household_caller(&state, &headers)?;
    let bytes = axum::body::to_bytes(request.into_body(), BODY_MAX)
        .await
        .map_err(|_| HttpError::new(413, "request body too large"))?;
    let wire: Value = serde_json::from_slice(&bytes).map_err(|_| HttpError::new(400, "not JSON"))?;
    let entry = state.households.get(&household).unwrap_or_default();
    let Some(ask) = opened::<Ask>(&entry, b"ask", &wire) else {
        tracing::warn!(household = %household, "household ask: not signed with the link's key, refused");
        return Err(HttpError::new(403, "not signed with this link's key"));
    };
    let message = crate::tools_knowledge::strip(&ask.message).to_string();
    if !plain_id(&ask.sender) || message.is_empty() || message.chars().count() > crate::agent_messages::MAX_CHARS {
        return Err(HttpError::new(400, "a sender and a message"));
    }
    let config = state.config();
    let Some(target) = config.agent(Some(&ask.target)).cloned() else {
        return Err(HttpError::new(404, "no such agent"));
    };
    let remote = format!("{}{SEPARATOR}{household}", ask.sender);
    if !crate::rules::allows(&config, &remote, &target.name) {
        tracing::info!(sender = %remote, target = %target.name, "household ask refused (the rules)");
        return Err(HttpError::json(403, json!({"state": "refused", "message": "not allowed by this person"})));
    }
    if !crate::agent_shares::guest_turns_closed(&config, Some(&target)) {
        // a turn another household drives, on a Hermes that would run it open (audit 11/11, BA-2)
        return Ok(crate::agent_shares::guest_turns_refused(&config, 503));
    }
    let household_name = entry.get("name").map(py::py_str).filter(|n| !n.trim().is_empty()).unwrap_or(household.clone());
    let name: String = crate::tools_knowledge::strip(&ask.sender_name).chars().take(60).collect();
    let name = if name.is_empty() { ask.sender.clone() } else { name };
    let shown = format!("{name} ({household_name})");
    tracing::info!(sender = %remote, target = %target.name, "household ask");
    let (answer, later) = crate::agent_pairs::ask_household(
        &state.http,
        Some(state.hub.clone()),
        &config,
        &remote,
        &shown,
        &target,
        &message,
        crate::agent_messages::ANSWER_SECONDS,
    )
    .await;
    if let Some(later) = later {
        // its answer, when it comes, kept in the asker's thread too
        let (state, household, target, sender) = (state.clone(), household.clone(), target.name.clone(), ask.sender);
        let back = state.clone();
        back.spawn(async move {
            let Ok(Ok(answer)) = tokio::time::timeout(Duration::from_secs_f64(LATE_SECONDS), later).await else { return };
            tell_late(&state, &household, &target, &sender, &answer).await;
        });
    }
    Ok(web::ok(state_of(&answer)))
}

/// A late answer of our `from` to their `to`, posted back to household `household`.
async fn tell_late(state: &AppState, household: &str, from: &str, to: &str, answer: &Answer) {
    let (kind, text) = match answer {
        Answer::Text(text) => ("answer", text.clone()),
        Answer::Empty => ("notice", "answered nothing".to_string()),
        Answer::Unreachable | Answer::Cycle => ("notice", "couldn't answer".to_string()),
        Answer::NotDelivered => ("notice", "message not delivered".to_string()),
        Answer::Waiting | Answer::Queued => return,
    };
    let Some(entry) = state.households.get(household) else { return };
    let late = Late { v: 1, from: from.to_string(), to: to.to_string(), kind: kind.into(), text, at: py::time_time() };
    let url = format!("{}/family/answer", crate::family_relay::reach_url(&entry));
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let sent = crate::door_tls::household_client(&state.http, &entry)
        .post(url)
        .timeout(Duration::from_secs(30))
        .bearer_auth(&key)
        .json(&signed(&entry, b"answer", &late))
        .send()
        .await;
    if !sent.is_ok_and(|r| r.status().is_success()) {
        tracing::info!(household, from, to, "household ask: the late answer not given back (kept here)");
    }
}

/// `POST /family/answer`: a late answer of their agent to one of ours, kept in that pair's thread (only a pair that
/// exists: ours asked first).
async fn answer_route(State(state): State<AppState>, headers: HeaderMap, request: Request) -> ApiResult<Response> {
    let household = crate::peers::household_caller(&state, &headers)?;
    let bytes = axum::body::to_bytes(request.into_body(), BODY_MAX)
        .await
        .map_err(|_| HttpError::new(413, "request body too large"))?;
    let wire: Value = serde_json::from_slice(&bytes).map_err(|_| HttpError::new(400, "not JSON"))?;
    let entry = state.households.get(&household).unwrap_or_default();
    let Some(late) = opened::<Late>(&entry, b"answer", &wire) else {
        return Err(HttpError::new(403, "not signed with this link's key"));
    };
    let config = state.config();
    let remote = format!("{}{SEPARATOR}{household}", late.from);
    let known = plain_id(&late.from)
        && config.agent(Some(&late.to)).is_some()
        && crate::agent_pairs::get(&config, &crate::agent_pairs::pair_id(&remote, &late.to)).is_some();
    if !known || !["answer", "notice"].contains(&late.kind.as_str()) {
        return Err(HttpError::new(404, "no such exchange"));
    }
    let text: String = crate::tools_knowledge::strip(&late.text).chars().take(crate::agent_pairs::TEXT_MAX).collect();
    kept(&config, &remote, &late.to, &late.kind, &text);
    tracing::info!(sender = %remote, target = %late.to, "household ask: a late answer kept");
    Ok(web::no_content())
}

/// What a linked household's bridge may call (mounted at the root, its key checked).
pub fn household_router() -> Router<AppState> {
    Router::new().route("/family/ask", post(ask_route)).route("/family/answer", post(answer_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ask_is_opened_only_with_the_links_key() {
        let ask = Ask { v: 1, sender: "vie".into(), sender_name: "Vie".into(), target: "cuisine".into(),
                        message: "On mange quoi ?".into(), at: 1.0 };
        let mut entry = Dict::new();
        // a link with no key of its own: the household's key alone says who calls
        assert_eq!(opened::<Ask>(&entry, b"ask", &signed(&entry, b"ask", &ask)), Some(ask.clone()));
        entry.insert("seal".into(), json!(crate::family_seal::encode_key(&[5u8; 32])));
        let wire = signed(&entry, b"ask", &ask);
        assert_eq!(opened::<Ask>(&entry, b"ask", &wire), Some(ask.clone()));
        assert!(opened::<Ask>(&entry, b"answer", &wire).is_none(), "another purpose's key");
        let mut forged = wire.clone();
        forged["body"]["target"] = json!("budget");
        assert!(opened::<Ask>(&entry, b"ask", &forged).is_none());
        assert!(opened::<Ask>(&entry, b"ask", &json!({"body": wire["body"]})).is_none(), "unsigned");
        assert!(plain_id("vie") && plain_id("kerfy-studio") && !plain_id("../x") && !plain_id(""));
    }

    #[test]
    fn every_answer_has_a_state() {
        assert_eq!(state_of(&Answer::Text("Oui.".into())), json!({"state": "answer", "text": "Oui."}));
        assert_eq!(state_of(&Answer::Waiting), json!({"state": "waiting"}));
        assert_eq!(state_of(&Answer::Cycle), json!({"state": "unreachable"}));
    }
}
