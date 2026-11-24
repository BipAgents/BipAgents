//! « Mémoire partagée », per agent, at the bottom of its page in the apps, and the defaults for an agent on an outside
//! model (Alex 2026-11-23: « on construit et on met un pop-up explicatif quand c'est un modèle externe : la mémoire
//! partagée et le Carnet sont désactivés par défaut pour un modèle connecté »; passation-pro
//! briefs/status-shared-memory.md).
//!
//! - **who has it**: an agent of the person's own that this bridge runs (agents_admin::managed, no `host_home`, not a
//!   profile of a Hermes the person brought: profile_files::hands_off), that no one else talks to (vault::
//!   others_facing). The others have no such setting (the key is absent from their settings) and are never in the
//!   circle anyway (shared_memory.rs);
//! - **on** (the default): the agent is in shared_memory.rs' circle while its Carnet is on;
//! - **off**: out of the circle, so what it had received from the common list leaves its `USER.md` and the day's copy
//!   of its prompt (`memories/.prompt-day.json`) goes (shared_memory.rs, `clean_left`); Hermes stops showing it
//!   `USER.md` at all (`memory.user_profile_enabled: false`, [`with_user_profile`], written by profile_files::
//!   reconcile); no « Nouvelles du Carnet » (notebook/news.rs), no handover into its memory (guide::hand_over), no
//!   imported facts in its `USER.md` (assistant_import.rs); its conversations take a new prompt at their next turn
//!   ([`after_change`]: agents_admin::refresh_prompts);
//! - **an outside model** (outside_model.rs): its shared memory and its Carnet are turned off by default, once per
//!   provider ([`guard`]): when it is created (agent_tools::register runs the guard before giving its tools), when
//!   its model changes (the model routes; the shared memory's look every 15 s for a change made elsewhere, by SSH or
//!   Hermes' own command line) and at the bridge's start (the agents made before this version). What the bridge
//!   turned off is noted (`outside.<agent>.off`); the person may turn it back on, knowingly (the apps ask), and any
//!   change they make of it clears the note. Back on a model that is not outside: what the bridge had turned off and
//!   the person never touched is turned on again. Until the guard ran (a model changed by SSH a few seconds ago), the
//!   agent is already kept out of the circle ([`enabled`]). Its in-conversation asks for the Carnet and Health are not
//!   offered (tool_asks.rs `askable`);
//! - **Outlook** (choice B, Alex 24/11): an agent on an outside model, or one the bridge cannot tell, doesn't get the
//!   person's Outlook (outlook.rs `kept_on_the_place`, read from where its model runs at every call: a model changed
//!   by SSH counts at once), until the person turns it back on for it, knowingly (« Rallumer Outlook ? » in the apps:
//!   [`outlook_turned_on`], kept for that provider only). It stays chosen meanwhile, so a return to a model that is
//!   not outside gives it back as the person had it; a new outside provider asks again.
//!
//!     data_dir/agent_shared_memory.json   {"off": [agent], "outside": {agent: {"provider", "label", "at", "off": [what]}},
//!                                          "outlook_ok": {agent: provider}}  (0600)
//!     PUT /v1/agents/{agent_id}/shared-memory {on} → {id, on}   404 unknown agent, 409 others_facing, 400 by_hand
//!
//! `GET /v1/agents/{agent_id}/settings` gives `shared_memory: {on, outside: {provider, label, since, outlook?} | null}`
//! (agents_admin.rs; `outlook: true` while it is chosen in Outlook and off for this model),
//! absent for an agent without the setting: its presence tells the apps this bridge has it
//! (contracts/shared-memory/cases.json). An app 1.0 doesn't show it: the bridge applies the defaults and the cleaning
//! all the same.
//!
//! The log says « shared memory on → off · agent X », nothing else. No Python counterpart.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::put;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents_admin::pyyaml::Yaml;
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::py;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};

/// What a 409 says: an agent other people talk to has no shared memory.
pub const OTHERS_FACING: &str = "others_facing";
/// What a 400 says: an agent set up by hand keeps its own memory.
pub const BY_HAND: &str = "by_hand";
/// What the bridge turns off for an outside model (`Guarded::off`).
pub const SHARED: &str = "shared_memory";
pub const NOTEBOOK: &str = "notebook";
pub const HEALTH: &str = "health";
/// Its links with the other agents: « à l'écart » (rules.rs `aside`: it writes to no one, no one writes to it).
pub const LINKS: &str = "links";

static LOCK: Mutex<()> = Mutex::new(());

/// The defaults applied for an outside model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Guarded {
    pub provider: String,
    pub label: String,
    #[serde(default)]
    pub at: f64,
    /// what the bridge turned off then, and the person has not changed since ([`SHARED`], [`NOTEBOOK`], [`HEALTH`],
    /// [`LINKS`])
    #[serde(default)]
    pub off: Vec<String>,
    /// the Health categories it read when the bridge turned Health off, given back on the return (review 2, C9)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Stored {
    /// the agents whose shared memory is off
    #[serde(default)]
    pub off: Vec<String>,
    #[serde(default)]
    pub outside: BTreeMap<String, Guarded>,
    /// the agents whose conversations go on in a new Hermes session (review B2): since when (an ISO time, as
    /// thread_segments' `added_at`); taken by [`fresh_sessions`]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fresh: BTreeMap<String, String>,
    /// the profiles whose `memory.user_profile_enabled: false` the bridge wrote (review R7: one the person wrote
    /// stays)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_off: Vec<String>,
    /// the agents on an outside model whose Outlook the person turned back on, knowingly: for which provider (choice
    /// B, Alex 24/11; [`outlook_allowed`])
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outlook_ok: BTreeMap<String, String>,
}

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent_shared_memory.json")
}

pub fn read(config: &Config) -> Stored {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write(config: &Config, stored: &Stored) -> std::io::Result<()> {
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string(stored).unwrap_or_else(|_| "{}".into());
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

/// `stored` changed by `change` under the lock, written when it changed; what `change` returned.
fn update<T>(config: &Config, change: impl FnOnce(&mut Stored) -> T) -> std::io::Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = read(config);
    let mut stored = read(config);
    let out = change(&mut stored);
    if stored != before {
        write(config, &stored)?;
    }
    Ok(out)
}

/// Whether the bridge runs `agent`'s profile itself: managed, no `host_home`, not a profile of a Hermes the person
/// brought. The outside model's protection applies to every such agent, those other people talk to included (review
/// C5: an agent shared with a relative still has its person's Carnet in their turns).
pub fn runs_profile(config: &Config, agent: &str) -> bool {
    let Ok(found) = crate::agents_admin::managed(config, agent) else { return false };
    found.host_home.as_deref().is_none_or(str::is_empty)
        && !crate::profile_files::hands_off(&crate::agents_admin::profile_folder(config, &found))
}

/// Whether `agent` has the setting (module doc): its profile is the bridge's and no one else talks to it.
pub fn has_setting(config: &Config, agent: &str) -> bool {
    runs_profile(config, agent) && !crate::vault::others_facing(config, agent)
}

/// Where `agent`'s model runs as the protection sees it: `Some((provider, label))` outside, a model it cannot tell
/// included (« in doubt, it protects »: review C1); None on our providers or the person's own.
pub fn outside_now(config: &Config, agent: &AgentConfig) -> Option<(String, String)> {
    match crate::outside_model::place(config, agent) {
        crate::outside_model::Place::Outside { provider, label } => Some((provider, label)),
        crate::outside_model::Place::Unknown => {
            Some((crate::outside_model::UNKNOWN.into(), crate::outside_model::unknown_label(config)))
        }
        _ => None,
    }
}

/// Whether `agent` may be in the circle as far as this setting goes: it has the setting, the person did not turn it
/// off, and it is not on an outside model the guard has not seen yet (the seconds between a model changed elsewhere
/// and the next look: kept out already).
pub fn enabled(config: &Config, agent: &str) -> bool {
    match config.agent(Some(agent)) {
        Some(found) => has_setting(config, agent) && switched_on(config, found, &read(config)),
        None => false,
    }
}

/// [`enabled`] without the question of who has the setting (shared_memory::circle asks it its own way), the switches
/// read once (`stored`). A model it cannot tell keeps out an agent the guard has never seen, and leaves an agent it
/// has as it is (review C1).
pub fn switched_on(config: &Config, agent: &AgentConfig, stored: &Stored) -> bool {
    if stored.off.contains(&agent.name) {
        return false;
    }
    match crate::outside_model::place(config, agent) {
        crate::outside_model::Place::Outside { provider, .. } => {
            stored.outside.get(&agent.name).is_some_and(|g| g.provider == provider)
        }
        crate::outside_model::Place::Unknown => stored.outside.contains_key(&agent.name),
        _ => true,
    }
}

/// Whether `agent` has the setting and it is off now ([`enabled`] false): no handover, no news, no `USER.md` in its
/// prompt.
pub fn turned_off(config: &Config, agent: &str) -> bool {
    has_setting(config, agent) && !enabled(config, agent)
}

/// Whether what the person's other agents hold stays away from `agent` (an agent whose profile the bridge runs): its
/// shared memory off, or a model outside or that it cannot tell. No handover into its memory, no imported facts, no
/// Carnet or Health offered on a card (tool_asks.rs).
pub fn protected(config: &Config, agent: &AgentConfig) -> bool {
    runs_profile(config, &agent.name) && (turned_off(config, &agent.name) || outside_now(config, agent).is_some())
}

/// Whether the Carnet and Health are kept off `agent`'s cards (tool_asks.rs: its own asks, « Aussi pour… », an answer
/// to a card waiting from before; review C3): a model outside, or one the bridge cannot tell once the protection
/// applied. They come back on only from the app, with its explanation.
pub fn kept_from_cards(config: &Config, agent: &AgentConfig) -> bool {
    match crate::outside_model::place(config, agent) {
        crate::outside_model::Place::Outside { .. } => true,
        crate::outside_model::Place::Unknown => read(config).outside.contains_key(&agent.name),
        _ => false,
    }
}

/// Whether `agent` may use the person's Outlook as far as the protection goes (outlook.rs `kept_on_the_place`; choice
/// B, Alex 24/11): its model is ours or the person's own; or it is outside (a model the bridge cannot tell counts,
/// even one the guard has never seen) and the person turned Outlook back on for it on that very provider.
pub fn outlook_allowed(config: &Config, agent: &AgentConfig) -> bool {
    match outside_now(config, agent) {
        None => true,
        Some((provider, _)) => read(config).outlook_ok.get(&agent.name) == Some(&provider),
    }
}

/// « Rallumer Outlook ? » answered « Rallumer » (the person chose `agent` in Outlook while it was on an outside model:
/// outlook.rs `set_agents`, `finish`): allowed for the provider it is on now. Nothing for a model that is not outside.
pub fn outlook_turned_on(config: &Config, agent: &AgentConfig) {
    let Some((provider, _)) = outside_now(config, agent) else { return };
    let done = update(config, |s| {
        s.outlook_ok.insert(agent.name.clone(), provider.clone());
    });
    match done {
        Ok(()) => tracing::info!(target: "bipbridge::agents", agent = %agent.name, provider = %provider, "outside model: Outlook turned back on by the person"),
        Err(err) => tracing::warn!(agent = %agent.name, error = %err, "outlook note not written"),
    }
}

/// The person took `agent` out of Outlook: what they had allowed goes with it (chosen again on an outside model, it
/// is asked again).
pub fn outlook_turned_off(config: &Config, agent: &str) {
    if let Err(err) = update(config, |s| {
        s.outlook_ok.remove(agent);
    }) {
        tracing::warn!(agent = %agent, error = %err, "outlook note not written");
    }
}

/// [`turned_off`] for a caller that has already left out the agents other people talk to (notebook/news.rs
/// `concerned`), the switches read once (`stored`).
pub fn off_for(config: &Config, agent: &AgentConfig, stored: &Stored) -> bool {
    runs_profile(config, &agent.name) && !switched_on(config, agent, stored)
}

/// The time thread_segments use (`added_at`).
fn iso_now() -> String {
    crate::store::iso(crate::store::utcnow())
}

/// The person's switch; whether it changed. Clears the bridge's note that it had turned it off. Off: its conversations
/// go on in a new session (review B2, [`fresh_sessions`]).
pub fn set(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let changed = update(config, |s| {
        let was_on = !s.off.iter().any(|a| a == agent);
        s.off.retain(|a| a != agent);
        if !on {
            s.off.push(agent.to_string());
            if was_on {
                s.fresh.insert(agent.to_string(), iso_now());
            }
        }
        if let Some(g) = s.outside.get_mut(agent) {
            g.off.retain(|w| w != SHARED); // the person decided
        }
        was_on != on
    })?;
    if changed {
        tracing::info!(target: "bipbridge::agents", "shared memory {} → {} · agent {agent}",
            if on { "off" } else { "on" }, if on { "on" } else { "off" });
    }
    Ok(changed)
}

/// Review 2, C8: an agent « à l'écart » (rules.rs `aside`: the outside-model protection puts it there, the person may
/// too) is out of the discussions between agents as well, until the person turns its links back on: no turn there
/// (rooms.rs `candidates`), its hidden room conversation made anew ([`crate::rooms::drop_sessions`]), no room note
/// joined to its own conversation and no `room_history` (rooms_memory.rs), no room mate's folder.
pub fn out_of_rooms(config: &Config, agent: &str) -> bool {
    config.agent(Some(agent)).is_some() && crate::rules::load(config).is_aside(agent)
}

/// Hermes' tool that reads the agent's earlier conversations.
pub const SESSION_SEARCH: &str = "session_search";

/// Review 2, B2-b: whether `agent`'s turns close Hermes' [`SESSION_SEARCH`] ([`protected`]: its shared memory off, a
/// model outside or that the bridge cannot tell). The tool reads the agent's earlier conversations word for word (the
/// « Nouvelles du Carnet » joined to the person's words, its answers that said what the others know): a conversation
/// that « starts afresh » would otherwise read them back. Closed per turn (hermes.rs `start_run_body`, a Live call's
/// turns, patch 0024 `closed_tools`), as during a relative's turn; open again once the shared memory is back on.
/// Review 3: read from the disk as each turn starts, never from a list kept in memory (empty after a restart, until a
/// guard had run).
pub fn search_closed(config: &Config, agent: &AgentConfig) -> bool {
    protected(config, agent)
}

/// The person changed one of `agent`'s switches the protection had turned off ([`NOTEBOOK`], [`HEALTH`], [`LINKS`]):
/// the bridge's note goes, so a return to a model that is not outside leaves it as they set it. Never called for the
/// bridge's own writes (templates.rs `import`, review C4).
pub fn person_set(config: &Config, agent: &str, what: &str) {
    let done = update(config, |s| {
        if let Some(g) = s.outside.get_mut(agent) {
            g.off.retain(|w| w != what);
        }
    });
    if let Err(err) = done {
        tracing::warn!(agent = %agent, error = %err, "shared memory note not updated");
    }
}

/// The person changed `agent`'s Carnet switch themselves (agents_admin.rs `write_settings`).
pub fn person_set_notebook(config: &Config, agent: &str) {
    person_set(config, agent, NOTEBOOK);
}

/// What [`guard`] changed of the shared memory and the Carnet.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changed {
    pub shared: bool,
    pub notebook: bool,
}

impl Changed {
    pub fn any(&self) -> bool {
        self.shared || self.notebook
    }
}

/// Everything [`guard_full_at`] changed: also Health and the links (`links`: whether each agent wrote to another
/// before, rules_live::agents_changed gives the tools again from it).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Guard {
    pub changed: Changed,
    pub health: bool,
    pub links: Option<Vec<(String, bool)>>,
    /// it is chosen in Outlook, which it loses or gets back with the model (outlook.rs `kept_on_the_place`): its
    /// tools are given again
    pub outlook: bool,
}

impl Guard {
    pub fn any(&self) -> bool {
        self.changed.any() || self.health || self.links.is_some() || self.outlook
    }

    /// Its tools change (its Carnet, Health or Outlook): given again.
    pub fn tools(&self) -> bool {
        self.changed.notebook || self.health || self.outlook
    }
}

/// The defaults of an outside model for `agent`, once per provider (module doc); a model that is no longer outside
/// gets back what the bridge had turned off and the person never touched. What changed.
pub fn guard(config: &Config, agent: &AgentConfig) -> Changed {
    guard_full(config, agent).changed
}

pub fn guard_full(config: &Config, agent: &AgentConfig) -> Guard {
    guard_full_at(config, agent, py::time_time())
}

pub fn guard_at(config: &Config, agent: &AgentConfig, now: f64) -> Changed {
    guard_full_at(config, agent, now).changed
}

/// The defaults for where `agent`'s model runs now (outside_model::place), and what changed.
pub fn guard_full_at(config: &Config, agent: &AgentConfig, now: f64) -> Guard {
    guard_place(config, agent, crate::outside_model::place(config, agent), now)
}

/// [`guard_full_at`] for a provider the agent is about to use (review R2: the protection is in place before the model
/// is written). Nothing for a provider that is not outside: the return is seen once the model is written.
pub fn guard_before(config: &Config, agent: &AgentConfig, provider: &str, label: &str) -> Guard {
    let place = crate::outside_model::Place::Outside { provider: provider.into(), label: label.into() };
    guard_place(config, agent, place, py::time_time())
}

fn guard_place(config: &Config, agent: &AgentConfig, place: crate::outside_model::Place, now: f64) -> Guard {
    use crate::outside_model::Place;
    let name = agent.name.as_str();
    let stored = read(config);
    let record = stored.outside.get(name).cloned();
    // nothing moved since the last look (the usual case, every 15 s): nothing more to ask
    let target = match (&place, &record) {
        (Place::Outside { provider, .. }, Some(g)) if g.provider == *provider => return Guard::default(),
        (Place::Unknown, Some(_)) => return Guard::default(), // in doubt: as it is (review C1)
        (Place::Own | Place::Europe, None) => return Guard::default(),
        (Place::Outside { provider, label }, _) => Some((provider.clone(), label.clone())),
        (Place::Unknown, None) => Some((crate::outside_model::UNKNOWN.to_string(), crate::outside_model::unknown_label(config))),
        (Place::Own | Place::Europe, Some(_)) => None,
    };
    if !runs_profile(config, name) {
        return Guard::default();
    }
    let mut out = Guard::default();
    let result: anyhow::Result<()> = (|| {
        match (target, record) {
            (Some((provider, label)), before) => {
                // what was turned off for an earlier provider stays noted (a return gives it back)
                let before_categories = before.as_ref().map(|g| g.health.clone()).unwrap_or_default();
                let mut off: Vec<String> = before.map(|g| g.off).unwrap_or_default();
                let mut note = |what: &str| {
                    if !off.iter().any(|w| w == what) {
                        off.push(what.into());
                    }
                };
                let shared_on = has_setting(config, name) && !stored.off.iter().any(|a| a == name);
                if shared_on {
                    note(SHARED);
                }
                if crate::notebook::enabled(config, name) && crate::notebook::set_enabled(config, name, false)? {
                    out.changed.notebook = true;
                    note(NOTEBOOK);
                }
                // the days kept stay, its categories are noted to give back as they were (review 2, C9)
                let mut categories: Vec<String> = before_categories;
                if let Some(read) = crate::health::set_off_keeping_days(config, name)? {
                    categories = read.iter().map(|c| c.to_string()).collect();
                    out.health = true;
                    note(HEALTH);
                }
                if !crate::rules::load(config).is_aside(name) {
                    let before_links = crate::rules_live::offered_now(config);
                    crate::rules::change(config, |r| {
                        r.aside.retain(|a| a != name);
                        r.aside.push(name.to_string());
                        for h in r.households.values_mut() {
                            h.changed = now; // what the linked households are told changes (rules.rs `Wire`)
                        }
                    })?;
                    out.links = Some(before_links);
                    note(LINKS);
                    crate::rooms::drop_sessions(config, name); // its room conversations made anew (review 2, C8)
                }
                update(config, |s| {
                    if shared_on {
                        s.off.retain(|a| a != name);
                        s.off.push(name.to_string());
                    }
                    // its conversations go on in a new session, without what was said there (review B2)
                    s.fresh.insert(name.to_string(), iso_now());
                    // Outlook allowed on another provider: asked again on this one (choice B)
                    s.outlook_ok.retain(|a, p| a != name || *p == provider);
                    s.outside.insert(name.to_string(), Guarded { provider: provider.clone(), label, at: now, off, health: categories });
                })?;
                out.changed.shared = shared_on;
                out.outlook = crate::outlook::chosen(config, name);
                tracing::info!(target: "bipbridge::agents", agent = %name, provider = %provider,
                    "outside model: shared memory, Carnet, Health, Outlook and links off by default");
            }
            (None, Some(g)) => {
                // back on our providers or the person's own model (read as such: never « in doubt »)
                let had = |what: &str| g.off.iter().any(|w| w == what);
                if had(NOTEBOOK) && crate::notebook::set_enabled(config, name, true)? {
                    out.changed.notebook = true;
                }
                if had(HEALTH) && crate::health::set_enabled(config, name, true)? {
                    out.health = true;
                    if !g.health.is_empty() {
                        crate::health::set_categories(config, name, &g.health)?; // as the person had chosen (review 2, C9)
                    }
                }
                if had(LINKS) && crate::rules::load(config).is_aside(name) {
                    let before_links = crate::rules_live::offered_now(config);
                    crate::rules::change(config, |r| {
                        r.aside.retain(|a| a != name);
                        for h in r.households.values_mut() {
                            h.changed = now;
                        }
                    })?;
                    out.links = Some(before_links);
                }
                update(config, |s| {
                    if had(SHARED) && s.off.iter().any(|a| a == name) {
                        s.off.retain(|a| a != name);
                        out.changed.shared = true;
                    }
                    s.outside.remove(name);
                    s.outlook_ok.remove(name); // a next outside model asks again (choice B)
                })?;
                out.outlook = crate::outlook::chosen(config, name); // back, as the person had it
                tracing::info!(target: "bipbridge::agents", agent = %name, "no longer on an outside model");
            }
            (None, None) => {}
        }
        Ok(())
    })();
    if let Err(err) = result {
        tracing::warn!(agent = %name, error = %format!("{err:#}"), "outside model defaults not applied");
    }
    out
}

/// `shared_memory` in the agent's settings (agents_admin.rs `read_settings`); None without the setting. `circle`:
/// whether it receives the common memory now (its Carnet on too: review C10); `outside.since`: when the protection
/// applied (the apps say it again after a new one: review R6).
pub fn view(config: &Config, agent: &AgentConfig) -> Option<Value> {
    if !has_setting(config, &agent.name) {
        return None;
    }
    let stored = read(config);
    let outside = outside_now(config, agent).map(|(provider, label)| {
        let since = stored.outside.get(&agent.name).filter(|g| g.provider == provider).map(|g| g.at);
        let mut shown = json!({"provider": provider, "label": label, "since": since});
        if crate::outlook::held(config, &agent.name) {
            shown["outlook"] = json!(true); // chosen in Outlook, off for it (« Rallumer Outlook ? », choice B)
        }
        shown
    });
    let on = switched_on(config, agent, &stored);
    let circle = on && crate::shared_memory::membership(config, &agent.name).is_some();
    Some(json!({"on": on, "circle": circle, "outside": outside}))
}

/// Hermes' `memory.user_profile_enabled` in a profile's config: false while the setting is off (Hermes then shows the
/// agent no `USER.md`, and its memory tool keeps to its own notes), the key taken out otherwise (Hermes' default, on).
/// None when nothing changes.
pub fn with_user_profile(data: &Yaml, off: bool) -> Option<Yaml> {
    let now = data.get("memory").and_then(|m| m.get("user_profile_enabled")).cloned();
    match (off, now) {
        (true, Some(Yaml::Bool(false))) | (false, None) => None,
        (true, _) => {
            let mut out = data.clone();
            crate::profile_files::set_key(&mut out, "memory.user_profile_enabled", Yaml::Bool(false));
            Some(out)
        }
        (false, Some(_)) => {
            let mut out = data.clone();
            if let Some(memory) = out.section("memory") {
                memory.remove("user_profile_enabled");
            }
            Some(out)
        }
    }
}

/// What profile_files::reconcile writes of `memory.user_profile_enabled` for `agent`: Some(true) to turn it off, Some
/// (false) to take the bridge's own `false` out, None to leave the file as it is (no setting, or a `false` the person
/// wrote themselves: review R7). The bridge's own writes are noted.
pub fn user_profile_change(config: &Config, agent: &str) -> Option<bool> {
    if !has_setting(config, agent) {
        return None;
    }
    let off = turned_off(config, agent);
    let ours = read(config).profile_off.iter().any(|a| a == agent);
    let _ = update(config, |s| {
        s.profile_off.retain(|a| a != agent);
        if off {
            s.profile_off.push(agent.to_string());
        }
    });
    match (off, ours) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
}

/// A deleted agent: its switch and its note go.
pub fn forget(config: &Config, agent: &str) {
    let done = update(config, |s| {
        s.off.retain(|a| a != agent);
        s.outside.remove(agent);
        s.fresh.remove(agent);
        s.profile_off.retain(|a| a != agent);
        s.outlook_ok.remove(agent);
    });
    if let Err(err) = done {
        tracing::warn!(agent = %agent, error = %err, "shared memory switch not removed");
    }
}

/// The setting changed (the person, or the guard): it leaves or joins the circle at once (what it had received taken
/// out: shared_memory::sync), its personality and profile follow (profile_files::reconcile: the shared memory block,
/// `memory.user_profile_enabled`), and its conversations take a new prompt from their next turn. `tools`: its Carnet
/// or Health changed too, its tools are given again; `links`: whether each agent wrote to another before its links
/// changed (rules_live::agents_changed).
pub async fn after_change(config: std::sync::Arc<Config>, agent: &str, tools: bool) {
    follow(config, agent, tools, None).await;
}

async fn follow(config: std::sync::Arc<Config>, agent: &str, tools: bool, links: Option<Vec<(String, bool)>>) {
    let Some(agent) = config.agent(Some(agent)).cloned() else { return };
    let c = config.clone();
    let _ = tokio::task::spawn_blocking(move || crate::shared_memory::sync(&c, py::time_time(), None)).await;
    let run = crate::agents_admin::default_run();
    if let Some(before) = links {
        crate::rules_live::agents_changed(&config, &run, &before).await;
    }
    if tools {
        if let Err(exc) = crate::agent_tools::register(&config, &agent, &run).await {
            tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %exc, "agent tools not registered");
        }
    } else if let Err(exc) = crate::profile_files::reconcile(&config, &agent) {
        tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %format!("{exc:#}"), "profile settings not brought up to date");
    }
    crate::shared_memory::take_refresh(&agent.name);
    crate::agents_admin::refresh_prompts(&config, &agent, &run).await;
}

/// Links changed by a guard that ran where their follow-up can't (agent_tools::register): (whether each agent wrote to
/// another before), taken by the shared memory's look ([`links_follow`]).
static LINKS_LATER: Mutex<Option<Vec<(String, bool)>>> = Mutex::new(None);

pub fn links_changed_later(before: Vec<(String, bool)>) {
    let mut later = LINKS_LATER.lock().unwrap_or_else(|e| e.into_inner());
    if later.is_none() {
        *later = Some(before); // the earliest « before » is the one to compare with
    }
}

/// The others' tools and contacts after links changed by a guard ([`links_changed_later`]).
pub async fn links_follow(config: &Config) {
    let before = LINKS_LATER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(before) = before {
        crate::rules_live::agents_changed(config, &crate::agents_admin::default_run(), &before).await;
    }
}

/// [`guard`], then what follows when it changed something ([`after_change`]); whether it did. After a model change
/// (the routes) and at each look of the shared memory (run_forever).
pub async fn guard_and_follow(config: std::sync::Arc<Config>, agent: &AgentConfig) -> bool {
    let guard = guard_full(&config, agent);
    let any = guard.any();
    if any {
        follow(config, &agent.name, guard.tools(), guard.links).await;
    }
    any
}

/// [`guard_before`] for a provider an agent's model is about to go to, then what follows (review R2).
pub async fn guard_before_and_follow(config: std::sync::Arc<Config>, agent: &AgentConfig, provider: &str, label: &str) {
    let guard = guard_before(&config, agent, provider, label);
    if guard.any() {
        follow(config, &agent.name, guard.tools(), guard.links).await;
    }
}

/// Review B2: the conversations of an agent whose shared memory went off (the person, or an outside model) go on in a
/// new Hermes session each: Hermes replays a conversation's history at every turn (its turn notes of the memory, patch
/// 0023, the « Nouvelles du Carnet » joined to the person's words), so what it was given before would otherwise go
/// with every new turn. The thread stays the same (a `moved` segment, threads::Threads::link): the apps keep showing
/// what was said. Made where a turn starts (the send, `fresh_tip`; the queue, msg_queue.rs) and by the round, the net
/// (a conversation whose turn is going on is taken at the next look). Each agent's mark goes once all its
/// conversations begun before it have a new session.
pub async fn fresh_sessions(state: &AppState) {
    let config = state.config();
    let pending = read(&config).fresh;
    if pending.is_empty() {
        return;
    }
    let threads = crate::threads::threads_of(state);
    for (name, since) in pending {
        let Some(agent) = config.agent(Some(&name)).cloned() else {
            let _ = update(&config, |s| s.fresh.remove(&name));
            continue;
        };
        let _one = FRESH.lock().await;
        let begun = match threads.begun_before(&name, &since).await {
            Ok(begun) => begun,
            Err(exc) => {
                tracing::warn!(agent = %name, error = %format!("{exc:#}"), "conversations not read for a new session");
                continue;
            }
        };
        let mut done = true;
        for thread in begun.iter().filter(|t| !t.hidden) {
            let Ok(segments) = threads.segments(&thread.id).await else { done = false; continue };
            let Some(tip) = segments.last() else { continue };
            if state.hub.session_run(&name, &tip.session_id).is_some() {
                done = false; // a turn going on: at the next look
                continue;
            }
            if make_fresh(&state.hermes, &threads, &agent, thread, &since).await.is_none() {
                done = false;
            }
        }
        if done {
            let _ = update(&config, |s| {
                if s.fresh.get(&name) == Some(&since) {
                    s.fresh.remove(&name);
                }
            });
            tracing::info!(target: "bipbridge::agents", agent = %name, conversations = begun.len(),
                "its conversations go on in a new session, without what was said before");
        }
    }
}

/// One new session at a time (the send, the queue, the round): a conversation is moved once.
static FRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Review 2, B2-a: the session a turn of `thread` goes on in. When the agent was marked (`fresh`) after the
/// conversation's last session was linked, a new one is made and linked first, and returned; None: the conversation's
/// session as it is (no mark, a hidden conversation, or made already).
pub async fn fresh_tip(
    config: &Config,
    hermes: &crate::hermes::HermesClient,
    threads: &crate::threads::Threads,
    agent: &AgentConfig,
    thread: &crate::threads::Thread,
) -> Option<String> {
    let since = read(config).fresh.get(&agent.name).cloned()?;
    if thread.hidden {
        return None;
    }
    let _one = FRESH.lock().await;
    // seen again under the lock: moved already by another send, the queue or the round
    let begun = threads.begun_before(&agent.name, &since).await.ok()?;
    let thread = begun.iter().find(|t| t.id == thread.id)?;
    make_fresh(hermes, threads, agent, thread, &since).await
}

/// Review 3, B2-a: whether `session` is the session of a conversation its agent's shared memory went off after (the
/// agent marked `fresh`, the conversation not moved yet, as [`fresh_tip`] reads it): a turn the bridge goes on with
/// there (runs.rs: a restart for new words, the brake's rescue, a promise's continuation) would replay what it was
/// given before. A hidden conversation is never moved.
pub async fn left_behind(config: &Config, threads: &crate::threads::Threads, agent: &AgentConfig, session: &str) -> bool {
    let Some(since) = read(config).fresh.get(&agent.name).cloned() else { return false };
    let Some(thread) = threads.thread_of(agent, session).await.filter(|t| !t.hidden) else { return false };
    threads.begun_before(&agent.name, &since).await.is_ok_and(|begun| begun.iter().any(|t| t.id == thread.id))
}

/// A new Hermes session for `thread`, linked as its tip (titled with the conversation's title and when).
async fn make_fresh(
    hermes: &crate::hermes::HermesClient,
    threads: &crate::threads::Threads,
    agent: &AgentConfig,
    thread: &crate::threads::Thread,
    since: &str,
) -> Option<String> {
    let base = thread.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "Conversation".into());
    let base: String = base.chars().take(50).collect();
    for attempt in 1..=9 {
        let title = format!("{base} · {}", if attempt == 1 { since.chars().take(16).collect::<String>() } else { format!("{since} ({attempt})") });
        match hermes.create_session(agent, &title).await {
            Ok(session) => {
                if let Err(exc) = threads.link(agent, thread, &session).await {
                    tracing::warn!(agent = %agent.name, error = %format!("{exc:#}"), "a new session not linked");
                    return None;
                }
                tracing::info!(target: "bipbridge::agents", agent = %agent.name, thread = %thread.id,
                    "a conversation goes on in a new session, without what was said before");
                return Some(session);
            }
            Err(crate::hermes::HermesError::Http { status: 400 | 409, .. }) => continue,
            Err(exc) => {
                tracing::warn!(agent = %agent.name, error = %exc, "a new session not made for a conversation");
                return None;
            }
        }
    }
    None
}

/// Review 2, B2-a: before a turn of the agent starts (the send path): an outside model it went to without the bridge
/// seeing it yet (changed by SSH, the bridge just started or updated) is protected now, not at the next round, so its
/// conversation goes on in a new session at once.
pub async fn guard_on_send(config: std::sync::Arc<Config>, agent: &AgentConfig) {
    if !runs_profile(&config, &agent.name) {
        return;
    }
    let _ = guard_and_follow(config, agent).await;
}

// --- the app's route ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSharedMemory {
    pub on: bool,
}

impl Validate for AgentSharedMemory {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

async fn put_shared_memory(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentSharedMemory>,
) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agents.get(&agent_id).cloned() else { return Err(HttpError::not_found()) };
    if crate::vault::others_facing(&config, &agent_id) {
        return Err(HttpError::new(409, OTHERS_FACING));
    }
    if !has_setting(&config, &agent_id) {
        return Err(HttpError::new(400, BY_HAND));
    }
    // the outside model's defaults first: a model changed a moment ago never makes the person's « on » undone later
    let guarded = guard_full(&config, &agent);
    let (c, id, on) = (config.clone(), agent_id.clone(), body.on);
    let changed = tokio::task::spawn_blocking(move || set(&c, &id, on))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
    if changed || guarded.any() {
        let tools = guarded.changed.notebook || guarded.health;
        follow(config.clone(), &agent_id, tools, guarded.links).await;
        fresh_sessions(&state).await; // off: its conversations go on in a new session at once (review B2)
    }
    Ok(web::ok(json!({"id": agent_id, "on": enabled(&config, &agent_id)})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/agents/{agent_id}/shared-memory", put(put_shared_memory))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn place() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
        config.hermes.bin = "hermes".into();
        config.hermes.hub_url = "http://127.0.0.1:9".into();
        config.hermes.home = dir.path().join("hub").to_string_lossy().into_owned();
        for name in ["budget", "gpt"] {
            let space = dir.path().join(format!("spaces/{name}"));
            std::fs::create_dir_all(space.join(format!("profiles/{name}/memories"))).unwrap();
            // the person's own model (no install's model: a profile without one is a model the bridge cannot tell)
            std::fs::write(space.join(format!("profiles/{name}/config.yaml")), SPARK).unwrap();
            config.agents.insert(
                name.into(),
                AgentConfig { name: name.into(), space: Some(space.to_string_lossy().into_owned()), ..Default::default() },
            );
        }
        (dir, config)
    }

    fn set_model(config: &Config, agent: &str, model: &str) {
        let a = config.agent(Some(agent)).unwrap();
        let folder = crate::agents_admin::profile_folder(config, a);
        std::fs::write(folder.join("config.yaml"), model).unwrap();
    }

    const CODEX: &str = "model:\n  provider: openai-codex\n  default: gpt-6.1\n";
    const ANTHROPIC: &str = "model:\n  provider: anthropic\n  default: claude-sonnet-5-5\n";
    const EUROUTER: &str = "model:\n  provider: custom\n  base_url: https://api.eurouter.ai/api/v1\n  default: glm-5.3\n";
    const SPARK: &str = "model:\n  provider: custom\n  base_url: http://192.168.1.123:8888/v1\n  default: qwen\n";

    #[test]
    fn an_outside_model_turns_shared_memory_and_the_carnet_off_once() {
        let (_dir, config) = place();
        set_model(&config, "budget", EUROUTER);
        set_model(&config, "gpt", CODEX);
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        let budget = config.agent(Some("budget")).unwrap().clone();
        // before the guard ran: already out
        assert!(!enabled(&config, "gpt") && enabled(&config, "budget"));
        assert!(crate::notebook::enabled(&config, "gpt"));
        assert_eq!(guard_at(&config, &gpt, 1.0), Changed { shared: true, notebook: true });
        assert!(!enabled(&config, "gpt") && !crate::notebook::enabled(&config, "gpt"));
        assert_eq!(guard_at(&config, &budget, 1.0), Changed::default(), "ours: untouched");
        assert!(enabled(&config, "budget") && crate::notebook::enabled(&config, "budget"));
        assert_eq!(
            view(&config, &gpt),
            Some(json!({"on": false, "circle": false, "outside": {"provider": "openai-codex", "label": "OpenAI", "since": 1.0}}))
        );
        assert_eq!(view(&config, &budget), Some(json!({"on": true, "circle": true, "outside": null})));
        // once: the person turns them back on, knowingly; the next start leaves them on
        assert!(set(&config, "gpt", true).unwrap());
        crate::notebook::set_enabled(&config, "gpt", true).unwrap();
        person_set_notebook(&config, "gpt");
        assert_eq!(guard_at(&config, &gpt, 2.0), Changed::default());
        assert!(enabled(&config, "gpt") && crate::notebook::enabled(&config, "gpt"));
        // another outside provider: the defaults again
        set_model(&config, "gpt", ANTHROPIC);
        assert!(!enabled(&config, "gpt"), "out until the guard sees the new provider");
        assert_eq!(guard_at(&config, &gpt, 3.0), Changed { shared: true, notebook: true });
        assert_eq!(read(&config).outside["gpt"].label, "Anthropic");
        // back on the person's own model: what the bridge turned off comes back on
        set_model(&config, "gpt", SPARK);
        assert_eq!(guard_at(&config, &gpt, 4.0), Changed { shared: true, notebook: true });
        assert!(enabled(&config, "gpt") && crate::notebook::enabled(&config, "gpt"));
        assert!(read(&config).outside.is_empty());
        assert_eq!(guard_at(&config, &gpt, 5.0), Changed::default());
    }

    #[test]
    fn the_person_s_own_off_stays_off_after_a_return_from_outside() {
        let (_dir, config) = place();
        set_model(&config, "gpt", CODEX);
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        guard_at(&config, &gpt, 1.0);
        // the person sets the Carnet themselves (off again, after trying it on): theirs from now on
        crate::notebook::set_enabled(&config, "gpt", true).unwrap();
        crate::notebook::set_enabled(&config, "gpt", false).unwrap();
        person_set_notebook(&config, "gpt");
        set(&config, "gpt", false).unwrap();
        set_model(&config, "gpt", EUROUTER);
        assert_eq!(guard_at(&config, &gpt, 2.0), Changed::default());
        assert!(!enabled(&config, "gpt") && !crate::notebook::enabled(&config, "gpt"), "the person's choice kept");
        // and an agent turned off by the person, never outside: no note, nothing comes back
        set(&config, "budget", false).unwrap();
        let budget = config.agent(Some("budget")).unwrap().clone();
        assert_eq!(guard_at(&config, &budget, 3.0), Changed::default());
        assert!(turned_off(&config, "budget"));
    }

    #[test]
    fn an_agent_others_talk_to_has_no_setting() {
        let (dir, mut config) = place();
        let data = PathBuf::from(config.data_dir());
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("family-agent.json"), r#"{"agent": "budget", "created": 1.0, "lang": "fr"}"#).unwrap();
        set_model(&config, "budget", CODEX);
        let budget = config.agent(Some("budget")).unwrap().clone();
        assert_eq!(view(&config, &budget), None);
        assert!(!turned_off(&config, "budget"), "no setting: never « off »");
        // but the outside model's protection all the same (review C5): its Carnet off
        assert_eq!(guard_at(&config, &budget, 1.0), Changed { shared: false, notebook: true });
        assert!(!crate::notebook::enabled(&config, "budget") && protected(&config, &budget));
        config.agents.get_mut("gpt").unwrap().host_home = Some(dir.path().to_string_lossy().into_owned());
        assert_eq!(view(&config, config.agent(Some("gpt")).unwrap()), None, "set up by hand");
    }

    /// Off: no handover of what another agent knew into its memory; « Ce que tes agents savent de toi » shows it,
    /// whatever its Carnet, with what its prompt really carries (2026-11-23).
    #[test]
    fn off_means_no_handover_and_the_page_says_what_it_really_knows() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        let budget = config.agent(Some("budget")).unwrap().clone();
        let memory = |a: &AgentConfig| {
            std::fs::read_to_string(crate::agents_admin::profile_folder(&config, a).join("memories/MEMORY.md")).unwrap_or_default()
        };
        let user = |a: &AgentConfig| crate::agents_admin::profile_folder(&config, a).join("memories/USER.md");
        std::fs::write(user(&gpt), "Alex aime le jazz.").unwrap();
        std::fs::write(user(&budget), "Alex habite Lyon.").unwrap();
        set(&config, "gpt", false).unwrap();
        crate::notebook::set_enabled(&config, "gpt", false).unwrap();
        crate::guide::hand_over(&config, &gpt, "Bip", "Alex a deux enfants, Tiago et Maëlle.", "fr").unwrap();
        assert!(!memory(&gpt).contains("Tiago"), "{}", memory(&gpt));
        crate::guide::hand_over(&config, &budget, "Bip", "Alex a deux enfants, Tiago et Maëlle.", "fr").unwrap();
        assert!(memory(&budget).contains("Tiago"));
        let now = py::time_time();
        let page = crate::notebook::known::view(&config, "gpt", "fr", now);
        let ids: Vec<&str> = page["agents"].as_array().unwrap().iter().filter_map(|a| a["id"].as_str()).collect();
        assert!(ids.contains(&"gpt") && ids.contains(&"budget"), "every agent, its Carnet on or not: {ids:?}");
        assert_eq!(page["always"]["lines"], json!([]), "Hermes shows it no USER.md");
        assert_eq!(page["always"]["note"], crate::notebook::known::texts("fr").shared_off);
        let page = crate::notebook::known::view(&config, "budget", "fr", now);
        assert_eq!(page["always"]["lines"][0]["text"], "Alex habite Lyon.");
        assert!(page["always"].get("note").is_none(), "the page as before");
        // on again (its Carnet still off): what it wrote itself
        set(&config, "gpt", true).unwrap();
        let page = crate::notebook::known::view(&config, "gpt", "fr", now);
        assert_eq!(page["always"]["lines"][0]["text"], "Alex aime le jazz.");
    }

    /// Review 23/11 (red): what Bip handed over before (« ↪ Passé par Bip … » in MEMORY.md, in every prompt: Hermes'
    /// memory block stays on with `user_profile_enabled: false`) is something it received from another agent. Gone to
    /// an outside model, it gives it back like the rest; today it stays and goes to OpenAI with every turn.
    #[test]
    fn review_an_outside_model_gives_back_the_handovers_it_had() {
        let (_dir, config) = place();
        set_model(&config, "budget", EUROUTER);
        set_model(&config, "gpt", EUROUTER);
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        let memory = || {
            std::fs::read_to_string(crate::agents_admin::profile_folder(&config, &gpt).join("memories/MEMORY.md"))
                .unwrap_or_default()
        };
        crate::shared_memory::sync(&config, 1.0, None);
        crate::guide::hand_over(&config, &gpt, "Bip", "Alex a deux enfants, Tiago et Maëlle.", "fr").unwrap();
        assert!(memory().contains("Tiago"), "the handover of before: {}", memory());
        set_model(&config, "gpt", CODEX);
        assert_eq!(guard_at(&config, &gpt, 2.0), Changed { shared: true, notebook: true });
        crate::shared_memory::sync(&config, 2.0, None);
        assert!(!memory().contains("Tiago"), "still in its MEMORY.md, sent with every prompt: {}", memory());
    }

    /// Review 23/11 (red): « in doubt » is not « back on our providers ». A model the bridge cannot tell any more (its
    /// config.yaml empty for a moment while it is rewritten, or `provider: custom` without its address — Hermes v0.23.1
    /// then goes to CUSTOM_BASE_URL, else OpenRouter's address with an OpenRouter key: runtime_provider_backends.py
    /// `_resolve_openrouter_runtime`) must not turn the shared memory and the Carnet back on.
    #[test]
    fn review_a_model_it_cannot_tell_gives_nothing_back() {
        let (_dir, config) = place();
        set_model(&config, "gpt", CODEX);
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        guard_at(&config, &gpt, 1.0);
        for unknown in ["", "model:\n  provider: custom\n  default: gpt-6.1\n"] {
            set_model(&config, "gpt", unknown);
            assert_eq!(guard_at(&config, &gpt, 2.0), Changed::default(), "turned back on for {unknown:?}");
            assert!(!enabled(&config, "gpt") && !crate::notebook::enabled(&config, "gpt"), "{unknown:?}");
        }
    }

    /// The protection puts Health and the links with the other agents away too (coordinator 23/11, C8, C9), notes it,
    /// and gives them back on a return to the person's own model; the conversations are marked to go on in a new
    /// session (B2).
    #[test]
    fn health_and_links_go_away_with_the_rest_and_come_back() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        crate::health::set_enabled(&config, "gpt", true).unwrap();
        assert!(!crate::rules::load(&config).is_aside("gpt"));
        set_model(&config, "gpt", CODEX);
        let guard = guard_full_at(&config, &gpt, 1.0);
        assert!(guard.health && guard.links.is_some() && guard.changed == Changed { shared: true, notebook: true }, "{guard:?}");
        assert!(!crate::health::enabled(&config, "gpt") && crate::rules::load(&config).is_aside("gpt"));
        let stored = read(&config);
        let mut off = stored.outside["gpt"].off.clone();
        off.sort();
        assert_eq!(off, [HEALTH, LINKS, NOTEBOOK, SHARED]);
        assert!(stored.fresh.contains_key("gpt"), "its conversations go on in a new session");
        set_model(&config, "gpt", SPARK);
        let back = guard_full_at(&config, &gpt, 2.0);
        assert!(back.health && back.links.is_some(), "{back:?}");
        assert!(crate::health::enabled(&config, "gpt") && !crate::rules::load(&config).is_aside("gpt"));
        assert!(enabled(&config, "gpt") && crate::notebook::enabled(&config, "gpt"));
    }

    /// Review 2 (red), C9: Health comes back as the person had it. The protection turns it off with
    /// health::set_enabled(false), which forgets the agent's categories, and the return turns it on with
    /// health::set_enabled(true), which gives the defaults (activity, sleep, heart): an agent that read only the
    /// person's activity reads their sleep and heart rate too after a return from outside (and one that read the cycle
    /// loses it).
    #[test]
    fn review2_health_comes_back_with_the_categories_the_person_chose() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        crate::health::set_enabled(&config, "gpt", true).unwrap();
        crate::health::set_categories(&config, "gpt", &["activity"]).unwrap();
        assert_eq!(crate::health::categories(&config, "gpt"), ["activity"]);
        set_model(&config, "gpt", CODEX);
        guard_full_at(&config, &gpt, 1.0);
        assert!(!crate::health::enabled(&config, "gpt"));
        set_model(&config, "gpt", SPARK);
        guard_full_at(&config, &gpt, 2.0);
        assert!(crate::health::enabled(&config, "gpt"));
        assert_eq!(crate::health::categories(&config, "gpt"), ["activity"], "not what the person had chosen");
    }

    /// The person's own changes of Health and of « à l'écart » are theirs: a return doesn't undo them.
    #[test]
    fn the_person_s_health_and_links_stay_as_they_set_them() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        crate::health::set_enabled(&config, "gpt", true).unwrap();
        set_model(&config, "gpt", CODEX);
        guard_full_at(&config, &gpt, 1.0);
        person_set(&config, "gpt", HEALTH);
        person_set(&config, "gpt", LINKS);
        set_model(&config, "gpt", SPARK);
        let back = guard_full_at(&config, &gpt, 2.0);
        assert!(!back.health && back.links.is_none(), "{back:?}");
        assert!(!crate::health::enabled(&config, "gpt") && crate::rules::load(&config).is_aside("gpt"));
    }

    /// In doubt, it protects (review C1): a model the bridge cannot tell, for an agent it has never protected, is
    /// treated as outside (named so for the apps); once protected, a moment of doubt changes nothing.
    #[test]
    fn in_doubt_it_protects() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        set_model(&config, "gpt", "");
        assert!(!enabled(&config, "gpt"), "out of the circle before the guard");
        assert_eq!(guard_at(&config, &gpt, 1.0), Changed { shared: true, notebook: true });
        let record = read(&config).outside["gpt"].clone();
        assert_eq!(record.provider, crate::outside_model::UNKNOWN);
        assert!(kept_from_cards(&config, &gpt) && protected(&config, &gpt));
        let shown = view(&config, &gpt).unwrap();
        assert_eq!(shown["outside"]["provider"], "unknown");
        assert!(shown["outside"]["label"].as_str().is_some_and(|l| !l.is_empty()));
        // the person turns it on knowingly: still doubt, it stays on
        set(&config, "gpt", true).unwrap();
        assert_eq!(guard_at(&config, &gpt, 2.0), Changed::default());
        assert!(enabled(&config, "gpt"));
        // its model readable again, the person's own: what the bridge turned off comes back
        set_model(&config, "gpt", SPARK);
        assert_eq!(guard_at(&config, &gpt, 3.0), Changed { shared: false, notebook: true });
        assert!(read(&config).outside.is_empty());
    }

    /// Off by the person: its conversations go on in a new session too (B2); `memory.user_profile_enabled: false` is
    /// the bridge's to take out again, never one the person wrote (R7).
    #[test]
    fn off_marks_new_sessions_and_the_profile_key_is_the_bridge_s_own() {
        let (_dir, config) = place();
        assert_eq!(user_profile_change(&config, "budget"), None, "on, nothing written by the bridge: left as it is");
        set(&config, "budget", false).unwrap();
        assert!(read(&config).fresh.contains_key("budget"));
        assert_eq!(user_profile_change(&config, "budget"), Some(true));
        set(&config, "budget", true).unwrap();
        assert_eq!(user_profile_change(&config, "budget"), Some(false), "the bridge's own false taken out");
        assert_eq!(user_profile_change(&config, "budget"), None, "once");
    }

    /// A model route puts the protection in place for where the model goes before it writes it (R2).
    #[test]
    fn where_a_model_goes_is_known_before_it_is_written() {
        let (_dir, config) = place();
        let budget = config.agent(Some("budget")).unwrap().clone();
        let to_anthropic = crate::outside_model::outside_after(&config, &budget, |m| {
            m.insert("provider".into(), json!("anthropic"));
            m.insert("base_url".into(), json!(""));
        });
        assert_eq!(to_anthropic.map(|o| o.1), Some("Anthropic".into()));
        let cloud = crate::outside_model::outside_after(&config, &budget, |m| {
            m.insert("provider".into(), json!("ollama"));
            m.insert("base_url".into(), json!(""));
            m.insert("default".into(), json!("gpt-oss:120b-cloud"));
        });
        assert!(cloud.is_some(), "an Ollama cloud model");
        assert_eq!(crate::outside_model::outside_after(&config, &budget, |m| { m.insert("default".into(), json!("qwen3")); }), None);
        let guard = guard_before(&config, &budget, "anthropic", "Anthropic");
        assert!(guard.changed.shared && !enabled(&config, "budget"), "{guard:?}");
    }

    #[test]
    fn hermes_shows_no_user_md_while_it_is_off() {
        let data = crate::agents_admin::pyyaml::load("model:\n  default: x\nmemory:\n  user_char_limit: 1375\n").unwrap();
        let off = with_user_profile(&data, true).unwrap();
        assert_eq!(off.get("memory").and_then(|m| m.get("user_profile_enabled")), Some(&Yaml::Bool(false)));
        assert_eq!(off.get("memory").and_then(|m| m.get("user_char_limit")), data.get("memory").and_then(|m| m.get("user_char_limit")));
        assert_eq!(with_user_profile(&off, true), None, "once");
        let on = with_user_profile(&off, false).unwrap();
        assert_eq!(on.get("memory").and_then(|m| m.get("user_profile_enabled")), None, "Hermes' default again");
        assert_eq!(with_user_profile(&on, false), None);
        let bare = crate::agents_admin::pyyaml::load("model:\n  default: x\n").unwrap();
        let off = with_user_profile(&bare, true).unwrap();
        assert_eq!(off.get("memory").and_then(|m| m.get("user_profile_enabled")), Some(&Yaml::Bool(false)));
    }

    /// Review 2, C9: the protection never deletes the Health days kept on the bridge (it only stops giving them), even
    /// when the agent was the only one with Health; the person's own « off » still does.
    #[test]
    fn the_health_days_kept_stay_through_the_protection() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        for name in config.agents.keys() {
            crate::health::set_enabled(&config, name, false).unwrap();
        }
        crate::health::set_enabled(&config, "gpt", true).unwrap();
        let days = crate::health::days_dir(&config);
        std::fs::create_dir_all(&days).unwrap();
        std::fs::write(days.join("2026-11-20.json"), "{}").unwrap();
        set_model(&config, "gpt", CODEX);
        guard_full_at(&config, &gpt, 1.0);
        assert!(!crate::health::enabled(&config, "gpt"));
        assert!(days.join("2026-11-20.json").exists(), "the days kept were deleted by the protection");
    }

    /// Review 2, B2-b: Hermes' `session_search` is closed in every turn of an agent the protection covers (outside, or
    /// its shared memory off), open again once it is back; the other options of a turn stay as they are.
    #[test]
    fn session_search_is_closed_while_the_protection_holds() {
        let (dir, mut config) = place();
        let space = dir.path().join("spaces/searcher");
        std::fs::create_dir_all(space.join("profiles/searcher/memories")).unwrap();
        std::fs::write(space.join("profiles/searcher/config.yaml"), SPARK).unwrap();
        config.agents.insert(
            "searcher".into(),
            AgentConfig { name: "searcher".into(), space: Some(space.to_string_lossy().into_owned()), ..Default::default() },
        );
        let gpt = config.agent(Some("searcher")).unwrap().clone();
        set_model(&config, "searcher", CODEX);
        guard_full_at(&config, &gpt, 1.0);
        assert!(search_closed(&config, &gpt));
        let options = crate::hermes::with_closed_search(&config, &gpt, Some(json!({"closed_tools": ["browser_*"], "reasoning": {"effort": "low"}})));
        assert_eq!(options, Some(json!({"closed_tools": ["browser_*", "session_search"], "reasoning": {"effort": "low"}})));
        assert_eq!(crate::hermes::with_closed_search(&config, &gpt, None), Some(json!({"closed_tools": ["session_search"]})));
        set_model(&config, "searcher", SPARK);
        guard_full_at(&config, &gpt, 2.0);
        assert!(!search_closed(&config, &gpt), "back on the Spark, shared memory given back: open again");
        set(&config, "searcher", false).unwrap();
        assert!(search_closed(&config, &gpt), "the person turned it off: closed");
        set(&config, "searcher", true).unwrap();
        assert!(!search_closed(&config, &gpt));
        assert_eq!(crate::hermes::with_closed_search(&config, &gpt, None), None);
    }

    /// Review 3, B2-b: `session_search` was closed from a list kept in memory only, filled by a guard (the send, the
    /// routes, the round: the first one 20 s after the start). After a restart (every update), an agent protected on
    /// the disk got it open in the turns the bridge starts before then: a card answered across the restart
    /// (card_place.rs resumes it at once), a queued message, a room, a routine, a report handed back. Now read from
    /// the disk as each turn starts.
    #[test]
    fn review3_session_search_is_closed_after_a_restart_too() {
        let (dir, mut config) = place();
        let space = dir.path().join("spaces/restarted");
        std::fs::create_dir_all(space.join("profiles/restarted/memories")).unwrap();
        std::fs::write(space.join("profiles/restarted/config.yaml"), SPARK).unwrap();
        config.agents.insert(
            "restarted".into(),
            AgentConfig { name: "restarted".into(), space: Some(space.to_string_lossy().into_owned()), ..Default::default() },
        );
        let gpt = config.agent(Some("restarted")).unwrap().clone();
        set_model(&config, "restarted", CODEX);
        guard_full_at(&config, &gpt, 1.0);
        assert!(protected(&config, &gpt));
        // the bridge restarts: nothing of the guard kept in memory, what is on the disk stays
        assert!(protected(&config, &gpt), "still protected after the restart");
        assert_eq!(
            crate::hermes::with_closed_search(&config, &gpt, None),
            Some(json!({"closed_tools": ["session_search"]})),
            "a turn started before the first guard reads the old conversations"
        );
        // the bridge's own client (app.rs, agent_pairs.rs) reads the place as each turn starts
        let client = crate::hermes::HermesClient::of_place(reqwest::Client::new(), crate::config::SharedConfig::new(config.clone()));
        assert_eq!(client.turn_options(&gpt, None), Some(json!({"closed_tools": ["session_search"]})));
    }

    /// Review 2, C10: an agent on the person's own model whose fallback goes outside is named « (OpenRouter, en
    /// secours) »: it is not « on OpenRouter ».
    #[test]
    fn an_outside_fallback_is_named_so() {
        let (_dir, config) = place();
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        set_model(&config, "gpt", &format!("{SPARK}fallback_providers:\n  - provider: openrouter\n    model: x\n"));
        let (provider, label) = outside_now(&config, &gpt).unwrap();
        // (in the install's language: « en secours » in French)
        assert_eq!(provider, "openrouter");
        assert!(["OpenRouter, en secours", "OpenRouter, as a fallback"].contains(&label.as_str()), "{label}");
        set_model(&config, "gpt", "model:\n  provider: openrouter\n  default: x\n");
        assert_eq!(outside_now(&config, &gpt).unwrap().1, "OpenRouter");
    }

    /// The person's Outlook with `agents` chosen, all of it (outlook.rs).
    fn outlook_for(config: &mut Config, agents: &[&str]) {
        config.outlook = crate::outlook::Settings { enabled: true };
        let link = crate::drives::Link {
            state: "connected".into(),
            agents: agents.iter().map(|a| a.to_string()).collect(),
            tokens: crate::connections::Tokens { scope: Some(crate::outlook::SCOPES.into()), ..Default::default() },
            ..Default::default()
        };
        let stored = crate::connections::Stored {
            outlook: Some(crate::outlook::Account { link, parts: Default::default() }),
            ..Default::default()
        };
        crate::connections::store_of(config).write(&stored).unwrap();
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    /// Choice B (Alex 24/11): an agent on openai-codex has no Outlook (`mail_for`), at once, before any guard; the
    /// person may turn it back on (« Rallumer Outlook ? »), for that provider only; a return to the person's own model
    /// gives it back as they had it.
    #[tokio::test]
    async fn choice_b_an_agent_on_openai_codex_has_no_outlook_until_the_person_says_so() {
        use crate::outlook;
        let (_dir, mut config) = place();
        outlook_for(&mut config, &["gpt", "budget"]);
        set_model(&config, "budget", EUROUTER);
        set_model(&config, "gpt", CODEX);
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        // before the guard ran (a model changed by SSH): already off
        assert!(!outlook::mail_for(&config, "gpt") && !outlook::calendar_for(&config, "gpt"));
        assert!(!outlook::contacts_for(&config, "gpt") && !outlook::kept_on_the_place(&config, "gpt"));
        assert!(outlook::mail_for(&config, "budget"), "ours: untouched");
        let guarded = guard_full_at(&config, &gpt, 1.0);
        assert!(guarded.outlook && guarded.tools(), "its tools given again without Outlook");
        assert!(!outlook::mail_for(&config, "gpt") && outlook::held(&config, "gpt") && !outlook::held(&config, "budget"));
        // its page says so (and the pop-up names Outlook), the Outlook screen too
        assert_eq!(view(&config, &gpt).unwrap()["outside"]["outlook"], json!(true));
        let shown = outlook::view(&config, outlook::stored(&config).as_ref());
        assert_eq!((&shown["agents"], &shown["outside"]), (&json!(["budget"]), &json!({"gpt": "OpenAI"})));
        // the app changes what they use (gpt shown off, so left out): gpt stays chosen, still off
        let mail_only = crate::outlook::Parts { mail: true, calendar: false, contacts: false };
        let (shown, _) = outlook::set_agents(&config, &names(&["budget"]), Some(mail_only)).await.unwrap();
        assert_eq!(shown["agents"], json!(["budget"]));
        assert!(outlook::chosen(&config, "gpt") && !outlook::mail_for(&config, "gpt"));
        // « Rallumer Outlook ? » → « Rallumer »: chosen in the app, on for OpenAI
        let (shown, changed) = outlook::set_agents(&config, &names(&["budget", "gpt"]), None).await.unwrap();
        assert!(outlook::mail_for(&config, "gpt") && changed.contains(&"gpt".to_string()), "{changed:?}");
        assert_eq!((&shown["agents"], &shown["outside"]), (&json!(["budget", "gpt"]), &json!({})));
        assert!(view(&config, &gpt).unwrap()["outside"].get("outlook").is_none());
        assert_eq!(guard_full_at(&config, &gpt, 2.0), Guard::default(), "the same provider: as the person set it");
        assert!(outlook::mail_for(&config, "gpt"));
        // another outside provider: off again, asked again
        set_model(&config, "gpt", ANTHROPIC);
        assert!(!outlook::mail_for(&config, "gpt"), "off before the guard sees Anthropic");
        assert!(guard_full_at(&config, &gpt, 3.0).outlook);
        assert!(!outlook::mail_for(&config, "gpt") && read(&config).outlook_ok.is_empty());
        // back on the person's own model: Outlook again, as they had it
        set_model(&config, "gpt", SPARK);
        assert!(outlook::mail_for(&config, "gpt"));
        assert!(guard_full_at(&config, &gpt, 4.0).outlook);
        assert!(outlook::mail_for(&config, "gpt") && read(&config).outlook_ok.is_empty());
        // turned back on, then taken out by the person while outside: not given back on the return
        set_model(&config, "gpt", CODEX);
        guard_full_at(&config, &gpt, 5.0);
        outlook::set_agents(&config, &names(&["budget", "gpt"]), None).await.unwrap();
        outlook::set_agents(&config, &names(&["budget"]), None).await.unwrap();
        assert!(!outlook::chosen(&config, "gpt") && read(&config).outlook_ok.is_empty());
        set_model(&config, "gpt", SPARK);
        guard_full_at(&config, &gpt, 6.0);
        assert!(!outlook::mail_for(&config, "gpt"), "the person's choice stays");
    }

    /// Choice B, « in doubt »: a model the bridge cannot tell has no Outlook, even before any guard saw it.
    #[test]
    fn choice_b_a_model_it_cannot_tell_has_no_outlook() {
        let (_dir, mut config) = place();
        outlook_for(&mut config, &["gpt"]);
        set_model(&config, "gpt", "model: {}\n");
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        assert_eq!(crate::outside_model::place(&config, &gpt), crate::outside_model::Place::Unknown);
        assert!(read(&config).outside.is_empty(), "never seen by the guard");
        assert!(!crate::outlook::mail_for(&config, "gpt") && crate::outlook::held(&config, "gpt"));
    }

    /// Review of shared-memory-1.1 (24/11), blocker: « Rallumer Outlook ? » answered for gpt on OpenAI, then the person
    /// disconnects Outlook (every agent taken out at once). Connecting it again — maybe another Microsoft account — must
    /// ask again: gpt is listed under `outside` (its line, « Rallumer Outlook ? », never chosen for the person when it is
    /// the only agent). Today `outlook_ok` survives the disconnection: gpt is shown as a normal agent, chosen d'office
    /// when alone, and gets the new account's mail, calendar and contacts on OpenAI without the question.
    #[tokio::test]
    async fn review_sm11_a_disconnection_forgets_rallumer_outlook() {
        use crate::outlook;
        let (_dir, mut config) = place();
        outlook_for(&mut config, &["budget"]);
        set_model(&config, "budget", EUROUTER);
        set_model(&config, "gpt", CODEX);
        let gpt = config.agent(Some("gpt")).unwrap().clone();
        guard_full_at(&config, &gpt, 1.0);
        // « Rallumer Outlook ? » → « Rallumer »
        outlook::set_agents(&config, &names(&["budget", "gpt"]), None).await.unwrap();
        assert!(outlook::mail_for(&config, "gpt"));
        // the person disconnects Outlook
        outlook::disconnect(&config).await.unwrap();
        // the screen before connecting again: gpt is said and asked first
        let shown = outlook::view(&config, outlook::stored(&config).as_ref());
        assert_eq!(shown["outside"], json!({"gpt": "OpenAI"}), "not asked again after a disconnection: {shown}");
        assert!(read(&config).outlook_ok.is_empty(), "« Rallumer » kept after the disconnection: {:?}", read(&config).outlook_ok);
        // connected again (here: the account written back as a sign-in would) with gpt among the agents sent without
        // the question: it must not have Outlook until « Rallumer » is answered again
        outlook_for(&mut config, &["gpt"]);
        assert!(!outlook::mail_for(&config, "gpt"), "gpt reads the reconnected mailbox on OpenAI without being asked");
    }
}
