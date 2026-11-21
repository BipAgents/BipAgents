//! « Agenda Google » (Alex 20/11: the catalogue of apps signed in by account, one service at a time; Gmail, Calendar,
//! Drive first): the person connects their Google account once in the app (« Applis et outils » › Agenda Google) and
//! chooses the agents that may use its calendar. Those agents read its upcoming events and ask to add, change or
//! delete one through the same calendar tools as the phone's (calendar.rs): `calendar_list` lists the Google events
//! beside the phone's and the mailboxes' (source « Google Calendar »), `calendar_add` writes there when it names that
//! calendar (or when the agent has no other calendar), `calendar_update` and `calendar_delete` act on its events
//! (ids `google:<event id>`).
//!
//! **Every write asks the person first**, on the bridge's card (person_ok.rs): « Oui » (this once), « Oui, pour cette
//! conversation » (adds and changes, for the rest of that Hermes session) or « Non »; a deletion is asked each time
//! (audit 1.3, as the phone's). Never silently: the kind is critical (approval_level.rs), asked at every level; a
//! turn nobody follows holds the card (held_approvals.rs) and its « Oui » lets that very call through once.
//!
//! The sign-in reuses Google Drive's (drives.rs): the same Google clients (`[drives.google_drive]`: the iOS client and
//! its redirect, Android's Web client from bridge.toml, the guichet or the person's choice), PKCE, the same renewal
//! (`drives::Slot::Calendar`), its own grant and tokens kept in the connections' encrypted store
//! (`Stored.google_calendar`). Scope `calendar.events` only, asked incrementally (`include_granted_scopes`); with it
//! Google gives the primary calendar's events (other calendars would need `calendar.calendarlist.readonly`, a choice
//! for later). No token ever reaches Hermes nor the model: the bridge calls Calendar API v3 itself and gives the agent
//! text. What an event says is written by whoever made or sent it: listed between calendar.rs's UNTRUSTED markers,
//! defused (archives.rs) so it can't close them. Updates are never sent to the guests (`sendUpdates=none`).
//!
//! Off by default: `calendar.events` is a « sensitive » scope, and Google must verify the app (a short video) before
//! anyone beyond its test users signs in. `[google_calendar] enabled = true` switches it on for one place (a test
//! user's), with Google Drive's client in `[drives.google_drive]` (drives itself may stay off). Turned off with a
//! calendar still connected: at the next start it is forgotten here and revoked (`ended_when_off`).
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

pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
const API: &str = "https://www.googleapis.com/calendar/v3";
/// The calendar's name in the agents' tools.
pub const LABEL: &str = "Google Calendar";
/// Its id in `calendar_list`, and its events' prefix.
pub const CALENDAR_ID: &str = "google:primary";
pub const ID_PREFIX: &str = "google:";
/// Events one listing reads at most (calendar.rs shows 200).
const PAGE: &str = "250";
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

fn events_url(config: &Config) -> String {
    format!("{}/calendars/primary/events", api_base(config))
}

fn event_url(config: &Config, id: &str) -> String {
    format!("{}/{}", events_url(config), url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>())
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

/// Google's sign-in address for the calendar: `calendar.events` added to what the person gave already.
pub fn authorize_url(ep: &drives::Endpoints, state: &str, verifier: &str) -> String {
    let challenge = connections::pkce_challenge(verifier);
    let params: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", &ep.client.client_id),
        ("redirect_uri", &ep.client.redirect_uri),
        ("scope", SCOPE),
        ("state", state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("include_granted_scopes", "true"),
    ];
    url::Url::parse_with_params(&ep.auth, &params).map(String::from).unwrap_or_default()
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
        return Ok(json!({"state": state, "server_client_id": web.client.client_id, "scopes": [SCOPE],
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
    // Google says which scopes it gave: the calendar's must be among them (the person may untick it)
    if query.get("scope").is_some_and(|s| !s.split([' ', ',']).any(|x| x == SCOPE)) {
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
    if tokens.scope.as_deref().is_some_and(|s| !s.split(' ').any(|x| x == SCOPE)) {
        return Err(coded(400, "denied", "the calendar was not allowed"));
    }
    let api = Api::new(ep, tokens.access_token.clone(), None);
    let account = match first_page(config, &api).await {
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
    let _guard = connections::LOCK.lock().await;
    let store = connections::store_of(config);
    let mut stored = store.read()?;
    let link = stored.google_calendar.as_mut().ok_or_else(not_connected)?;
    let before = std::mem::replace(&mut link.agents, agents.clone());
    let changed = agents
        .iter()
        .filter(|a| !before.contains(a))
        .chain(before.iter().filter(|a| !agents.contains(a)))
        .cloned()
        .collect();
    let shown = view(config, Some(&*link));
    store.write(&stored)?;
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
    let ep = drives::endpoints(config, Provider::Google);
    let revoked =
        !shared && drives::revoke(Provider::Google, ep.as_ref().and_then(|e| e.revoke.as_deref()), &link.tokens).await;
    tracing::info!(revoked, shared, "google calendar: disconnected");
    Ok((json!({"ok": true, "revoked": revoked}), link.agents))
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
            "the connection to {LABEL} ended: your person connects it again in the app (Applis et outils › Agenda Google)"
        ),
        Fault::NotFound => "no such event any more (moved or deleted)".to_string(),
        Fault::RateLimited(Some(s)) => format!("{LABEL} asks to slow down: try again in {s} seconds"),
        Fault::RateLimited(None) => format!("{LABEL} asks to slow down: try again in a minute"),
        Fault::Unavailable => format!("{LABEL} has trouble right now: try again later"),
        Fault::Refused => format!("{LABEL} refuses this one"),
        Fault::Unreachable => format!("{LABEL} can't be reached right now"),
        Fault::TooLarge => "too large".to_string(),
        Fault::Other(why) => why.clone(),
    }
}

async fn fetch(api: &Api, url: &str, query: Vec<(String, String)>) -> Result<Value, Fault> {
    let url = url.to_string();
    let resp = api
        .send(Box::new(move |http: &reqwest::Client, token: &str| http.get(&url).bearer_auth(token).query(&query)))
        .await?;
    resp.json().await.map_err(|_| Fault::Unavailable)
}

async fn write(api: &Api, method: reqwest::Method, url: &str, body: Option<Value>) -> Result<Value, Fault> {
    let url = url.to_string();
    let resp = api
        .send(Box::new(move |http: &reqwest::Client, token: &str| {
            let request = http.request(method.clone(), &url).bearer_auth(token).query(&[("sendUpdates", "none")]);
            match &body {
                Some(body) => request.json(body),
                None => request,
            }
        }))
        .await?;
    Ok(resp.json().await.unwrap_or(Value::Null))
}

/// One event of the primary calendar (its title and time zone known from the listing's head).
async fn first_page(config: &Config, api: &Api) -> Result<Value, Fault> {
    fetch(api, &events_url(config), vec![("maxResults".into(), "1".into())]).await
}

async fn signed_in(config: &Config) -> Result<Api, Fault> {
    drives::access_slot(config, Slot::Calendar).await
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A Google event as calendar.rs lists them: {id, calendar, title, start, end, all_day, location, notes, recurring}
/// (an all-day event's `end` its last day: Google's is the day after). None: cancelled, or no start.
pub fn event_of(v: &Value) -> Option<Value> {
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
        "id": format!("{ID_PREFIX}{id}"), "calendar": CALENDAR_ID,
        "title": defused(text(v, "summary"), 300), "start": start, "end": end, "all_day": all_day,
        "location": defused(text(v, "location"), 300), "notes": defused(text(v, "description"), 2000),
        "recurring": v.get("recurringEventId").is_some() || v.get("recurrence").is_some(),
    }))
}

/// What Google's calendar adds to `calendar_list` (calendar.rs): its calendar, and its events of the period, read now.
pub async fn listed(config: &Config, remote: &mut crate::caldav::Remote, from: NaiveDate, to: NaiveDate) {
    remote.any = true;
    // shown « alex@gmail.com (Google Calendar) », as a mailbox's calendars are by their mailbox
    let account = stored(config).and_then(|l| l.account).unwrap_or_else(|| "Google".to_string());
    remote.calendars.push(json!({"id": CALENDAR_ID, "title": account, "source": LABEL}));
    let api = match signed_in(config).await {
        Ok(api) => api,
        Err(fault) => return remote.failed.push(format!("{LABEL}: {}", words(&fault))),
    };
    let stamp = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc().to_rfc3339()).unwrap_or_default();
    let query = vec![
        ("timeMin".to_string(), stamp(from - Duration::days(1))),
        ("timeMax".to_string(), stamp(to + Duration::days(2))),
        ("singleEvents".to_string(), "true".to_string()),
        ("orderBy".to_string(), "startTime".to_string()),
        ("maxResults".to_string(), PAGE.to_string()),
    ];
    match fetch(&api, &events_url(config), query).await {
        Ok(page) => {
            let items = page.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            remote.events.extend(items.iter().filter_map(event_of));
        }
        Err(fault) => remote.failed.push(format!("{LABEL}: {}", words(&fault))),
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

/// Whether this call of a calendar write tool is for Google's calendar: an event of it (`google:`), or an event to
/// add that names it (« Google Calendar », « google », its account) or has nowhere else to go (no phone calendar, no
/// mailbox calendar). `account`: the Google account's name.
pub fn targets(name: &str, arguments: &Dict, phone: bool, mailboxes: bool, account: Option<&str>) -> bool {
    match name {
        "calendar_update" | "calendar_delete" => tk::str_or_empty(arguments.get("id")).trim().starts_with(ID_PREFIX),
        "calendar_add" => {
            let wanted = tk::str_or_empty(arguments.get("calendar")).trim().to_lowercase();
            if wanted.is_empty() {
                return !phone && !mailboxes;
            }
            let names = ["google calendar", "google", "agenda google", "google agenda", CALENDAR_ID];
            names.contains(&wanted.as_str())
                || wanted.ends_with("(google calendar)")
                || account.is_some_and(|a| a.to_lowercase() == wanted)
        }
        _ => false,
    }
}

/// What the card shows and what is sent to Google, once checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// `calendar_add`, `calendar_update`, `calendar_delete`.
    pub tool: &'static str,
    /// Google's event id (not for an add).
    pub event: Option<String>,
    /// The event's title and when, as they will be (an add, a change) or are (a deletion).
    pub title: String,
    pub when: String,
    pub location: String,
    /// Google's body (an add: the whole event; a change: only what changes).
    pub body: Option<Value>,
}

impl Planned {
    /// What a held « Oui » grants exactly (held_approvals.rs): the tool, the event and what is sent.
    pub fn command(&self) -> String {
        let body = self.body.as_ref().map(|b| b.to_string()).unwrap_or_default();
        match &self.event {
            Some(id) => format!("{} {ID_PREFIX}{id} {body}", self.tool).trim_end().to_string(),
            None => format!("{} {CALENDAR_ID} {body}", self.tool),
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

/// The call checked against Google (the event there, not repeating; the times right) before any card: Err the
/// tool's answer (nothing asked, nothing sent).
pub async fn plan(config: &Config, api: &Api, name: &str, arguments: &Dict) -> Result<Planned, Dict> {
    let zone = |head: &Value| text(head, "timeZone");
    match name {
        "calendar_add" => {
            let event = crate::calendar::new_event(arguments).map_err(|why| tool_text(&why, true))?;
            let start =
                Moment::parse(&text(&event, "start")).ok_or_else(|| tool_text("start: an ISO date-time", true))?;
            let end = Moment::parse(&text(&event, "end")).unwrap_or(start);
            let local = matches!(start, Moment::Local(_)) || matches!(end, Moment::Local(_));
            let tz = if local {
                first_page(config, api).await.map(|h| zone(&h)).map_err(|f| fault_result(&f))?
            } else {
                String::new()
            };
            let mut body = json!({"summary": text(&event, "title"), "start": google_time(start, false, &tz),
                                  "end": google_time(end, true, &tz)});
            for (key, google) in [("location", "location"), ("notes", "description")] {
                if !text(&event, key).is_empty() {
                    body[google] = json!(text(&event, key));
                }
            }
            Ok(Planned {
                tool: "calendar_add",
                event: None,
                title: text(&event, "title"),
                when: when_of(&event),
                location: text(&event, "location"),
                body: Some(body),
            })
        }
        "calendar_update" | "calendar_delete" => {
            let id = tk::str_or_empty(arguments.get("id")).trim().trim_start_matches(ID_PREFIX).to_string();
            if id.is_empty() {
                return Err(tool_text("unknown event id (see calendar_list)", true));
            }
            let raw = fetch(api, &event_url(config, &id), Vec::new()).await.map_err(|f| fault_result(&f))?;
            let Some(current) = event_of(&raw) else {
                return Err(tool_text("no such event any more", true));
            };
            if current.get("recurring").and_then(Value::as_bool).unwrap_or(false) {
                return Err(tool_text(
                    "a repeating event: the person changes or deletes it in Google Calendar (tell them)",
                    true,
                ));
            }
            if name == "calendar_delete" {
                return Ok(Planned {
                    tool: "calendar_delete",
                    event: Some(id),
                    title: text(&current, "title"),
                    when: when_of(&current),
                    location: text(&current, "location"),
                    body: None,
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
                if e.wall() < s.wall() {
                    return Err(tool_text("the end must come after the start", true));
                }
                // a local time: in the event's own time zone, else the calendar's
                let local = matches!(s, Moment::Local(_)) || matches!(e, Moment::Local(_));
                let tz = match raw.pointer("/start/timeZone").and_then(Value::as_str) {
                    Some(own) if local => own.to_string(),
                    None if local => first_page(config, api).await.map(|h| zone(&h)).map_err(|f| fault_result(&f))?,
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
                title: text(&shown, "title"),
                when: when_of(&shown),
                location: text(&shown, "location"),
                body: Some(Value::Object(body)),
            })
        }
        _ => Err(tool_text(&format!("unknown tool: {name}"), true)),
    }
}

fn fault_result(fault: &Fault) -> Dict {
    match fault {
        Fault::NotFound => tool_text(&words(fault), true),
        Fault::Expired => tool_failure(&words(fault), "Your Google Calendar needs to be connected again in the app."),
        _ => tool_failure(&words(fault), "I can't reach your Google Calendar just now."),
    }
}

/// What was planned, done at Google (after the person's « Oui »).
pub async fn apply(config: &Config, api: &Api, planned: &Planned) -> Dict {
    let done = match (planned.tool, &planned.event) {
        ("calendar_add", _) => write(api, reqwest::Method::POST, &events_url(config), planned.body.clone()).await,
        ("calendar_update", Some(id)) => {
            write(api, reqwest::Method::PATCH, &event_url(config, id), planned.body.clone()).await
        }
        ("calendar_delete", Some(id)) => write(api, reqwest::Method::DELETE, &event_url(config, id), None).await,
        _ => return tool_text("unknown event id (see calendar_list)", true),
    };
    match done {
        Ok(answer) => {
            tracing::info!(tool = planned.tool, "google calendar: written");
            let title = crate::archives::defused(&planned.title);
            match planned.tool {
                "calendar_add" => tool_text(
                    &format!(
                        "Added to {LABEL}: id {ID_PREFIX}{} · {} · {title}. The person sees it in their calendar.",
                        text(&answer, "id"),
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
    let (phone, mailboxes, account) = tokio::task::spawn_blocking(move || {
        (crate::calendar::enabled(&c, &a), crate::caldav::shared_with(&c, &a), stored(&c).and_then(|l| l.account))
    })
    .await
    .ok()?;
    if !targets(name, arguments, phone, mailboxes, account.as_deref()) {
        return None;
    }
    let api = match signed_in(&config).await {
        Ok(api) => api,
        Err(fault) => return Some(fault_result(&fault)),
    };
    let planned = match plan(&config, &api, name, arguments).await {
        Ok(planned) => planned,
        Err(said) => return Some(said),
    };
    let lang = crate::mail_tidy::language(&config, &agent.name);
    let shown = crate::mail_tidy::shown_agent(&config, &agent.name);
    let mut detail = planned.when.clone();
    if !planned.location.is_empty() {
        detail += &format!(" · {}", crate::cards::quoted(&planned.location, 120));
    }
    detail += &format!(" ({LABEL})\n{}", crate::cards::quoted(&planned.title, 300));
    let delete = planned.tool == "calendar_delete";
    let card = crate::person_ok::Card {
        kind: if delete { crate::person_ok::CALENDAR_DELETE } else { crate::person_ok::CALENDAR_WRITE },
        tool: planned.tool,
        command: planned.command(),
        summary: sentence(&lang, planned.tool, &shown, &planned.title),
        detail,
        always: None,
        // a deletion is asked each time (audit 1.3); adds and changes may be allowed for the conversation
        session: !delete,
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
