//! The hub's Hermes gateway asked over its control socket (`<HERMES_HOME>/gateway.sock`, or the path written in
//! `gateway.sock.path` when it is too long for a socket address; one JSON line per connection: `{"verb", "id",
//! "protocol", "params"}` → `{"ok", "result" | "error"}`).
//!
//! Why (26/11, measured on a real Hermes v0.23.1, the /tmp/acl-rig bench): `hermes profile create` has the gateway
//! serve the new profile at once, and its MCP servers are discovered right then, before the bridge has written its
//! own entry into the profile's config.yaml. Hermes only reconciles them again at its housekeeping tick (60 s, the
//! first one per profile skipped): 60 to 120 s. The bridge waited 20 s for the new agent's tools, then restarted the
//! hub's whole gateway (agents_admin.rs `tools_loaded`), a creation took 30 to 140 s, and several cards in a row went
//! past the apps' waits. The `reload-plugins` verb reconciles one served profile's MCP servers now (its tools listed
//! in ~0.13 s), with no cut for its sessions nor anything for the other profiles.
//!
//! No Python counterpart.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};

/// How long the gateway gets to answer (`reload-plugins` waits up to 10 s for the servers to connect).
pub const ANSWER_WAIT: Duration = Duration::from_secs(12);

/// The gateway's control socket in `home` (its HERMES_HOME).
pub fn socket_path(home: &Path) -> PathBuf {
    match std::fs::read_to_string(home.join("gateway.sock.path")) {
        Ok(text) if !text.trim().is_empty() => PathBuf::from(text.trim()),
        _ => home.join("gateway.sock"),
    }
}

/// One request: the answer's `result`. Err: no gateway listening, no answer in time, or its refusal.
#[cfg(unix)]
pub async fn call(home: &Path, verb: &str, params: Value, wait: Duration) -> anyhow::Result<Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let path = socket_path(home);
    let work = async {
        let mut stream = tokio::net::UnixStream::connect(&path).await?;
        let line = json!({"verb": verb, "id": 1, "protocol": 1, "params": params}).to_string() + "\n";
        stream.write_all(line.as_bytes()).await?;
        let mut answer = String::new();
        tokio::io::BufReader::new(stream).read_line(&mut answer).await?;
        let answer: Value = serde_json::from_str(answer.trim())?;
        let result = answer.get("result").cloned().unwrap_or(Value::Null);
        let refused = answer.get("error").or_else(|| result.get("error")).filter(|e| !e.is_null());
        if answer.get("ok") == Some(&json!(false)) || refused.is_some() {
            anyhow::bail!("{verb} refused: {}", refused.map(Value::to_string).unwrap_or_default());
        }
        Ok(result)
    };
    tokio::time::timeout(wait, work).await.map_err(|_| anyhow::anyhow!("{verb}: no answer"))?
}

#[cfg(not(unix))]
pub async fn call(_home: &Path, verb: &str, _params: Value, _wait: Duration) -> anyhow::Result<Value> {
    anyhow::bail!("{verb}: no control socket here")
}

/// The gateway home serving `agent`: its space's, else the hub's.
fn gateway_home(config: &Config, agent: &AgentConfig) -> PathBuf {
    match agent.space.as_deref().filter(|s| !s.is_empty()) {
        Some(space) => PathBuf::from(space),
        None => PathBuf::from(crate::agents_admin::install_home(config)),
    }
}

/// A served profile's MCP servers brought in line with its config.yaml now (Hermes' `reload-plugins` verb): the
/// bridge's tools listed by a new agent's Hermes in a moment, not at the gateway's next tick. Whether it was done (an
/// older Hermes, or none listening: the bridge's own wait and restart follow, as before).
pub async fn reload_mcp(config: &Config, agent: &AgentConfig) -> bool {
    if !config.hermes.enabled() {
        return false;
    }
    let home = gateway_home(config, agent);
    let profile = home.join("profiles").join(&agent.name);
    let started = std::time::Instant::now();
    match call(&home, "reload-plugins", json!({"home": profile.to_string_lossy()}), ANSWER_WAIT).await {
        Ok(_) => {
            tracing::info!(target: "bipbridge::agents", agent = %agent.name, ms = started.elapsed().as_millis() as u64, "agent's tools reloaded by its gateway");
            true
        }
        Err(exc) => {
            tracing::info!(target: "bipbridge::agents", agent = %agent.name, error = %crate::agents_admin::tail(&format!("{exc:#}"), 200), "agent's tools not reloaded by its gateway");
            false
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A gateway's socket answering one line: what it was asked, and its answer.
    async fn gateway(home: &Path, answer: &'static str) -> tokio::task::JoinHandle<String> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::UnixListener::bind(home.join("gateway.sock")).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut asked = String::new();
            tokio::io::BufReader::new(read).read_line(&mut asked).await.unwrap();
            write.write_all(answer.as_bytes()).await.unwrap();
            asked
        })
    }

    #[tokio::test]
    async fn a_verb_is_one_json_line_and_its_result_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let asked = gateway(dir.path(), "{\"ok\": true, \"result\": {\"reloaded\": 1}}\n").await;
        let result = call(dir.path(), "reload-plugins", json!({"home": "/h/profiles/cuisine"}), ANSWER_WAIT).await;
        assert_eq!(result.unwrap(), json!({"reloaded": 1}));
        let asked: Value = serde_json::from_str(asked.await.unwrap().trim()).unwrap();
        assert_eq!(asked, json!({"verb": "reload-plugins", "id": 1, "protocol": 1, "params": {"home": "/h/profiles/cuisine"}}));
    }

    #[tokio::test]
    async fn a_refusal_or_no_gateway_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(call(dir.path(), "reload-plugins", json!({}), Duration::from_secs(1)).await.is_err());
        let _asked = gateway(dir.path(), "{\"ok\": true, \"result\": {\"error\": \"home is not served by this gateway\"}}\n").await;
        let refused = call(dir.path(), "reload-plugins", json!({}), ANSWER_WAIT).await.unwrap_err();
        assert!(refused.to_string().contains("not served"), "{refused}");
    }

    #[test]
    fn a_long_socket_path_is_read_from_its_pointer() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(socket_path(dir.path()), dir.path().join("gateway.sock"));
        std::fs::write(dir.path().join("gateway.sock.path"), "/run/user/1000/hermes-1a2b.sock\n").unwrap();
        assert_eq!(socket_path(dir.path()), PathBuf::from("/run/user/1000/hermes-1a2b.sock"));
    }
}
