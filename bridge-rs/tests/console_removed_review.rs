//! Security review of remove-web-console (28/11): what tests/console_removed.rs leaves out.

mod common;

use common::{auth, config, TestApp, Urls};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

fn data_of(dir: &std::path::Path) -> PathBuf {
    PathBuf::from(config(dir, &Urls::default()).data_dir())
}

/// Every method, with or without a key, with an old console's cookie: the plain 404, no cookie, no redirect.
#[tokio::test]
async fn every_method_on_the_console_paths_is_a_plain_404() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let paths = [
        "/console", "/console/", "/console/static", "/console/static/", "/console/static/app.js",
        "/console/static/vendor/preact-htm.js", "/console/api/me", "/console/api/login", "/console/index.html",
        "/v1/console", "/v1/console/", "/v1/console/devices", "/v1/console/devices/", "/v1/console/devices/a1b2c3",
        "/v1/console/pairings/123456", "/v1/console/pairings/123456/",
    ];
    for path in paths {
        for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            let console = [("x-bip-console", "1"), ("cookie", "bip_console=whatever")];
            let keyed = [(k, v.as_str())];
            let keyed_console = [(k, v.as_str()), ("x-bip-console", "1"), ("cookie", "bip_console=whatever")];
            for headers in [&[][..], &keyed[..], &console[..], &keyed_console[..]] {
                let said = app.request(method, path, headers, None).await;
                assert_eq!(said.status.as_u16(), 404, "{method} {path} {headers:?}: {}", said.text());
                if method != "HEAD" {
                    assert_eq!(said.json(), json!({"detail": "Not Found"}), "{method} {path}");
                }
                assert!(said.header("set-cookie").is_none() && said.header("location").is_none(), "{method} {path}");
                assert!(said.header("www-authenticate").is_none(), "{method} {path}: an auth hint");
            }
        }
    }
    // never counted as wrong keys either
    assert_eq!(app.get("/v1/agents", &[(k, &v)]).await.status, 200);
    app.shutdown().await;
}

/// A link where console.json was: the link goes, never what it points to. A folder of that name goes too.
#[tokio::test]
async fn links_go_their_targets_stay() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let data = data_of(dir.path());
    std::fs::create_dir_all(&data).unwrap();
    let target = outside.path().join("precious.json");
    std::fs::write(&target, r#"{"sessions": {"a": {}}}"#).unwrap();
    let target_dir = outside.path().join("precious-dir");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(target_dir.join("inside"), "keep").unwrap();
    std::os::unix::fs::symlink(&target, data.join("console.json")).unwrap();
    std::os::unix::fs::symlink(&target_dir, data.join("console.json.tmp")).unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    assert!(std::fs::symlink_metadata(data.join("console.json")).is_err(), "the link deleted");
    assert!(std::fs::symlink_metadata(data.join("console.json.tmp")).is_err(), "the link deleted");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), r#"{"sessions": {"a": {}}}"#, "its target kept");
    assert_eq!(std::fs::read_to_string(target_dir.join("inside")).unwrap(), "keep", "the folder it points to kept");
    app.shutdown().await;

    // a dangling link
    std::os::unix::fs::symlink(outside.path().join("nothing"), data.join("console.json")).unwrap();
    assert!(bipbipbridge_at_start(dir.path()));
    assert!(std::fs::symlink_metadata(data.join("console.json")).is_err());
}

fn bipbipbridge_at_start(dir: &std::path::Path) -> bool {
    bipbridge::console_removed::at_start(&config(dir, &Urls::default()))
}

/// A file that can't be deleted (a folder of that name with a read-only folder inside): logged, the start goes on,
/// the keys open as before; the next start, once it can, deletes it.
#[tokio::test]
async fn a_failed_deletion_never_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_of(dir.path());
    let stuck = data.join("console.json").join("inner");
    std::fs::create_dir_all(&stuck).unwrap();
    std::fs::write(stuck.join("f"), "x").unwrap();
    std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o500)).unwrap();
    let root = unsafe { libc_geteuid() } == 0;
    if !root {
        assert!(!bipbipbridge_at_start(dir.path()), "a failure is said");
    }
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    assert_eq!(app.get("/v1/agents", &[(k, &v)]).await.status, 200, "the bridge serves all the same");
    app.shutdown().await;
    std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(bipbipbridge_at_start(dir.path()));
    assert!(std::fs::symlink_metadata(data.join("console.json")).is_err());
}

extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

/// Red: a FIFO (or a link to one) where console.json was. `at_start` reads it before deleting it (to count the
/// sessions), and `read_to_string` follows links and blocks on a FIFO with no writer: the bridge never starts.
#[test]
fn a_fifo_where_the_file_was_does_not_hang_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_of(dir.path());
    std::fs::create_dir_all(&data).unwrap();
    let fifo = data.join("console.json");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let (tx, rx) = std::sync::mpsc::channel();
    let path = dir.path().to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(bipbipbridge_at_start(&path));
    });
    let answered = rx.recv_timeout(Duration::from_secs(5));
    if answered.is_err() {
        // release the blocked reader (a writer opens, then closes: EOF)
        drop(std::fs::OpenOptions::new().write(true).open(&fifo));
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
    assert!(answered.is_ok(), "console_removed::at_start blocked on a FIFO named console.json: the start hangs");
    assert!(std::fs::symlink_metadata(&fifo).is_err());
}
