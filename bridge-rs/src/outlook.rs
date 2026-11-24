//! « Outlook » (Alex 23/11, « Oui, Outlook ensuite »): a Microsoft account — a personal one (Outlook.com, Hotmail) or
//! a work one — gives its mail, its Outlook calendar and its contacts to the agents the person chooses, through the
//! tools those agents already have, never new ones:
//!
//! - the mail tools (outlook_mail.rs): `mail_search`, `mail_read`, `mail_attachment`, `mail_draft` and the tidying
//!   `mail_overview` / `mail_bulk`, on a mailbox named « Outlook » beside the IMAP ones (mailbox.rs). Nothing is ever
//!   sent: a draft waits in Outlook's Drafts for the person; a tidying (archive, move, mark read, trash) follows the
//!   person's approval level as an IMAP box's (1.1: done at once under « Seulement le critique », its line with
//!   « Annuler »; else its card, person_ok.rs); its standing sort rules are the IMAP boxes' (mail_rules.rs), and its
//!   new mail starts the routines that wait for one (outlook_watch.rs);
//! - the calendar tools (outlook_calendar.rs): its calendars listed beside the phone's, the mailboxes' and Google's;
//!   every add or change asked on the same card as Google Calendar's (`calendar_write`, « Toujours autoriser » for the
//!   default calendar only, kept apart from Google's), every deletion asked each time (`calendar_delete`);
//! - the contacts' tools (outlook_contacts.rs): `contacts_search`, `contacts_birthdays`, `contacts_list_group`
//!   (`Contacts.Read`), and once « Ajouter et changer des contacts » is on, `contacts_add` / `contacts_update` on cards
//!   by the approval level (`Contacts.ReadWrite`; never a deletion).
//!
//! Several Microsoft accounts per place (Alex 24/11, `MAX_ACCOUNTS`): each its number (`id`), its own tokens, parts,
//! agents, « Toujours » for its default calendar ([`always_key`]) and « Déconnecter »; a second one never breaks the
//! first. The first is kept where a bridge before these additions reads it (`Stored.outlook`), the others beside
//! (`Stored.outlook_more`); the view keeps the first one in its old fields, so an app that knows one account stays
//! right. With several, the agents' answers and the cards name each account (« Outlook (lea@outlook.com) »).
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
//!                                      accounts: [{id, mailbox, state, account, connected_at, agents, parts,
//!                                      missing}], max_accounts}   (the old fields: the first account)
//!     POST   /v1/outlook/start       {agents, parts?, account?} → {authorize_url, state, redirect_uri, expires_in}
//!     POST   /v1/outlook/callback    {url} → as listed  (400 denied | admin_consent | unknown_state | bad_callback,
//!                                      409 too_many)
//!     PUT    /v1/outlook/agents      {agents, parts?: {mail, calendar, contacts, contacts_write?}, account?} → as
//!                                      listed  (no `account`: the first; no `contacts_write`: kept)
//!     DELETE /v1/outlook[?account=]  {ok, revoked, account}  (no `account`: the first)
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
/// and tidied (never `Mail.Send`), the calendars read and written, the contacts read and written (1.1: added and
/// changed, never deleted). Always the base ones ([`BASE_SCOPES`]), then each part's ([`scopes_for`]).
pub const SCOPES: &str = "offline_access User.Read Mail.ReadWrite Calendars.ReadWrite Contacts.ReadWrite";
pub const BASE_SCOPES: &str = "offline_access User.Read";
pub const MAIL_SCOPE: &str = "Mail.ReadWrite";
pub const CALENDAR_SCOPE: &str = "Calendars.ReadWrite";
/// The contacts read only (« Contacts » on, « Ajouter et changer des contacts » off).
pub const CONTACTS_SCOPE: &str = "Contacts.Read";
/// The contacts read and written (« Ajouter et changer des contacts » on): asked instead of `Contacts.Read`.
pub const CONTACTS_WRITE_SCOPE: &str = "Contacts.ReadWrite";
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

/// An account's number on this place (1 for the first one ever connected; never given again, `Stored.outlook_last`).
pub type AccountId = u32;
/// The first account's number (an account kept before 1.1 has none: it is this one).
pub const FIRST: AccountId = 1;
/// Microsoft accounts one place keeps at most (Alex 24/11: several per place).
pub const MAX_ACCOUNTS: usize = 5;

fn first_id() -> AccountId {
    FIRST
}

/// What of the account the chosen agents use (mail, calendar and contacts at first; the contacts written only once
/// switched on, and only with the contacts read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parts {
    pub mail: bool,
    pub calendar: bool,
    pub contacts: bool,
    /// Outlook's contacts added and changed (never deleted), on cards by the approval level (outlook_contacts.rs).
    pub contacts_write: bool,
}

impl Default for Parts {
    fn default() -> Parts {
        Parts { mail: true, calendar: true, contacts: true, contacts_write: false }
    }
}

/// The parts' keys, in their order on the screen.
pub const PART_KEYS: [&str; 4] = ["mail", "calendar", "contacts", "contacts_write"];

impl Parts {
    pub const NONE: Parts = Parts { mail: false, calendar: false, contacts: false, contacts_write: false };

    pub fn get(self, key: &str) -> bool {
        match key {
            "mail" => self.mail,
            "calendar" => self.calendar,
            "contacts" => self.contacts,
            "contacts_write" => self.contacts_write && self.contacts,
            _ => false,
        }
    }

    /// Only what both have.
    pub fn and(self, other: Parts) -> Parts {
        Parts {
            mail: self.mail && other.mail,
            calendar: self.calendar && other.calendar,
            contacts: self.contacts && other.contacts,
            contacts_write: self.get("contacts_write") && other.get("contacts_write"),
        }
    }

    /// What either has (the parts an agent gets of all its accounts, for its tools' list).
    pub fn or(self, other: Parts) -> Parts {
        Parts {
            mail: self.mail || other.mail,
            calendar: self.calendar || other.calendar,
            contacts: self.contacts || other.contacts,
            contacts_write: self.get("contacts_write") || other.get("contacts_write"),
        }
    }

    pub fn any(self) -> bool {
        self.mail || self.calendar || self.contacts
    }

    /// The parts on here and not in `given` (`mail`, `calendar`, `contacts`, `contacts_write`).
    pub fn missing(self, given: Parts) -> Vec<&'static str> {
        PART_KEYS.iter().copied().filter(|key| self.get(key) && !given.get(key)).collect()
    }
}

/// The parts sent by an app: `contacts_write` left out (an app before 1.1's additions) keeps what was kept.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct PartsBody {
    #[serde(default)]
    pub mail: Option<bool>,
    #[serde(default)]
    pub calendar: Option<bool>,
    #[serde(default)]
    pub contacts: Option<bool>,
    #[serde(default)]
    pub contacts_write: Option<bool>,
}

impl PartsBody {
    /// The parts it says, over `kept` for what it leaves out.
    pub fn over(self, kept: Parts) -> Parts {
        Parts {
            mail: self.mail.unwrap_or(kept.mail),
            calendar: self.calendar.unwrap_or(kept.calendar),
            contacts: self.contacts.unwrap_or(kept.contacts),
            contacts_write: self.contacts_write.unwrap_or(kept.contacts_write),
        }
    }
}

/// The rights a sign-in asks for `parts`: the base ones, then each part's on (the contacts: `Contacts.ReadWrite` when
/// they are written, else `Contacts.Read`).
pub fn scopes_for(parts: Parts) -> String {
    let mut scopes = BASE_SCOPES.to_string();
    for (key, scope) in [("mail", MAIL_SCOPE), ("calendar", CALENDAR_SCOPE)] {
        if parts.get(key) {
            scopes.push(' ');
            scopes.push_str(scope);
        }
    }
    if parts.get("contacts_write") {
        scopes.push(' ');
        scopes.push_str(CONTACTS_WRITE_SCOPE);
    } else if parts.contacts {
        scopes.push(' ');
        scopes.push_str(CONTACTS_SCOPE);
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
    let Some(scopes) = tokens.scope.as_deref() else { return Parts::NONE };
    let words = scope_words(scopes);
    let has = |scope: &str| words.contains(&scope.to_ascii_lowercase());
    Parts {
        mail: has(MAIL_SCOPE),
        calendar: has(CALENDAR_SCOPE),
        contacts: has(CONTACTS_SCOPE) || has(CONTACTS_WRITE_SCOPE),
        contacts_write: has(CONTACTS_WRITE_SCOPE),
    }
}

/// What the agents get of the account: what the person switched on, among what Microsoft gave.
pub fn effective(account: &Account) -> Parts {
    account.parts.and(granted(&account.link.tokens))
}

/// The rights a renewal of account `id` asks (Microsoft: those of the sign-in or fewer, never more): the parts its
/// agents get.
pub fn renewal_scope(stored: &connections::Stored, id: AccountId) -> String {
    scopes_for(account_in(stored, id).map(effective).unwrap_or(Parts::NONE))
}

/// A Microsoft account connected (connections' encrypted store, `outlook` then `outlook_more`): its number, tokens,
/// address, agents (`Link`), and what of it they use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Account {
    #[serde(flatten)]
    pub link: Link,
    #[serde(default)]
    pub parts: Parts,
    #[serde(default = "first_id")]
    pub id: AccountId,
}

/// The accounts kept, the first one first (`Stored.outlook`: the one a bridge before 1.1's additions reads too).
pub fn accounts_in(stored: &connections::Stored) -> Vec<&Account> {
    stored.outlook.iter().chain(stored.outlook_more.iter()).collect()
}

pub fn account_in(stored: &connections::Stored, id: AccountId) -> Option<&Account> {
    accounts_in(stored).into_iter().find(|a| a.id == id)
}

pub fn account_in_mut(stored: &mut connections::Stored, id: AccountId) -> Option<&mut Account> {
    if stored.outlook.as_ref().is_some_and(|a| a.id == id) {
        return stored.outlook.as_mut();
    }
    stored.outlook_more.iter_mut().find(|a| a.id == id)
}

/// Account `id` taken out of the store; the next one becomes the first (what an older bridge reads).
fn take_account(stored: &mut connections::Stored, id: AccountId) -> Option<Account> {
    if stored.outlook.as_ref().is_some_and(|a| a.id == id) {
        let taken = stored.outlook.take();
        if !stored.outlook_more.is_empty() {
            stored.outlook = Some(stored.outlook_more.remove(0));
        }
        return taken;
    }
    let at = stored.outlook_more.iter().position(|a| a.id == id)?;
    Some(stored.outlook_more.remove(at))
}

fn push_account(stored: &mut connections::Stored, account: Account) {
    if stored.outlook.is_none() {
        stored.outlook = Some(account);
    } else {
        stored.outlook_more.push(account);
    }
}

/// Every account kept, the first one first.
pub fn all(config: &Config) -> Vec<Account> {
    connections::store_of(config).read().map(|s| accounts_in(&s).into_iter().cloned().collect()).unwrap_or_default()
}

/// Account `id`.
pub fn one(config: &Config, id: AccountId) -> Option<Account> {
    all(config).into_iter().find(|a| a.id == id)
}

/// The first account (the one an app before 1.1's additions shows).
pub fn stored(config: &Config) -> Option<Account> {
    all(config).into_iter().next()
}

/// The mailbox's id of account `id` for the routines and the standing rules (triggers.rs, mail_rules.rs): never an
/// IMAP box's (theirs are random hex).
pub fn mailbox_key(id: AccountId) -> String {
    format!("{BOX_ID}-{id}")
}

/// The account a mailbox's id names (`outlook-2`). None: an IMAP box's.
pub fn key_account(key: &str) -> Option<AccountId> {
    key.strip_prefix(&format!("{BOX_ID}-"))?.parse().ok()
}

/// The key of the « Toujours » for adds and changes in account `id`'s default calendar (the first account's is
/// `calendar_write:outlook`, as before; another's `calendar_write:outlook:<id>`): apart from Google Calendar's, and
/// each account's apart from the others'.
pub fn always_key(id: AccountId) -> String {
    if id == FIRST {
        ALWAYS_KEY.to_string()
    } else {
        format!("{ALWAYS_KEY}:{id}")
    }
}

/// The account an `always_key` is of. None: not one of Outlook's.
pub fn always_account(key: &str) -> Option<AccountId> {
    if key == ALWAYS_KEY {
        return Some(FIRST);
    }
    key.strip_prefix(&format!("{ALWAYS_KEY}:"))?.parse().ok().filter(|n| *n > 0)
}

fn shared_with_others(config: &Config, agent: &str) -> bool {
    crate::family_agent::is_family(config, agent) || crate::project_agent::is_project(config, agent)
}

/// Whether `agent` is « protected »: what it reads goes only to a model that keeps nothing (review R13). The hook for
/// the shared-memory branch (`outside_model::place`): when both meet, this returns false for a model outside or
/// unknown, and Outlook is off for that agent by default, as the shared memory and the Carnet are. Until then, every
/// agent of the place is.
pub fn kept_on_the_place(config: &Config, agent: &str) -> bool {
    let _ = (config, agent);
    true
}

/// After the person's « Oui » (review C4): account `id` still connected and the same one as when the card was made
/// (`address`), `part` still on and given, the agent still chosen. Err: what to tell the agent (nothing written).
pub fn still_allowed(config: &Config, agent: &str, part: &str, id: AccountId, address: Option<&str>) -> Result<(), &'static str> {
    let now = one(config, id);
    let same = now.as_ref().is_some_and(|a| a.link.state == "connected" && a.link.account.as_deref() == address);
    if !same || !parts_of(config, agent, id).is_some_and(|p| p.get(part)) {
        return Err(TAKEN_BACK);
    }
    Ok(())
}

pub const TAKEN_BACK: &str = "Not done: while the card waited, your person disconnected Outlook or took this away from \
you: nothing was written. Tell them plainly; ask again only once they connect it again for you.";

fn agent_may(config: &Config, agent: &str) -> bool {
    config.outlook.enabled && !shared_with_others(config, agent) && kept_on_the_place(config, agent)
}

/// The accounts `agent` uses, each with what it uses of it (connected or ended: it is told to ask for a new sign-in),
/// the first one first. Never for the family's agent nor a project agent (shown to others).
pub fn accounts_for(config: &Config, agent: &str) -> Vec<(Account, Parts)> {
    if !agent_may(config, agent) {
        return Vec::new();
    }
    all(config).into_iter().filter(|a| a.link.agents.iter().any(|x| x == agent)).map(|a| {
        let parts = effective(&a);
        (a, parts)
    }).collect()
}

/// The accounts `agent` uses `part` of (`mail`, `calendar`, `contacts`, `contacts_write`), the first one first.
pub fn accounts_with(config: &Config, agent: &str, part: &str) -> Vec<Account> {
    accounts_for(config, agent).into_iter().filter(|(_, p)| p.get(part)).map(|(a, _)| a).collect()
}

/// What `agent` uses of account `id`. None: nothing.
pub fn parts_of(config: &Config, agent: &str, id: AccountId) -> Option<Parts> {
    accounts_for(config, agent).into_iter().find(|(a, _)| a.id == id).map(|(_, p)| p)
}

/// What `agent` uses of all its accounts together (its tools' list). None: no account.
pub fn parts_for(config: &Config, agent: &str) -> Option<Parts> {
    accounts_for(config, agent).into_iter().map(|(_, p)| p).reduce(Parts::or)
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

pub fn contacts_write_for(config: &Config, agent: &str) -> bool {
    parts_for(config, agent).is_some_and(|p| p.get("contacts_write"))
}

/// The account a name an agent gives (a `mailbox`, a `book`) designates among `mine` (its accounts for that use):
/// nothing or « Outlook » → the first of them; its address, « Outlook (its address) », « its address (Outlook) » or its
/// mailbox's id (`outlook-2`) → that one. None: any other name — never another account instead (review C2).
pub fn named_among(config: &Config, wanted: &str, mine: &[AccountId]) -> Option<AccountId> {
    let wanted = wanted.trim().to_lowercase();
    let inner = wanted
        .strip_prefix("outlook (")
        .and_then(|w| w.strip_suffix(')'))
        .or_else(|| wanted.strip_suffix("(outlook)"))
        .unwrap_or(&wanted)
        .trim();
    if inner.is_empty() || inner == "outlook" {
        return mine.first().copied();
    }
    if let Some(id) = key_account(inner) {
        return mine.contains(&id).then_some(id);
    }
    all(config)
        .into_iter()
        .filter(|a| mine.contains(&a.id))
        .find(|a| a.link.account.as_deref().is_some_and(|x| x.trim().to_lowercase() == inner))
        .map(|a| a.id)
}

/// Account `id`'s address (its name for the agents and the cards), when known.
pub fn address(config: &Config, id: AccountId) -> Option<String> {
    one(config, id).and_then(|a| a.link.account)
}

/// Whether the place keeps several accounts (the agents' answers and the cards then name each one).
pub fn several(config: &Config) -> bool {
    all(config).len() > 1
}

/// Account `id` as the agents and the cards read it: « Outlook », or « Outlook (address) » when the place keeps several.
pub fn label(config: &Config, id: AccountId) -> String {
    match address(config, id).filter(|_| several(config)) {
        Some(address) => format!("{LABEL} ({})", crate::archives::defused(&address)),
        None => LABEL.to_string(),
    }
}

fn account_view(account: &Account) -> Value {
    json!({
        "id": account.id,
        "mailbox": mailbox_key(account.id),
        "state": account.link.state,
        "account": account.link.account,
        "connected_at": account.link.connected_at,
        "agents": account.link.agents,
        "parts": account.parts,
        // switched on and not given by Microsoft yet: « Reconnecter » asks it
        "missing": account.parts.missing(granted(&account.link.tokens)),
    })
}

/// The person's view (the app's screen): the first account in the fields an app before 1.1's additions reads, every
/// account in `accounts`.
pub fn view(config: &Config, accounts: &[Account]) -> Value {
    let first = accounts.first();
    json!({
        "enabled": config.outlook.enabled,
        "configured": drives::endpoints(config, Provider::Microsoft).is_some(),
        "state": first.map(|a| a.link.state.as_str()).unwrap_or("not_connected"),
        "account": first.and_then(|a| a.link.account.clone()),
        "connected_at": first.map(|a| a.link.connected_at),
        "agents": first.map(|a| a.link.agents.clone()).unwrap_or_default(),
        "parts": first.map(|a| a.parts).unwrap_or_default(),
        "missing": first.map(|a| a.parts.missing(granted(&a.link.tokens))).unwrap_or_default(),
        "accounts": accounts.iter().map(account_view).collect::<Vec<_>>(),
        "max_accounts": MAX_ACCOUNTS,
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

/// A sign-in under way, by its state (drives' `Flow` has no room for them): the parts it asks, the account it
/// reconnects (None: a new one, or the same address signed in again).
type Asked = (std::time::Instant, Parts, Option<AccountId>);
static ASKED: std::sync::LazyLock<Mutex<HashMap<String, Asked>>> = std::sync::LazyLock::new(Default::default);

/// A sign-in for `agents` (the same on iPhone and Android: Microsoft's web sign-in, back to the app's address),
/// asking the rights of `parts` (none given: those kept for `account`, else the first account's, else the default
/// ones). `account`: the one « Reconnecter » is for; None: « Connecter », « Ajouter un compte Microsoft ».
pub fn start(config: &Config, agents: &[String], parts: Option<PartsBody>, account: Option<AccountId>) -> ApiResult<Value> {
    if !config.outlook.enabled {
        return Err(off());
    }
    let ep = drives::endpoints(config, Provider::Microsoft).ok_or_else(not_configured)?;
    let agents = agents_checked(config, agents)?;
    let kept = match account {
        Some(id) => Some(one(config, id).ok_or_else(not_connected)?.parts),
        None => stored(config).map(|a| a.parts),
    };
    let parts = parts.map(|p| p.over(kept.unwrap_or_default())).or(kept).unwrap_or_default();
    if !parts.any() {
        return Err(coded(422, "parts", "switch on mail, calendar or contacts"));
    }
    let state = py::token_hex(16);
    let verifier = py::token_hex(32);
    let url = authorize_url(&ep, &state, &verifier, parts);
    {
        let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
        asked.retain(|_, (at, _, _)| at.elapsed().as_secs() < FLOW_SECONDS);
        asked.insert(state.clone(), (std::time::Instant::now(), parts, account));
    }
    drives::keep_flow(
        &state,
        drives::Flow {
            provider: Provider::Microsoft,
            verifier,
            agents,
            client: String::new(),
            started: std::time::Instant::now(),
            slot: Slot::Outlook(account.unwrap_or(0)),
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

fn same_address(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

/// The callback (the address the web sign-in came back to): the code exchanged, the account named. The same address
/// as an account kept updates it (its agents kept, those chosen at start added; its parts those asked); another one is
/// ADDED beside the others (Alex 24/11: a second account never breaks the first), `MAX_ACCOUNTS` at most. Returns the
/// view and the account's number.
pub async fn finish(config: &Config, query: HashMap<String, String>) -> ApiResult<(Value, AccountId)> {
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
        .and_then(|s| drives::take_flow_of(s, |slot| matches!(slot, Slot::Outlook(_))))
        .ok_or_else(|| coded(400, "unknown_state", "this sign-in is too old: start again"))?;
    let code = query
        .get("code")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| coded(400, "bad_callback", "the sign-in came back without its code"))?;
    let ep = drives::endpoints(config, Provider::Microsoft).ok_or_else(not_configured)?;
    let (asked, reconnect) = query
        .get("state")
        .and_then(|s| ASKED.lock().unwrap_or_else(|e| e.into_inner()).remove(s))
        .map(|(_, parts, account)| (parts, account))
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
    let address = api.account().await.or_else(|| connections::account_of(&answer));
    let (accounts, id, added) = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        // the same Microsoft account: the one kept of its address; without an address, the one « Reconnecter » named
        let kept = accounts_in(&stored)
            .into_iter()
            .find(|a| same_address(a.link.account.as_deref(), address.as_deref()))
            .or_else(|| {
                address.is_none().then(|| reconnect.and_then(|id| account_in(&stored, id))).flatten()
            })
            .map(|a| a.id);
        let added = kept.is_none();
        if added && accounts_in(&stored).len() >= MAX_ACCOUNTS {
            return Err(coded(409, "too_many", &format!("at most {MAX_ACCOUNTS} Microsoft accounts")));
        }
        // a new number, never one given before (review B2): an account disconnected keeps its number to itself
        let id = kept.unwrap_or_else(|| accounts_in(&stored).iter().map(|a| a.id).max().unwrap_or(0).max(stored.outlook_last) + 1);
        stored.outlook_last = stored.outlook_last.max(id);
        let mut agents = kept.and_then(|id| account_in(&stored, id)).map(|a| a.link.agents.clone()).unwrap_or_default();
        for a in &flow.agents {
            if !agents.contains(a) {
                agents.push(a.clone());
            }
        }
        let link = Link {
            tokens,
            account: address,
            connected_at: py::time_time(),
            state: "connected".into(),
            client: String::new(),
            folders: Vec::new(),
            agents,
            carnet: false,
        };
        // what was asked is what they use (the app sends what the person switched on)
        let account = Account { link, parts: asked, id };
        match account_in_mut(&mut stored, id) {
            Some(slot) => *slot = account,
            None => push_account(&mut stored, account),
        }
        store.write(&stored)?;
        (accounts_in(&stored).into_iter().cloned().collect::<Vec<_>>(), id, added)
    };
    tracing::info!(account = id, added, accounts = accounts.len(), "outlook: connected");
    Ok((view(config, &accounts), id))
}

/// The agents chosen now for account `id` (None: the first; the whole list) and, when given, what they use. Returns
/// the view and the agents whose tools change.
pub async fn set_agents(
    config: &Config,
    agents: &[String],
    parts: Option<PartsBody>,
    id: Option<AccountId>,
) -> ApiResult<(Value, Vec<String>)> {
    if !config.outlook.enabled {
        return Err(off());
    }
    // an agent deleted since the app listed them: left out, never a refusal (as drives.rs)
    let present: Vec<String> = agents.iter().filter(|a| config.agent(Some(a.as_str())).is_some()).cloned().collect();
    let agents = agents_checked(config, &present)?;
    let (shown, changed, removed, calendar_off, id) = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let id = id.or_else(|| stored.outlook.as_ref().map(|a| a.id)).ok_or_else(not_connected)?;
        let account = account_in_mut(&mut stored, id).ok_or_else(not_connected)?;
        let before = std::mem::replace(&mut account.link.agents, agents.clone());
        let before_parts = account.parts;
        if let Some(parts) = parts {
            account.parts = parts.over(before_parts);
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
        let shown = view(config, &accounts_in(&stored).into_iter().cloned().collect::<Vec<_>>());
        (shown, changed, removed, calendar_off, id)
    };
    // their « Toujours » for this account's calendar goes with it: chosen again, they are asked again
    if calendar_off {
        revoke_always(config, id, None).await;
    } else {
        revoke_always(config, id, Some(&removed)).await;
    }
    Ok((shown, changed))
}

/// Account `id` (None: the first) forgotten here (Microsoft can't be asked to revoke a public client's grant); the
/// others untouched. Works with the switch off too. The answer, and the agents that had it.
pub async fn disconnect(config: &Config, id: Option<AccountId>) -> ApiResult<(Value, Vec<String>)> {
    let account = {
        let _guard = connections::LOCK.lock().await;
        let store = connections::store_of(config);
        let mut stored = store.read()?;
        let missing = || if config.outlook.enabled { not_connected() } else { off() };
        let id = id.or_else(|| stored.outlook.as_ref().map(|a| a.id)).ok_or_else(missing)?;
        let account = take_account(&mut stored, id).ok_or_else(missing)?;
        store.write(&stored)?;
        account
    };
    revoke_always(config, account.id, None).await;
    forget_local(config, account.id);
    // its mailbox's standing rules stopped and its routines turned off, their cards waiting taken away (review B2):
    // nothing set on this mailbox ever acts on another
    let key = mailbox_key(account.id);
    let rules = crate::mail_rules::forget_mailbox(config, &key).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "outlook: a disconnected account's rules not stopped");
        0
    });
    let routines = crate::triggers::mailbox_gone(config, &key);
    tracing::info!(account = account.id, rules, routines, "outlook: disconnected");
    Ok((json!({"ok": true, "revoked": false, "account": account.id}), account.link.agents))
}

/// The « Toujours » of `agents` (None: every agent) for account `id`'s calendar taken back, their personalities
/// rendered again.
async fn revoke_always(config: &Config, id: AccountId, agents: Option<&[String]>) {
    for agent in crate::approval_level::revoke_always_of(config, &always_key(id), agents) {
        crate::approval_level::granted_now(config, &agent).await;
    }
}

/// The switch turned off with accounts still connected: all forgotten at the bridge's start.
pub async fn ended_when_off(config: &Config) -> bool {
    if config.outlook.enabled || all(config).is_empty() {
        return false;
    }
    let mut ended = false;
    for account in all(config) {
        ended |= disconnect(config, Some(account.id)).await.is_ok();
    }
    ended
}

/// [`ended_when_off`] at the bridge's start.
pub async fn end_when_off(state: AppState) {
    ended_when_off(&state.config()).await;
}

/// An agent deleted (agents_admin.rs): no longer among any account's chosen ones.
pub async fn forget_agent(config: &Config, agent: &str) {
    let _guard = connections::LOCK.lock().await;
    let store = connections::store_of(config);
    let Ok(mut stored) = store.read() else { return };
    let mut changed = false;
    let ids: Vec<AccountId> = accounts_in(&stored).iter().map(|a| a.id).collect();
    for id in ids {
        if let Some(account) = account_in_mut(&mut stored, id) {
            let before = account.link.agents.len();
            account.link.agents.retain(|a| a != agent);
            changed |= account.link.agents.len() != before;
        }
        crate::approval_level::revoke_always_of(config, &always_key(id), Some(&[agent.to_string()]));
    }
    if changed {
        if let Err(e) = store.write(&stored) {
            tracing::warn!(error = %e, "outlook: a deleted agent can't be taken out");
        }
    }
}

// -- Graph -------------------------------------------------------------------------------------------------------------

/// A working Graph access for account `id` (renewed first when it ends within five minutes).
pub async fn signed_in(config: &Config, id: AccountId) -> Result<Api, Fault> {
    drives::access_slot(config, Slot::Outlook(id)).await
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

/// A Graph id as the table keeps it: the first account's as it is (as before 1.1's additions), another's after its
/// number (`2:AAMk…`; Graph's ids never start with digits and a colon).
fn entry(id: AccountId, long: &str) -> String {
    if id == FIRST {
        long.to_string()
    } else {
        format!("{id}:{long}")
    }
}

/// The account and Graph's id of a table's entry.
fn split_entry(kept: &str) -> (AccountId, String) {
    match kept.split_once(':') {
        Some((n, long)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (n.parse().unwrap_or(FIRST), long.to_string())
        }
        _ => (FIRST, kept.to_string()),
    }
}

/// The short numbers of account `id`'s Graph ids, made for those new (the same id always gets the same number; the
/// numbers are the place's, one table for all its accounts: a number names its account). In memory; the new ones are
/// written by a lot, in the background.
pub fn short_ids(config: &Config, id: AccountId, longs: &[String]) -> Vec<u64> {
    let (out, changed) = with_table(config, |t| {
        let mut changed = false;
        let out: Vec<u64> = longs
            .iter()
            .map(|long| entry(id, long))
            .map(|long| match t.by_long.get(&long) {
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

pub fn short_id(config: &Config, id: AccountId, long: &str) -> u64 {
    short_ids(config, id, &[long.to_string()]).first().copied().unwrap_or(0)
}

/// The account and Graph's id of a short number. None: not one of this place's.
pub fn long_id(config: &Config, n: u64) -> Option<(AccountId, String)> {
    with_table(config, |t| t.ids.ids.get(&n).map(|kept| split_entry(kept)))
}

/// Graph's id of a short number of account `id`. None: not one of that account's.
pub fn long_id_of(config: &Config, id: AccountId, n: u64) -> Option<String> {
    long_id(config, n).filter(|(of, _)| *of == id).map(|(_, long)| long)
}

/// The ids, drafts, tidyings and watch of account `id`, no longer connected (a write on its way never lands after);
/// the others' kept. The last one gone: the whole folder.
fn forget_local(config: &Config, id: AccountId) {
    let _writing = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let last = all(config).is_empty();
    let kept = with_table(config, |t| {
        if last {
            t.ids = Ids::default();
            t.by_long.clear();
            t.dirty = false;
        } else {
            t.ids.ids.retain(|_, kept| split_entry(kept).0 != id);
            t.by_long.retain(|kept, _| split_entry(kept).0 != id);
            t.dirty = false;
        }
        t.generation += 1;
        (!last).then(|| t.ids.clone())
    });
    match kept {
        None => {
            let _ = std::fs::remove_dir_all(folder(config));
        }
        Some(ids) => {
            if let Err(e) = serde_json::to_vec(&ids).map_err(anyhow::Error::from).and_then(|b| write_file(config, "ids.json", &b)) {
                tracing::warn!(error = %e, "outlook: short ids not kept");
            }
            crate::outlook_mail::forget_account(config, id);
            crate::outlook_watch::forget_account(config, id);
        }
    }
    crate::outlook_mail::forget_scans(id);
    crate::outlook_contacts::forget_cache(id);
}

// -- routes ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartBody {
    #[serde(default)]
    pub agents: Vec<String>,
    /// What they will use (an older app sends none: those kept, else all of them).
    #[serde(default)]
    pub parts: Option<PartsBody>,
    /// The account « Reconnecter » is for (None: connect one, or add another).
    #[serde(default)]
    pub account: Option<AccountId>,
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
    pub parts: Option<PartsBody>,
    /// The account they are for (None: the first, as an app before 1.1's additions sends it).
    #[serde(default)]
    pub account: Option<AccountId>,
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
    Ok(web::ok(view(&config, &all(&config))))
}

async fn start_route(State(state): State<AppState>, Json(body): Json<StartBody>) -> ApiResult<Response> {
    Ok(web::ok(start(&state.config(), &body.agents, body.parts, body.account)?))
}

async fn callback_route(State(state): State<AppState>, Json(body): Json<CallbackBody>) -> ApiResult<Response> {
    let config = state.config();
    // the agents before and after (review R11: every account's, an account added changes its agents' tools)
    let before: Vec<String> = all(&config).into_iter().flat_map(|a| a.link.agents).collect();
    let (shown, _) = finish(&config, connections::callback_query(&body.url)).await?;
    let mut agents: Vec<String> = Vec::new();
    for a in all(&config).into_iter().flat_map(|a| a.link.agents).chain(before) {
        if !agents.contains(&a) {
            agents.push(a);
        }
    }
    tools_changed(&config, &agents).await;
    Ok(web::ok(shown))
}

async fn agents_route(State(state): State<AppState>, Json(body): Json<AgentsBody>) -> ApiResult<Response> {
    let config = state.config();
    let (shown, changed) = set_agents(&config, &body.agents, body.parts, body.account).await?;
    tools_changed(&config, &changed).await;
    Ok(web::ok(shown))
}

async fn disconnect_route(State(state): State<AppState>, q: web::QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let account = match q.get("account").map(|a| a.trim()).filter(|a| !a.is_empty()) {
        Some(a) => Some(a.parse::<AccountId>().map_err(|_| coded(422, "account", "account: a number"))?),
        None => None,
    };
    let (answer, agents) = disconnect(&config, account).await?;
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
