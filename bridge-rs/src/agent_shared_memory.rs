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
//!   offered (tool_asks.rs `askable`).
//!
//!     data_dir/agent_shared_memory.json   {"off": [agent], "outside": {agent: {"provider", "label", "at", "off": [what]}}}
//!                                         (0600)
//!     PUT /v1/agents/{agent_id}/shared-memory {on} → {id, on}   404 unknown agent, 409 others_facing, 400 by_hand
//!
//! `GET /v1/agents/{agent_id}/settings` gives `shared_memory: {on, outside: {provider, label} | null}` (agents_admin.rs),
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
/// What the bridge turned off for an outside model.
pub const SHARED: &str = "shared_memory";
pub const NOTEBOOK: &str = "notebook";

static LOCK: Mutex<()> = Mutex::new(());

/// The defaults applied for an outside model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Guarded {
    pub provider: String,
    pub label: String,
    #[serde(default)]
    pub at: f64,
    /// what the bridge turned off then, and the person has not changed since ([`SHARED`], [`NOTEBOOK`])
    #[serde(default)]
    pub off: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Stored {
    /// the agents whose shared memory is off
    #[serde(default)]
    pub off: Vec<String>,
    #[serde(default)]
    pub outside: BTreeMap<String, Guarded>,
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

/// Whether `agent` has the setting (module doc).
pub fn has_setting(config: &Config, agent: &str) -> bool {
    let Ok(found) = crate::agents_admin::managed(config, agent) else { return false };
    found.host_home.as_deref().is_none_or(str::is_empty)
        && !crate::vault::others_facing(config, agent)
        && !crate::profile_files::hands_off(&crate::agents_admin::profile_folder(config, &found))
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
/// read once (`stored`).
pub fn switched_on(config: &Config, agent: &AgentConfig, stored: &Stored) -> bool {
    if stored.off.contains(&agent.name) {
        return false;
    }
    match crate::outside_model::outside(config, agent) {
        Some((provider, _)) => stored.outside.get(&agent.name).is_some_and(|g| g.provider == provider),
        None => true,
    }
}

/// Whether `agent` has the setting and it is off now ([`enabled`] false): no handover, no news, no `USER.md` in its
/// prompt.
pub fn turned_off(config: &Config, agent: &str) -> bool {
    has_setting(config, agent) && !enabled(config, agent)
}

/// [`turned_off`] for a caller that has already left out the agents other people talk to (notebook/news.rs
/// `concerned`), the switches read once (`stored`).
pub fn off_for(config: &Config, agent: &AgentConfig, stored: &Stored) -> bool {
    crate::agents_admin::managed(config, &agent.name).is_ok()
        && agent.host_home.as_deref().is_none_or(str::is_empty)
        && !crate::profile_files::hands_off(&crate::agents_admin::profile_folder(config, agent))
        && !switched_on(config, agent, stored)
}

/// The person's switch; whether it changed. Clears the bridge's note that it had turned it off.
pub fn set(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let changed = update(config, |s| {
        let was_on = !s.off.iter().any(|a| a == agent);
        s.off.retain(|a| a != agent);
        if !on {
            s.off.push(agent.to_string());
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

/// The person changed `agent`'s Carnet switch themselves (agents_admin.rs `write_settings`): the bridge's note that it
/// had turned it off goes, so a return to a model that is not outside leaves it as they set it.
pub fn person_set_notebook(config: &Config, agent: &str) {
    let done = update(config, |s| {
        if let Some(g) = s.outside.get_mut(agent) {
            g.off.retain(|w| w != NOTEBOOK);
        }
    });
    if let Err(err) = done {
        tracing::warn!(agent = %agent, error = %err, "shared memory note not updated");
    }
}

/// What [`guard`] changed.
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

/// The defaults of an outside model for `agent`, once per provider (module doc); a model that is no longer outside
/// gets back what the bridge had turned off and the person never touched. What changed.
pub fn guard(config: &Config, agent: &AgentConfig) -> Changed {
    guard_at(config, agent, py::time_time())
}

pub fn guard_at(config: &Config, agent: &AgentConfig, now: f64) -> Changed {
    let name = agent.name.as_str();
    let outside = crate::outside_model::outside(config, agent);
    let stored = read(config);
    let record = stored.outside.get(name).cloned();
    // nothing moved since the last look (the usual case, every 15 s): no question of who has the setting
    match (&outside, &record) {
        (Some((provider, _)), Some(g)) if g.provider == *provider => return Changed::default(),
        (None, None) => return Changed::default(),
        _ => {}
    }
    if !has_setting(config, name) {
        return Changed::default();
    }
    let mut changed = Changed::default();
    let result: std::io::Result<()> = (|| {
        match (outside, record) {
            (Some((provider, _)), Some(g)) if g.provider == provider => {}
            (Some((provider, label)), before) => {
                // what was turned off for an earlier outside provider stays noted (a return gives it back)
                let mut off: Vec<String> = before.map(|g| g.off).unwrap_or_default();
                let shared_on = !stored.off.iter().any(|a| a == name);
                let notebook_on = crate::notebook::enabled(config, name);
                if notebook_on && crate::notebook::set_enabled(config, name, false)? {
                    changed.notebook = true;
                    if !off.iter().any(|w| w == NOTEBOOK) {
                        off.push(NOTEBOOK.into());
                    }
                }
                update(config, |s| {
                    if shared_on {
                        s.off.retain(|a| a != name);
                        s.off.push(name.to_string());
                        if !off.iter().any(|w| w == SHARED) {
                            off.push(SHARED.into());
                        }
                    }
                    s.outside.insert(name.to_string(), Guarded { provider: provider.clone(), label, at: now, off });
                })?;
                changed.shared = shared_on;
                tracing::info!(target: "bipbridge::agents", agent = %name, provider = %provider,
                    "outside model: shared memory and Carnet off by default");
            }
            (None, Some(g)) => {
                // back on our providers or the person's own model
                if g.off.iter().any(|w| w == NOTEBOOK) && crate::notebook::set_enabled(config, name, true)? {
                    changed.notebook = true;
                }
                let shared_back = g.off.iter().any(|w| w == SHARED);
                update(config, |s| {
                    if shared_back && s.off.iter().any(|a| a == name) {
                        s.off.retain(|a| a != name);
                        changed.shared = true;
                    }
                    s.outside.remove(name);
                })?;
                tracing::info!(target: "bipbridge::agents", agent = %name, "no longer on an outside model");
            }
            (None, None) => {}
        }
        Ok(())
    })();
    if let Err(err) = result {
        tracing::warn!(agent = %name, error = %err, "outside model defaults not applied");
    }
    changed
}

/// `shared_memory` in the agent's settings (agents_admin.rs `read_settings`); None without the setting.
pub fn view(config: &Config, agent: &AgentConfig) -> Option<Value> {
    if !has_setting(config, &agent.name) {
        return None;
    }
    let outside = crate::outside_model::outside(config, agent)
        .map(|(provider, label)| json!({"provider": provider, "label": label}));
    Some(json!({"on": enabled(config, &agent.name), "outside": outside}))
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

/// A deleted agent: its switch and its note go.
pub fn forget(config: &Config, agent: &str) {
    let done = update(config, |s| {
        s.off.retain(|a| a != agent);
        s.outside.remove(agent);
    });
    if let Err(err) = done {
        tracing::warn!(agent = %agent, error = %err, "shared memory switch not removed");
    }
}

/// The setting changed (the person, or the guard): it leaves or joins the circle at once (what it had received taken
/// out: shared_memory::sync), its personality and profile follow (profile_files::reconcile: the shared memory block,
/// `memory.user_profile_enabled`), and its conversations take a new prompt from their next turn. `tools`: its Carnet
/// changed too, its tools are given again.
pub async fn after_change(config: std::sync::Arc<Config>, agent: &str, tools: bool) {
    let Some(agent) = config.agent(Some(agent)).cloned() else { return };
    let c = config.clone();
    let _ = tokio::task::spawn_blocking(move || crate::shared_memory::sync(&c, py::time_time(), None)).await;
    let run = crate::agents_admin::default_run();
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

/// [`guard`], then what follows when it changed something ([`after_change`]); whether it did. After a model change
/// (the routes) and at each look of the shared memory (run_forever).
pub async fn guard_and_follow(config: std::sync::Arc<Config>, agent: &AgentConfig) -> bool {
    let changed = guard(&config, agent);
    if changed.any() {
        after_change(config, &agent.name, changed.notebook).await;
    }
    changed.any()
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
    let guarded = guard(&config, &agent);
    let (c, id, on) = (config.clone(), agent_id.clone(), body.on);
    let changed = tokio::task::spawn_blocking(move || set(&c, &id, on))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
    if changed || guarded.any() {
        after_change(config.clone(), &agent_id, guarded.notebook).await;
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
            Some(json!({"on": false, "outside": {"provider": "openai-codex", "label": "OpenAI"}}))
        );
        assert_eq!(view(&config, &budget), Some(json!({"on": true, "outside": null})));
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
        assert!(!turned_off(&config, "budget"), "no setting: never « off » (its handovers, its news stay as they are)");
        assert_eq!(guard_at(&config, &budget, 1.0), Changed::default());
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
}
