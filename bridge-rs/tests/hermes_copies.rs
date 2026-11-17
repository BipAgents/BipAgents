//! The agents' Hermes copies of the vault in memory (src/hermes_copies.rs; design note « Clé du coffre », lot 2), with
//! a fake Hermes: each agent's vault a JSON file in the folder its request names, the operations as HERMES_SCRIPT runs
//! them (`add`, `add_many`, `remove`, the listing). The real Hermes v0.23.1 is the black-box suite's
//! (bridge/tests_blackbox/test_vault_hermes_copies.py); this file is the regular suite's.
//!
//! What is checked: a place not enrolled keeps its copies on its disk and runs nothing more; once enrolled, each agent's
//! `<HERMES_HOME>/vault` becomes a link into the memory folder, its copy rebuilt there in one run per agent, the bridge's
//! handles following, the disk's copy erased only after that rebuild is checked (never when its Hermes fails or answers
//! short); a second pass only looks; while closed nothing is given nor rebuilt; an agent moved into its space leaves its
//! copy behind.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bipbridge::config::{AgentConfig, Config};
use bipbridge::sealed_keys::{self, Place, State};
use bipbridge::space::{self, KeyService, ShareError, ShareSource};
use bipbridge::vault::{Vault, VaultRun};
use bipbridge::Dict;
use common::{config_dict, Urls};
use serde_json::{json, Value};
use zeroize::Zeroizing;

const AGENTS: [&str; 3] = ["vie", "budget", "coach"];

struct Service(Vec<u8>);

#[async_trait::async_trait]
impl ShareSource for Service {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        Ok(Zeroizing::new(self.0.clone()))
    }
}

/// The fake Hermes: what it ran, and the agents whose `add_many` fails or answers one item short.
#[derive(Default)]
struct Hermes {
    runs: Mutex<Vec<(String, String, usize)>>,
    failing: Mutex<HashSet<String>>,
    short: Mutex<HashSet<String>>,
    /// Labels its `add_many` refuses (a Hermes that validates more strictly): `null` for them.
    refused: Mutex<HashSet<String>>,
}

impl Hermes {
    fn ops(&self, op: &str) -> Vec<(String, usize)> {
        self.runs.lock().unwrap().iter().filter(|(_, o, _)| o == op).map(|(a, _, n)| (a.clone(), *n)).collect()
    }

    fn count(&self) -> usize {
        self.runs.lock().unwrap().len()
    }
}

fn store_of(folder: &Path) -> Vec<Value> {
    std::fs::read(folder.join("vault.json.enc"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok())
        .unwrap_or_default()
}

fn write_store(folder: &Path, items: &[Value]) -> anyhow::Result<()> {
    // as Hermes' VaultStore: its folder made (it fails on a link to nothing), its key beside its file
    if std::fs::symlink_metadata(folder).is_ok() && !folder.is_dir() {
        anyhow::bail!("FileExistsError: [Errno 17] File exists: '{}'", folder.display());
    }
    std::fs::create_dir_all(folder)?;
    if !folder.join("vault.key").exists() {
        std::fs::write(folder.join("vault.key"), format!("hermes-key-{}", bipbridge::py::uuid4_hex()))?;
    }
    std::fs::write(folder.join("vault.json.enc"), serde_json::to_vec(items)?)?;
    Ok(())
}

fn fake_run(hermes: Arc<Hermes>) -> VaultRun {
    Arc::new(move |agent: AgentConfig, request: Dict| {
        let hermes = hermes.clone();
        Box::pin(async move {
            let op = request.get("op").and_then(Value::as_str).unwrap_or("list").to_string();
            let items: Vec<Value> = request.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            hermes.runs.lock().unwrap().push((agent.name.clone(), op.clone(), items.len()));
            let folder = match request.get("vault").and_then(Value::as_str) {
                Some(v) => PathBuf::from(v),
                None => PathBuf::from(agent.hermes_home.clone().unwrap_or_default()).join("vault"),
            };
            let new_item = |r: &Value| {
                json!({"id": format!("vault_{}", &bipbridge::py::uuid4_hex()[..12]), "kind": r["kind"],
                       "label": r["label"], "origin": r["origin"], "secret": r["secret"]})
            };
            let meta = |items: &[Value]| -> Vec<Value> {
                items.iter().map(|i| json!({"id": i["id"], "kind": i["kind"], "label": i["label"], "origin": i["origin"]})).collect()
            };
            let mut out = Dict::new();
            match op.as_str() {
                "add" => {
                    let mut items = store_of(&folder);
                    let item = new_item(&Value::Object(request.clone()));
                    out.insert("id".into(), item["id"].clone());
                    items.push(item);
                    write_store(&folder, &items)?;
                }
                "add_many" => {
                    if hermes.failing.lock().unwrap().contains(&agent.name) {
                        anyhow::bail!("{}: the agent's vault could not be reached", agent.name);
                    }
                    let mut stored = store_of(&folder);
                    let mut ids = Vec::new();
                    for item in &items {
                        if item["label"].as_str().is_some_and(|l| hermes.refused.lock().unwrap().contains(l)) {
                            ids.push(Value::Null);
                            continue;
                        }
                        let made = new_item(item);
                        ids.push(made["id"].clone());
                        stored.push(made);
                    }
                    write_store(&folder, &stored)?;
                    let mut listed = meta(&stored);
                    if hermes.short.lock().unwrap().contains(&agent.name) {
                        listed.remove(0);
                    }
                    out.insert("ids".into(), json!(ids));
                    out.insert("items".into(), json!(listed));
                }
                "remove" => {
                    let mut items = store_of(&folder);
                    let before = items.len();
                    items.retain(|i| i["id"] != request["id"]);
                    let removed = items.len() != before;
                    if removed {
                        write_store(&folder, &items)?;
                    }
                    out.insert("removed".into(), json!(removed));
                }
                "export" => {
                    // (reviewer, pass 3) as Hermes v0.23.1's VaultStore._key: ciphertext without its key is a VaultError
                    if folder.join("vault.json.enc").is_file() && !folder.join("vault.key").exists() {
                        anyhow::bail!("agent.vault_store.VaultError: The vault key could not be found (vault.key is missing)");
                    }
                    // as HERMES_SCRIPT's: each item with its secret (a disk copy about to be erased, read once)
                    let items: Vec<Value> = store_of(&folder)
                        .iter()
                        .map(|i| {
                            let secret = i["secret"].clone();
                            json!({"id": i["id"], "kind": i["kind"], "label": i["label"], "origin": i["origin"],
                                   "identifier_type": i.get("identifier_type").cloned().unwrap_or(secret["identifier_type"].clone()),
                                   "identifier": i.get("identifier").cloned().unwrap_or(secret["identifier"].clone()),
                                   "secret": secret})
                        })
                        .collect();
                    out.insert("items".into(), json!(items));
                }
                _ => {
                    if std::fs::symlink_metadata(&folder).is_ok() && !folder.is_dir() {
                        anyhow::bail!("FileExistsError: [Errno 17] File exists: '{}'", folder.display());
                    }
                    out.insert("items".into(), json!(meta(&store_of(&folder))));
                }
            }
            Ok(out)
        })
    })
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    runtime: PathBuf,
    place: Place,
    config: Config,
    hermes: Arc<Hermes>,
    vault: Vault,
    share: Vec<u8>,
}

impl Fixture {
    fn home(&self, agent: &str) -> PathBuf {
        self.root.join("data").join("spaces").join(agent).join("profiles").join(agent)
    }

    fn link(&self, agent: &str) -> PathBuf {
        self.home(agent).join("vault")
    }

    fn records(&self) -> Vec<Dict> {
        let key = sealed_keys::data_key(&self.root.join("data").join("vault"), sealed_keys::DataKey::Vault)
            .unwrap()
            .map(|k| k.to_string())
            .unwrap_or_else(|| std::fs::read_to_string(self.root.join("data/vault/vault.key")).unwrap());
        let token = std::fs::read_to_string(self.root.join("data/vault/vault.json.enc")).unwrap();
        let plain = fernet::Fernet::new(key.trim()).unwrap().decrypt(token.trim()).unwrap();
        serde_json::from_slice::<Vec<Dict>>(&plain).unwrap()
    }

    /// label -> this agent's handle in the bridge's master copy.
    fn handles(&self, agent: &str) -> BTreeMap<String, String> {
        self.records()
            .iter()
            .filter_map(|r| {
                let handle = r.get("agents")?.get(agent)?.as_str()?.to_string();
                Some((r["label"].as_str()?.to_string(), handle))
            })
            .collect()
    }

    /// label -> id in the agent's Hermes copy, read through its link.
    fn copy(&self, agent: &str) -> BTreeMap<String, String> {
        store_of(&self.link(agent))
            .iter()
            .map(|i| (i["label"].as_str().unwrap().to_string(), i["id"].as_str().unwrap().to_string()))
            .collect()
    }

    async fn give(&self, label: &str, site: &str, agents: &[&str]) {
        let body = json!({"label": label, "site": site, "identifier": "alex@example.org",
                          "password": format!("pw-{label}-0123456789"), "agents": agents});
        self.vault.create(&self.config, body.as_object().unwrap()).await.unwrap();
    }

    async fn enrol(&self) {
        let service = Service(self.share.clone());
        let settings = KeyService { url: "https://keys.test".into(), token: "t".repeat(40), grant: None, signature: None };
        sealed_keys::enrol(&self.place, Some(settings), &service).await.unwrap();
        assert_eq!(sealed_keys::state(&self.place.data), State::Open);
        bipbridge::hermes_copies::forget_checked(&self.place.data);
    }

    /// A server restart: the memory folder empty, the bridge a new process.
    fn reboot(&self) {
        let _ = std::fs::remove_dir_all(self.runtime.join("bipagents"));
        sealed_keys::forget_memory(&self.place.data);
        bipbridge::hermes_copies::forget_checked(&self.place.data);
    }

    fn files_on_disk(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    walk(&entry.path(), out);
                } else if kind.is_file() {
                    out.push(entry.path());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root.join("data"), &mut out);
        out
    }

    fn vault_folders_on_disk(&self) -> Vec<PathBuf> {
        let spaces = self.root.join("data").join("spaces");
        AGENTS
            .iter()
            .map(|a| spaces.join(a).join("profiles").join(a).join("vault"))
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
            .collect()
    }
}

fn fixture() -> Fixture {
    fixture_with(|_, _| {})
}

/// The place, `extra` changing its config first (the test's root given): the hub's Hermes home is `<root>/hub`.
fn fixture_with(extra: impl FnOnce(&mut Value, &Path)) -> Fixture {
    std::env::set_var(space::FAKE_ENV, "1");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let runtime = root.join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    let place = Place::new(root.join("data"), root.join("space"));
    sealed_keys::forget_memory(&place.data);
    bipbridge::hermes_copies::forget_checked(&place.data);
    let mut dict = config_dict(&root, &Urls::default());
    dict["hermes"]["isolated"] = json!(true);
    // the hub's own home here, never the account's ~/.hermes (the copies no agent uses there are erased)
    dict["hermes"]["home"] = json!(root.join("hub").to_string_lossy());
    std::fs::create_dir_all(root.join("hub").join("profiles")).unwrap();
    for agent in AGENTS {
        let space = root.join("data").join("spaces").join(agent);
        let home = space.join("profiles").join(agent);
        std::fs::create_dir_all(&home).unwrap();
        dict["agents"][agent] = json!({"hermes_url": "http://127.0.0.1:9/p", "hermes_key": "k".repeat(32),
            "isolated": true, "sandboxed": true, "space": space.to_string_lossy(), "python": "/nowhere/python",
            "hermes_home": home.to_string_lossy()});
    }
    extra(&mut dict, &root);
    let config = bipbridge::config::parse_config(&dict, None).unwrap();
    std::fs::create_dir_all(root.join("data").join("vault")).unwrap();
    let hermes = Arc::new(Hermes::default());
    let vault = Vault::with_run(&root.join("data").join("vault").to_string_lossy(), fake_run(hermes.clone()));
    Fixture { _dir: dir, root, runtime, place, config, hermes, vault, share: space::new_share().to_vec() }
}

async fn given(h: &Fixture) {
    h.give("SNCF", "sncf-connect.com", &["vie", "budget"]).await;
    h.give("Impots", "impots.gouv.fr", &["vie"]).await;
}

// One test at a time: they set XDG_RUNTIME_DIR and the space's test switch for the whole process.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn a_place_not_enrolled_keeps_its_copies_on_its_disk_and_runs_nothing_more() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let runs = h.hermes.count();
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.rebuilt.is_empty() && report.failed.is_empty() && report.erased == 0);
    assert_eq!(h.hermes.count(), runs, "nothing run");
    for agent in ["vie", "budget"] {
        let meta = std::fs::symlink_metadata(h.link(agent)).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink(), "today's layout: {agent}'s copy on its disk");
    }
    assert!(!h.runtime.join("bipagents").exists());
    assert_eq!(h.copy("vie").values().cloned().collect::<HashSet<_>>(), h.handles("vie").values().cloned().collect());
}

#[tokio::test]
async fn once_enrolled_each_copy_is_rebuilt_in_memory_in_one_run_and_the_disk_s_erased_after() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let before = h.handles("vie");
    let disk_key = std::fs::read(h.link("vie").join("vault.key")).unwrap();
    h.enrol().await;
    assert!(h.link("vie").join("vault.json.enc").is_file(), "enrolling alone leaves the copies");
    let runs = h.hermes.count();
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let mut rebuilt = h.hermes.ops("add_many");
    rebuilt.sort();
    assert_eq!(rebuilt, vec![("budget".to_string(), 1), ("vie".to_string(), 2)]);
    let mut read = h.hermes.ops("export");
    read.sort();
    assert_eq!(read, vec![("budget".to_string(), 0), ("vie".to_string(), 0)], "each disk copy read once before it goes");
    assert_eq!(h.hermes.count(), runs + 4, "one rebuild per agent holding items (and one read of its disk copy)");
    for agent in AGENTS {
        let link = h.link(agent);
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "{agent}");
        let target = std::fs::read_link(&link).unwrap();
        assert!(target.starts_with(h.runtime.join("bipagents")), "{}", target.display());
        assert!(target.is_dir());
    }
    assert!(h.vault_folders_on_disk().is_empty());
    for file in h.files_on_disk() {
        let data = std::fs::read(&file).unwrap();
        assert!(!data.windows(disk_key.len()).any(|w| w == disk_key.as_slice()), "{}", file.display());
        assert_ne!(file.file_name().unwrap(), "vault.key", "{}", file.display());
    }
    for agent in ["vie", "budget"] {
        assert_eq!(h.copy(agent), h.handles(agent), "{agent}'s handles follow its copy");
    }
    assert!(h.handles("vie").values().all(|v| !before.values().any(|b| b == v)));
    // a second pass only looks (the bridge restarted: what it checked forgotten): no rebuild
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let handles = h.handles("vie");
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.rebuilt.is_empty() && report.failed.is_empty(), "{report:?}");
    assert_eq!(h.hermes.ops("add_many").len(), 2);
    assert_eq!(h.handles("vie"), handles);
    // what is given now goes into memory, what is taken back leaves it
    h.give("Ameli", "ameli.fr", &["vie"]).await;
    assert!(h.copy("vie").contains_key("Ameli"));
    assert_eq!(h.copy("vie"), h.handles("vie"));
    let ameli = h.vault.items().into_iter().find(|i| i.label == "Ameli").unwrap();
    h.vault.delete(&h.config, &ameli.id).await.unwrap();
    assert!(!h.copy("vie").contains_key("Ameli"));
    assert!(h.vault_folders_on_disk().is_empty());
}

#[tokio::test]
async fn a_copy_on_the_disk_is_kept_until_its_rebuild_is_checked() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    let snapshot = |h: &Fixture| -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(h.link("vie"))
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect()
    };
    let copy = snapshot(&h);
    let handles = h.handles("vie");
    h.hermes.failing.lock().unwrap().insert("vie".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()]);
    assert!(!std::fs::symlink_metadata(h.link("vie")).unwrap().file_type().is_symlink());
    assert_eq!(snapshot(&h), copy, "untouched");
    assert_eq!(h.handles("vie"), handles);
    assert!(std::fs::symlink_metadata(h.link("budget")).unwrap().file_type().is_symlink(), "the others go on");
    // a Hermes answering one item short is not trusted either
    h.hermes.failing.lock().unwrap().clear();
    h.hermes.short.lock().unwrap().insert("vie".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()]);
    assert_eq!(snapshot(&h), copy);
    assert_eq!(h.handles("vie"), handles);
    // nothing left of the failed tries in memory
    let leftovers: Vec<_> = std::fs::read_dir(h.runtime.join("bipagents"))
        .unwrap()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("vie"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // a give to it meanwhile is refused (its copy not ready), never written on the disk
    let body = json!({"label": "Ameli", "site": "ameli.fr", "identifier": "a@b.c", "password": "pw-ameli-0123456789",
                      "agents": ["vie"]});
    assert!(h.vault.create(&h.config, body.as_object().unwrap()).await.is_err());
    assert_eq!(snapshot(&h), copy);
    // mended
    h.hermes.short.lock().unwrap().clear();
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(report.erased >= 1);
    assert_eq!(h.copy("vie"), h.handles("vie"));
    assert!(h.vault_folders_on_disk().is_empty());
}

#[tokio::test]
async fn while_closed_nothing_is_given_nor_rebuilt_then_the_reopened_vault_rebuilds() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    h.vault.copies_ready_all(&h.config).await;
    h.reboot();
    assert!(matches!(sealed_keys::state(&h.place.data), State::Closed(_)));
    assert!(!h.link("vie").exists(), "the link points at nothing: no copy to read");
    let runs = h.hermes.count();
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.rebuilt.is_empty());
    let body = json!({"label": "Ameli", "site": "ameli.fr", "identifier": "a@b.c", "password": "pw-ameli-0123456789",
                      "agents": ["vie"]});
    let refused = h.vault.create(&h.config, body.as_object().unwrap()).await.unwrap_err();
    assert!(sealed_keys::is_locked(&refused), "{refused:#}");
    assert_eq!(h.hermes.count(), runs, "nothing run while closed");
    assert!(h.vault_folders_on_disk().is_empty(), "never a copy made on the disk meanwhile");
    // reopened (the key service answered): the copies rebuilt from the master, one run per agent
    assert!(sealed_keys::try_open(&h.place, &Service(h.share.clone())).await);
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let mut rebuilt: Vec<(String, usize)> = h.hermes.ops("add_many").into_iter().skip(2).collect();
    rebuilt.sort();
    assert_eq!(rebuilt, vec![("budget".to_string(), 1), ("vie".to_string(), 2)]);
    for agent in ["vie", "budget"] {
        assert_eq!(h.copy(agent), h.handles(agent), "{agent}");
    }
}

#[test]
fn an_agent_moved_into_its_space_leaves_its_copy_behind_when_the_copies_are_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hub").join("profiles").join("vie");
    std::fs::create_dir_all(source.join("vault")).unwrap();
    std::fs::write(source.join("vault").join("vault.key"), "k").unwrap();
    std::fs::write(source.join("SOUL.md"), "Tu es Vie.").unwrap();
    std::fs::create_dir_all(source.join("skills").join("vault")).unwrap();
    // what a swap cut off leaves beside the link (the disk's copy set aside, a link not renamed yet)
    std::fs::create_dir_all(source.join(".vault-disk-1a2b3c4d")).unwrap();
    std::fs::write(source.join(".vault-disk-1a2b3c4d").join("vault.key"), "k").unwrap();
    std::os::unix::fs::symlink("/nowhere", source.join(".vault-link-0badc0de")).unwrap();
    let kept = dir.path().join("kept");
    bipbridge::spaces::copy_profile_except(&source, &kept, &[]).unwrap();
    assert!(kept.join("vault").join("vault.key").is_file(), "today: copied with the rest");
    let moved = dir.path().join("moved");
    bipbridge::spaces::copy_profile_except(&source, &moved, &bipbridge::hermes_copies::NOT_COPIED).unwrap();
    assert!(!moved.join("vault").exists(), "in memory: never copied");
    let names: Vec<String> =
        std::fs::read_dir(&moved).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n.starts_with(".vault-")), "nor what a cut-off swap left: {names:?}");
    assert!(moved.join("SOUL.md").is_file() && moved.join("skills").join("vault").is_dir(), "only the profile's own");
}

// -- review of 17/11 (B1, R1, R2, R6) ------------------------------------------------------------------------------------

/// Files named vault.key under `dir` (links not followed), and entries a swap sets aside (`.vault-*`).
fn left_on_disk(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type().unwrap();
            if name.starts_with(".vault-") || (kind.is_file() && name == "vault.key") {
                out.push(entry.path());
            }
            if kind.is_dir() {
                walk(&entry.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

#[tokio::test]
async fn a_swap_cut_off_before_the_erase_leaves_no_copy_on_the_disk_once_the_bridge_starts_again() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    let disk_key = std::fs::read(h.link("vie").join("vault.key")).unwrap();
    let root = bipbridge::hermes_copies::root(&h.place.data, true).unwrap();
    // Vie: its copy in memory (its items as they were), the link swapped, the bridge stopped before the erase
    let folder = bipbridge::hermes_copies::new_folder(&root, "vie").unwrap();
    for file in ["vault.key", "vault.json.enc"] {
        std::fs::copy(h.link("vie").join(file), folder.join(file)).unwrap();
    }
    let aside = bipbridge::hermes_copies::point(&h.link("vie"), &folder).unwrap();
    assert_eq!(aside.len(), 1);
    // Budget: its copy set aside and never put back (a put back that failed), no link
    std::fs::rename(h.link("budget"), h.home("budget").join(".vault-disk-0badc0de")).unwrap();
    // Coach: a link made, the bridge stopped before its rename
    std::os::unix::fs::symlink(&folder, h.home("coach").join(".vault-link-0badc0de")).unwrap();
    // and in the hub's home, beside its default profile
    std::fs::create_dir_all(h.root.join("hub").join(".vault-disk-12345678")).unwrap();
    std::fs::write(h.root.join("hub").join(".vault-disk-12345678").join("vault.key"), "hub-key").unwrap();
    // the next start
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    // the set-aside copy read before it goes (anything only its Hermes held taken into the vault), then rebuilt
    assert!(report.rebuilt.contains(&"vie".to_string()), "{report:?}");
    assert!(report.rebuilt.contains(&"budget".to_string()), "{report:?}");
    let left: Vec<PathBuf> = [h.root.join("data"), h.root.join("hub")].iter().flat_map(|d| left_on_disk(d)).collect();
    assert!(left.is_empty(), "a Hermes copy and its key left on the disk: {left:?}");
    for file in h.files_on_disk() {
        let data = std::fs::read(&file).unwrap();
        assert!(!data.windows(disk_key.len()).any(|w| w == disk_key.as_slice()), "{}", file.display());
    }
    for agent in ["vie", "budget"] {
        assert_eq!(h.copy(agent), h.handles(agent), "{agent}");
    }
}

#[tokio::test]
async fn a_master_copy_not_written_puts_the_disk_s_copy_back_and_leaves_nothing_aside() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    let copy = std::fs::read(h.link("vie").join("vault.key")).unwrap();
    let handles = h.handles("vie");
    let master = h.root.join("data").join("vault");
    std::fs::set_permissions(&master, std::fs::Permissions::from_mode(0o500)).unwrap();
    let report = h.vault.copies_ready_all(&h.config).await;
    std::fs::set_permissions(&master, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(report.failed.contains(&"vie".to_string()), "{report:?}");
    let meta = std::fs::symlink_metadata(h.link("vie")).unwrap();
    assert!(meta.is_dir() && !meta.file_type().is_symlink(), "the disk's copy back in its place");
    assert_eq!(std::fs::read(h.link("vie").join("vault.key")).unwrap(), copy);
    assert_eq!(h.handles("vie"), handles);
    let aside: Vec<_> = left_on_disk(&h.home("vie")).into_iter().filter(|p| p != &h.link("vie").join("vault.key")).collect();
    assert!(aside.is_empty(), "{aside:?}");
    // written again: rebuilt, the disk's copy gone
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(left_on_disk(&h.root.join("data")).is_empty());
}

#[tokio::test]
async fn copies_the_bridge_does_not_rebuild_are_never_erased() {
    let _guard = SERIAL.lock().await;
    // Perso: an agent of the hub whose Hermes runs in a container (its copy where it is, the bridge can't link it)
    let h = fixture_with(|dict, root| {
        let hub = root.join("hub");
        dict["hermes"]["container_home"] = json!("/opt/data");
        dict["hermes"]["python"] = json!("/nowhere/python");
        dict["hermes"]["bin"] = json!("/nowhere/hermes");
        dict["hermes"]["hub_url"] = json!("http://127.0.0.1:9");
        dict["agents"]["perso"] = json!({"hermes_url": "http://127.0.0.1:9/p/perso", "hermes_key": "p".repeat(32)});
        std::fs::create_dir_all(hub.join("profiles").join("perso").join("vault")).unwrap();
        std::fs::write(hub.join("profiles").join("perso").join("vault").join("vault.key"), "perso-key").unwrap();
        // a profile of the hub the bridge doesn't know (Hermes in a container: its own), and its default profile's
        std::fs::create_dir_all(hub.join("profiles").join("jardin").join("vault")).unwrap();
        std::fs::write(hub.join("profiles").join("jardin").join("vault").join("vault.key"), "jardin-key").unwrap();
        std::fs::create_dir_all(hub.join("vault")).unwrap();
        std::fs::write(hub.join("vault").join("vault.key"), "default-key").unwrap();
    });
    given(&h).await;
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let hub = h.root.join("hub");
    for kept in [hub.join("profiles/perso/vault/vault.key"), hub.join("profiles/jardin/vault/vault.key"),
                 hub.join("vault/vault.key")] {
        assert!(kept.is_file(), "erased with no rebuild: {} ({report:?})", kept.display());
    }
    assert!(h.vault_folders_on_disk().is_empty(), "the agents in memory: none left");
}

#[tokio::test]
async fn copies_no_agent_uses_on_a_place_of_the_offer_are_erased_and_memory_tidied() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    // a deleted agent's copy in its old space, the hub's default profile's, a moved agent's left in the hub
    let old = h.root.join("data/spaces/ancien/profiles/ancien/vault");
    let hub = h.root.join("hub");
    for folder in [old.clone(), hub.join("vault"), hub.join("profiles/vie/vault")] {
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("vault.key"), "stray-key").unwrap();
    }
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    for gone in [old, hub.join("vault"), hub.join("profiles/vie/vault")] {
        assert!(!gone.exists(), "{}", gone.display());
    }
    // memory: a folder no link points to (a rebuild cut off) goes at the next pass
    let root = bipbridge::hermes_copies::root(&h.place.data, false).unwrap();
    let extra = bipbridge::hermes_copies::new_folder(&root, "vie").unwrap();
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    h.vault.copies_ready_all(&h.config).await;
    assert!(!extra.exists());
    assert!(std::fs::read_link(h.link("vie")).unwrap().is_dir());
}

#[tokio::test]
async fn an_item_hermes_refuses_fails_the_agent_while_a_disk_copy_is_at_stake_else_the_rest_is_rebuilt() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    let copy = std::fs::read(h.link("vie").join("vault.key")).unwrap();
    h.hermes.refused.lock().unwrap().insert("Impots".into());
    // its copy on the disk still works: all or nothing, the disk's kept
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()], "{report:?}");
    assert_eq!(std::fs::read(h.link("vie").join("vault.key")).unwrap(), copy);
    assert!(report.rebuilt.contains(&"budget".to_string()));
    // no copy on the disk any more (refused only after a server restart): the rest rebuilt, that one not given
    h.hermes.refused.lock().unwrap().clear();
    h.vault.copies_ready_all(&h.config).await;
    h.reboot();
    assert!(sealed_keys::try_open(&h.place, &Service(h.share.clone())).await);
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    h.hermes.refused.lock().unwrap().insert("Impots".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(h.copy("vie").keys().cloned().collect::<Vec<_>>(), vec!["SNCF".to_string()]);
    assert_eq!(h.copy("vie"), h.handles("vie"), "Impots no longer Vie's (the app shows it not given)");
    assert!(bipbridge::hermes_copies::failed(&h.place.data).is_empty());
}

#[tokio::test]
async fn an_agent_whose_copy_fails_is_shown_in_health_and_not_retried_at_every_give() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    h.hermes.failing.lock().unwrap().insert("vie".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()]);
    assert_eq!(bipbridge::hermes_copies::failed(&h.place.data), vec!["vie".to_string()]);
    let health = sealed_keys::health_fields(&h.config);
    assert_eq!(health.get("vault_copies_not_ready"), Some(&json!(1)), "{health:?}");
    // a give to another agent meanwhile: Vie's rebuild not run again under the lock
    let tries = h.hermes.ops("add_many").iter().filter(|(a, _)| a == "vie").count();
    h.give("Ameli", "ameli.fr", &["budget"]).await;
    assert_eq!(h.hermes.ops("add_many").iter().filter(|(a, _)| a == "vie").count(), tries);
    // mended, at the next try (the bridge's own, in the background): gone from /health
    h.hermes.failing.lock().unwrap().clear();
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!sealed_keys::health_fields(&h.config).contains_key("vault_copies_not_ready"));
}

#[tokio::test]
async fn an_agent_of_the_hub_has_its_copy_in_memory_too() {
    let _guard = SERIAL.lock().await;
    let h = fixture_with(|dict, root| {
        let home = root.join("hub").join("profiles").join("maison");
        std::fs::create_dir_all(&home).unwrap();
        dict["agents"]["maison"] = json!({"hermes_url": "http://127.0.0.1:9/p/maison", "hermes_key": "m".repeat(32),
            "isolated": true, "sandboxed": true, "python": "/nowhere/python", "hermes_home": home.to_string_lossy()});
    });
    h.give("Free", "free.fr", &["maison"]).await;
    let link = h.root.join("hub/profiles/maison/vault");
    assert!(link.join("vault.key").is_file());
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty() && report.rebuilt.contains(&"maison".to_string()), "{report:?}");
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert!(std::fs::read_link(&link).unwrap().starts_with(h.runtime.join("bipagents")));
    assert!(left_on_disk(&h.root.join("hub")).is_empty());
}

#[tokio::test]
async fn an_agent_moved_into_another_space_gets_its_copy_rebuilt_there() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    h.vault.copies_ready_all(&h.config).await;
    let before = std::fs::read_link(h.link("vie")).unwrap();
    // its space moved (spaces.rs copies the profile without its vault: NOT_COPIED), its record follows
    let space = h.root.join("data").join("spaces").join("vie-2");
    let home = space.join("profiles").join("vie");
    bipbridge::spaces::copy_profile_except(&h.home("vie"), &home, &bipbridge::hermes_copies::NOT_COPIED).unwrap();
    assert!(std::fs::symlink_metadata(home.join("vault")).is_err());
    let mut moved = h.config.agents["vie"].clone();
    moved.space = Some(space.to_string_lossy().into_owned());
    moved.hermes_home = Some(home.to_string_lossy().into_owned());
    let mut config = h.config.clone();
    config.agents.insert("vie".into(), moved.clone());
    h.vault.copies_ready_for(&config, &moved).await.unwrap();
    let link = home.join("vault");
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    let now = std::fs::read_link(&link).unwrap();
    assert_ne!(now, before);
    let items: BTreeMap<String, String> = store_of(&link)
        .iter()
        .map(|i| (i["label"].as_str().unwrap().to_string(), i["id"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(items, h.handles("vie"));
    // the old folder in memory tidied at the next pass
    std::fs::remove_dir_all(h.root.join("data").join("spaces").join("vie")).unwrap();
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let report = h.vault.copies_ready_all(&config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!before.exists());
}

// -- second review, 17/11 (the reviewer's, not pushed) ------------------------------------------------------------------

#[tokio::test]
async fn pass2_a_person_s_existing_hermes_hub_is_never_erased() {
    let _guard = SERIAL.lock().await;
    let h = fixture_with(|_, root| {
        let hub = root.join("hub");
        std::fs::create_dir_all(hub.join("profiles").join("jardin").join("vault")).unwrap();
        std::fs::write(hub.join("profiles").join("jardin").join("vault").join("vault.key"), "jardin-key").unwrap();
        std::fs::create_dir_all(hub.join("vault")).unwrap();
        std::fs::write(hub.join("vault").join("vault.key"), "default-key").unwrap();
    });
    let conf = h.root.join("conf");
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(conf.join("existing-hermes.json"), r#"{"preexisting": true}"#).unwrap();
    std::fs::write(conf.join("bridge.toml"), "").unwrap();
    let mut config = h.config.clone();
    config.source_path = Some(conf.join("bridge.toml").to_string_lossy().into_owned());
    given(&h).await;
    h.enrol().await;
    let report = h.vault.copies_ready_all(&config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let hub = h.root.join("hub");
    for kept in [hub.join("profiles/jardin/vault/vault.key"), hub.join("vault/vault.key")] {
        assert!(kept.is_file(), "the person's own Hermes erased: {}", kept.display());
    }
}

#[tokio::test]
async fn pass2_an_agent_removed_while_its_copy_fails_is_not_counted_in_health_for_ever() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    h.hermes.failing.lock().unwrap().insert("vie".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()]);
    // Vie deleted: the next pass has nothing failing (the retry loop stops)
    let mut config = h.config.clone();
    config.agents.retain(|name, _| name != "vie");
    let report = h.vault.copies_ready_all(&config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!sealed_keys::health_fields(&config).contains_key("vault_copies_not_ready"),
        "a deleted agent still counted: {:?}", bipbridge::hermes_copies::failed(&h.place.data));
}

#[tokio::test]
async fn pass2_a_disk_copy_set_aside_by_a_cut_off_swap_is_still_at_stake_for_a_refused_item() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    // a put back that failed (or the bridge stopped between the two renames of `point`): Vie's disk copy set aside,
    // no link; then its Hermes refuses one item
    std::fs::rename(h.link("vie"), h.home("vie").join(".vault-disk-0badc0de")).unwrap();
    h.hermes.refused.lock().unwrap().insert("Impots".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.contains(&"vie".to_string()) || h.handles("vie").contains_key("Impots"),
        "Impots taken from Vie and the disk copy holding it erased: {report:?} {:?}", h.handles("vie"));
}

#[tokio::test]
async fn pass2_an_item_only_the_agent_s_hermes_holds_is_not_erased_without_a_trace() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    // one its Hermes saved itself (Hermes' browser_vault save_login; or a Hermes alone sent to the place, to_place.py)
    let mut items = store_of(&h.link("vie"));
    // (as Hermes v0.23.1 keeps a login: its identifier in the metadata, only the password secret)
    items.push(json!({"id": "vault_0123456789ab", "kind": "login", "label": "Perso", "origin": "https://perso.example",
                      "identifier_type": "email", "identifier": "alex@perso.example",
                      "secret": {"password": "only-in-hermes"}}));
    write_store(&h.link("vie"), &items).unwrap();
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let in_copy = h.copy("vie").contains_key("Perso");
    let in_master = h.records().iter().any(|r| r.get("label").and_then(Value::as_str) == Some("Perso"));
    let on_disk = h.files_on_disk().iter().any(|f| {
        std::fs::read(f).is_ok_and(|d| d.windows(14).any(|w| w == b"only-in-hermes"))
    });
    assert!(in_copy || in_master || on_disk, "the person's own saved login erased, nowhere left: {report:?}");
}

// -- second review: the fixes' own tests -----------------------------------------------------------------------------

#[tokio::test]
async fn a_hermes_sent_to_the_place_keeps_the_logins_it_saved_itself_in_the_vault() {
    let _guard = SERIAL.lock().await;
    // to_place.py « un Hermes seul »: the person's own profile becomes an agent of the place, its vault the logins it
    // saved itself (none in the bridge's master copy); the place stays enrolled
    let h = fixture_with(|dict, root| {
        let home = root.join("hub").join("profiles").join("hermes");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.yaml"), "model:\n  default: x\n").unwrap();
        write_store(&home.join("vault"), &[
            json!({"id": "vault_aaaaaaaaaaaa", "kind": "login", "label": "Forum", "origin": "https://forum.example",
                   "identifier_type": "username", "identifier": "alex", "secret": {"password": "forum-pw-only-hermes"}}),
            json!({"id": "vault_bbbbbbbbbbbb", "kind": "payment", "label": "Carte perso", "origin": null,
                   "secret": {"card_number": "4111111111111111", "cardholder_name": "Alex", "exp_month": "12",
                              "exp_year": "2030", "cvc": "123"}}),
        ]).unwrap();
        dict["agents"]["hermes"] = json!({"hermes_url": "http://127.0.0.1:9/p/hermes", "hermes_key": "h".repeat(32),
            "isolated": true, "sandboxed": true, "python": "/nowhere/python", "hermes_home": home.to_string_lossy()});
    });
    given(&h).await;
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty() && report.rebuilt.contains(&"hermes".to_string()), "{report:?}");
    let link = h.root.join("hub/profiles/hermes/vault");
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    let handles = h.handles("hermes");
    // its card is not taken (Alex, 17/11: never a Hermes payment item, its security code with it): the person is told
    // once to add it in the Coffre; its disk copy goes with the rest
    assert_eq!(handles.keys().cloned().collect::<Vec<_>>(), vec!["Forum".to_string()]);
    assert!(!h.records().iter().any(|r| r["kind"] == json!("payment")));
    let notices = h.vault.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].text.contains("Carte perso") && notices[0].text.contains("Coffre"), "{notices:?}");
    let copy: BTreeMap<String, String> = store_of(&link)
        .iter()
        .map(|i| (i["label"].as_str().unwrap().to_string(), i["id"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(copy, handles, "in its copy in memory, under the bridge's handles");
    let forum = h.records().into_iter().find(|r| r["label"] == json!("Forum")).unwrap();
    assert_eq!(forum["identifier"], json!("alex"));
    assert_eq!(forum["secret"]["password"], json!("forum-pw-only-hermes"));
    assert!(left_on_disk(&h.root.join("hub")).is_empty(), "taken into the vault, then erased from the disk");
}

#[tokio::test]
async fn an_item_only_hermes_holds_that_the_vault_cannot_take_keeps_its_disk_copy() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let mut items = store_of(&h.link("vie"));
    items.push(json!({"id": "vault_cccccccccccc", "kind": "login", "label": "Sans site", "origin": null,
                      "identifier_type": "email", "identifier": "a@b.c", "secret": {"password": "no-site-pw"}}));
    write_store(&h.link("vie"), &items).unwrap();
    let copy = std::fs::read(h.link("vie").join("vault.json.enc")).unwrap();
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()], "{report:?}");
    assert_eq!(std::fs::read(h.link("vie").join("vault.json.enc")).unwrap(), copy, "kept as it was");
}

#[tokio::test]
async fn a_profile_left_without_its_agent_keeps_the_logins_its_hermes_holds() {
    let _guard = SERIAL.lock().await;
    // an imported Hermes profile the bridge could not adopt (to_place.py `adopt` failed): no agent, a live profile
    let h = fixture_with(|_, root| {
        let home = root.join("hub").join("profiles").join("hermes");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.yaml"), "model:\n  default: x\n").unwrap();
        write_store(&home.join("vault"), &[json!({"id": "vault_dddddddddddd", "kind": "login", "label": "Forum",
            "origin": "https://forum.example", "identifier": "alex", "secret": {"password": "forum-pw"}})]).unwrap();
    });
    given(&h).await;
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(h.root.join("hub/profiles/hermes/vault/vault.json.enc").is_file(), "never erased with no agent to take it");
}

#[tokio::test]
async fn an_item_hermes_refuses_is_marked_refused_then_given_again_once_it_accepts() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    h.vault.copies_ready_all(&h.config).await;
    h.reboot();
    assert!(sealed_keys::try_open(&h.place, &Service(h.share.clone())).await);
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    h.hermes.refused.lock().unwrap().insert("Impots".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let impots = h.vault.items().into_iter().find(|i| i.label == "Impots").unwrap();
    assert!(!impots.agents.contains_key("vie"));
    let line = impots.refused.get("vie").cloned().unwrap_or_default();
    assert!(line.starts_with("Pas donnée à ") && line.contains("refuse"), "{line:?}");
    // its Hermes accepts it again: given back at the next pass (the next start)
    h.hermes.refused.lock().unwrap().clear();
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty() && report.rebuilt.contains(&"vie".to_string()), "{report:?}");
    let impots = h.vault.items().into_iter().find(|i| i.label == "Impots").unwrap();
    assert!(impots.agents.contains_key("vie") && impots.refused.is_empty(), "{:?}", impots.refused);
    assert_eq!(h.copy("vie"), h.handles("vie"));
}

// -- third review, 17/11 (the reviewer's, not pushed) -------------------------------------------------------------------

#[tokio::test]
async fn pass3_a_copy_set_aside_after_its_handles_were_written_is_not_taken_in_twice() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let disk: Vec<(String, Vec<u8>)> = ["vault.key", "vault.json.enc"]
        .iter()
        .map(|f| (f.to_string(), std::fs::read(h.link("vie").join(f)).unwrap()))
        .collect();
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let before = h.records().len();
    // the bridge stopped after the handles were written, before the erase (or the erase failed): the disk copy, with
    // the ids of before, still set aside beside the link
    let aside = h.home("vie").join(".vault-disk-0badc0de");
    std::fs::create_dir_all(&aside).unwrap();
    for (name, data) in &disk {
        std::fs::write(aside.join(name), data).unwrap();
    }
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let labels: Vec<String> = h.records().iter().map(|r| r["label"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(labels.len(), before, "every item of Vie taken into the vault a second time: {labels:?}");
}

#[tokio::test]
async fn pass3_a_login_taken_back_while_the_agent_s_copy_fails_is_not_given_again() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    // Vie's Hermes failing a while: its copy on the disk kept (all or nothing)
    h.hermes.failing.lock().unwrap().insert("vie".into());
    let report = h.vault.copies_ready_all(&h.config).await;
    assert_eq!(report.failed, vec!["vie".to_string()]);
    // meanwhile the person takes SNCF back from Vie (Budget keeps it)
    let sncf = h.vault.items().into_iter().find(|i| i.label == "SNCF").unwrap();
    let body = json!({"agents": ["budget"]});
    h.vault.update(&h.config, &sncf.id, body.as_object().unwrap()).await.unwrap();
    assert!(!h.handles("vie").contains_key("SNCF"));
    // Vie's Hermes mended: its copy rebuilt
    h.hermes.failing.lock().unwrap().clear();
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(!h.handles("vie").contains_key("SNCF"), "taken back, then given again: {:?} (records: {})",
        h.handles("vie"), h.records().len());
}

#[tokio::test]
async fn pass3_an_entry_the_person_takes_from_a_refused_agent_is_not_given_again_later() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    h.enrol().await;
    h.vault.copies_ready_all(&h.config).await;
    h.reboot();
    assert!(sealed_keys::try_open(&h.place, &Service(h.share.clone())).await);
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    h.hermes.refused.lock().unwrap().insert("SNCF".into());
    h.vault.copies_ready_all(&h.config).await;
    let sncf = h.vault.items().into_iter().find(|i| i.label == "SNCF").unwrap();
    assert!(sncf.refused.contains_key("vie"));
    // the person says: SNCF for Budget only
    let body = json!({"agents": ["budget"]});
    h.vault.update(&h.config, &sncf.id, body.as_object().unwrap()).await.unwrap();
    h.hermes.refused.lock().unwrap().clear();
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    h.vault.copies_ready_all(&h.config).await;
    assert!(!h.handles("vie").contains_key("SNCF"), "given to Vie again against the person's choice");
}

#[tokio::test]
async fn pass3_a_hermes_payment_item_is_not_kept_with_its_security_code() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let mut items = store_of(&h.link("vie"));
    items.push(json!({"id": "vault_eeeeeeeeeeee", "kind": "payment", "label": "Carte", "origin": null,
                      "secret": {"card_number": "4111111111111111", "cardholder_name": "Alex", "exp_month": "12",
                                 "exp_year": "2030", "cvc": "123"}}));
    write_store(&h.link("vie"), &items).unwrap();
    h.enrol().await;
    h.vault.copies_ready_all(&h.config).await;
    let kept: Vec<Dict> = h.records().into_iter().filter(|r| r["kind"] == json!("payment")).collect();
    assert!(kept.is_empty(), "a payment item with its cvc kept and given back to the agent (cards_allowed: never)");
}

#[tokio::test]
async fn pass3_a_contact_the_vault_cannot_take_does_not_put_its_details_in_the_error() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let mut items = store_of(&h.link("vie"));
    items.push(json!({"id": "vault_ffffffffffff", "kind": "address", "label": "Maison", "origin": null,
                      "secret": {"full_name": "Alex", "city": "Lyon", "phone": "06 12 34 56 78 poste 4"}}));
    write_store(&h.link("vie"), &items).unwrap();
    h.enrol().await;
    let agent = h.config.agents["vie"].clone();
    let err = h.vault.copies_ready_for(&h.config, &agent).await.unwrap_err();
    let text = format!("{err:#}");
    assert!(!text.contains("06 12 34"), "the contact's phone in the error (logged): {text}");
}

#[tokio::test]
async fn pass3_a_set_aside_copy_whose_erase_was_cut_off_does_not_fail_the_agent_for_ever() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let enc = std::fs::read(h.link("vie").join("vault.json.enc")).unwrap();
    h.enrol().await;
    assert!(h.vault.copies_ready_all(&h.config).await.failed.is_empty());
    // the bridge stopped while erasing the set-aside copy: its vault.key wiped, its vault.json.enc not yet
    let aside = h.home("vie").join(".vault-disk-0badc0de");
    std::fs::create_dir_all(&aside).unwrap();
    std::fs::write(aside.join("vault.json.enc"), &enc).unwrap();
    for _ in 0..2 {
        bipbridge::hermes_copies::forget_checked(&h.place.data); // each start
        let report = h.vault.copies_ready_all(&h.config).await;
        assert!(report.failed.is_empty(), "Vie fails at every start, its set-aside never erased: {report:?}");
    }
    assert!(!aside.exists());
}

// -- third review: the fixes' own tests ------------------------------------------------------------------------------

#[tokio::test]
async fn a_card_notice_is_said_once_and_goes_when_the_app_has_shown_it() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    let mut items = store_of(&h.link("vie"));
    items.push(json!({"id": "vault_eeeeeeeeeeee", "kind": "payment", "label": "Visa", "origin": null,
                      "secret": {"card_number": "4111111111111111", "cvc": "123"}}));
    write_store(&h.link("vie"), &items).unwrap();
    h.enrol().await;
    h.vault.copies_ready_all(&h.config).await;
    let notices = h.vault.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].text, "Ta carte « Visa » n'a pas été reprise : ajoute-la dans le Coffre.");
    for file in h.files_on_disk() {
        let data = std::fs::read(&file).unwrap();
        assert!(!data.windows(16).any(|w| w == b"4111111111111111"), "{}", file.display());
    }
    // the next passes say nothing more; the app's « vu » takes it away
    bipbridge::hermes_copies::forget_checked(&h.place.data);
    h.vault.copies_ready_all(&h.config).await;
    assert_eq!(h.vault.notices().len(), 1);
    assert!(h.vault.notice_seen(&notices[0].id));
    assert!(h.vault.notices().is_empty());
}

#[tokio::test]
async fn a_login_hermes_holds_twice_is_one_entry_and_another_password_never_wins_over_the_person_s() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    given(&h).await;
    // the same SNCF login saved by Hermes itself again (same password: the same entry), and Impots with another
    // password (the person's in the vault stays the one typed)
    let mut items = store_of(&h.link("vie"));
    items.push(json!({"id": "vault_111111111111", "kind": "login", "label": "SNCF bis", "origin": "https://sncf-connect.com",
                      "identifier_type": "email", "identifier": "alex@example.org",
                      "created_at": "2030-01-02T03:04:05.000000+00:00",
                      "secret": {"password": "pw-SNCF-0123456789"}}));
    items.push(json!({"id": "vault_222222222222", "kind": "login", "label": "Impots", "origin": "https://impots.gouv.fr",
                      "identifier_type": "email", "identifier": "alex@example.org",
                      "created_at": "2020-01-02T03:04:05.000000+00:00",
                      "secret": {"password": "another-impots-password"}}));
    write_store(&h.link("vie"), &items).unwrap();
    h.enrol().await;
    let report = h.vault.copies_ready_all(&h.config).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let records = h.records();
    let sncf: Vec<&Dict> = records.iter().filter(|r| r["origin"] == json!("https://sncf-connect.com")).collect();
    assert_eq!(sncf.len(), 1, "one entry");
    assert_eq!(sncf[0]["updated_at"], json!("2030-01-02T03:04:05+00:00"), "Hermes' date kept, the newer");
    let impots: Vec<&Dict> = records.iter().filter(|r| r["origin"] == json!("https://impots.gouv.fr")).collect();
    assert_eq!(impots.len(), 2, "never lose either password");
    let original = impots.iter().find(|r| r.get("duplicate_of").is_none()).unwrap();
    let copy = impots.iter().find(|r| r.get("duplicate_of").is_some()).unwrap();
    assert_eq!(copy["duplicate_of"], original["id"]);
    assert_eq!(copy["created_at"], json!("2020-01-02T03:04:05+00:00"), "Hermes' own date");
    // vault_login fills the person's own
    let items = h.vault.items();
    let given: Vec<bipbridge::vault::Item> = items.into_iter().filter(|i| i.agents.contains_key("vie")).collect();
    match bipbridge::vault_match::choose(&given, "https://impots.gouv.fr/connexion", None) {
        bipbridge::vault_match::Choice::One(item) => assert_eq!(Value::String(item.id.clone()), original["id"]),
        _ => panic!("vault_login can't choose between the person's login and Hermes' copy"),
    }
}
