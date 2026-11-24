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
//! in the connections' encrypted store (`Stored.outlook`), renewed as OneDrive's (`drives::Slot::Outlook`). The sign-in
//! asks only the rights of what the person switched on (« Mails », « Agenda », « Contacts », [`scopes_for`]: as
//! OneDrive asks its write right only once an agent writes); a sign-in that leaves one out is refused; a part switched
//! on later that Microsoft hasn't given yet is `missing` until « Reconnecter » (its agents don't get it before).
//! No token ever reaches Hermes nor the model: the bridge calls
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
//!     GET    /v1/outlook             {enabled, configured, state, account, connected_at, agents, parts, missing,
//!                                     outside: {agent: provider's label}}
//!     POST   /v1/outlook/start       {agents, parts?} → {authorize_url, state, redirect_uri, expires_in}
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

/// What a sign-in with every part asks (Graph's delegated rights; Kerfy's Entra app lists them): mail read, drafted
/// and tidied (never `Mail.Send`), the calendars read and written, the contacts read. Always the base ones
/// ([`BASE_SCOPES`]), then each part's ([`scopes_for`]).
pub const SCOPES: &str = "offline_access User.Read Mail.ReadWrite Calendars.ReadWrite Contacts.Read";
pub const BASE_SCOPES: &str = "offline_access User.Read";
/// Each part's right: (part, scope).
const PART_SCOPES: [(&str, &str); 3] = [("mail", "Mail.ReadWrite"), ("calendar", "Calendars.ReadWrite"), ("contacts", "Contacts.Read")];
/// The rights a full sign-in must give (lower case, without Graph's address).
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

impl Parts {
    pub fn get(self, key: &str) -> bool {
        match key {
            "mail" => self.mail,
            "calendar" => self.calendar,
            _ => self.contacts,
        }
    }

    /// Only what both have.
    pub fn and(self, other: Parts) -> Parts {
        Parts { mail: self.mail && other.mail, calendar: self.calendar && other.calendar, contacts: self.contacts && other.contacts }
    }

    pub fn any(self) -> bool {
        self.mail || self.calendar || self.contacts
    }

    /// The parts on here and not in `given` (`mail`, `calendar`, `contacts`).
    pub fn missing(self, given: Parts) -> Vec<&'static str> {
        PART_SCOPES.iter().map(|(key, _)| *key).filter(|key| self.get(key) && !given.get(key)).collect()
    }
}

/// The rights a sign-in asks for `parts`: the base ones, then each part's on.
pub fn scopes_for(parts: Parts) -> String {
    let mut scopes = BASE_SCOPES.to_string();
    for (key, scope) in PART_SCOPES {
        if parts.get(key) {
            scopes.push(' ');
            scopes.push_str(scope);
        }
    }
    scopes
}

fn scope_words(scopes: &str) -> Vec<String> {
    scopes
        .split([' ', ','])
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The parts Microsoft gave, by the rights a token's answer lists. None listed: none (review R12: closed by default;
/// Microsoft always lists them, and a renewal without the list keeps the sign-in's, connections.rs `tokens_of`).
pub fn granted(tokens: &connections::Tokens) -> Parts {
    let Some(scopes) = tokens.scope.as_deref() else { return Parts { mail: false, calendar: false, contacts: false } };
    let words = scope_words(scopes);
    let has = |key: &str| {
        let wanted = PART_SCOPES.iter().find(|(k, _)| *k == key).map(|(_, s)| s.to_ascii_lowercase()).unwrap_or_default();
        words.contains(&wanted)
    };
    Parts { mail: has("mail"), calendar: has("calendar"), contacts: has("contacts") }
}

/// What the agents get of the account: what the person switched on, among what Microsoft gave.
pub fn effective(account: &Account) -> Parts {
    account.parts.and(granted(&account.link.tokens))
}

/// The rights a renewal asks (Microsoft: those of the sign-in or fewer, never more): the parts the agents get.
pub fn renewal_scope(stored: &connections::Stored) -> String {
    scopes_for(stored.outlook.as_ref().map(effective).unwrap_or(Parts { mail: false, calendar: false, contacts: false }))
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

/// Whether what `agent` reads of the account stays with a model that keeps nothing (review R13; choice B, Alex 24/11):
/// false for a model outside or one the bridge cannot tell (`outside_model::place`, read at every call: a model
/// changed by SSH counts at once, before the shared memory's guard has seen it), unless the person turned Outlook back
/// on for it on that provider (« Rallumer Outlook ? », agent_shared_memory::outlook_allowed). An agent off this way
/// stays chosen: back on a model that is not outside, it has Outlook again as the person had it.
pub fn kept_on_the_place(config: &Config, agent: &str) -> bool {
    let Some(found) = config.agent(Some(agent)) else { return false };
    crate::agent_shared_memory::outlook_allowed(config, found)
}

/// Whether `agent` is among the account's chosen agents (whether it uses it now or not).
pub fn chosen(config: &Config, agent: &str) -> bool {
    stored(config).is_some_and(|a| a.link.agents.iter().any(|x| x == agent))
}

/// Whether `agent` is chosen but off for its model ([`kept_on_the_place`] false): the apps say why, and ask
/// « Rallumer Outlook ? » before choosing it again.
pub fn held(config: &Config, agent: &str) -> bool {
    config.outlook.enabled && config.agent(Some(agent)).is_some() && !kept_on_the_place(config, agent) && chosen(config, agent)
}

/// An agent of the place off for its model now (outside or unknown, not allowed by the person: [`kept_on_the_place`]).
fn held_now(config: &Config, agent: &str) -> bool {
    config.agent(Some(agent)).is_some() && !kept_on_the_place(config, agent)
}

/// The agents the person may choose that are on an outside model and don't get Outlook for it: {name: provider's
/// label} (the apps' line under its row, and « Rallumer Outlook ? » before choosing it).
fn outside_agents(config: &Config) -> serde_json::Map<String, Value> {
    config
        .agents
        .values()
        .filter(|a| !shared_with_others(config, &a.name) && !crate::agent_shared_memory::outlook_allowed(config, a))
        .filter_map(|a| crate::agent_shared_memory::outside_now(config, a).map(|(_, label)| (a.name.clone(), json!(label))))
        .collect()
}

/// After the person's choice (`set_agents`, `finish`): each agent of `chosen` still off for its model was chosen with
/// « Rallumer Outlook ? » answered (the apps never send one otherwise): allowed on its provider. Each of `removed`
/// loses what was allowed. The agents allowed now.
fn person_chose(config: &Config, chosen: &[String], removed: &[String]) -> Vec<String> {
    let mut allowed = Vec::new();
    for name in chosen {
        if let Some(agent) = config.agent(Some(name)).filter(|_| held_now(config, name)) {
            crate::agent_shared_memory::outlook_turned_on(config, agent);
            allowed.push(name.clone());
        }
    }
    for name in removed {
        crate::agent_shared_memory::outlook_turned_off(config, name);
    }
    allowed
}

/// After the person's « Oui » (review C4): the account still connected and the same one as when the card was made
/// (`account`), `part` still on and given, the agent still chosen. Err: what to tell the agent (nothing written).
pub fn still_allowed(config: &Config, agent: &str, part: &str, account: Option<&str>) -> Result<(), &'static str> {
    let now = stored(config);
    let same = now.as_ref().is_some_and(|a| a.link.state == "connected" && a.link.account.as_deref() == account);
    if !same || !parts_for(config, agent).is_some_and(|p| p.get(part)) {
        return Err(TAKEN_BACK);
    }
    Ok(())
}

pub const TAKEN_BACK: &str = "Not done: while the card waited, your person disconnected Outlook or took this away from \
you: nothing was written. Tell them plainly; ask again only once they connect it again for you.";

/// What `agent` uses of the account: on, connected (or ended: it is told to ask for a new sign-in), chosen. None:
/// nothing. Never the family's agent nor a project agent (shown to others).
pub fn parts_for(config: &Config, agent: &str) -> Option<Parts> {
    if !config.outlook.enabled || shared_with_others(config, agent) || !kept_on_the_place(config, agent) {
        return None;
    }
    stored(config).filter(|a| a.link.agents.iter().any(|x| x == agent)).map(|a| effective(&a))
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
        // the agents that use it now: one chosen but off for its model is in `outside` (choice B)
        "agents": account.map(|a| a.link.agents.iter().filter(|x| !held_now(config, x)).cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "outside": outside_agents(config),
        "parts": account.map(|a| a.parts).unwrap_or_default(),
        // switched on and not given by Microsoft yet: « Reconnecter » asks it
        "missing": account.map(|a| a.parts.missing(granted(&a.link.tokens))).unwrap_or_default(),
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

/// Microsoft's sign-in address for Outlook, asking `parts`' rights (its account chosen in Microsoft's window:
/// `prompt=select_account`).
pub fn authorize_url(ep: &drives::Endpoints, state: &str, verifier: &str, parts: Parts) -> String {
    drives::authorize_url_with(ep, state, verifier, drives::Pick::Folders, &scopes_for(parts))
}

/// Whether Microsoft gave the three rights (its list, by spaces; `https://graph.microsoft.com/Mail.ReadWrite` or
/// `Mail.ReadWrite`, any case).
pub fn all_given(scopes: &str) -> bool {
    let given = scope_words(scopes);
    NEEDED.iter().all(|want| given.iter().any(|g| g == want))
}

/// Whether Microsoft gave every right `parts` asked.
pub fn given_for(scopes: &str, parts: Parts) -> bool {
    parts.missing(granted(&connections::Tokens { scope: Some(scopes.to_string()), ..Default::default() })).is_empty()
}

/// The parts a sign-in under way asks, by its state (drives' `Flow` has no room for them).
static ASKED: std::sync::LazyLock<Mutex<HashMap<String, (std::time::Instant, Parts)>>> =
    std::sync::LazyLock::new(Default::default);

/// A sign-in for `agents` (the same on iPhone and Android: Microsoft's web sign-in, back to the app's address),
/// asking the rights of `parts` (none given: those kept for the account, else all of them).
pub fn start(config: &Config, agents: &[String], parts: Option<Parts>) -> ApiResult<Value> {
    if !config.outlook.enabled {
        return Err(off());
    }
    let ep = drives::endpoints(config, Provider::Microsoft).ok_or_else(not_configured)?;
    let agents = agents_checked(config, agents)?;
    let parts = parts.or_else(|| stored(config).map(|a| a.parts)).unwrap_or_default();
    if !parts.any() {
        return Err(coded(422, "parts", "switch on mail, calendar or contacts"));
    }
    let state = py::token_hex(16);
    let verifier = py::token_hex(32);
    let url = authorize_url(&ep, &state, &verifier, parts);
    {
        let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
        asked.retain(|_, (at, _)| at.elapsed().as_secs() < FLOW_SECONDS);
        asked.insert(state.clone(), (std::time::Instant::now(), parts));
    }
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
    let asked = query
        .get("state")
        .and_then(|s| ASKED.lock().unwrap_or_else(|e| e.into_inner()).remove(s))
        .map(|(_, parts)| parts)
        .unwrap_or_default();
    let form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.clone()),
        ("redirect_uri", ep.client.redirect_uri.clone()),
        ("code_verifier", flow.verifier.clone()),
        ("scope", scopes_for(asked)),
    ];
    let answer = ep.token_call(&drives::http(), form).await.map_err(|(status, why)| {
        tracing::warn!(status, why = %why, "outlook: code exchange refused");
        coded(502, "exchange_failed", &format!("Microsoft refused the sign-in ({status} {why})"))
    })?;
    let tokens = connections::tokens_of(&answer, None);
    if !tokens.scope.as_deref().is_some_and(|s| given_for(s, asked)) {
        return Err(coded(400, "denied", "Microsoft did not give every right asked"));
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
        // chosen in the app while off for its model: « Rallumer Outlook ? » answered (one the app did not ask, allowed
        // for the previous account only, is not)
        let answered: Vec<String> = flow.agents.iter().filter(|a| held_now(config, a)).cloned().collect();
        if other_account {
            // another account: what the person allowed for the previous one is asked again (review B1)
            for a in previous.iter().flat_map(|p| &p.link.agents) {
                crate::agent_shared_memory::outlook_turned_off(config, a);
            }
        }
        let same = previous.filter(|_| !other_account);
        // what was asked is what they use (the app sends what the person switched on)
        let parts = asked;
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
        person_chose(config, &answered, &[]);
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
        // one off for its model, that the app showed off and so left out: still chosen (back on a model that is not
        // outside, it has it again as the person had it; choice B)
        let asked = agents.clone();
        let mut agents = agents.clone();
        for a in &account.link.agents {
            if !agents.contains(a) && held_now(config, a) {
                agents.push(a.clone());
            }
        }
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
        store.write(&stored)?;
        // « Rallumer Outlook ? » answered for an agent on an outside model: its tools change too
        for a in person_chose(config, &asked, &removed) {
            if !changed.contains(&a) {
                changed.push(a);
            }
        }
        let shown = view(config, stored.outlook.as_ref());
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
        // « Rallumer Outlook ? » goes with it: connected again, maybe with another account, it is asked again (review B1)
        for a in &account.link.agents {
            crate::agent_shared_memory::outlook_turned_off(config, a);
        }
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Ids {
    next: u64,
    ids: BTreeMap<u64, String>,
}

/// A place's table, kept in memory (review of outlook C3: read once, never again for each item), written by a lot:
/// `dirty` until the writer thread has written it.
#[derive(Debug, Default)]
struct Table {
    ids: Ids,
    by_long: HashMap<String, u64>,
    dirty: bool,
    /// A write is on its way (one thread at a time per place).
    writing: bool,
    /// Bumped when the account's local data is forgotten: a write started before never lands after.
    generation: u64,
}

/// The tables, by place (its data folder: tests run several bridges in one process). Only memory under this lock:
/// the file is read once and written by a thread of its own.
static TABLES: std::sync::LazyLock<Mutex<HashMap<String, Table>>> = std::sync::LazyLock::new(Default::default);
/// The file's writes and its removal, one at a time (the writer thread, `flush_ids`, `forget_local`).
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// How long the writer waits for the rest of a lot before writing.
const WRITE_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

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
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), py::token_hex(4)));
    py::write_with_mode(&tmp.to_string_lossy(), bytes, 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// One of this module's files, read (None: none yet).
pub fn read_file(config: &Config, name: &str) -> Option<Vec<u8>> {
    std::fs::read(folder(config).join(name)).ok()
}

/// `f` on the place's table (read from its file the first time).
fn with_table<T>(config: &Config, f: impl FnOnce(&mut Table) -> T) -> T {
    let mut tables = TABLES.lock().unwrap_or_else(|e| e.into_inner());
    let table = tables.entry(config.data_dir()).or_insert_with(|| {
        let ids = load_ids(config);
        let by_long = ids.ids.iter().map(|(n, id)| (id.clone(), *n)).collect();
        Table { ids, by_long, ..Table::default() }
    });
    f(table)
}

/// The table written in the background (a thread: callers never wait on the disk, in or out of the runtime). One
/// writer per place; what changes meanwhile is written by the same thread's next round.
fn write_later(config: &Config) {
    let start = with_table(config, |t| {
        let start = t.dirty && !t.writing;
        if start {
            t.writing = true;
        }
        start
    });
    if !start {
        return;
    }
    let config = config.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(WRITE_AFTER);
        let lot = with_table(&config, |t| {
            if !t.dirty {
                t.writing = false;
                return None;
            }
            t.dirty = false;
            Some((t.ids.clone(), t.generation))
        });
        let Some((ids, generation)) = lot else { return };
        let Ok(bytes) = serde_json::to_vec(&ids) else { continue };
        let _writing = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // forgotten meanwhile (disconnected): never written again
        if with_table(&config, |t| t.generation) != generation {
            continue;
        }
        if let Err(e) = write_file(&config, "ids.json", &bytes) {
            tracing::warn!(error = %e, "outlook: short ids not kept");
        }
    });
}

/// Writes the table now if it changed (tests; the writer thread does it otherwise).
pub fn flush_ids(config: &Config) {
    let _writing = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let lot = with_table(config, |t| {
        let lot = t.dirty.then(|| t.ids.clone());
        t.dirty = false;
        lot
    });
    if let Some(bytes) = lot.and_then(|ids| serde_json::to_vec(&ids).ok()) {
        if let Err(e) = write_file(config, "ids.json", &bytes) {
            tracing::warn!(error = %e, "outlook: short ids not kept");
        }
    }
}

/// The short numbers of Graph's ids, made for those new (the same id always gets the same number). In memory; the
/// new ones are written by a lot, in the background.
pub fn short_ids(config: &Config, longs: &[String]) -> Vec<u64> {
    let (out, changed) = with_table(config, |t| {
        let mut changed = false;
        let out: Vec<u64> = longs
            .iter()
            .map(|long| match t.by_long.get(long) {
                Some(n) => *n,
                None => {
                    t.ids.next += 1;
                    let n = t.ids.next;
                    t.ids.ids.insert(n, long.clone());
                    t.by_long.insert(long.clone(), n);
                    changed = true;
                    n
                }
            })
            .collect();
        if changed {
            while t.ids.ids.len() > IDS_MAX {
                let Some((oldest, long)) = t.ids.ids.pop_first() else { break };
                if t.by_long.get(&long) == Some(&oldest) {
                    t.by_long.remove(&long);
                }
            }
            t.dirty = true;
        }
        (out, changed)
    });
    if changed {
        write_later(config);
    }
    out
}

pub fn short_id(config: &Config, long: &str) -> u64 {
    short_ids(config, &[long.to_string()]).first().copied().unwrap_or(0)
}

/// Graph's id of a short number. None: not one of this account's.
pub fn long_id(config: &Config, n: u64) -> Option<String> {
    with_table(config, |t| t.ids.ids.get(&n).cloned())
}

/// The ids, drafts and tidyings of an account no longer connected (a write on its way never lands after).
fn forget_local(config: &Config) {
    let _writing = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_table(config, |t| {
        t.ids = Ids::default();
        t.by_long.clear();
        t.dirty = false;
        t.generation += 1;
    });
    let _ = std::fs::remove_dir_all(folder(config));
    crate::outlook_mail::forget_scans();
    crate::outlook_contacts::forget_cache();
}

// -- routes ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartBody {
    #[serde(default)]
    pub agents: Vec<String>,
    /// What they will use (an older app sends none: those kept, else all of them).
    #[serde(default)]
    pub parts: Option<Parts>,
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
    Ok(web::ok(start(&state.config(), &body.agents, body.parts)?))
}

async fn callback_route(State(state): State<AppState>, Json(body): Json<CallbackBody>) -> ApiResult<Response> {
    let config = state.config();
    // the agents before (another account signed in: theirs change too, review R11), and after
    let before = stored(&config).map(|a| a.link.agents).unwrap_or_default();
    let shown = finish(&config, connections::callback_query(&body.url)).await?;
    let mut agents: Vec<String> =
        shown["agents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    for a in before {
        if !agents.contains(&a) {
            agents.push(a);
        }
    }
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
