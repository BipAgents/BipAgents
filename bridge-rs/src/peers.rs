//! Agents of two households writing to each other, and the households' links.
//!
//! Since 21/11 (Alex: « pas le Bot Mode, par le pont ») an agent writes to another household's agent only with the
//! bridge's own `message_agent` (agent_messages.rs): this bridge checks its rules (rules.rs: both households must
//! agree), asks the other household's bridge over the household link (household_asks.rs, `POST /family/ask`, signed
//! with the link's own key), which checks its own rules and asks its agent in their pair's conversation (agent_pairs.rs),
//! a relative's turn (patch 0018's guest mark). The answer comes back in the same call. No agent ever leaves the shared
//! hub for it, and Hermes' Bot Mode is off everywhere: a place set up before loses what it still has at start
//! (`drop_bot_mode`: the profiles' mark, the bridge as `bot_peers.bip`, the gateways' tokens in peers.json).
//!
//! This module also writes each agent's contacts (`write_contacts`: whom the rules let it write to), turns an older
//! app's pairs into rules (`apply`), and holds what a linked household's bridge may call (introduce itself, list our
//! agents, its relay pass, its door) and the person's household routes (name, invitations, join, unlink).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use axum::body::Body;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::Router;
use bytes::Bytes;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::households::{own_url, split, HouseholdError, Households};
use crate::web::{self, ApiResult, HttpError};
use crate::Dict;

pub use crate::households::SEPARATOR;

/// What agents see in their contacts: message_agent target "bip/<agent>".
pub const PEER_NAME: &str = "bip";

/// Python's `ValueError` of `apply` and `manual_token` (safe to show).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct InvalidLink(pub String);

/// The tokens given to the gateways (`peers.json`). Its `links`, the pairs of before the rules (rules.rs), stay as the
/// migration found them (a bridge rolled back finds them), never read again.
pub struct Links {
    pub path: PathBuf,
}

impl Links {
    pub fn new(path: &str) -> Self {
        Links { path: PathBuf::from(path) }
    }

    /// `{"tokens": {gateway: digest}}`, and the old `links` as they are.
    pub fn load(&self) -> Dict {
        let data: Value = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        let data = data.as_object().cloned().unwrap_or_default();
        let mut out = Dict::new();
        if let Some(old) = data.get("links") {
            out.insert("links".into(), old.clone());
        }
        out.insert("tokens".into(), data.get("tokens").cloned().unwrap_or_else(|| json!({})));
        out
    }

    pub fn save(&self, data: &Dict) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut content = json!({"tokens": data.get("tokens").cloned().unwrap_or_else(|| json!({}))});
        if let Some(old) = data.get("links") {
            content["links"] = old.clone();
        }
        let tmp = format!("{}.tmp", self.path.display());
        crate::py::write_with_mode(&tmp, crate::py::dumps(&content).as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// The HERMES_HOME of the gateway serving an agent this bridge manages (None: set up by hand).
pub fn managed_home(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    if let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(space));
    }
    if config.hermes.enabled() && agent.hermes_url.starts_with(&format!("{}/p/", config.hermes.hub_url)) {
        let home = if config.hermes.home.is_empty() {
            crate::py::join(&crate::py::expanduser("~"), ".hermes")
        } else {
            config.hermes.home.clone()
        };
        return Some(PathBuf::from(home));
    }
    None
}

/// How Hermes and this bridge sign an agent's message to another agent: « Message from Vie (@vie): … ».
pub static SIGNATURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Message from .{0,200}? \(@([A-Za-z0-9_-]{1,64})\):").expect("valid regex"));


/// The pairs an older app sends (« Qui peut écrire à qui », the whole list), as rules (rules.rs `from_pairs`), then
/// what follows from them ([`refresh`]). Returns `{"manual": {}, "changed": [household…]}`: the households whose rules
/// changed (to tell them); `manual` stays for the older apps' `needs_setup` (no token to give any more: no Bot Mode).
/// `InvalidLink` (Python's `ValueError`) for a link that cannot be.
pub fn apply(
    config: &Config,
    links: &Links,
    wanted: &[(String, String)],
    households: Option<&Households>,
) -> anyhow::Result<Dict> {
    let linked: BTreeSet<String> = match households {
        Some(h) => h
            .load()
            .get("households")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default(),
        None => BTreeSet::new(),
    };
    let known = |name: &str| match split(name) {
        Some((_, household)) => linked.contains(&household),
        None => config.agent(Some(name)).is_some(),
    };
    for (sender, target) in wanted {
        // Between two households, one end is ours: « vie → cuisine__lea » (ours writes) or « vie__alex → cuisine ».
        if sender == target || !known(sender) || !known(target) || (split(sender).is_some() && split(target).is_some())
        {
            return Err(InvalidLink(format!("unknown link {sender} → {target}")).into());
        }
    }
    // the family's agent reaches only this place's agents (agent_messages.rs, Alex 21/11): a link of it to another
    // household's agent is not kept (an older app may still send one; rules.rs leaves it out anyway)
    let unique: BTreeSet<(String, String)> = wanted
        .iter()
        .filter(|(sender, target)| !(split(target).is_some() && crate::family_agent::is_family(config, sender)))
        .cloned()
        .collect();
    let pairs: Vec<(String, String)> = unique.into_iter().collect();
    let world = crate::rules::World::of(config, false);
    let mut changed = Vec::new();
    crate::rules::change(config, |rules| {
        changed = crate::rules::from_pairs(&world, rules, &pairs, crate::py::time_time());
    })?;
    let manual = refresh(config, links, households)?;
    tracing::info!(links = pairs.len(), "agent links set (as rules)");
    let mut out = Dict::new();
    out.insert("manual".into(), Value::Object(manual));
    out.insert("changed".into(), json!(changed));
    Ok(out)
}

/// What the rules give, set up: the skills that list whom each agent may write to ([`write_contacts`]). Returns
/// what `apply` passes on as `manual` (nothing any more).
pub fn refresh(config: &Config, _links: &Links, households: Option<&Households>) -> anyhow::Result<Dict> {
    let world = crate::rules::World::of(config, false);
    let rules = crate::rules::load(config);
    write_contacts(config, &crate::rules::effective_links(&world, &rules), households);
    Ok(Dict::new())
}

/// Hermes turns Bot Mode on for an install where a profile is marked (`ui_meta.hermes-bots` in its profile.yaml):
/// then each agent's Bot Chat, its main conversation, gets Hermes' `message_agent` with the install's other profiles
/// as its roster, whatever the person allowed (lab of 2026-11-11: Prof, linked to nobody, had it). No profile this
/// bridge manages keeps the mark.
fn unmark_bots(config: &Config) {
    for agent in config.agents.values() {
        let Some(folder) = crate::shares::profile_dir(config, agent).filter(|f| f.is_dir()) else { continue };
        let done = crate::profile_files::edit(&folder.join("profile.yaml"), |data| {
            if let Some(meta) = data.get_mut("ui_meta").filter(|m| m.is_map()) {
                meta.remove("hermes-bots");
            }
        });
        if let Err(exc) = done {
            tracing::warn!(agent = %agent.name, error = %exc, "Bot Mode mark not taken off");
        }
    }
}

/// `key`'s lines taken out of a gateway's `.env`, the others kept as they are: written only when there is one, through
/// a temporary file renamed over it with its mode, owner and group (profile_files.rs `write_atomic_keeping_owner`:
/// when the bridge may not keep them, the file stays as it is and the error is logged). Each start does this for every
/// gateway: until review 2 of 21/11 the file was written again in place each time, and a full disk would have left the
/// hub's `.env` (its model keys) empty. Whether it was written.
fn drop_env_line(path: &Path, key: &str) -> std::io::Result<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let ours = |line: &str| line.split('=').next().unwrap_or("").trim() == key;
    if !text.split_inclusive('\n').any(ours) {
        return Ok(false);
    }
    let kept: String = text.split_inclusive('\n').filter(|line| !ours(line)).collect();
    crate::profile_files::write_atomic_keeping_owner(path, kept.as_bytes())?;
    Ok(true)
}

/// At start: Hermes' Bot Mode off everywhere (21/11, Alex: another household's agents through the bridge only). A
/// place set up before loses every profile's mark, the bridge as its gateways' `bot_peers.bip` and their key, and the
/// tokens peers.json gave them (its links of before stay as they were).
pub fn drop_bot_mode(config: &Config, links: &Links) {
    unmark_bots(config);
    let mut data = links.load();
    let tokens: Vec<String> =
        data.get("tokens").and_then(Value::as_object).map(|t| t.keys().cloned().collect()).unwrap_or_default();
    let mut gateways: BTreeSet<PathBuf> = config.agents.values().filter_map(|a| managed_home(config, a)).collect();
    gateways.extend(tokens.iter().filter(|g| !g.starts_with("manual:")).map(PathBuf::from));
    let key_env = format!("HERMES_PEER_{}_KEY", PEER_NAME.to_uppercase());
    for gateway in gateways.iter().filter(|g| g.is_dir()) {
        if let Err(exc) = drop_env_line(&gateway.join(".env"), &key_env) {
            tracing::warn!(gateway = %gateway.display(), error = %exc, "Bot Mode key not taken out");
        }
        let peers_file = gateway.join("config.yaml");
        if peers_file.exists() {
            let removed = crate::profile_files::edit(&peers_file, |yaml| {
                if let Some(peers) = yaml.get_mut("bot_peers").filter(|p| p.is_map()) {
                    peers.remove(PEER_NAME);
                }
            });
            if let Err(exc) = removed {
                tracing::warn!(gateway = %gateway.display(), error = %exc, "Bot Mode peer not taken out");
            }
        }
    }
    if !tokens.is_empty() {
        data.insert("tokens".into(), json!({}));
        match links.save(&data) {
            Ok(()) => tracing::info!(tokens = tokens.len(), "Bot Mode's gateway tokens taken out (agents write through the bridge)"),
            Err(exc) => tracing::warn!(error = %exc, "Bot Mode's gateway tokens not taken out"),
        }
    }
}

const CONTACTS: &str = "---
name: agent-contacts
description: \"The other agents you may write to with message_agent or give files to with send_files_to_agent, and how to address them.\"
version: 1.3.0
author: BipAgents
license: MIT
---

# Agents you may write to

Use `message_agent` with one of these targets (the person allowed exactly these; any other is refused):

{lines}

Each one answers in the same call, another household's agent too (through your person's bridge and theirs). A
message an agent of another household writes to you comes signed with its household, for example « Message from
Vie (Alex) (@vie__alex) », in your own conversation with it: just answer there, in text.

Another agent never sees your files, your shared folder /partage included, nor a path of yours you write to it. To
give files to one of your person's agents, use `send_files_to_agent` (it says the path that agent reads them at),
then tell it with `message_agent`, giving those paths. Another household's agents get no files.
";

/// The family's agent's (family_agent.rs, Alex 21/11): its creator's agents it may ask, nothing else (no files, no
/// other household).
const FAMILY_CONTACTS: &str = "---
name: agent-contacts
description: \"The agents you may ask with message_agent, and how to address them.\"
version: 1.2.0
author: BipAgents
license: MIT
---

# Agents you may ask

The member of the family who made you lets you write to these agents of theirs with `message_agent` (exactly these;
any other is refused):

{lines}

They answer in the same call. Say for whom in the family you ask, then answer here with what they said:
every member of the family reads it. Another household's agents are not reached from here, and you give no files to
these agents.
";

pub fn contacts_skill_path(home: &Path, agent: &str) -> PathBuf {
    home.join("profiles").join(agent).join("skills").join("agent-contacts").join("SKILL.md")
}

/// Each agent this bridge manages gets the list of agents it may write to (a skill in its profile).
pub fn write_contacts(config: &Config, links: &[(String, String)], households: Option<&Households>) {
    let names: BTreeMap<String, String> = households
        .map(|h| {
            h.public()
                .iter()
                .map(|x| {
                    (
                        x.get("id").map(crate::py::py_str).unwrap_or_default(),
                        x.get("name").map(crate::py::py_str).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for agent in config.agents.values() {
        let Some(home) = managed_home(config, agent) else { continue };
        if !home.join("profiles").join(&agent.name).is_dir() {
            continue; // no profile (yet): nothing to write into, and never a folder made for it
        }
        let skill = contacts_skill_path(&home, &agent.name);
        if crate::profile_files::hands_off(&skill) {
            continue; // the person's own Hermes (profile_files::hands_off)
        }
        // only those it can reach (Alex 21/11: the family's agent was told it could write to Vie, without the tool):
        // this place's agents that are still here; another household's only through Bot Mode, never the family's
        let family = crate::family_agent::is_family(config, &agent.name);
        let mut targets: Vec<&String> = links
            .iter()
            .filter(|(s, t)| {
                *s == agent.name
                    && match split(t) {
                        Some(_) => !family,
                        None => config.agent(Some(t)).is_some(),
                    }
            })
            .map(|(_, t)| t)
            .collect();
        targets.sort();
        if targets.is_empty() {
            if let Some(folder) = skill.parent().filter(|f| f.exists()) {
                let _ = std::fs::remove_dir_all(folder);
            }
            continue;
        }
        let mut lines = Vec::new();
        for target in targets {
            match split(target) {
                Some((other, household)) => {
                    let household_name = names.get(&household).cloned().unwrap_or(household);
                    lines.push(format!("- `bip/{target}`: {other}, an agent of {household_name}'s household"));
                }
                None => {
                    let shown =
                        config.agent(Some(target)).map(|a| a.display_name.clone()).unwrap_or_else(|| target.clone());
                    lines.push(format!("- `bip/{target}`: {shown}"));
                }
            }
        }
        let written = skill
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| {
                let text = if family { FAMILY_CONTACTS } else { CONTACTS };
                std::fs::write(&skill, text.replace("{lines}", &lines.join("\n")))
            });
        if written.is_err() {
            tracing::warn!(agent = %agent.name, "contacts not written");
        }
    }
}


/// A household message's turn is taken as going on for this long at most (agent_messages::ANSWER_SECONDS).
pub const HOUSEHOLD_TURN_SECONDS: f64 = 600.0;
/// Household messages' turns going on: (id, agent, since).
static HOUSEHOLD_TURNS: Mutex<Vec<(u64, String, f64)>> = Mutex::new(Vec::new());
static HOUSEHOLD_TURN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn household_turn_began(agent: &str) -> u64 {
    let id = HOUSEHOLD_TURN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut turns = HOUSEHOLD_TURNS.lock().unwrap_or_else(|e| e.into_inner());
    turns.push((id, agent.to_string(), crate::py::time_time()));
    id
}

pub(crate) fn household_turn_ended(id: u64) {
    HOUSEHOLD_TURNS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(t, _, _)| *t != id);
}

/// Whether another household's agent has a turn of `agent` going on (a session chat until it answers, a run for
/// `HOUSEHOLD_TURN_SECONDS`: the bridge doesn't follow its end).
pub fn household_turn(agent: &str) -> bool {
    let at = crate::py::time_time();
    let mut turns = HOUSEHOLD_TURNS.lock().unwrap_or_else(|e| e.into_inner());
    turns.retain(|(_, _, since)| at - since < HOUSEHOLD_TURN_SECONDS);
    turns.iter().any(|(_, a, _)| a == agent)
}

// -- between households -----------------------------------------------------------------------------------------------

/// The caller's token (`authorization` without its « Bearer » prefix).
fn bearer(headers: &HeaderMap) -> String {
    let header =
        headers.get("authorization").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    header.strip_prefix("Bearer ").unwrap_or(&header).trim().to_string()
}

/// The household whose key this request carries (401 otherwise).
pub(crate) fn household_caller(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let token = bearer(headers);
    let household = if token.is_empty() { None } else { state.households.caller(&token) };
    household.ok_or_else(|| HttpError::new(401, "unknown household"))
}

/// Starlette's `await request.json()`: invalid JSON is an uncaught error (500). Small bodies only (64 KiB): some of
/// these routes are open (`/family/accept`).
async fn request_json(request: Request) -> ApiResult<Value> {
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024).await.map_err(|e| {
        if crate::body_limit::is_too_large(&e) {
            HttpError::new(413, "request body too large")
        } else {
            HttpError::new(400, "There was an error parsing the body")
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|e| HttpError::from(anyhow::anyhow!("JSONDecodeError: {e}")))
}

/// `body.get(key, "")` with Python's `str()`; a body that is not a dict is an uncaught error (500).
fn body_str(body: &Value, key: &str) -> ApiResult<String> {
    match body {
        Value::Object(map) => Ok(map.get(key).map(crate::py::py_str).unwrap_or_default()),
        _ => Err(HttpError::from(anyhow::anyhow!("AttributeError: object has no attribute 'get'"))),
    }
}

/// A household linked or unlinked: the agents get the family's tools, or lose them (agent_tools.rs `tools_for`).
fn family_tools_changed(state: &AppState) {
    let config = state.config();
    if config.hermes.enabled() {
        state.spawn(crate::agent_tools::register_all(state.config.clone(), crate::agents_admin::default_run()));
    }
}

pub(crate) async fn family_accept(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    // an invitation's exchange that came sealed with its code (family_rendezvous.rs), through Kerfy Studio's relay or
    // straight to our address: a mark only a request run in-process carries
    let introduced = request.extensions().get::<crate::family_rendezvous::Introduced>().cloned();
    let through = introduced.as_ref().and_then(|i| i.relay.clone());
    // a household already linked that links again says so with its current key (it then keeps its place)
    let current = Some(bearer(request.headers())).filter(|t| !t.is_empty());
    let body = request_json(request).await?;
    let (code, name, url, key) =
        (body_str(&body, "code")?, body_str(&body, "name")?, body_str(&body, "url")?, body_str(&body, "key")?);
    // the certificate their address serves when it is a door (sent over ours, checked, with our one-time code)
    let fingerprint = body_str(&body, "fingerprint")?;
    let fingerprint = Some(fingerprint.as_str()).filter(|f| !f.is_empty());
    // the link's own key (family_seal.rs): their half and ours, with the invitation's code
    let their_half = body_str(&body, "dh")?;
    let mut link = None;
    if !their_half.is_empty() {
        if let Ok(handshake) = crate::family_seal::Handshake::new() {
            let ours = handshake.public();
            link = handshake.link_key(&their_half, &code, true).map(|key| (key, ours));
        }
    }
    if introduced.is_some() && link.is_none() {
        return Err(HttpError::new(400, "a sealed introduction needs the link's key"));
    }
    // through the relay: our own mailbox and a pass to it, for their mailbox only (only the households we link with
    // may call it, and only while we accept that pass)
    let mut ours_at_relay = None;
    if through.is_some() {
        let their_box = body_str(&body, "mailbox")?;
        if !crate::family_rendezvous::valid_mailbox(&their_box) {
            return Err(HttpError::new(400, "an introduction through the relay says its mailbox"));
        }
        let own = crate::family_rendezvous::mailbox(&state).await;
        let pass = crate::family_rendezvous::new_pass(&state, &their_box).await;
        match (own, pass) {
            (Ok((own, _)), Ok(pass)) => ours_at_relay = Some((own, pass)),
            _ => return Err(HttpError::new(503, "Kerfy Studio's relay can't be reached right now")),
        }
    }
    match state.households.accept(&code, &name, &url, &key, fingerprint, current.as_deref()) {
        Ok(mut answer) => {
            let household = crate::households::slug(&name);
            let config = state.config();
            if let Some((sealed, ours)) = &link {
                state.households.mark(&household, "seal", json!(crate::family_seal::encode_key(sealed)))?;
                state.households.mark(&household, "role", json!("inviter"))?;
                let their_box = body_str(&body, "mailbox")?;
                if crate::family_rendezvous::valid_mailbox(&their_box) {
                    state.households.mark(&household, "mailbox", json!(their_box))?;
                }
                answer.insert("dh".into(), json!(ours));
            }
            if let Some((own, (pass, id, expires))) = &ours_at_relay {
                answer.insert("mailbox".into(), json!(own));
                answer.insert("pass".into(), json!(pass));
                answer.insert("pass_expires".into(), json!(expires));
                // the pass we gave them, accepted until the link is removed (their pass for our calls comes next:
                // `POST /family/relay-pass`, once they know our mailbox)
                state.households.mark(&household, "given_pass", json!(id))?;
                state.households.mark(&household, "given_pass_at", json!(crate::py::time_time()))?;
                crate::family_rendezvous::announce_passes(&state).await;
            }
            if through.is_some() {
                // they reached us through Kerfy Studio's relay: who reaches whom from now on
                let ours_private = own_url(&config).is_some_and(|u| crate::households::private_address(&u));
                if ours_private && !crate::households::private_address(&url) {
                    // a place scanned our invitation (we are at home): we fetch its calls for us from it, as when we
                    // scan its invitation (family_relay.rs), and the relay is not needed any more
                    state.households.mark(&household, "pull", json!(true))?;
                    let (later, id) = (state.clone(), household.clone());
                    state.spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await; // once it keeps them
                        crate::family_relay::start_pulling(&later, &id);
                    });
                    answer.insert("reach_you".into(), json!("pull"));
                } else {
                    // they call us through the relay (we keep a long poll on our mailbox); we call them so too when
                    // their address can't be reached either
                    state.households.mark(&household, "relay_in", json!(true))?;
                    if crate::households::private_address(&url) {
                        state.households.mark(&household, "via", json!("relay"))?;
                        state.households.mark(&household, "route", json!(crate::family_relay::new_token()))?;
                        answer.insert("reach_you".into(), json!("relay"));
                    } else {
                        answer.insert("reach_you".into(), json!("direct"));
                    }
                    crate::family_rendezvous::wake();
                }
            } else if body.get("pull").and_then(Value::as_bool) == Some(true) {
                // their address can't be reached from here (a home): their bridge will fetch our calls (family_relay.rs)
                if state.households.mark(&household, "relay", json!(crate::family_relay::new_token()))? {
                    answer.insert("relay".into(), json!(true));
                }
            }
            family_tools_changed(&state);
            if let Some(own) = crate::door_tls::door_fingerprint(&config) {
                answer.insert("fingerprint".into(), json!(own));
            }
            Ok(web::ok(Value::Object(answer)))
        }
        Err(HouseholdError(message)) if message == crate::households::NAME_TAKEN => Err(HttpError::new(409, message)),
        Err(HouseholdError(message)) => Err(HttpError::new(403, message)),
    }
}

/// `POST /family/relay-pass {pass, expires}` (a linked household's key): the pass it gave us to call its mailbox
/// through the relay (after a link made through it, once it knew our mailbox: family_rendezvous.rs).
async fn family_relay_pass(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let household = household_caller(&state, request.headers())?;
    let body = request_json(request).await?;
    let pass = body_str(&body, "pass")?;
    if !pass.starts_with("fp1.") {
        return Err(HttpError::new(400, "not a pass"));
    }
    state.households.mark(&household, "pass", json!(pass))?;
    let expires = body.get("expires").and_then(Value::as_f64).unwrap_or(0.0);
    state.households.mark(&household, "pass_expires", json!(expires))?;
    Ok(web::no_content())
}

/// `POST /family/accept/sealed`: an invitation's exchange sealed with its code, straight to our address (the scanning
/// bridge's first try, before the relay): answered sealed, or 404 when it doesn't open (family_rendezvous.rs).
async fn family_accept_sealed(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let wire = axum::body::to_bytes(request.into_body(), crate::family_rendezvous::INVITE_BYTES)
        .await
        .map_err(|_| HttpError::new(413, "request body too large"))?;
    Ok(crate::family_rendezvous::accept_sealed(&state, wire).await)
}

/// `POST /family/door {fingerprint}`: a linked household's bridge tells the certificate its address serves (links
/// made before invitations carried it, a certificate put back after a restore). Its key says who it is.
async fn family_door(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let household = household_caller(&state, request.headers())?;
    let body = request_json(request).await?;
    let (fingerprint, url) = (body_str(&body, "fingerprint")?, body_str(&body, "url")?);
    let url = Some(url.as_str()).filter(|u| !u.is_empty());
    match state.households.set_fingerprint(&household, &fingerprint, url) {
        Ok(changed) => {
            if changed {
                tracing::info!(household = %household, "household door certificate kept");
            }
            Ok(web::ok(json!({"ok": true})))
        }
        Err(e) if e.downcast_ref::<HouseholdError>().is_some() => {
            Err(HttpError::new(409, e.downcast_ref::<HouseholdError>().map(|h| h.0.clone()).unwrap_or_default()))
        }
        Err(_) => Err(HttpError::new(401, "unknown household")),
    }
}

async fn family_agents(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let household = household_caller(&state, &headers)?;
    let config = state.config();
    // every agent (the pairing screen lists those not paired yet); `told_by`: that household's agents whose family
    // news it reads (the rules, family_news::told_by), so their tell_family names it only along them
    let (world, rules) = (crate::rules::World::of(&config, false), crate::rules::load(&config));
    let agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| {
            let told_by = crate::family_news::told_by(&world, &rules, &household, &a.name);
            json!({"id": a.name, "name": a.display_name, "told_by": told_by})
        })
        .collect();
    Ok(web::ok(json!({"name": state.households.name(), "agents": agents})))
}

/// What a linked household's bridge may call (the key each was given): introduce itself with an invitation code,
/// list our agents, its relay pass and its door (its agents' messages: household_asks.rs). Mounted at the root.
pub fn family_router() -> Router<AppState> {
    Router::new()
        .route("/family/accept", post(family_accept))
        .route("/family/accept/sealed", post(family_accept_sealed))
        .route("/family/relay-pass", post(family_relay_pass))
        .route("/family/agents", get(family_agents))
        .route("/family/door", post(family_door))
}

// -- the app's side -----------------------------------------------------------------------------------------------------

async fn family(State(state): State<AppState>) -> Response {
    let config = state.config();
    web::ok(json!({"name": state.households.name(), "url": own_url(&config), "households": state.households.public()}))
}

async fn set_name(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let name = body_str(&body, "name")?;
    if let Err(exc) = state.households.set_name(&name) {
        return match exc.downcast_ref::<HouseholdError>() {
            Some(e) => Err(HttpError::new(400, e.0.clone())),
            None => Err(exc.into()),
        };
    }
    Ok(web::ok(json!({"name": state.households.name()})))
}

/// `{days?}`: an invitation sent through the person's account (« Inviter par son compte ») waits longer than a QR
/// code shown in the same room (ten minutes): up to a week.
async fn invite(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body"))?;
    let days = serde_json::from_slice::<Value>(&bytes).ok().and_then(|b| b.get("days").and_then(Value::as_f64));
    let seconds = days.map(|d| d.clamp(0.0, 7.0) * 86400.0).filter(|s| *s > 0.0);
    let config = state.config();
    let Some(url) = own_url(&config).filter(|u| !u.is_empty()) else {
        return Err(HttpError::new(409, "this bridge has no address on the tailnet (public_url)"));
    };
    let code = match state.households.invite_for(seconds.unwrap_or(crate::households::INVITE_SECONDS)) {
        Ok(code) => code,
        Err(exc) => {
            return match exc.downcast_ref::<HouseholdError>() {
                Some(e) => Err(HttpError::new(409, e.0.clone())),
                None => Err(exc.into()),
            }
        }
    };
    let (url, fingerprint) = family_address(&config, url);
    let mut invitation = json!({"v": 1, "kind": "family", "url": url, "code": code, "name": state.households.name()});
    // our address is a door with a self-signed certificate: the other bridge pins it, as the apps do
    if let Some(fingerprint) = fingerprint {
        invitation["fingerprint"] = json!(fingerprint);
    }
    // the invitation's own mailbox at Kerfy Studio's relay (family_rendezvous.rs), ending with it: a household that
    // can't reach our address (another home, a place scanning ours) introduces itself there; the QR code never
    // carries our household's own mailbox
    // this version's invitation: its exchange always sealed with its code, relay or not (the code says so too:
    // households.rs `SEALED`, what every app passes on as it is)
    invitation["sealed"] = json!(1);
    let until = crate::py::time_time() + seconds.unwrap_or(crate::households::INVITE_SECONDS);
    if crate::family_rendezvous::base(&config).is_none() {
        let _ = crate::family_rendezvous::remember_invite(&config, &code, until, ("", ""));
    } else {
        match crate::family_rendezvous::invite_mailbox(&state, until).await {
            Ok((mailbox, token)) => match crate::family_rendezvous::remember_invite(&config, &code, until, (&mailbox, &token)) {
                Ok(()) => {
                    invitation["relay"] = json!(mailbox);
                    crate::family_rendezvous::wake(); // its poll now, for its answer
                }
                Err(e) => tracing::warn!(error = %e, "family invitation: its relay key not kept"),
            },
            Err(e) => {
                tracing::info!(error = %e, "family invitation without the relay (it can't be reached)");
                // still answered sealed at our address
                let _ = crate::family_rendezvous::remember_invite(&config, &code, until, ("", ""));
            }
        }
    }
    Ok(web::ok(invitation))
}

/// Where another household's bridge reaches this one, and the certificate to pin there. A place of the offer with
/// its own name (`<label>.p.bipagents.com`, place_cert.rs) is reached by that name on 443, with its publicly trusted
/// certificate: a home network that lets out only 443 (Alex's, 30/10) never reaches the IP door on 444+.
fn family_address(config: &Config, public: String) -> (String, Option<String>) {
    let now = crate::py::time_time() as i64;
    if let Some(held) = crate::place_cert::held_for(config).filter(|h| config.cloud && h.expires > now) {
        return (format!("https://{}", held.host), None);
    }
    (public, crate::door_tls::door_fingerprint(config))
}

/// The other household's invitation, scanned in the app: this bridge introduces itself to theirs, at their address,
/// or through Kerfy Studio's relay when nothing answers there as them (family_rendezvous.rs: two homes, a place
/// scanning a home's invitation).
async fn join(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let url = body_str(&body, "url")?.trim_end_matches('/').to_string();
    let code = body_str(&body, "code")?;
    let config = state.config();
    let mine = own_url(&config).filter(|u| !u.is_empty());
    let (Some(mine), false) = (mine, state.households.name().is_empty()) else {
        return Err(HttpError::new(409, "name this household and give the bridge a tailnet address first"));
    };
    // the inviter's certificate when its address is a door (in its invitation, as the apps get it at pairing)
    let given = body_str(&body, "fingerprint")?;
    let fingerprint = match given.trim() {
        "" => None,
        given => Some(
            crate::door_tls::normalize(given).ok_or_else(|| HttpError::new(400, "this invitation is damaged"))?,
        ),
    };
    if fingerprint.is_none() && crate::door_tls::door_address(&url) {
        // a door's address without its certificate: an older app, or an invitation from an older bridge
        return Err(HttpError::new(409, "this invitation is too old: ask for a new one (and update the app)"));
    }
    // the invitation's own mailbox at Kerfy Studio's relay, when it carries one (a bridge of this version)
    let relay_box =
        Some(body_str(&body, "relay")?.trim().to_string()).filter(|m| crate::family_rendezvous::valid_mailbox(m));
    let handshake = crate::family_seal::Handshake::new().ok();
    let key = crate::py::token_urlsafe(32);
    let (mine, own) = family_address(&config, mine);
    let mut payload = json!({"code": code, "name": state.households.name(), "url": mine, "key": key});
    if let Some(own) = own {
        payload["fingerprint"] = json!(own);
    }
    // our half of the link's own key (family_seal.rs)
    if let Some(handshake) = &handshake {
        payload["dh"] = json!(handshake.public());
    }
    // our address can't be reached from the internet (a home): theirs keeps its calls for us (family_relay.rs)
    let pull = crate::households::private_address(&mine);
    // theirs can't be reached from here: a home's invitation scanned on a place of the offer, from an older bridge
    // (no relay in it: the apps say who should scan whose invitation)
    let out_of_reach = || {
        HttpError::json(
            502,
            json!({"code": "their_side_unreachable", "private": crate::households::private_address(&url),
                   "message": "their side does not answer right now: try again later"}),
        )
    };
    let relay_down = || {
        HttpError::json(
            502,
            json!({"code": "relay_unreachable",
                   "message": "Kerfy Studio's relay can't be reached right now: try again in a moment"}),
        )
    };
    // an invitation of this version (its code says so, or it carries a relay mailbox): never in clear, relay or not
    let sealed = handshake.is_some() && (relay_box.is_some() || code.starts_with(crate::households::SEALED));
    let by_relay = sealed && crate::family_rendezvous::base(&config).is_some();
    // already linked to the household at this address: our current key lets theirs know we are linking again
    let current = state
        .households
        .public()
        .into_iter()
        .find(|h| h.get("url").and_then(Value::as_str) == Some(url.as_str()))
        .and_then(|h| state.households.get(&crate::py::py_str(&h["id"])))
        .and_then(|e| e.get("key_out").and_then(Value::as_str).map(str::to_string));
    // their address first (a place never reaches a home network or a tailnet). An invitation of this version is
    // answered there only sealed with its code: what answers at that address on our network must prove it holds the
    // invitation, and never learns the code. Anything else there, and we go through the relay.
    let mut answered: Option<(u16, Bytes)> = None;
    if !(config.cloud && crate::households::private_address(&url)) {
        let mut direct = payload.clone();
        if pull {
            direct["pull"] = json!(true);
        }
        let wait = if by_relay && crate::households::private_address(&url) { 5 } else { 15 };
        let client = crate::door_tls::client_for(&state.http, fingerprint.as_deref());
        if sealed {
            let (wire, salt) = crate::family_rendezvous::sealed_accept(&code, &direct, current.as_deref());
            let sent = client
                .post(format!("{url}/family/accept/sealed"))
                .timeout(std::time::Duration::from_secs(wait))
                .header("content-type", "application/octet-stream")
                .body(wire)
                .send()
                .await;
            match sent {
                Ok(resp) if resp.status().as_u16() == 200 => {
                    answered = crate::family_rendezvous::opened_accept(resp, &code, &salt).await;
                    if answered.is_none() {
                        tracing::warn!("family link: something at their address answered, not as them: the relay");
                    }
                }
                _ => tracing::info!("family link: their address does not answer as them, through the relay"),
            }
            if answered.is_none() && !by_relay {
                return Err(out_of_reach());
            }
        } else {
            // an invitation from an older bridge: its address, as before
            let mut call =
                client.post(format!("{url}/family/accept")).timeout(std::time::Duration::from_secs(wait)).json(&direct);
            if let Some(current) = &current {
                call = call.bearer_auth(current);
            }
            let resp = call.send().await.map_err(|_| out_of_reach())?;
            let status = resp.status().as_u16();
            answered = Some((status, resp.bytes().await.map_err(|_| out_of_reach())?));
        }
    } else if !by_relay {
        return Err(out_of_reach());
    }
    let through = answered.is_none();
    let (status, bytes) = match answered {
        Some(direct) => direct,
        None => {
            // through Kerfy Studio's relay: our own mailbox (they give a pass for it; ours for theirs follows)
            let (own_box, _) = crate::family_rendezvous::mailbox(&state).await.map_err(|_| relay_down())?;
            payload["mailbox"] = json!(own_box);
            let invitation = relay_box.as_deref().unwrap_or_default();
            crate::family_rendezvous::accept(&state, invitation, &code, &payload, current.as_deref()).await.map_err(
                |failure| match failure {
                    crate::family_rendezvous::CallFailure::RelayDown => relay_down(),
                    // their bridge is off, or no longer has this invitation (used, or ended)
                    _ => HttpError::json(
                        502,
                        json!({"code": "their_side_unreachable", "private": false,
                               "message": "their side does not answer right now: try again later"}),
                    ),
                },
            )?
        }
    };
    if status == 409 {
        return Err(HttpError::new(409, "their bridge is already linked to a household with this name"));
    }
    if status != 200 {
        return Err(HttpError::new(403, "the invitation was refused (expired?)"));
    }
    let answer: Value =
        serde_json::from_slice(&bytes).map_err(|e| HttpError::from(anyhow::anyhow!("JSONDecodeError: {e}")))?;
    let name = body_str(&answer, "name")?;
    let key_in = body_str(&answer, "key")?;
    let household = match state.households.joined(&name, &url, &key, &key_in, fingerprint.as_deref()) {
        Ok(h) => h,
        Err(exc) => {
            return match exc.downcast_ref::<HouseholdError>() {
                Some(e) => Err(HttpError::new(409, e.0.clone())),
                None => Err(exc.into()),
            }
        }
    };
    // the link's own key, both halves crossed (an older bridge sends none)
    if let (Some(handshake), Some(theirs)) = (handshake, answer.get("dh").and_then(Value::as_str)) {
        if let Some(sealed) = handshake.link_key(theirs, &code, false) {
            state.households.mark(&household, "seal", json!(crate::family_seal::encode_key(&sealed)))?;
            state.households.mark(&household, "role", json!("joiner"))?;
            // their household's own mailbox (never the invitation's) and the pass they gave us for it
            let their_box =
                answer.get("mailbox").and_then(Value::as_str).filter(|m| crate::family_rendezvous::valid_mailbox(m));
            if let Some(their_box) = their_box {
                state.households.mark(&household, "mailbox", json!(their_box))?;
            }
            if let Some(pass) = answer.get("pass").and_then(Value::as_str).filter(|p| !p.is_empty()) {
                state.households.mark(&household, "pass", json!(pass))?;
                let expires = answer.get("pass_expires").and_then(Value::as_f64).unwrap_or(0.0);
                state.households.mark(&household, "pass_expires", json!(expires))?;
            }
        }
    }
    if through {
        match answer.get("reach_you").and_then(Value::as_str) {
            // they are at home, we are a place: they fetch our calls for them from us (family_relay.rs)
            Some("pull") => {
                state.households.mark(&household, "relay", json!(crate::family_relay::new_token()))?;
            }
            // we call them through the relay; they call us so too (`relay`), or at our address (`direct`)
            reach => {
                state.households.mark(&household, "via", json!("relay"))?;
                state.households.mark(&household, "route", json!(crate::family_relay::new_token()))?;
                if reach == Some("relay") {
                    // they call us through it from now on, with a pass we give them for their mailbox only, sent
                    // through the link itself; made before our poll is turned on (`give_pass`)
                    give_pass(&state, &household).await?;
                }
            }
        }
    } else if pull && answer.get("relay").and_then(Value::as_bool) == Some(true) {
        // they keep their calls for us: we fetch them (an older bridge doesn't answer `relay`)
        state.households.mark(&household, "pull", json!(true))?;
        crate::family_relay::start_pulling(&state, &household);
    }
    family_tools_changed(&state);
    Ok(web::ok(json!({"id": household, "name": answer.get("name").cloned().unwrap_or(Value::Null)})))
}

/// After a link made through the relay where they call us through it too: a pass to our mailbox, for theirs only,
/// accepted from now on, then our mailbox polled (`relay_in`), then the pass given to them through the link (sealed,
/// with their key: `POST /family/relay-pass`). The pass comes before the poll: the relay keeps what a mailbox says it
/// accepts only once it polls, so our first poll must already say it. Turned on first (until 20/11), that poll,
/// numbered before the pass, said « none »; the pass's own statement reaching the relay before it was dropped, and
/// for that whole poll (25 s) the mailbox took no pass: their calls were refused, their new pass after a link
/// removed waited a minute (test_family_relay_first_poll.py).
async fn give_pass(state: &AppState, household: &str) -> anyhow::Result<()> {
    let made = make_pass(state, household, true).await;
    state.households.mark(household, "relay_in", json!(true))?;
    crate::family_rendezvous::wake();
    if !made {
        return Ok(());
    }
    crate::family_rendezvous::announce_passes(state).await;
    if !hand_pass(state, household, 3).await {
        tracing::warn!(household, "family link: our pass not given to them yet (given again later)");
    }
    Ok(())
}

/// A household's pass is made again this often (passes sealed with a key the relay retires keep working).
const PASS_RENEW_SECS: f64 = 30.0 * 86400.0;

/// A pass to our mailbox, for this household's mailbox only, made now at the relay (of a newer generation than any
/// before): kept to be given to it (`pass_to_give`, until it has it) and, when `now`, at once the one we take from it
/// (`given_pass`; otherwise once it has it). False when the relay gave none.
async fn make_pass(state: &AppState, household: &str, now: bool) -> bool {
    let Some(entry) = state.households.get(household) else { return false };
    let Some(their_box) = entry.get("mailbox").and_then(Value::as_str).map(str::to_string) else { return false };
    let (pass, id, expires) = match crate::family_rendezvous::new_pass(state, &their_box).await {
        Ok(made) => made,
        Err(e) => {
            tracing::warn!(error = %e, household, "family link: no pass for them from the relay (made again later)");
            return false;
        }
    };
    let made = json!({"pass": pass, "id": id, "expires": expires});
    let mut fields = vec![("pass_to_give", made)];
    if now {
        fields.extend([
            ("given_pass", json!(id)),
            ("given_pass_at", json!(crate::py::time_time())),
            ("renew_pass", Value::Null),
        ]);
    }
    match state.households.mark_all(household, &fields) {
        Ok(marked) => marked,
        Err(e) => {
            tracing::warn!(error = %e, household, "family link: its new pass not written down (made again later)");
            false
        }
    }
}

/// Gives this household the pass made for it (`pass_to_give`) through the link (sealed, with their key:
/// `POST /family/relay-pass`), up to `tries` times; once it has it, that is the one we take from it. True when it
/// has it (or there was nothing to give).
async fn hand_pass(state: &AppState, household: &str, tries: usize) -> bool {
    let Some(entry) = state.households.get(household) else { return true };
    let Some(made) = entry.get("pass_to_give").and_then(Value::as_object).cloned() else { return true };
    let field = |k: &str| made.get(k).cloned().unwrap_or(Value::Null);
    let url = format!("{}/family/relay-pass", crate::family_relay::reach_url(&entry));
    let key = entry.get("key_out").and_then(Value::as_str).unwrap_or_default().to_string();
    for attempt in 0..tries {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        let sent = crate::door_tls::household_client(&state.http, &entry)
            .post(&url)
            .timeout(std::time::Duration::from_secs(30))
            .bearer_auth(&key)
            .json(&json!({"pass": field("pass"), "expires": field("expires")}))
            .send()
            .await;
        if sent.is_ok_and(|r| r.status().is_success()) {
            let newer = entry.get("given_pass") != made.get("id");
            let fields = [
                ("given_pass", field("id")),
                ("given_pass_at", json!(crate::py::time_time())),
                ("pass_to_give", Value::Null),
            ];
            if let Err(e) = state.households.mark_all(household, &fields) {
                tracing::warn!(error = %e, household, "family link: its pass given, not written down (given again later)");
                return false;
            }
            if newer {
                crate::family_rendezvous::announce_passes(state).await;
            }
            return true;
        }
    }
    false
}

/// The households we gave a pass to call us through the relay.
fn given_passes(state: &AppState) -> Vec<String> {
    let data = state.households.load();
    let Some(households) = data.get("households").and_then(Value::as_object) else { return Vec::new() };
    households
        .iter()
        .filter(|(_, v)| v.get("given_pass").and_then(Value::as_str).is_some())
        .map(|(id, _)| id.clone())
        .collect()
}

/// A link removed whose household could call us through the relay: every household that still can gets a pass of a
/// newer generation, and the relay is told at once to take none older, the one withdrawn included (whatever it
/// remembers: after a restart our first poll says it again). Their new passes go through their links; until they
/// have them their calls wait, never the other way round.
async fn renew_after_unlink(state: &AppState) {
    // marked `renew_pass` with the removal itself (Households::withdraw): not said until their new passes are made
    let kept = given_passes(state);
    for household in &kept {
        make_pass(state, household, true).await;
    }
    crate::family_rendezvous::announce_passes(state).await;
    let later = state.clone();
    state.spawn(async move {
        for household in kept {
            hand_pass(&later, &household, 3).await;
        }
    });
}

/// Every minute (family_rendezvous.rs): the passes still to make (a link removed while the relay was out of
/// reach) or to give (a household that didn't answer), and every 30 days a fresh one for each household that calls us
/// through the relay (given first, then the one taken: a key the relay retires never cuts a link).
pub(crate) async fn renew_passes(state: &AppState) {
    let now = crate::py::time_time();
    let mut made = false;
    for household in given_passes(state) {
        let Some(entry) = state.households.get(&household) else { continue };
        if entry.get("renew_pass").and_then(Value::as_bool) == Some(true) {
            made |= make_pass(state, &household, true).await;
        } else if entry.get("pass_to_give").and_then(Value::as_object).is_none()
            && entry.get("given_pass_at").and_then(Value::as_f64).is_none_or(|at| now - at > PASS_RENEW_SECS)
        {
            make_pass(state, &household, false).await;
        }
    }
    if made {
        crate::family_rendezvous::announce_passes(state).await;
    }
    for household in given_passes(state) {
        hand_pass(state, &household, 1).await;
    }
}

async fn unlink(state: &AppState, household: &str) -> ApiResult<Response> {
    let Some(before) = state.households.get(household) else {
        return Err(HttpError::new(404, "no such household"));
    };
    // the household holding her family Carnet: her kept searches there go while the link still carries the calls
    // (through Kerfy Studio's relay too: review 2 of S6, R2)
    crate::family_carnet::before_unlink(state, household).await;
    // its pass goes with those every pass made from now on is newer than (R7), before anything else
    if let Some(given) = before.get("given_pass").and_then(Value::as_str) {
        if let Err(e) = crate::family_rendezvous::withdraw_pass(&state.config(), given) {
            tracing::warn!(error = %e, household, "family link not removed: the pass withdrawn can't be written down");
            return Err(HttpError::new(500, "the link can't be removed right now (this computer's disk?)"));
        }
    }
    // the link removed and the others marked to renew their passes, in one write (R8)
    let entry = match state.households.withdraw(household) {
        Ok(Some(entry)) => Some(entry),
        Ok(None) => return Err(HttpError::new(404, "no such household")),
        Err(e) => {
            tracing::warn!(error = %e, household, "family link not removed: households.json can't be written");
            return Err(HttpError::new(500, "the link can't be removed right now (this computer's disk?)"));
        }
    };
    // the calls kept for it (a home that fetched them, family_relay.rs) fail at once
    crate::family_relay::forget(household);
    // the pass we gave it, withdrawn at the relay at once: it can't reach our mailbox any more, nor tell we're on
    if entry.as_ref().is_some_and(|e| e.get("given_pass").is_some()) {
        renew_after_unlink(state).await;
    }
    // the family's agent held here: that member gets what is the family's (family_agent.rs)
    crate::family_agent::after_unlink(state, household, entry);
    // its rules go, and what they gave (Bot Mode, the contacts)
    crate::rules::forget_household(&state.config(), household);
    refresh(&state.config(), &state.links, Some(&state.households))?;
    family_tools_changed(state);
    Ok(web::no_content())
}

async fn unlink_route(State(state): State<AppState>, UrlPath(household): UrlPath<String>) -> ApiResult<Response> {
    unlink(&state, &household).await
}

/// Starlette tries the routes in order: `DELETE /v1/family/name` (a path with another method) falls through to
/// `DELETE /v1/family/{household}`. Other methods: 405 with this path's own methods.
async fn or_unlink(state: AppState, request: Request, literal: &str, allow: &'static str) -> Response {
    if request.method() == Method::DELETE {
        return match unlink(&state, literal).await {
            Ok(r) => r,
            Err(e) => axum::response::IntoResponse::into_response(e),
        };
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    response.headers_mut().insert("allow", HeaderValue::from_static(allow));
    response
}

async fn name_fallback(State(state): State<AppState>, request: Request) -> Response {
    or_unlink(state, request, "name", "PUT").await
}

async fn invite_fallback(State(state): State<AppState>, request: Request) -> Response {
    or_unlink(state, request, "invite", "POST").await
}

async fn join_fallback(State(state): State<AppState>, request: Request) -> Response {
    or_unlink(state, request, "join", "POST").await
}

/// For the app: this household's name, its invitations, joining another one, the households linked (mounted under
/// /v1, bridge key).
pub fn household_admin_router() -> Router<AppState> {
    Router::new()
        .route("/family", get(family))
        .route("/family/name", put(set_name).fallback(name_fallback))
        .route("/family/invite", post(invite).fallback(invite_fallback))
        .route("/family/join", post(join).fallback(join_fallback))
        .route("/family/{household}", delete(unlink_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_with_its_own_name_is_reached_by_it_on_443() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.lan.enabled = true;
        config.lan.cert_dir = dir.path().to_string_lossy().into_owned();
        let door = "https://51.75.1.2:444".to_string();
        assert_eq!(family_address(&config, door.clone()).0, door);
        for name in [crate::place_cert::CERT_NAME, crate::place_cert::KEY_NAME] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        let meta = json!({"host": "k3x9q2.p.bipagents.com", "expires": crate::py::time_time() as i64 + 86400});
        std::fs::write(dir.path().join(crate::place_cert::META_NAME), meta.to_string()).unwrap();
        assert_eq!(family_address(&config, door.clone()).0, door); // a self-install keeps its own address
        config.cloud = true;
        assert_eq!(family_address(&config, door), ("https://k3x9q2.p.bipagents.com".to_string(), None));
    }

    #[test]
    fn a_households_turn_closes_the_tools_while_it_goes_on() {
        assert!(!household_turn("cuisine-test"));
        let turn = household_turn_began("cuisine-test");
        assert!(household_turn("cuisine-test") && !household_turn("vie-test"));
        household_turn_ended(turn);
        assert!(!household_turn("cuisine-test"));
    }

    /// Review 2 of 21/11: a gateway's `.env` (the hub's model keys) is written only when Bot Mode's key is in it, never
    /// in place.
    #[test]
    fn a_gateway_env_is_written_only_to_take_the_bot_mode_key_out() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        let key = "HERMES_PEER_BIP_KEY";
        // without the line: untouched (not even written again)
        std::fs::write(&env, "CUSTOM_MODEL_API_KEY=sk-hub\nAPI_SERVER_KEY=k\n").unwrap();
        std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o640)).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options().write(true).open(&env).unwrap().set_modified(old).unwrap();
        assert!(!drop_env_line(&env, key).unwrap());
        assert_eq!(std::fs::metadata(&env).unwrap().modified().unwrap(), old);
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "CUSTOM_MODEL_API_KEY=sk-hub\nAPI_SERVER_KEY=k\n");
        // with it: only that line out, the rest as it was (no newline added at the end), its mode kept, no leftover
        std::fs::write(&env, "CUSTOM_MODEL_API_KEY=sk-hub\nHERMES_PEER_BIP_KEY=old\n# a note\nAPI_SERVER_KEY=k").unwrap();
        let owner = |m: std::fs::Metadata| (std::os::unix::fs::MetadataExt::uid(&m), std::os::unix::fs::MetadataExt::gid(&m));
        let before = owner(std::fs::metadata(&env).unwrap());
        assert!(drop_env_line(&env, key).unwrap());
        assert_eq!(owner(std::fs::metadata(&env).unwrap()), before); // its owner and group too (profile_files.rs test)
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "CUSTOM_MODEL_API_KEY=sk-hub\n# a note\nAPI_SERVER_KEY=k");
        assert_eq!(std::fs::metadata(&env).unwrap().permissions().mode() & 0o777, 0o640);
        let names: Vec<String> =
            std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [".env"]);
        assert!(!drop_env_line(&env, key).unwrap()); // once
        // no file: none made
        assert!(!drop_env_line(&dir.path().join("missing.env"), key).unwrap());
        assert!(!dir.path().join("missing.env").exists());
    }

    #[test]
    fn signatures_are_read() {
        let found = SIGNATURE.captures("Message from Vie (Alex) (@vie__alex): bonjour").unwrap();
        assert_eq!(&found[1], "vie__alex");
        assert!(SIGNATURE.captures("bonjour").is_none());
    }
}
