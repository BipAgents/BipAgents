//! An agent's space whose Hermes refuses every turn because its runtime epoch is no longer its own (409
//! `stale_epoch`) gets its gateway restarted once by the bridge, and the turn is tried again once.
//!
//! Investigation of 24/11: an agent that moved into its own space at its first secret had the hub's database copied
//! over its space's at the end of the hub's turn (spaces.rs `resync_database`), the hub's runtime epoch with it. Its
//! space's Hermes then answered every `POST /v1/runs` 409 `stale_epoch` (hermes_state_runtime.py `_epoch`), the bridge
//! a 502 to every message, and only `systemctl --user restart bipagents-space-<agent>` gave it back (its Hermes begins
//! the next epoch at its start). The copy now keeps the target's epoch; this heals the agents already mute in
//! production, and any other way a space's epoch could go stale.
//!
//! - Only an agent of its own space: the hub's gateway serves every agent of the hub, never restarted for this.
//! - At most one restart per agent per [`GAP`]. A turn that starts while one runs waits for it and is tried again once
//!   too; a refusal after that, or within the gap, is the same error as before.
//! - The restart is the one settings changes take (agents_admin.rs `restart_gateway`: a message finding the gateway
//!   down meanwhile waits in line, threads/send.rs), then the bridge waits for its Hermes to answer again.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::{AgentConfig, SharedConfig};

/// The shortest time between two restarts of one agent's space for this.
pub const GAP: Duration = Duration::from_secs(10 * 60);
/// A restart that ended so recently: a turn refused meanwhile (or that waited for it) is tried again once.
const JUST_DONE: Duration = Duration::from_secs(60);
/// How long the bridge waits for the restarted Hermes to answer.
const SERVED_WITHIN: f64 = 90.0;

static CONFIG: OnceLock<SharedConfig> = OnceLock::new();
/// One agent's last restart for this (when it ended), behind its own lock: one restart at a time.
type Slot = Arc<tokio::sync::Mutex<Option<Instant>>>;
static LAST: LazyLock<Mutex<HashMap<String, Slot>>> = LazyLock::new(Default::default);

/// The bridge's configuration, for the restart's command (set at start; none in unit tests: nothing is restarted).
pub fn init(config: &SharedConfig) {
    let _ = CONFIG.set(config.clone());
}

/// Whether Hermes refused a turn's start because its runtime epoch is not its own any more: 409 with the code
/// `stale_epoch` (gateway/platforms/api_server_authority_runs.py `store_error_response`, OpenAI's error envelope).
pub fn is_stale_epoch(status: u16, body: &str) -> bool {
    status == 409
        && serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.pointer("/error/code").and_then(Value::as_str).map(|c| c == "stale_epoch"))
            .unwrap_or(false)
}

/// What a refused start does, given the agent's last restart for this.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    /// Restart its gateway, then try again.
    Restart,
    /// A restart just ended: try again, without another.
    Again,
    /// Within the gap: the refusal stands.
    Refused,
}

fn step(last: Option<Instant>, now: Instant) -> Step {
    match last.map(|at| now.saturating_duration_since(at)) {
        None => Step::Restart,
        Some(since) if since < JUST_DONE => Step::Again,
        Some(since) if since < GAP => Step::Refused,
        Some(_) => Step::Restart,
    }
}

/// After a start refused with `stale_epoch`: the agent's space's gateway restarted (at most once per [`GAP`]), true
/// once its Hermes answers again or a restart just ended: the caller tries the turn again once.
pub async fn heal(http: &reqwest::Client, agent: &AgentConfig) -> bool {
    if agent.space.as_deref().is_none_or(str::is_empty) {
        return false;
    }
    let Some(config) = CONFIG.get() else { return false };
    let slot = LAST.lock().unwrap_or_else(|e| e.into_inner()).entry(agent.name.clone()).or_default().clone();
    let mut last = slot.lock().await;
    match step(*last, Instant::now()) {
        Step::Again => return true,
        Step::Refused => return false,
        Step::Restart => {}
    }
    tracing::warn!(target: "bipbridge::space_heal", agent = %agent.name,
        "its space's Hermes refuses every turn (stale_epoch): its gateway restarted once");
    let restarted =
        crate::agents_admin::restart_gateway(&config.get(), agent, &crate::agents_admin::default_run()).await;
    let healed = match restarted {
        Ok(()) => match crate::agents_admin::wait_served_within(http, agent, false, SERVED_WITHIN).await {
            Ok(()) => true,
            Err(exc) => {
                tracing::warn!(target: "bipbridge::space_heal", agent = %agent.name, error = %exc,
                    "its space's Hermes not answering after its restart");
                false
            }
        },
        Err(exc) => {
            tracing::warn!(target: "bipbridge::space_heal", agent = %agent.name, error = %format!("{exc:#}"),
                "its space's gateway not restarted");
            false
        }
    };
    *last = Some(Instant::now());
    healed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_epoch_is_hermes_409_with_its_code() {
        let body = r#"{"error": {"message": "stale_epoch", "type": "invalid_request_error", "param": null, "code": "stale_epoch"}}"#;
        assert!(is_stale_epoch(409, body));
        assert!(!is_stale_epoch(502, body));
        let other = r#"{"error": {"message": "admission_conflict", "code": "admission_conflict"}}"#;
        assert!(!is_stale_epoch(409, other));
        assert!(!is_stale_epoch(409, "stale_epoch"));
    }

    #[test]
    fn one_restart_per_gap_and_one_more_try_right_after_it() {
        let now = Instant::now();
        assert_eq!(step(None, now), Step::Restart);
        assert_eq!(step(Some(now - Duration::from_secs(5)), now), Step::Again);
        assert_eq!(step(Some(now - JUST_DONE), now), Step::Refused);
        assert_eq!(step(Some(now - GAP + Duration::from_secs(1)), now), Step::Refused);
        assert_eq!(step(Some(now - GAP), now), Step::Restart);
    }

    #[tokio::test]
    async fn an_agent_of_the_hub_is_never_restarted_for_it() {
        let http = reqwest::Client::new();
        let hub = AgentConfig { name: "unit-heal-hub".into(), ..Default::default() };
        assert!(!heal(&http, &hub).await);
        assert!(LAST.lock().unwrap().get("unit-heal-hub").is_none());
    }
}
