//! The way back into the shared hub (Alex, 21/11, with « tout ce qui vient de quelqu'un d'autre tourne en tour
//! d'invité »): an agent that left the hub for a space of its own only because other people talk to it (the family's
//! agent, an agent shared with a relative, one another household's agents write to: moved out before 21/11,
//! own_space.rs) comes back by itself once idle, and its space (its own Hermes gateway, ~250 MB, and its sandbox) is
//! freed. What other people send it runs as guest turns now (Hermes patch 0018), so it keeps the hub private.
//!
//! Never a project agent (its guests' turns are its own: vault.rs `hub_breakers`), never while the hub hosts one,
//! never an agent whose credentials the hub couldn't keep (vault.rs `holds_in_hub`: the hub's Python, its tools in a
//! sandbox of their own; the family's agent holds none, only the hub's privacy counts for it), never a space this
//! bridge didn't make (`[spaces] root`), never an agent set up by hand, never on an install that doesn't protect the
//! vault. An agent that moved at its first secret and that nobody else talks to stays where it is.
//!
//! How (agents_admin.rs `move_back_to_hub`): once it does nothing (agent_restart.rs `activity`, as for the way out),
//! its profile is copied into the hub while its space keeps serving, the hub takes it up, its conversations are copied
//! once more, the bridge switches to the hub (its section rewritten), its tools, its sandbox's files and its
//! credentials are set up there (written again from the bridge's master copy where its Hermes vault lacks them), and
//! once the last turn in its space ended (its end copied over unless the hub has had a turn since) the space is
//! stopped and removed. No line in the conversation: the way out had none either (only a move for a secret says one,
//! `own_space_moving`).
//!
//! A restart mid-way: `own-space-back.json` names the agents on their way back. At start, one its space still serves
//! (the switch not made) gets the hub's copy deleted and comes back later as any other; one the hub serves already
//! gets the rest done again and its space removed.
//!
//! No Python counterpart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};

/// The agents on their way back (agent → its space), in the bridge's data folder.
const JOURNAL: &str = "own-space-back.json";
/// How long a turn going on in its space may keep the space after the switch.
const SPACE_WAIT: Duration = Duration::from_secs(30 * 60);
/// Between two rounds of the start's tidy while an agent is busy.
const BUSY_AGAIN: Duration = Duration::from_secs(30);
/// After a failed way back, that agent is tried again so much later.
const FAILED_AGAIN: Duration = Duration::from_secs(15 * 60);

/// Whether `agent` comes back into the shared hub now (module doc).
pub fn comes_back(config: &Config, agent: &AgentConfig) -> bool {
    comes_back_with(config, agent, crate::sandbox::isolates(config, agent))
}

/// `comes_back`, `sandboxed`: its tools keep a sandbox of their own in the hub (sandbox.rs `isolates`, asked of its
/// space's profile, which the hub's copy is).
fn comes_back_with(config: &Config, agent: &AgentConfig, sandboxed: bool) -> bool {
    if !config.hermes.isolated
        || !config.hermes.enabled()
        || !agent.isolated
        || agent.shares == "external"
        || agent.host_home.as_deref().is_some_and(|h| !h.is_empty())
    {
        return false;
    }
    let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) else { return false };
    if config.spaces.root.is_empty() || Path::new(space) != Path::new(&config.spaces.root).join(&agent.name) {
        return false; // not a space this bridge made
    }
    if crate::project_agent::is_project(config, &agent.name) || !crate::vault::others_facing(config, &agent.name) {
        return false;
    }
    let breakers = crate::vault::hub_breakers(config);
    if config.agents.values().any(|a| crate::vault::in_hub(config, a) && breakers.contains(&a.name)) {
        return false; // the hub isn't private now
    }
    crate::family_agent::is_family(config, &agent.name) || (!config.hermes.python.is_empty() && sandboxed)
}

/// The agents that come back into the shared hub (`comes_back`), in name order.
pub fn to_bring_back(config: &Config) -> Vec<String> {
    let mut names: Vec<String> =
        config.agents.values().filter(|a| comes_back(config, a)).map(|a| a.name.clone()).collect();
    names.sort();
    names
}

// --- the journal -----------------------------------------------------------------------------------------------------

fn journal_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(JOURNAL)
}

/// The agents on their way back: agent → the space it comes from.
fn journal(config: &Config) -> BTreeMap<String, String> {
    std::fs::read(journal_path(config))
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, BTreeMap<String, String>>>(&b).ok())
        .and_then(|mut j| j.remove("agents"))
        .unwrap_or_default()
}

/// `agent` noted on its way back from `space` (None: no longer).
fn note(config: &Config, agent: &str, space: Option<&str>) {
    let mut agents = journal(config);
    match space {
        Some(space) => agents.insert(agent.to_string(), space.to_string()),
        None => agents.remove(agent),
    };
    let path = journal_path(config);
    let done = if agents.is_empty() {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        let text = serde_json::to_string(&serde_json::json!({ "agents": agents })).unwrap_or_default();
        let tmp = path.with_file_name(format!(".{JOURNAL}.{}.tmp", std::process::id()));
        crate::py::makedirs(&config.data_dir(), 0o700)
            .and_then(|_| crate::py::write_with_mode(&tmp.to_string_lossy(), text.as_bytes(), 0o600))
            .and_then(|_| std::fs::rename(&tmp, &path))
    };
    if let Err(exc) = done {
        tracing::error!(target: "bipbridge::own_space", agent = %agent, error = %exc, "the way back's journal not written");
    }
}

// --- the way back ----------------------------------------------------------------------------------------------------

/// What one try at bringing an agent back did.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Back {
    /// Back, or nothing to do (gone, in the hub already, not one that comes back).
    Done,
    /// Busy (a turn, a Live call, a background task, a scan, a watcher, a routine): later.
    Busy,
    /// The way back failed (logged), the agent served by its space as before: later.
    Failed,
}

/// One try: the agent comes back now if it should and does nothing (agent_restart.rs `activity`, the way out's checks).
async fn back_once(state: &AppState, name: &str) -> Back {
    let config = state.config();
    let Some(agent) = config.agent(Some(name)).cloned() else { return Back::Done };
    if !comes_back(&config, &agent) {
        return Back::Done;
    }
    if state.hub.agent_runs_at(name, &agent.hermes_url) > 0 {
        return Back::Busy;
    }
    if let Some(why) = crate::agent_restart::activity(state, &config, &agent, crate::py::time_time()).await.busy() {
        tracing::debug!(target: "bipbridge::own_space", agent = %name, why, "busy: comes back into the shared hub later");
        return Back::Busy;
    }
    let lock = crate::own_space::lock_of(name);
    let _one_at_a_time = lock.lock_owned().await;
    let config = state.config();
    let Some(agent) = config.agent(Some(name)).cloned() else { return Back::Done };
    if !comes_back(&config, &agent) {
        return Back::Done;
    }
    if state.hub.agent_runs_at(name, &agent.hermes_url) > 0 {
        return Back::Busy;
    }
    let space = agent.space.clone().unwrap_or_default();
    note(&config, name, Some(&space));
    tracing::info!(target: "bipbridge::own_space", agent = %name, "other people's turns are guest turns: back into the shared hub");
    match crate::agents_admin::move_back_to_hub(state, &agent).await {
        Ok((back, settled)) => {
            let state = state.clone();
            tokio::spawn(async move { finish(state, agent, back, settled).await });
            Back::Done
        }
        Err(exc) => {
            note(&config, name, None);
            tracing::warn!(target: "bipbridge::own_space", agent = %name, error = ?exc, "the way back into the shared hub failed: its space serves it as before");
            Back::Failed
        }
    }
}

/// After the switch: once the turns still going on in its space ended, their end copied into the hub (unless the hub
/// has had a turn since), then the space stopped and removed. The journal forgets the agent once all of it is done.
async fn finish(state: AppState, from: AgentConfig, back: AgentConfig, settled: bool) {
    let name = from.name.clone();
    let started = Instant::now();
    let had_turns = state.hub.agent_runs_at(&name, &from.hermes_url) > 0;
    let mut hub_used = false;
    while state.hub.agent_runs_at(&name, &from.hermes_url) > 0 && started.elapsed() < SPACE_WAIT {
        hub_used |= state.hub.agent_runs_at(&name, &back.hermes_url) > 0;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    hub_used |= state.hub.agent_runs_at(&name, &back.hermes_url) > 0;
    let config = state.config();
    if had_turns && !hub_used {
        let source = crate::agents_admin::profile_folder(&config, &from);
        let target = crate::spaces::hub_profile(&config, &name);
        let id = crate::spaces::hub_profile_id(&config, &name);
        let copied = tokio::task::spawn_blocking(move || crate::spaces::resync_database(&source, &target, &id)).await;
        if !matches!(copied, Ok(Ok(()))) {
            tracing::warn!(target: "bipbridge::own_space", agent = %name, "the end of its space's turn not copied into the hub");
        }
    } else if had_turns {
        tracing::warn!(target: "bipbridge::own_space", agent = %name, "a turn ended in its space after one in the hub: its end not copied");
    }
    remove_space(&config, &from).await;
    if settled {
        note(&config, &name, None);
    }
    state.vault.warn_hub_holders(&config);
    tracing::info!(target: "bipbridge::own_space", agent = %name, "back in the shared hub: its space removed");
}

/// The space `from` lived in: its gateway stopped, its folder gone.
async fn remove_space(config: &Config, from: &AgentConfig) {
    let spaces = crate::spaces::Spaces::new(crate::agents_admin::default_run());
    if let Err(exc) = spaces.remove(config, from).await {
        tracing::warn!(target: "bipbridge::own_space", agent = %from.name, error = %exc, "its space not removed");
    }
}

/// A way back a restart cut off (`own-space-back.json`): its space still serving it (the switch not made), the hub's
/// copy deleted (it comes back later as any other); the hub serving it, the rest done again and its space removed; the
/// agent deleted meanwhile, its space removed.
async fn settle_journal(state: &AppState) {
    let config = state.config();
    for (name, space) in journal(&config) {
        let from = AgentConfig { name: name.clone(), space: Some(space.clone()), ..Default::default() };
        match config.agent(Some(&name)).cloned() {
            Some(agent) if agent.space.as_deref() == Some(space.as_str()) => {
                tracing::info!(target: "bipbridge::own_space", agent = %name, "a way back cut off before the switch: the hub's copy deleted, its space serves it");
                let spaces = crate::spaces::Spaces::new(crate::agents_admin::default_run());
                let staging = crate::spaces::hub_profile(&config, &name).with_file_name(format!(".{name}.bip-back"));
                let _ = std::fs::remove_dir_all(staging);
                match spaces.clear_hub_copy(&config, &name).await {
                    Ok(()) => note(&config, &name, None),
                    Err(exc) => tracing::warn!(target: "bipbridge::own_space", agent = %name, error = %exc, "the hub's copy not deleted yet"),
                }
            }
            Some(agent) if crate::vault::in_hub(&config, &agent) => {
                tracing::info!(target: "bipbridge::own_space", agent = %name, "a way back cut off after the switch: finished");
                let (_, settled) = crate::agents_admin::settle_back(state, agent).await;
                remove_space(&config, &from).await;
                if settled {
                    note(&config, &name, None);
                }
            }
            Some(_) => note(&config, &name, None), // elsewhere now: nothing of ours
            None => {
                if Path::new(&space).exists() {
                    remove_space(&config, &from).await;
                }
                note(&config, &name, None);
            }
        }
    }
}

/// At start (app.rs): a way back a restart cut off settled first, then the agents that come back (`to_bring_back`), one
/// at a time, each once it does nothing; a busy one looked at again a bit later, a failed one 15 minutes later.
pub async fn at_start(state: AppState) {
    settle_journal(&state).await;
    if to_bring_back(&state.config()).is_empty() {
        return;
    }
    tokio::time::sleep(Duration::from_secs_f64(state.config().spaces.tidy_after)).await;
    let mut failed: BTreeMap<String, Instant> = BTreeMap::new();
    loop {
        let names = to_bring_back(&state.config());
        if names.is_empty() {
            return;
        }
        let mut busy = false;
        for name in &names {
            if failed.get(name).is_some_and(|at| at.elapsed() < FAILED_AGAIN) {
                continue;
            }
            match back_once(&state, name).await {
                Back::Done => {
                    failed.remove(name);
                }
                Back::Busy => busy = true,
                Back::Failed => {
                    failed.insert(name.clone(), Instant::now());
                }
            }
        }
        let next = if busy {
            BUSY_AGAIN
        } else if failed.is_empty() {
            Duration::from_secs(1)
        } else {
            failed.values().map(|at| FAILED_AGAIN.saturating_sub(at.elapsed())).min().unwrap_or(BUSY_AGAIN)
        };
        tokio::time::sleep(next.max(Duration::from_secs(1))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A protected place whose hub serves `hub`, with `spaced` in spaces this bridge made, its data in `dir`.
    fn place(dir: &Path, hub: &[&str], spaced: &[&str]) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.join("outbox.db").to_string_lossy().into_owned();
        config.hermes.isolated = true;
        config.hermes.bin = "/usr/bin/hermes".into();
        config.hermes.hub_url = "http://127.0.0.1:8642".into();
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.hermes.python = vec!["/usr/bin/python3".into()];
        config.spaces.root = dir.join("spaces").to_string_lossy().into_owned();
        for name in hub {
            let agent = AgentConfig {
                name: name.to_string(),
                hermes_url: format!("http://127.0.0.1:8642/p/{name}"),
                ..Default::default()
            };
            config.agents.insert(name.to_string(), agent);
        }
        for (i, name) in spaced.iter().enumerate() {
            let space = dir.join("spaces").join(name);
            let agent = AgentConfig {
                name: name.to_string(),
                hermes_url: format!("http://127.0.0.1:{}/p/{name}", 8660 + i),
                isolated: true,
                sandboxed: true,
                space: Some(space.to_string_lossy().into_owned()),
                python: vec!["/usr/bin/python3".into()],
                hermes_home: Some(space.join("profiles").join(name).to_string_lossy().into_owned()),
                ..Default::default()
            };
            config.agents.insert(name.to_string(), agent);
        }
        config
    }

    fn write(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
    }

    fn back(config: &Config, name: &str) -> bool {
        comes_back_with(config, &config.agents[name], true)
    }

    #[test]
    fn an_agent_other_people_talk_to_comes_back_into_the_hub() {
        let households = r#"{"households": {"lea": {"name": "Léa"}}}"#;
        let cases: [(&str, &str, &str); 3] = [
            ("family", "family-agent.json", r#"{"agent": "fete"}"#),
            ("shared", "agent-shares.json", r#"{"shared": {"fete": ["lea"]}}"#),
            ("written to", "agent-rules.json",
             r#"{"households": {"lea": {"in": {"mode": "only", "agents": ["fete"]}, "theirs": {"in": {"mode": "all"}}}}}"#),
        ];
        for (which, file, text) in cases {
            if which == "written to" && !crate::rules::RECEIVERS_FACE_OTHERS {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let config = place(dir.path(), &["coach"], &["fete", "budget"]);
            write(dir.path(), "households.json", households);
            write(dir.path(), file, text);
            assert!(back(&config, "fete"), "{which}");
            assert!(!back(&config, "budget"), "{which}: moved at its first secret, nobody else talks to it: stays");
            assert!(!back(&config, "coach"), "{which}: in the hub already");
        }
    }

    #[test]
    fn never_a_project_agent_nor_while_the_hub_is_not_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = place(dir.path(), &["coach", "proj"], &["fete", "chantier"]);
        write(dir.path(), "households.json", r#"{"households": {"lea": {"name": "Léa"}}}"#);
        write(dir.path(), "agent-shares.json", r#"{"shared": {"fete": ["lea"]}}"#);
        let projects = |names: &[&str]| {
            let rows: Vec<String> = names
                .iter()
                .map(|n| format!(r#""{n}": {{"created": 1.0, "ends": 9999999999.0, "archived": null}}"#))
                .collect();
            format!(r#"{{"projects": {{{}}}}}"#, rows.join(", "))
        };
        // a project agent in its space: never back; another one still in the hub: nobody comes back yet
        write(dir.path(), "project-agents.json", &projects(&["chantier", "proj"]));
        assert!(!back(&config, "chantier"));
        assert!(!back(&config, "fete"), "the hub hosts a project agent");
        write(dir.path(), "project-agents.json", &projects(&["chantier"]));
        assert!(back(&config, "fete"));
        assert!(!back(&config, "chantier"));
        // its credentials need a sandbox of its own and the hub's Python there; the family's agent holds none
        assert!(!comes_back_with(&config, &config.agents["fete"], false));
        config.hermes.python.clear();
        assert!(!back(&config, "fete"));
        write(dir.path(), "family-agent.json", r#"{"agent": "fete"}"#);
        assert!(back(&config, "fete") && comes_back_with(&config, &config.agents["fete"], false));
        // never on an install that doesn't protect the vault
        config.hermes.isolated = false;
        assert!(!back(&config, "fete"));
    }

    #[test]
    fn never_a_space_this_bridge_did_not_make() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach"], &["fete"]);
        write(dir.path(), "households.json", r#"{"households": {"lea": {"name": "Léa"}}}"#);
        write(dir.path(), "agent-shares.json", r#"{"shared": {"fete": ["lea"]}}"#);
        assert!(back(&config, "fete"));
        let mut elsewhere = config.clone();
        elsewhere.agents.get_mut("fete").unwrap().space = Some("/srv/its-own/fete".into());
        assert!(!back(&elsewhere, "fete"));
        let mut by_hand = config.clone();
        by_hand.agents.get_mut("fete").unwrap().shares = "external".into();
        assert!(!back(&by_hand, "fete"));
        let mut hosted = config.clone();
        hosted.agents.get_mut("fete").unwrap().host_home = Some("/home/x/.hermes".into());
        assert!(!back(&hosted, "fete"));
    }

    #[test]
    fn the_journal_names_the_agents_on_their_way_back() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &[], &[]);
        assert!(journal(&config).is_empty());
        note(&config, "fete", Some("/s/fete"));
        note(&config, "jardin", Some("/s/jardin"));
        assert_eq!(journal(&config), BTreeMap::from([("fete".into(), "/s/fete".into()), ("jardin".into(), "/s/jardin".into())]));
        note(&config, "fete", None);
        assert_eq!(journal(&config).keys().collect::<Vec<_>>(), ["jardin"]);
        note(&config, "jardin", None);
        assert!(!journal_path(&config).exists());
    }
}
