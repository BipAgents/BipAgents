//! `bipbridge vault-keys export` (design note « Clé du coffre », lot 3: `bipagents export` of an enrolled place): the
//! keys the place key opens, given only while the vault is open (from this process's memory or the tmpfs, never by
//! asking the key service), refused when the place is not enrolled or closed. Written back as today's layout (each
//! key beside its store, no keys.sealed.json), as the installer's import does, the stores read exactly as before.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bipbridge::connections::{self, Connection, Stored, Tokens};
use bipbridge::mailbox::Mailboxes;
use bipbridge::sealed_keys::{self, Place};
use bipbridge::space::{self, KeyService, ShareError, ShareSource};
use bipbridge::vault::Vault;
use serde_json::{json, Value};
use zeroize::Zeroizing;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The key service: always the same share; counts the requests.
struct Service {
    share: Vec<u8>,
    asked: Mutex<u32>,
}

#[async_trait::async_trait]
impl ShareSource for Service {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        *self.asked.lock().unwrap() += 1;
        Ok(Zeroizing::new(self.share.clone()))
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    place: Place,
    runtime: PathBuf,
}

impl Fixture {
    fn vault(&self) -> Vault {
        Vault::new(&self.place.data.join("vault").to_string_lossy())
    }

    fn mail(&self) -> Mailboxes {
        Mailboxes::new(&self.place.data.join("mail"))
    }

    fn connections(&self) -> connections::Store {
        connections::Store::new(&self.place.data.join("connections"))
    }

    fn contents(&self) -> (Vec<Value>, Value, Value) {
        let items = self.vault().try_items().unwrap().iter().map(|i| serde_json::to_value(i).unwrap()).collect();
        (
            items,
            serde_json::to_value(self.mail().read().unwrap()).unwrap(),
            serde_json::to_value(self.connections().read().unwrap()).unwrap(),
        )
    }
}

fn fixture() -> Fixture {
    std::env::set_var(space::FAKE_ENV, "1");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let runtime = root.join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    let place = Place::new(root.join("data"), root.join("space"));
    sealed_keys::forget_memory(&place.data);
    let vault = place.data.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let key = fernet::Fernet::generate_key();
    std::fs::write(vault.join("vault.key"), &key).unwrap();
    let records = json!([{"id": "item_aaaaaaaaaaaa", "kind": "login", "label": "Impôts",
        "origin": "https://impots.example", "identifier_type": "email", "identifier": "alex@example.test",
        "has_otp": false, "secret": {"password": "fake-Pw-0001-impots"}, "agents": {},
        "created_at": "2026-11-01T10:00:00Z", "updated_at": "2026-11-01T10:00:00Z"}]);
    let token = fernet::Fernet::new(&key).unwrap().encrypt(&serde_json::to_vec(&records).unwrap());
    std::fs::write(vault.join("vault.json.enc"), token).unwrap();
    let h = Fixture { _dir: dir, place, runtime };
    let account = json!({"id": "mail_cccccccccccc", "provider": "imap", "label": "Perso",
        "username": "alex@example.test", "password": "fake-imap-pw-0002", "host": "127.0.0.1", "port": 9,
        "agents": ["wellness"]});
    h.mail().write(&[account.as_object().unwrap().clone()]).unwrap();
    let mut stored = Stored::default();
    stored.connections.insert(
        "notion".into(),
        Connection {
            service: "notion".into(),
            tokens: Tokens {
                access_token: "fake-access-0003".into(),
                refresh_token: Some("fake-refresh-0004".into()),
                scope: None,
                expires_at: Some(2_000_000_000.0),
            },
            agents: vec!["wellness".into()],
            connected_at: 1_762_000_000.0,
            state: "connected".into(),
            ..Connection::default()
        },
    );
    h.connections().write(&stored).unwrap();
    h
}

fn key_service() -> KeyService {
    KeyService { url: "https://keys.test".into(), token: "t".repeat(40), grant: None, signature: None }
}

#[tokio::test]
async fn the_export_gives_the_keys_the_place_key_opens_only_while_the_vault_is_open() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let before = h.contents();
    // not enrolled: nothing to give (the export takes the key files as they are)
    let refused = sealed_keys::export_keys(&h.place).unwrap_err();
    assert!(format!("{refused:#}").contains("not sealed"), "{refused:#}");
    let service = Arc::new(Service { share: space::new_share().to_vec(), asked: Mutex::new(0) });
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    let asked = *service.asked.lock().unwrap();
    // open (a new process: the place key from the tmpfs only)
    sealed_keys::forget_memory(&h.place.data);
    let exported = sealed_keys::export_keys(&h.place).unwrap();
    assert_eq!(*service.asked.lock().unwrap(), asked, "never asks the key service");
    assert_eq!(exported["v"], 1);
    let keys = exported["keys"].as_object().unwrap();
    assert_eq!(keys.len(), 3);
    for (store, key_file) in [("vault", "vault.key"), ("mail", "mail.key"), ("connections", "connections.key")] {
        let key = keys[store].as_str().unwrap();
        assert!(fernet::Fernet::new(key).is_some(), "{store}: a Fernet key");
        assert!(!h.place.data.join(store).join(key_file).exists(), "{store}: still sealed on the disk");
    }
    assert!(exported.get("sk").is_none(), "the place key itself never leaves");
    // closed (the server restarted: the tmpfs empty): refused, nothing asked
    let _ = std::fs::remove_dir_all(h.runtime.join("bipagents"));
    sealed_keys::forget_memory(&h.place.data);
    let refused = sealed_keys::export_keys(&h.place).unwrap_err();
    assert!(format!("{refused:#}").contains("closed"), "{refused:#}");
    assert_eq!(*service.asked.lock().unwrap(), asked);
    // today's layout written back from the export (what the import does): the stores read as before
    for (store, key_file) in [("vault", "vault.key"), ("mail", "mail.key"), ("connections", "connections.key")] {
        std::fs::write(h.place.data.join(store).join(key_file), keys[store].as_str().unwrap()).unwrap();
    }
    std::fs::remove_file(h.place.data.join(sealed_keys::SEALED_FILE)).unwrap();
    sealed_keys::forget_memory(&h.place.data);
    assert_eq!(h.contents(), before);
}

#[test]
fn the_command_line_knows_export() {
    let usage = sealed_keys::usage();
    assert!(usage.contains("export"), "{usage}");
}
