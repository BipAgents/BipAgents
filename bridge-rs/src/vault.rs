//! The agents' vault: credentials given from the app, used by the agents without them ever reading one.
//!
//! The bridge keeps the master copy, encrypted (Fernet) in its data folder, which no agent can reach: agents
//! run in their own container or sandbox. Each item is then written into the Hermes vault of the agents the
//! person allowed, and only theirs. Hermes fills the login form itself on the item's exact origin, and the
//! password never reaches the model, the conversation or the logs (`browser_vault_fill`).
//!
//! Write-only: the app never reads a secret back. It replaces or deletes it. One exception: a login an agent created
//! with a password the vault made (vault_logins.rs), which the person never typed, is shown to them on request.
//!
//! An agent holds items in its own space (`isolated`), or, since 2026-10-28 (Alex), in the shared hub
//! (`holds_in_hub`): in a shared gateway an agent with a terminal could read another's vault, which is accepted
//! between one person's agents. Since 21/11 (Alex: « tout ce qui vient de quelqu'un d'autre tourne en tour d'invité,
//! et aucun agent ne sort du hub parce que d'autres lui parlent ») what other people send an agent (a relative of a
//! shared agent, the family, another household's agents) runs as a guest turn (Hermes patch 0018: no terminal, no
//! files, no vault, no browser), so an agent other people talk to (`others_facing`) stays in the hub and keeps the
//! hub private; only a project agent breaks it (`hub_breakers`: its guests' turns are its own), and while the hub
//! hosts one, its agents move into their own space at their first secret (own_space.rs), as before. Who may use
//! which item (`others_facing` asks the person first, vault_contacts.rs) is unchanged. Trade-off: one agent tricked
//! by what it read could reach the person's other agents' passwords, in exchange for no extra container per agent.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::browsers::pyx;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError};
use crate::Dict;

pub const KINDS: [&str; 4] = ["login", "payment", "address", crate::cards::KIND];
/// A card's handle for an agent allowed to use it: nothing is written into the agent's own vault.
pub const CARD_HANDLE: &str = "bridge";
/// An agent whose Hermes copy failed to be made ready (a place enrolled for sealed keys, hermes_copies.rs) is not tried
/// again by each give or take-back for this long: the bridge's own retry does (sealed_keys.rs `copies_ready_retrying`).
pub const COPY_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(60);
pub const IDENTIFIER_TYPES: [&str; 3] = ["email", "phone", "username"];
pub const PAYMENT_FIELDS: [&str; 6] =
    ["card_number", "cardholder_name", "exp_month", "exp_year", "cvc", "billing_postal_code"];
pub const ADDRESS_FIELDS: [&str; 6] = ["address_line1", "address_line2", "city", "state", "postal_code", "country"];

/// Runs inside the agent's Hermes (its Python, its HERMES_HOME): the request, secrets included, comes on stdin,
/// never on the command line. Prints JSON only. `add_many` (hermes_copies.rs, a place enrolled for sealed keys): a
/// copy rebuilt in one run, in the folder `vault` names, its ids (null for an item Hermes refuses) and its listing;
/// `export`: a disk copy's items with their secrets, read once before it is erased.
pub const HERMES_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
req = json.load(sys.stdin)
if req.get("root"):  # Hermes' own Python with its code folder and dependencies, as its launcher sets them up
    sys.path.insert(0, req["root"])
    import hermes_bootstrap
from agent.vault_store import VaultStore
store = VaultStore(base_dir=Path(req["vault"]) if req.get("vault") else Path(req["home"]) / "vault")
if req["op"] == "add":
    meta = store.add_item(req["kind"], req["label"], req["secret"], origin=req.get("origin"))
    print(json.dumps({"id": meta.id}))
elif req["op"] == "add_many":
    from agent.vault_store import VaultError
    ids = []
    for item in req["items"]:
        try:
            ids.append(store.add_item(item["kind"], item["label"], item["secret"], origin=item.get("origin")).id)
        except VaultError:
            ids.append(None)
    print(json.dumps({"ids": ids, "items": [m.to_dict() for m in store.list_items()]}))
elif req["op"] == "export":
    # a copy of the disk about to be erased (hermes_copies.rs): each item with its secret, once, for the bridge to
    # take what only Hermes holds into its master copy (stdout to the bridge only)
    out = []
    for meta in store.list_items():
        item = meta.to_dict()
        item["secret"] = store.resolve_secret(meta.id)
        out.append(item)
    print(json.dumps({"items": out}))
elif req["op"] == "remove":
    print(json.dumps({"removed": store.remove_item(req["id"])}))
else:
    print(json.dumps({"items": [m.to_dict() for m in store.list_items()]}))
"#;

/// Python's `VaultError` and its kinds.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VaultError {
    #[error("{0}")]
    Invalid(String),
    /// `CannotHold(reasons)`: agent id → why it cannot hold credentials.
    #[error("some agents cannot hold credentials")]
    CannotHold(IndexMap<String, String>),
    /// `Unprotected`.
    #[error("this server does not isolate its agents: no credentials are kept")]
    Unprotected,
    /// Payment cards are off on this install (`[vault] cards`).
    #[error("payment cards are off on this server")]
    CardsOff,
    /// `ConfirmOthers(agents)`: other people talk to these agents and the person did not confirm (`confirmed_others`).
    #[error("other people talk to these agents: the person must confirm")]
    ConfirmOthers(Vec<String>),
    /// `Unavailable(agent)`: the agent's Hermes vault could not be reached (its Python missing, or failing before it
    /// read the item): nothing the person typed is wrong, the server has to be fixed (a 503, never « vérifie ce que
    /// tu as écrit »).
    #[error("{0}: the agent's vault could not be reached")]
    Unavailable(String),
}

/// A card found for an agent (`Vault::card_for`): its numbers stay in the bridge.
pub struct CardFound {
    pub id: String,
    pub label: String,
    /// Its public part (cards.rs).
    pub meta: Dict,
    /// number, exp_month, exp_year, csc, name, postal_code.
    pub secret: Dict,
}

/// A line the person is told once about the vault (in the app's Coffre, until it is shown: `DELETE
/// /v1/vault/notices/{id}`): today, a card of an agent's Hermes not taken (never kept with its security code).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Notice {
    pub id: String,
    pub text: String,
    /// What it is about, once (`card_not_kept:<agent>:<label>`): never said twice.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub at: String,
}

/// « Ta carte « X » n'a pas été reprise : ajoute-la dans le Coffre. », in the person's language.
pub fn card_not_kept_text(lang: &str, label: &str) -> String {
    let label = cut_name(label);
    match lang {
        "de" => format!("Deine Karte „{label}“ wurde nicht übernommen: Füge sie im Tresor hinzu."),
        "es" => format!("Tu tarjeta «{label}» no se ha conservado: añádela en la Caja fuerte."),
        "fr" => format!("Ta carte « {label} » n'a pas été reprise : ajoute-la dans le Coffre."),
        _ => format!("Your card “{label}” wasn't kept: add it in the Vault."),
    }
}

/// A card's name as Hermes gave it, for its notice: cut, and never with a card's number in it (an agent may have named
/// it so): its last four digits only then.
pub fn card_name(label: &str) -> String {
    if let Some(&(start, end)) = crate::card_details::card_numbers(label).last() {
        // the last four digits of the number found, never of what follows it (a security code, a date)
        let digits: String = label[start..end].chars().filter(char::is_ascii_digit).collect();
        return format!("•••• {}", &digits[digits.len().saturating_sub(4)..]);
    }
    cut_name(label)
}

/// A name Hermes gave, at most 60 characters in a line told to the person.
fn cut_name(name: &str) -> String {
    let name = name.trim();
    if name.chars().count() <= 60 {
        name.to_string()
    } else {
        format!("{}…", name.chars().take(59).collect::<String>().trim_end())
    }
}

/// Why an entry is not given to an agent (an entry's `refused` mark): its Hermes refused it at a rebuild of its copy in
/// memory. A code: the apps write the sentence in the phone's language, in plain words (never « Hermes »).
pub const AGENT_REFUSED: &str = "agent_refused";

/// What [`Vault::copies_ready_all`] did, by agent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CopiesReport {
    /// Copies found right in memory (a listing, or nothing to hold).
    pub checked: Vec<String>,
    /// Copies rebuilt (one Hermes run each).
    pub rebuilt: Vec<String>,
    /// Copies not ready (its Hermes failing or answering short): its copy on the disk, if any, kept; tried again in
    /// the background (sealed_keys.rs `copies_ready_retrying`), counted in /health.
    pub failed: Vec<String>,
    /// Files erased from the disk (copies whose rebuild was checked, copies no agent uses).
    pub erased: usize,
}

enum CopyReady {
    /// Found right; what a cut-off swap left beside its link erased (how many files).
    Checked(usize),
    Rebuilt(usize),
    /// A Hermes the bridge reaches in a container: its copy where it is.
    Elsewhere,
}

/// Python's `KeyError(item_id)`: no item with this id.
#[derive(Debug, Clone, thiserror::Error)]
#[error("'{0}'")]
pub struct NoSuchItem(pub String);

fn invalid(text: &str) -> VaultError {
    VaultError::Invalid(text.to_string())
}

/// An item's metadata (never its secret).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub origin: Option<String>,
    /// A login's own login page, as it was typed (`login_url_of`): on its origin only. The agent opens it; the login
    /// fills on its origin's registrable domain (vault_match.rs), the page telling apart logins of one domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    pub identifier_type: Option<String>,
    pub identifier: Option<String>,
    pub has_otp: bool,
    /// agent id -> its handle in that agent's Hermes vault
    pub agents: IndexMap<String, String>,
    /// agent id -> why the entry is not given to it, a code ([`AGENT_REFUSED`]): its Hermes refused it at a rebuild of
    /// its copy in memory (hermes_copies.rs); given again at the next rebuild it accepts, or when the person gives it
    /// again.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub refused: IndexMap<String, String>,
    /// A login an agent's Hermes held for the same site and user name as this one, with another password (taken into
    /// the vault so neither is lost): the id of the person's own, which `vault_login` fills first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// A payment card's public part (cards.rs): last 4 digits, expiry, flags, sites, payments. Never its numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<Value>,
    /// A login an agent created on a website with a password of the vault (vault_logins.rs): that agent's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Such a login until the agent (or the person) confirms the site accepted the sign-up: « en attente de
    /// confirmation ».
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
    /// A secret an agent asked for (secrets.rs): its kind (password, token, code), the identifier's label, when last
    /// used. Never its value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_info: Option<Value>,
    /// « Coordonnées » (vault_contacts.rs): when the agent should use the entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// « Coordonnées »: the fields, read back by the app (personal data, not secrets). Filled from the record's secret.
    #[serde(skip)]
    pub contact: Option<Value>,
}

impl Item {
    /// What the app sees: the agents' ids, sorted, without their handles.
    pub fn public(&self) -> Value {
        let mut agents: Vec<&String> = self.agents.keys().collect();
        agents.sort();
        let mut out = json!({"id": self.id, "kind": self.kind, "label": self.label, "origin": self.origin,
               "identifier_type": self.identifier_type, "identifier": self.identifier, "has_otp": self.has_otp,
               "agents": agents, "created_at": self.created_at, "updated_at": self.updated_at});
        if let (false, Value::Object(map)) = (self.refused.is_empty(), &mut out) {
            // agent id -> its code; the app writes the line under the entry (« Coach ne peut pas encore s'en servir. »)
            // (a mark of before the code, its line, read as the code)
            let codes: IndexMap<&String, &str> = self.refused.keys().map(|agent| (agent, AGENT_REFUSED)).collect();
            map.insert("refused".into(), json!(codes));
        }
        if let (Some(of), Value::Object(map)) = (&self.duplicate_of, &mut out) {
            // another password for the same login, kept from an agent's Hermes: the app names the entry tried first
            map.insert("duplicate_of".into(), json!(of));
        }
        if let (Some(card), Value::Object(map)) = (&self.card, &mut out) {
            map.insert("card".into(), crate::cards::public(card));
        }
        if let (Some(contact), Value::Object(map)) = (&self.contact, &mut out) {
            map.insert("hint".into(), json!(self.hint.clone().unwrap_or_default()));
            map.insert("contact".into(), contact.clone());
            if let Some(agent) = &self.created_by {
                map.insert("created_by".into(), json!(agent));
            }
            return out;
        }
        if let (Some(info), Value::Object(map)) = (&self.secret_info, &mut out) {
            map.insert("created_by".into(), json!(self.created_by));
            map.insert("secret".into(), info.clone());
            return out;
        }
        if let (true, Value::Object(map)) = (self.kind == "login", &mut out) {
            map.insert("login_url".into(), json!(self.login_url));
            // the site the vault fills on, the apps' groups (vault_used.rs, 17/11)
            map.insert("domain".into(), json!(self.origin.as_deref().and_then(crate::vault_used::domain_of)));
        }
        if let (Some(agent), Value::Object(map)) = (&self.created_by, &mut out) {
            map.insert("created_by".into(), json!(agent));
            map.insert("pending".into(), json!(self.pending));
            map.insert("confirmed_at".into(), json!(self.confirmed_at));
        }
        out
    }
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// « sncf-connect.com/login » → « https://sncf-connect.com »: the origin of what was typed, nothing guessed
/// (no « www. » added). Hermes fills only on this exact origin.
pub fn normalize_origin(url: &str) -> Result<String, VaultError> {
    let raw = pyx::strip(url).to_string();
    if raw.is_empty() {
        return Err(invalid("site is required"));
    }
    let raw = if raw.contains("://") { raw } else { format!("https://{raw}") };
    let parts = pyx::urlsplit(&raw).map_err(VaultError::Invalid)?;
    let hostname = parts.hostname();
    if !(parts.scheme == "https" || parts.scheme == "http") || hostname.is_none() {
        return Err(invalid("site must be a web address"));
    }
    let port = parts.port().map_err(VaultError::Invalid)?;
    let default = if parts.scheme == "https" { 443 } else { 80 };
    let port = match port {
        Some(p) if p != 0 && p != default => format!(":{p}"),
        _ => String::new(),
    };
    Ok(format!("{}://{}{port}", parts.scheme, hostname.unwrap_or_default().to_lowercase()))
}

/// The longest login page address kept (`login_url`).
pub const LOGIN_URL_MAX: usize = 2000;

/// A login's own login page (`login_url`) from what was typed (« extranet.example.fr/V5/connexion.html »): kept when it
/// says more than its site (a path, a query), on exactly `origin` (anything else is dropped, never kept), without a
/// user name or password in it nor its fragment. None otherwise.
pub fn login_url_of(typed: &str, origin: &str) -> Option<String> {
    let raw = pyx::strip(typed).to_string();
    if raw.is_empty() || raw.chars().count() > LOGIN_URL_MAX {
        return None;
    }
    let raw = if raw.contains("://") { raw } else { format!("https://{raw}") };
    if normalize_origin(&raw).ok()? != origin {
        return None;
    }
    let mut url = url::Url::parse(&raw).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    // review R8: the site as the `url` crate (and Chrome) reads it, never another one behind a difference with
    // `normalize_origin` (Python's urlsplit: `https://other.example\@site.example/` is site.example there, other.example
    // here)
    let same = |a: &crate::vault_match::Site, b: &crate::vault_match::Site| {
        a.scheme == b.scheme && a.host == b.host && a.port == b.port
    };
    match (crate::vault_match::site_of(url.as_str()), crate::vault_match::site_of(origin)) {
        (Some(here), Some(kept)) if same(&here, &kept) => {}
        _ => return None,
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    if url.path() == "/" && url.query().is_none() {
        return None;
    }
    Some(url.to_string())
}

pub fn in_hub(config: &Config, agent: &AgentConfig) -> bool {
    config.hermes.enabled()
        && agent.space.as_deref().map(str::is_empty).unwrap_or(true)
        && agent.hermes_url.starts_with(&format!("{}/p/", config.hermes.hub_url))
}

/// The agents of this place that people other than its person talk to (Alex, 2026-10-28): the family's agent,
/// project agents (with guests), agents shared with a relative (agent_shares.rs). Another household's agents writing
/// to one count only while `rules::RECEIVERS_FACE_OTHERS` (its turns are a relative's, through the bridge:
/// household_asks.rs); an agent that writes to another household's never counts (Alex 21/11: no agent leaves the
/// shared hub for that). Each file read once.
pub fn others_facing_agents(config: &Config) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for family in crate::family_agent::owns(config) {
        out.insert(family);
    }
    if crate::rules::RECEIVERS_FACE_OTHERS {
        let world = crate::rules::World::of(config, false);
        out.extend(crate::rules::household_receivers(&world, &crate::rules::load(config)));
    }
    for name in config.agents.keys() {
        if crate::project_agent::is_project(config, name) || !crate::agent_shares::shared_with(config, name).is_empty() {
            out.insert(name.clone());
        }
    }
    out
}

/// The agents whose presence makes the shared hub no longer private (21/11): project agents, whose guests' turns are
/// their own. Every other agent other people talk to gets them as guest turns (patch 0018) and stays in the hub.
pub fn hub_breakers(config: &Config) -> std::collections::HashSet<String> {
    config.agents.keys().filter(|n| crate::project_agent::is_project(config, n)).cloned().collect()
}

/// Whether `agent` must live out of the shared hub (`hub_breakers`).
pub fn leaves_hub(config: &Config, agent: &str) -> bool {
    hub_breakers(config).contains(&agent.trim().to_lowercase())
}

/// Whether people other than this place's person talk to `agent` (`others_facing_agents`).
pub fn others_facing(config: &Config, agent: &str) -> bool {
    others_facing_agents(config).contains(&agent.trim().to_lowercase())
}

/// Who else talks to `agent`, by name, for the confirmation before it gets a credential (secrets.rs): the linked
/// households of the family's agent, the relatives it is shared with, the households whose agents are linked with it.
/// Empty when none is known by name (the apps then say « d'autres personnes »). Project guests have no names here: a
/// project agent never gets credentials.
pub fn others_names(config: &Config, agent: &str) -> Vec<String> {
    let agent = agent.trim().to_lowercase();
    let linked = crate::households::Households::new(&crate::households::path_for(config));
    let mut households: Vec<String> = crate::agent_shares::shared_with(config, &agent);
    let world = crate::rules::World::of(config, false);
    households.extend(crate::rules::households_of(&world, &crate::rules::load(config), &agent));
    let mut names: Vec<String> = Vec::new();
    for household in households {
        let name = linked
            .get(&household)
            .and_then(|h| h.get("name").map(crate::py::py_str))
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_default();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Whether `agent`, of the shared hub, holds credentials right there, with no move (Alex, 2026-10-28: a person's own
/// agents are no longer separated from each other): the install protects the vault, the hub's Python can write its
/// items, the agent's tools run in a sandbox of their own (sandbox.rs: connectors need one, the move gives it one),
/// the agent breaks the hub's privacy no more than the others and the hub hosts no agent that does (`hub_breakers`).
/// `holder` asks at each secret's use, not only when the tools are offered: once a project agent appears in the hub, a
/// new secret moves its agent as before.
pub fn holds_in_hub(config: &Config, agent: &AgentConfig) -> bool {
    hub_may_hold(config, agent, &hub_breakers(config))
        && crate::sandbox::isolates(config, agent)
}

/// `holds_in_hub` but the sandbox (unit tests). `breakers`: `hub_breakers`.
fn hub_may_hold(config: &Config, agent: &AgentConfig, breakers: &std::collections::HashSet<String>) -> bool {
    config.hermes.isolated
        && !agent.isolated
        && in_hub(config, agent)
        && !config.hermes.python.is_empty()
        && !breakers.contains(&agent.name)
        && !config.agents.values().any(|a| in_hub(config, a) && breakers.contains(&a.name))
}

/// The hub's agents holding credentials although the hub now hosts an agent that breaks its privacy (`hub_breakers`;
/// logged at start and at each move: their items stay in their profile's vault until they move or are taken back).
pub fn hub_holders_at_risk(config: &Config, records: &[Dict]) -> Vec<String> {
    let facing = hub_breakers(config);
    if !config.agents.values().any(|a| in_hub(config, a) && facing.contains(&a.name)) {
        return Vec::new();
    }
    let mut out: Vec<String> = config
        .agents
        .values()
        .filter(|a| in_hub(config, a) && !a.isolated)
        .filter(|a| {
            records.iter().any(|r| {
                r.get("agents").and_then(Value::as_object).and_then(|m| m.get(&a.name)).and_then(Value::as_str)
                    .is_some_and(|h| h != CARD_HANDLE && !h.is_empty())
            })
        })
        .map(|a| a.name.clone())
        .collect();
    out.sort();
    out
}

/// The agent as the vault reaches its Hermes: an agent holding credentials in the hub gets the hub's Python and its
/// profile's HERMES_HOME (as that Python sees it), the others are as they are. Either way the Python is the one
/// Hermes runs on today (config.rs `current_python`): a space's, written down when it was made, may name the one
/// before an update of Hermes (`bipagents hermes-update --to-bipagents`, 2026-11-09: every item refused).
pub fn vault_target(config: &Config, agent: &AgentConfig) -> AgentConfig {
    let mut target = vault_target_as_written(config, agent);
    target.python = crate::config::current_python(&config.hermes.home, &target.python);
    target
}

fn vault_target_as_written(config: &Config, agent: &AgentConfig) -> AgentConfig {
    if agent.isolated || !in_hub(config, agent) || config.hermes.python.is_empty() {
        return agent.clone();
    }
    let mut target = agent.clone();
    let home = if !config.hermes.container_home.is_empty() {
        crate::py::join(&crate::py::join(&config.hermes.container_home, "profiles"), &agent.name)
    } else {
        crate::spaces::hub_profile(config, &agent.name).to_string_lossy().into_owned()
    };
    target.python = config.hermes.python.clone();
    target.hermes_home = Some(home);
    target.hermes_root = Some(config.hermes.root.clone()).filter(|r| !r.is_empty());
    target
}

fn is_address(record: &Dict) -> bool {
    record.get("kind").and_then(Value::as_str) == Some(crate::vault_contacts::KIND)
}

/// Whether `agent`'s own browser is on (browsers.rs: its `stealth` MCP server enabled in its Hermes profile).
pub fn browser_on(config: &Config, agent: &AgentConfig) -> bool {
    crate::browsers::state(&crate::agents_admin::home_root(config, agent), &agent.name) == Some(true)
}

/// None when the agent can hold credentials, else why not.
pub fn holder(config: &Config, agent: &AgentConfig) -> Option<String> {
    if !config.hermes.isolated {
        return Some("unprotected_install".into()); // the agents could read the bridge's own copy: no credentials at all
    }
    if !agent.isolated {
        if holds_in_hub(config, agent) {
            return None; // one of the person's own agents, in a hub only they use
        }
        // An agent of the shared hub moves into its own space by itself (own_space.rs); one set up by hand is up to its
        // administrator (its own container, then `isolated`, `python` and `hermes_home` in this file).
        return Some(if in_hub(config, agent) { "needs_own_space" } else { "not_configured" }.into());
    }
    if agent.space.as_deref().map(|s| !s.is_empty()).unwrap_or(false) && !agent.sandboxed {
        // Its own gateway, but under the same account as the hub's agents, without a sandbox: they could still
        // read its vault on disk.
        return Some("needs_sandbox".into());
    }
    if agent.python.is_empty() || agent.hermes_home.as_deref().map(str::is_empty).unwrap_or(true) {
        return Some("not_configured".into());
    }
    None
}

/// How the vault reaches an agent's Hermes vault (tests give their own, as Python's `run=`).
pub type VaultRun = Arc<dyn Fn(AgentConfig, Dict) -> BoxFuture<'static, anyhow::Result<Dict>> + Send + Sync>;

/// At most this much of what an agent's Hermes prints is read (32 MB: an agent's whole vault, `export`).
pub const HERMES_OUTPUT_MAX: usize = 32 * 1024 * 1024;

/// The request on the agent's own Python, its answer the last line it prints.
pub async fn run_in_hermes(agent: AgentConfig, request: Dict) -> anyhow::Result<Dict> {
    use tokio::io::AsyncWriteExt;
    let Some((program, rest)) = agent.python.split_first() else {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no python for this agent").into());
    };
    let spawned = tokio::process::Command::new(program)
        .args(rest)
        .args(["-c", HERMES_SCRIPT])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true) // a timeout (or any early return) never leaves it running
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(exc) => {
            tracing::warn!(agent = %agent.name, python = %program, error = %exc,
                "vault: the agent's Hermes Python does not start");
            return Err(VaultError::Unavailable(agent.name.clone()).into());
        }
    };
    let mut payload = Dict::new();
    payload.insert("home".into(), json!(agent.hermes_home));
    payload.insert("root".into(), json!(agent.hermes_root));
    payload.extend(request);
    let payload = crate::py::dumps(&Value::Object(payload));
    let mut stdin = child.stdin.take();
    let writing = async move {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(payload.as_bytes()).await;
        }
        drop(stdin);
    };
    // what it prints, capped (an agent's own vault, read whole by `export`: never more than this kept in memory)
    let (mut out_pipe, mut err_pipe) = (child.stdout.take(), child.stderr.take());
    let child_ref = &mut child;
    let past_cap = tokio::sync::Notify::new();
    let reading = async {
        use tokio::io::AsyncReadExt;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // both read to their end (a pipe left full would block Hermes until the timeout, under the vault's lock):
        // stdout kept up to the cap, then Hermes stopped at once (an answer that long is no answer: review pass 5, 1;
        // its pipes close with it), stderr's last 64 KB
        let read_out = async {
            if let Some(pipe) = out_pipe.as_mut() {
                let mut chunk = vec![0u8; 64 * 1024];
                while let Ok(n) = pipe.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    out.extend_from_slice(&chunk[..n]);
                    if out.len() > HERMES_OUTPUT_MAX {
                        let _ = child_ref.start_kill();
                        past_cap.notify_one(); // stderr not waited for (a process it started may hold it open)
                        break;
                    }
                }
            }
        };
        let read_err = async {
            if let Some(pipe) = err_pipe.as_mut() {
                let mut chunk = vec![0u8; 16 * 1024];
                loop {
                    let n = tokio::select! {
                        read = pipe.read(&mut chunk) => match read {
                            Ok(n) if n > 0 => n,
                            _ => break,
                        },
                        () = past_cap.notified() => break,
                    };
                    err.extend_from_slice(&chunk[..n]);
                    if err.len() > 64 * 1024 {
                        err.drain(..err.len() - 64 * 1024);
                    }
                }
            }
        };
        tokio::join!(read_out, read_err);
        (out, err)
    };
    let ((), (stdout, stderr)) = tokio::time::timeout(Duration::from_secs(60), async { tokio::join!(writing, reading) })
        .await
        .map_err(|_| anyhow::anyhow!("TimeoutError"))?;
    if stdout.len() > HERMES_OUTPUT_MAX {
        let _ = child.start_kill();
        let _ = child.wait().await;
        tracing::warn!(agent = %agent.name, "vault: the agent's Hermes answered more than the bridge reads");
        return Err(VaultError::Invalid(format!("{}: Hermes' answer is too long", agent.name)).into());
    }
    drop((out_pipe, err_pipe));
    let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
        .await
        .map_err(|_| anyhow::anyhow!("TimeoutError"))??;
    let output = std::process::Output { status, stdout, stderr };
    if !output.status.success() {
        // Hermes' VaultError messages never contain secrets; keep only the last line.
        let err = String::from_utf8_lossy(&output.stderr).into_owned();
        let lines = crate::hermes::splitlines(pyx::strip(&err));
        let last = lines.last().cloned().unwrap_or_else(|| "?".into());
        if !refused_by_vault(&last) {
            // Not the vault judging the item: its Python failing before (an import, the request never read…).
            tracing::warn!(agent = %agent.name, python = %program, error = %pyx::tail(&last, 200),
                "vault: the agent's Hermes vault could not be reached");
            return Err(VaultError::Unavailable(agent.name.clone()).into());
        }
        tracing::info!(agent = %agent.name, error = %pyx::tail(&last, 200), "vault: Hermes refused the item");
        return Err(VaultError::Invalid(format!(
            "{}: Hermes refused the item ({})",
            agent.name,
            pyx::tail(&last, 200)
        ))
        .into());
    }
    let unexpected = || VaultError::Invalid(format!("{}: unexpected answer from Hermes", agent.name));
    let out = String::from_utf8(output.stdout).map_err(|_| unexpected())?;
    let last = crate::hermes::splitlines(pyx::strip(&out)).last().cloned().ok_or_else(unexpected)?;
    match serde_json::from_str::<Value>(&last).map_err(|_| unexpected())? {
        Value::Object(map) => Ok(map),
        other => Err(anyhow::anyhow!("TypeError: {} is not a dict", crate::py::py_repr(&other))),
    }
}

/// Whether stderr's last line is Hermes' vault refusing the item itself (`agent.vault_store.VaultError: …`, what the
/// person may change), not its Python failing before it got there.
pub fn refused_by_vault(last_line: &str) -> bool {
    let line = last_line.trim_start();
    line.starts_with("agent.vault_store.VaultError") || line.starts_with("VaultError")
}

/// `[str(a) for a in value]` (a str gives its characters, a dict its keys; a number cannot be iterated).
fn strs_of(value: &Value) -> anyhow::Result<Vec<String>> {
    match value {
        Value::Array(items) => Ok(items.iter().map(crate::py::py_str).collect()),
        Value::String(s) => Ok(s.chars().map(|c| c.to_string()).collect()),
        Value::Object(map) => Ok(map.keys().cloned().collect()),
        other => Err(anyhow::anyhow!("TypeError: {} object is not iterable", crate::py::py_repr(other))),
    }
}

fn given<'a>(body: &'a Dict, key: &str) -> Option<&'a Value> {
    body.get(key).filter(|v| !v.is_null())
}

fn truthy(body: &Dict, key: &str) -> bool {
    body.get(key).map(crate::py::truthy).unwrap_or(false)
}

/// The person's credentials (Fernet-encrypted `vault.json.enc`, its key `vault.key`), given to the agents' own
/// Hermes vaults.
pub struct Vault {
    pub folder: PathBuf,
    pub path: PathBuf,
    pub key_path: PathBuf,
    pub run: VaultRun,
    lock: tokio::sync::Mutex<()>,
}

impl Vault {
    pub fn new(folder: &str) -> Self {
        Self::with_run(folder, Arc::new(|agent, request| Box::pin(run_in_hermes(agent, request))))
    }

    pub fn with_run(folder: &str, run: VaultRun) -> Self {
        let folder = PathBuf::from(folder);
        Vault {
            path: folder.join("vault.json.enc"),
            key_path: folder.join("vault.key"),
            folder,
            run,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    // -- storage ---------------------------------------------------------------------------------------

    fn fernet(&self) -> anyhow::Result<fernet::Fernet> {
        crate::py::makedirs(&self.folder.to_string_lossy(), 0o700)?;
        // a place enrolled for sealed keys: its key from keys.sealed.json, never a file (sealed_keys.rs)
        if let Some(key) = crate::sealed_keys::data_key(&self.folder, crate::sealed_keys::DataKey::Vault)? {
            return fernet::Fernet::new(&key)
                .ok_or_else(|| anyhow::anyhow!("the vault's sealed key is not a Fernet key"));
        }
        if !self.key_path.exists() {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut fh = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&self.key_path)?;
            fh.write_all(fernet::Fernet::generate_key().as_bytes())?;
        }
        let key = std::fs::read_to_string(&self.key_path)?;
        fernet::Fernet::new(pyx::strip(&key))
            .ok_or_else(|| VaultError::Invalid("Fernet key must be 32 url-safe base64-encoded bytes.".into()).into())
    }

/// Logs the hub's agents that hold credentials while the hub hosts an others-facing agent (`hub_holders_at_risk`):
    /// their items stay in their profile's vault (nothing is taken back by itself), new secrets move them.
    pub fn warn_hub_holders(&self, config: &Config) {
        let at_risk = hub_holders_at_risk(config, &self.read().unwrap_or_default());
        if !at_risk.is_empty() {
            tracing::warn!(target: "bipbridge::vault", agents = ?at_risk, "the hub hosts an agent other people talk to, \
                and these agents of the hub hold credentials there: they stay until the agent moves or the person \
                takes them back; a new secret moves the agent into its own space");
        }
    }

    fn read(&self) -> anyhow::Result<Vec<Dict>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let token = std::fs::read_to_string(&self.path)?;
        let plain = self
            .fernet()?
            .decrypt(pyx::strip(&token))
            .map_err(|_| invalid("vault cannot be decrypted with its key"))?;
        match serde_json::from_slice::<Value>(&plain) {
            Ok(Value::Array(rows)) => Ok(rows.into_iter().filter_map(|r| r.as_object().cloned()).collect()),
            Ok(_) => Ok(Vec::new()),
            Err(_) => Err(VaultError::Invalid(pyx::json_error(&plain).unwrap_or_default()).into()),
        }
    }

    fn write(&self, records: &[Dict]) -> anyhow::Result<()> {
        let text = crate::py::dumps(&Value::Array(records.iter().cloned().map(Value::Object).collect()));
        let blob = self.fernet()?.encrypt(text.as_bytes());
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = self.path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        crate::py::write_with_mode(&tmp.to_string_lossy(), blob.as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn item(record: &Dict) -> Item {
        let meta: Map<String, Value> =
            record.iter().filter(|(k, _)| k.as_str() != "secret").map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut item: Item = serde_json::from_value(Value::Object(meta)).unwrap_or_default();
        if item.kind == crate::vault_contacts::KIND {
            let secret = match record.get("secret") {
                Some(Value::Object(map)) => map.clone(),
                _ => Dict::new(),
            };
            item.contact = Some(crate::vault_contacts::public(&secret));
        }
        item
    }

    /// The items, or what kept them from being read.
    pub fn try_items(&self) -> anyhow::Result<Vec<Item>> {
        Ok(self.read()?.iter().map(Self::item).collect())
    }

    fn notices_path(&self) -> PathBuf {
        self.folder.join("vault-notices.json")
    }

    fn all_notices(&self) -> Vec<Notice> {
        std::fs::read(self.notices_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write_notices(&self, notices: &[Notice]) {
        let text = serde_json::to_vec(notices).unwrap_or_default();
        let path = self.notices_path();
        let tmp = path.with_file_name(format!(".vault-notices.{}.tmp", std::process::id()));
        let done = crate::py::makedirs(&self.folder.to_string_lossy(), 0o700)
            .and_then(|_| crate::py::write_with_mode(&tmp.to_string_lossy(), &text, 0o600))
            .and_then(|_| std::fs::rename(&tmp, &path));
        if let Err(exc) = done {
            tracing::error!(error = %exc, "vault notices not written");
        }
    }

    /// What the person is told once about the vault, not shown yet (the app's listing carries them).
    pub fn notices(&self) -> Vec<Notice> {
        self.all_notices().into_iter().filter(|n| !n.text.is_empty()).collect()
    }

    /// `(key, text)` told to the person, each once ever (a shown one is kept without its text, so never said again).
    fn add_notices(&self, new: &[(String, String)]) {
        let mut all = self.all_notices();
        let mut changed = false;
        for (key, text) in new {
            if all.iter().any(|n| &n.key == key) {
                continue;
            }
            all.push(Notice {
                id: format!("notice_{}", &crate::py::uuid4_hex()[..12]),
                text: text.clone(),
                key: key.clone(),
                at: now(),
            });
            changed = true;
        }
        if changed {
            self.write_notices(&all);
        }
    }

    /// A notice the app showed: not listed again. Whether it was there.
    pub fn notice_seen(&self, id: &str) -> bool {
        let mut all = self.all_notices();
        let Some(notice) = all.iter_mut().find(|n| n.id == id && !n.text.is_empty()) else { return false };
        notice.text.clear(); // its key kept: never said twice
        self.write_notices(&all);
        true
    }

    pub fn items(&self) -> Vec<Item> {
        self.try_items().unwrap_or_default()
    }

    // -- what the app sends ------------------------------------------------------------------------------

    /// A full record (metadata + secret) from the app's fields; `existing` for an update (fields left out
    /// are kept, the secret included).
    pub fn parse(body: &Dict, existing: Option<&Dict>) -> Result<Dict, VaultError> {
        Self::parse_with(body, existing, None)
    }

    /// `parse`, with a place of the offer's rules for cards (`phone`: their numbers stay on the phone).
    pub fn parse_with(
        body: &Dict,
        existing: Option<&Dict>,
        phone: Option<crate::cards::PhoneRules>,
    ) -> Result<Dict, VaultError> {
        let mut record = existing.cloned().unwrap_or_default();
        let kind = body
            .get("kind")
            .filter(|v| crate::py::truthy(v))
            .or_else(|| record.get("kind").filter(|v| crate::py::truthy(v)))
            .map(crate::py::py_str)
            .unwrap_or_else(|| "login".into());
        if !KINDS.contains(&kind.as_str()) {
            return Err(invalid("unknown kind"));
        }
        record.insert("kind".into(), json!(kind));
        let label = match given(body, "label") {
            Some(v) => crate::py::py_str(v),
            None => record.get("label").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default(),
        };
        let label = pyx::strip(&label).to_string();
        if label.is_empty() {
            return Err(invalid("name is required"));
        }
        record.insert("label".into(), json!(pyx::head(&label, 120)));
        let mut secret: Dict = match record.get("secret") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        if kind == "login" {
            if given(body, "site").is_some() || !truthy(&record, "origin") {
                let site = body.get("site").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default();
                let origin = normalize_origin(&site)?;
                // the login page typed is kept beside its site (16/11: a shared extranet, a page per agency); the
                // app sends the site back as its origin when the person changes something else: its page stays
                let page = login_url_of(&site, &origin);
                let same = record.get("origin").and_then(Value::as_str) == Some(origin.as_str());
                record.insert("origin".into(), json!(origin));
                if page.is_some() || !same || !record.contains_key("login_url") {
                    record.insert("login_url".into(), json!(page));
                }
            }
            if let (Some(page), Some(origin)) = (given(body, "login_url"), record.get("origin").and_then(Value::as_str)) {
                let kept = login_url_of(&crate::py::py_str(page), origin);
                record.insert("login_url".into(), json!(kept));
            }
            let id_type = body
                .get("identifier_type")
                .filter(|v| crate::py::truthy(v))
                .or_else(|| record.get("identifier_type").filter(|v| crate::py::truthy(v)))
                .map(crate::py::py_str)
                .unwrap_or_else(|| "email".into());
            if !IDENTIFIER_TYPES.contains(&id_type.as_str()) {
                return Err(invalid("unknown identifier type"));
            }
            let identifier = match given(body, "identifier") {
                Some(v) => crate::py::py_str(v),
                None => {
                    record.get("identifier").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default()
                }
            };
            let identifier = pyx::strip(&identifier).to_string();
            if identifier.is_empty() {
                return Err(invalid("user name is required"));
            }
            record.insert("identifier_type".into(), json!(id_type));
            record.insert("identifier".into(), json!(pyx::head(&identifier, 200)));
            if truthy(body, "password") {
                secret.insert("password".into(), json!(crate::py::py_str(&body["password"])));
            }
            if let Some(otp) = given(body, "otp_secret") {
                let otp = crate::py::py_str(otp);
                if !pyx::strip(&otp).is_empty() {
                    secret.insert("otp_secret".into(), json!(pyx::strip(&otp)));
                } else {
                    secret.shift_remove("otp_secret");
                }
            }
            if !secret.get("password").map(crate::py::truthy).unwrap_or(false) {
                return Err(invalid("password is required"));
            }
            let has_otp = secret.get("otp_secret").map(crate::py::truthy).unwrap_or(false);
            record.insert("has_otp".into(), json!(has_otp));
        } else if kind == crate::cards::KIND {
            match phone {
                Some(rules) => crate::cards::parse_phone(body, &mut record, &mut secret, rules)?,
                None => crate::cards::parse(body, &mut record, &mut secret)?,
            }
        } else {
            if kind == crate::vault_contacts::KIND {
                crate::vault_contacts::parse(body, &mut record, &mut secret)?;
            }
            let allowed: &[&str] = if kind == "payment" { &PAYMENT_FIELDS } else { &[] };
            for name in allowed {
                if let Some(v) = given(body, name) {
                    secret.insert((*name).into(), json!(pyx::strip(&crate::py::py_str(v))));
                }
            }
            let origin = if truthy(body, "site") {
                json!(normalize_origin(&crate::py::py_str(&body["site"]))?)
            } else {
                record.get("origin").cloned().unwrap_or(Value::Null)
            };
            record.insert("origin".into(), origin);
            record.insert("identifier_type".into(), Value::Null);
            record.insert("identifier".into(), Value::Null);
            record.insert("has_otp".into(), json!(false));
        }
        record.insert("secret".into(), Value::Object(secret));
        Ok(record)
    }

    pub fn to_hermes(record: &Dict) -> Dict {
        let mut secret: Dict = match record.get("secret") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        if record.get("kind").and_then(Value::as_str) == Some("login") {
            secret.insert("identifier_type".into(), record.get("identifier_type").cloned().unwrap_or(Value::Null));
            secret.insert("identifier".into(), record.get("identifier").cloned().unwrap_or(Value::Null));
        }
        if record.get("kind").and_then(Value::as_str) == Some(crate::vault_contacts::KIND) {
            secret = crate::vault_contacts::hermes_secret(&secret);
        }
        let mut out = Dict::new();
        out.insert("op".into(), json!("add"));
        out.insert("kind".into(), record.get("kind").cloned().unwrap_or(Value::Null));
        out.insert("label".into(), record.get("label").cloned().unwrap_or(Value::Null));
        out.insert("origin".into(), record.get("origin").cloned().unwrap_or(Value::Null));
        out.insert("secret".into(), Value::Object(secret));
        out
    }

    // -- operations ------------------------------------------------------------------------------------

    fn check(config: &Config, agent_ids: &[String]) -> Result<Vec<AgentConfig>, VaultError> {
        let mut agents = Vec::new();
        let mut reasons = IndexMap::new();
        for agent_id in agent_ids {
            let Some(agent) = config.agent(Some(agent_id)) else {
                tracing::info!(agent = %agent_id, "vault item refused: unknown agent");
                return Err(VaultError::Invalid(format!("unknown agent {agent_id}")));
            };
            if let Some(why) = holder(config, agent) {
                reasons.insert(agent_id.clone(), why);
            }
            agents.push(agent.clone());
        }
        if !reasons.is_empty() {
            tracing::info!(agents = ?reasons, "vault item refused: these agents cannot hold credentials");
            return Err(VaultError::CannotHold(reasons));
        }
        Ok(agents)
    }

    /// An others-facing agent (`others_facing`) gets a login only once the person confirmed it (`confirmed_others`
    /// in the body, the app's « … parle aussi avec … Continuer ? »), as for a secret card (secrets.rs `give`).
    /// A card is left out: each of its payments already asks the person (card_pay.rs).
    fn confirmed(config: &Config, body: &Dict, record: &Dict, agents: &[AgentConfig]) -> Result<(), VaultError> {
        if body.get("confirmed_others").and_then(Value::as_bool) == Some(true)
            || record.get("kind").and_then(Value::as_str) == Some(crate::cards::KIND)
        {
            return Ok(());
        }
        let others: Vec<String> =
            agents.iter().filter(|a| others_facing(config, &a.name)).map(|a| a.name.clone()).collect();
        if others.is_empty() {
            Ok(())
        } else {
            tracing::info!(agents = ?others, "vault item not given: other people talk to the agents, not confirmed");
            Err(VaultError::ConfirmOthers(others))
        }
    }

    /// The others-facing agents given the item now, which the person confirmed (`confirmed` passed): kept on the item
    /// (`confirmed_for`), so an agent that becomes others-facing later reads no « Coordonnées » it got before.
    fn keep_confirmed(config: &Config, record: &mut Dict, agents: &[AgentConfig]) {
        let facing: Vec<Value> =
            agents.iter().filter(|a| others_facing(config, &a.name)).map(|a| json!(a.name)).collect();
        if facing.is_empty() {
            return;
        }
        let mut kept = match record.get("confirmed_for") {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        for agent in facing {
            if !kept.contains(&agent) {
                kept.push(agent);
            }
        }
        record.insert("confirmed_for".into(), Value::Array(kept));
    }

    /// `agent`'s « refused » mark on `record` set (a line for the app) or cleared.
    fn set_refused(record: &mut Dict, agent: &str, line: Option<String>) {
        match line {
            Some(line) => {
                if !matches!(record.get("refused"), Some(Value::Object(_))) {
                    record.insert("refused".into(), json!({}));
                }
                if let Some(Value::Object(map)) = record.get_mut("refused") {
                    map.insert(agent.into(), json!(line));
                }
            }
            None => {
                let empty = match record.get_mut("refused") {
                    Some(Value::Object(map)) => {
                        map.shift_remove(agent);
                        map.is_empty()
                    }
                    _ => false,
                };
                if empty {
                    record.shift_remove("refused");
                }
            }
        }
    }

    fn holds(record: &Dict, agent: &str) -> bool {
        matches!(record.get("agents"), Some(Value::Object(map)) if map.contains_key(agent))
    }

    fn refused_for(record: &Dict, agent: &str) -> bool {
        matches!(record.get("refused"), Some(Value::Object(map)) if map.contains_key(agent))
    }

    /// The vault's record for an item only an agent's Hermes holds (`export`: one it saved itself, a Hermes sent to the
    /// place), as the app would have made it; Err when the vault can't keep it as it is.
    fn from_hermes(item: &Value) -> Result<Dict, VaultError> {
        let text = |v: Option<&Value>| v.filter(|v| crate::py::truthy(v)).map(crate::py::py_str);
        let secret = item.get("secret").and_then(Value::as_object).cloned().unwrap_or_default();
        let kind = text(item.get("kind")).unwrap_or_default();
        let mut body = Dict::new();
        body.insert("kind".into(), json!(kind));
        body.insert("label".into(), item.get("label").cloned().unwrap_or(Value::Null));
        if let Some(origin) = text(item.get("origin")) {
            body.insert("site".into(), json!(origin));
        }
        match kind.as_str() {
            "login" => {
                for key in ["identifier_type", "identifier"] {
                    if let Some(v) = text(item.get(key)).or_else(|| text(secret.get(key))) {
                        body.insert(key.into(), json!(v));
                    }
                }
                for key in ["password", "otp_secret"] {
                    if let Some(v) = text(secret.get(key)) {
                        body.insert(key.into(), json!(v));
                    }
                }
            }
            k if k == crate::vault_contacts::KIND => {
                for (key, _) in crate::vault_contacts::TEXT_FIELDS {
                    if let Some(v) = text(secret.get(key)) {
                        body.insert(key.into(), json!(v));
                    }
                }
                for (single, many) in [("phone", crate::vault_contacts::PHONES), ("email", crate::vault_contacts::EMAILS)] {
                    if let Some(v) = text(secret.get(single)) {
                        body.insert(many.into(), json!([v]));
                    }
                }
            }
            _ => return Err(invalid("an unknown kind")),
        }
        let mut record = Self::parse_with(&body, None, None)?;
        // Hermes' own date (when it saved it), so the person sees which of two is the newer
        let date = Self::hermes_date(item).unwrap_or_else(now);
        record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
        record.insert("agents".into(), json!({}));
        record.insert("created_at".into(), json!(date));
        record.insert("updated_at".into(), json!(date));
        Ok(record)
    }

    /// When Hermes saved `item` (its `created_at`), written as [`now`] writes dates.
    fn hermes_date(item: &Value) -> Option<String> {
        let text = item.get("created_at").and_then(Value::as_str)?;
        let at = chrono::DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&chrono::Utc);
        // never later than now (a clock wrong on the machine Hermes ran on)
        Some(at.min(chrono::Utc::now()).format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
    }

    /// A login of the vault for `item`'s site and user name (another password, maybe).
    fn same_login(record: &Dict, item: &Value) -> bool {
        let field = |v: Option<&Value>| v.filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default();
        let secret = item.get("secret").and_then(Value::as_object);
        let identifier = Some(field(item.get("identifier")))
            .filter(|i| !i.is_empty())
            .unwrap_or_else(|| field(secret.and_then(|s| s.get("identifier"))));
        let origin = normalize_origin(&field(item.get("origin"))).unwrap_or_default();
        item.get("kind").and_then(Value::as_str) == Some("login")
            && record.get("kind").and_then(Value::as_str) == Some("login")
            && record.get("duplicate_of").is_none()
            && !origin.is_empty()
            && record.get("origin").and_then(Value::as_str) == Some(origin.as_str())
            && !identifier.is_empty()
            && record.get("identifier").and_then(Value::as_str).is_some_and(|i| i.trim() == identifier.trim())
    }

    /// The vault's own record of `item` (the same entry: a login's site, user name and password; « Coordonnées »' every
    /// field Hermes holds).
    fn same_entry(record: &Dict, item: &Value) -> bool {
        let secret = item.get("secret").and_then(Value::as_object).cloned().unwrap_or_default();
        match item.get("kind").and_then(Value::as_str) {
            Some("login") => {
                // its authenticator key too: one only Hermes holds is taken over below; two different keys are two
                // entries (the import kept beside, `duplicate_of`)
                let otp = |v: Option<&Value>| v.map(crate::py::py_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                let (ours, theirs) = (otp(record.get("secret").and_then(|s| s.get("otp_secret"))), otp(secret.get("otp_secret")));
                Self::same_login(record, item)
                    && record.get("secret").and_then(|s| s.get("password")) == secret.get("password")
                    && (ours.is_none() || theirs.is_none() || ours == theirs)
            }
            Some(k) if k == crate::vault_contacts::KIND && is_address(record) => {
                let ours = Self::to_hermes(record);
                let strings = |m: &Dict| -> std::collections::BTreeMap<String, String> {
                    m.iter().map(|(k, v)| (k.clone(), crate::py::py_str(v).trim().to_string())).filter(|(_, v)| !v.is_empty()).collect()
                };
                ours.get("label") == item.get("label")
                    && ours.get("secret").and_then(Value::as_object).map(strings) == Some(strings(&secret))
            }
            _ => false,
        }
    }

    fn handles(record: &mut Dict) -> &mut Dict {
        if !matches!(record.get("agents"), Some(Value::Object(_))) {
            record.insert("agents".into(), Value::Object(Dict::new()));
        }
        match record.get_mut("agents") {
            Some(Value::Object(map)) => map,
            _ => unreachable!("just made"),
        }
    }

    async fn give(&self, config: &Config, record: &mut Dict, agents: &[AgentConfig]) -> anyhow::Result<()> {
        let card = record.get("kind").and_then(Value::as_str) == Some(crate::cards::KIND);
        for agent in agents {
            Self::set_refused(record, &agent.name, None); // given again by the person: a refusal of before forgotten
            if card {
                // A card stays in the bridge: the agent only gets the right to ask for it (card_pay.rs).
                Self::handles(record).insert(agent.name.clone(), json!(CARD_HANDLE));
                continue;
            }
            if is_address(record) && !browser_on(config, agent) {
                // « Coordonnées » reach the agent's Hermes vault only for its browser fill: without its browser the
                // entry stays the bridge's (read with vault_contact_details), and Hermes shows no vault tools for it
                // (`follow_browser` writes it when the browser is turned on).
                Self::handles(record).insert(agent.name.clone(), json!(CARD_HANDLE));
                continue;
            }
            let answer = match async {
                Self::hermes_ready(config, agent)?;
                (self.run)(vault_target(config, agent), Self::to_hermes(record)).await
            }
            .await
            {
                Ok(answer) => answer,
                Err(exc) if record.get("kind").and_then(Value::as_str) == Some(crate::vault_contacts::KIND) => {
                    // A Hermes without patch 0017 refuses an address without street, city, postal code and country:
                    // the entry stays the bridge's (the agent reads it), without its browser fill.
                    tracing::warn!(agent = %agent.name, error = %exc, "contact details not written into the agent's \
                        Hermes vault: its browser won't fill them");
                    Self::handles(record).insert(agent.name.clone(), json!(CARD_HANDLE));
                    continue;
                }
                Err(exc) => return Err(exc),
            };
            let handle = answer.get("id").ok_or_else(|| anyhow::anyhow!("KeyError: 'id'"))?;
            Self::handles(record).insert(agent.name.clone(), json!(crate::py::py_str(handle)));
            let item = record.get("id").map(crate::py::py_str).unwrap_or_default();
            tracing::info!(agent = %agent.name, item = %item, "vault item given");
        }
        Ok(())
    }

    /// Takes the item back from these agents' Hermes vaults. A removal Hermes did not confirm keeps its handle in
    /// `vault-removals.json` (never only a log line): retried at the next change of the vault and at the bridge's
    /// start, and listed for the app (`removing`) until Hermes confirms it.
    async fn take_back(&self, config: &Config, record: &mut Dict, agent_ids: &[String]) {
        let item = record.get("id").map(crate::py::py_str).unwrap_or_default();
        let mut waiting = self.retried_removals(config).await;
        for agent_id in agent_ids {
            let handle = Self::handles(record).shift_remove(agent_id);
            let handle = handle.filter(|h| crate::py::truthy(h) && h.as_str() != Some(CARD_HANDLE));
            if let Some(handle) = handle {
                if let Err(exc) = self.remove_from(config, agent_id, &handle).await {
                    tracing::warn!(agent = %agent_id, item = %item, error = %exc,
                        "vault item not removed from the agent's Hermes vault: kept, retried later");
                    let row = json!({"agent": agent_id, "id": handle, "item": item});
                    waiting.extend(row.as_object().cloned());
                    continue;
                }
            }
            tracing::info!(agent = %agent_id, item = %item, "vault item taken back");
        }
        self.write_removals(&waiting);
    }

    /// Removes `handle` from `agent_id`'s Hermes vault. Nothing to do for an agent no longer configured (its vault
    /// went with it) or one with no Hermes to run.
    async fn remove_from(&self, config: &Config, agent_id: &str, handle: &Value) -> anyhow::Result<()> {
        let Some(agent) = config.agent(Some(agent_id)).map(|a| vault_target(config, a)) else { return Ok(()) };
        if agent.python.is_empty() || agent.hermes_home.as_deref().is_none_or(str::is_empty) {
            return Ok(());
        }
        Self::hermes_ready(config, &agent)?;
        let mut request = Dict::new();
        request.insert("op".into(), json!("remove"));
        request.insert("id".into(), handle.clone());
        (self.run)(agent, request).await.map(|_| ())
    }

    fn removals_path(&self) -> PathBuf {
        self.folder.join("vault-removals.json")
    }

    /// The removals Hermes has not confirmed yet: `{agent, id (its handle), item}`.
    fn removals(&self) -> Vec<Dict> {
        match std::fs::read(self.removals_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            Some(Value::Array(rows)) => rows.into_iter().filter_map(|r| r.as_object().cloned()).collect(),
            _ => Vec::new(),
        }
    }

    fn write_removals(&self, rows: &[Dict]) {
        let path = self.removals_path();
        let done = if rows.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            let text = crate::py::dumps(&Value::Array(rows.iter().cloned().map(Value::Object).collect()));
            let tmp = path.with_file_name(format!(".vault-removals.{}.tmp", std::process::id()));
            crate::py::makedirs(&self.folder.to_string_lossy(), 0o700)
                .and_then(|_| crate::py::write_with_mode(&tmp.to_string_lossy(), text.as_bytes(), 0o600))
                .and_then(|_| std::fs::rename(&tmp, &path))
        };
        if let Err(exc) = done {
            tracing::error!(error = %exc, waiting = rows.len(), "vault removals not written");
        }
    }

    /// The waiting removals tried again (the vault's lock held): those Hermes still does not confirm.
    async fn retried_removals(&self, config: &Config) -> Vec<Dict> {
        let mut left = Vec::new();
        for row in self.removals() {
            let agent = row.get("agent").and_then(Value::as_str).unwrap_or("");
            let handle = row.get("id").cloned().unwrap_or(Value::Null);
            match self.remove_from(config, agent, &handle).await {
                Ok(()) => tracing::info!(agent = %agent, item = ?row.get("item"), "vault item taken back (retried)"),
                Err(_) => left.push(row),
            }
        }
        left
    }

    /// The waiting removals tried again (at the bridge's start): how many are still waiting.
    pub async fn retry_removals(&self, config: &Config) -> usize {
        let _lock = self.lock.lock().await;
        if !self.removals_path().exists() {
            return 0;
        }
        if self.copies_ready(config).await.is_err() {
            return self.removals().len(); // closed: tried again once it reopens
        }
        let left = self.retried_removals(config).await;
        self.write_removals(&left);
        if !left.is_empty() {
            tracing::warn!(waiting = left.len(), "vault items still in agents' Hermes vaults: their removal is retried");
        }
        left.len()
    }

    /// `(agent, item)` of each removal still waiting, for the app.
    pub fn removing(&self) -> Vec<(String, String)> {
        let text = |row: &Dict, key: &str| row.get(key).map(crate::py::py_str).unwrap_or_default();
        self.removals().iter().map(|r| (text(r, "agent"), text(r, "item"))).collect()
    }

    // -- the agents' Hermes copies in memory (hermes_copies.rs, a place enrolled for sealed keys) -------------------

    /// Whether the bridge may reach `agent`'s Hermes vault now: always on a place not enrolled (today's layout); once
    /// enrolled, only through a copy checked in memory (never a copy made on the disk).
    fn hermes_ready(config: &Config, agent: &AgentConfig) -> anyhow::Result<()> {
        if !crate::hermes_copies::in_memory(config) || crate::hermes_copies::link_of(config, agent).is_none() {
            return Ok(());
        }
        if crate::sealed_keys::closed(config) {
            return Err(crate::sealed_keys::Locked::now(config).into());
        }
        match crate::hermes_copies::checked(std::path::Path::new(&config.data_dir()), &agent.name) {
            Some(_) => Ok(()),
            None => Err(VaultError::Unavailable(agent.name.clone()).into()),
        }
    }

    /// Before the bridge gives or takes back (the lock held, before the records are read: a rebuild writes new
    /// handles): each agent's copy ready in memory. Closed: [`crate::sealed_keys::Locked`]. An agent whose copy isn't
    /// ready is refused at its own give or take-back ([`Self::hermes_ready`]), the others go on. Not enrolled: nothing.
    async fn copies_ready(&self, config: &Config) -> anyhow::Result<()> {
        if !crate::hermes_copies::in_memory(config) {
            return Ok(());
        }
        if crate::sealed_keys::closed(config) {
            return Err(crate::sealed_keys::Locked::now(config).into());
        }
        let data = PathBuf::from(config.data_dir());
        for agent in config.agents.values() {
            // one that just failed waits for the bridge's own retry: not run again under the lock at each give
            if crate::hermes_copies::failed_within(&data, &agent.name, COPY_RETRY_PAUSE) {
                continue;
            }
            if let Err(exc) = self.copy_ready_noted(config, agent).await {
                tracing::warn!(target: "bipbridge::vault", agent = %agent.name, error = %format!("{exc:#}"),
                    "vault: the agent's Hermes copy is not ready in memory");
            }
        }
        Ok(())
    }

    /// `agent`'s copy ready in memory (an agent just moved into its space: its copy rebuilt there).
    pub async fn copies_ready_for(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<()> {
        if !crate::hermes_copies::in_memory(config) {
            return Ok(());
        }
        if crate::sealed_keys::closed(config) {
            return Err(crate::sealed_keys::Locked::now(config).into());
        }
        let _lock = self.lock.lock().await;
        self.copy_ready_noted(config, agent).await.map(|_| ())
    }

    /// An agent back in the shared hub (own_space_back.rs): what it holds, in its hub profile's Hermes vault. A place
    /// whose copies are in memory rebuilds its copy from the master copy (`copies_ready_for`: nothing of its space's
    /// copy is read). Elsewhere its Hermes vault came with its profile (as on the way out): each item its listing
    /// misses is written again from the master copy, with a new handle, never twice. Returns how many were written.
    pub async fn held_again(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<usize> {
        if crate::hermes_copies::in_memory(config) {
            self.copies_ready_for(config, agent).await?;
            return Ok(0);
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let handle_of = |record: &Dict| -> Option<String> {
            let handle = record.get("agents")?.get(&agent.name)?.as_str()?;
            (!handle.is_empty() && handle != CARD_HANDLE).then(|| handle.to_string())
        };
        let held: Vec<(usize, String)> =
            records.iter().enumerate().filter_map(|(i, r)| Some((i, handle_of(r)?))).collect();
        if held.is_empty() {
            return Ok(0);
        }
        let mut request = Dict::new();
        request.insert("op".into(), json!("list"));
        let answer = (self.run)(vault_target(config, agent), request).await?;
        let listed: std::collections::HashSet<String> = Self::listed(&answer).into_iter().map(|(id, _, _)| id).collect();
        let mut again = 0;
        let mut failed = None;
        for (i, handle) in held {
            if listed.contains(&handle) {
                continue;
            }
            match self.give(config, &mut records[i], std::slice::from_ref(agent)).await {
                Ok(()) => again += 1,
                Err(exc) => {
                    failed = Some(exc);
                    break;
                }
            }
        }
        if again > 0 {
            self.write(&records)?; // the new handles, those written before a failure too
            tracing::info!(target: "bipbridge::vault", agent = %agent.name, items = again,
                "vault: items written again into the agent's Hermes vault in the shared hub");
        }
        match failed {
            Some(exc) => Err(exc),
            None => Ok(again),
        }
    }

    /// [`Self::copy_ready_locked`], its failure noted ([`crate::hermes_copies::failed`]).
    async fn copy_ready_noted(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<CopyReady> {
        let done = self.copy_ready_locked(config, agent).await;
        crate::hermes_copies::set_failed(std::path::Path::new(&config.data_dir()), &agent.name, done.is_err());
        done
    }

    /// Every agent's copy ready in memory (at the bridge's start, once the vault is open): checked, rebuilt where it is
    /// missing or not right, the disk's copies erased once their rebuild is checked; then what no agent uses tidied
    /// (folders in memory; once every agent's copy is ready, copies left on the disk). Not enrolled, or closed: nothing.
    pub async fn copies_ready_all(&self, config: &Config) -> CopiesReport {
        let mut report = CopiesReport::default();
        if !crate::hermes_copies::in_memory(config) || crate::sealed_keys::closed(config) {
            return report;
        }
        let data = PathBuf::from(config.data_dir());
        let _lock = self.lock.lock().await;
        // an agent deleted (or moved off this place) meanwhile: no longer counted as failing (/health, the retry)
        crate::hermes_copies::keep_failed(&data, &config.agents.values().map(|a| a.name.clone()).collect());
        let mut agents: Vec<&AgentConfig> = config.agents.values().collect();
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        for agent in agents {
            match self.copy_ready_noted(config, agent).await {
                Ok(CopyReady::Checked(erased)) => {
                    report.checked.push(agent.name.clone());
                    report.erased += erased;
                }
                Ok(CopyReady::Rebuilt(erased)) => {
                    report.rebuilt.push(agent.name.clone());
                    report.erased += erased;
                }
                Ok(CopyReady::Elsewhere) => {}
                Err(exc) => {
                    tracing::warn!(target: "bipbridge::vault", agent = %agent.name, error = %format!("{exc:#}"),
                        "vault: the agent's Hermes copy is not ready in memory (its copy on the disk, if any, kept)");
                    report.failed.push(agent.name.clone());
                }
            }
        }
        let links: Vec<PathBuf> =
            config.agents.values().filter_map(|a| crate::hermes_copies::link_of(config, a)).collect();
        if let Ok(root) = crate::hermes_copies::root(&data, false) {
            let keep = links.iter().filter_map(|l| crate::hermes_copies::current(l, &root)).collect();
            crate::hermes_copies::tidy(&root, &keep);
        }
        if report.failed.is_empty() {
            for stray in crate::hermes_copies::strays(config) {
                match crate::hermes_copies::erase(&stray) {
                    Ok(files) => report.erased += files,
                    Err(exc) => tracing::warn!(target: "bipbridge::vault", folder = %stray.display(), error = %exc,
                        "vault: a Hermes copy left on the disk not erased"),
                }
            }
        }
        tracing::info!(target: "bipbridge::vault", checked = ?report.checked, rebuilt = ?report.rebuilt,
            failed = ?report.failed, erased = report.erased, "vault: the agents' Hermes copies are in memory");
        report
    }

    /// `agent`'s copy in memory, the lock held: checked once per process (its link to one of this place's folders in
    /// memory, its listing exactly the agent's handles), else rebuilt in one Hermes run into a new folder, the link
    /// swapped, the handles written; only then a copy on the disk erased.
    async fn copy_ready_locked(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<CopyReady> {
        use crate::hermes_copies as copies;
        let data = PathBuf::from(config.data_dir());
        let Some(link) = copies::link_of(config, agent) else { return Ok(CopyReady::Elsewhere) };
        if !link.parent().is_some_and(std::path::Path::is_dir) {
            return Ok(CopyReady::Elsewhere); // no profile of its Hermes here yet: nothing to hold (never made by us)
        }
        let root = copies::root(&data, true)?;
        let current = copies::current(&link, &root);
        if current.is_some() && copies::checked(&data, &agent.name) == current {
            return Ok(CopyReady::Checked(0));
        }
        let profile = link.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        // Hermes' copies on the disk: its folder (today's layout, before its first rebuild), or one a cut-off swap set
        // aside (the link gone)
        let mut disks: Vec<PathBuf> = Vec::new();
        if std::fs::symlink_metadata(&link).is_ok_and(|m| m.is_dir()) {
            disks.push(link.clone());
        }
        disks.extend(copies::set_aside_in(&profile).into_iter().filter(|p| {
            p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(copies::SET_ASIDE[0]))
                && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
        }));
        // A set-aside copy whose handles were written already (marked: the bridge stopped before its erase, or the erase
        // failed) or that our own erase left half done (its key, or its file, gone or zeroed): nothing to read, erased
        // with the rest. Only a whole copy not marked is read.
        let (readable, spent): (Vec<PathBuf>, Vec<PathBuf>) =
            disks.iter().cloned().partition(|d| *d == link || copies::readable_set_aside(d));
        if !spent.is_empty() {
            tracing::info!(target: "bipbridge::vault", agent = %agent.name, folders = spent.len(),
                "vault: copies set aside after their handles were written (or half erased): erased, not read");
        }
        let mut records = self.read()?;
        let mut expected: Vec<(usize, String)> = records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let handle = r.get("agents")?.get(&agent.name)?.as_str()?;
                (!handle.is_empty() && handle != CARD_HANDLE).then(|| (i, handle.to_string()))
            })
            .collect();
        let target = vault_target(config, agent);
        // refused at an earlier rebuild: tried again at this one (once per process: a start, a give after a failure)
        let refused: Vec<usize> = (0..records.len())
            .filter(|i| Self::refused_for(&records[*i], &agent.name) && !Self::holds(&records[*i], &agent.name))
            .collect();
        if let Some(folder) = current.as_ref().filter(|_| readable.is_empty() && refused.is_empty()) {
            let fits = if expected.is_empty() && !folder.join("vault.json.enc").exists() {
                true
            } else {
                let mut request = Dict::new();
                request.insert("op".into(), json!("list"));
                match (self.run)(target.clone(), request).await {
                    Ok(answer) => {
                        let listed: Vec<String> = Self::listed(&answer).into_iter().map(|(id, _, _)| id).collect();
                        let mut wanted: Vec<String> = expected.iter().map(|(_, h)| h.clone()).collect();
                        let mut listed_sorted = listed.clone();
                        wanted.sort();
                        listed_sorted.sort();
                        listed_sorted == wanted
                    }
                    Err(exc) => {
                        tracing::info!(target: "bipbridge::vault", agent = %agent.name, error = %exc,
                            "vault: the agent's Hermes copy could not be listed: rebuilt");
                        false
                    }
                }
            };
            if fits {
                copies::set_checked(&data, &agent.name, folder);
                // a swap cut off before its erase (the bridge stopped, a put back failed): what it set aside goes now
                return Ok(CopyReady::Checked(copies::erase_set_aside(&profile)));
            }
        }
        copies::unset_checked(&data, &agent.name);
        // Hermes' copy on the disk still there (today's layout, before its first rebuild): it keeps working if this one
        // fails, so all or nothing. Without one (a server restart emptied the memory), an item Hermes refuses is left
        // out (no longer given to this agent: the app shows it so) and the rest is rebuilt.
        let disk_at_stake = !readable.is_empty();
        // What only its Hermes holds on the disk (a login it saved itself, a Hermes sent to the place: to_place.py) is
        // taken into the master copy first, never erased with its copy: read once (`export`), made the vault's own
        // records given to this agent, rebuilt with the rest. Not taken: what the person took back from it (a removal
        // waiting: never given again), what the vault holds already (the same entry: given, or kept as it is), a
        // Hermes card (never kept with its security code: the person is told to add it in the Coffre). One the vault
        // can't keep: its copy stays as it is.
        let known: std::collections::HashSet<String> = expected.iter().map(|(_, h)| h.clone()).collect();
        let removed: std::collections::HashSet<String> = self
            .removals()
            .iter()
            .filter(|r| r.get("agent").and_then(Value::as_str) == Some(agent.name.as_str()))
            .filter_map(|r| r.get("id").map(crate::py::py_str))
            .collect();
        let lang = crate::handback::language_of(config, agent);
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut notices: Vec<(String, String)> = Vec::new();
        let mut dated = false;
        for disk in &readable {
            let mut request = Dict::new();
            request.insert("op".into(), json!("export"));
            request.insert("vault".into(), json!(disk.to_string_lossy()));
            let answer = (self.run)(target.clone(), request).await?;
            let items = match answer.get("items") {
                Some(Value::Array(items)) => items.clone(),
                _ => anyhow::bail!("{}: Hermes gave no listing of its copy on the disk", agent.name),
            };
            for item in items {
                let id = item.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                if id.is_empty() || known.contains(&id) || removed.contains(&id) || !taken.insert(id.clone()) {
                    continue;
                }
                let kind = item.get("kind").and_then(Value::as_str).unwrap_or("");
                let label = item.get("label").map(crate::py::py_str).unwrap_or_default();
                if kind == "payment" {
                    // its name as the person is told it, never a card's number, cut (the key is kept in clear in
                    // vault-notices.json: review pass 5, 4)
                    let named = card_name(&label);
                    notices.push((format!("card_not_kept:{}:{named}", agent.name), card_not_kept_text(&lang, &named)));
                    continue;
                }
                // the same entry the vault holds already (a cut-off swap's copy, Hermes saving it again)
                if let Some(i) = (0..records.len()).find(|i| Self::same_entry(&records[*i], &item)) {
                    // an authenticator key only Hermes held: kept in the vault's entry (review pass 4, R-I)
                    let theirs = item.get("secret").and_then(|s| s.get("otp_secret")).map(crate::py::py_str);
                    let has_ours = records[i].get("secret").and_then(|s| s.get("otp_secret")).is_some_and(crate::py::truthy);
                    if let (Some(otp), false) = (theirs.filter(|t| !t.trim().is_empty()), has_ours) {
                        if let Some(Value::Object(secret)) = records[i].get_mut("secret") {
                            secret.insert("otp_secret".into(), json!(otp.trim()));
                        }
                        records[i].insert("has_otp".into(), json!(true));
                        dated = true;
                    }
                    if let Some(newer) = Self::hermes_date(&item).filter(|d| {
                        records[i].get("updated_at").and_then(Value::as_str).is_none_or(|u| d.as_str() > u)
                    }) {
                        records[i].insert("updated_at".into(), json!(newer));
                        dated = true;
                    }
                    if !expected.iter().any(|(e, _)| *e == i) && !Self::refused_for(&records[i], &agent.name) {
                        expected.push((i, id));
                    }
                    continue;
                }
                let mut record = Self::from_hermes(&item).map_err(|_| {
                    // never the entry's values (a phone, an address): its kind and its name only
                    anyhow::anyhow!("{}: the entry « {} » ({kind}) only its Hermes holds can't be taken into the vault \
                                     as it is: its copy on the disk kept", agent.name, pyx::head(&label, 60))
                })?;
                // the same login with another password: kept too (never lose either), the person's own filled first
                if let Some(original) = records.iter().find(|r| Self::same_login(r, &item)) {
                    record.insert("duplicate_of".into(), original.get("id").cloned().unwrap_or(Value::Null));
                }
                records.push(record);
                expected.push((records.len() - 1, id));
            }
        }
        if !taken.is_empty() {
            tracing::info!(target: "bipbridge::vault", agent = %agent.name, items = taken.len(),
                "vault: the items of the agent's Hermes copy on the disk looked at (those only it held taken in)");
        }
        let held = expected.len();
        let wanted: Vec<usize> = expected.iter().map(|(i, _)| *i).chain(refused.iter().copied()).collect();
        let folder = copies::new_folder(&root, &agent.name)?;
        let made: anyhow::Result<Vec<Option<String>>> = async {
            if wanted.is_empty() {
                return Ok(Vec::new());
            }
            let items: Vec<Dict> = wanted
                .iter()
                .map(|i| {
                    let mut item = Self::to_hermes(&records[*i]);
                    item.shift_remove("op");
                    item
                })
                .collect();
            let mut request = Dict::new();
            request.insert("op".into(), json!("add_many"));
            request.insert("vault".into(), json!(folder.to_string_lossy()));
            request.insert("items".into(), Value::Array(items.iter().cloned().map(Value::Object).collect()));
            let answer = (self.run)(target.clone(), request).await?;
            let ids: Vec<Option<String>> = match answer.get("ids") {
                Some(Value::Array(ids)) => ids.iter().map(|v| v.as_str().map(str::to_string)).collect(),
                _ => anyhow::bail!("{}: Hermes gave no ids back", agent.name),
            };
            if ids.len() != items.len() {
                anyhow::bail!("{}: Hermes gave {} ids for {} items", agent.name, ids.len(), items.len());
            }
            // checked: every item made is listed back, as it was given, and nothing else
            let listed = Self::listed(&answer);
            let made: Vec<&String> = ids.iter().flatten().collect();
            if listed.len() != made.len() {
                anyhow::bail!("{}: Hermes lists {} items of {} made", agent.name, listed.len(), made.len());
            }
            for (n, (item, id)) in items.iter().zip(&ids).enumerate() {
                let Some(id) = id else {
                    if n >= held {
                        continue; // refused before, refused again: still not given
                    }
                    // refused: only « Coordonnées » stay the bridge's (an older Hermes without patch 0017), as at a give
                    if item.get("kind").and_then(Value::as_str) == Some(crate::vault_contacts::KIND) || !disk_at_stake {
                        continue;
                    }
                    anyhow::bail!("{}: Hermes refused an item it held", agent.name);
                };
                let found = listed.iter().find(|(listed_id, _, _)| listed_id == id);
                let same = found.is_some_and(|(_, kind, label)| {
                    Some(kind.as_str()) == item.get("kind").and_then(Value::as_str)
                        && Some(label.as_str()) == item.get("label").and_then(Value::as_str).map(str::trim)
                });
                if !same {
                    anyhow::bail!("{}: an item made is not listed back as it was given", agent.name);
                }
            }
            Ok(ids)
        }
        .await;
        let ids = match made {
            Ok(ids) => ids,
            Err(exc) => {
                let _ = std::fs::remove_dir_all(&folder);
                return Err(exc);
            }
        };
        let previous = std::fs::read_link(&link).ok().filter(|p| p.parent() == Some(root.as_path()));
        let aside = copies::point(&link, &folder).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&folder);
        })?;
        let mut left_out = 0;
        if !wanted.is_empty() {
            for (i, id) in wanted.iter().zip(&ids) {
                let contact = is_address(&records[*i]);
                match id {
                    Some(id) => {
                        Self::handles(&mut records[*i]).insert(agent.name.clone(), json!(id));
                        Self::set_refused(&mut records[*i], &agent.name, None);
                    }
                    None if contact => {
                        Self::handles(&mut records[*i]).insert(agent.name.clone(), json!(CARD_HANDLE));
                        Self::set_refused(&mut records[*i], &agent.name, None);
                    }
                    None => {
                        Self::handles(&mut records[*i]).shift_remove(&agent.name);
                        Self::set_refused(&mut records[*i], &agent.name, Some(AGENT_REFUSED.to_string()));
                        left_out += 1;
                    }
                }
            }
            if let Err(exc) = self.write(&records) {
                copies::put_back(&link, previous.as_deref(), &aside);
                let _ = std::fs::remove_dir_all(&folder);
                return Err(exc);
            }
        } else if dated {
            // what Hermes had newer (a date, an authenticator key only it held: nothing given to this agent now): its
            // copy on the disk is erased only once this is written (review pass 5, 3)
            if let Err(exc) = self.write(&records) {
                copies::put_back(&link, previous.as_deref(), &aside);
                let _ = std::fs::remove_dir_all(&folder);
                return Err(exc);
            }
        }
        // the handles written: what was set aside is never read again (erased now, or at the next pass if cut off)
        for folder in aside.iter().chain(&disks) {
            copies::mark_handles_written(folder);
        }
        copies::set_checked(&data, &agent.name, &folder);
        // the removals waiting for this agent are done: its new copy holds only what it is given
        let removals = self.removals();
        if removals.iter().any(|r| r.get("agent").and_then(Value::as_str) == Some(agent.name.as_str())) {
            let left: Vec<Dict> = removals
                .into_iter()
                .filter(|r| r.get("agent").and_then(Value::as_str) != Some(agent.name.as_str()))
                .collect();
            self.write_removals(&left);
        }
        self.add_notices(&notices);
        let mut erased = 0;
        for disk in aside {
            match copies::erase(&disk) {
                Ok(files) => erased += files,
                Err(exc) => tracing::warn!(target: "bipbridge::vault", agent = %agent.name, error = %exc,
                    "vault: the agent's Hermes copy on the disk not erased"),
            }
        }
        erased += copies::erase_set_aside(&profile); // what an earlier swap cut off left (the bridge stopped)
        if left_out > 0 {
            tracing::warn!(target: "bipbridge::vault", agent = %agent.name, items = left_out,
                "vault: items its Hermes refused are marked refused for the agent (the rest rebuilt; tried again later)");
        }
        if let Some(previous) = previous.filter(|p| *p != folder) {
            let _ = std::fs::remove_dir_all(previous);
        }
        tracing::info!(target: "bipbridge::vault", agent = %agent.name, items = ids.len(), erased,
            "vault: the agent's Hermes copy rebuilt in memory");
        Ok(CopyReady::Rebuilt(erased))
    }

    /// (id, kind, label) of each item of a Hermes listing (`{"items": [...]}`).
    fn listed(answer: &Dict) -> Vec<(String, String, String)> {
        let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        match answer.get("items") {
            Some(Value::Array(items)) => {
                items.iter().map(|i| (text(i, "id"), text(i, "kind"), text(i, "label"))).collect()
            }
            _ => Vec::new(),
        }
    }

    pub async fn create(&self, config: &Config, body: &Dict) -> anyhow::Result<Item> {
        if !config.hermes.isolated {
            return Err(VaultError::Unprotected.into());
        }
        let mut record = Self::parse_with(body, None, crate::cards::phone_rules(config))?;
        Self::cards_allowed(config, &record, true)?;
        let wanted = match body.get("agents").filter(|v| crate::py::truthy(v)) {
            Some(v) => strs_of(v)?,
            None => Vec::new(),
        };
        let agents = Self::check(config, &wanted)?;
        Self::confirmed(config, body, &record, &agents)?;
        Self::keep_confirmed(config, &mut record, &agents);
        record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
        record.insert("agents".into(), json!({}));
        record.insert("created_at".into(), json!(now()));
        record.insert("updated_at".into(), json!(now()));
        let _lock = self.lock.lock().await;
        self.copies_ready(config).await?;
        if let Err(exc) = self.give(config, &mut record, &agents).await {
            let given: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
            self.take_back(config, &mut record, &given).await;
            return Err(exc);
        }
        let mut records = self.read()?;
        records.push(record.clone());
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    pub async fn update(&self, config: &Config, item_id: &str, body: &Dict) -> anyhow::Result<Item> {
        if !config.hermes.isolated {
            return Err(VaultError::Unprotected.into());
        }
        let _lock = self.lock.lock().await;
        self.copies_ready(config).await?;
        let mut records = self.read()?;
        let Some(index) = records.iter().position(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let before = records[index].clone();
        let mut record = Self::parse_with(body, Some(&before), crate::cards::phone_rules(config))?;
        Self::cards_allowed(config, &record, false)?;
        let before_agents = match before.get("agents") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        record.insert("agents".into(), Value::Object(before_agents.clone()));
        let had: Vec<String> = before_agents.keys().cloned().collect();
        let wanted = match given(body, "agents") {
            Some(v) => strs_of(v)?,
            None => had.clone(),
        };
        let changed = ["label", "origin", "identifier", "identifier_type", "secret"]
            .iter()
            .any(|k| record.get(*k) != before.get(*k));
        // A changed item is written again everywhere (Hermes has no update): taken back, then given.
        let to_give: Vec<String> = wanted.iter().filter(|a| !had.contains(a) || changed).cloned().collect();
        let give = Self::check(config, &to_give)?;
        // only an agent that gets this login now: one that already holds it was confirmed when it got it
        let added: Vec<AgentConfig> = give.iter().filter(|a| !had.contains(&a.name)).cloned().collect();
        Self::confirmed(config, body, &record, &added)?;
        Self::keep_confirmed(config, &mut record, &added);
        if let Some(Value::Array(confirmed)) = record.get_mut("confirmed_for") {
            confirmed.retain(|a| a.as_str().is_some_and(|a| wanted.iter().any(|w| w == a)));
        }
        // the person's choice wins over a refusal of an agent's Hermes: not wanted for it now, never given it later
        let refused_agents: Vec<String> = match record.get("refused") {
            Some(Value::Object(map)) => map.keys().cloned().collect(),
            _ => Vec::new(),
        };
        for agent in refused_agents.iter().filter(|a| !wanted.contains(a)) {
            Self::set_refused(&mut record, agent, None);
        }
        let to_take: Vec<String> = had.iter().filter(|a| !wanted.contains(a) || changed).cloned().collect();
        self.take_back(config, &mut record, &to_take).await;
        self.give(config, &mut record, &give).await?;
        record.insert("updated_at".into(), json!(now()));
        records[index] = record.clone();
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    pub async fn delete(&self, config: &Config, item_id: &str) -> anyhow::Result<()> {
        let _lock = self.lock.lock().await;
        self.copies_ready(config).await?;
        let records = self.read()?;
        let Some(mut record) = records.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)).cloned()
        else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let all: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
        self.take_back(config, &mut record, &all).await;
        let kept: Vec<Dict> =
            records.into_iter().filter(|r| r.get("id").and_then(Value::as_str) != Some(item_id)).collect();
        self.write(&kept)
    }

    /// A card added while payments are off (or kept on an install that forbids them): refused. Hermes' own `payment`
    /// items are never kept: they would hold the security code, in the agent's own vault (cards replace them).
    fn cards_allowed(config: &Config, record: &Dict, creating: bool) -> Result<(), VaultError> {
        let kind = record.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "payment" {
            return Err(VaultError::Invalid("payment items are not kept: add a card".into()));
        }
        let on = if creating { crate::cards::enabled(config) } else { crate::cards::allowed(config) };
        if kind == crate::cards::KIND && !on {
            return Err(VaultError::CardsOff);
        }
        Ok(())
    }

    // -- cards (card_pay.rs) ---------------------------------------------------------------------------

    /// The card `label` that `agent` may use (case and spaces ignored): its id, label, public part and numbers.
    /// Not found: the labels of the cards it may use, for the agent.
    pub fn card_for(&self, agent: &str, label: &str) -> anyhow::Result<Result<CardFound, Vec<String>>> {
        let wanted = pyx::strip(label).to_lowercase();
        let mut labels = Vec::new();
        for record in self.read()? {
            let allowed = matches!(record.get("agents"), Some(Value::Object(a)) if a.contains_key(agent));
            if record.get("kind").and_then(Value::as_str) != Some(crate::cards::KIND) || !allowed {
                continue;
            }
            let name = record.get("label").map(crate::py::py_str).unwrap_or_default();
            if pyx::strip(&name).to_lowercase() == wanted {
                let object = |key: &str| match record.get(key) {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Dict::new(),
                };
                return Ok(Ok(CardFound {
                    id: record.get("id").map(crate::py::py_str).unwrap_or_default(),
                    label: name,
                    meta: object("card"),
                    secret: object("secret"),
                }));
            }
            labels.push(name);
        }
        Ok(Err(labels))
    }

    /// The cards `agent` may use: label and public part.
    pub fn cards_of(&self, agent: &str) -> Vec<(String, Value)> {
        self.items()
            .into_iter()
            .filter(|i| i.kind == crate::cards::KIND && i.agents.contains_key(agent))
            .map(|i| (i.label.clone(), i.card.clone().unwrap_or(Value::Null)))
            .collect()
    }

    /// The payment `payment` of `cents` in `currency` with card `item_id` kept on it, its card about to be typed
    /// (audit 17/11, 2.3): under the vault's lock, the card read again (removed, or its cap lowered, meanwhile) and its
    /// cap checked for this payment alone (a limit per payment, Alex 18/11); a single-use card used meanwhile, or being
    /// used by another payment, is refused. Err: what to tell the agent (nothing is typed).
    pub async fn reserve_card_payment(
        &self,
        item_id: &str,
        payment: &str,
        cents: i64,
        currency: &str,
    ) -> anyhow::Result<Result<crate::cards::Reservation, String>> {
        let _lock = self.lock.lock().await;
        let records = self.read()?;
        let Some(record) = records.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Ok(Err("The card was removed from the vault meanwhile: nothing was typed. Don't pay.".into()));
        };
        let label = record.get("label").map(crate::py::py_str).unwrap_or_default();
        let meta = record.get("card").cloned().unwrap_or(Value::Null);
        let others = crate::cards::in_flight_on(item_id);
        let single_use = meta.get("single_use").and_then(Value::as_bool) == Some(true);
        if single_use && (meta.get("used").and_then(Value::as_bool) == Some(true) || others > 0) {
            return Ok(Err(format!(
                "The single-use card « {label} » was used by another payment meanwhile: nothing was typed. Don't pay; \
                 tell the person."
            )));
        }
        if let Some(why) = crate::cards::over_cap(&meta, &label, cents, currency) {
            return Ok(Err(format!("Nothing was typed. {why}")));
        }
        Ok(Ok(crate::cards::Reservation::take(payment, item_id)))
    }

    /// A payment made with card `item_id`, kept on it (a single-use card is then used); its reservation
    /// (`reserve_card_payment`) ends under the same lock, counted in `spent` (shown in the app) from now on.
    pub async fn record_card_payment(
        &self,
        item_id: &str,
        payment: Value,
        reservation: Option<crate::cards::Reservation>,
    ) -> anyhow::Result<()> {
        let _lock = self.lock.lock().await;
        let _ends_here = reservation;
        let mut records = self.read()?;
        let Some(record) = records.iter_mut().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let mut meta = match record.get("card") {
            Some(Value::Object(m)) => m.clone(),
            _ => Dict::new(),
        };
        crate::cards::record_payment(&mut meta, payment);
        record.insert("card".into(), Value::Object(meta));
        self.write(&records)
    }

    /// The payment `payment` of card `item_id` taken off its `spent` and its payments, under the vault's lock: its pay
    /// button was never pressed (review 18/11, R1). A single-use card stays used. False: no such payment on it.
    pub async fn unrecord_card_payment(&self, item_id: &str, payment: &str) -> anyhow::Result<bool> {
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let Some(record) = records.iter_mut().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Ok(false);
        };
        let Some(Value::Object(mut meta)) = record.get("card").cloned() else { return Ok(false) };
        if !crate::cards::unrecord_payment(&mut meta, payment) {
            return Ok(false);
        }
        record.insert("card".into(), Value::Object(meta));
        self.write(&records)?;
        Ok(true)
    }

    // -- « Coordonnées » (vault_contacts.rs) ------------------------------------------------------------------

    /// The « Coordonnées » entries `agent` may read (`vault_contacts::readable_by`), fields included.
    pub fn contacts_for(&self, config: &Config, agent: &str) -> anyhow::Result<Vec<crate::vault_contacts::Entry>> {
        Ok(crate::vault_contacts::entries_for(config, &self.read()?, agent))
    }

    /// Details the person gave `agent` in a conversation, kept as an entry of theirs (`created_by`) given to that agent
    /// only; the same label again changes an entry that agent saved, never one the person made.
    pub async fn save_contact(&self, config: &Config, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Item> {
        if !config.hermes.isolated {
            return Err(VaultError::Unprotected.into());
        }
        let mut body = Dict::new();
        for (name, _) in crate::vault_contacts::TEXT_FIELDS {
            if let Some(v) = given(arguments, name) {
                body.insert(name.into(), v.clone());
            }
        }
        for key in [crate::vault_contacts::PHONES, crate::vault_contacts::EMAILS, "label", "hint"] {
            if let Some(v) = given(arguments, key) {
                body.insert(key.into(), v.clone());
            }
        }
        let label = body.get("label").map(crate::py::py_str).unwrap_or_default();
        let label = crate::cards::quoted(&label, 120);
        if label.is_empty() {
            return Err(invalid("give the entry a label").into());
        }
        body.insert("label".into(), json!(label));
        body.insert("kind".into(), json!(crate::vault_contacts::KIND));
        let same = self.items().into_iter().find(|i| {
            i.kind == crate::vault_contacts::KIND && pyx::strip(&i.label).to_lowercase() == label.to_lowercase()
        });
        if let Some(existing) = same {
            if existing.created_by.as_deref() != Some(agent.name.as_str()) {
                return Err(VaultError::Invalid(format!(
                    "an entry « {} » is already in the vault and only the person changes it: use another label, or \
                     tell them to edit it in the vault",
                    existing.label
                ))
                .into());
            }
            body.shift_remove("label"); // its name as the person last saw it
            return self.update(config, &existing.id, &body).await;
        }
        if let Some(why) = holder(config, agent) {
            return Err(VaultError::CannotHold([(agent.name.clone(), why)].into_iter().collect()).into());
        }
        body.insert("agents".into(), json!([agent.name]));
        let item = self.create(config, &body).await?;
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let Some(record) = records.iter_mut().find(|r| r.get("id").and_then(Value::as_str) == Some(item.id.as_str()))
        else {
            return Err(NoSuchItem(item.id).into());
        };
        record.insert("created_by".into(), json!(agent.name));
        let item = Self::item(record);
        self.write(&records)?;
        Ok(item)
    }

    /// The « Coordonnées » given to `agent` follow its browser (2026-10-29, model bench: each entry written into its
    /// Hermes vault showed Hermes' five `browser_vault_*` tools, ≈ 1 220 tokens per call, to agents that never
    /// browse): written into its Hermes vault while its browser is on, taken out of it when it is off; the bridge's own
    /// copy stays (`vault_contact_details`). Logins and cards are left as they are. Whether anything changed.
    pub async fn follow_browser(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<bool> {
        if !self.path.exists() {
            return Ok(false);
        }
        let on = browser_on(config, agent);
        let _lock = self.lock.lock().await;
        self.copies_ready(config).await?;
        let mut records = self.read()?;
        let mut changed = false;
        for record in records.iter_mut().filter(|r| is_address(r)) {
            let handle = match record.get("agents").and_then(|a| a.get(&agent.name)) {
                Some(handle) => crate::py::py_str(handle),
                None => continue,
            };
            let item = record.get("id").map(crate::py::py_str).unwrap_or_default();
            if on && handle == CARD_HANDLE {
                // its browser now fills it (a Hermes without patch 0017 may refuse it: then it stays the bridge's)
                let given = async {
                    Self::hermes_ready(config, agent)?;
                    (self.run)(vault_target(config, agent), Self::to_hermes(record)).await
                };
                match given.await {
                    Ok(answer) => {
                        let Some(id) = answer.get("id").map(crate::py::py_str) else { continue };
                        Self::handles(record).insert(agent.name.clone(), json!(id));
                        tracing::info!(agent = %agent.name, item = %item, "contact details given to the agent's browser");
                        changed = true;
                    }
                    Err(exc) => tracing::warn!(agent = %agent.name, item = %item, error = %exc,
                        "contact details not written into the agent's Hermes vault: its browser won't fill them"),
                }
            } else if !on && handle != CARD_HANDLE {
                let mut request = Dict::new();
                request.insert("op".into(), json!("remove"));
                request.insert("id".into(), json!(handle));
                let removed = async {
                    Self::hermes_ready(config, agent)?;
                    (self.run)(vault_target(config, agent), request).await
                };
                if let Err(exc) = removed.await {
                    tracing::warn!(agent = %agent.name, item = %item, error = %exc,
                        "contact details not removed from the agent's Hermes vault");
                    continue;
                }
                Self::handles(record).insert(agent.name.clone(), json!(CARD_HANDLE));
                tracing::info!(agent = %agent.name, item = %item, "contact details taken out of the agent's Hermes vault: \
                    no browser");
                changed = true;
            }
        }
        if changed {
            self.write(&records)?;
        }
        Ok(changed)
    }

    /// `follow_browser` for every agent (at the bridge's start: entries written before 2026-10-29 for agents without
    /// their browser, and browsers turned on or off while it was stopped).
    pub async fn follow_browsers(&self, config: &Config) -> bool {
        let mut changed = false;
        for agent in config.agents.values() {
            match self.follow_browser(config, agent).await {
                Ok(c) => changed |= c,
                Err(exc) => tracing::warn!(agent = %agent.name, error = %exc, "contact details not matched to its browser"),
            }
        }
        changed
    }

    // -- logins created by an agent (vault_logins.rs) ---------------------------------------------------------

    /// The login kept for `origin` and `identifier` (case ignored), if any.
    pub fn login_for(&self, origin: &str, identifier: &str) -> anyhow::Result<Option<Item>> {
        let wanted = pyx::strip(identifier).to_lowercase();
        Ok(self.read()?.iter().map(Self::item).find(|i| {
            i.kind == "login"
                && i.origin.as_deref() == Some(origin)
                && i.identifier.as_deref().map(|x| pyx::strip(x).to_lowercase()).as_deref() == Some(wanted.as_str())
        }))
    }

    /// A login `agent` created with a password of the vault, kept « en attente de confirmation » and written into that
    /// agent's own Hermes vault (so it can log in with `browser_vault_fill` later). The one it created before for the
    /// same site and identifier, still waiting, is replaced (a password the site refused). Never another one.
    pub async fn save_created_login(
        &self,
        config: &Config,
        agent: &AgentConfig,
        origin: &str,
        identifier: &str,
        password: &str,
    ) -> anyhow::Result<Item> {
        if let Some(why) = holder(config, agent) {
            return Err(VaultError::CannotHold([(agent.name.clone(), why)].into_iter().collect()).into());
        }
        let _lock = self.lock.lock().await;
        self.copies_ready(config).await?;
        let mut records = self.read()?;
        let wanted = pyx::strip(identifier).to_lowercase();
        let index = records.iter().position(|r| {
            r.get("kind").and_then(Value::as_str) == Some("login")
                && r.get("origin").and_then(Value::as_str) == Some(origin)
                && r.get("identifier").and_then(Value::as_str).map(|x| pyx::strip(x).to_lowercase()).as_deref()
                    == Some(wanted.as_str())
        });
        let host = origin.split("://").nth(1).unwrap_or(origin).to_string();
        let mut body = Dict::new();
        body.insert("kind".into(), json!("login"));
        body.insert("label".into(), json!(host));
        body.insert("site".into(), json!(origin));
        body.insert("identifier_type".into(), json!(identifier_type_of(identifier)));
        body.insert("identifier".into(), json!(identifier));
        body.insert("password".into(), json!(password));
        let mut record = match index {
            Some(at) => {
                let before = records[at].clone();
                let item = Self::item(&before);
                if item.created_by.as_deref() != Some(agent.name.as_str()) || !item.pending {
                    return Err(invalid("this login is already in the vault").into());
                }
                let mut record = Self::parse(&body, Some(&before))?;
                let had: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
                self.take_back(config, &mut record, &had).await;
                record
            }
            None => {
                let mut record = Self::parse(&body, None)?;
                record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
                record.insert("agents".into(), json!({}));
                record.insert("created_at".into(), json!(now()));
                record
            }
        };
        record.insert("created_by".into(), json!(agent.name));
        record.insert("pending".into(), json!(true));
        record.insert("confirmed_at".into(), Value::Null);
        record.insert("updated_at".into(), json!(now()));
        if let Err(exc) = self.give(config, &mut record, std::slice::from_ref(agent)).await {
            let given: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
            self.take_back(config, &mut record, &given).await;
            return Err(exc);
        }
        match index {
            Some(at) => records[at] = record.clone(),
            None => records.push(record.clone()),
        }
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    /// The site accepted the sign-up: the login is no longer « en attente de confirmation ». `agent`: only one it
    /// created (the person confirms any).
    pub async fn confirm_login(&self, item_id: &str, agent: Option<&str>) -> anyhow::Result<Item> {
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let Some(record) = records.iter_mut().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let item = Self::item(record);
        if item.created_by.is_none() || agent.is_some_and(|a| item.created_by.as_deref() != Some(a)) {
            return Err(NoSuchItem(item_id.to_string()).into());
        }
        if item.pending {
            record.insert("pending".into(), json!(false));
            record.insert("confirmed_at".into(), json!(now()));
            record.insert("updated_at".into(), json!(now()));
        }
        let item = Self::item(record);
        self.write(&records)?;
        Ok(item)
    }

    /// The password of a login an agent created, for the person (the app asks for Face ID first): they never typed
    /// it, so it is the only way they can know it. Items the person entered stay write-only.
    pub fn created_password(&self, item_id: &str) -> anyhow::Result<zeroize::Zeroizing<String>> {
        let records = self.read()?;
        let Some(record) = records.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let item = Self::item(record);
        if item.kind != "login" || item.created_by.is_none() {
            return Err(VaultError::Invalid("only a login created by an agent can be shown".into()).into());
        }
        let password = record.get("secret").and_then(|s| s.get("password")).and_then(Value::as_str).unwrap_or("");
        Ok(zeroize::Zeroizing::new(password.to_string()))
    }

    /// The logins given to `agent` (metadata), for `vault_login` to choose from (vault_match.rs).
    /// An agent other people talk to: only the logins the person confirmed for it (`confirmed_for`, review R4), as
    /// for « Coordonnées » (vault_contacts.rs `readable_by`).
    pub fn logins_of(&self, config: &Config, agent: &str) -> anyhow::Result<Vec<Item>> {
        let facing = others_facing(config, agent);
        let confirmed = |r: &Dict| {
            matches!(r.get("confirmed_for"), Some(Value::Array(a)) if a.iter().any(|v| v.as_str() == Some(agent)))
        };
        Ok(self
            .read()?
            .iter()
            .filter(|r| !facing || confirmed(r))
            .map(Self::item)
            .filter(|i| i.kind == "login" && i.agents.contains_key(agent))
            .collect())
    }

    /// The password of a login given to `agent`, for `vault_login` to type it into the agent's page: it stays in the
    /// bridge (never in an answer, a log, the agent's Hermes).
    pub fn login_password(&self, item_id: &str, agent: &str) -> anyhow::Result<zeroize::Zeroizing<String>> {
        let records = self.read()?;
        let Some(record) = records.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let item = Self::item(record);
        if item.kind != "login" || !item.agents.contains_key(agent) {
            return Err(NoSuchItem(item_id.to_string()).into());
        }
        let password = record.get("secret").and_then(|s| s.get("password")).and_then(Value::as_str).unwrap_or("");
        Ok(zeroize::Zeroizing::new(password.to_string()))
    }

    /// An agent deleted: its handles go (its own vault is deleted with it).
    pub async fn forget_agent(&self, _config: &Config, agent_id: &str) -> anyhow::Result<()> {
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        // the secrets it asked for go with it (nobody else may use them)
        records.retain(|r| !Self::is_secret_of(r, agent_id, None));
        for record in records.iter_mut() {
            Self::handles(record).shift_remove(agent_id);
        }
        let mut waiting = self.removals();
        waiting.retain(|r| r.get("agent").and_then(Value::as_str) != Some(agent_id));
        self.write_removals(&waiting);
        self.write(&records)
    }

    // -- secrets an agent asked for (secrets.rs) --------------------------------------------------------------------

    fn is_secret_of(record: &Dict, agent: &str, name: Option<&str>) -> bool {
        record.get("kind").and_then(Value::as_str) == Some(crate::secrets::VAULT_KIND)
            && record.get("created_by").and_then(Value::as_str) == Some(agent)
            && name.is_none_or(|n| record.get("label").and_then(Value::as_str) == Some(n))
    }

    /// The secret `new.name` of `agent` kept (replacing the one of that name). `value: None` (a one-time code): its
    /// name and identifier only, never a value.
    pub async fn save_secret(&self, config: &Config, agent: &AgentConfig, new: &NewSecret<'_>) -> anyhow::Result<Item> {
        if let Some(why) = holder(config, agent) {
            return Err(VaultError::CannotHold([(agent.name.clone(), why)].into_iter().collect()).into());
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let index = records.iter().position(|r| Self::is_secret_of(r, &agent.name, Some(new.name)));
        let mut record = match index {
            Some(at) => records[at].clone(),
            None => {
                let mut record = Dict::new();
                record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
                record.insert("created_at".into(), json!(now()));
                record
            }
        };
        record.insert("kind".into(), json!(crate::secrets::VAULT_KIND));
        record.insert("label".into(), json!(new.name));
        record.insert("origin".into(), if new.origin.is_empty() { Value::Null } else { json!(new.origin) });
        let identifier = pyx::strip(new.identifier).to_string();
        if identifier.is_empty() {
            record.insert("identifier_type".into(), Value::Null);
            record.insert("identifier".into(), Value::Null);
        } else {
            record.insert("identifier_type".into(), json!(identifier_type_of(&identifier)));
            record.insert("identifier".into(), json!(pyx::head(&identifier, 200)));
        }
        record.insert("has_otp".into(), json!(false));
        record.insert("agents".into(), json!({agent.name.as_str(): CARD_HANDLE}));
        record.insert("created_by".into(), json!(agent.name));
        let last = record.get("secret_info").and_then(|i| i.get("last_used_at")).cloned().unwrap_or(Value::Null);
        record.insert(
            "secret_info".into(),
            json!({"kind": new.kind, "identifier_label": new.identifier_label, "last_used_at": last}),
        );
        let secret = match new.value {
            Some(value) => json!({"value": value}),
            None => json!({}),
        };
        record.insert("secret".into(), secret);
        record.insert("updated_at".into(), json!(now()));
        match index {
            Some(at) => records[at] = record.clone(),
            None => records.push(record.clone()),
        }
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    /// `agent`'s secrets (metadata).
    pub fn secret_items(&self, agent: &str) -> Vec<Item> {
        self.read().unwrap_or_default().iter().filter(|r| Self::is_secret_of(r, agent, None)).map(Self::item).collect()
    }

    /// `agent`'s secrets with a value: (name, item, value). Only for a connector's run and for blanking.
    pub fn secret_values(&self, agent: &str) -> anyhow::Result<Vec<(String, Item, zeroize::Zeroizing<String>)>> {
        Ok(self
            .read()?
            .iter()
            .filter(|r| Self::is_secret_of(r, agent, None))
            .filter_map(|r| {
                let value = r.get("secret").and_then(|s| s.get("value")).and_then(Value::as_str)?;
                let item = Self::item(r);
                Some((item.label.clone(), item, zeroize::Zeroizing::new(value.to_string())))
            })
            .collect())
    }

    /// When `agent`'s secrets `names` were last used (a connector's run).
    pub async fn touch_secrets(&self, agent: &str, names: &[String]) -> anyhow::Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let mut changed = false;
        for record in records.iter_mut() {
            let name = record.get("label").and_then(Value::as_str).unwrap_or("").to_string();
            if Self::is_secret_of(record, agent, None) && names.contains(&name) {
                if let Some(Value::Object(info)) = record.get_mut("secret_info") {
                    info.insert("last_used_at".into(), json!(now()));
                    changed = true;
                }
            }
        }
        if changed {
            self.write(&records)?;
        }
        Ok(())
    }
}

/// A secret as the person gave it (secrets.rs).
pub struct NewSecret<'a> {
    pub name: &'a str,
    /// `https://connect.garmin.com`, or empty.
    pub origin: &'a str,
    pub kind: &'a str,
    pub identifier_label: &'a str,
    pub identifier: &'a str,
    /// None for a one-time code: never kept.
    pub value: Option<&'a str>,
}

/// What an identifier looks like: an e-mail, a phone number, else a user name.
pub fn identifier_type_of(identifier: &str) -> &'static str {
    if identifier.contains('@') {
        "email"
    } else if identifier.chars().all(|c| c.is_ascii_digit() || " +-.()".contains(c))
        && identifier.chars().filter(char::is_ascii_digit).count() >= 6
    {
        "phone"
    } else {
        "username"
    }
}

// -- routes (bridge key; the app asks for Face ID before calling them) ------------------------------------

fn answer(exc: anyhow::Error) -> HttpError {
    if let Some(err) = exc.downcast_ref::<VaultError>() {
        return match err {
            VaultError::CannotHold(reasons) => HttpError::json(409, json!({"code": "cannot_hold", "agents": reasons})),
            VaultError::Unprotected => HttpError::json(409, json!({"code": "unprotected_install"})),
            VaultError::CardsOff => HttpError::json(409, json!({"code": "cards_off"})),
            VaultError::ConfirmOthers(agents) => HttpError::json(409, json!({"code": crate::secrets::CONFIRM_OTHERS,
                "agents": agents, "message": crate::secrets::CONFIRM_OTHERS_TEXT})),
            VaultError::Invalid(text) => HttpError::new(400, text.clone()),
            VaultError::Unavailable(agent) => {
                HttpError::json(503, json!({"code": "vault_unavailable", "agents": [agent]}))
            }
        };
    }
    if exc.downcast_ref::<NoSuchItem>().is_some() {
        return HttpError::new(404, "no such item");
    }
    HttpError::from(exc)
}

/// `await request.json()` for a route that answers its `ValueError` with a 400.
fn json_body(body: &Bytes) -> ApiResult<Value> {
    serde_json::from_slice::<Value>(body).map_err(|_| HttpError::new(400, pyx::json_error(body).unwrap_or_default()))
}

/// `body.get(...)` on what is not a dict: Python's AttributeError, a 500.
fn as_dict(value: Value) -> ApiResult<Dict> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(anyhow::anyhow!("AttributeError: {} has no attribute 'get'", crate::py::py_repr(&other)).into()),
    }
}

async fn listing(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let mut items: Vec<Value> = state
        .vault
        .try_items()?
        .iter()
        .map(|item| {
            let mut out = item.public();
            if let (Some(agent), Some(info)) = (&item.created_by, out.get_mut("secret")) {
                info["used_by"] = json!(crate::connectors::used_by(&config, agent, &item.label));
            }
            out
        })
        .collect();
    crate::vault_used::decorate(&state.vault.folder, &mut items); // « Récents » (vault_used.rs, 17/11)
    let agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| {
            let why = holder(&config, a);
            let mut row = json!({"id": a.name, "name": a.display_name, "can_hold": why.is_none(), "why": why});
            if others_facing(&config, &a.name) {
                // the app asks one confirmation before giving it a login (`confirmed_others`)
                row["others"] = json!(true);
                row["others_names"] = json!(others_names(&config, &a.name));
            }
            row
        })
        .collect();
    let mut out = json!({"protected": config.hermes.isolated, "items": items, "agents": agents,
                         "cards": crate::cards::enabled(&config), "cards_allowed": crate::cards::allowed(&config)});
    if crate::cards::remote_off() {
        // Kerfy Studio's cut (cards.rs `remote_off`): `cards` false meanwhile, a plain line for the app
        out["cards_suspended"] = json!(true);
        out["cards_notice"] = json!(crate::cards::REMOTE_OFF_NOTICE);
    }
    if let (Some(rules), Value::Object(map)) = (crate::cards::phone_rules(&config), &mut out) {
        // A place of the offer: cards' numbers stay on the phone, a cap is required (cards.rs).
        map.insert("cards_on_phone".into(), json!(true));
        map.insert("card_cap_max_eur".into(), json!(rules.cap_max_cents / 100));
    }
    let notices = state.vault.notices();
    if let (false, Value::Object(map)) = (notices.is_empty(), &mut out) {
        // told once (a card of an agent's Hermes not taken): the app shows each line, then `DELETE …/notices/{id}`
        let rows: Vec<Value> = notices.iter().map(|n| json!({"id": n.id, "text": n.text})).collect();
        map.insert("notices".into(), json!(rows));
    }
    let removing: Vec<Value> =
        state.vault.removing().into_iter().map(|(agent, item)| json!({"agent": agent, "item": item})).collect();
    if let (false, Value::Object(map)) = (removing.is_empty(), &mut out) {
        // taken back, but Hermes has not confirmed their removal from these agents' vaults yet (retried)
        map.insert("removing".into(), json!(removing));
    }
    Ok(web::ok(out))
}

async fn notice_seen(State(state): State<AppState>, Path(notice_id): Path<String>) -> ApiResult<Response> {
    if !state.vault.notice_seen(&notice_id) {
        return Err(HttpError::new(404, "no such notice"));
    }
    Ok(web::ok(json!({"ok": true})))
}

async fn create(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let body = json_body(&body)?;
    if !config.hermes.isolated {
        return Err(answer(VaultError::Unprotected.into()));
    }
    let body = as_dict(body)?;
    let item = state.vault.create(&config, &body).await.map_err(answer)?;
    crate::profile_files::reconcile_all(&config); // its vault tools in view for the agents that now hold an item
    Ok(web::json_response(201, &item.public()))
}

async fn update(State(state): State<AppState>, Path(item_id): Path<String>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let body = json_body(&body)?;
    if !config.hermes.isolated {
        return Err(answer(VaultError::Unprotected.into()));
    }
    let body = as_dict(body)?;
    let item = state.vault.update(&config, &item_id, &body).await.map_err(answer)?;
    crate::profile_files::reconcile_all(&config);
    Ok(web::ok(item.public()))
}

async fn delete(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let config = state.config();
    state.vault.delete(&config, &item_id).await.map_err(answer)?;
    crate::profile_files::reconcile_all(&config);
    Ok(web::no_content())
}

/// The person confirms a login an agent created (the site accepted it).
async fn confirm(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let item = state.vault.confirm_login(&item_id, None).await.map_err(answer)?;
    tracing::info!(item = %item_id, "created login confirmed by the person");
    Ok(web::ok(item.public()))
}

/// The password of a login an agent created, shown to the person (after Face ID in the app). Never logged.
async fn reveal(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let password = state.vault.created_password(&item_id).map_err(answer)?;
    tracing::info!(item = %item_id, "created login shown to the person");
    let mut response = web::ok(json!({"password": password.as_str()}));
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    Ok(response)
}

/// `vault_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/vault", get(listing).post(create))
        .route("/vault/{item_id}", patch(update).delete(delete))
        .route("/vault/notices/{notice_id}", axum::routing::delete(notice_seen))
        .route("/vault/{item_id}/confirm", axum::routing::post(confirm))
        .route("/vault/{item_id}/reveal", axum::routing::post(reveal))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A protected place whose hub serves `agents`, its data in `dir`.
    fn place(dir: &std::path::Path, agents: &[&str]) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.join("outbox.db").to_string_lossy().into_owned();
        config.hermes.isolated = true;
        config.hermes.bin = "/usr/bin/hermes".into();
        config.hermes.hub_url = "http://127.0.0.1:8642".into();
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.hermes.python = vec!["/usr/bin/python3".into()];
        for name in agents {
            let agent = AgentConfig {
                name: name.to_string(),
                hermes_url: format!("http://127.0.0.1:8642/p/{name}"),
                ..Default::default()
            };
            config.agents.insert(name.to_string(), agent);
        }
        config
    }

    fn may(config: &Config, name: &str) -> bool {
        hub_may_hold(config, &config.agents[name], &hub_breakers(config))
    }

    #[test]
    fn a_hub_of_the_person_s_own_agents_holds_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "cuisine"]);
        assert!(!others_facing(&config, "coach"));
        assert!(may(&config, "coach") && may(&config, "cuisine"));
        // the hub's Python and the profile's home as that Python sees it
        let target = vault_target(&config, &config.agents["coach"]);
        assert_eq!(target.python, vec!["/usr/bin/python3".to_string()]);
        assert_eq!(target.hermes_home, Some(dir.path().join("hermes/profiles/coach").to_string_lossy().into_owned()));
        let mut contained = config.clone();
        contained.hermes.container_home = "/opt/data".into();
        assert_eq!(vault_target(&contained, &config.agents["coach"]).hermes_home.as_deref(), Some("/opt/data/profiles/coach"));
        // not on an unprotected install, nor without the hub's Python
        let mut open = config.clone();
        open.hermes.isolated = false;
        assert!(!may(&open, "coach"));
        assert_eq!(holder(&open, &open.agents["coach"]).as_deref(), Some("unprotected_install"));
        let mut no_python = config.clone();
        no_python.hermes.python.clear();
        assert!(!may(&no_python, "coach"));
        assert_eq!(holder(&no_python, &no_python.agents["coach"]).as_deref(), Some("needs_own_space"));
    }

    #[test]
    fn a_space_reaches_its_vault_with_the_python_hermes_runs_on_today() {
        // Alex's install, 2026-11-09: spaces made on Hermes 517b5e10f (Python 3.14.7), then the move onto bf0203a794
        // (3.14.9, the old folder kept): every item given to a space was refused.
        let dir = tempfile::tempdir().unwrap();
        let mut config = place(dir.path(), &["admin-kerfy"]);
        let home = dir.path().join("hermes");
        let python = |folder: &str| {
            let bin = home.join("tools").join(folder).join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("python3"), "").unwrap();
            bin.join("python3").to_string_lossy().into_owned()
        };
        let before = python("python-3.14.7+20260901-linux-x64");
        let after = python("python-3.14.9+20261021-linux-x64");
        std::fs::write(home.join("tools/facts.json"),
                       r#"{"packages": {"python": {"entry": "python-3.14.9+20261021-linux-x64"}}}"#).unwrap();
        config.hermes.python = vec![before.clone(), "-I".into()]; // the bridge not restarted since the move
        let space = AgentConfig {
            name: "kerfy-studio".into(),
            hermes_url: "http://127.0.0.1:8660/p/kerfy-studio".into(),
            isolated: true,
            sandboxed: true,
            space: Some("/spaces/kerfy-studio".into()),
            python: vec![before.clone(), "-I".into()],
            hermes_home: Some("/spaces/kerfy-studio/profiles/kerfy-studio".into()),
            ..Default::default()
        };
        config.agents.insert(space.name.clone(), space.clone());
        assert_eq!(holder(&config, &space), None);
        assert_eq!(vault_target(&config, &space).python, vec![after.clone(), "-I".to_string()]);
        assert_eq!(vault_target(&config, &config.agents["admin-kerfy"]).python, vec![after, "-I".to_string()]);
    }

    #[test]
    fn only_the_vault_s_own_refusal_is_the_person_s_to_change() {
        assert!(refused_by_vault("agent.vault_store.VaultError: login items require identifier and password"));
        assert!(refused_by_vault("VaultError: origin not allowed"));
        // its Python failing before it read the item: the server's to fix (a 503, vault_unavailable)
        assert!(!refused_by_vault("json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)"));
        assert!(!refused_by_vault("ModuleNotFoundError: No module named 'agent'"));
        assert!(!refused_by_vault("?"));
    }

    #[test]
    fn only_a_project_agent_in_the_hub_keeps_everyone_moving() {
        // a project agent: its guests' turns are its own, the hub isn't private with it there
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "fete"]);
        let projects = r#"{"projects": {"fete": {"created": 1.0, "ends": 9999999999.0, "archived": null}}}"#;
        std::fs::write(dir.path().join("project-agents.json"), projects).unwrap();
        assert!(others_facing(&config, "fete") && !others_facing(&config, "coach"));
        assert!(!may(&config, "fete") && !may(&config, "coach"));
        assert_eq!(holder(&config, &config.agents["coach"]).as_deref(), Some("needs_own_space"));
        assert_eq!(holder(&config, &config.agents["fete"]).as_deref(), Some("needs_own_space"));
        // the start's tidy moves it out (own_space.rs), never on an unprotected install
        assert_eq!(crate::own_space::to_tidy(&config), vec!["fete".to_string()]);
        let mut open = config.clone();
        open.hermes.isolated = false;
        assert!(crate::own_space::to_tidy(&open).is_empty());
        // once it lives in its own space, the hub is private again
        let mut moved = config.clone();
        let fete = moved.agents.get_mut("fete").unwrap();
        fete.isolated = true;
        fete.space = Some("/srv/spaces/fete".into());
        fete.hermes_url = "http://127.0.0.1:8700".into();
        assert!(may(&moved, "coach"));
        assert!(crate::own_space::to_tidy(&moved).is_empty());
    }

    /// Alex 21/11: what comes from someone else runs as a guest turn, and no agent leaves the hub because others talk
    /// to it: the family's agent, an agent shared with a relative, one another household's agents write to (or that
    /// writes to theirs) stay in the hub, and the person's other agents keep their credentials there. Who may use
    /// which item stays as it was (`others_facing`: the person is asked first).
    #[test]
    fn agents_other_people_talk_to_stay_in_the_hub() {
        let cases: [(&str, &str, &str, bool); 4] = [
            ("family", "family-agent.json", r#"{"agent": "fete"}"#, true),
            ("shared", "agent-shares.json", r#"{"shared": {"fete": ["lea"]}}"#, true),
            ("written to", "agent-rules.json",
             r#"{"households": {"lea": {"in": {"mode": "only", "agents": ["fete"]}, "theirs": {"in": {"mode": "all"}}}}}"#,
             crate::rules::RECEIVERS_FACE_OTHERS),
            ("writing", "agent-rules.json",
             r#"{"households": {"lea": {"out": {"mode": "all"}, "theirs": {"out": {"mode": "all"}}}}}"#, false),
        ];
        for (which, file, text, facing) in cases {
            let dir = tempfile::tempdir().unwrap();
            let config = place(dir.path(), &["coach", "fete"]);
            std::fs::write(dir.path().join("households.json"), r#"{"households": {"lea": {"name": "Léa"}}}"#).unwrap();
            std::fs::write(dir.path().join(file), text).unwrap();
            assert_eq!(others_facing(&config, "fete"), facing, "{which}");
            assert!(!others_facing(&config, "coach"), "{which}");
            assert!(may(&config, "coach") && may(&config, "fete"), "{which}: the hub stays private");
            assert!(crate::own_space::to_tidy(&config).is_empty(), "{which}: nobody moves");
            assert!(!crate::own_space::should_leave_hub(&config, &config.agents["fete"]), "{which}");
        }
        // a link between two of the person's own agents changes nothing
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "cuisine"]);
        std::fs::write(dir.path().join("peers.json"), r#"{"links": [["coach", "cuisine"]]}"#).unwrap();
        assert!(may(&config, "coach") && !others_facing(&config, "cuisine"));
    }

    #[test]
    fn who_else_talks_to_an_agent_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "cuisine", "notes"]);
        let data = std::path::PathBuf::from(config.data_dir());
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("households.json"),
                       r#"{"households": {"lea": {"name": "Léa"}, "marc": {"name": "Marc"}, "anon": {}}}"#).unwrap();
        std::fs::write(data.join("agent-shares.json"), r#"{"shared": {"coach": ["lea", "gone"]}}"#).unwrap();
        // Marc's agents and the unnamed household's write to Cuisine, both sides agreeing (rules.rs)
        std::fs::write(data.join("agent-rules.json"), r#"{"write": "some", "pairs": [["coach", "notes"]], "households": {
            "marc": {"in": {"mode": "only", "agents": ["cuisine"]}, "theirs": {"in": {"mode": "all"}}},
            "anon": {"in": {"mode": "only", "agents": ["cuisine"]}, "theirs": {"in": {"mode": "all"}}}}}"#).unwrap();
        assert!(others_facing(&config, "coach"));
        assert_eq!(others_facing(&config, "cuisine"), crate::rules::RECEIVERS_FACE_OTHERS); // written to by a household
        assert_eq!(others_names(&config, "coach"), vec!["Léa".to_string()]);
        assert_eq!(others_names(&config, "Cuisine"), vec!["Marc".to_string()]); // a household with no name: left out
        assert!(others_names(&config, "notes").is_empty() && !others_facing(&config, "notes"));
    }

    #[test]
    fn credentials_left_in_a_hub_no_longer_private_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "fete"]);
        let records: Vec<Dict> = vec![json!({"agents": {"coach": "vault_1"}}).as_object().unwrap().clone()];
        assert!(hub_holders_at_risk(&config, &records).is_empty());
        // the family's agent in the hub: still private (its members' turns are guest turns, Alex 21/11)
        std::fs::write(dir.path().join("family-agent.json"), r#"{"agent": "fete"}"#).unwrap();
        assert!(hub_holders_at_risk(&config, &records).is_empty());
        // a project agent there: no more
        std::fs::remove_file(dir.path().join("family-agent.json")).unwrap();
        let projects = r#"{"projects": {"fete": {"created": 1.0, "ends": 9999999999.0, "archived": null}}}"#;
        std::fs::write(dir.path().join("project-agents.json"), projects).unwrap();
        assert_eq!(hub_holders_at_risk(&config, &records), vec!["coach".to_string()]);
        let cards: Vec<Dict> = vec![json!({"agents": {"coach": CARD_HANDLE}}).as_object().unwrap().clone()];
        assert!(hub_holders_at_risk(&config, &cards).is_empty());
    }

    #[test]
    fn origins_as_typed() {
        assert_eq!(normalize_origin("HTTPS://Example.com:443/a?b").unwrap(), "https://example.com");
        assert_eq!(normalize_origin("example.com:8443").unwrap(), "https://example.com:8443");
        assert_eq!(normalize_origin("sncf-connect.com/login").unwrap(), "https://sncf-connect.com");
        assert!(
            matches!(normalize_origin("ftp://x"), Err(VaultError::Invalid(t)) if t == "site must be a web address")
        );
        assert!(matches!(normalize_origin(" "), Err(VaultError::Invalid(t)) if t == "site is required"));
        assert!(
            matches!(normalize_origin("x.com:abc"), Err(VaultError::Invalid(t)) if t == "Port could not be cast to integer value as 'abc'")
        );
    }

    #[test]
    fn the_login_page_typed_is_kept_on_its_own_origin_only() {
        let page = "https://extranet2.example.fr/V5/connexion-agence-a.html";
        let parse = |body: Value, before: Option<&Dict>| Vault::parse(body.as_object().unwrap(), before).unwrap();
        let login = |site: &str| json!({"label": "Agence A", "site": site, "identifier": "a@b.c", "password": "p"});
        let record = parse(login(page), None);
        assert_eq!(record["origin"], json!("https://extranet2.example.fr"));
        assert_eq!(record["login_url"], json!(page));
        // its fragment, a user name or password in it are never kept; the query is
        let record = parse(login("https://me:pw@extranet2.example.fr/V5/connexion.html?groupe=a#top"), None);
        assert_eq!(record["login_url"], json!("https://extranet2.example.fr/V5/connexion.html?groupe=a"));
        // typed without its scheme, as the app's field allows
        assert_eq!(parse(login("extranet2.example.fr/V5/c.html"), None)["login_url"], json!("https://extranet2.example.fr/V5/c.html"));
        // the site alone says nothing more
        for site in ["https://extranet2.example.fr", "https://extranet2.example.fr/", "extranet2.example.fr"] {
            assert_eq!(parse(login(site), None)["login_url"], Value::Null, "{site}");
        }
        // one given apart: kept on the login's origin, dropped on any other (a subdomain, http, a look-alike)
        let record = parse(login("https://extranet2.example.fr"), None);
        let with = |url: &str| parse(json!({"login_url": url}), Some(&record))["login_url"].clone();
        assert_eq!(with(page), json!(page));
        for other in [
            "https://www.extranet2.example.fr/V5/c.html",
            "http://extranet2.example.fr/V5/c.html",
            "https://extranet2.example.fr:8443/V5/c.html",
            "https://extranet2.example.fr.evil.test/V5/c.html",
            "javascript:alert(1)",
        ] {
            assert_eq!(with(other), Value::Null, "{other}");
        }
        // kept when the rest changes (the app sends the site back as its origin), replaced with the site
        let changed = parse(json!({"label": "Agence A (nouveau)"}), Some(&parse(login(page), None)));
        assert_eq!(changed["login_url"], json!(page));
        let edited = parse(json!({"site": "https://extranet2.example.fr", "password": "q"}), Some(&changed));
        assert_eq!(edited["login_url"], json!(page));
        let other_site = parse(json!({"site": "https://autre.example.fr"}), Some(&changed));
        assert_eq!(other_site["login_url"], Value::Null);
        let moved = parse(json!({"site": "https://autre.example.fr/login"}), Some(&changed));
        assert_eq!(moved["login_url"], json!("https://autre.example.fr/login"));
        assert_eq!(login_url_of(&format!("{page}?{}", "x".repeat(LOGIN_URL_MAX)), "https://extranet2.example.fr"), None);
        // the app's list shows it, for logins only
        let item = Vault::item(&parse(login(page), None));
        assert_eq!(item.public()["login_url"], json!(page));
        let old = Vault::item(json!({"kind": "login", "label": "x", "origin": "https://a.example"}).as_object().unwrap());
        assert_eq!(old.public()["login_url"], Value::Null);
        assert!(old.public().as_object().unwrap().contains_key("login_url"));
        let card = Vault::item(json!({"kind": "address", "label": "x"}).as_object().unwrap());
        assert!(!card.public().as_object().unwrap().contains_key("login_url"));
    }

    #[test]
    fn the_record_keeps_python_s_order() {
        let body = json!({"label": "SNCF", "site": "sncf-connect.com/login", "identifier": "a@b.c", "password": "p", "otp_secret": " x "});
        let record = Vault::parse(body.as_object().unwrap(), None).unwrap();
        let keys: Vec<&String> = record.keys().collect();
        assert_eq!(keys, ["kind", "label", "origin", "login_url", "identifier_type", "identifier", "has_otp", "secret"]);
        assert_eq!(record["secret"], json!({"password": "p", "otp_secret": "x"}));
        assert_eq!(record["login_url"], json!("https://sncf-connect.com/login"));
        let again = Vault::parse(json!({"otp_secret": ""}).as_object().unwrap(), Some(&record)).unwrap();
        assert_eq!(again["secret"], json!({"password": "p"}));
        assert_eq!(again["has_otp"], json!(false));
    }

    // review offer-cap-per-payment (18/11): what `reserve_card_payment` still guards once the cap is per payment.
    fn card_record(id: &str, card: Value) -> Dict {
        json!({"id": id, "kind": "card", "label": "Petits achats", "card": card}).as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn review_cap_per_payment_two_payments_at_the_cap_at_once_but_a_single_use_card_never_twice() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(&dir.path().to_string_lossy());
        // per payment: two at the cap at once, a big `spent` ignored
        vault.write(&[card_record("item_rv_multi", json!({"cap_cents": 2000, "spent": {"EUR": 90000}}))]).unwrap();
        let a = vault.reserve_card_payment("item_rv_multi", "pay_rv_m1", 2000, "EUR").await.unwrap().unwrap();
        let b = vault.reserve_card_payment("item_rv_multi", "pay_rv_m2", 2000, "EUR").await.unwrap().unwrap();
        let over = vault.reserve_card_payment("item_rv_multi", "pay_rv_m3", 2001, "EUR").await.unwrap().unwrap_err();
        assert!(over.contains("at most 20.00 EUR per payment"), "{over}");
        drop((a, b));
        // single use: the second payment approved meanwhile is refused while the first one is in flight
        vault.write(&[card_record("item_rv_single", json!({"cap_cents": 2000, "single_use": true}))]).unwrap();
        let first = vault.reserve_card_payment("item_rv_single", "pay_rv_s1", 1000, "EUR").await.unwrap().unwrap();
        let second = vault.reserve_card_payment("item_rv_single", "pay_rv_s2", 1000, "EUR").await.unwrap().unwrap_err();
        assert!(second.contains("used by another payment meanwhile"), "{second}");
        // typed: used from now on, the reservation gone with it
        vault.record_card_payment("item_rv_single", json!({"cents": 1000, "currency": "EUR", "payment": "pay_rv_s1"}),
                                  Some(first)).await.unwrap();
        assert_eq!(crate::cards::in_flight_on("item_rv_single"), 0);
        let third = vault.reserve_card_payment("item_rv_single", "pay_rv_s3", 1000, "EUR").await.unwrap().unwrap_err();
        assert!(third.contains("used by another payment meanwhile"), "{third}");
        // an error path (give_up, timeout, stop_all): the reservation dropped, nothing stays in flight
        vault.write(&[card_record("item_rv_single2", json!({"cap_cents": 2000, "single_use": true}))]).unwrap();
        drop(vault.reserve_card_payment("item_rv_single2", "pay_rv_t1", 1000, "EUR").await.unwrap().unwrap());
        assert_eq!(crate::cards::in_flight_on("item_rv_single2"), 0);
        let again = vault.reserve_card_payment("item_rv_single2", "pay_rv_t2", 1000, "EUR").await.unwrap();
        assert!(again.is_ok(), "never typed: the single-use card is still free");
    }

    #[tokio::test]
    async fn review_cap_per_payment_card_removed_or_cap_lowered_after_approval_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(&dir.path().to_string_lossy());
        vault.write(&[card_record("item_rv_low", json!({"cap_cents": 1000}))]).unwrap();
        let low = vault.reserve_card_payment("item_rv_low", "pay_rv_l1", 1500, "EUR").await.unwrap().unwrap_err();
        assert!(low.starts_with("Nothing was typed.") && low.contains("at most 10.00 EUR per payment"), "{low}");
        assert_eq!(crate::cards::in_flight_on("item_rv_low"), 0);
        vault.write(&[]).unwrap();
        let gone = vault.reserve_card_payment("item_rv_low", "pay_rv_l2", 500, "EUR").await.unwrap().unwrap_err();
        assert!(gone.contains("removed from the vault meanwhile"), "{gone}");
    }
}
