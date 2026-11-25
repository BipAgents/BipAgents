//! Credentials and the shared hub (Alex, 2026-10-28 evening: a person's own agents are no longer separated from each
//! other; 24/11, for 1.1: « les agents de la personne ne déménagent plus jamais »).
//!
//! A person's own agents hold credentials right where they are, in the shared hub (vault.rs `holds_in_hub`), and never
//! move for it. Since 21/11 an agent other people talk to (the family's agent, one shared with a relative, one another
//! household's agents write to) gets their messages as guest turns (Hermes patch 0018) and stays in the hub too (one
//! that moved out before comes back by itself, own_space_back.rs). Only a project agent breaks the hub's privacy
//! (vault.rs `hub_breakers`: its guests' turns are its own), so it never lives there:
//!
//! - made (project_agent.rs), it leaves the hub at once (`leave_soon`), no turn awaited: its space is made while the
//!   hub still serves it, a turn going on there ends there and its end is copied over (`finish`); its friends' share
//!   opens once that move ended (read_shares.rs `create`, `out_of_hub`);
//! - one found in the hub at start (made before 24/11), or whose move at creation failed, leaves once idle
//!   (`tidy_at_start`, tried again later after a failure).
//!
//! While one is on its way out (vault.rs `hub_leaving`), the hub's agents keep nothing new: an agent asking for a
//! secret or an account waits for the move under way (`hub_private`), or, none under way, gets the plain answer at once
//! (`LATER_FOR_AGENT`: it tries again a bit later); the app's gives get `needs_own_space` (« … pourra en garder dans
//! quelques minutes »). The agent that asks never moves. One of the person's agents moved out by the rule of before
//! 24/11 (at its first secret) stays in its space. The credentials already kept in the hub stay there (logged at start
//! and at each move, `Vault::warn_hub_holders`). The trade-off, plainly: one of the person's agents tricked by what it
//! read could reach another of their agents' passwords; it costs no ~250 MB container per agent.
//!
//! No Python counterpart.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::Dict;

/// How long the hub keeps serving a moved agent's profile for a turn still going on there.
const HUB_WAIT: Duration = Duration::from_secs(30 * 60);
/// How long an agent's ask for a secret waits for a move out of the hub under way (one takes up to two minutes).
const LEAVE_WAIT: Duration = Duration::from_secs(180);
/// How long a project's share waits for it to be out of the hub (the apps wait a minute for the share's first upload).
const SHARE_WAIT: Duration = Duration::from_secs(40);

/// The agent may hold credentials now, or will once the agent on its way out of the hub has left: the tools that keep
/// some are offered.
pub fn may_hold(config: &Config, agent: &AgentConfig) -> bool {
    crate::vault::holder(config, agent).is_none() || crate::vault::may_hold_in_hub(config, agent)
}

/// What an agent's call for a secret or an account gets while the hub can't keep one yet (the tool's existing words).
pub const LATER_FOR_AGENT: &str = "Setting up a safe place to keep this access did not work this time; nothing was \
asked or kept. Tell the person in one plain sentence that you will try again a bit later (no technical words, never \
another way to give you the password), and call again later.";

static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);
/// The agents whose move out of the hub is under way (`Leaving`).
static LEAVING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// A tidy loop runs (`tidy`): one at a time.
static TIDYING: AtomicBool = AtomicBool::new(false);

/// The agent's own lock: one move (out of the hub, back into it) at a time, and a delete of its hub copy that waited
/// (spaces.rs `forget_unless_served`) never in the middle of one.
pub fn lock_of(agent: &str) -> Arc<tokio::sync::Mutex<()>> {
    LOCKS.lock().unwrap_or_else(|e| e.into_inner()).entry(agent.to_string()).or_default().clone()
}

/// An agent's move out of the hub under way, for as long as this lives.
struct Leaving(String);

impl Leaving {
    fn new(agent: &str) -> Self {
        LEAVING.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.to_string());
        Leaving(agent.to_string())
    }
}

impl Drop for Leaving {
    fn drop(&mut self) {
        LEAVING.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

fn leaving_now(agent: Option<&str>) -> bool {
    let leaving = LEAVING.lock().unwrap_or_else(|e| e.into_inner());
    agent.map_or(!leaving.is_empty(), |a| leaving.contains(a))
}

/// Before an agent keeps a secret of its own asking (ask_secret, a connector, an account): Ok when the hub may keep it,
/// or the agent isn't one the hub would hold (its own checks follow). While an agent other people talk to is on its way
/// out of the hub (`vault::hub_leaving`), the move under way is waited for (`LEAVE_WAIT` at most); none under way, or
/// still not out: Err, the plain answer for its call. The agent that asks never moves.
pub async fn hub_private(state: &AppState, agent: &str) -> Result<(), Dict> {
    let started = Instant::now();
    loop {
        let config = state.config();
        let waits = config.agent(Some(agent)).is_some_and(|a| crate::vault::may_hold_in_hub(&config, a));
        if !waits || !crate::vault::hub_leaving(&config) {
            return Ok(());
        }
        if !leaving_now(None) || started.elapsed() > LEAVE_WAIT {
            return Err(tool_text(LATER_FOR_AGENT, true));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Before a project's friends' share opens (read_shares.rs): its move out of the hub, when one is under way, is waited
/// for (`SHARE_WAIT` at most). One that failed is tried again later (`tidy`); the share opens meanwhile, as before 24/11.
pub async fn out_of_hub(agent: &str) {
    let started = Instant::now();
    while leaving_now(Some(agent)) && started.elapsed() < SHARE_WAIT {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Once the turns still going on in the hub have ended: their end copied into the space (unless the space has had a
/// turn of its own since), then the hub forgets the profile.
async fn finish(state: AppState, agent: String, hub_url: String, new_url: String, home: Option<String>) {
    let started = Instant::now();
    let had_turns = state.hub.agent_runs_at(&agent, &hub_url) > 0;
    let mut space_used = false;
    while state.hub.agent_runs_at(&agent, &hub_url) > 0 && started.elapsed() < HUB_WAIT {
        space_used |= state.hub.agent_runs_at(&agent, &new_url) > 0;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    space_used |= state.hub.agent_runs_at(&agent, &new_url) > 0;
    let config = state.config();
    let source = crate::spaces::hub_profile(&config, &agent);
    if let (true, false, Some(home)) = (had_turns, space_used, home) {
        let target = PathBuf::from(home);
        let root = config.agent(Some(&agent)).and_then(|a| a.space.clone()).map(PathBuf::from).unwrap_or_default();
        let profile_id = crate::spaces::hermes_profile_id(&config, &root, &agent);
        let copied =
            tokio::task::spawn_blocking(move || crate::spaces::resync_database(&source, &target, &profile_id)).await;
        if !matches!(copied, Ok(Ok(()))) {
            tracing::warn!(target: "bipbridge::own_space", agent = %agent, "the hub turn's end not copied into the space");
        }
    }
    // (up to 30 minutes after the move: by then a way back may have put the agent into the hub again, review 3 B1)
    let spaces = crate::spaces::Spaces::new(crate::agents_admin::default_run());
    match spaces.forget_unless_served(&state.config, &agent).await {
        Ok(crate::spaces::Forgot::Deleted) => {
            tracing::info!(target: "bipbridge::own_space", agent = %agent, "the hub forgot the moved agent")
        }
        Ok(_) => {}
        Err(exc) => {
            tracing::warn!(target: "bipbridge::own_space", agent = %agent, error = %exc, "the hub kept the moved agent's profile: tried again later");
            spaces.forget_later(&state.config, &agent);
        }
    }
}

/// A project agent just made (project_agent.rs) leaves the shared hub now, no turn awaited (module doc); a move that
/// fails is logged and tried again later, once it is idle (`tidy`). Only on an install that protects the vault
/// (elsewhere nothing holds credentials, nothing to keep apart).
pub fn leave_soon(state: &AppState, agent: &str) {
    let (state, name, leaving) = (state.clone(), agent.to_string(), Leaving::new(agent));
    tokio::spawn(async move {
        let left = leave_now(&state, &name).await;
        drop(leaving);
        if left == Leave::Failed {
            tidy_soon(state, TIDY_FAILED_AGAIN);
        }
    });
}

/// Whether `agent` should leave the hub now (`leave_soon`, `tidy`).
pub fn should_leave_hub(config: &Config, agent: &AgentConfig) -> bool {
    config.hermes.isolated
        && !agent.isolated
        && crate::vault::in_hub(config, agent)
        && crate::vault::leaves_hub(config, &agent.name)
}

/// What one try at moving an agent out of the hub did.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Leave {
    /// Moved, or nothing to do any more (gone, already in its own space…).
    Done,
    /// Busy (a turn, a Live call, a background task, a scan, a watcher, a routine): later.
    Busy,
    /// The move failed (logged): later.
    Failed,
}

/// The agent out of the hub now if it should (the app's move, agents_admin.rs `move_into_space`: conversations, memory
/// and files go with it), the hub serving it until the turns going on there ended (`finish`).
async fn leave_now(state: &AppState, name: &str) -> Leave {
    let lock = lock_of(name);
    let _one_at_a_time = lock.lock_owned().await;
    let config = state.config();
    let Some(agent) = config.agent(Some(name)).cloned() else { return Leave::Done };
    if !should_leave_hub(&config, &agent) {
        return Leave::Done;
    }
    let _leaving = Leaving::new(name);
    tracing::info!(target: "bipbridge::own_space", agent = %name, "other people talk to it: moving out of the shared hub");
    let outcome = match crate::agents_admin::move_into_space(state, &agent).await {
        Ok(moved) => {
            tracing::info!(target: "bipbridge::own_space", agent = %name, "in its own space: the hub is the person's alone");
            let (state2, name, new_url) = (state.clone(), name.to_string(), moved.hermes_url.clone());
            tokio::spawn(async move { finish(state2, name, agent.hermes_url, new_url, moved.hermes_home).await });
            Leave::Done
        }
        Err(exc) => {
            tracing::warn!(target: "bipbridge::own_space", agent = %name, error = ?exc, "move out of the shared hub failed: it stays there for now, the hub's agents keep nothing new meanwhile");
            Leave::Failed
        }
    };
    state.vault.warn_hub_holders(&state.config());
    outcome
}

/// One try of the tidy: the agent leaves the hub now if it should and does nothing (agent_restart.rs `activity`).
async fn leave_when_idle(state: &AppState, name: &str) -> Leave {
    let config = state.config();
    let Some(agent) = config.agent(Some(name)).cloned() else { return Leave::Done };
    if !should_leave_hub(&config, &agent) {
        return Leave::Done;
    }
    if state.hub.agent_runs_at(name, &agent.hermes_url) > 0 {
        return Leave::Busy;
    }
    if let Some(why) = crate::agent_restart::activity(state, &config, &agent, crate::py::time_time()).await.busy() {
        tracing::debug!(target: "bipbridge::own_space", agent = %name, why, "busy: moves out of the shared hub later");
        return Leave::Busy;
    }
    leave_now(state, name).await
}

/// Between two rounds of the tidy while an agent is busy.
const TIDY_BUSY_AGAIN: Duration = Duration::from_secs(30);
/// After a failed move, the tidy tries that agent again so much later.
const TIDY_FAILED_AGAIN: Duration = Duration::from_secs(15 * 60);

/// The agents of a protected install's shared hub that other people talk to and should leave it (`should_leave_hub`),
/// in name order.
pub fn to_tidy(config: &Config) -> Vec<String> {
    let mut names: Vec<String> =
        config.agents.values().filter(|a| should_leave_hub(config, a)).map(|a| a.name.clone()).collect();
    names.sort();
    names
}

/// At start (app.rs, so at each update): the project agents that still live in the shared hub of a protected install
/// (made before 24/11) leave it, by themselves, one at a time, each once it does nothing (`tidy`). Done when the hub
/// hosts none: the person's agents then keep their credentials right there (vault.rs `holds_in_hub`).
pub async fn tidy_at_start(state: AppState) {
    let after = Duration::from_secs_f64(state.config().spaces.tidy_after);
    tidy_soon(state, after);
}

/// The tidy, `after` so long, unless one runs: a busy agent is looked at again a bit later, a failed move is logged
/// and tried again later. Never an unprotected install, never an agent already in its own space.
fn tidy_soon(state: AppState, after: Duration) {
    if to_tidy(&state.config()).is_empty() || TIDYING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        tidy(&state).await;
        TIDYING.store(false, Ordering::SeqCst);
    });
}

async fn tidy(state: &AppState) {
    let mut failed: HashMap<String, Instant> = HashMap::new();
    loop {
        let names = to_tidy(&state.config());
        if names.is_empty() {
            tracing::info!(target: "bipbridge::own_space", "the shared hub hosts no agent other people talk to");
            return;
        }
        let mut busy = false;
        for name in &names {
            if failed.get(name).is_some_and(|at| at.elapsed() < TIDY_FAILED_AGAIN) {
                continue;
            }
            match leave_when_idle(state, name).await {
                Leave::Done => {
                    failed.remove(name);
                }
                Leave::Busy => busy = true,
                Leave::Failed => {
                    tracing::warn!(target: "bipbridge::own_space", agent = %name, "tried again in 15 minutes");
                    failed.insert(name.clone(), Instant::now());
                }
            }
        }
        let next = if busy {
            TIDY_BUSY_AGAIN
        } else if failed.is_empty() {
            Duration::from_secs(1) // all moved: the next round says so
        } else {
            failed.values().map(|at| TIDY_FAILED_AGAIN.saturating_sub(at.elapsed())).min().unwrap_or(TIDY_BUSY_AGAIN)
        };
        tokio::time::sleep(next.max(Duration::from_secs(1))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_is_plain() {
        for word in ["espace", "space", "sandbox", "container", "conteneur", "Hermes", "profile", "navigateur", "browser"] {
            assert!(!LATER_FOR_AGENT.contains(word), "{word}");
        }
    }

    #[test]
    fn a_move_under_way_is_known_until_it_ends() {
        assert!(!leaving_now(Some("unit-own-space")));
        let leaving = Leaving::new("unit-own-space");
        assert!(leaving_now(Some("unit-own-space")) && leaving_now(None));
        drop(leaving);
        assert!(!leaving_now(Some("unit-own-space")));
    }
}
