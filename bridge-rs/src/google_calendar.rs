//! « Google Calendar » (Alex 20/11: the catalogue of apps signed in by account, one service at a time; Gmail, Calendar,
//! Drive first): the person connects their Google account once in the app (« Applis et outils » › Google Calendar) and
//! chooses the agents that may use its calendars. Those agents read the upcoming events of the calendars the person
//! shows in Google Calendar (Alex 21/11: its other calendars too, not only the primary one) and ask to add, change or
//! delete one through the same calendar tools as the phone's (calendar.rs): `calendar_list` lists them beside the
//! phone's and the mailboxes' (« Travail (Google Calendar) », the primary one by its account), `calendar_add` writes
//! to the one it names when the person may write there (the primary one when it names Google itself, or when the
//! agent has no other calendar), `calendar_update` and `calendar_delete` act on its events: ids
//! `google:<calendar>:<event>`, both percent-encoded (a calendar's id holds `@` and `#`), the primary one `primary`.
//!
//! **Every write asks the person first**, on the bridge's card (person_ok.rs): « Oui » (this once), « Toujours
//! autoriser » (adds and changes in the primary calendar, Alex 23/11: in place of « Oui, pour cette conversation »)
//! or « Non »; a deletion is asked each time (audit 1.3, as the phone's), and so is a write in another calendar
//! (shared by someone else, review 2 of 21/11). Never silently: the kind is critical (approval_level.rs), asked at
//! every level; only the person's own « Toujours » for that agent lets its adds and changes in the primary calendar
//! through, with no card from then on (routines and restarts too), each an « auto » line marked « toujours autorisé »;
//! it goes when the agent is taken out of the list or Google Calendar is disconnected. Without it, a turn nobody
//! follows holds the card (held_approvals.rs) and its « Oui » lets that very call through once.
//!
//! The sign-in reuses Google Drive's (drives.rs): the same Google clients (`[drives.google_drive]`: the iOS client and
//! its redirect, Android's Web client from bridge.toml, the guichet or the person's choice), PKCE, the same renewal
//! (`drives::Slot::Calendar`), its own grant and tokens kept in the connections' encrypted store
//! (`Stored.google_calendar`). Scopes `calendar.events` (the events of every calendar) and
//! `calendar.calendarlist.readonly` (which calendars, their names, whether the person may write there), asked
//! together and incrementally (`include_granted_scopes`); a sign-in that leaves one out is refused. No token ever
//! reaches Hermes nor the model: the bridge calls Calendar API v3 itself and gives the agent text. What an event and a
//! calendar's name say is written by whoever made or shared it: listed between calendar.rs's UNTRUSTED markers,
//! defused (archives.rs) so it can't close them. Updates are never sent to the guests (`sendUpdates=none`); a change
//! or a deletion carries the event's `etag` (`If-Match`), so an event changed since the card (made repeating…) is
//! left as it is.
//!
//! Off by default: `calendar.events` is a « sensitive » scope (the list's is not, Google says), and Google must verify
//! the app (a short video) before anyone beyond its test users signs in. `[google_calendar] enabled = true` switches
//! it on for one place (a test user's), with Google Drive's client in `[drives.google_drive]` (drives itself may stay
//! off). Turned off with a calendar still connected: at the next start it is forgotten here and revoked
//! (`ended_when_off`).
//!
//!     [google_calendar]
//!     enabled = true
//!     # api_url = "…"   (tests)
//!
//! App routes (the bridge key; 404 while off). Errors `{"detail": {"code", "message"}}` as drives.rs':
//!
//!     GET    /v1/google-calendar             {enabled, configured, state, account, connected_at, agents, android}
//!     POST   /v1/google-calendar/start       {agents, client: ios|android} → {authorize_url, state, redirect_uri,
//!                                            expires_in} (Android: {state, server_client_id, scopes, expires_in})
//!     POST   /v1/google-calendar/callback    {url} or {state, code} → as listed
//!     PUT    /v1/google-calendar/agents      {agents} → as listed
//!     DELETE /v1/google-calendar             {ok, revoked}
//!
//! Not a Python module.

use std::collections::HashMap;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{Duration, NaiveDate};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::calendar::Moment;
use crate::config::{AgentConfig, Config};
use crate::connections;
use crate::drives::{self, coded, Api, Fault, Link, Provider, Slot};
use crate::tools_knowledge::{self as tk, tool_failure, tool_text};
use crate::web::{self, check_max_items, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

/// The events of every calendar of the account.
pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
/// Which calendars the account has: names, rights, shown or not (Alex 21/11).
pub const LIST_SCOPE: &str = "https://www.googleapis.com/auth/calendar.calendarlist.readonly";
pub const SCOPES: [&str; 2] = [SCOPE, LIST_SCOPE];
const API: &str = "https://www.googleapis.com/calendar/v3";
/// The calendar's name in the agents' tools.
pub const LABEL: &str = "Google Calendar";
/// The primary calendar's id in `calendar_list`; every Google calendar's and event's id starts with the prefix.
pub const CALENDAR_ID: &str = "google:primary";
pub const ID_PREFIX: &str = "google:";
const PRIMARY: &str = "primary";
/// Events one listing reads at most from each calendar (calendar.rs shows 200 in all).
const PAGE: &str = "250";
/// Calendars listed at most, the primary one first: one Google call each per listing, and an agent has no use for
/// dozens (subscribed holidays, sports…). Those beyond are left out.
const MAX_CALENDARS: usize = 20;
/// What a calendar's or event's id may keep as it is in a URL and in the agents' ids (`:` separates them).
const ID_SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'-').remove(b'.');
const AGENTS_MAX: usize = 32;
const FLOW_SECONDS: u64 = 900;

// -- settings ----------------------------------------------------------------------------------------------------------

/// `[google_calendar]` of bridge.toml.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub enabled: bool,
    /// Calendar API's address (tests).
    pub api_url: Option<String>,
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            api_url: table
                .get("api_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(|u| u.trim_end_matches('/').to_string()),
        }
    }
}

fn api_base(config: &Config) -> String {
    config.google_calendar.api_url.clone().unwrap_or_else(|| API.to_string())
}

fn encoded(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, ID_SAFE).to_string()
}

/// `calendar`: `primary` or a calendar's id.
fn events_url(config: &Config, calendar: &str) -> String {
    format!("{}/calendars/{}/events", api_base(config), encoded(calendar))
}

fn event_url(config: &Config, calendar: &str, id: &str) -> String {
    format!("{}/{}", events_url(config, calendar), encoded(id))
}

fn list_url(config: &Config) -> String {
    format!("{}/users/me/calendarList", api_base(config))
}

/// A Google calendar's id in the agents' tools: `google:primary`, `google:<its id, encoded>`.
pub fn calendar_ref(calendar: &str) -> String {
    format!("{ID_PREFIX}{}", encoded(calendar))
}

/// An event's: `google:<calendar>:<event>`.
pub fn event_ref(calendar: &str, event: &str) -> String {
    format!("{}:{}", calendar_ref(calendar), encoded(event))
}

/// The calendar and the event an agent's id names. None: not one of Google's events.
pub fn split_ref(id: &str) -> Option<(String, String)> {
    let (calendar, event) = id.trim().strip_prefix(ID_PREFIX)?.split_once(':')?;
    let decoded = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8().ok().map(|s| s.trim().to_string());
    let (calendar, event) = (decoded(calendar)?, decoded(event)?);
    (!calendar.is_empty() && !event.is_empty()).then_some((calendar, event))
}

// -- what is kept ------------------------------------------------------------------------------------------------------

fn stored(config: &Config) -> Option<Link> {
    connections::store_of(config).read().ok()?.google_calendar
}

/// Whether `agent` gets the calendar tools for Google's: on, connected (or ended: it is told to ask for a new
/// sign-in), chosen. Never the family's agent nor a project agent (shown to others).
pub fn offered(config: &Config, agent: &str) -> bool {
    if !config.google_calendar.enabled
        || crate::family_agent::is_family(config, agent)
        || crate::project_agent::is_project(config, agent)
    {
        return false;
    }
    stored(config).is_some_and(|l| l.agents.iter().any(|a| a == agent))
}

/// The person's view of it (the app's screen).
pub fn view(config: &Config, link: Option<&Link>) -> Value {
    json!({
        "enabled": config.google_calendar.enabled,
        "configured": drives::endpoints(config, Provider::Google).is_some(),
        "state": link.map(|l| l.state.as_str()).unwrap_or("not_connected"),
        "account": link.and_then(|l| l.account.clone()),
        "connected_at": link.map(|l| l.connected_at),
        "agents": link.map(|l| l.agents.clone()).unwrap_or_default(),
        "android": drives::android_way(config),
    })
}

fn off() -> HttpError {
    coded(404, "off", "Google Calendar is off on this bridge")
}

fn not_connected() -> HttpError {
    coded(404, "not_connected", "not connected")
}

/// The agents the person may choose: this place's own, never the family's nor a project's.
fn agents_checked(config: &Config, agents: &[String]) -> ApiResult<Vec<String>> {
    let mut out = Vec::new();
    for name in agents {
        if config.agent(Some(name)).is_none() {
            return Err(coded(422, "agent", &format!("no agent {name:?}")));
        }
        if crate::family_agent::is_family(config, name) || crate::project_agent::is_project(config, name) {
            return Err(coded(
                422,
                "agent",
                &format!("{name:?} is shared with others: it can't use a person's calendar"),
            ));
        }
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    if out.len() > AGENTS_MAX {
        return Err(coded(422, "agent", &format!("at most {AGENTS_MAX} agents")));
    }
    Ok(out)
}

// -- sign-in -----------------------------------------------------------------------------------------------------------

/// Google's sign-in address for the calendar: its two scopes added to what the person gave already.
pub fn authorize_url(ep: &drives::Endpoints, state: &str, verifier: &str) -> String {
    let challenge = connections::pkce_challenge(verifier);
    let scopes = SCOPES.join(" ");
    let params: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", &ep.client.client_id),
        ("redirect_uri", &ep.client.redirect_uri),
        ("scope", &scopes),
        ("state", state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("include_granted_scopes", "true"),
    ];
    url::Url::parse_with_params(&ep.auth, &params).map(String::from).unwrap_or_default()
}

/// Whether Google gave both scopes (its list of those given, by spaces or commas).
fn all_given(scopes: &str) -> bool {
    SCOPES.iter().all(|want| scopes.split([' ', ',']).any(|x| x == *want))
}

/// A sign-in for `agents`. `client` "android": what the phone's AuthorizationClient needs (a code for the Web
/// client), as drives.rs' Google.
pub fn start(config: &Config, agents: &[String], client: &str) -> ApiResult<Value> {
    if !config.google_calendar.enabled {
        return Err(off());
    }
    let ep = drives::endpoints(config, Provider::Google)
        .ok_or_else(|| coded(409, "not_configured", "no Google client on this bridge"))?;
    let agents = agents_checked(config, agents)?;
    let state = py::token_hex(16);
    let started = std::time::Instant::now();
    let flow = |verifier: String, client: &str| drives::Flow {
        provider: Provider::Google,
        verifier,
        agents: agents.clone(),
        client: client.to_string(),
        started,
        slot: Slot::Calendar,
    };
    if client == "android" {
        let web = drives::endpoints_for(config, Provider::Google, "web").ok_or_else(|| {
            match drives::android_way(config) {
                "not_configured" => coded(409, "not_configured", "no Google client on this bridge"),
                _ => {
                    coded(409, "android_choice", "Google on Android: choose how to sign in first (Documents en ligne)")
                }
            }
        })?;
        drives::keep_flow(&state, flow(String::new(), "web"));
        return Ok(json!({"state": state, "server_client_id": web.client.client_id, "scopes": SCOPES,
                         "expires_in": FLOW_SECONDS}));
    }
    let verifier = py::token_hex(32);
    let url = authorize_url(&ep, &state, &verifier);
    drives::keep_flow(&state, flow(verifier, ""));
    Ok(
        json!({"authorize_url": url, "state": state, "redirect_uri": ep.client.redirect_uri, "expires_in": FLOW_SECONDS}),
    )
}

/// The callback (the address the web sign-in came back to, or Android's state and code): the code exchanged, the
/// account named, the agents chosen at start kept (a sign-in again of the same account keeps the ones chosen before).
pub async fn finish(config: &Config, query: HashMap<String, String>) -> ApiResult<Value> {
    if !config.google_calendar.enabled {
        return Err(off());
    }
    if query.contains_key("error") {
        return Err(coded(400, "denied", "the person did not allow it"));
    }
    let flow = query
        .get("state")
        .and_then(|s| drives::take_flow(s, true))
        .ok_or_else(|| coded(400, "unknown_state", "this sign-in is too old: start again"))?;
    let code = query
        .get("code")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| coded(400, "bad_callback", "the sign-in came back without its code"))?;
    // Google says which scopes it gave: the calendar's two must be among them (the person may untick one)
    if query.get("scope").is_some_and(|s| !all_given(s)) {
        return Err(coded(400, "denied", "the calendar was not allowed"));
    }
    let ep = drives::endpoints_for(config, Provider::Google, &flow.client)
        .ok_or_else(|| coded(409, "not_configured", "no Google client on this bridge"))?;
    let mut form = vec![("grant_type", "authorization_code".to_string()), ("code", code.clone())];
    if flow.client.is_empty() {
        form.push(("redirect_uri", ep.client.redirect_uri.clone()));
        form.push(("code_verifier", flow.verifier.clone()));
    } else {
        form.push(("redirect_uri", String::new()));
    }
    let answer = ep.token_call(&drives::http(), form).await.map_err(|(status, why)| {
        tracing::warn!(status, why = %why, "google calendar: code exchange refused");
        coded(502, "exchange_failed", &format!("Google refused the sign-in ({status} {why})"))
    })?;
    let tokens = connections::tokens_of(&answer, None);
    if tokens.scope.as_deref().is_some_and(|s| !all_given(s)) {
        return Err(coded(400, "denied", "the calendar was not allowed"));
    }
    let api = Api::new(ep, tokens.access_token.clone(), None);
    let account = match first_page(config, &api, PRIMARY).await {
        Ok(page) => page.get("summary").and_then(Value::as_str).map(str::to_string),
        Err(_) => connections::account_of(&answer),
    };
    let _guard = connections::LOCK.lock().await;
    let store = connections::store_of(config);
    let mut stored = store.read()?;
    let previous = stored.google_calendar.take();
    let mut agents = previous
        .filter(|l| l.account.is_none() || account.is_none() || l.account == account)
        .map(|l| l.agents)
        .unwrap_or_default();
    for a in &flow.agents {
        if !agents.contains(a) {
            agents.push(a.clone());
        }
    }
    let link = Link {
        tokens,
        account,
        connected_at: py::time_time(),
        state: "connected".into(),
        client: flow.client.clone(),
        folders: Vec::new(),
        agents,
        carnet: false,
    };
    stored.google_calendar = Some(link.clone());
    store.write(&stored)?;
    tracing::info!(agents = link.agents.len(), "google calendar: connected");
    Ok(view(config, Some(&link)))
}

/// The agents chosen now (the whole list). Returns the view and the agents whose tools change.
pub async fn set_agents(config: &Config, agents: &[String]) -> ApiResult<(Value, Vec<String>)> {
    if !config.google_calendar.enabled {
        return Err(off());
    }
    // an agent deleted since the app listed them: left out, never a refusal (as drives.rs)
    let present: Vec<String> = agents.iter().filter(|a| config.agent(Some(a.as_str())).is_some()).cloned().collect();
    let agents = agents_checked(config, &present)?;
    let (shown, changed, removed) = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let link = stored.google_calendar.as_mut().ok_or_else(not_connected)?;
        let before = std::mem::replace(&mut link.agents, agents.clone());
        let removed: Vec<String> = before.iter().filter(|a| !agents.contains(a)).cloned().collect();
        let changed: Vec<String> =
            agents.iter().filter(|a| !before.contains(a)).chain(removed.iter()).cloned().collect();
        let shown = view(config, Some(&*link));
        store.write(&stored)?;
        (shown, changed, removed)
    };
    // their « Toujours » goes with the calendar: chosen again, they are asked again
    revoke_always(config, Some(&removed)).await;
    Ok((shown, changed))
}

/// Forgets it here and revokes the grant at Google, unless Google Drive was signed in with the same client (one
/// grant: revoking would end Drive's too). Works with the switch off too. The agents that had it.
pub async fn disconnect(config: &Config) -> ApiResult<(Value, Vec<String>)> {
    let (link, shared) = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let link = stored.google_calendar.take().ok_or_else(|| {
            if config.google_calendar.enabled {
                not_connected()
            } else {
                off()
            }
        })?;
        store.write(&stored)?;
        let shared = Slot::Calendar.other_google(&stored, &link.client);
        (link, shared)
    };
    // every « Toujours » given for it: a new connection asks again
    revoke_always(config, None).await;
    let ep = drives::endpoints(config, Provider::Google);
    let revoked =
        !shared && drives::revoke(Provider::Google, ep.as_ref().and_then(|e| e.revoke.as_deref()), &link.tokens).await;
    tracing::info!(revoked, shared, "google calendar: disconnected");
    Ok((json!({"ok": true, "revoked": revoked}), link.agents))
}

/// The « Toujours » of `agents` (None: every agent) for the calendar's adds and changes taken back, their
/// personalities rendered again (approval_level.rs `always_rule`).
async fn revoke_always(config: &Config, agents: Option<&[String]>) {
    for agent in crate::approval_level::revoke_always_of(config, crate::person_ok::CALENDAR_WRITE, agents) {
        crate::approval_level::granted_now(config, &agent).await;
    }
}

/// The switch turned off with the calendar still connected: forgotten and revoked at the bridge's start.
pub async fn ended_when_off(config: &Config) -> bool {
    if config.google_calendar.enabled || stored(config).is_none() {
        return false;
    }
    disconnect(config).await.is_ok()
}

/// [`ended_when_off`] at the bridge's start.
pub async fn end_when_off(state: AppState) {
    ended_when_off(&state.config()).await;
}

/// An agent deleted (agents_admin.rs): no longer among the chosen ones.
pub async fn forget_agent(config: &Config, agent: &str) {
    let _guard = connections::LOCK.lock().await;
    let store = connections::store_of(config);
    let Ok(mut stored) = store.read() else { return };
    let Some(link) = stored.google_calendar.as_mut() else { return };
    let before = link.agents.len();
    link.agents.retain(|a| a != agent);
    crate::approval_level::revoke_always_of(config, crate::person_ok::CALENDAR_WRITE, Some(&[agent.to_string()]));
    if link.agents.len() != before {
        if let Err(e) = store.write(&stored) {
            tracing::warn!(error = %e, "google calendar: a deleted agent can't be taken out");
        }
    }
}

// -- Calendar API ------------------------------------------------------------------------------------------------------

/// What goes wrong, to the agent.
pub fn words(fault: &Fault) -> String {
    match fault {
        Fault::Expired => format!(
            "the connection to {LABEL} ended: your person connects it again in the app (Applis et outils › {LABEL})"
        ),
        Fault::NotFound => "no such event any more (moved or deleted)".to_string(),
        Fault::RateLimited(Some(s)) => format!("{LABEL} asks to slow down: try again in {s} seconds"),
        Fault::RateLimited(None) => format!("{LABEL} asks to slow down: try again in a minute"),
        Fault::Unavailable => format!("{LABEL} has trouble right now: try again later"),
        Fault::Refused => format!("{LABEL} refuses this one"),
        Fault::Unreachable => format!("{LABEL} can't be reached right now"),
        Fault::Changed => CHANGED.to_string(),
        Fault::TooLarge => "too large".to_string(),
        Fault::Other(why) => why.clone(),
    }
}

/// The event changed at Google between its check and the person's « Oui » (412 to the `If-Match`).
pub const CHANGED: &str = "the event changed in Google Calendar since it was read (moved, edited, or made \
repeating): nothing was changed. Read it again with calendar_list and ask your person again if it still needs doing";

async fn fetch(api: &Api, url: &str, query: Vec<(String, String)>) -> Result<Value, Fault> {
    let url = url.to_string();
    let resp = api
        .send(Box::new(move |http: &reqwest::Client, token: &str| http.get(&url).bearer_auth(token).query(&query)))
        .await?;
    resp.json().await.map_err(|_| Fault::Unavailable)
}

/// `etag`: the event as it was read (a change or a deletion): Google answers 412 if it changed since.
async fn write(
    api: &Api,
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
    etag: Option<String>,
) -> Result<Value, Fault> {
    let url = url.to_string();
    let resp = api
        .send(Box::new(move |http: &reqwest::Client, token: &str| {
            let mut request = http.request(method.clone(), &url).bearer_auth(token).query(&[("sendUpdates", "none")]);
            if let Some(etag) = &etag {
                request = request.header(reqwest::header::IF_MATCH, etag);
            }
            match &body {
                Some(body) => request.json(body),
                None => request,
            }
        }))
        .await?;
    Ok(resp.json().await.unwrap_or(Value::Null))
}

/// One event of `calendar` (its title and time zone known from the listing's head).
async fn first_page(config: &Config, api: &Api, calendar: &str) -> Result<Value, Fault> {
    fetch(api, &events_url(config, calendar), vec![("maxResults".into(), "1".into())]).await
}

async fn signed_in(config: &Config) -> Result<Api, Fault> {
    drives::access_slot(config, Slot::Calendar).await
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// One of the account's calendars.
#[derive(Debug, Clone, PartialEq)]
pub struct Calendar {
    /// `primary`, or its id at Google.
    pub key: String,
    /// As the person reads it in Google Calendar (their own name for it first), on one line; the primary one: the
    /// account. Written by whoever shared it: defused, shown between the markers only.
    pub name: String,
    /// The person may add and change events there (owner, writer), not only read them.
    pub writable: bool,
    /// Its time zone (empty: not known).
    pub zone: String,
}

impl Calendar {
    pub fn primary(account: Option<&str>) -> Calendar {
        let name = account.map(|a| a.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|a| !a.is_empty());
        Calendar {
            key: PRIMARY.into(),
            name: crate::archives::defused(tk::head(name.as_deref().unwrap_or("Google"), 120)),
            writable: true,
            zone: String::new(),
        }
    }

    pub fn is_primary(&self) -> bool {
        self.key == PRIMARY
    }
}

/// An entry of Google's calendar list. None: hidden by the person, deleted, or only its free and busy times shared.
pub fn calendar_of(v: &Value, account: Option<&str>) -> Option<Calendar> {
    let flag = |key: &str| v.get(key).and_then(Value::as_bool).unwrap_or(false);
    let role = text(v, "accessRole");
    if flag("hidden") || flag("deleted") || !matches!(role.as_str(), "owner" | "writer" | "reader") {
        return None;
    }
    let zone = text(v, "timeZone");
    if flag("primary") {
        let summary = text(v, "summary");
        let named = account.filter(|a| !a.is_empty()).or(Some(summary.as_str()).filter(|s| !s.is_empty()));
        return Some(Calendar { zone, ..Calendar::primary(named) });
    }
    let id = text(v, "id");
    let own = text(v, "summaryOverride");
    let name = if own.trim().is_empty() { text(v, "summary") } else { own };
    let name = crate::archives::defused(tk::head(&name.split_whitespace().collect::<Vec<_>>().join(" "), 120));
    (!id.is_empty()).then(|| Calendar {
        key: id.clone(),
        name: if name.is_empty() { id } else { name },
        writable: matches!(role.as_str(), "owner" | "writer"),
        zone,
    })
}

/// The calendars the person shows in Google Calendar (`selected`), the primary one first, `MAX_CALENDARS` at most.
/// (One page of Google's list, 250 entries: nobody has more.)
pub async fn calendars(config: &Config, api: &Api, account: Option<&str>) -> Result<Vec<Calendar>, Fault> {
    let query =
        vec![("minAccessRole".to_string(), "reader".to_string()), ("maxResults".to_string(), "250".to_string())];
    let page = fetch(api, &list_url(config), query).await?;
    let items = page.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let flag = |v: &Value, key: &str| v.get(key).and_then(Value::as_bool).unwrap_or(false);
    let shown = |v: &&Value| flag(v, "selected") || flag(v, "primary");
    let mut found: Vec<Calendar> = items.iter().filter(shown).filter_map(|v| calendar_of(v, account)).collect();
    found.sort_by_key(|c| !c.is_primary());
    if !found.first().is_some_and(Calendar::is_primary) {
        found.insert(0, Calendar::primary(account));
    }
    found.truncate(MAX_CALENDARS);
    Ok(found)
}

/// The calendar of an event the agent names (`calendar_update`, `calendar_delete`). None: not one of the person's.
async fn calendar_named_by_id(
    config: &Config,
    api: &Api,
    key: &str,
    account: Option<&str>,
) -> Result<Option<Calendar>, Fault> {
    if key == PRIMARY {
        return Ok(Some(Calendar::primary(account)));
    }
    match fetch(api, &format!("{}/{}", list_url(config), encoded(key)), Vec::new()).await {
        Ok(entry) => Ok(calendar_of(&entry, account).filter(|c| c.key == key)),
        Err(Fault::NotFound) => Ok(None),
        Err(fault) => Err(fault),
    }
}

/// A Google event of `calendar` as calendar.rs lists them: {id, calendar, title, start, end, all_day, location,
/// notes, recurring} (an all-day event's `end` its last day: Google's is the day after). None: cancelled, no start.
pub fn event_of(v: &Value, calendar: &str) -> Option<Value> {
    if text(v, "status") == "cancelled" {
        return None;
    }
    let id = text(v, "id");
    let moment = |key: &str| -> (String, bool) {
        let part = v.get(key).cloned().unwrap_or(Value::Null);
        match (part.get("dateTime").and_then(Value::as_str), part.get("date").and_then(Value::as_str)) {
            (Some(at), _) => (at.to_string(), false),
            (None, Some(day)) => (day.to_string(), true),
            _ => (String::new(), false),
        }
    };
    let (start, all_day) = moment("start");
    let (mut end, _) = moment("end");
    if id.is_empty() || start.is_empty() {
        return None;
    }
    if all_day {
        if let Ok(day) = NaiveDate::parse_from_str(&end, "%Y-%m-%d") {
            end =
                (day - Duration::days(1)).max(NaiveDate::parse_from_str(&start, "%Y-%m-%d").unwrap_or(day)).to_string();
        }
    }
    let defused = |s: String, max: usize| crate::archives::defused(tk::head(&s, max));
    Some(json!({
        "id": event_ref(calendar, &id), "calendar": calendar_ref(calendar),
        "title": defused(text(v, "summary"), 300), "start": start, "end": end, "all_day": all_day,
        "location": defused(text(v, "location"), 300), "notes": defused(text(v, "description"), 2000),
        "recurring": v.get("recurringEventId").is_some() || v.get("recurrence").is_some(),
    }))
}

/// What Google's calendars add to `calendar_list` (calendar.rs): each one shown, and its events of the period, read
/// now (all at once). Shown « Travail (Google Calendar) », the primary one « alex@gmail.com (Google Calendar) », as a
/// mailbox's calendars are by their mailbox.
pub async fn listed(config: &Config, remote: &mut crate::caldav::Remote, from: NaiveDate, to: NaiveDate) {
    remote.any = true;
    let account = stored(config).and_then(|l| l.account);
    let failed = |remote: &mut crate::caldav::Remote, fault: &Fault| {
        remote.calendars.push(json!({"id": CALENDAR_ID, "title": Calendar::primary(account.as_deref()).name,
                                     "source": LABEL}));
        remote.failed.push(format!("{LABEL}: {}", words(fault)));
    };
    let api = match signed_in(config).await {
        Ok(api) => api,
        Err(fault) => return failed(remote, &fault),
    };
    let found = match calendars(config, &api, account.as_deref()).await {
        Ok(found) => found,
        Err(fault) => return failed(remote, &fault),
    };
    let stamp = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc().to_rfc3339()).unwrap_or_default();
    let query = vec![
        ("timeMin".to_string(), stamp(from - Duration::days(1))),
        ("timeMax".to_string(), stamp(to + Duration::days(2))),
        ("singleEvents".to_string(), "true".to_string()),
        ("orderBy".to_string(), "startTime".to_string()),
        ("maxResults".to_string(), PAGE.to_string()),
    ];
    let urls: Vec<String> = found.iter().map(|c| events_url(config, &c.key)).collect();
    let pages = futures::future::join_all(urls.iter().map(|url| fetch(&api, url, query.clone()))).await;
    for (calendar, page) in found.iter().zip(pages) {
        remote.calendars.push(json!({"id": calendar_ref(&calendar.key), "title": calendar.name, "source": LABEL}));
        match page {
            Ok(page) => {
                let items = page.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                remote.events.extend(items.iter().filter_map(|v| event_of(v, &calendar.key)));
            }
            // (its name stays between the markers: not said here)
            Err(fault) => remote.failed.push(format!("{LABEL}, one of its calendars: {}", words(&fault))),
        }
    }
}

/// Google's side of `start`/`end`: a day ({date}, the end the day after the last one), a time with its offset
/// ({dateTime}), or a local time in the calendar's own time zone ({dateTime, timeZone}).
pub fn google_time(moment: Moment, end: bool, zone: &str) -> Value {
    match moment {
        Moment::Day(day) => json!({"date": (if end { day + Duration::days(1) } else { day }).to_string()}),
        Moment::Zoned(at) => json!({"dateTime": at.to_rfc3339()}),
        Moment::Local(at) if zone.is_empty() => json!({"dateTime": at.format("%Y-%m-%dT%H:%M:%S").to_string()}),
        Moment::Local(at) => json!({"dateTime": at.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": zone}),
    }
}

// -- the agents' writes ------------------------------------------------------------------------------------------------

/// Whether a call of a calendar write tool is for Google's calendars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    /// Not theirs: the phone's queue or a mailbox's calendar takes it.
    No,
    /// Theirs: an event of them (`google:`), or an add that names Google, its account, one of its calendars as
    /// calendar_list shows it (« Travail (Google Calendar) »), or that has nowhere else to go.
    Google,
    /// An add naming a calendar neither the phone nor a mailbox has: theirs if one of Google's is named so.
    IfNamed,
}

/// The names that mean the primary calendar (the old ones an agent may still use included).
const GOOGLE_NAMES: [&str; 5] = ["google calendar", "google", "agenda google", "google agenda", CALENDAR_ID];

/// A calendar's name as compared: one line, defused as the listing shows it, lowercase.
fn folded(name: &str) -> String {
    crate::archives::defused(&name.split_whitespace().collect::<Vec<_>>().join(" ")).to_lowercase()
}

/// Which calendar a write tool's call is for. `account`: the Google account's name; `elsewhere`: the names of the
/// agent's other calendars (the phone's, the mailboxes'), which keep a bare name they share.
pub fn aim(
    name: &str,
    arguments: &Dict,
    phone: bool,
    mailboxes: bool,
    account: Option<&str>,
    elsewhere: &[String],
) -> Aim {
    match name {
        "calendar_update" | "calendar_delete" => {
            if tk::str_or_empty(arguments.get("id")).trim().starts_with(ID_PREFIX) {
                Aim::Google
            } else {
                Aim::No
            }
        }
        "calendar_add" => {
            let wanted = folded(&tk::str_or_empty(arguments.get("calendar")));
            if wanted.is_empty() {
                return if !phone && !mailboxes { Aim::Google } else { Aim::No };
            }
            if GOOGLE_NAMES.contains(&wanted.as_str())
                || wanted.starts_with(ID_PREFIX)
                || wanted.ends_with("(google calendar)")
                || account.is_some_and(|a| folded(a) == wanted)
            {
                Aim::Google
            } else if elsewhere.iter().any(|n| folded(n) == wanted) {
                Aim::No
            } else {
                Aim::IfNamed
            }
        }
        _ => Aim::No,
    }
}

/// The calendar an add names, among the account's (`calendars`): nothing, Google itself or its account → the
/// primary one; else the one of that name, « (Google Calendar) » after it or not. None: no such calendar.
pub fn pick<'a>(found: &'a [Calendar], wanted: &str, account: Option<&str>) -> Option<&'a Calendar> {
    let wanted = folded(wanted);
    let base = wanted.strip_suffix("(google calendar)").map(str::trim_end).unwrap_or(&wanted);
    if base.is_empty() || GOOGLE_NAMES.contains(&base) || account.is_some_and(|a| folded(a) == base) {
        return found.iter().find(|c| c.is_primary());
    }
    found.iter().find(|c| folded(&c.name) == base || calendar_ref(&c.key).to_lowercase() == wanted)
}

/// A calendar the person may only read (shared with them so, a subscribed one), named by the agent.
pub const READ_ONLY: &str = "this calendar is read-only for your person (shared with them to read only): nothing was \
asked nor changed. Tell them; to add an event, name one of their calendars they can write to.";
pub const NO_SUCH_CALENDAR: &str = "no Google calendar of that name: their names are in calendar_list \
(« name (Google Calendar) »).";
/// A day with a time, or the reverse (Google refuses them after the card: never asked).
pub const MIXED: &str = "start and end: both dates (an all-day event) or both date-times, not one of each";

/// What the card shows and what is sent to Google, once checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// `calendar_add`, `calendar_update`, `calendar_delete`.
    pub tool: &'static str,
    /// Google's calendar: `primary` or its id.
    pub calendar: String,
    /// Its name, shown on the card when it is not the primary one.
    pub calendar_name: Option<String>,
    /// Google's event id (not for an add).
    pub event: Option<String>,
    /// The event as it was checked (a change, a deletion): sent as `If-Match`, so it is left as it is if it changed.
    pub etag: Option<String>,
    /// The event's title and when, as they will be (an add, a change) or are (a deletion).
    pub title: String,
    pub when: String,
    pub location: String,
    /// Google's body (an add: the whole event; a change: only what changes).
    pub body: Option<Value>,
}

impl Planned {
    /// What a held « Oui » grants exactly (held_approvals.rs): the tool, the calendar, the event and what is sent.
    pub fn command(&self) -> String {
        let body = self.body.as_ref().map(|b| b.to_string()).unwrap_or_default();
        match &self.event {
            Some(id) => format!("{} {} {body}", self.tool, event_ref(&self.calendar, id)).trim_end().to_string(),
            None => format!("{} {} {body}", self.tool, calendar_ref(&self.calendar)),
        }
    }
}

fn when_of(event: &Value) -> String {
    let (start, end) = (text(event, "start"), text(event, "end"));
    let all_day = event.get("all_day").and_then(Value::as_bool).unwrap_or(false);
    let clock = |v: &str| v.get(11..16).unwrap_or("").to_string();
    let (first, last) = (start.get(..10).unwrap_or(&start).to_string(), end.get(..10).unwrap_or(&end).to_string());
    if all_day {
        return if last.is_empty() || last == first { first } else { format!("{first} → {last}") };
    }
    if last.is_empty() || last == first {
        format!("{first} {}–{}", clock(&start), clock(&end)).trim_end_matches('–').to_string()
    } else {
        format!("{first} {} → {last} {}", clock(&start), clock(&end))
    }
}

fn mixed(start: Moment, end: Moment) -> bool {
    matches!(start, Moment::Day(_)) != matches!(end, Moment::Day(_))
}

/// The time zone a local time is read in: the calendar's own (from the list, else its events' head).
async fn zone_of(config: &Config, api: &Api, calendar: &Calendar) -> Result<String, Dict> {
    if !calendar.zone.is_empty() {
        return Ok(calendar.zone.clone());
    }
    first_page(config, api, &calendar.key).await.map(|h| text(&h, "timeZone")).map_err(|f| fault_result(&f))
}

/// The call checked against Google (the event there, not repeating; the times right) before any card, for
/// `calendar` (one the person may write to): Err the tool's answer (nothing asked, nothing sent).
pub async fn plan(
    config: &Config,
    api: &Api,
    name: &str,
    arguments: &Dict,
    calendar: &Calendar,
) -> Result<Planned, Dict> {
    let base = Planned {
        tool: "calendar_add",
        calendar: calendar.key.clone(),
        calendar_name: (!calendar.is_primary()).then(|| calendar.name.clone()),
        event: None,
        etag: None,
        title: String::new(),
        when: String::new(),
        location: String::new(),
        body: None,
    };
    match name {
        "calendar_add" => {
            let event = crate::calendar::new_event(arguments).map_err(|why| tool_text(&why, true))?;
            let start =
                Moment::parse(&text(&event, "start")).ok_or_else(|| tool_text("start: an ISO date-time", true))?;
            let end = Moment::parse(&text(&event, "end")).unwrap_or(start);
            if mixed(start, end) {
                return Err(tool_text(MIXED, true));
            }
            let local = matches!(start, Moment::Local(_)) || matches!(end, Moment::Local(_));
            let tz = if local { zone_of(config, api, calendar).await? } else { String::new() };
            let mut body = json!({"summary": text(&event, "title"), "start": google_time(start, false, &tz),
                                  "end": google_time(end, true, &tz)});
            for (key, google) in [("location", "location"), ("notes", "description")] {
                if !text(&event, key).is_empty() {
                    body[google] = json!(text(&event, key));
                }
            }
            Ok(Planned {
                title: text(&event, "title"),
                when: when_of(&event),
                location: text(&event, "location"),
                body: Some(body),
                ..base
            })
        }
        "calendar_update" | "calendar_delete" => {
            let Some((_, id)) = split_ref(&tk::str_or_empty(arguments.get("id"))) else {
                return Err(tool_text("unknown event id (see calendar_list)", true));
            };
            let raw =
                fetch(api, &event_url(config, &calendar.key, &id), Vec::new()).await.map_err(|f| fault_result(&f))?;
            let Some(current) = event_of(&raw, &calendar.key) else {
                return Err(tool_text("no such event any more", true));
            };
            if current.get("recurring").and_then(Value::as_bool).unwrap_or(false) {
                return Err(tool_text(
                    "a repeating event: the person changes or deletes it in the Google Calendar app (tell them; \
« Google Calendar » is its name in every language)",
                    true,
                ));
            }
            let etag = Some(text(&raw, "etag")).filter(|e| !e.is_empty());
            if name == "calendar_delete" {
                return Ok(Planned {
                    tool: "calendar_delete",
                    event: Some(id),
                    etag,
                    title: text(&current, "title"),
                    when: when_of(&current),
                    location: text(&current, "location"),
                    ..base
                });
            }
            let mut merged: Dict = Dict::new();
            for key in ["title", "location", "notes"] {
                merged.insert(key.into(), json!(text(&current, key)));
            }
            merged.insert("start".into(), json!(text(&current, "start")));
            merged.insert("end".into(), json!(text(&current, "end")));
            let mut body = Dict::new();
            for (key, google, max) in
                [("title", "summary", 300), ("location", "location", 300), ("notes", "description", 2000)]
            {
                if arguments.contains_key(key) {
                    let given = tk::str_or_empty(arguments.get(key));
                    let value = tk::head(&given, max).to_string();
                    merged.insert(key.into(), json!(value));
                    body.insert(google.into(), json!(value));
                }
            }
            let given = |k: &str| Some(tk::str_or_empty(arguments.get(k)).trim().to_string()).filter(|v| !v.is_empty());
            let (old_start, old_end) = (Moment::parse(&text(&current, "start")), Moment::parse(&text(&current, "end")));
            let start = match given("start") {
                Some(v) => Some(Moment::parse(&v).ok_or_else(|| tool_text("start: an ISO date-time or a date", true))?),
                None => None,
            };
            let end = match given("end") {
                Some(v) => Some(Moment::parse(&v).ok_or_else(|| tool_text("end: an ISO date-time or a date", true))?),
                // a new start alone: the same length
                None => match (start, old_start, old_end) {
                    (Some(new), Some(s), Some(e)) => Some(new.plus((e.wall() - s.wall()).num_minutes())),
                    _ => None,
                },
            };
            if start.is_some() || end.is_some() {
                let (s, e) = (start.or(old_start), end.or(old_end));
                let (Some(s), Some(e)) = (s, e) else {
                    return Err(tool_text("start: an ISO date-time or a date", true));
                };
                if mixed(s, e) {
                    return Err(tool_text(MIXED, true));
                }
                if e.wall() < s.wall() {
                    return Err(tool_text("the end must come after the start", true));
                }
                // a local time: in the event's own time zone, else the calendar's
                let local = matches!(s, Moment::Local(_)) || matches!(e, Moment::Local(_));
                let tz = match raw.pointer("/start/timeZone").and_then(Value::as_str) {
                    Some(own) if local => own.to_string(),
                    None if local => zone_of(config, api, calendar).await?,
                    _ => String::new(),
                };
                body.insert("start".into(), google_time(s, false, &tz));
                body.insert("end".into(), google_time(e, true, &tz));
                merged.insert("start".into(), json!(s.write()));
                merged.insert("end".into(), json!(e.write()));
                merged.insert("all_day".into(), json!(matches!(s, Moment::Day(_))));
            } else {
                merged.insert("all_day".into(), current.get("all_day").cloned().unwrap_or(json!(false)));
            }
            if body.is_empty() {
                return Err(tool_text("nothing to change", true));
            }
            let shown = Value::Object(merged);
            Ok(Planned {
                tool: "calendar_update",
                event: Some(id),
                etag,
                title: text(&shown, "title"),
                when: when_of(&shown),
                location: text(&shown, "location"),
                body: Some(Value::Object(body)),
                ..base
            })
        }
        _ => Err(tool_text(&format!("unknown tool: {name}"), true)),
    }
}

fn fault_result(fault: &Fault) -> Dict {
    match fault {
        Fault::NotFound | Fault::Changed => tool_text(&words(fault), true),
        Fault::Expired => tool_failure(&words(fault), "Your Google Calendar needs to be connected again in the app."),
        _ => tool_failure(&words(fault), "I can't reach your Google Calendar just now."),
    }
}

/// What was planned, done at Google (after the person's « Oui »).
pub async fn apply(config: &Config, api: &Api, planned: &Planned) -> Dict {
    let calendar = &planned.calendar;
    let (body, etag) = (planned.body.clone(), planned.etag.clone());
    let done = match (planned.tool, &planned.event) {
        ("calendar_add", _) => write(api, reqwest::Method::POST, &events_url(config, calendar), body, None).await,
        ("calendar_update", Some(id)) => {
            write(api, reqwest::Method::PATCH, &event_url(config, calendar, id), body, etag).await
        }
        ("calendar_delete", Some(id)) => {
            write(api, reqwest::Method::DELETE, &event_url(config, calendar, id), None, etag).await
        }
        _ => return tool_text("unknown event id (see calendar_list)", true),
    };
    match done {
        Ok(answer) => {
            tracing::info!(tool = planned.tool, "google calendar: written");
            let title = crate::archives::defused(&planned.title);
            match planned.tool {
                "calendar_add" => tool_text(
                    &format!(
                        "Added to {LABEL}: id {} · {} · {title}. The person sees it in their calendar.",
                        event_ref(calendar, &text(&answer, "id")),
                        planned.when
                    ),
                    false,
                ),
                "calendar_update" => tool_text(&format!("Changed in {LABEL}: {} · {title}.", planned.when), false),
                _ => tool_text(&format!("Deleted from {LABEL}."), false),
            }
        }
        Err(fault) => fault_result(&fault),
    }
}

// -- the card before each write ----------------------------------------------------------------------------------------

/// The card's sentence (the agent's name `shown`, the event's title), in the agent's language.
pub fn sentence(lang: &str, tool: &str, shown: &str, title: &str) -> String {
    use crate::person_ok::pick;
    let said = match tool {
        "calendar_add" => pick(
            lang,
            "{n} veut ajouter un événement à ton agenda Google",
            "{n} wants to add an event to your Google Calendar",
            "{n} möchte einen Termin in deinen Google Kalender eintragen",
            "{n} quiere añadir un evento a tu Google Calendar",
        ),
        "calendar_update" => pick(
            lang,
            "{n} veut modifier un événement de ton agenda Google",
            "{n} wants to change an event in your Google Calendar",
            "{n} möchte einen Termin in deinem Google Kalender ändern",
            "{n} quiere cambiar un evento de tu Google Calendar",
        ),
        _ => pick(
            lang,
            "{n} veut supprimer un événement de ton agenda Google",
            "{n} wants to delete an event from your Google Calendar",
            "{n} möchte einen Termin aus deinem Google Kalender löschen",
            "{n} quiere borrar un evento de tu Google Calendar",
        ),
    }
    .replace("{n}", shown);
    let title = crate::cards::quoted(title, 120);
    match (title.is_empty(), lang.starts_with("fr")) {
        (true, _) => said,
        (false, true) => format!("{said} : « {title} »"),
        (false, false) => format!("{said}: « {title} »"),
    }
}

pub const WRITE_REFUSED: &str = "Your person said no: their Google Calendar is not changed. Tell them plainly; don't \
ask again unless they tell you to.";
pub const WRITE_NO_ANSWER: &str = "Your person did not answer in time: nothing was changed in their Google Calendar. \
Tell them their « Oui » waits in the app, and call again once they say so.";
pub const WRITE_HELD: &str = "Your person is not here now (this turn runs on its own): a card asks them before their \
Google Calendar is changed; it was not refused. When they say yes you will be told: then make the same call again. \
End with your report saying what waits for their OK.";

/// A calendar write of `agent` for Google's calendar: checked there, asked on the person's card, done on « Oui ».
/// None: not for Google's calendar (the phone's queue or a mailbox's calendar takes it).
pub async fn write_call(state: &AppState, agent: &AgentConfig, name: &str, arguments: &Dict) -> Option<Dict> {
    if !matches!(name, "calendar_add" | "calendar_update" | "calendar_delete") {
        return None;
    }
    let config = state.config();
    if !offered(&config, &agent.name) {
        // an id of Google's from an agent that may not use it: no such event for it
        return tk::str_or_empty(arguments.get("id"))
            .trim()
            .starts_with(ID_PREFIX)
            .then(|| tool_text("unknown event id (see calendar_list)", true));
    }
    let (c, a) = (config.clone(), agent.name.clone());
    let (phone, mailboxes, account, elsewhere) = tokio::task::spawn_blocking(move || {
        let phone = crate::calendar::enabled(&c, &a);
        let sources = crate::caldav::sources(&c, &a);
        // the other calendars' names: a bare name they share stays theirs
        let mut elsewhere: Vec<String> = Vec::new();
        if phone {
            let data = crate::calendar::read(&c);
            let listed = data.get("calendars").and_then(Value::as_array).cloned().unwrap_or_default();
            elsewhere.extend(listed.iter().map(|x| text(x, "title")));
        }
        for source in &sources {
            for calendar in &source.calendars {
                elsewhere.push(calendar.title.clone());
                elsewhere.push(format!("{} ({})", calendar.title, source.label));
            }
        }
        (phone, !sources.is_empty(), stored(&c).and_then(|l| l.account), elsewhere)
    })
    .await
    .ok()?;
    let aim = aim(name, arguments, phone, mailboxes, account.as_deref(), &elsewhere);
    if aim == Aim::No {
        return None;
    }
    // a name nobody else has: Google's only if one of its calendars has it, else as before (not reached: not ours)
    let maybe = aim == Aim::IfNamed;
    let api = match signed_in(&config).await {
        Ok(api) => api,
        Err(_) if maybe => return None,
        Err(fault) => return Some(fault_result(&fault)),
    };
    let calendar = if name == "calendar_add" {
        let found = match calendars(&config, &api, account.as_deref()).await {
            Ok(found) => found,
            Err(_) if maybe => return None,
            Err(fault) => return Some(fault_result(&fault)),
        };
        match pick(&found, &tk::str_or_empty(arguments.get("calendar")), account.as_deref()) {
            Some(calendar) => calendar.clone(),
            None if maybe => return None,
            None => return Some(tool_text(NO_SUCH_CALENDAR, true)),
        }
    } else {
        let unknown = || Some(tool_text("unknown event id (see calendar_list)", true));
        let Some((key, _)) = split_ref(&tk::str_or_empty(arguments.get("id"))) else { return unknown() };
        match calendar_named_by_id(&config, &api, &key, account.as_deref()).await {
            Ok(Some(calendar)) => calendar,
            Ok(None) => return unknown(),
            Err(fault) => return Some(fault_result(&fault)),
        }
    };
    if !calendar.writable {
        return Some(tool_text(READ_ONLY, true));
    }
    let planned = match plan(&config, &api, name, arguments, &calendar).await {
        Ok(planned) => planned,
        Err(said) => return Some(said),
    };
    let lang = crate::mail_tidy::language(&config, &agent.name);
    let shown = crate::mail_tidy::shown_agent(&config, &agent.name);
    let mut detail = planned.when.clone();
    if !planned.location.is_empty() {
        detail += &format!(" · {}", crate::cards::quoted(&planned.location, 120));
    }
    // another calendar than the primary one: its name too (« Travail » · Google Calendar)
    let place = match &planned.calendar_name {
        Some(other) => format!("« {} » · {LABEL}", crate::cards::quoted(other, 60)),
        None => LABEL.to_string(),
    };
    detail += &format!(" ({place})\n{}", crate::cards::quoted(&planned.title, 300));
    let delete = planned.tool == "calendar_delete";
    let summary = sentence(&lang, planned.tool, &shown, &planned.title);
    // a deletion is asked each time (audit 1.3); adds and changes may be allowed for good (Alex 23/11), in the primary
    // calendar only: another one (shared by someone else) is asked each time (review 2, 21/11). The family's agent and
    // a project agent never get here (`offered`), and grant_always refuses them.
    let may_always = !delete && planned.calendar_name.is_none();
    if may_always && crate::approval_level::granted(&config, &agent.name, crate::person_ok::CALENDAR_WRITE) {
        // the person's « Toujours » for this agent (never their level): no card, in routines too
        let session = crate::runs::CALL_SESSION.try_with(|s| s.clone()).ok().flatten().filter(|s| !s.is_empty());
        crate::approval_level::record_always(&config, &agent.name, crate::person_ok::CALENDAR_WRITE, &summary, session);
        tracing::info!(agent = %agent.name, tool = planned.tool, "google calendar: always allowed by the person");
        return Some(apply(&config, &api, &planned).await);
    }
    let card = crate::person_ok::Card {
        kind: if delete { crate::person_ok::CALENDAR_DELETE } else { crate::person_ok::CALENDAR_WRITE },
        tool: planned.tool,
        command: planned.command(),
        summary,
        detail,
        // « Toujours autoriser »: kept for this agent by the person's answer (app.rs `approve`, its `bip_always_kind`)
        always: if may_always {
            crate::approval_level::always_button(&lang, crate::person_ok::CALENDAR_WRITE)
        } else {
            None
        },
    };
    use crate::person_ok::Answer;
    let refused = |a: &'static str, b: &'static str| if delete { a } else { b };
    match crate::person_ok::ask(state, agent, card).await {
        Answer::Once | Answer::Always => Some(apply(&config, &api, &planned).await),
        Answer::Deny => Some(tool_text(refused(crate::calendar::DELETE_REFUSED, WRITE_REFUSED), true)),
        Answer::NoAnswer => Some(tool_text(refused(crate::calendar::DELETE_NO_ANSWER, WRITE_NO_ANSWER), true)),
        Answer::Held => Some(tool_text(refused(crate::calendar::DELETE_HELD, WRITE_HELD), false)),
    }
}

/// An event the person accepted on an offer's card (plans/offers.rs: that card was this write's, it showed the event
/// exactly), added to the primary calendar as `event` says (calendar.rs `new_event`'s fields), no other card. Err: why
/// not, in a few words for the log.
pub async fn add_accepted(config: &Config, event: &Dict) -> Result<(), String> {
    let api = signed_in(config).await.map_err(|f| words(&f))?;
    let account = stored(config).and_then(|l| l.account);
    let found = calendars(config, &api, account.as_deref()).await.map_err(|f| words(&f))?;
    let calendar = found.iter().find(|c| c.is_primary()).cloned().unwrap_or_else(|| Calendar::primary(account.as_deref()));
    if !calendar.writable {
        return Err("its primary calendar is read only".into());
    }
    let planned =
        plan(config, &api, "calendar_add", event, &calendar).await.map_err(|_| "not taken as given".to_string())?;
    let done = apply(config, &api, &planned).await;
    match done.get("isError").and_then(Value::as_bool) {
        Some(false) => Ok(()),
        _ => Err("Google Calendar refused it".into()),
    }
}

// -- routes ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartBody {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub client: String,
}

impl Validate for StartBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "agents", Some(&self.agents), AGENTS_MAX);
        web::check_max_len(errors, "client", Some(&self.client), 20);
    }
}

#[derive(Debug, Deserialize)]
pub struct CallbackBody {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub code: String,
}

impl Validate for CallbackBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "url", Some(&self.url), 8000);
        web::check_max_len(errors, "state", Some(&self.state), 200);
        web::check_max_len(errors, "code", Some(&self.code), 4000);
    }
}

#[derive(Debug, Deserialize)]
pub struct AgentsBody {
    #[serde(default)]
    pub agents: Vec<String>,
}

impl Validate for AgentsBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "agents", Some(&self.agents), AGENTS_MAX);
    }
}

/// The agents whose tools changed: their entry registered again, so their Hermes fetches the new list.
async fn tools_changed(config: &Config, names: &[String]) {
    let run = crate::agents_admin::default_run();
    for name in names {
        if let Some(agent) = config.agent(Some(name)) {
            if let Err(exc) = crate::agent_tools::register(config, agent, &run).await {
                tracing::warn!(agent = %agent.name, error = %exc, "agent tools not registered");
            }
        }
    }
}

async fn listing_route(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    if !config.google_calendar.enabled {
        return Err(off());
    }
    Ok(web::ok(view(&config, stored(&config).as_ref())))
}

async fn start_route(State(state): State<AppState>, Json(body): Json<StartBody>) -> ApiResult<Response> {
    let config = state.config();
    if body.client == "android" && config.google_calendar.enabled {
        // a place of the offer: its Google Web client asked of the guichet before Google's window (drives.rs)
        drives::ask_place_web(&config, true).await;
    }
    Ok(web::ok(start(&config, &body.agents, &body.client)?))
}

async fn callback_route(State(state): State<AppState>, Json(body): Json<CallbackBody>) -> ApiResult<Response> {
    let config = state.config();
    let query = if !body.url.is_empty() {
        connections::callback_query(&body.url)
    } else {
        [("state", body.state), ("code", body.code)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    };
    let shown = finish(&config, query).await?;
    let agents: Vec<String> =
        shown["agents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    tools_changed(&config, &agents).await;
    Ok(web::ok(shown))
}

async fn agents_route(State(state): State<AppState>, Json(body): Json<AgentsBody>) -> ApiResult<Response> {
    let config = state.config();
    let (shown, changed) = set_agents(&config, &body.agents).await?;
    tools_changed(&config, &changed).await;
    Ok(web::ok(shown))
}

async fn disconnect_route(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let (answer, agents) = disconnect(&config).await?;
    tools_changed(&config, &agents).await;
    Ok(web::ok(answer))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/google-calendar", get(listing_route).delete(disconnect_route))
        .route("/google-calendar/start", post(start_route))
        .route("/google-calendar/callback", post(callback_route))
        .route("/google-calendar/agents", put(agents_route))
}

#[cfg(test)]
#[path = "google_calendar_tests.rs"]
mod tests;
