//! The agents' Hermes copies of the vault, in memory (design note « Clé du coffre », 16/11/2026, lot 2).
//!
//! Each agent's Hermes keeps its own copy of the items it was given (`agent/vault_store.py`: `<HERMES_HOME>/vault/`,
//! `vault.json.enc` and its `vault.key` beside it), which `browser_vault` fills from. On a place enrolled for sealed
//! keys (sealed_keys.rs), that folder is a link into memory, never a folder on the disk:
//!
//!     <HERMES_HOME>/vault  ->  $XDG_RUNTIME_DIR/bipagents/hermes-vault-<place id>/<agent>.<8 hex>   (tmpfs, 0700)
//!
//! Hermes reads, writes and fills through the link as it always did (no patch: its folder is made with
//! `mkdir(exist_ok=True)`, its files written beside and renamed). A server restart empties the memory folder: the
//! links then point at nothing, so Hermes reads nothing (never a stale copy) until the vault reopens and the bridge
//! rebuilds each copy from its master copy, in one Python run per agent (`add_many` in vault.rs `HERMES_SCRIPT`), the
//! new handles written into the master copy. A copy left on the disk (today's layout, before the enrolment) is moved
//! aside and erased only once its rebuild is checked (every item listed back, the handles written); a Hermes that
//! fails or answers short leaves it as it was.
//!
//! Each rebuild goes into a new folder and the link is swapped in one rename (a reader sees the old copy or the new
//! one). The bridge remembers, per agent, the folder it checked (`checked`): a give or a take-back then only looks at
//! the link. A bridge restart checks each copy once (a listing, no rebuild when it holds exactly the agent's items).
//!
//! Not enrolled: nothing here runs; the copies stay on the disk as Hermes writes them (today's layout). An agent whose
//! Hermes the bridge reaches in a container (its HERMES_HOME isn't its profile's folder here) is left as it is.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{anyhow, Result};

use crate::config::{AgentConfig, Config};

/// The folder Hermes keeps an agent's vault in, under its HERMES_HOME.
pub const LINK: &str = "vault";
/// The place's own folder in memory for the copies.
const ROOT_PREFIX: &str = "hermes-vault";

/// Whether this place keeps its agents' Hermes copies in memory: its vault's keys sealed (or a sealing under way).
pub fn in_memory(config: &Config) -> bool {
    crate::sealed_keys::enrolled(Path::new(&config.data_dir()))
}

/// The place's folder for the copies in memory (made if `make`).
pub fn root(data: &Path, make: bool) -> Result<PathBuf> {
    crate::sealed_keys::memory_folder_of(data, ROOT_PREFIX, make)
}

/// Where `agent`'s Hermes keeps its vault on this server: its profile's folder, when that is the HERMES_HOME its
/// Python is given (a Hermes in a container sees another one: None).
pub fn link_of(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    let target = crate::vault::vault_target(config, agent);
    let home = target.hermes_home.as_deref().filter(|h| !h.is_empty())?;
    let folder = crate::agents_admin::profile_folder(config, agent);
    (Path::new(home) == folder.as_path()).then(|| folder.join(LINK))
}

/// The folder in memory `link` points to, when it is one of this place's (in `root`, a real folder).
pub fn current(link: &Path, root: &Path) -> Option<PathBuf> {
    if !std::fs::symlink_metadata(link).ok()?.file_type().is_symlink() {
        return None;
    }
    let target = std::fs::read_link(link).ok()?;
    if target.parent()? != root {
        return None;
    }
    std::fs::symlink_metadata(&target).ok()?.is_dir().then_some(target)
}

fn safe(agent: &str) -> String {
    agent.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn short_id() -> String {
    crate::py::uuid4_hex()[..8].to_string()
}

/// A new, empty folder for `agent`'s copy in `root` (0700).
pub fn new_folder(root: &Path, agent: &str) -> Result<PathBuf> {
    let path = root.join(format!("{}.{}", safe(agent), short_id()));
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    Ok(path)
}

/// `link` made to point at `target` in one rename (a reader sees the old copy or the new one). A folder there (Hermes'
/// copy on the disk) is moved aside beside it first: returned, to be erased once the handles follow ([`erase`]) or put
/// back ([`put_back`]).
pub fn point(link: &Path, target: &Path) -> Result<Vec<PathBuf>> {
    let parent = link.parent().ok_or_else(|| anyhow!("no folder for {}", link.display()))?;
    std::fs::create_dir_all(parent)?;
    let mut aside = Vec::new();
    for _ in 0..3 {
        if std::fs::symlink_metadata(link).is_ok_and(|m| m.is_dir()) {
            let moved = parent.join(format!(".vault-disk-{}", short_id()));
            std::fs::rename(link, &moved)?;
            aside.push(moved);
        }
        let tmp = parent.join(format!(".vault-link-{}", short_id()));
        std::os::unix::fs::symlink(target, &tmp)?;
        match std::fs::rename(&tmp, link) {
            Ok(()) => return Ok(aside),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                // a folder made there meanwhile (Hermes making its vault): aside too, then again
                if !std::fs::symlink_metadata(link).is_ok_and(|m| m.is_dir()) {
                    put_back(link, None, &aside);
                    return Err(e.into());
                }
            }
        }
    }
    put_back(link, None, &aside);
    Err(anyhow!("{} kept changing", link.display()))
}

/// What [`point`] changed, undone: the link to `previous` again, or the folder set aside back in its place.
pub fn put_back(link: &Path, previous: Option<&Path>, aside: &[PathBuf]) {
    if let Some(first) = aside.first() {
        if std::fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = std::fs::remove_file(link);
        }
        if let Err(e) = std::fs::rename(first, link) {
            tracing::error!(target: "bipbridge::vault", error = %e, folder = %first.display(),
                "vault: a Hermes copy set aside could not be put back");
        }
        return;
    }
    if let Some(previous) = previous {
        let _ = point(link, previous);
    }
}

/// A folder of the disk erased: each file overwritten then removed (best effort on an SSD), then the folders. How many
/// files.
pub fn erase(folder: &Path) -> Result<usize> {
    let mut count = 0;
    let meta = std::fs::symlink_metadata(folder)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        crate::sealed_keys::wipe(folder)?;
        return Ok(1);
    }
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        let kind = std::fs::symlink_metadata(&path)?;
        if kind.is_dir() {
            count += erase(&path)?;
        } else {
            crate::sealed_keys::wipe(&path)?;
            count += 1;
        }
    }
    std::fs::remove_dir(folder)?;
    Ok(count)
}

// -- what this process checked ------------------------------------------------------------------------------------------

/// (place, agent) -> the folder in memory its copy was checked in.
static CHECKED: LazyLock<Mutex<HashMap<(PathBuf, String), PathBuf>>> = LazyLock::new(Default::default);

fn checked_map() -> std::sync::MutexGuard<'static, HashMap<(PathBuf, String), PathBuf>> {
    CHECKED.lock().unwrap_or_else(|e| e.into_inner())
}

/// The folder `agent`'s copy was last checked in by this process.
pub fn checked(data: &Path, agent: &str) -> Option<PathBuf> {
    checked_map().get(&(crate::sealed_keys::id(data), agent.to_string())).cloned()
}

pub(crate) fn set_checked(data: &Path, agent: &str, folder: &Path) {
    checked_map().insert((crate::sealed_keys::id(data), agent.to_string()), folder.to_path_buf());
}

pub(crate) fn unset_checked(data: &Path, agent: &str) {
    checked_map().remove(&(crate::sealed_keys::id(data), agent.to_string()));
}

/// What this process checked for the place of `data` forgotten (each copy looked at again).
pub fn forget_checked(data: &Path) {
    let id = crate::sealed_keys::id(data);
    checked_map().retain(|(place, _), _| *place != id);
}

/// The copies in memory dropped (the vault closed at a start: nothing to read until it reopens and they are rebuilt).
pub fn drop_all(data: &Path) {
    forget_checked(data);
    let Ok(root) = root(data, false) else { return };
    let Ok(entries) = std::fs::read_dir(&root) else { return };
    let mut dropped = 0;
    for entry in entries.flatten() {
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            dropped += 1;
        }
    }
    if dropped > 0 {
        tracing::info!(target: "bipbridge::vault", dropped, "vault: closed, the agents' Hermes copies dropped until it reopens");
    }
}

/// The folders in `root` no agent's link points to (an agent moved or deleted, a rebuild cut off) removed. How many.
pub fn tidy(root: &Path, keep: &HashSet<PathBuf>) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else { return 0 };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| !keep.contains(p))
        .filter(|p| std::fs::remove_dir_all(p).is_ok())
        .count()
}

/// The Hermes homes of this place: the hub's, and each space's.
fn hermes_homes(config: &Config) -> Vec<PathBuf> {
    let hub = if config.hermes.home.is_empty() {
        PathBuf::from(crate::py::expanduser("~")).join(".hermes")
    } else {
        PathBuf::from(&config.hermes.home)
    };
    let mut homes = vec![hub];
    if let Ok(entries) = std::fs::read_dir(&config.spaces.root) {
        homes.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for agent in config.agents.values() {
        if let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) {
            let space = PathBuf::from(space);
            if !homes.contains(&space) {
                homes.push(space);
            }
        }
    }
    homes
}

/// Hermes copies on the disk no agent of this place uses (a moved agent's left in the hub, a deleted agent's, the
/// default profile's): each home's `vault` and `profiles/*/vault` folders (never a link).
pub fn strays(config: &Config, keep: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for home in hermes_homes(config) {
        let mut candidates = vec![home.join(LINK)];
        if let Ok(profiles) = std::fs::read_dir(home.join("profiles")) {
            candidates.extend(profiles.flatten().map(|p| p.path().join(LINK)));
        }
        for candidate in candidates {
            let is_folder = std::fs::symlink_metadata(&candidate).is_ok_and(|m| m.is_dir());
            if is_folder && !keep.contains(&candidate) && !found.contains(&candidate) {
                found.push(candidate);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_swapped_in_one_rename_sets_the_disk_copy_aside_and_puts_it_back_on_demand() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("mem");
        std::fs::create_dir_all(&root).unwrap();
        let home = dir.path().join("profiles").join("vie");
        std::fs::create_dir_all(home.join(LINK)).unwrap();
        std::fs::write(home.join(LINK).join("vault.key"), "disk").unwrap();
        let link = home.join(LINK);
        let first = new_folder(&root, "vie").unwrap();
        let aside = point(&link, &first).unwrap();
        assert_eq!(aside.len(), 1);
        assert_eq!(current(&link, &root), Some(first.clone()));
        assert_eq!(std::fs::read_to_string(aside[0].join("vault.key")).unwrap(), "disk");
        // undone: the disk's copy where it was
        put_back(&link, None, &aside);
        assert!(std::fs::symlink_metadata(&link).unwrap().is_dir());
        assert_eq!(std::fs::read_to_string(link.join("vault.key")).unwrap(), "disk");
        // again, then a second rebuild replaces the link itself
        let aside = point(&link, &first).unwrap();
        assert_eq!(erase(&aside[0]).unwrap(), 1);
        assert!(!aside[0].exists());
        let second = new_folder(&root, "vie").unwrap();
        assert!(point(&link, &second).unwrap().is_empty());
        assert_eq!(current(&link, &root), Some(second.clone()));
        // a link elsewhere, or to nothing, is not one of ours
        std::fs::remove_dir(&second).unwrap();
        assert_eq!(current(&link, &root), None);
        let elsewhere = dir.path().join("elsewhere.x");
        std::fs::create_dir_all(&elsewhere).unwrap();
        point(&link, &elsewhere).unwrap();
        assert_eq!(current(&link, &root), None);
        // tidy: only the folders no link points to
        let kept: HashSet<PathBuf> = [first.clone()].into_iter().collect();
        let extra = new_folder(&root, "budget").unwrap();
        assert_eq!(tidy(&root, &kept), 1);
        assert!(first.exists() && !extra.exists());
    }

    #[test]
    fn what_was_checked_is_per_place_and_forgotten_per_place() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        set_checked(a.path(), "vie", Path::new("/m/vie.1"));
        set_checked(b.path(), "vie", Path::new("/m/vie.2"));
        assert_eq!(checked(a.path(), "vie"), Some(PathBuf::from("/m/vie.1")));
        forget_checked(a.path());
        assert_eq!(checked(a.path(), "vie"), None);
        assert_eq!(checked(b.path(), "vie"), Some(PathBuf::from("/m/vie.2")));
        unset_checked(b.path(), "vie");
        assert_eq!(checked(b.path(), "vie"), None);
    }

    #[test]
    fn agent_names_make_safe_folder_names() {
        assert_eq!(safe("vie"), "vie");
        assert_eq!(safe("../x y"), "___x_y");
    }
}
