//! « Outlook » (Alex 23/11, « Oui, Outlook ensuite »): a Microsoft account — a personal one (Outlook.com, Hotmail) or
//! a work one — gives its mail, its Outlook calendar and its contacts to the agents the person chooses, through the
//! tools those agents already have, never new ones:
//!
//! - the mail tools (outlook_mail.rs): `mail_search`, `mail_read`, `mail_attachment`, `mail_draft` and the tidying
//!   `mail_overview` / `mail_bulk`, on a mailbox named « Outlook » beside the IMAP ones (mailbox.rs). Nothing is ever
//!   sent: a draft waits in Outlook's Drafts for the person; a tidying (archive, move, mark read, trash) waits for the
//!   person's card (person_ok.rs);
//! - the calendar tools (outlook_calendar.rs): its calendars listed beside the phone's, the mailboxes' and Google's;
//!   every add or change asked on the same card as Google Calendar's (`calendar_write`, « Toujours autoriser » for the
//!   default calendar only, kept apart from Google's), every deletion asked each time (`calendar_delete`);
//! - the contacts' read tools (outlook_contacts.rs): `contacts_search`, `contacts_birthdays`, `contacts_list_group`,
//!   read only (`Contacts.Read`).
//!
//! The sign-in is OneDrive's (drives.rs): the same Microsoft client (Kerfy's Entra app by default, `/common`, any
//! directory and personal accounts, `bipagents://oauth/callback`, code + PKCE, no secret), its own grant and tokens kept
//! in the connections' encrypted store (`Stored.outlook`), renewed as OneDrive's (`drives::Slot::Outlook`). Scopes
//! [`SCOPES`]; a sign-in that leaves one out is refused. No token ever reaches Hermes nor the model: the bridge calls
//! Microsoft Graph itself and gives the agents text. What a mail, an event, a calendar's name or a contact says is
//! written by others: data between the markers that say so, defused (archives.rs), never instructions.
//!
//! Microsoft has no revocation address for a public client's delegated grant (as for OneDrive): « Déconnecter » forgets
//! the tokens here (`revoked: false`); the person withdraws the consent in their Microsoft account if they want.
//!
//! Graph's ids are long: the agents read short numbers instead (`outlook|<folder>|<n>` for a mail, as mail_search's
//! ids; `outlook:<n>:<m>` for an event), kept here (`<data>/outlook/ids.json`, 0600) with Graph's immutable ids
//! (`Prefer: IdType="ImmutableId"`: a moved mail keeps its id).
//!
//! Off by default: `[outlook] enabled = true` switches it on for one place, with Microsoft's client of `[drives]`
//! (Kerfy's by default; drives itself may stay off). Turned off with an account still connected: forgotten at the next
//! start (`ended_when_off`).
//!
//!     [outlook]
//!     enabled = true
//!
//! App routes (the bridge key; 404 while off). Errors `{"detail": {"code", "message"}}` as drives.rs':
//!
//!     GET    /v1/outlook             {enabled, configured, state, account, connected_at, agents, parts}
//!     POST   /v1/outlook/start       {agents} → {authorize_url, state, redirect_uri, expires_in}
//!     POST   /v1/outlook/callback    {url} → as listed  (400 denied | admin_consent | unknown_state | bad_callback)
//!     PUT    /v1/outlook/agents      {agents, parts?: {mail, calendar, contacts}} → as listed
//!     DELETE /v1/outlook             {ok, revoked}
//!
//! Not a Python module.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::connections;
use crate::drives::{self, coded, Api, Fault, Link, Provider, Slot};
use crate::py;
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::web::{self, check_max_items, ApiResult, FieldError, HttpError, Json, Validate};

/// What the sign-in asks (Graph's delegated rights; Kerfy's Entra app lists them): mail read, drafted and tidied
/// (never `Mail.Send`), the calendars read and written, the contacts read.
pub const SCOPES: &str = "offline_access User.Read Mail.ReadWrite Calendars.ReadWrite Contacts.Read";
/// The rights a sign-in must give (lower case, without Graph's address).
const NEEDED: [&str; 3] = ["mail.readwrite", "calendars.readwrite", "contacts.read"];
/// Its name in the agents' tools and on the cards (the same in every language).
pub const LABEL: &str = "Outlook";
/// The mailbox part of its mails' ids (`outlook|<folder>|<n>`).
pub const BOX_ID: &str = "outlook";
/// Graph's ids that never change (a mail moved to another folder keeps its id).
pub const PREFER_IMMUTABLE: &str = "IdType=\"ImmutableId\"";
/// The key of the « Toujours » for adds and changes in Outlook's default calendar (apart from Google Calendar's).
pub const ALWAYS_KEY: &str = crate::person_ok::CALENDAR_WRITE_OUTLOOK;
const AGENTS_MAX: usize = 32;
const FLOW_SECONDS: u64 = 900;
/// Short ids kept at most (the oldest go first).
const IDS_MAX: usize = 20_000;

// -- settings ----------------------------------------------------------------------------------------------------------

/// `[outlook]` of bridge.toml.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub enabled: bool,
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        Settings { enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(false) }
    }
}

// -- what is kept ------------------------------------------------------------------------------------------------------

/// What of the account the chosen agents use (all of it at first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parts {
    pub mail: bool,
    pub calendar: bool,
    pub contacts: bool,
}

impl Default for Parts {
    fn default() -> Parts {
        Parts { mail: true, calendar: true, contacts: true }
    }
}

/// The Microsoft account connected (connections' encrypted store, `outlook`): its tokens, account, agents (`Link`),
/// and what of it they use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Account {
    #[serde(flatten)]
    pub link: Link,
    #[serde(default)]
    pub parts: Parts,
}

pub fn stored(config: &Config) -> Option<Account> {
    connections::store_of(config).read().ok()?.outlook
}

fn shared_with_others(config: &Config, agent: &str) -> bool {
    crate::family_agent::is_family(config, agent) || crate::project_agent::is_project(config, agent)
}

/// What `agent` uses of the account: on, connected (or ended: it is told to ask for a new sign-in), chosen. None:
/// nothing. Never the family's agent nor a project agent (shown to others).
pub fn parts_for(config: &Config, agent: &str) -> Option<Parts> {
    if !config.outlook.enabled || shared_with_others(config, agent) {
        return None;
    }
    stored(config).filter(|a| a.link.agents.iter().any(|x| x == agent)).map(|a| a.parts)
}

pub fn mail_for(config: &Config, agent: &str) -> bool {
    parts_for(config, agent).is_some_and(|p| p.mail)
}

pub fn calendar_for(config: &Config, agent: &str) -> bool {
    parts_for(config, agent).is_some_and(|p| p.calendar)
}

pub fn contacts_for(config: &Config, agent: &str) -> bool {
    parts_for(config, agent).is_some_and(|p| p.contacts)
}

/// The account's address (its name for the agents and the cards), when known.
pub fn account_name(config: &Config) -> Option<String> {
    stored(config).and_then(|a| a.link.account)
}

/// The person's view of it (the app's screen).
pub fn view(config: &Config, account: Option<&Account>) -> Value {
    json!({
        "enabled": config.outlook.enabled,
        "configured": drives::endpoints(config, Provider::Microsoft).is_some(),
        "state": account.map(|a| a.link.state.as_str()).unwrap_or("not_connected"),
        "account": account.and_then(|a| a.link.account.clone()),
        "connected_at": account.map(|a| a.link.connected_at),
        "agents": account.map(|a| a.link.agents.clone()).unwrap_or_default(),
        "parts": account.map(|a| a.parts).unwrap_or_default(),
    })
}

fn off() -> HttpError {
    coded(404, "off", "Outlook is off on this bridge")
}

fn not_connected() -> HttpError {
    coded(404, "not_connected", "not connected")
}

fn not_configured() -> HttpError {
    coded(409, "not_configured", "no Microsoft client on this bridge")
}

/// The agents the person may choose: this place's own, never the family's nor a project's.
fn agents_checked(config: &Config, agents: &[String]) -> ApiResult<Vec<String>> {
    let mut out = Vec::new();
    for name in agents {
        if config.agent(Some(name)).is_none() {
            return Err(coded(422, "agent", &format!("no agent {name:?}")));
        }
        if shared_with_others(config, name) {
            return Err(coded(
                422,
                "agent",
                &format!("{name:?} is shared with others: it can't use a person's mail, calendar or contacts"),
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

/// Microsoft's sign-in address for Outlook (its account chosen in Microsoft's window: `prompt=select_account`).
pub fn authorize_url(ep: &drives::Endpoints, state: &str, verifier: &str) -> String {
    drives::authorize_url_with(ep, state, verifier, drives::Pick::Folders, SCOPES)
}

/// Whether Microsoft gave the three rights (its list, by spaces; `https://graph.microsoft.com/Mail.ReadWrite` or
/// `Mail.ReadWrite`, any case).
pub fn all_given(scopes: &str) -> bool {
    let given: Vec<String> = scopes
        .split([' ', ','])
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    NEEDED.iter().all(|want| given.iter().any(|g| g == want))
}

/// A sign-in for `agents` (the same on iPhone and Android: Microsoft's web sign-in, back to the app's address).
pub fn start(config: &Config, agents: &[String]) -> ApiResult<Value> {
    if !config.outlook.enabled {
        return Err(off());
    }
    let ep = drives::endpoints(config, Provider::Microsoft).ok_or_else(not_configured)?;
    let agents = agents_checked(config, agents)?;
    let state = py::token_hex(16);
    let verifier = py::token_hex(32);
    let url = authorize_url(&ep, &state, &verifier);
    drives::keep_flow(
        &state,
        drives::Flow {
            provider: Provider::Microsoft,
            verifier,
            agents,
            client: String::new(),
            started: std::time::Instant::now(),
            slot: Slot::Outlook,
        },
    );
    Ok(json!({"authorize_url": url, "state": state, "redirect_uri": ep.client.redirect_uri, "expires_in": FLOW_SECONDS}))
}

/// Microsoft's answer when the person's organisation must allow the app first (a work account whose directory lets
/// nobody consent alone: AADSTS65001 « consent required », AADSTS90094 « admin approval needed »).
fn admin_needed(query: &HashMap<String, String>) -> bool {
    let said = format!(
        "{} {}",
        query.get("error").map(String::as_str).unwrap_or(""),
        query.get("error_description").map(String::as_str).unwrap_or("")
    );
    said.contains("consent_required") || said.contains("AADSTS65001") || said.contains("AADSTS90094")
}

/// The callback (the address the web sign-in came back to): the code exchanged, the account named, the agents chosen
/// at start kept (the same account signed in again keeps its agents and parts; another one starts afresh).
pub async fn finish(config: &Config, query: HashMap<String, String>) -> ApiResult<Value> {
    if !config.outlook.enabled {
        return Err(off());
    }
    if query.contains_key("error") {
        if admin_needed(&query) {
            return Err(coded(400, "admin_consent", "the person's organisation must allow BipAgents first"));
        }
        return Err(coded(400, "denied", "the person did not allow it"));
    }
    let flow = query
        .get("state")
        .and_then(|s| drives::take_flow_of(s, |slot| slot == Slot::Outlook))
        .ok_or_else(|| coded(400, "unknown_state", "this sign-in is too old: start again"))?;
    let code = query
        .get("code")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| coded(400, "bad_callback", "the sign-in came back without its code"))?;
    let ep = drives::endpoints(config, Provider::Microsoft).ok_or_else(not_configured)?;
    let form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.clone()),
        ("redirect_uri", ep.client.redirect_uri.clone()),
        ("code_verifier", flow.verifier.clone()),
        ("scope", SCOPES.to_string()),
    ];
    let answer = ep.token_call(&drives::http(), form).await.map_err(|(status, why)| {
        tracing::warn!(status, why = %why, "outlook: code exchange refused");
        coded(502, "exchange_failed", &format!("Microsoft refused the sign-in ({status} {why})"))
    })?;
    let tokens = connections::tokens_of(&answer, None);
    if tokens.scope.as_deref().is_some_and(|s| !all_given(s)) {
        return Err(coded(400, "denied", "mail, calendar and contacts were not all allowed"));
    }
    let api = Api::new(ep, tokens.access_token.clone(), None);
    let account = api.account().await.or_else(|| connections::account_of(&answer));
    let (link, parts, other_account) = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let previous = stored.outlook.take();
        let other_account = previous.as_ref().is_some_and(|p| {
            p.link.account.is_some() && account.is_some() && p.link.account != account
        });
        let same = previous.filter(|_| !other_account);
        let parts = same.as_ref().map(|a| a.parts).unwrap_or_default();
        let mut agents = same.map(|a| a.link.agents).unwrap_or_default();
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
            client: String::new(),
            folders: Vec::new(),
            agents,
            carnet: false,
        };
        stored.outlook = Some(Account { link: link.clone(), parts });
        store.write(&stored)?;
        (link, parts, other_account)
    };
    if other_account {
        // another account: the previous one's short ids, drafts and tidyings are of no use
        forget_local(config);
        revoke_always(config, None).await;
    }
    tracing::info!(agents = link.agents.len(), "outlook: connected");
    Ok(view(config, Some(&Account { link, parts })))
}

/// The agents chosen now (the whole list) and, when given, what they use. Returns the view and the agents whose tools
/// change.
pub async fn set_agents(config: &Config, agents: &[String], parts: Option<Parts>) -> ApiResult<(Value, Vec<String>)> {
    if !config.outlook.enabled {
        return Err(off());
    }
    // an agent deleted since the app listed them: left out, never a refusal (as drives.rs)
    let present: Vec<String> = agents.iter().filter(|a| config.agent(Some(a.as_str())).is_some()).cloned().collect();
    let agents = agents_checked(config, &present)?;
    let (shown, changed, removed, calendar_off) = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let account = stored.outlook.as_mut().ok_or_else(not_connected)?;
        let before = std::mem::replace(&mut account.link.agents, agents.clone());
        let before_parts = account.parts;
        if let Some(parts) = parts {
            account.parts = parts;
        }
        let removed: Vec<String> = before.iter().filter(|a| !agents.contains(a)).cloned().collect();
        let mut changed: Vec<String> =
            agents.iter().filter(|a| !before.contains(a)).chain(removed.iter()).cloned().collect();
        if account.parts != before_parts {
            // what every chosen agent may use changed: their tools too
            for a in agents.iter().chain(before.iter()) {
                if !changed.contains(a) {
                    changed.push(a.clone());
                }
            }
        }
        let calendar_off = before_parts.calendar && !account.parts.calendar;
        let shown = view(config, Some(&*account));
        store.write(&stored)?;
        (shown, changed, removed, calendar_off)
    };
    // their « Toujours » for Outlook's calendar goes with it: chosen again, they are asked again
    if calendar_off {
        revoke_always(config, None).await;
    } else {
        revoke_always(config, Some(&removed)).await;
    }
    Ok((shown, changed))
}

/// Forgets it here (Microsoft can't be asked to revoke a public client's grant). Works with the switch off too.
/// The agents that had it.
pub async fn disconnect(config: &Config) -> ApiResult<(Value, Vec<String>)> {
    let account = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let account = stored.outlook.take().ok_or_else(|| if config.outlook.enabled { not_connected() } else { off() })?;
        store.write(&stored)?;
        account
    };
    revoke_always(config, None).await;
    forget_local(config);
    tracing::info!("outlook: disconnected");
    Ok((json!({"ok": true, "revoked": false}), account.link.agents))
}

/// The « Toujours » of `agents` (None: every agent) for Outlook's calendar taken back, their personalities rendered
/// again.
async fn revoke_always(config: &Config, agents: Option<&[String]>) {
    for agent in crate::approval_level::revoke_always_of(config, ALWAYS_KEY, agents) {
        crate::approval_level::granted_now(config, &agent).await;
    }
}

/// The switch turned off with an account still connected: forgotten at the bridge's start.
pub async fn ended_when_off(config: &Config) -> bool {
    if config.outlook.enabled || stored(config).is_none() {
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
    let Some(account) = stored.outlook.as_mut() else { return };
    let before = account.link.agents.len();
    account.link.agents.retain(|a| a != agent);
    crate::approval_level::revoke_always_of(config, ALWAYS_KEY, Some(&[agent.to_string()]));
    if account.link.agents.len() != before {
        if let Err(e) = store.write(&stored) {
            tracing::warn!(error = %e, "outlook: a deleted agent can't be taken out");
        }
    }
}

// -- Graph -------------------------------------------------------------------------------------------------------------

/// A working Graph access for the account (renewed first when it ends within five minutes).
pub async fn signed_in(config: &Config) -> Result<Api, Fault> {
    drives::access_slot(config, Slot::Outlook).await
}

/// Graph's address (`https://graph.microsoft.com/v1.0`, or bridge.toml's for Microsoft in `[drives]`).
pub fn base(api: &Api) -> String {
    api.ep.api.clone()
}

fn prefer_header(prefer: &[&str]) -> String {
    prefer.join(", ")
}

/// One GET, Graph's `Prefer` asks given (immutable ids, text bodies, a time zone).
pub async fn fetch(api: &Api, url: &str, query: &[(&str, String)], prefer: &[&str]) -> Result<Value, Fault> {
    let url = url.to_string();
    let query: Vec<(String, String)> = query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let prefer = prefer_header(prefer);
    api.json(Box::new(move |http: &reqwest::Client, token: &str| {
        let request = http.get(&url).bearer_auth(token).query(&query);
        if prefer.is_empty() {
            request
        } else {
            request.header("Prefer", prefer.clone())
        }
    }))
    .await
}

/// The next page Graph names, only on Graph's own address (never another host: the bearer would go with it).
pub fn next_page(api: &Api, page: &Value) -> Option<String> {
    let next = page.get("@odata.nextLink").and_then(Value::as_str)?;
    next.starts_with(&format!("{}/", base(api))).then(|| next.to_string())
}

/// The items of a listing, its next pages followed up to `max` items. The bool: more were left.
pub async fn pages(
    api: &Api,
    url: &str,
    query: &[(&str, String)],
    prefer: &[&str],
    max: usize,
) -> Result<(Vec<Value>, bool), Fault> {
    let mut items = Vec::new();
    let mut page = fetch(api, url, query, prefer).await?;
    loop {
        items.extend(page.get("value").and_then(Value::as_array).cloned().unwrap_or_default());
        if items.len() >= max {
            let more = items.len() > max || next_page(api, &page).is_some();
            items.truncate(max);
            return Ok((items, more));
        }
        match next_page(api, &page) {
            Some(next) => page = fetch(api, &next, &[], prefer).await?,
            None => return Ok((items, false)),
        }
    }
}

/// A write (POST, PATCH, DELETE): its answer's JSON (Null when it has none). `etag`: the item as it was read, sent as
/// `If-Match` (Graph answers 412 when it changed since).
pub async fn send(
    api: &Api,
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
    etag: Option<String>,
    prefer: &[&str],
) -> Result<Value, Fault> {
    let url = url.to_string();
    let prefer = prefer_header(prefer);
    let resp = api
        .send(Box::new(move |http: &reqwest::Client, token: &str| {
            let mut request = http.request(method.clone(), &url).bearer_auth(token);
            if !prefer.is_empty() {
                request = request.header("Prefer", prefer.clone());
            }
            if let Some(etag) = &etag {
                request = request.header(reqwest::header::IF_MATCH, etag);
            }
            match &body {
                Some(body) => request.json(body),
                None => request.header(reqwest::header::CONTENT_LENGTH, "0"),
            }
        }))
        .await?;
    Ok(resp.json().await.unwrap_or(Value::Null))
}

/// A file's bytes (an attachment's `$value`), `max` at most.
pub async fn bytes(api: &Api, url: &str, max: u64) -> Result<Vec<u8>, Fault> {
    let url = url.to_string();
    let resp = api.send(Box::new(move |http: &reqwest::Client, token: &str| http.get(&url).bearer_auth(token))).await?;
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(Fault::TooLarge);
    }
    let body = resp.bytes().await.map_err(|_| Fault::Unavailable)?;
    if body.len() as u64 > max {
        return Err(Fault::TooLarge);
    }
    Ok(body.to_vec())
}

/// What goes wrong, to the agent.
pub fn words(fault: &Fault) -> String {
    match fault {
        Fault::Expired => format!(
            "the connection to {LABEL} ended: your person connects it again in the app (Applis et outils › {LABEL})"
        ),
        Fault::NotFound => format!("not found in {LABEL} any more (moved or deleted)"),
        Fault::RateLimited(Some(s)) => format!("{LABEL} asks to slow down: try again in {s} seconds"),
        Fault::RateLimited(None) => format!("{LABEL} asks to slow down: try again in a minute"),
        Fault::Unavailable => format!("{LABEL} has trouble right now: try again later"),
        Fault::Refused => format!("{LABEL} refuses this one"),
        Fault::Unreachable => format!("{LABEL} can't be reached right now"),
        Fault::Changed => format!("changed in {LABEL} since it was read: nothing was changed; read it again"),
        Fault::MissingScope => {
            format!("{LABEL} hasn't given BipAgents this right: your person reconnects it in the app")
        }
        Fault::TooLarge => "too large".to_string(),
        Fault::Other(why) => why.clone(),
    }
}

/// A fault as the tool's answer (`plain`: what the agent may tell its person).
pub fn fault_result(fault: &Fault, plain: &str) -> crate::Dict {
    match fault {
        Fault::NotFound | Fault::Changed | Fault::TooLarge | Fault::Refused => tool_text(&words(fault), true),
        Fault::Expired => tool_failure(&words(fault), "Your Outlook needs to be connected again in the app."),
        _ => tool_failure(&words(fault), plain),
    }
}

// -- short ids ---------------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct Ids {
    next: u64,
    ids: BTreeMap<u64, String>,
}

static IDS_LOCK: Mutex<()> = Mutex::new(());

fn folder(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("outlook")
}

fn ids_path(config: &Config) -> PathBuf {
    folder(config).join("ids.json")
}

fn load_ids(config: &Config) -> Ids {
    std::fs::read(ids_path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Writes one of this module's files (its folder 0700, the file 0600, by a rename).
pub fn write_file(config: &Config, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = folder(config);
    py::makedirs(&dir.to_string_lossy(), 0o700)?;
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    py::write_with_mode(&tmp.to_string_lossy(), bytes, 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// One of this module's files, read (None: none yet).
pub fn read_file(config: &Config, name: &str) -> Option<Vec<u8>> {
    std::fs::read(folder(config).join(name)).ok()
}

/// The short numbers of Graph's ids, made for those new (the same id always gets the same number).
pub fn short_ids(config: &Config, longs: &[String]) -> Vec<u64> {
    let _guard = IDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ids = load_ids(config);
    let mut known: HashMap<String, u64> = ids.ids.iter().map(|(n, id)| (id.clone(), *n)).collect();
    let mut changed = false;
    let out = longs
        .iter()
        .map(|long| match known.get(long) {
            Some(n) => *n,
            None => {
                ids.next += 1;
                let n = ids.next;
                ids.ids.insert(n, long.clone());
                known.insert(long.clone(), n);
                changed = true;
                n
            }
        })
        .collect();
    if changed {
        while ids.ids.len() > IDS_MAX {
            let Some(oldest) = ids.ids.keys().next().copied() else { break };
            ids.ids.remove(&oldest);
        }
        if let Ok(bytes) = serde_json::to_vec(&ids) {
            if let Err(e) = write_file(config, "ids.json", &bytes) {
                tracing::warn!(error = %e, "outlook: short ids not kept");
            }
        }
    }
    out
}

pub fn short_id(config: &Config, long: &str) -> u64 {
    short_ids(config, &[long.to_string()]).first().copied().unwrap_or(0)
}

/// Graph's id of a short number. None: not one of this account's.
pub fn long_id(config: &Config, n: u64) -> Option<String> {
    let _guard = IDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_ids(config).ids.get(&n).cloned()
}

/// The ids, drafts and tidyings of an account no longer connected.
fn forget_local(config: &Config) {
    let _guard = IDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_dir_all(folder(config));
    crate::outlook_mail::forget_scans();
    crate::outlook_contacts::forget_cache();
}

// -- routes ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartBody {
    #[serde(default)]
    pub agents: Vec<String>,
}

impl Validate for StartBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "agents", Some(&self.agents), AGENTS_MAX);
    }
}

#[derive(Debug, Deserialize)]
pub struct CallbackBody {
    #[serde(default)]
    pub url: String,
}

impl Validate for CallbackBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "url", Some(&self.url), 8000);
    }
}

#[derive(Debug, Deserialize)]
pub struct AgentsBody {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub parts: Option<Parts>,
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
    if !config.outlook.enabled {
        return Err(off());
    }
    Ok(web::ok(view(&config, stored(&config).as_ref())))
}

async fn start_route(State(state): State<AppState>, Json(body): Json<StartBody>) -> ApiResult<Response> {
    Ok(web::ok(start(&state.config(), &body.agents)?))
}

async fn callback_route(State(state): State<AppState>, Json(body): Json<CallbackBody>) -> ApiResult<Response> {
    let config = state.config();
    let shown = finish(&config, connections::callback_query(&body.url)).await?;
    let agents: Vec<String> =
        shown["agents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    tools_changed(&config, &agents).await;
    Ok(web::ok(shown))
}

async fn agents_route(State(state): State<AppState>, Json(body): Json<AgentsBody>) -> ApiResult<Response> {
    let config = state.config();
    let (shown, changed) = set_agents(&config, &body.agents, body.parts).await?;
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
        .route("/outlook", get(listing_route).delete(disconnect_route))
        .route("/outlook/start", post(start_route))
        .route("/outlook/callback", post(callback_route))
        .route("/outlook/agents", put(agents_route))
}

#[cfg(test)]
#[path = "outlook_tests.rs"]
mod tests;
