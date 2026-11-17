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
    for agent in AGENTS {
        let space = root.join("data").join("spaces").join(agent);
        let home = space.join("profiles").join(agent);
        std::fs::create_dir_all(&home).unwrap();
        dict["agents"][agent] = json!({"hermes_url": "http://127.0.0.1:9/p", "hermes_key": "k".repeat(32),
            "isolated": true, "sandboxed": true, "space": space.to_string_lossy(), "python": "/nowhere/python",
            "hermes_home": home.to_string_lossy()});
    }
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
    assert_eq!(h.hermes.count(), runs + 2, "one run per agent holding items, none for Coach");
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
    let kept = dir.path().join("kept");
    bipbridge::spaces::copy_profile_except(&source, &kept, &[]).unwrap();
    assert!(kept.join("vault").join("vault.key").is_file(), "today: copied with the rest");
    let moved = dir.path().join("moved");
    bipbridge::spaces::copy_profile_except(&source, &moved, &[bipbridge::hermes_copies::LINK]).unwrap();
    assert!(!moved.join("vault").exists(), "in memory: never copied");
    assert!(moved.join("SOUL.md").is_file() && moved.join("skills").join("vault").is_dir(), "only the profile's own");
}
