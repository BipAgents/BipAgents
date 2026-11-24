//! The text files of messages too long for their words (send.rs `long_text_as_file`). Alex, 24/11: a text pasted is
//! a message of its conversation, not a file sent: its file lives as long as the conversation — never purged after the
//! « Fichiers envoyés » days (app.rs `purge_loop`), deleted with the conversation (`delete_thread`, threads
//! `forget_agent`) and with its agent. The agent's other files are as they were.
//!
//! Kept in the agent's uploads like any document (its sandbox's copy, the apps' route to open it unchanged), each one
//! noted in `long_text_files` with its conversation, by its path under the uploads folder (`<month>/<id12>-<name>`).

use std::collections::HashSet;

use super::text;
use crate::config::{AgentConfig, Config};
use crate::store::{iso, utcnow, Store};

/// A long text's file (its path as the agent sees it, send.rs `Attachment::path`) noted with its conversation.
pub async fn keep(store: &Store, agent: &AgentConfig, thread: &str, seen: &str) -> anyhow::Result<()> {
    let Some(relative) = relative(agent, seen) else { return Ok(()) };
    store
        .run(
            "INSERT OR IGNORE INTO long_text_files (agent, path, thread_id, created_at) VALUES (?,?,?,?)",
            vec![text(&agent.name), text(&relative), text(thread), text(&iso(utcnow()))],
        )
        .await?;
    Ok(())
}

/// The agent's long texts' files, under its uploads folder (`files::purge_uploads` leaves them).
pub async fn kept(store: &Store, agent: &str) -> anyhow::Result<HashSet<String>> {
    let rows = store.run("SELECT path FROM long_text_files WHERE agent = ?", vec![text(agent)]).await?;
    Ok(rows.iter().filter_map(|r| r.get("path").and_then(|p| p.as_str()).map(str::to_string)).collect())
}

/// The long texts' files of a conversation (`thread`), or of all the agent's (none), deleted: (files removed).
pub async fn forget(store: &Store, config: &Config, agent: &AgentConfig, thread: Option<&str>) -> anyhow::Result<usize> {
    let filter = if thread.is_some() { "agent = ? AND thread_id = ?" } else { "agent = ?" };
    let params = || [Some(text(&agent.name)), thread.map(text)].into_iter().flatten().collect::<Vec<_>>();
    let rows = store.run(&format!("SELECT path FROM long_text_files WHERE {filter}"), params()).await?;
    let paths: Vec<String> = rows.iter().filter_map(|r| r.get("path").and_then(|p| p.as_str()).map(str::to_string)).collect();
    let root = agent.upload_dir_host.clone().filter(|r| !r.is_empty());
    let removed = match root {
        Some(root) if !paths.is_empty() => {
            let (config, name) = (config.clone(), agent.name.clone());
            tokio::task::spawn_blocking(move || {
                let removed = paths.iter().filter(|p| remove(&root, p)).count();
                // and from its sandbox's copy (sandbox.rs)
                if removed > 0 {
                    if let Err(exc) = crate::sandbox::sync_agent(&config, &name) {
                        tracing::warn!(agent = %name, error = %exc, "uploads not copied to the sandbox");
                    }
                }
                removed
            })
            .await?
        }
        _ => 0,
    };
    store.run(&format!("DELETE FROM long_text_files WHERE {filter}"), params()).await?;
    if removed > 0 {
        tracing::info!(agent = %agent.name, removed = removed, "long texts' files deleted with their conversation");
    }
    Ok(removed)
}

/// `<month>/<id12>-<name>` of a path the agent sees in its uploads; none for any other path.
fn relative(agent: &AgentConfig, seen: &str) -> Option<String> {
    let root = agent.upload_dir_container.as_deref().filter(|c| !c.is_empty())?;
    let rest = seen.strip_prefix(root)?.strip_prefix('/')?;
    stored(rest).then(|| rest.to_string())
}

/// A bridge's stored upload, one month folder down (never `..`, never further).
fn stored(relative: &str) -> bool {
    let mut parts = relative.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(month), Some(file), None) => {
            crate::files::MONTH_DIR.is_match(month) && crate::files::STORED_NAME.is_match(file) && !file.contains("..")
        }
        _ => false,
    }
}

/// The file at `relative` under `root` deleted (only a bridge's stored upload, never a link).
fn remove(root: &str, relative: &str) -> bool {
    if !stored(relative) {
        return false;
    }
    let path = std::path::Path::new(root).join(relative);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => std::fs::remove_file(&path).is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(dir: &std::path::Path) -> AgentConfig {
        AgentConfig {
            name: "vie".into(),
            upload_dir_host: Some(dir.to_string_lossy().into_owned()),
            upload_dir_container: Some("/fichiers".into()),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn only_a_stored_upload_is_noted() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(relative(&a, "/fichiers/2026-11/0123456789ab-Article.txt").as_deref(), Some("2026-11/0123456789ab-Article.txt"));
        assert_eq!(relative(&a, "/fichiers/2026-11/../../etc/passwd"), None);
        assert_eq!(relative(&a, "/autre/2026-11/0123456789ab-Article.txt"), None);
        assert_eq!(relative(&a, "/fichiers/0123456789ab-Article.txt"), None);
    }

    /// Alex 24/11: kept from the days' purge, deleted with its conversation, the others' with their agent.
    #[tokio::test]
    async fn a_long_texts_file_goes_with_its_conversation_and_its_agent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("bridge.db").to_str().unwrap()).unwrap();
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(uploads.join("2026-11")).unwrap();
        let a = agent(&uploads);
        let mut config = Config::with_key("k");
        config.agents.insert(a.name.clone(), a.clone());
        for (file, thread) in [("0123456789ab-Un.txt", "t_one"), ("0123456789ac-Deux.txt", "t_two")] {
            std::fs::write(uploads.join("2026-11").join(file), b"x").unwrap();
            keep(&store, &a, thread, &format!("/fichiers/2026-11/{file}")).await.unwrap();
        }
        std::fs::write(uploads.join("2026-11/0123456789ad-sent.pdf"), b"x").unwrap();
        let kept_now = kept(&store, "vie").await.unwrap();
        assert_eq!(kept_now.len(), 2);
        assert_eq!(crate::files::purge_uploads(&uploads.to_string_lossy(), 30, Some(crate::py::time_time() + 400.0 * 86400.0), &kept_now), 1);
        assert!(uploads.join("2026-11/0123456789ab-Un.txt").exists());

        assert_eq!(forget(&store, &config, &a, Some("t_one")).await.unwrap(), 1);
        assert!(!uploads.join("2026-11/0123456789ab-Un.txt").exists() && uploads.join("2026-11/0123456789ac-Deux.txt").exists());
        assert_eq!(kept(&store, "vie").await.unwrap().len(), 1);
        assert_eq!(forget(&store, &config, &a, None).await.unwrap(), 1);
        assert!(!uploads.join("2026-11/0123456789ac-Deux.txt").exists());
        assert!(kept(&store, "vie").await.unwrap().is_empty());
    }
}
