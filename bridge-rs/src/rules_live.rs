//! The rules of « Entre tes agents » (rules.rs) at work: the apps' screen and its small changes, what two linked
//! households tell each other of their rules, the card a proposal makes in the other person's Boîte, and what follows
//! a change (Bot Mode, the contacts, the tools, the shared folders' mounts).
//!
//! The apps (piste A, Alex 21/11), under /v1 with the bridge key; each change answers with the whole screen:
//!
//!     GET  /v1/agent-rules                          the screen: rules, agents, households both ways, what works
//!     PUT  /v1/agent-rules/write                    {mode: none|some|all}        « Tes agents peuvent s'écrire »
//!     PUT  /v1/agent-rules/read                     {mode: none|some|all}        « …lisent les dossiers partagés »
//!     PUT  /v1/agent-rules/pairs                    {from, to, on}               a « lien à part » (under some)
//!     PUT  /v1/agent-rules/reads                    {reader, source, on}         an explicit read (under some)
//!     PUT  /v1/agent-rules/aside                    {agent, on}                  « À l'écart »
//!     PUT  /v1/agent-rules/family/{agent}           {target, on} | {targets}     « Peut demander à »
//!     PUT  /v1/agent-rules/households/{id}          {way: in|out, mode: none|all|only, agents?}
//!     POST /v1/agent-rules/households/{id}/answer   {way, accept}                « D'accord » / « Non merci »
//!
//! Between households (their key; `{rules: Wire, sig}`, `sig` the link's own key's HMAC when the link has one):
//!
//!     POST /family/rules     their side of both ways → ours in the answer, the same shape
//!
//! Each bridge tells the other its side after every change of it, at start, when the screen is opened, and again every
//! minute until the other has it (`run_forever`). Only the rules cross (rules.rs `Wire`), never other data.
//!
//! A proposal of theirs that asks more than you allow (« Alex ouvre son agent Vie à tes agents ») is a card of the
//! bridge's own (`bip-rules-<way>-<fingerprint>-<household>`, « D'accord » = once, « Non merci » = deny), in the
//! Boîte, notified once; it goes when answered or when they change what they propose.
//!
//! No Python counterpart.

use std::time::Duration;

use axum::extract::{Path as UrlPath, Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::{json, Value};

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::config::Config;
use crate::rules::{self, Mode, Rule, Rules, Scope, Way, Wire, World};
use crate::web::{self, ApiResult, HttpError, JsonObject};
use crate::{py, Dict};

pub const RUN_PREFIX: &str = "bip-rules-";
/// How long a call to the other household waits.
const TELL_SECONDS: u64 = 15;
/// While the screen waits.
const SCREEN_SECONDS: u64 = 4;
/// How often the households that don't have our side yet are told again (a home that fetches its calls takes them
/// at its next poll); everyone, this often anyway.
const AGAIN: Duration = Duration::from_secs(60);
const EVERYONE: f64 = 6.0 * 3600.0;
/// The most agents a rule from another household may name.
const MAX_NAMED: usize = 200;

// -- what follows a change ------------------------------------------------------------------------------------------

/// After the rules changed: the contacts (peers.rs `refresh`), the tools (who may write to whom decides
/// `send_files_to_agent` and `message_agent`), the shared folders mounted again once (shares.rs `sync`), and the
/// households in `told` told our side (waited for a few seconds: the screen then says where they stand; one that
/// doesn't answer is told again later). No agent leaves the hub for a rule (Alex 21/11: guest turns).
pub async fn follow(state: &AppState, told: &[String]) {
    take_effect(state).await;
    tell_all(state, told).await;
}

/// What follows a change on this place (`follow` but telling no one).
async fn take_effect(state: &AppState) {
    let config = state.config();
    if let Err(exc) = crate::peers::refresh(&config, &state.links, Some(&state.households)) {
        tracing::warn!(error = %exc, "agent rules: contacts not set");
    }
    let run = crate::agents_admin::default_run();
    for agent in config.agents.values() {
        let _ = crate::agents_admin::give_tools(&config, agent, &run).await;
    }
    if let Err(exc) = crate::shares::sync(&config, &run).await {
        tracing::warn!(error = %crate::agents_admin::tail(&exc.to_string(), 200), "agent rules: shared folders not mounted");
    }
}

/// Tells these households our side at once, each waited for `SCREEN_SECONDS` at most (`run_forever` tells again
/// those that didn't answer).
pub async fn tell_all(state: &AppState, households: &[String]) {
    futures::future::join_all(households.iter().map(|h| tell_within(state, h, SCREEN_SECONDS))).await;
}

/// An agent made or deleted: the rules cover it at once. The contacts as they now are, the tools of the agents that
/// just got their first recipient (or lost their last), the shared folders' mounts.
pub async fn agents_changed(config: &Config, run: &Run, before: &[(String, bool)]) {
    let links = crate::peers::Links::new(&py::join(&config.data_dir(), "peers.json"));
    let households = crate::households::Households::new(&crate::households::path_for(config));
    if let Err(exc) = crate::peers::refresh(config, &links, Some(&households)) {
        tracing::warn!(error = %exc, "agent rules: contacts not set");
    }
    for agent in config.agents.values() {
        let had = before.iter().find(|(name, _)| *name == agent.name).map(|(_, offered)| *offered);
        if had.is_some_and(|had| had != crate::agent_messages::offered(config, &agent.name)) {
            let _ = crate::agents_admin::give_tools(config, agent, run).await;
        }
    }
    if let Err(exc) = crate::shares::sync(config, run).await {
        tracing::warn!(error = %crate::agents_admin::tail(&exc.to_string(), 200), "agent rules: shared folders not mounted");
    }
    // (no agent leaves the hub because a rule lets others write to it, Alex 21/11: their messages are guest turns)
}

/// Whether each agent writes to another one now (for `agents_changed`).
pub fn offered_now(config: &Config) -> Vec<(String, bool)> {
    config.agents.values().map(|a| (a.name.clone(), crate::agent_messages::offered(config, &a.name))).collect()
}

// -- between two households -----------------------------------------------------------------------------------------

/// The link's own key's signing key (households.json `seal`), when the link has one.
fn key_of(entry: &Dict) -> Option<[u8; 32]> {
    entry.get("seal").and_then(Value::as_str).and_then(rules::signing_key)
}

/// `{rules, sig}`.
fn signed(entry: &Dict, wire: &Wire) -> Value {
    let mut body = json!({"rules": wire});
    if let Some(key) = key_of(entry) {
        body["sig"] = json!(rules::sign(&key, wire));
    }
    body
}

/// A rule from another household, as far as it may be one: agents' ids only, a few hundred at most.
fn plausible(rule: &Rule) -> bool {
    let id = |a: &String| !a.is_empty() && a.len() <= 64 && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    rule.agents.len() <= MAX_NAMED && rule.except.len() <= MAX_NAMED && rule.agents.iter().chain(&rule.except).all(id)
}

/// Their side, from `{rules, sig}`: signed with the link's own key when the link has one. None: not theirs.
fn opened(entry: &Dict, body: &Value) -> Option<Wire> {
    let wire: Wire = serde_json::from_value(body.get("rules")?.clone()).ok()?;
    if let Some(key) = key_of(entry) {
        let signature = body.get("sig").and_then(Value::as_str)?;
        if !rules::signed_by(&key, &wire, signature) {
            return None;
        }
    }
    if !plausible(&wire.from_you) || !plausible(&wire.to_you) {
        return None;
    }
    Some(Wire { from_you: wire.from_you.tidy(), to_you: wire.to_you.tidy(), ..wire })
}

/// Our side as we tell it to `household` now.
fn our_side(config: &Config, household: &str) -> Wire {
    rules::wire_for(&World::of(config, false), &rules::load(config), household, py::time_time())
}

/// Tells `household` our side, and keeps theirs from its answer. True when its bridge had it.
pub async fn tell(state: &AppState, household: &str) -> bool {
    tell_within(state, household, TELL_SECONDS).await
}

async fn tell_within(state: &AppState, household: &str, seconds: u64) -> bool {
    let config = state.config();
    let Some(entry) = state.households.get(household) else { return false };
    let wire = our_side(&config, household);
    let url = format!("{}/family/rules", crate::family_relay::reach_url(&entry));
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let sent = crate::door_tls::household_client(&state.http, &entry)
        .post(url)
        .timeout(Duration::from_secs(seconds))
        .bearer_auth(&key)
        .json(&signed(&entry, &wire))
        .send()
        .await;
    let answer = match sent {
        Ok(resp) if resp.status().as_u16() == 200 => resp.json::<Value>().await.ok(),
        Ok(resp) => {
            tracing::info!(household, status = resp.status().as_u16(), "agent rules: their bridge did not take our side");
            None
        }
        Err(_) => {
            tracing::info!(household, "agent rules: their bridge does not answer (told again later)");
            None
        }
    };
    let Some(theirs) = answer.as_ref().and_then(|a| opened(&entry, a)) else { return false };
    received(state, household, &theirs, Some(wire.at)).await;
    true
}

/// Their side kept (`told_at`: when they got ours, in the same exchange). What works changed: what follows from it;
/// a new proposal of theirs: the person told, once.
async fn received(state: &AppState, household: &str, theirs: &Wire, told_at: Option<f64>) {
    let config = state.config();
    let world = World::of(&config, false);
    let changed = rules::change(&config, |r| {
        rules::heard(r, household, theirs);
        if let (Some(at), Some(h)) = (told_at, r.households.get_mut(household)) {
            h.told = h.told.max(at);
        }
    });
    let (before, after) = match changed {
        Ok(both) => both,
        Err(exc) => {
            tracing::warn!(household, error = %exc, "agent rules: their side not kept");
            return;
        }
    };
    let works = |r: &Rules| rules::effective_links(&world, r);
    if works(&before) != works(&after) {
        tracing::info!(household, "agent rules: what works with them changed");
        let later = state.clone();
        state.spawn(async move { take_effect(&later).await });
    }
    // a proposal naming agents of theirs not listed yet: their list first (the card and its notification name them)
    let (later, id) = (state.clone(), household.to_string());
    state.spawn(async move {
        let current = rules::load(&later.config());
        let listed = current.households.get(&id).map(rules::Household::their_agents).unwrap_or_default();
        let named = rules::pending_proposal(&world, &current, &id, Way::Out).map(|(p, _)| p.agents).unwrap_or_default();
        if named.iter().any(|a| !listed.contains(a)) {
            their_agents(&later, &id, SCREEN_SECONDS).await;
        }
        notify_proposals(&later, &world, &id).await;
    });
}

/// A proposal of theirs waiting for the person, not notified yet: a notification (one per proposal).
async fn notify_proposals(state: &AppState, world: &World, household: &str) {
    let config = state.config();
    for way in [Way::In, Way::Out] {
        let current = rules::load(&config);
        let Some((proposal, fingerprint)) = rules::pending_proposal(world, &current, household, way) else { continue };
        let mut first = false;
        if rules::change(&config, |r| first = rules::first_notice(r, household, way, &fingerprint)).is_err() || !first {
            continue;
        }
        let card = card(&config, world, &current, household, way, &proposal, &fingerprint);
        let (agent, run_id, request) = (str_of(&card, "agent"), str_of(&card, "run_id"), str_of(&card, "request_id"));
        let line = str_of(&card, "description");
        let choices = ["once".to_string(), "deny".to_string()];
        tracing::info!(household, way = way.as_str(), "agent rules: a proposal of theirs in the Boîte");
        state
            .push
            .notify_approval(&agent, &run_id, Some(request.as_str()), Some(&choices[..]), Some(line.as_str()), None)
            .await;
    }
}

fn str_of(item: &Dict, key: &str) -> String {
    item.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The caller's token (`authorization` without its « Bearer » prefix).
fn bearer(headers: &HeaderMap) -> String {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
    header.strip_prefix("Bearer ").unwrap_or(header).trim().to_string()
}

/// `POST /family/rules`: a linked household's side of both ways (its key, signed with the link's own key when the
/// link has one); ours in the answer.
async fn rules_route(State(state): State<AppState>, headers: HeaderMap, request: Request) -> ApiResult<Response> {
    let token = bearer(&headers);
    let household = if token.is_empty() { None } else { state.households.caller(&token) };
    let Some(household) = household else { return Err(HttpError::new(401, "unknown household")) };
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| HttpError::new(413, "request body too large"))?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| HttpError::new(400, "not JSON"))?;
    let entry = state.households.get(&household).unwrap_or_default();
    let Some(theirs) = opened(&entry, &body) else {
        tracing::warn!(household = %household, "agent rules: a side not signed with the link's key refused");
        return Err(HttpError::new(403, "not signed with this link's key"));
    };
    received(&state, &household, &theirs, None).await;
    let config = state.config();
    let ours = our_side(&config, &household);
    let _ = rules::change(&config, |r| {
        if let Some(h) = r.households.get_mut(&household) {
            h.told = h.told.max(ours.at);
        }
    });
    Ok(web::ok(signed(&entry, &ours)))
}

/// What a linked household's bridge may call (mounted at the root, its key checked).
pub fn household_router() -> Router<AppState> {
    Router::new().route("/family/rules", post(rules_route))
}

/// Every linked household told our side once at start, then again every minute those that don't have its last change,
/// and everyone every few hours (a change of theirs we missed).
pub async fn run_forever(state: AppState) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    let config = state.config();
    if config.hermes.enabled() {
        if let Err(exc) = crate::shares::sync(&config, &crate::agents_admin::default_run()).await {
            tracing::warn!(error = %crate::agents_admin::tail(&exc.to_string(), 200), "agent rules: shared folders not mounted");
        }
    }
    let mut last_all = 0.0;
    loop {
        let now = py::time_time();
        let everyone = now - last_all > EVERYONE;
        let current = rules::load(&state.config());
        for household in state.households.public().iter().map(|h| h.get("id").map(py::py_str).unwrap_or_default()) {
            let behind = current.households.get(&household).is_none_or(|h| h.changed >= h.told || h.told == 0.0);
            if everyone || behind {
                tell(&state, &household).await;
            }
        }
        if everyone {
            last_all = now;
        }
        tokio::time::sleep(AGAIN).await;
    }
}

// -- the card of a proposal -----------------------------------------------------------------------------------------

fn shown_of(config: &Config, id: &str) -> String {
    config.agent(Some(id)).map(|a| crate::agent_files::shown(a).to_string()).unwrap_or_else(|| id.to_string())
}

/// An agent of theirs not listed yet, by its id: « cuisine » → « Cuisine ».
fn capitalized(id: &str) -> String {
    let mut chars = id.chars();
    chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

fn joined(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} et {last}", rest.join(", ")),
    }
}

/// The card of household `id`'s proposal that way, in the Boîte (Hermes' approvals' shape, as the others).
fn card(config: &Config, world: &World, current: &Rules, id: &str, way: Way, proposal: &Rule, fingerprint: &str) -> Dict {
    let households = crate::households::Households::new(&crate::households::path_for(config));
    let name = households
        .get(id)
        .and_then(|h| h.get("name").map(py::py_str))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| id.to_string());
    let household = current.households.get(id).cloned().unwrap_or_default();
    let mine = rules::eligible(world, current);
    let (agents, shown): (Vec<String>, Vec<String>) = match way {
        Way::In => {
            let listed: Vec<String> = match proposal.mode {
                Scope::All => Vec::new(),
                _ => proposal.agents.iter().filter(|a| mine.contains(*a)).cloned().collect(),
            };
            let shown = listed.iter().map(|a| shown_of(config, a)).collect();
            (listed, shown)
        }
        Way::Out => {
            let listed: Vec<String> = proposal.agents.clone();
            let shown = listed
                .iter()
                .map(|a| household.agents.iter().find(|(x, _)| x == a).map(|(_, n)| n.clone()).unwrap_or_else(|| capitalized(a)))
                .collect();
            (listed, shown)
        }
    };
    let all = proposal.mode == Scope::All;
    let (sentence, line) = match (way, all, shown.len()) {
        (Way::In, true, _) => (format!("{name} propose que ses agents écrivent à tous tes agents"), "Ses agents pourront leur écrire.".to_string()),
        (Way::In, false, 1) => (format!("{name} propose que ses agents écrivent à ton agent {}", shown[0]), "Ses agents pourront lui écrire.".to_string()),
        (Way::In, false, _) => (format!("{name} propose que ses agents écrivent à tes agents {}", joined(&shown)), "Ses agents pourront leur écrire.".to_string()),
        (Way::Out, true, _) => (format!("{name} ouvre tous ses agents à tes agents"), "Tes agents pourront leur écrire.".to_string()),
        (Way::Out, false, 1) => (format!("{name} ouvre son agent {} à tes agents", shown[0]), "Tes agents pourront lui écrire.".to_string()),
        (Way::Out, false, _) => (format!("{name} ouvre ses agents {} à tes agents", joined(&shown)), "Tes agents pourront leur écrire.".to_string()),
    };
    // whose card: your first agent it names, else your first agent
    let agent = agents
        .iter()
        .find(|a| way == Way::In && mine.contains(*a))
        .cloned()
        .or_else(|| mine.first().cloned())
        .or_else(|| config.agents.keys().next().cloned())
        .unwrap_or_default();
    let request = format!("{}-{fingerprint}-{id}", way.as_str());
    let named: Vec<Value> = agents.iter().zip(&shown).map(|(a, n)| json!({"id": a, "name": n})).collect();
    json!({
        "agent": agent,
        "run_id": format!("{RUN_PREFIX}{request}"),
        "request_id": request,
        "command": sentence,
        "description": sentence,
        "choices": ["once", "deny"],
        "created_at": household.theirs.as_ref().map(|t| t.at).unwrap_or(0.0),
        "session_id": null,
        "bip_rules": {"household": id, "household_name": name, "way": way.as_str(), "all": all, "agents": named,
                      "line": line},
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// The proposals waiting for the person, as cards (card_place.rs `bridge_cards`).
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let current = rules::load(config);
    if current.households.is_empty() {
        return Vec::new();
    }
    let world = World::of(config, false);
    let mut out = Vec::new();
    for id in current.households.keys() {
        for way in [Way::In, Way::Out] {
            if let Some((proposal, fingerprint)) = rules::pending_proposal(&world, &current, id, way) {
                let item = card(config, &world, &current, id, way, &proposal, &fingerprint);
                if agent.is_none_or(|a| item.get("agent").and_then(Value::as_str) == Some(a)) {
                    out.push(item);
                }
            }
        }
    }
    out
}

/// « D'accord » (accept) or « Non merci » to household `id`'s proposal that way; `fingerprint`: only that very
/// proposal (a card). False: no such proposal waits.
async fn answer(state: &AppState, id: &str, way: Way, accept: bool, fingerprint: Option<&str>) -> anyhow::Result<bool> {
    let config = state.config();
    let world = World::of(&config, false);
    let mut done = false;
    rules::change(&config, |r| {
        let Some((_, now)) = rules::pending_proposal(&world, r, id, way) else { return };
        if fingerprint.is_some_and(|f| f != now) {
            return;
        }
        done = if accept { rules::accept(&world, r, id, way) } else { rules::decline(r, id, way) };
        if let Some(h) = r.households.get_mut(id).filter(|_| done) {
            h.changed = py::time_time();
        }
    })?;
    if done {
        tracing::info!(household = id, way = way.as_str(), accept, "agent rules: a proposal answered");
        follow(state, &[id.to_string()]).await;
    }
    Ok(done)
}

/// `POST /v1/approve` for a card `bip-rules-…` (app.rs `bridge_card`).
pub async fn approve(state: &AppState, id: &str, choice: &str) -> ApiResult<Response> {
    let parsed = id.split_once('-').and_then(|(way, rest)| Some((Way::parse(way)?, rest.split_once('-')?)));
    let Some((way, (fingerprint, household))) = parsed else {
        return Err(HttpError::new(404, "no such request (answered already?)"));
    };
    let allow = crate::card_place::allowed(choice);
    let _guard = crate::agents_admin::LOCK.lock().await;
    if answer(state, household, way, allow, Some(fingerprint)).await? {
        Ok(web::ok(json!({"ok": true, "choice": if allow { "once" } else { "deny" }})))
    } else {
        Err(HttpError::new(404, "no such request (answered already?)"))
    }
}

// -- the apps' screen -----------------------------------------------------------------------------------------------

/// Household `id`'s agents as its bridge lists them (`/family/agents`), kept for the listings.
async fn their_agents(state: &AppState, id: &str, seconds: u64) {
    let Some(entry) = state.households.get(id) else { return };
    let url = crate::family_relay::reach_url(&entry);
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let listed = crate::door_tls::household_client(&state.http, &entry)
        .get(format!("{url}/family/agents"))
        .timeout(Duration::from_secs(seconds))
        .bearer_auth(&key)
        .send()
        .await;
    let Ok(resp) = listed.and_then(|r| r.error_for_status()) else { return };
    let Ok(answer) = resp.json::<Value>().await else { return };
    let agents: Vec<(String, String)> = answer
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|a| (a.get("id").map(py::py_str).unwrap_or_default(), a.get("name").map(py::py_str).unwrap_or_default()))
        .filter(|(id, _)| !id.is_empty())
        .collect();
    rules::remember_agents(&state.config(), id, &agents);
}

/// Their agents and their side of the rules, for every linked household at once, each waiting `SCREEN_SECONDS` at
/// most.
async fn refresh_households(state: &AppState) {
    let ids: Vec<String> = state.households.public().iter().map(|h| h.get("id").map(py::py_str).unwrap_or_default()).collect();
    let calls = ids.into_iter().map(|id| {
        let state = state.clone();
        async move {
            their_agents(&state, &id, SCREEN_SECONDS).await;
            tell_within(&state, &id, SCREEN_SECONDS).await;
        }
    });
    futures::future::join_all(calls).await;
}

fn pairs_json(pairs: &[(String, String)], a: &str, b: &str) -> Vec<Value> {
    pairs.iter().map(|(x, y)| json!({a: x, b: y})).collect()
}

/// The whole screen (`refresh`: their agents and their side asked first).
pub async fn screen(state: &AppState, refresh: bool) -> Value {
    if refresh {
        refresh_households(state).await;
    }
    let config = state.config();
    let world = World::of(&config, true);
    let current = rules::load(&config);
    let agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| {
            json!({"id": a.name, "name": a.display_name, "color": a.color,
                   "family": world.agent(&a.name).is_some_and(|x| x.family),
                   "project": world.agent(&a.name).is_some_and(|x| x.project), "aside": current.is_aside(&a.name),
                   "why_not": crate::shares::why_not(&config, a),
                   "managed": crate::peers::managed_home(&config, a).is_some()})
        })
        .collect();
    let family: Vec<Value> = world
        .agents
        .iter()
        .filter(|a| a.family)
        .map(|a| json!({"id": a.id, "name": shown_of(&config, &a.id), "targets": current.family.get(&a.id).cloned().unwrap_or_default()}))
        .collect();
    let households: Vec<Value> = state
        .households
        .public()
        .into_iter()
        .map(|h| {
            let id = h.get("id").map(py::py_str).unwrap_or_default();
            let entry = current.households.get(&id).cloned().unwrap_or_default();
            let theirs: Vec<Value> = entry.agents.iter().map(|(a, n)| json!({"id": a, "name": n})).collect();
            json!({"id": id, "name": h.get("name").cloned().unwrap_or(Value::Null),
                   "relink": h.get("relink").cloned().unwrap_or(json!(false)), "agents": theirs,
                   "in": rules::way_state(&world, &current, &id, Way::In),
                   "out": rules::way_state(&world, &current, &id, Way::Out),
                   "heard_at": entry.theirs.as_ref().map(|t| t.at), "told_at": entry.told, "changed_at": entry.changed})
        })
        .collect();
    json!({
        "write": current.write, "pairs": pairs_json(&current.pairs, "from", "to"),
        "read": current.read, "reads": pairs_json(&current.reads, "reader", "source"),
        "aside": current.aside, "family": family, "agents": agents, "households": households,
        "effective": {"links": pairs_json(&rules::effective_links(&world, &current), "from", "to"),
                      "reads": pairs_json(&rules::effective_reads(&world, &current), "reader", "source")},
        "default": {"write": rules::DEFAULT_WRITE, "read": rules::DEFAULT_READ},
    })
}

async fn get_screen(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(screen(&state, true).await))
}

fn text(body: &Dict, key: &str) -> String {
    body.get(key).map(py::py_str).unwrap_or_default().trim().to_lowercase()
}

fn flag(body: &Dict, key: &str) -> ApiResult<bool> {
    body.get(key).and_then(Value::as_bool).ok_or_else(|| HttpError::new(400, format!("{key}: true or false")))
}

fn own_agent<'a>(world: &'a World, id: &str) -> ApiResult<&'a rules::Agent> {
    world.agent(id).ok_or_else(|| HttpError::new(400, format!("unknown agent {id}")))
}

/// A change: `edit` on the rules (Err: why not, a 400), then what follows it, then the screen.
async fn changed(
    state: &AppState,
    told: Vec<String>,
    edit: impl FnOnce(&mut Rules),
) -> ApiResult<Response> {
    let _guard = crate::agents_admin::LOCK.lock().await;
    let config = state.config();
    let (before, after) = rules::change(&config, edit)?;
    if before != after {
        follow(state, &told).await;
    }
    Ok(web::ok(screen(state, false).await))
}

async fn set_write(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let mode = Mode::parse(&text(&body, "mode")).ok_or_else(|| HttpError::new(400, "mode: none, some or all"))?;
    changed(&state, Vec::new(), |r| r.write = mode).await
}

async fn set_read(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let mode = Mode::parse(&text(&body, "mode")).ok_or_else(|| HttpError::new(400, "mode: none, some or all"))?;
    changed(&state, Vec::new(), |r| r.read = mode).await
}

async fn set_pair(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let (from, to, on) = (text(&body, "from"), text(&body, "to"), flag(&body, "on")?);
    let world = World::of(&state.config(), false);
    let (a, _) = (own_agent(&world, &from)?, own_agent(&world, &to)?);
    if from == to {
        return Err(HttpError::new(400, "an agent and itself"));
    }
    if a.family {
        return Err(HttpError::new(400, "the family's agent: PUT /v1/agent-rules/family/{agent}"));
    }
    changed(&state, Vec::new(), |r| {
        r.pairs.retain(|(x, y)| !(*x == from && *y == to));
        if on {
            r.pairs.push((from, to));
        }
    })
    .await
}

async fn set_read_pair(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let (reader, source, on) = (text(&body, "reader"), text(&body, "source"), flag(&body, "on")?);
    let config = state.config();
    let world = World::of(&config, true);
    if reader == source {
        return Err(HttpError::new(400, "an agent always reads its own files"));
    }
    for id in [&reader, &source] {
        let agent = own_agent(&world, id)?;
        if on && !agent.can_share {
            let why = config.agent(Some(id.as_str())).and_then(|a| crate::shares::why_not(&config, a)).unwrap_or_default();
            return Err(HttpError::new(400, format!("{id}: {why}")));
        }
    }
    if on && own_agent(&world, &reader)?.family {
        return Err(HttpError::new(400, "the family's agent reads no one's folder"));
    }
    changed(&state, Vec::new(), |r| {
        r.reads.retain(|(x, y)| !(*x == reader && *y == source));
        if on {
            r.reads.push((reader, source));
        }
    })
    .await
}

async fn set_aside(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let (agent, on) = (text(&body, "agent"), flag(&body, "on")?);
    let world = World::of(&state.config(), false);
    own_agent(&world, &agent)?;
    // what the households are told changes (rules.rs `Wire`: yours left out of `all`)
    let told: Vec<String> = world.linked.iter().cloned().collect();
    let now = py::time_time();
    changed(&state, told, |r| {
        r.aside.retain(|a| *a != agent);
        if on {
            r.aside.push(agent);
        }
        for h in r.households.values_mut() {
            h.changed = now;
        }
    })
    .await
}

async fn set_family(
    State(state): State<AppState>,
    UrlPath(agent): UrlPath<String>,
    JsonObject(body): JsonObject,
) -> ApiResult<Response> {
    let agent = agent.trim().to_lowercase();
    let world = World::of(&state.config(), false);
    if !own_agent(&world, &agent)?.family {
        return Err(HttpError::new(400, format!("{agent} is not the family's agent")));
    }
    let wanted: Option<Vec<String>> = match body.get("targets") {
        Some(Value::Array(items)) => Some(items.iter().map(|v| py::py_str(v).trim().to_lowercase()).collect()),
        Some(_) => return Err(HttpError::new(400, "targets: a list of agents")),
        None => None,
    };
    let one = match &wanted {
        Some(_) => None,
        None => Some((text(&body, "target"), flag(&body, "on")?)),
    };
    for target in wanted.iter().flatten().chain(one.iter().map(|(t, _)| t)) {
        if own_agent(&world, target)?.family {
            return Err(HttpError::new(400, "the family's agent asks your own agents only"));
        }
    }
    changed(&state, Vec::new(), |r| {
        let targets = r.family.entry(agent.clone()).or_default();
        match (wanted, one) {
            (Some(all), _) => *targets = all,
            (None, Some((target, on))) => {
                targets.retain(|t| *t != target);
                if on {
                    targets.push(target);
                }
            }
            (None, None) => {}
        }
        if r.family.get(&agent).is_some_and(Vec::is_empty) {
            r.family.remove(&agent);
        }
    })
    .await
}

/// The rule asked for one way with a household, checked: under `only`, your own agents (way in), or ids of theirs
/// (way out).
fn wanted_rule(world: &World, rules_now: &Rules, way: Way, body: &Dict) -> ApiResult<Rule> {
    let mode = Scope::parse(&text(body, "mode")).ok_or_else(|| HttpError::new(400, "mode: none, all or only"))?;
    if mode != Scope::Only {
        return Ok(Rule { mode, ..Rule::default() });
    }
    let agents: Vec<String> = match body.get("agents") {
        Some(Value::Array(items)) => items.iter().map(|v| py::py_str(v).trim().to_lowercase()).collect(),
        _ => return Err(HttpError::new(400, "agents: the agents of `only`")),
    };
    if agents.is_empty() {
        return Err(HttpError::new(400, "agents: at least one (or mode none)"));
    }
    let mine = rules::eligible(world, rules_now);
    for agent in &agents {
        let fine = match way {
            Way::In => mine.contains(agent),
            // their agents' ids: one may also be one of yours (every place's guide is « bip »)
            Way::Out => plausible(&Rule::only([agent.clone()])),
        };
        if !fine {
            return Err(HttpError::new(400, format!("{agent}: not an agent that way")));
        }
    }
    Ok(Rule::only(agents))
}

async fn set_household(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonObject(body): JsonObject,
) -> ApiResult<Response> {
    let config = state.config();
    let world = World::of(&config, false);
    if !world.linked.contains(&id) {
        return Err(HttpError::new(404, "no such household"));
    }
    let way = Way::parse(&text(&body, "way")).ok_or_else(|| HttpError::new(400, "way: in or out"))?;
    let rule = wanted_rule(&world, &rules::load(&config), way, &body)?;
    let now = py::time_time();
    let key = id.clone();
    changed(&state, vec![id], |r| {
        rules::set_mine(&world, r, &key, way, rule, now);
    })
    .await
}

async fn answer_household(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonObject(body): JsonObject,
) -> ApiResult<Response> {
    let way = Way::parse(&text(&body, "way")).ok_or_else(|| HttpError::new(400, "way: in or out"))?;
    let accept = flag(&body, "accept")?;
    let done = {
        let _guard = crate::agents_admin::LOCK.lock().await;
        answer(&state, &id, way, accept, None).await?
    };
    if !done {
        return Err(HttpError::new(404, "nothing proposed that way"));
    }
    Ok(web::ok(screen(&state, false).await))
}

/// The apps' routes (under /v1, bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agent-rules", get(get_screen))
        .route("/agent-rules/write", put(set_write))
        .route("/agent-rules/read", put(set_read))
        .route("/agent-rules/pairs", put(set_pair))
        .route("/agent-rules/reads", put(set_read_pair))
        .route("/agent-rules/aside", put(set_aside))
        .route("/agent-rules/family/{agent}", put(set_family))
        .route("/agent-rules/households/{id}", put(set_household))
        .route("/agent-rules/households/{id}/answer", post(answer_household))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn their_agent_may_have_the_id_of_one_of_yours() {
        // every place's guide is « bip »: « seulement à son Bip » is theirs, whatever yours are called (review 1, B3)
        let world = World {
            agents: vec![rules::Agent { id: "bip".into(), can_share: true, ..Default::default() }],
            linked: ["chi".to_string()].into_iter().collect(),
        };
        let mut body = Dict::new();
        body.insert("mode".into(), json!("only"));
        body.insert("agents".into(), json!(["bip"]));
        assert_eq!(wanted_rule(&world, &Rules::default(), Way::Out, &body).unwrap(), Rule::only(["bip"]));
        body.insert("agents".into(), json!(["../x"]));
        assert!(wanted_rule(&world, &Rules::default(), Way::Out, &body).is_err());
        body.insert("agents".into(), json!(["sante"]));
        assert!(wanted_rule(&world, &Rules::default(), Way::In, &body).is_err(), "way in: yours only");
    }

    #[test]
    fn names_are_joined_in_french() {
        assert_eq!(joined(&["Vie".into()]), "Vie");
        assert_eq!(joined(&["Vie".into(), "Wellness".into()]), "Vie et Wellness");
        assert_eq!(joined(&["A".into(), "B".into(), "C".into()]), "A, B et C");
    }

    #[test]
    fn a_side_is_opened_only_with_the_links_key() {
        let mut entry = Dict::new();
        let wire = Wire { v: 1, to_you: Rule::only(["bip"]), at: 3.0, ..Wire::default() };
        // a link with no key of its own (made before): the household's key alone says who calls
        assert_eq!(opened(&entry, &json!({"rules": wire})), Some(wire.clone()));
        entry.insert("seal".into(), json!(crate::family_seal::encode_key(&[3u8; 32])));
        assert!(opened(&entry, &json!({"rules": wire})).is_none(), "unsigned");
        let body = signed(&entry, &wire);
        assert_eq!(opened(&entry, &body), Some(wire.clone()));
        let mut forged = body.clone();
        forged["rules"]["to_you"] = json!({"mode": "all"});
        assert!(opened(&entry, &forged).is_none(), "changed after signing");
        // never more than rules: ids only, a few hundred at most
        let odd = Wire { to_you: Rule::only(["../x"]), ..wire.clone() };
        let mut plain = Dict::new();
        plain.insert("x".into(), json!(1));
        assert!(opened(&plain, &json!({"rules": odd})).is_none());
    }
}
