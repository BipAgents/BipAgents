//! tests/test_browser_mcp.py: the MCP proxy that answers Hermes' handshake from its cache, starts the agent's browser
//! at the first tool call and stops it once nobody uses it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bipbridge::browser_mcp::{self, Proxy};
use bipbridge::Dict;
use serde_json::{json, Value};

const FAKE_SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "spawn_browser", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "no"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

fn init() -> Value {
    json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
           "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes"}}})
}

/// What the proxy writes to Hermes.
#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Out {
    fn messages(&self) -> Vec<Value> {
        let data = self.0.lock().unwrap().clone();
        String::from_utf8(data).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

fn python() -> String {
    ["python3", "/usr/bin/python3"]
        .iter()
        .find(|p| std::process::Command::new(p).arg("-V").output().is_ok())
        .unwrap()
        .to_string()
}

fn config(tmp: &Path) -> Dict {
    std::fs::write(tmp.join("server.py"), FAKE_SERVER).unwrap();
    let log = tmp.join("log").to_string_lossy().into_owned();
    let running = tmp.join("running").to_string_lossy().into_owned();
    let value = json!({
        "start": ["sh", "-c", format!("echo start >> {log}; touch {running}")],
        "ready": ["true"],
        "active": ["test", "-e", running],
        "server": [python(), tmp.join("server.py").to_string_lossy()],
        "stop": ["sh", "-c", format!("echo stop >> {log}; rm -f {running}")],
        "cache": tmp.join("cache.json").to_string_lossy(), "busy": tmp.join("busy").to_string_lossy(),
        "cdp_port": 19602, "idle": 600,
    });
    value.as_object().unwrap().clone()
}

fn ran(tmp: &Path) -> Vec<String> {
    std::fs::read_to_string(tmp.join("log"))
        .map(|t| t.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

fn ids(messages: &[Value]) -> Vec<i64> {
    messages.iter().map(|m| m["id"].as_i64().unwrap()).collect()
}

fn set_mtime(path: &PathBuf, ago: u64) {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(ago)).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn hermes_connects_without_starting_the_browser_once_it_was_seen() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    // The first time ever: started to learn what it serves, then answered as it said.
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    let first = out.messages();
    assert_eq!(ids(&first), vec![0, 1, 2]);
    assert_eq!(first[0]["result"]["serverInfo"]["name"], "stealth");
    assert_eq!(first[1]["result"]["tools"][0]["name"], "spawn_browser");
    assert_eq!(first[2]["result"]["content"][0]["text"], "did navigate");
    assert_eq!(ran(tmp), vec!["start"]);
    assert!(tmp.join("busy").exists());
    proxy.stop_child().await;

    // Its gateway restarted: the handshake and the tool list come from what it said, nothing starts.
    std::fs::remove_file(tmp.join("log")).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    let mut again = init();
    again["id"] = json!(10);
    proxy.handle(again).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 11, "method": "tools/list"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 12, "method": "ping"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 14, "method": "prompts/list"})).await; // never asked before: empty
    let messages = out.messages();
    assert_eq!(ids(&messages), vec![10, 11, 12, 14]);
    assert!(ran(tmp).is_empty() && !proxy.running());
    assert_eq!(messages[3]["result"], json!({"prompts": []}));
    assert_eq!(messages[1]["result"], first[1]["result"]);
    // The first tool call starts it.
    proxy
        .handle(json!({"jsonrpc": "2.0", "id": 13, "method": "tools/call", "params": {"name": "spawn_browser"}}))
        .await;
    assert_eq!(
        out.messages().last().unwrap(),
        &json!({"jsonrpc": "2.0", "id": 13, "result": {"content": [{"type": "text", "text": "did spawn_browser"}]}})
    );
    assert_eq!(ran(tmp), vec!["start"]);
    assert!(proxy.running());
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_once_nobody_uses_it() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    config.insert("idle".into(), json!(0.4));
    let proxy = Proxy::new(config, Box::new(Out::default()));
    proxy.handle(init()).await;
    let watcher = tokio::spawn(proxy.clone().watch_idle(0.05));
    // The person watching its desktop (the bridge touching `busy`) keeps it on.
    let busy = tmp.join("busy");
    for _ in 0..8 {
        set_mtime(&busy, 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(proxy.running() && !ran(tmp).contains(&"stop".to_string()));
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!proxy.running());
    assert_eq!(ran(tmp), vec!["start", "stop"]);
    // Started by someone else (the desktop) and left unused: stopped too.
    std::fs::write(tmp.join("running"), "").unwrap();
    set_mtime(&busy, 5);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ran(tmp), vec!["start", "stop", "stop"]);
    assert!(!tmp.join("running").exists());
    watcher.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_soon_after_the_run_that_used_it_ended_unless_used_again() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp); // idle 600 s, `ended` beside `busy`, after_run 480 s by default
    let proxy = Proxy::new(config.clone(), Box::new(Out::default()));
    assert_eq!(proxy.after_run, browser_mcp::AFTER_RUN_SECONDS as f64);
    assert_eq!(proxy.ended_path, tmp.join("ended"));
    let (busy, ended) = (tmp.join("busy"), tmp.join("ended"));
    // No run told its end: only the 600 s rule.
    set_mtime(&busy, 400);
    assert!(!proxy.done_with());
    // The run ended 7 minutes ago, its last call before: not yet (warm for the person's next message).
    set_mtime(&busy, 900);
    set_mtime(&ended, 420);
    assert!(!proxy.done_with());
    // 8 minutes: done with.
    set_mtime(&ended, 481);
    assert!(proxy.done_with());
    // Used since the end (a new call, the person watching or driving its desktop): the 600 s rule again.
    set_mtime(&busy, 100);
    assert!(!proxy.done_with());
    // `after_run` 0: never sooner.
    let mut never = config.clone();
    never.insert("after_run".into(), json!(0));
    set_mtime(&busy, 400);
    assert!(!Proxy::new(never, Box::new(Out::default())).done_with());

    // The watcher stops it, its idle time far from over.
    let mut quick = config.clone();
    quick.insert("after_run".into(), json!(0.3));
    let proxy = Proxy::new(quick, Box::new(Out::default()));
    proxy.handle(init()).await; // started once to learn its tools (touches `busy`)
    assert!(proxy.running());
    let watcher = tokio::spawn(proxy.clone().watch_idle(0.05));
    // a call under way keeps it on whatever the run's end says
    proxy.calls.store(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(20)).await;
    set_mtime(&busy, 2);
    set_mtime(&ended, 1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(proxy.running());
    proxy.calls.store(0, Ordering::SeqCst);
    set_mtime(&busy, 2);
    set_mtime(&ended, 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!proxy.running());
    assert_eq!(ran(tmp), vec!["start", "stop"]);
    watcher.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_call_or_a_devtools_connection_keeps_it_on() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let table = tmp.join("tcp");
    let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
    std::fs::write(
        &table,
        format!("{header}   0: 0100007F:B798 0100007F:4C92 01 00000000:00000000 00:00000000 00000000  1000 0 1\n"),
    )
    .unwrap();
    let tables = vec![table.to_string_lossy().into_owned()];
    assert!(browser_mcp::devtools_connected(Some(&json!(19602)), &tables));
    assert!(!browser_mcp::devtools_connected(Some(&json!(19604)), &tables));

    let mut config = config(tmp);
    config.insert("idle".into(), json!(0.3));
    let proxy = Proxy::new(config, Box::new(Out::default()));
    *proxy.tcp_tables.lock().unwrap() = vec![tmp.join("none").to_string_lossy().into_owned()];
    proxy.handle(init()).await;
    proxy.calls.store(1, Ordering::SeqCst); // a call under way
    set_mtime(&tmp.join("busy"), 60);
    let watcher = tokio::spawn(proxy.clone().watch_idle(0.05));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let age = SystemTime::now()
        .duration_since(std::fs::metadata(tmp.join("busy")).unwrap().modified().unwrap())
        .unwrap_or_default();
    assert!(proxy.running() && age < Duration::from_secs(1)); // touched for the others
    watcher.abort();
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_cannot_start_is_said_to_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    config.insert("start".into(), json!(["false"]));
    let out = Out::default();
    std::fs::write(tmp.join("cache.json"), json!({"initialize": {"serverInfo": {"name": "stealth"}}}).to_string())
        .unwrap();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "navigate"}})).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], 1);
    // said in the answer, never a JSON-RPC error (Hermes' breaker counts one: browser_mcp `could_not_start`)
    assert!(answer.get("error").is_none(), "{answer}");
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("could not start"), "{answer}");
    assert_eq!(proxy.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_agent_connects_from_what_another_agents_server_said_and_its_container_is_set_up_at_first_use() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let log = tmp.join("log").to_string_lossy().into_owned();
    let mut config = config(tmp);
    let shared = tmp.join("mcp-cache.json");
    let unit = tmp.join("units").join("bipagents-browser-new.service");
    config.insert("shared_cache".into(), json!(shared.to_string_lossy()));
    config.insert("unit".into(), json!({"path": unit.to_string_lossy(), "text": "[Unit]\n"}));
    config.insert("profile".into(), json!(tmp.join("profile").to_string_lossy()));
    config.insert("reload".into(), json!(["sh", "-c", format!("echo reload >> {log}")]));
    // Another agent's server, seen once: its answers kept for every agent.
    std::fs::write(
        &shared,
        json!({"initialize": {"serverInfo": {"name": "stealth"}}, "tools/list": {"tools": [{"name": "spawn_browser"}]}})
            .to_string(),
    )
    .unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    assert_eq!(out.messages()[0]["result"]["serverInfo"]["name"], "stealth");
    assert_eq!(out.messages()[1]["result"]["tools"][0]["name"], "spawn_browser");
    // Nothing set up nor started for an agent that never browses.
    assert!(ran(tmp).is_empty() && !unit.exists() && !tmp.join("profile").exists());
    // Its first tool call: its unit and profile written, systemd told, then started.
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "spawn_browser"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did spawn_browser");
    assert_eq!(ran(tmp), vec!["reload", "start"]);
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), "[Unit]\n");
    assert!(tmp.join("profile").is_dir());
    // What its own server said is now its own too.
    assert!(tmp.join("cache.json").exists());
    proxy.stop_child().await;
    // Started again later: set up once only.
    let proxy = Proxy::new(config, Box::new(Out::default()));
    proxy.handle(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(ran(tmp), vec!["reload", "start", "start"]);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_cold_start_is_waited_for_and_the_first_call_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    // Its screen shows up 2 s after the start (a loaded server): the call waits, it is not failed for a retry.
    let screen = tmp.join("screen").to_string_lossy().into_owned();
    config.insert("start".into(), json!(["sh", "-c", format!("(sleep 2; touch {screen}) >/dev/null 2>&1 &")]));
    config.insert("ready".into(), json!(["test", "-e", screen]));
    std::fs::write(tmp.join("cache.json"), json!({"initialize": {"serverInfo": {"name": "stealth"}}}).to_string())
        .unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    let started = std::time::Instant::now();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "spawn_browser"}})).await;
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did spawn_browser");
    proxy.stop_child().await;
    // One that never shows its screen: a clear error once its deadline is over, not before.
    config.insert("ready".into(), json!(["false"]));
    config.insert("ready_seconds".into(), json!(1));
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    let started = std::time::Instant::now();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert!(started.elapsed() >= Duration::from_secs(1));
    // said in the answer, never a JSON-RPC error (Hermes counts one against the server: browser_mcp `could_not_start`)
    let answer = out.messages().last().unwrap().clone();
    assert!(answer.get("error").is_none(), "{answer}");
    let message = answer["result"]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(message.contains("did not start within 1 s"), "{message}");
}

#[test]
fn hermes_runs_it_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let path = tmp.join("mcp.json");
    std::fs::write(&path, Value::Object(config(tmp)).to_string()).unwrap();
    let mut process = std::process::Command::new(env!("CARGO_BIN_EXE_bip-browser-mcp"))
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let lines = [
        init(),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "take_screenshot"}}),
    ];
    {
        let mut stdin = process.stdin.take().unwrap();
        for m in &lines {
            stdin.write_all(format!("{m}\n").as_bytes()).unwrap();
        }
    }
    let output = process.wait_with_output().unwrap();
    let answers: Vec<Value> =
        String::from_utf8(output.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(ids(&answers), vec![0, 1]);
    assert_eq!(answers[1]["result"]["content"][0]["text"], "did take_screenshot");
    assert_eq!(ran(tmp), vec!["start"]); // Hermes gone: its server stops, the browser stays until unused
}

#[tokio::test(flavor = "multi_thread")]
async fn while_the_month_s_credit_is_used_its_tools_are_refused_without_starting_it() {
    // credit.rs (Alex 2026-10-26): the browser pauses while the plan's AI credit is used; the agent is told plainly.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    let credit = tmp.join("credit.json");
    config.insert("credit".into(), json!(credit.to_string_lossy()));
    std::fs::write(&credit, json!({"state": "full", "resumes": 4.0e9}).to_string()).unwrap();
    std::fs::write(tmp.join("cache.json"), json!({"initialize": {"serverInfo": {"name": "stealth"}}}).to_string())
        .unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "navigate"}})).await;
    let answer = out.messages().last().unwrap().clone();
    // never `isError` (Hermes' breaker would pause the whole browser: browser_mcp.rs `soften`)
    assert_eq!((answer["id"].clone(), answer["result"].get("isError")), (json!(1), None));
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("credit is used up"), "{answer}");
    assert!(ran(tmp).is_empty() && !proxy.running());
    // stopped (the grace used too): the same
    std::fs::write(&credit, json!({"state": "stopped"}).to_string()).unwrap();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert!(out.messages().last().unwrap()["result"]["content"][0]["text"].as_str().unwrap().contains("credit"));
    // the credit back: the next call starts it as usual
    std::fs::write(&credit, json!({"state": "ok"}).to_string()).unwrap();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    assert_eq!(ran(tmp), vec!["start"]);
    proxy.stop_child().await;
}

fn now() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64()
}

fn read_page(id: i64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": "get_page_content", "arguments": {"instance_id": "319ceedb"}}})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_too_large_to_come_back_is_said_plainly_and_a_rescue_turn_reads_no_page() {
    // Token lab of 2026-11-07 (GLM-5.3 Flash, the French Wikipedia page of the Eiffel tower): the real server saved the
    // page to a file of its container and gave its path, out of the agent's reach; it read the page 16 times, then its
    // rescue turn read it again and waited the full 120 s.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    let server = FAKE_SERVER.replace(
        r#"r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}"#,
        r#"if m["params"]["name"] == "get_page_content":
            saved = {"file_path": "/opt/stealth/stealth-browser-mcp/element_clones/page_content_1.json",
                     "file_size_kb": 1399.12, "estimated_tokens": 353880,
                     "reason": "Response too large, automatically saved to file"}
            r = {"content": [{"type": "text", "text": json.dumps(saved)}], "structuredContent": saved}
        else:
            r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}"#,
    );
    std::fs::write(tmp.join("server.py"), server).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(read_page(1)).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(1));
    let content = answer["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    let text = content[0]["text"].as_str().unwrap();
    assert!(text.starts_with("This page is too large to come back whole (about 1.4 MB)"), "{text}");
    assert!(text.contains("Don't read it again") && text.contains(browser_mcp::AROUND_A_WORD));
    assert!(!answer.to_string().contains("element_clones"), "its path never shown");
    // its structured copy kept for the tool's output schema (Hermes' MCP client refuses a result without one)
    assert_eq!(answer["result"]["structuredContent"]["file_path"], json!(""));
    assert_eq!(answer["result"]["structuredContent"]["reason"], json!(browser_mcp::OUT_OF_REACH));
    // the other answers as they are
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");

    // The brake's rescue turn (the bridge's `rescue` file, beside `busy`): page reads refused at once, other calls go on.
    let rescue = tmp.join("rescue");
    assert_eq!(proxy.rescue_path, rescue);
    std::fs::write(&rescue, format!("{:.0}\n", now() + 180.0)).unwrap();
    for (id, name) in [(3, "get_page_content"), (4, "take_screenshot")] {
        proxy.handle(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name}})).await;
        let refused = out.messages().last().unwrap().clone();
        assert_eq!(refused["id"], json!(id));
        assert_eq!(refused["result"].get("isError"), None, "never an error (Hermes' breaker): {refused}");
        assert_eq!(refused["result"]["content"][0]["text"], browser_mcp::RESCUE_REFUSED);
    }
    assert!(browser_mcp::RESCUE_REFUSED.contains("Answer your person now with what you already have"));
    proxy.handle(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    // past its deadline (a bridge stopped meanwhile), or gone: the page read again
    std::fs::write(&rescue, format!("{:.0}\n", now() - 1.0)).unwrap();
    proxy.handle(read_page(6)).await;
    assert!(out.messages().last().unwrap()["result"]["content"][0]["text"].as_str().unwrap().starts_with("This page"));
    std::fs::remove_file(&rescue).unwrap();
    proxy.handle(read_page(7)).await;
    assert!(out.messages().last().unwrap()["result"]["content"][0]["text"].as_str().unwrap().starts_with("This page"));
    proxy.stop_child().await;

    // Straight away: a browser stopped meanwhile is not started for a refused read.
    std::fs::remove_file(tmp.join("log")).unwrap();
    std::fs::write(&rescue, format!("{:.0}\n", now() + 180.0)).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await; // from its cache: nothing started
    proxy.handle(read_page(8)).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], browser_mcp::RESCUE_REFUSED);
    assert!(ran(tmp).is_empty() && !proxy.running());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_exit_s_refusal_and_a_challenge_page_are_said_to_the_agent() {
    // phone_exit (docs/phone-exit.md §7): a page that failed because the person's phone was not relaying gets the
    // hub's reason (never « the site blocks servers »); a challenge page gets the card's news; nothing otherwise.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    let server = FAKE_SERVER.replace(
        r#"r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}"#,
        r#"url = (m["params"].get("arguments") or {}).get("url", "")
        page = "<title>Just a moment...</title>" if "challenge" in url else "did " + m["params"]["name"]
        r = {"content": [{"type": "text", "text": page}]}"#,
    );
    std::fs::write(tmp.join("server.py"), server).unwrap();
    // the hub: WHY answers « paused 3 » for ameli.fr, ASK answers CARD; every line kept
    let short = tempfile::Builder::new().prefix("pe").tempdir_in("/tmp").unwrap();
    let sock = short.path().join("hub.sock");
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    {
        let asked = asked.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            loop {
                let (conn, _) = listener.accept().await.unwrap();
                let (read, mut write) = conn.into_split();
                let mut line = String::new();
                tokio::io::BufReader::new(read).read_line(&mut line).await.unwrap();
                asked.lock().unwrap().push(line.trim().to_string());
                // the challenge's card, then (Alex 22/11, review 1) the person chose their phone for the site
                let asks = asked.lock().unwrap().iter().filter(|l| l.starts_with("ASK")).count();
                let answer = if line.starts_with("WHY vie www.ameli.fr") {
                    "paused 3\n"
                } else if line.starts_with("WHY") {
                    "NONE\n"
                } else if asks > 1 {
                    "LIVE\n"
                } else {
                    "CARD\n"
                };
                write.write_all(answer.as_bytes()).await.unwrap();
            }
        });
    }
    config.insert("agent".into(), json!("vie"));
    config.insert("exit_sock".into(), json!(sock.to_string_lossy()));
    config.insert("hand_wait_seconds".into(), json!(20.0));
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    let navigate = |id: i64, url: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": "navigate", "arguments": {"url": url}}})
    };
    let texts = |out: &Out| -> Vec<String> {
        let last = out.messages().last().unwrap().clone();
        last["result"]["content"].as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap().to_string()).collect()
    };
    proxy.handle(navigate(1, "https://www.ameli.fr/assure")).await;
    let said = texts(&out);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[1].starts_with("Phone exit (ameli.fr, paused)") && said[1].contains("Do NOT say the site blocks you"));
    // a later read of the same page: about the same site (the last URL navigated to)
    proxy
        .handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_page_content"}}))
        .await;
    assert_eq!(texts(&out).len(), 2);
    // a challenge page: the hub asked (its card), the person's phone offered beside the hand; they chose their phone
    // while the call waited: the agent reloads the page
    let started = std::time::Instant::now();
    proxy.handle(navigate(3, "https://www.leboncoin.fr/challenge")).await;
    let said = texts(&out);
    assert_eq!(said[1], browser_mcp::check_phone_text("leboncoin.fr"), "{said:?}");
    assert!(started.elapsed() < Duration::from_secs(15));
    // an ordinary page: nothing added
    proxy.handle(navigate(4, "https://www.doctolib.fr/")).await;
    assert_eq!(texts(&out), vec!["did navigate".to_string()]);
    let asked = asked.lock().unwrap().clone();
    assert!(asked.contains(&"ASK vie www.leboncoin.fr Cloudflare challenge".to_string()), "{asked:?}");
    assert!(asked.contains(&"WHY vie www.doctolib.fr".to_string()));
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_phone_exit_nothing_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config(dir.path()), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": "navigate", "arguments": {"url": "https://www.ameli.fr/"}}}))
        .await;
    let last = out.messages().last().unwrap().clone();
    assert_eq!(last["result"]["content"].as_array().unwrap().len(), 1);
    proxy.stop_child().await;
}

/// The real server for the browser-speed answers: `execute_script` runs the page's scripts (its readable text,
/// `document.readyState`), `take_screenshot` gives its image inline or, with `"full_page": true`, saved in its
/// container; every call written to `calls.jsonl`.
const SPEED_SERVER: &str = r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
page = {"title": "Boîte de réception", "url": "https://mail.example.test/inbox", "total": 40,
        "text": "Facture d'octobre — Portail Exemple", "main": True, "links": ["Écrire"]}
states = ["loading", "interactive", "complete"]
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": []}
    else:
        name, args = m["params"]["name"], m["params"].get("arguments", {})
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": name, "arguments": args}) + "\n")
        if name == "execute_script" and args["script"] == "document.readyState":
            answer = {"success": True, "result": states.pop(0) if states else "complete", "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "execute_script" and "JSON.stringify" in args["script"]:
            answer = {"success": True, "result": json.dumps(page), "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "take_screenshot" and args.get("full_page"):
            saved = {"file_path": "/opt/stealth/stealth-browser-mcp/element_clones/screenshot_1.jpeg",
                     "filename": "screenshot_1.jpeg", "file_size_kb": 180.2,
                     "reason": "Screenshot too large, automatically saved to file"}
            r = {"content": [{"type": "text", "text": json.dumps(saved)}], "structuredContent": {"result": saved}}
        elif name == "take_screenshot":
            data = "/9j/" + "A" * 200
            r = {"content": [{"type": "text", "text": data}], "structuredContent": {"result": data}}
        else:
            r = {"content": [{"type": "text", "text": "did " + name}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

/// `<exec prefix> cat|rm <path in the container>`: the container's clone folder is `clones/` here.
const FAKE_EXEC: &str = r#"
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
command, path = sys.argv[1], sys.argv[-1]
local = os.path.join(here, "clones", os.path.basename(path))
if command == "cat":
    sys.stdout.buffer.write(open(local, "rb").read())
elif command == "rm":
    os.remove(local)
"#;

fn calls(tmp: &Path) -> Vec<Value> {
    std::fs::read_to_string(tmp.join("calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn tool(id: i64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_come_back_as_text_screenshots_as_images_and_navigation_once_loaded() {
    // Alex's case of 8/11 (a test mailbox read after 3–4 minutes): a page « set aside », a screenshot « in the wrong
    // place », a pause after the click; each one more model call.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    std::fs::write(tmp.join("server.py"), SPEED_SERVER).unwrap();
    std::fs::write(tmp.join("exec.py"), FAKE_EXEC).unwrap();
    config.insert("exec".into(), json!([python(), tmp.join("exec.py").to_string_lossy()]));
    // no DevTools here: the page's own frame masked through the real server (a DevTools set up but out of reach
    // refuses the screenshot: a_screenshot_waits_for_devtools_when_they_are_out_of_reach)
    config.shift_remove("cdp_port");
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;

    // the page read: its readable text, never its HTML; the structured copy small (Hermes would send it too)
    proxy.handle(tool(1, "get_page_content", json!({"instance_id": "319ceedb"}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(1));
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        text,
        "Boîte de réception\nhttps://mail.example.test/inbox\n\nFacture d'octobre — Portail Exemple\n\n[5 more \
characters of this page. To read on, run execute_script with: "
            .to_string()
            + &browser_mcp::readable_script(35, browser_mcp::READABLE_CHARS)
            + " — or only the part around a word, in place of `word`: "
            + browser_mcp::AROUND_A_WORD
            + "]\n\nLinks and buttons: Écrire"
    );
    assert_eq!(answer["result"]["structuredContent"]["html"], json!(""));
    assert_eq!(answer["result"]["structuredContent"]["url"], json!("https://mail.example.test/inbox"));
    // the readable text, then the check that it holds no password (fifth review 13/11: one put in the page's title)
    let scripts = calls(tmp);
    assert_eq!(scripts.iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["execute_script"; 2]);
    assert!(scripts[1]["arguments"]["script"].as_str().unwrap().starts_with("/* bipagents guard: shown */"));
    // with its frames: the real tool
    proxy.handle(tool(2, "get_page_content", json!({"instance_id": "319ceedb", "include_frames": true}))).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did get_page_content");

    // navigate: answered once the page is loaded; `networkidle` (the real server's blind 2 s) not passed on
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    let url = "https://mail.example.test/";
    proxy
        .handle(tool(3, "navigate", json!({"instance_id": "319ceedb", "url": url, "wait_until": "networkidle"})))
        .await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    let seen = calls(tmp);
    assert_eq!(seen[0]["arguments"]["wait_until"], json!("domcontentloaded"));
    let states: Vec<_> = seen[1..seen.len() - 1].iter().map(|c| c["arguments"]["script"].as_str().unwrap()).collect();
    assert_eq!(states, ["document.readyState"; 3], "loading, interactive, complete");
    // then one look at the loaded page for a bot check that still blocks (antibot-guard): none here
    assert!(seen.last().unwrap()["arguments"]["script"].as_str().unwrap().contains("bipagents-bot-check"));
    // a click too; a call without an instance waits for nothing
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    proxy.handle(tool(4, "click_element", json!({"instance_id": "319ceedb", "selector": "a"}))).await;
    // the look for a form whose captcha is not answered (antibot-guard review 1: never sent), what the target is
    // (never the site's terms box), the page's fingerprints before (browser_mcp `guard_before`), the click, the
    // page's state, the check after it (`guard_after`), then the look for a bot check
    assert_eq!(calls(tmp).len(), 7);
    assert!(calls(tmp)[0]["arguments"]["script"].as_str().unwrap().contains("bipagents-bot-check"));
    assert!(calls(tmp)[1]["arguments"]["script"].as_str().unwrap().contains("found: true"));
    assert!(calls(tmp)[2]["arguments"]["script"].as_str().unwrap().contains("bipagents guard: before"));
    assert_eq!(calls(tmp)[3]["arguments"]["timeout"], json!(3000), "3 s to find it, not the real server's 10");
    assert!(calls(tmp)[5]["arguments"]["script"].as_str().unwrap().contains("bipagents guard: after"));
    assert!(calls(tmp)[6]["arguments"]["script"].as_str().unwrap().contains("bipagents-bot-check"));
    proxy.handle(tool(5, "list_tabs", json!({"instance_id": "319ceedb"}))).await;
    proxy.handle(tool(6, "click_element", json!({"selector": "a", "timeout": 8000}))).await;
    assert_eq!(calls(tmp).len(), 9);
    assert_eq!(calls(tmp)[8]["arguments"]["timeout"], json!(8000), "the agent's own time kept");

    // a screenshot: an image block, the agent's path never passed on (it would land in the container)
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    proxy.handle(tool(7, "take_screenshot", json!({"instance_id": "319ceedb", "file_path": "/root/shot.png"}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(7));
    let content = answer["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/jpeg");
    assert!(content[1]["text"].as_str().unwrap().contains("not written to /root/shot.png"));
    // the page's password fields out of sight first (vault-no-readback, 16/11: the site's eye shows one in clear)
    assert!(calls(tmp)[0]["arguments"]["script"].as_str().unwrap().starts_with("/* bipagents guard: conceal */"));
    assert_eq!(calls(tmp)[1]["arguments"], json!({"instance_id": "319ceedb", "format": "jpeg"}));
    // too large for the real server: saved in its container, fetched from it and removed there
    std::fs::create_dir_all(tmp.join("clones")).unwrap();
    std::fs::write(tmp.join("clones").join("screenshot_1.jpeg"), [0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
    proxy.handle(tool(8, "take_screenshot", json!({"instance_id": "319ceedb", "full_page": true}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["result"]["content"][0], json!({"type": "image", "data": "/9j/4A==", "mimeType": "image/jpeg"}));
    assert!(!answer.to_string().contains("element_clones"));
    assert!(!tmp.join("clones").join("screenshot_1.jpeg").exists(), "removed from the container");
    // gone before it was fetched: the real answer, as it was
    proxy.handle(tool(9, "take_screenshot", json!({"instance_id": "319ceedb", "full_page": true}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("too large"));
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_screenshot_waits_for_devtools_when_they_are_out_of_reach() {
    // Audit 17/11, 1.7 follow-up: the frames of other sites (a payment provider's, a login's) are masked through
    // DevTools only; with DevTools set up but out of reach (port 19602: nothing listens), the main frame alone could
    // be masked, so no screenshot is taken. Other page reads keep the main frame's masking.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), SPEED_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(tool(7, "take_screenshot", json!({"instance_id": "319ceedb"}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(7));
    assert!(
        calls(tmp).iter().all(|c| c["name"] != "take_screenshot"),
        "the real server never took it: {:?}",
        calls(tmp)
    );
    let text = answer["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.starts_with("No screenshot right now") && text.contains("DevTools"), "{answer}");
    assert!(browser_mcp::own_refusal(answer.as_object().unwrap()), "not a failed step of the page");
    proxy.stop_child().await;
}

/// The real server's two launch tools as stealth-browser-mcp (patched) lists them; every call written to `calls.jsonl`.
const LAUNCH_SERVER: &str = r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
spawn = {"name": "spawn_browser",
         "description": "Spawn a new browser instance.\n\nArgs:\n    headless (bool): Run in headless mode.\n    proxy (str): Proxy server URL.\n    browser_args (list): Extra Chrome flags.\n    viewport_width (int): Width.\n    user_data_dir (str): Profile folder.",
         "inputSchema": {"type": "object", "required": ["browser_args"], "properties": {
             "headless": {"type": "boolean"}, "user_agent": {"type": "string"}, "viewport_width": {"type": "integer"},
             "viewport_height": {"type": "integer"}, "proxy": {"type": "string"}, "browser_args": {"type": "array"},
             "timezone_id": {"type": "string"}, "idle_timeout_seconds": {"type": "integer"},
             "block_resources": {"type": "array"}, "extra_headers": {"type": "object"},
             "user_data_dir": {"type": "string"}, "sandbox": {"type": "boolean"}}}}
close = {"name": "close_instance",
         "description": "Close a browser instance. Only when the person asks to quit the browser, pass quit_browser=True.\n\nArgs:\n    instance_id (str): Browser instance ID.\n    quit_browser (bool): Quit the browser itself.",
         "inputSchema": {"type": "object", "required": ["instance_id"], "properties": {
             "instance_id": {"type": "string"}, "quit_browser": {"type": "boolean"}}}}
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [spawn, close, {"name": "navigate", "inputSchema": {"type": "object", "properties": {"url": {}}}}]}
    else:
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": m["params"]["name"], "arguments": m["params"].get("arguments")}) + "\n")
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_never_chooses_how_its_browser_is_launched() {
    // Audit 11/11, BV-3: `spawn_browser(browser_args=["--no-proxy-server"])` after `close_instance(quit_browser=True)`
    // started Chrome without the egress proxy's flag (no send guard), steered by a page the agent read.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), LAUNCH_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;

    // the tool list: none of what the bridge takes off is offered
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    let check_list = |listed: &Value| {
        let tools = listed["result"]["tools"].as_array().unwrap();
        let spawn = &tools[0];
        let mut offered: Vec<&str> =
            spawn["inputSchema"]["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        offered.sort();
        assert_eq!(offered, ["block_resources", "viewport_height", "viewport_width"]);
        assert_eq!(spawn["inputSchema"]["required"], json!([]));
        let text = spawn["description"].as_str().unwrap();
        assert!(text.contains("viewport_width (int)") && !text.contains("proxy") && !text.contains("browser_args"));
        assert!(!text.contains("headless") && !text.contains("user_data_dir"), "{text}");
        let close = &tools[1];
        assert_eq!(close["inputSchema"]["properties"], json!({"instance_id": {"type": "string"}}));
        assert!(!close["description"].as_str().unwrap().contains("quit_browser"));
        assert_eq!(tools[2]["inputSchema"]["properties"], json!({"url": {}})); // the other tools as they are
    };
    check_list(&out.messages().last().unwrap().clone());

    // the call: the launch arguments never reach the browser's container, the harmless ones do
    let asked = json!({"browser_args": ["--no-proxy-server"], "proxy": "http://203.0.113.9:1",
        "user_data_dir": "/tmp/elsewhere", "headless": true, "sandbox": true, "user_agent": "x",
        "extra_headers": {"Host": "x"}, "timezone_id": "UTC", "idle_timeout_seconds": 1,
        "viewport_width": 1280, "block_resources": ["image"]});
    proxy.handle(tool(2, "spawn_browser", asked)).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did spawn_browser");
    proxy.handle(tool(3, "spawn_browser", json!(["--no-proxy-server"]))).await; // not even an object
    proxy.handle(tool(4, "close_instance", json!({"instance_id": "319ceedb", "quit_browser": true}))).await;
    let page = json!({"instance_id": "319ceedb", "url": "https://example.test/", "proxy": "p"});
    proxy.handle(tool(5, "navigate", page)).await;
    let seen = calls(tmp);
    assert_eq!(seen[0]["arguments"], json!({"viewport_width": 1280, "block_resources": ["image"]}));
    assert_eq!(seen[1]["arguments"], json!({}));
    assert_eq!(seen[2]["arguments"], json!({"instance_id": "319ceedb"}), "the person quits it from the app");
    assert_eq!(seen[3]["arguments"]["proxy"], json!("p"), "other tools' arguments untouched");
    proxy.stop_child().await;

    // its gateway restarted: the cached list is the same tamed one, the browser not started for it
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"})).await;
    assert!(!proxy.running());
    check_list(&out.messages().last().unwrap().clone());
}

#[test]
fn a_list_cached_before_the_rule_is_tamed_too() {
    let mut listed = json!({"tools": [{"name": "spawn_browser", "description": "Args:\n    proxy (str): x\n    headless (bool): y",
        "inputSchema": {"type": "object", "properties": {"proxy": {}, "headless": {}, "viewport_width": {}}}}]});
    browser_mcp::tame_tool_list(&mut listed);
    assert_eq!(listed["tools"][0]["inputSchema"]["properties"], json!({"viewport_width": {}}));
    assert_eq!(listed["tools"][0]["description"], json!("Args:"));
    let mut call = json!({"name": "spawn_browser", "arguments": {"proxy": "x", "viewport_height": 900}});
    assert_eq!(browser_mcp::tame_call("spawn_browser", &mut call), ["proxy"]);
    assert_eq!(call["arguments"], json!({"viewport_height": 900}));
    let mut other = json!({"name": "navigate", "arguments": {"proxy": "x"}});
    assert!(browser_mcp::tame_call("navigate", &mut other).is_empty());
}

/// A browser's server as stealth-browser-mcp behind hold_gate.py: while `held` exists every call gets the gate's refusal
/// (`isError`); `#folded` can't be clicked (an error); the target script (browser_mcp `target_script`) says
/// `#register-password` is a password field and `#cgu` a terms box. Every call written to `calls.jsonl`.
const GUARD_SERVER: &str = r##"
import json, os, sys, time
here = os.path.dirname(os.path.abspath(__file__))
GATE = ("The person has taken over this browser in the BipAgents app (to log in, or to do something themselves). "
        "Do not use the browser until they hand it back: tell them you are waiting, or go on with something else.")
page = {"typed": 0, "ticked": 0, "opened": None}
def text(t, error=False):
    r = {"content": [{"type": "text", "text": t}]}
    if error:
        r["isError"] = True
    return r
def script_answer(value):
    answer = {"success": True, "result": value, "error": None}
    return {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        wrapped = {"type": "object", "properties": {"result": {"type": "boolean"}}, "required": ["result"],
                   "x-fastmcp-wrap-result": True}
        shot = {"type": "object", "required": ["result"], "x-fastmcp-wrap-result": True,
                "properties": {"result": {"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": True}]}}}
        r = {"tools": [{"name": "navigate", "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "additionalProperties": True}},
                       {"name": "click_element", "inputSchema": {"type": "object"}, "outputSchema": wrapped},
                       {"name": "type_text", "inputSchema": {"type": "object"}, "outputSchema": wrapped},
                       {"name": "take_screenshot", "inputSchema": {"type": "object"}, "outputSchema": shot},
                       {"name": "execute_script", "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "additionalProperties": True}}]}
    else:
        name, args = m["params"]["name"], m["params"].get("arguments", {})
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": name, "arguments": args}) + "\n")
        script = args.get("script", "")
        selector = args.get("selector", "")
        late_there = page["opened"] is not None and time.time() - page["opened"] >= 0.6
        if os.path.exists(os.path.join(here, "held")):
            r = text(GATE, True)
        elif name == "execute_script" and "bipagents guard: before" in script:
            # a page with a card's field when the test says so (`cardpage`)
            cards = {"#cc": "811c9dc5"} if os.path.exists(os.path.join(here, "cardpage")) else {}
            r = script_answer(json.dumps({"pw": {"#register-password": ""}, "known": [], "boxes": ["#cgu"], "cards": cards}))
        elif name == "execute_script" and "bipagents guard: after" in script:
            r = script_answer(json.dumps({"passwords": page["typed"], "terms": page["ticked"]}))
            page["typed"] = page["ticked"] = 0
        elif name == "execute_script" and "found: true" in script:
            if '"#late-password"' in script and not late_there:
                target = {"found": False}
            else:
                target = {"found": True, "password": "password" in script.split(";")[0],
                          "terms": "J'accepte les conditions générales de vente  cgu cgu"
                                   if '"#cgu"' in script or "span.box" in script else ""}
                # the contact form of contact.example (audit 1.2): its send button, and its message field (Enter)
                if '"button[type=submit]"' in script:
                    target["send"] = "contact.example"
                if '"#message"' in script:
                    target["enter"] = "contact.example"
            r = script_answer(json.dumps(target))
        elif name == "execute_script" and script == "document.readyState":
            r = script_answer("complete")
        elif name == "execute_script" and "querySelectorAll('input, select, textarea')" in script:
            r = script_answer("#lastname (text, Nom); #phone (tel, Téléphone)")
        elif name == "click_element" and selector == "#folded":
            r = text("Error calling tool 'click_element': Failed to click element: DOM Error while querying [code: -32000]", True)
        else:
            if name == "click_element" and selector == "#open-register":
                page["opened"] = time.time()
            if name == "click_element" and "conditions" in args.get("text_match", ""):
                page["ticked"] = 1            # the real server clicks by text, the selector unused
            if name in ("type_text", "paste_text") and "password" in selector:
                page["typed"] = 1
            if name == "press_key" and "password" in selector and len(args.get("key", "")) == 1:
                page["typed"] = 1
            if name == "execute_script" and ("Provisoire" in script + json.dumps(args.get("args", []))):
                page["typed"] = 1
            if name == "execute_script" and "'cgu'" in script and ".checked = true" in script:
                page["ticked"] = 1
            r = text("did " + name)
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"##;

/// The proxy over `GUARD_SERVER`, connected and its tool list (with their output schemas) learned, as Hermes does.
async fn guarded(tmp: &Path) -> (Arc<Proxy>, Out) {
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), GUARD_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 99, "method": "tools/list"})).await;
    (proxy, out)
}

fn last(out: &Out) -> Value {
    out.messages().last().unwrap().clone()
}

fn text_of(answer: &Value) -> String {
    answer["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn handed_back_the_browser_answers_at_once_and_nothing_pauses_it() {
    // 13/11 (Vie on aibox, then the lab): three calls refused while the person held the browser opened Hermes' breaker
    // (« Paused for ~54s »), so the browser stayed paused a minute after « Rendre la main ». Never an error now.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    std::fs::write(tmp.join("held"), "human t0k").unwrap();
    for id in 1..=4 {
        proxy.handle(tool(id, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
        let answer = last(&out);
        assert_eq!(answer["id"], json!(id));
        assert_eq!(answer["result"].get("isError"), None, "{answer}");
        assert_eq!(text_of(&answer), browser_mcp::PERSON_HOLDS, "said plainly, no streak hint for a hold");
        // with what its output schema asks (Hermes' client refuses a result without: lab, 13/11)
        assert_eq!(answer["result"]["structuredContent"], json!({"success": false}));
    }
    // « Rendre la main »: the very next call goes through
    std::fs::remove_file(tmp.join("held")).unwrap();
    let started = std::time::Instant::now();
    proxy.handle(tool(5, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
    assert_eq!(text_of(&last(&out)), "did navigate");
    assert!(started.elapsed() < Duration::from_secs(5));

    // fields still folded in a panel: errors, never `isError`; the third in a row says what to do
    for id in 6..=8 {
        proxy.handle(tool(id, "click_element", json!({"instance_id": "i", "selector": "#folded"}))).await;
        let answer = last(&out);
        assert_eq!(answer["result"].get("isError"), None, "{answer}");
        assert!(text_of(&answer).starts_with("Error calling tool 'click_element'"));
        assert_eq!(text_of(&answer).contains(browser_mcp::STREAK_HINT), id == 8, "{answer}");
        // with the page's real fields (lab 13/11: guessed names, then the panel clicked open again and again)
        let fields = "instructions): #lastname (text, Nom); #phone (tel, Téléphone)";
        assert_eq!(text_of(&answer).contains(fields), id == 8, "{answer}");
        assert_eq!(answer["result"]["structuredContent"], json!({"result": false}));
    }
    assert_eq!(proxy.failures.load(Ordering::SeqCst), 3);
    proxy.handle(tool(9, "click_element", json!({"instance_id": "i", "selector": "#open-register"}))).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    assert_eq!(proxy.failures.load(Ordering::SeqCst), 0, "a step that worked starts again from 0");
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_never_types_a_password_nor_ticks_the_terms() {
    // 13/11: Vie typed « a provisional password » itself; on the lab GLM typed into #register-password three times,
    // and agents ticked the terms with a script
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    let typed = |tmp: &Path| -> Vec<String> {
        calls(tmp)
            .iter()
            .filter(|c| c["name"] != "execute_script")
            .map(|c| format!("{} {}", c["name"].as_str().unwrap(), c["arguments"]["selector"].as_str().unwrap_or("")))
            .collect()
    };
    let password = json!({"instance_id": "i", "selector": "#register-password", "text": "Provisoire123!"});
    proxy.handle(tool(1, "type_text", password.clone())).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    assert_eq!(last(&out)["result"].get("isError"), None);
    assert_eq!(last(&out)["result"]["structuredContent"], json!({"result": false}));
    proxy.handle(tool(2, "paste_text", password)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    proxy.handle(tool(3, "press_key", json!({"instance_id": "i", "selector": "#register-password", "key": "P"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    assert!(typed(tmp).is_empty(), "nothing typed into the password field: {:?}", typed(tmp));
    // the other fields, and a key (« Enter ») in the password field, go on
    proxy.handle(tool(4, "type_text", json!({"instance_id": "i", "selector": "#lastname", "text": "Testeur"}))).await;
    assert_eq!(text_of(&last(&out)), "did type_text");
    proxy
        .handle(tool(5, "press_key", json!({"instance_id": "i", "selector": "#register-password", "key": "Enter"})))
        .await;
    assert_eq!(text_of(&last(&out)), "did press_key");
    // a script writing the password field: refused before the page
    let script = "document.getElementById('register-password').value = 'Provisoire123!'";
    proxy.handle(tool(6, "execute_script", json!({"instance_id": "i", "script": script}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_PASSWORD_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["arguments"]["script"].as_str().is_some_and(|s| s.contains("Provisoire"))));
    // the terms: the person's to tick, by a click or a script; the newsletter box is not the terms
    proxy.handle(tool(7, "click_element", json!({"instance_id": "i", "selector": "#cgu"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::TERMS_REFUSED);
    proxy
        .handle(tool(
            8,
            "execute_script",
            json!({"instance_id": "i", "script": "document.getElementById('cgu').checked = true"}),
        ))
        .await;
    // a script that ticks: never run (hotfix v2: only a read V8 proves runs, and this proxy has no DevTools)
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED, "a script that writes is never run");
    assert!(!calls(tmp).iter().any(|c| c["arguments"]["script"].as_str().is_some_and(|s| s.contains(".checked = true"))));
    proxy.handle(tool(9, "click_element", json!({"instance_id": "i", "selector": "#newsletter"}))).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    assert_eq!(typed(tmp), ["type_text #lastname", "press_key #register-password", "click_element #newsletter"]);
    // a script sent under another tool's name is a script (DeepSeek on the lab, 13/11): checked as one, never given to
    // the real server (hotfix v2: only V8's proof of a read runs it; this proxy has no DevTools)
    proxy
        .handle(tool(
            10,
            "take_screenshot",
            json!({"instance_id": "i", "script": "return document.title", "full_page": true}),
        ))
        .await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["name"] == "execute_script" && c["arguments"]["script"] == "return document.title"));
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_slips_past_the_checks_before_a_call_is_undone_after_it() {
    // review 13/11: a click on the terms' text (the real server finds it by `text_match`, the selector unused), the
    // field and the password in a script's `args`; a password field drawn 600 ms after the panel opens, waited for
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    let terms =
        json!({"instance_id": "i", "selector": "label", "text_match": "J'accepte les conditions générales de vente"});
    proxy.handle(tool(1, "click_element", terms)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::TERMS_UNDONE);
    assert_eq!(last(&out)["result"]["structuredContent"], json!({"result": false}));
    // a box drawn inside the terms' label: refused before the click (its label's box looked at)
    proxy.handle(tool(2, "click_element", json!({"instance_id": "i", "selector": ".custom-checkbox span.box"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::TERMS_REFUSED);
    let script = json!({"instance_id": "i", "script": "const e = document.querySelector(arguments[0]); e.value = arguments[1];",
                        "args": ["#register-password", "Provisoire123!"]});
    proxy.handle(tool(3, "execute_script", script)).await;
    // the page's password field named in its args: refused before it runs (fifth review 13/11)
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_PASSWORD_REFUSED);
    // the panel opened, then at once its password field: waited for, then refused, never typed
    proxy.handle(tool(4, "click_element", json!({"instance_id": "i", "selector": "#open-register"}))).await;
    let late = json!({"instance_id": "i", "selector": "#late-password", "text": "Provisoire123!"});
    proxy.handle(tool(5, "type_text", late)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["name"] == "type_text"), "nothing typed");
    // « P » with Shift is a character too
    let shift = json!({"instance_id": "i", "selector": "#register-password", "key": "p", "modifiers": ["Shift"]});
    proxy.handle(tool(6, "press_key", shift)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    // refusing cookies is no terms box; a passenger's name no password
    let cookies = "document.querySelector('.fc-cta-do-not-consent').click()";
    // (past the guards: refused only as unproven reads, hotfix v2, this proxy having no DevTools)
    proxy.handle(tool(7, "execute_script", json!({"instance_id": "i", "script": cookies}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    let passenger = "document.querySelector('#passenger-firstname').value = 'Camille'";
    proxy.handle(tool(8, "execute_script", json!({"instance_id": "i", "script": passenger}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    // none of the proxy's own refusals counts as a step that failed
    assert_eq!(proxy.failures.load(Ordering::SeqCst), 0);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_its_own_world_the_guard_refuses_a_script_that_acts_on_a_card_page() {
    // second review of card-guards, 14/11: the guard looked silently through the page's own world when DevTools
    // couldn't be reached (here: nothing listens on its port); now a script that writes or clicks is refused where the
    // page has card fields, and reading goes on, and so do pages without cards
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    std::fs::write(tmp.join("cardpage"), "").unwrap();
    let click = json!({"instance_id": "i", "script": "document.querySelector('#go').click()"});
    proxy.handle(tool(1, "execute_script", click.clone())).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_BLIND_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["arguments"]["script"].as_str().is_some_and(|s| s.contains("#go"))));
    // a read there: refused too since the hotfix of 15/11 (no script on a page showing card fields)
    proxy.handle(tool(2, "execute_script", json!({"instance_id": "i", "script": "return document.title"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_ON_CARDS_REFUSED);
    std::fs::remove_file(tmp.join("cardpage")).unwrap();
    proxy.handle(tool(3, "execute_script", click)).await;
    // past the guards there too, never run without V8's proof of a read (hotfix v2)
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    proxy.stop_child().await;
}

/// hold_gate.py while a card payment (or the vault typing a password) holds the browser: `PAYING`, as an `isError`.
const PAYING_SERVER: &str = r##"
import json, sys
PAYING = ("A card payment is going on in this browser (or the bridge is typing a saved password): nobody may read the "
          "page. Your browser tools wait until it ends: if you started a payment, use confirm_payment (or "
          "cancel_payment) from the BipAgents tools; otherwise wait a few minutes, the bridge gives the browser back "
          "by itself.")
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "navigate", "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "additionalProperties": True}}]}
    else:
        r = {"content": [{"type": "text", "text": PAYING}], "isError": True}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"##;

#[tokio::test(flavor = "multi_thread")]
async fn a_payment_holding_the_browser_is_no_streak_of_failed_steps() {
    // review 13/11: like the person's hold, a payment's (or the vault fill's) is no step that failed on the page
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), PAYING_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 99, "method": "tools/list"})).await;
    for id in 1..=3 {
        proxy.handle(tool(id, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
    }
    let third = last(&out);
    proxy.stop_child().await;
    assert!(text_of(&third).starts_with("A card payment is going on"), "{third}");
    assert!(!text_of(&third).contains(browser_mcp::STREAK_HINT), "{third}");
    assert_eq!(third["result"].get("isError"), None);
    assert_eq!(third["result"]["structuredContent"], json!({"success": false}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_cannot_start_is_never_a_json_rpc_error() {
    // review 13/11: Hermes counts a JSON-RPC error (McpError) against the server, as `isError` (`_on_failure`)
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    let init_result = json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                             "serverInfo": {"name": "stealth", "version": "1"}});
    let tools = json!({"tools": [{"name": "navigate", "inputSchema": {"type": "object"},
                                  "outputSchema": {"type": "object", "additionalProperties": true}}]});
    std::fs::write(tmp.join("cache.json"), json!({"initialize": init_result, "tools/list": tools}).to_string())
        .unwrap();
    config.insert("start".into(), json!(["false"])); // its container does not start
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    for id in 1..=3 {
        proxy.handle(tool(id, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
        let answer = last(&out);
        assert!(answer.get("error").is_none(), "a strike for Hermes' breaker: {answer}");
        assert!(text_of(&answer).starts_with("the agent's browser could not start"), "{answer}");
        assert_eq!(answer["result"]["structuredContent"], json!({"success": false}));
    }
}

#[test]
fn a_terms_box_that_also_names_cookies_is_the_terms() {
    // second review 13/11: many French sign-up forms word their terms box this way since the CNIL's guidance (terms,
    // privacy and cookie policies in one sentence); a cookie banner's own choice is not the terms
    let not_terms = [
        "J'ai lu et j'accepte les conditions générales d'utilisation et la politique de confidentialité et de cookies  cgu cgu",
        "Ich akzeptiere die AGB und die Datenschutz- und Cookie-Richtlinie  agb agb",
        "I agree to the Terms of Use and the Privacy and Cookie Policy  terms terms",
    ]
    .into_iter()
    .filter(|label| !browser_mcp::terms_box(label))
    .collect::<Vec<_>>();
    assert!(not_terms.is_empty(), "left to the agent: {not_terms:#?}");
    assert!(!browser_mcp::terms_box("Accepter les cookies de mesure d'audience (consentement) cookie-consent"));
}

/// What `guard_before` answers on a sign-up page (a password field, the terms not ticked) and on a page with neither.
fn signup_page() -> Value {
    json!({"pw": {"#register-password": "", "#pass1": "", "#pass2": ""}, "known": [], "boxes": ["#cgu"]})
}
fn plain_page() -> Value {
    json!({"pw": {}, "known": [], "boxes": []})
}

#[test]
fn a_script_that_fills_a_form_and_sends_it_is_refused() {
    // second review 13/11: the fields (the password among them) set from a table, the terms ticked, then sent, in one
    // script: the site got everything before the check after the call
    let page = signup_page();
    let script = "const data = {'#lastname': 'Testeur', '#register-password': 'Provisoire123!'}; \
        for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; \
        document.querySelector('#cgu').checked = true; document.getElementById('register-form').requestSubmit();";
    assert!(browser_mcp::script_fills_and_sends(script, Some(&page)));
    // third review: an app without a form, its button clicked by the same script
    let app = "const data = {'#email': 'c@example.org', '#password': 'Provisoire123!'}; \
        for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; document.querySelector('#create').click();";
    assert!(browser_mcp::script_fills_and_sends(app, Some(&page)));
    assert!(browser_mcp::script_fills_and_sends(
        "document.querySelector('#cgu').checked = true; document.querySelector('#go').click()",
        Some(&page)
    ));
    // one step at a time goes on: a field set, the form sent
    assert!(!browser_mcp::script_fills_and_sends("document.querySelector('#lastname').value = 'Testeur'", Some(&page)));
    assert!(!browser_mcp::script_fills_and_sends(
        "document.getElementById('register-form').requestSubmit()",
        Some(&page)
    ));
    // the page can't say: as a sign-up page
    assert!(browser_mcp::script_fills_and_sends(app, None));
}

#[test]
fn a_search_typed_and_sent_by_a_script_goes_on() {
    // third review 13/11: a site's search box filled and sent in one script: no password, no box
    let page = plain_page();
    let refused = [
        "document.querySelector('input[name=q]').value = 'doliprane'; document.querySelector('#search-form').submit();",
        "const f = document.forms[0]; f.elements.q.value = 'horaires'; f.requestSubmit(); 'ok'",
    ]
    .into_iter()
    .filter(|script| browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(refused.is_empty(), "refused: {refused:#?}");
}

// --- fourth review of fix-signup-hold (81c8cbdbf) ---------------------------------------------------------------------

#[test]
fn review4_a_box_clicked_or_a_password_named_otherwise_then_a_button_clicked_is_refused() {
    // decided from the page (a password field, the terms not ticked), not the script's words
    let page = signup_page();
    let passed = [
        // ticking a box is a click on it
        "document.querySelector('#cgu').click(); document.querySelector('#create').click(); 'sent';",
        // in a loop
        "[document.querySelector('#cgu'), document.querySelector('#create')].forEach((e) => e.click());",
        // WordPress's password fields (pass1, pass2), set from a table
        "const data = {'#email': 'camille@example.org', '#pass1': 'Provisoire123!', '#pass2': 'Provisoire123!'}; \
         for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; \
         document.querySelector('#create').click(); 'sent';",
    ]
    .into_iter()
    .filter(|script| !browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
}

#[test]
fn review4_a_shop_filter_or_a_reset_e_mail_sent_by_a_script_goes_on() {
    // no password field, no terms box on the page: a product filter applied, a « mot de passe oublié » e-mail asked for
    let page = plain_page();
    let refused = [
        "document.querySelector('#filter-bio').checked = true; document.querySelector('#apply-filters').click();",
        "document.querySelector('#forgot-password-email').value = 'camille@example.org'; \
         document.querySelector('#send-reset-link').click();",
    ]
    .into_iter()
    .filter(|script| browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(refused.is_empty(), "refused: {refused:#?}");
}

#[test]
fn a_scripts_answer_holding_a_password_is_checked_in_the_page() {
    // the value never leaves the page: the answer goes in, true or false comes back
    let script = browser_mcp::shows_password("{\"result\": \"[\\\"camille\\\", \\\"Xy7-secret\\\"]\"}");
    assert!(script.starts_with("/* bipagents guard: shown */"));
    assert!(script.contains("'password'") && script.contains("A.includes(f)"));
    assert!(!script.contains("return e.value") && !script.contains("JSON.stringify({"));
}

// --- fifth review of fix-signup-hold (80be896d5) ----------------------------------------------------------------------

#[test]
fn review5_jquery_setting_a_password_or_ticking_the_terms_then_clicking_is_refused() {
    // many shops load jQuery (PrestaShop, WooCommerce, Magento): `.val()` writes a value, `.prop('checked', true)` and
    // `.attr('checked', …)` tick a box, on a page with a password field and the terms not ticked yet
    let page = json!({"pw": {"#pass1": ""}, "known": [], "boxes": ["#cgu"]});
    let passed = [
        "$('#pass1').val('Provisoire123!'); $('#pass2').val('Provisoire123!'); $('#create').click();",
        "$('#cgu').prop('checked', true); $('#create').click();",
        "jQuery('#cgu').attr('checked', 'checked'); jQuery('#signup-form').trigger('submit');",
    ]
    .into_iter()
    .filter(|script| !browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
}

#[test]
fn two_clicks_in_one_script_on_a_page_whose_terms_arent_ticked_are_refused() {
    // sixth review 13/11: what a click aims at can't be told from a script's words (a label found by its text); a
    // drop-down list of the site's own (signup.html's « Civilité ») takes two click_element there
    let page = signup_page();
    let script = "document.querySelector('#civility').click(); \
                  document.querySelector('#civility-options li[data-value=\"M\"]').click(); 'ok';";
    assert!(browser_mcp::script_fills_and_sends(script, Some(&page)));
    assert!(browser_mcp::SCRIPT_SENDS_REFUSED.contains("one click_element per click"));
    // no terms on the page: it goes on
    assert!(!browser_mcp::script_fills_and_sends(script, Some(&plain_page())));
    // one click goes on (a panel opened)
    assert!(!browser_mcp::script_fills_and_sends("document.querySelector('#open-register').click();", Some(&page)));
}

#[test]
fn a_script_reading_a_password_field_by_its_id_or_all_the_forms_values_is_refused() {
    // fifth review: `#pass1` read (then encoded, or put in the page's title); every field's value while one holds text
    let filled = json!({"pw": {"#pass1": "9f3a01bc", "INPUT|pass2|0": "9f3a01bc"}, "known": ["9f3a01bc"], "boxes": []});
    for script in [
        "btoa(document.querySelector('#pass1').value)",
        "document.title = document.getElementsByName('pass2')[0].value; 'ok'",
        "JSON.stringify([...document.querySelectorAll('input')].map((e) => e.value))",
        "JSON.stringify([...new FormData(document.forms[0])])",
        "$('#register-form').serialize()",
    ] {
        assert!(browser_mcp::script_reads_password(script, Some(&filled)), "{script}");
    }
    for script in [
        "document.getElementById('email').value",
        "document.querySelector('#password-rules').innerText",
        "document.querySelector('#pass1').getBoundingClientRect().top",
    ] {
        assert!(!browser_mcp::script_reads_password(script, Some(&filled)), "{script}");
    }
    // nothing typed in it yet: listing the fields' values goes on
    let empty = json!({"pw": {"#pass1": ""}, "known": [], "boxes": []});
    assert!(!browser_mcp::script_reads_password("[...document.querySelectorAll('input')].map((e) => e.value)", Some(&empty)));
}

// --- sixth review of fix-signup-hold (ad232660d) ----------------------------------------------------------------------

#[test]
fn review6_the_terms_label_found_by_its_text_then_a_button_clicked_is_refused() {
    // how a model clicks what it read: no id, no « checkbox », no word of the terms in the script
    let page = json!({"pw": {}, "known": [], "boxes": ["#cgu"]});
    let script = "[...document.querySelectorAll('label')].find((l) => l.innerText.includes(\"J'accepte\")).click(); \
                  document.querySelector('#create').click(); 'sent';";
    assert!(browser_mcp::script_fills_and_sends(script, Some(&page)), "{script}");
}

#[test]
fn review6_a_password_read_as_the_focused_field_is_refused() {
    // the password field the agent clicked into (before vault_create_login), read back as `document.activeElement`
    let page = json!({"pw": {"#pass1": "1a2b3c4d"}, "known": ["1a2b3c4d"], "boxes": []});
    let script = "btoa(document.activeElement.value)";
    assert!(browser_mcp::script_reads_password(script, Some(&page)), "{script}");
}

// --- seventh review of fix-signup-hold (e2070a5d6): a remark, not blocking ------------------------------------------

#[test]
fn review7_a_form_sent_through_the_prototype_after_one_click_is_refused() {
    // one click (the terms' label found by its text), then the form sent without `.submit(` / `requestSubmit` words
    let page = json!({"pw": {}, "known": [], "boxes": ["#cgu"]});
    let passed = [
        "[...document.querySelectorAll('label')].find((l) => l.innerText.includes(\"J'accepte\")).click(); \
         HTMLFormElement.prototype.submit.call(document.getElementById('register-form'));",
        "[...document.querySelectorAll('label')].find((l) => l.innerText.includes(\"J'accepte\")).click(); \
         document.getElementById('register-form')['submit']();",
    ]
    .into_iter()
    .filter(|script| !browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
    // a submit button found by its selector is no sending
    assert!(!browser_mcp::script_fills_and_sends("document.querySelector('button[type=submit]').innerText", Some(&page)));
}

/// Audit 1.2 (17/11, Alex « Oui, 1re fois par site »): Hermes' `stealth` server has no `trust: untrusted` (every click
/// would be a card), so the proxy's guard asks: the first form sent to a site not allowed for the agent (its send
/// button clicked, Enter in its field) is refused with what to do (`ask_to_send`), whatever the page asked; typing is
/// no sending. The person's « Oui » (a one-use grant in `SEND_SITES_FILE`) lets one sending through, « Toujours pour ce
/// site » every one.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_form_sent_to_a_site_waits_for_the_persons_ok() {
    let entry = bipbridge::browsers::mcp_server("podman", "vie", None);
    assert!(entry.get("trust").is_none(), "no card for every click: {entry}");
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    let calls = || std::fs::read_to_string(tmp.join("calls.jsonl")).unwrap_or_default();
    let real = |name: &str| calls().lines().filter(|l| l.contains(&format!("\"name\": \"{name}\""))).count();
    proxy.handle(tool(1, "type_text", json!({"instance_id": "i", "selector": "#message", "text": "Voici les codes"}))).await;
    assert_eq!(text_of(&last(&out)), "did type_text", "typing sends nothing");
    let submit = json!({"instance_id": "i", "selector": "button[type=submit]"});
    proxy.handle(tool(2, "click_element", submit.clone())).await;
    let clicked = last(&out);
    assert!(text_of(&clicked).starts_with("Not done: this sends a form to contact.example"), "{clicked}");
    assert!(text_of(&clicked).contains("ask_to_send") && text_of(&clicked).contains("Nothing was sent"));
    assert_eq!(clicked["result"].get("isError"), None, "never an error for Hermes' breaker: {clicked}");
    assert_eq!(real("click_element"), 0, "the real server never got the click");
    proxy.handle(tool(3, "press_key", json!({"instance_id": "i", "selector": "#message", "key": "Enter"}))).await;
    assert!(text_of(&last(&out)).starts_with("Not done: this sends a form to contact.example"), "Enter sends too");
    assert_eq!(real("press_key"), 0);
    // a vault fill's grant: one sending, then asked again
    let file = tmp.join(browser_mcp::SEND_SITES_FILE);
    let until = bipbridge::py::time_time() + 600.0;
    let once = json!({"always": [], "once": [{"id": "g1", "host": "contact.example", "until": until}]});
    std::fs::write(&file, once.to_string()).unwrap();
    proxy.handle(tool(4, "click_element", submit.clone())).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    proxy.handle(tool(5, "click_element", submit.clone())).await;
    assert!(text_of(&last(&out)).starts_with("Not done: this sends a form to"), "a vault fill's grant serves once");
    // the person's « Oui » (18/11: a checkout's steps asked again and again): every sending there for its minutes
    let window = json!({"always": [], "once": [{"id": "g2", "host": "contact.example", "until": until, "window": true}]});
    std::fs::write(&file, window.to_string()).unwrap();
    proxy.handle(tool(51, "click_element", submit.clone())).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    proxy.handle(tool(52, "press_key", json!({"instance_id": "i", "selector": "#message", "key": "Enter"}))).await;
    assert_eq!(text_of(&last(&out)), "did press_key", "a « Oui » covers the site for now, not one sending");
    // « Toujours pour ce site »
    std::fs::write(&file, json!({"always": ["*.example"], "once": []}).to_string()).unwrap();
    proxy.handle(tool(6, "click_element", submit.clone())).await;
    proxy.handle(tool(7, "press_key", json!({"instance_id": "i", "selector": "#message", "key": "Enter"}))).await;
    assert_eq!(text_of(&last(&out)), "did press_key");
    assert_eq!((real("click_element"), real("press_key")), (3, 2));
    proxy.stop_child().await;
}

#[test]
fn a_site_allowed_for_good_covers_its_subdomains_only() {
    assert!(browser_mcp::send_covered(&["*.example.com".into()], "example.com"));
    assert!(browser_mcp::send_covered(&["*.example.com".into()], "Login.Example.com."));
    assert!(!browser_mcp::send_covered(&["*.example.com".into()], "badexample.com"));
    assert!(!browser_mcp::send_covered(&["shop.example".into()], ""));
}

/// One browser per running task (19/11): through the one proxy Hermes keeps, a task's calls (its Hermes session in
/// the agent's `sessions.json`, patch 0028) start and reach that task's own browser; the person's reach the agent's.
#[tokio::test(flavor = "multi_thread")]
async fn a_tasks_calls_start_and_reach_its_own_browser_the_persons_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, task) = (dir.path().join("vie"), dir.path().join("vie--t1"));
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&task).unwrap();
    let agent_config = config(&agent);
    let mut task_config = config(&task);
    // the task's browser answers in its own words
    std::fs::write(task.join("server.py"), FAKE_SERVER.replace("\"did \"", "\"task did \"")).unwrap();
    task_config.insert("server".into(), json!([python(), task.join("server.py").to_string_lossy()]));
    std::fs::write(task.join("mcp.json"), Value::Object(task_config).to_string()).unwrap();
    let sessions = json!({"api_task": task.join("mcp.json").to_string_lossy()});
    std::fs::write(agent.join(bipbridge::browsers::SESSIONS_FILE), sessions.to_string()).unwrap();
    let call = |id: i64, session: Option<&str>| {
        let mut params = json!({"name": "navigate", "arguments": {"url": "https://shop.example"}});
        if let Some(session) = session {
            params["_meta"] = json!({bipbridge::agent_tools::CALL_SESSION_META: session});
        }
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params})
    };
    let lines: String = [init(), call(1, Some("api_task")), call(2, None), call(3, Some("api_other"))]
        .iter()
        .map(|m| format!("{m}\n"))
        .collect();
    let out = Out::default();
    let proxy = Proxy::new(agent_config, Box::new(out.clone()));
    proxy.serve(lines.as_bytes()).await;
    let answers: std::collections::HashMap<i64, String> = out
        .messages()
        .iter()
        .filter_map(|m| Some((m["id"].as_i64()?, m["result"]["content"][0]["text"].as_str()?.to_string())))
        .collect();
    assert_eq!(answers.get(&1).map(String::as_str), Some("task did navigate"), "{answers:?}");
    assert_eq!(answers.get(&2).map(String::as_str), Some("did navigate"));
    assert_eq!(answers.get(&3).map(String::as_str), Some("did navigate"), "a session nobody announced: the agent's");
    assert_eq!(ran(&task), vec!["start"], "the task's own browser started");
    assert_eq!(ran(&agent), vec!["start"]);
}

/// Review 3 of subtasks-redesign (20/11, head 357a87a0c): Hermes patch 0029 sends the `stealth` entry's calls of
/// different sessions side by side (`call_context: true`), but the agent's proxy keeps ONE `call_session` for « the
/// current call » (browser_mcp.rs `Proxy::call_session`, « Hermes sends this server's calls one at a time »). A call
/// of another session that arrives while a form-sending click is being checked (its page read) overwrites it: the
/// click is judged with the other session's « Oui » — refused although its own conversation said Oui (or let through
/// on the other conversation's).
#[tokio::test(flavor = "multi_thread")]
async fn review3_two_sessions_calls_at_once_each_keep_their_own_oui() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    // the page read of the send button takes a second (a slow page): the other conversation's call arrives meanwhile
    let slow = GUARD_SERVER.replace(
        "                if '\"button[type=submit]\"' in script:\n",
        "                if '\"#slow-send\"' in script:\n                    time.sleep(1.0)\n                    target[\"send\"] = \"contact.example\"\n                if '\"button[type=submit]\"' in script:\n",
    );
    assert_ne!(slow, GUARD_SERVER);
    std::fs::write(tmp.join("server.py"), slow).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 99, "method": "tools/list"})).await;
    // the person said « Oui » to « Envoyer sur ce site ? » in conversation api_a (form_send.rs `grant_for_now`)
    let until = bipbridge::py::time_time() + 600.0;
    let grant = json!({"always": [], "once": [{"id": "g1", "host": "contact.example", "until": until, "window": true, "session": "api_a"}]});
    std::fs::write(tmp.join(browser_mcp::SEND_SITES_FILE), grant.to_string()).unwrap();
    let call = |id: i64, name: &str, arguments: Value, session: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments,
               "_meta": {bipbridge::agent_tools::CALL_SESSION_META: session}}})
    };
    let a = {
        let proxy = proxy.clone();
        let message = call(1, "click_element", json!({"instance_id": "i", "selector": "#slow-send"}), "api_a");
        tokio::spawn(async move { proxy.handle(message).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    // another conversation of the same agent (a routine, the person's other conversation) uses the browser meanwhile
    proxy.handle(call(2, "navigate", json!({"instance_id": "j", "url": "https://news.example/"}), "api_b")).await;
    a.await.unwrap();
    let answer = out.messages().into_iter().find(|m| m["id"] == json!(1)).expect("an answer to api_a's click");
    assert_eq!(text_of(&answer), "did click_element", "api_a's own « Oui » not seen: judged as api_b's call: {answer}");
    proxy.stop_child().await;
}

/// Review 3 of subtasks-redesign (20/11, head 357a87a0c): a self-install's bridge.toml has no `[hermes] root` (only a
/// shared Hermes writes one, installer stack.py), so `hermes_says_call_session` answers « yes » without looking, and
/// every task of an agent with a browser gets a browser of its own (a copy of its logins, the memory wait) on a
/// person's own Hermes without patch 0028, where no call says its session and the task browses in the agent's anyway.
/// agent_shares.rs `guest_turns_closed` looks in `<home>/hermes-agent`, the official install's folder.
#[test]
fn review3_an_own_hermes_without_patch_0028_gives_no_task_browsers() {
    let dir = tempfile::tempdir().unwrap();
    let code = dir.path().join("hermes-agent").join("tools");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(code.join("mcp_tool_handlers.py"), "def _make_tool_handler(server_name, tool_name, tool_timeout):\n    pass\n").unwrap();
    let mut config = bipbridge::config::parse_config(
        &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().join("data").to_string_lossy()}),
        None,
    )
    .unwrap();
    config.hermes.home = dir.path().to_string_lossy().into_owned();
    assert!(config.hermes.root.is_empty(), "a self-install: no [hermes] root");
    assert!(
        !bipbridge::browsers::hermes_says_call_session(&config),
        "its Hermes (<home>/hermes-agent) has no patch 0028: no task browser"
    );
}

/// A stealth server answering its calls side by side (each in a thread, 0.4 s each), writing when each call ran
/// (`calls.log`: `<name> <start> <end>`) to the file named by its first argument.
const CONCURRENT_SERVER: &str = r#"
import json, sys, threading, time
log, name = sys.argv[1], sys.argv[2]
out = threading.Lock()
def answer(m, r):
    with out:
        print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
def call(m):
    start = time.time()
    time.sleep(0.4)
    with out, open(log, "a") as f:
        f.write(f"{name} {start} {time.time()}\n")
    answer(m, {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]})
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        answer(m, {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
                   "serverInfo": {"name": "stealth", "version": "1"}})
    elif m["method"] == "tools/list":
        answer(m, {"tools": [{"name": "navigate", "inputSchema": {"type": "object"}}]})
    else:
        threading.Thread(target=call, args=(m,)).start()
"#;

/// Lab of 20/11 (the places' real browser image): the stealth server (nodriver) is not safe with calls overlapping on
/// one Chrome — `KeyError: FrameStoppedLoading` in its handler table, a navigate answered with the other call's page.
/// Since Hermes patch 0029 sends the calls of several sessions side by side, the agent's own browser takes them one at
/// a time again (as before 0029); the tasks' own browsers, one Chrome each, still run side by side.
#[tokio::test(flavor = "multi_thread")]
async fn the_agents_browser_takes_its_calls_one_at_a_time_the_tasks_browsers_side_by_side() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    let place = |name: &str| {
        let folder = dir.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let mut settings = config(&folder);
        std::fs::write(folder.join("server.py"), CONCURRENT_SERVER).unwrap();
        settings.insert(
            "server".into(),
            json!([python(), folder.join("server.py").to_string_lossy(), log.to_string_lossy(), name]),
        );
        std::fs::write(folder.join("mcp.json"), Value::Object(settings.clone()).to_string()).unwrap();
        (folder, settings)
    };
    let (agent, agent_settings) = place("vie");
    let (t1, _) = place("vie--t1");
    let (t2, _) = place("vie--t2");
    let sessions = json!({"api_t1": t1.join("mcp.json").to_string_lossy(), "api_t2": t2.join("mcp.json").to_string_lossy()});
    std::fs::write(agent.join(bipbridge::browsers::SESSIONS_FILE), sessions.to_string()).unwrap();
    let call = |id: i64, session: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "navigate", "arguments": {},
               "_meta": {bipbridge::agent_tools::CALL_SESSION_META: session}}})
    };
    let lines: String = [init(), call(1, "api_a"), call(2, "api_b"), call(3, "api_c"), call(4, "api_t1"), call(5, "api_t2")]
        .iter()
        .map(|m| format!("{m}\n"))
        .collect();
    let out = Out::default();
    let proxy = Proxy::new(agent_settings, Box::new(out.clone()));
    proxy.serve(lines.as_bytes()).await;
    let answered: Vec<i64> = out.messages().iter().filter(|m| m["result"]["content"].is_array()).map(|m| m["id"].as_i64().unwrap()).collect();
    assert_eq!(answered.len(), 5, "every call answered: {:?}", out.messages());
    let ran: Vec<(String, f64, f64)> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| {
            let p: Vec<&str> = l.split(' ').collect();
            (p[0].to_string(), p[1].parse().unwrap(), p[2].parse().unwrap())
        })
        .collect();
    let of = |name: &str| ran.iter().filter(|r| r.0 == name).map(|r| (r.1, r.2)).collect::<Vec<_>>();
    let mut own = of("vie");
    own.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert_eq!(own.len(), 3);
    for pair in own.windows(2) {
        assert!(pair[1].0 >= pair[0].1 - 0.01, "two calls on the agent's own Chrome at once: {own:?}");
    }
    let (a, b) = (of("vie--t1")[0], of("vie--t2")[0]);
    assert!(a.0 < b.1 && b.0 < a.1, "the tasks' own browsers waited for each other: {a:?} {b:?}");
}

/// A stealth server where one call never comes back (a page's `alert()` or a script's promise that never settles holds
/// nodriver's evaluate; a wedged CDP connection): `hang` is never answered, every other call at once.
const HANGING_SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "navigate", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call" and m["params"]["name"] == "hang":
        continue
    else:
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

/// Review 5 (d834ce434, `Proxy::one_call`): the agent's browser takes one call at a time, and nothing bounds how long
/// one call holds it. A call the stealth server never answers keeps the lock for ever (`request` waits on its oneshot
/// with no deadline; `calls` stays 1, so `watch_idle` never stops the server either): every later call of every
/// conversation of that agent (crons, Live, the person's own) queues behind it until Hermes restarts, each one timing
/// out in Hermes at 120 s and still queued afterwards. Before d834ce434 only the stuck call was lost.
/// Expected: a call is bounded (here the settings' `call_seconds`, 2 s; in production about Hermes' own 120 s tool
/// timeout, `browsers.rs` `proxy_config`): the stuck call is answered plainly (the server restarted if need be) and
/// the next session's call is served.
#[tokio::test(flavor = "multi_thread")]
async fn review5_a_call_that_never_comes_back_does_not_hold_the_agents_browser_for_ever() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut settings = config(tmp);
    std::fs::write(tmp.join("hang.py"), HANGING_SERVER).unwrap();
    settings.insert("server".into(), json!([python(), tmp.join("hang.py").to_string_lossy()]));
    settings.insert("call_seconds".into(), json!(2));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    let call = |id: i64, name: &str, session: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": {},
               "_meta": {bipbridge::agent_tools::CALL_SESSION_META: session}}})
    };
    let stuck = tokio::spawn({
        let proxy = proxy.clone();
        let message = call(1, "hang", "api_a");
        async move { proxy.handle(message).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let next = tokio::spawn({
        let proxy = proxy.clone();
        let message = call(2, "navigate", "api_b");
        async move { proxy.handle(message).await }
    });
    let answered = |id: i64| out.messages().iter().any(|m| m["id"] == json!(id));
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !(answered(1) && answered(2)) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let messages = out.messages();
    stuck.abort();
    next.abort();
    proxy.stop_child().await;
    assert!(answered(2), "another conversation's call waited for ever behind the stuck one: {messages:?}");
    assert!(
        messages.iter().any(|m| m["id"] == json!(2) && m["result"]["content"][0]["text"] == json!("did navigate")),
        "the next call served: {messages:?}"
    );
    assert!(answered(1), "the stuck call was never answered: {messages:?}");
}

/// Review 6: after a call ran over, `stop_child` ends only `podman exec`; in an image before review 6 the server's
/// door (hold_gate.py) waits for ever on its stuck server, its call count at 1 (card payments never start again). The
/// proxy then ends them in the container (its `exec` prefix) and clears the counts; a call Hermes cancelled while it
/// waited its turn never starts.
#[tokio::test(flavor = "multi_thread")]
async fn after_an_overrun_the_stuck_door_is_ended_in_the_container_and_a_cancelled_waiting_call_never_runs() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut settings = config(tmp);
    std::fs::write(tmp.join("hang.py"), HANGING_SERVER).unwrap();
    settings.insert("server".into(), json!([python(), tmp.join("hang.py").to_string_lossy()]));
    settings.insert("call_seconds".into(), json!(2));
    // the container's command prefix: what it is asked to run is written down
    let ran = tmp.join("in-container.txt");
    std::fs::write(tmp.join("record.py"), format!("import sys\nopen({:?}, 'a').write(' '.join(sys.argv[1:]) + '\\n')\n", ran.to_string_lossy())).unwrap();
    settings.insert("exec".into(), json!([python(), tmp.join("record.py").to_string_lossy()]));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    let call = |id: i64, name: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": {},
               "_meta": {bipbridge::agent_tools::CALL_SESSION_META: "api_a"}}})
    };
    let stuck = tokio::spawn({
        let proxy = proxy.clone();
        let message = call(1, "hang");
        async move { proxy.handle(message).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let queued = tokio::spawn({
        let proxy = proxy.clone();
        let message = call(2, "navigate");
        async move { proxy.handle(message).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    // the person sent a new message: Hermes cancels the call still waiting its turn
    proxy.handle(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2, "reason": "new message"}})).await;
    let _ = tokio::time::timeout(Duration::from_secs(20), stuck).await;
    let _ = tokio::time::timeout(Duration::from_secs(20), queued).await;
    let messages = out.messages();
    proxy.stop_child().await;
    assert!(messages.iter().any(|m| m["id"] == json!(1)), "the stuck call answered: {messages:?}");
    assert!(!messages.iter().any(|m| m["id"] == json!(2)), "a cancelled call ran after the stuck one: {messages:?}");
    let asked = std::fs::read_to_string(&ran).unwrap_or_default();
    assert!(asked.contains("[h]old_gate.py") && asked.contains("/tmp/bipagents-calls/"), "the stuck door left in the container: {asked:?}");
}

/// A stealth server that writes down (`seen.log`) every `notifications/cancelled` it gets, with the ids of the calls
/// it is running at that moment; each call takes 1.5 s.
const CANCEL_LOG_SERVER: &str = r#"
import json, sys, threading, time
log = sys.argv[1]
out = threading.Lock()
running = set()
def answer(m, r):
    with out:
        print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
def call(m):
    running.add(m["id"])
    time.sleep(1.5)
    running.discard(m["id"])
    answer(m, {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]})
for line in sys.stdin:
    m = json.loads(line)
    if m.get("method") == "notifications/cancelled":
        with open(log, "a") as f:
            f.write(json.dumps({"cancelled": m["params"]["requestId"], "running": sorted(running)}) + "\n")
        continue
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        answer(m, {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
                   "serverInfo": {"name": "stealth", "version": "1"}})
    elif m["method"] == "tools/list":
        answer(m, {"tools": [{"name": "navigate", "inputSchema": {"type": "object"}}]})
    else:
        threading.Thread(target=call, args=(m,)).start()
"#;

/// Review 7 (cancellation path, 2cf58bc40): Hermes' MCP SDK 2.3.0 sends `notifications/cancelled` with ITS request
/// id whenever it abandons a call (the person's new message, its tool timeout: `cancel_on_abandon`, mcp/shared/
/// dispatcher.py). The proxy numbers its own requests to the real server (`next_id`) but passes the notification on
/// unchanged, so the id names whatever call of the real server has that number: the stealth server (SDK 1.30) cancels
/// that one — another conversation's call — and hold_gate.py ends ITS count (`calls.end(requestId)`), the count card
/// payments wait on (card_pay.rs `calls_check_command`). Expected: a cancellation reaches the real server only for
/// the call it names, in the real server's numbering (or not at all): Hermes cancelling requests that are not in
/// flight never cancels the call that is.
#[tokio::test(flavor = "multi_thread")]
async fn review7_hermes_cancelling_its_own_request_never_cancels_another_call_of_the_real_server() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let log = tmp.join("seen.log");
    let mut settings = config(tmp);
    std::fs::write(tmp.join("cancel.py"), CANCEL_LOG_SERVER).unwrap();
    settings.insert("server".into(), json!([python(), tmp.join("cancel.py").to_string_lossy(), log.to_string_lossy()]));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    // Hermes' call 40, in flight on the real server under the proxy's own number
    let running = tokio::spawn({
        let proxy = proxy.clone();
        let message = json!({"jsonrpc": "2.0", "id": 40, "method": "tools/call", "params": {"name": "navigate",
                             "arguments": {}, "_meta": {bipbridge::agent_tools::CALL_SESSION_META: "api_a"}}});
        async move { proxy.handle(message).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Hermes abandons requests of its own that are not in flight here (answered, or other sessions' it gave up on)
    for id in 1..=12 {
        proxy.handle(json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                            "params": {"requestId": id, "reason": "abandoned"}})).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), running).await;
    let messages = out.messages();
    proxy.stop_child().await;
    let seen: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let hit: Vec<&Value> = seen
        .iter()
        .filter(|s| s["running"].as_array().unwrap().iter().any(|r| r == &s["cancelled"]))
        .collect();
    assert!(hit.is_empty(), "Hermes' cancellations of other requests reached the real server and named its running call: {hit:?}");
    assert!(
        messages.iter().any(|m| m["id"] == json!(40) && m["result"]["content"][0]["text"] == json!("did navigate")),
        "{messages:?}"
    );
}

/// Review 7, the other half: Hermes cancelling the very call in flight reaches the real server under the proxy's
/// own id of that call (the one it is running), so the server stops it and hold_gate.py ends its count.
#[tokio::test(flavor = "multi_thread")]
async fn hermes_cancelling_the_call_in_flight_cancels_it_on_the_real_server_under_its_own_id() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let log = tmp.join("seen.log");
    let mut settings = config(tmp);
    std::fs::write(tmp.join("cancel.py"), CANCEL_LOG_SERVER).unwrap();
    settings.insert("server".into(), json!([python(), tmp.join("cancel.py").to_string_lossy(), log.to_string_lossy()]));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    let running = tokio::spawn({
        let proxy = proxy.clone();
        let message = json!({"jsonrpc": "2.0", "id": 40, "method": "tools/call", "params": {"name": "navigate",
                             "arguments": {}, "_meta": {bipbridge::agent_tools::CALL_SESSION_META: "api_a"}}});
        async move { proxy.handle(message).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    proxy.handle(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 40, "reason": "new message"}})).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), running).await;
    proxy.stop_child().await;
    let seen: Vec<Value> = std::fs::read_to_string(&log).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let named = &seen[0]["cancelled"];
    assert!(seen[0]["running"].as_array().unwrap().contains(named), "the cancellation named the call running: {seen:?}");
    assert_ne!(named, &json!(40), "Hermes' own id passed on as it is: {seen:?}");
}

/// A stealth server that writes down (`calls.log`) the name of every tools/call it receives, and answers at once.
const CALL_LOG_SERVER: &str = r#"
import json, sys
log = sys.argv[1]
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "navigate", "inputSchema": {"type": "object"}}]}
    else:
        with open(log, "a") as f:
            f.write(m["params"]["name"] + "\n")
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

/// Review 8 (d775376ee): a call is `in_flight` from the moment it takes its turn, but its requests to the real server
/// are recorded only when sent. Before that — its browser still starting (`ensure_server`: the container's start and
/// `ready`, up to `ready_seconds`; after every overrun restart), the guard's own looks — a cancellation from Hermes
/// finds no unanswered request and the call is no longer `waiting`: it is dropped, and the call then runs in full
/// although Hermes abandoned it (the person's new message). Expected: a call cancelled after it took its turn but
/// before it reached the real server never reaches it.
#[tokio::test(flavor = "multi_thread")]
async fn review8_a_call_cancelled_while_its_browser_starts_never_reaches_the_real_server() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let log = tmp.join("calls.log");
    let mut settings = config(tmp);
    std::fs::write(tmp.join("calllog.py"), CALL_LOG_SERVER).unwrap();
    settings.insert("server".into(), json!([python(), tmp.join("calllog.py").to_string_lossy(), log.to_string_lossy()]));
    let running = tmp.join("running").to_string_lossy().into_owned();
    // the container takes 1.5 s to start (a cold start, or the restart after an overrun)
    settings.insert("start".into(), json!(["sh", "-c", format!("sleep 1.5; touch {running}")]));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.stop_child().await;
    let _ = std::fs::remove_file(tmp.join("running"));
    let call = tokio::spawn({
        let proxy = proxy.clone();
        let message = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "navigate",
                             "arguments": {}, "_meta": {bipbridge::agent_tools::CALL_SESSION_META: "api_a"}}});
        async move { proxy.handle(message).await }
    });
    tokio::time::sleep(Duration::from_millis(400)).await; // it has its turn, its browser is starting
    proxy.handle(json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                        "params": {"requestId": 9, "reason": "the person sent a new message"}})).await;
    let _ = tokio::time::timeout(Duration::from_secs(15), call).await;
    let received = std::fs::read_to_string(&log).unwrap_or_default();
    let messages = out.messages();
    proxy.stop_child().await;
    assert!(
        !received.contains("navigate"),
        "the cancelled call still reached the real server once its browser had started: {received:?} {messages:?}"
    );
}

// --- a bot check that still blocks: handed to the person (antibot-guard, 22/11) -----------------------------------

/// The real server for bot checks: `navigate` answers its address and a title, `document.readyState` « complete »,
/// the guard's look at the page (`browser_mcp::CHECK_PROBE`) the text of `page.txt` beside it (an ordinary page
/// when there is none); every call written to `calls.jsonl`.
const CHECK_SERVER: &str = r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
ORDINARY = json.dumps({"title": "Accueil", "marks": [], "widgets": [], "inputs": 2, "length": 5000, "body": "",
                       "form": False})
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": []}
    else:
        name, args = m["params"]["name"], m["params"].get("arguments", {})
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": name, "arguments": args}) + "\n")
        script = args.get("script", "")
        if name == "execute_script" and script == "document.readyState":
            answer = {"success": True, "result": "complete", "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "execute_script" and "bipagents-bot-check" in script:
            path = os.path.join(here, "page.txt")
            page = open(path).read() if os.path.exists(path) else ORDINARY
            answer = {"success": True, "result": page, "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "execute_script":
            # a script's answer that only says the words of an interstitial (review 1 B2): never a check
            answer = {"success": True, "result": "Just a moment... Security verification", "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "list_tabs":
            r = {"content": [{"type": "text", "text": "Vérification de sécurité | Just a moment..."}]}
        elif name == "navigate":
            r = {"content": [{"type": "text", "text": json.dumps({"url": args.get("url"), "title": "page"})}]}
        else:
            r = {"content": [{"type": "text", "text": "did " + name}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

/// What the guard's look (`browser_mcp::check_probe_script`) answers on Cloudflare's interstitial.
const INTERSTITIAL: &str = r#"{"title": "Just a moment...", "marks": ["cf_chl_opt"], "widgets": [], "inputs": 0,
"length": 80, "body": "Vérification que vous êtes un humain", "form": false}"#;

/// The look on a page with a widget: `inputs` fields to type into, `length` characters of text, `form` when the
/// element the agent acts on sends the widget's form.
fn widget_page(kind: &str, inputs: u64, length: u64, form: bool) -> String {
    json!({"title": "Contact", "marks": [], "widgets": [kind], "inputs": inputs, "length": length, "body": "",
           "form": form})
    .to_string()
}

fn checking(tmp: &Path, clear: f64, wait: f64) -> Dict {
    std::fs::write(tmp.join("checks.py"), CHECK_SERVER).unwrap();
    let mut settings = config(tmp);
    settings.insert("server".into(), json!([python(), tmp.join("checks.py").to_string_lossy()]));
    settings.insert("agent".into(), json!("vie"));
    settings.insert("challenge_clear_seconds".into(), json!(clear));
    settings.insert("hand_wait_seconds".into(), json!(wait));
    settings
}

fn go(id: i64, url: &str) -> Value {
    tool(id, "navigate", json!({"instance_id": "i1", "url": url}))
}

fn said(out: &Out) -> Vec<String> {
    let last = out.messages().last().unwrap().clone();
    assert_eq!(last["result"].get("isError"), None, "never an error (Hermes' breaker): {last}");
    last["result"]["content"].as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap_or("").to_string()).collect()
}

fn navigations(tmp: &Path) -> Vec<String> {
    calls(tmp)
        .iter()
        .filter(|c| c["name"] == "navigate")
        .map(|c| c["arguments"]["url"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_check_that_stays_is_handed_to_the_person_and_its_site_is_not_opened_again() {
    // Decathlon, 22/11 (passation-pro/briefs/enquete-antibot-1122.md §3): without the phone exit nothing was said,
    // and nothing stopped a second try
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    std::fs::write(tmp.join("page.txt"), INTERSTITIAL).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(checking(tmp, 1.0, 2.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(go(1, "https://www.decathlon.fr/p/velo-st-100")).await;
    let lines = said(&out);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let stop = &lines[1];
    assert!(stop.starts_with(browser_mcp::CHECK_STOP_START), "{stop}");
    assert!(stop.contains("decathlon.fr") && stop.contains("Cloudflare challenge"), "{stop}");
    assert!(stop.contains("« Prendre la main »") && stop.contains("Don't try again"), "{stop}");
    for word in ["headless", "proxy", "captcha service"] {
        assert!(stop.contains(word), "{word}: {stop}");
    }
    // the bridge is told (its Boîte line, challenge_hand.rs), beside `busy`
    let note: Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.join(browser_mcp::CHECK_FILE)).unwrap()).unwrap();
    assert_eq!((note["site"].as_str(), note["kind"].as_str()), (Some("decathlon.fr"), Some("Cloudflare challenge")));
    assert!(note["at"].as_f64().unwrap() > now() - 60.0);
    // nobody came within the wait: the last note says the agent stopped waiting (the Boîte says so, review 1)
    assert_eq!((note["stopped"].as_bool(), note["phone"].as_bool()), (Some(true), Some(false)));
    // a second try at the site in this turn: refused, the real server never asked
    proxy.handle(go(2, "https://www.decathlon.fr/recherche?q=velo")).await;
    let lines = said(&out);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with(browser_mcp::CHECK_REFUSED_START) && lines[0].contains("decathlon.fr"), "{lines:?}");
    proxy.handle(tool(3, "reload_page", json!({"instance_id": "i1"}))).await;
    assert!(said(&out)[0].starts_with(browser_mcp::CHECK_REFUSED_START));
    proxy.handle(tool(4, "new_tab", json!({"instance_id": "i1", "url": "https://decathlon.fr/"}))).await;
    assert!(said(&out)[0].starts_with(browser_mcp::CHECK_REFUSED_START));
    assert_eq!(navigations(tmp), vec!["https://www.decathlon.fr/p/velo-st-100".to_string()]);
    // reading the page it is on goes on: no second card, no second wait
    std::fs::remove_file(tmp.join(browser_mcp::CHECK_FILE)).unwrap();
    let started = std::time::Instant::now();
    proxy.handle(tool(5, "get_page_content", json!({"instance_id": "i1"}))).await;
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(!tmp.join(browser_mcp::CHECK_FILE).exists());
    // another site: as ever
    std::fs::remove_file(tmp.join("page.txt")).unwrap();
    proxy.handle(go(6, "https://www.example.org/")).await;
    assert_eq!(said(&out).len(), 1);
    // the run over (the bridge touches `ended`): a later turn may open the site again
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(tmp.join("ended"), "").unwrap();
    proxy.handle(go(7, "https://www.decathlon.fr/p/velo-st-100")).await;
    assert_eq!(said(&out).len(), 1);
    assert_eq!(navigations(tmp).len(), 3);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_takes_over_passes_the_check_hands_back_and_the_agent_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().to_path_buf();
    std::fs::write(tmp.join("page.txt"), INTERSTITIAL).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(checking(&tmp, 0.0, 20.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    // the person: takes the hand (browsers.rs `hold` writes hold.token), passes the check, hands back
    let person = {
        let tmp = tmp.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            std::fs::write(tmp.join("hold.token"), "t").unwrap();
            tokio::time::sleep(Duration::from_millis(1500)).await;
            std::fs::remove_file(tmp.join("page.txt")).unwrap();
            std::fs::remove_file(tmp.join("hold.token")).unwrap();
        })
    };
    let started = std::time::Instant::now();
    proxy.handle(go(1, "https://www.decathlon.fr/p/velo-st-100")).await;
    person.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(15), "waited for the hand, not for the whole wait");
    let lines = said(&out);
    assert_eq!(lines.last().unwrap(), browser_mcp::CHECK_PASSED, "{lines:?}");
    // the site open again
    proxy.handle(go(2, "https://www.decathlon.fr/p/velo-st-100")).await;
    assert_eq!(said(&out).len(), 1);
    assert_eq!(navigations(&tmp).len(), 2);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_still_holding_or_the_check_still_there_after_the_hand_stops_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().to_path_buf();
    std::fs::write(tmp.join("page.txt"), INTERSTITIAL).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(checking(&tmp, 0.0, 2.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    std::fs::write(tmp.join("hold.token"), "t").unwrap();
    proxy.handle(go(1, "https://www.decathlon.fr/a")).await;
    assert_eq!(said(&out).last().unwrap(), &browser_mcp::check_held_text("decathlon.fr"));
    // handed back, the check still there
    std::fs::remove_file(tmp.join("hold.token")).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(checking(&tmp, 0.0, 10.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    let person = {
        let tmp = tmp.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(800)).await;
            std::fs::write(tmp.join("hold.token"), "t").unwrap();
            tokio::time::sleep(Duration::from_millis(1200)).await;
            std::fs::remove_file(tmp.join("hold.token")).unwrap();
        })
    };
    proxy.handle(go(2, "https://www.decathlon.fr/b")).await;
    person.await.unwrap();
    let last = said(&out).last().unwrap().clone();
    assert!(last.starts_with(browser_mcp::CHECK_STOP_START) && last.contains("still"), "{last}");
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_that_clears_by_itself_is_never_handed_over() {
    // the stealth browser gets past many checks by itself (Alex 22/11: « seulement s'il est bloqué quand même »)
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().to_path_buf();
    std::fs::write(tmp.join("page.txt"), INTERSTITIAL).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(checking(&tmp, 6.0, 30.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    let cleared = {
        let tmp = tmp.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            std::fs::remove_file(tmp.join("page.txt")).unwrap();
        })
    };
    let started = std::time::Instant::now();
    proxy.handle(go(1, "https://www.ameli.fr/assure")).await;
    cleared.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(6));
    let lines = said(&out);
    assert_eq!(lines.last().unwrap(), browser_mcp::CHECK_CLEARED, "{lines:?}");
    assert!(!tmp.join(browser_mcp::CHECK_FILE).exists());
    proxy.handle(go(2, "https://www.ameli.fr/assure")).await;
    assert_eq!(said(&out).len(), 1);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_pages_and_a_solved_widget_trigger_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let out = Out::default();
    let proxy = Proxy::new(checking(tmp, 1.0, 30.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    // what the look says of an ordinary page (DataDome's tag, Cloudflare's analytics and reCAPTCHA v3's badge give no
    // mark: `check_probe_script` looks for widgets shown and not answered), and of an article that says the words of an
    // interstitial in its text (review 1 B2: its title is an article's)
    let pages = [
        r#"{"title": "Annonces", "marks": [], "widgets": [], "inputs": 1, "length": 9000, "body": "", "form": false}"#,
        r#"{"title": "Accueil", "marks": [], "widgets": [], "inputs": 0, "length": 200, "body": "Bienvenue", "form": false}"#,
        r#"{"title": "Mon article", "marks": [], "widgets": [], "inputs": 0, "length": 900,
            "body": "Just a moment... Security verification, Vérification de sécurité", "form": false}"#,
    ];
    for (n, page) in pages.iter().enumerate() {
        std::fs::write(tmp.join("page.txt"), page).unwrap();
        let started = std::time::Instant::now();
        proxy.handle(go(n as i64 + 1, "https://www.leboncoin.fr/")).await;
        assert_eq!(said(&out).len(), 1, "{page}");
        assert!(started.elapsed() < Duration::from_secs(3), "{page}");
    }
    // a widget the page answers with its token already (the look says none is waiting): nothing either, even when
    // the answer's own text names it
    std::fs::write(tmp.join("page.txt"), widget_page("turnstile", 1, 900, false).replace("[\"turnstile\"]", "[]"))
        .unwrap();
    let mut settings = checking(tmp, 1.0, 30.0);
    let server = CHECK_SERVER.replace(
        r#"r = {"content": [{"type": "text", "text": json.dumps({"url": args.get("url"), "title": "page"})}]}"#,
        r#"r = {"content": [{"type": "text", "text": "<div class=\"cf-turnstile\"></div>"}]}"#,
    );
    std::fs::write(tmp.join("checks2.py"), server).unwrap();
    settings.insert("server".into(), json!([python(), tmp.join("checks2.py").to_string_lossy()]));
    let out = Out::default();
    let solved = Proxy::new(settings, Box::new(out.clone()));
    solved.handle(init()).await;
    solved.handle(go(9, "https://www.example.org/login")).await;
    assert_eq!(said(&out).len(), 1);
    assert!(!tmp.join(browser_mcp::CHECK_FILE).exists());
    proxy.stop_child().await;
    solved.stop_child().await;
}

/// Review 1 of antibot-guard, B1: a captcha box on a page the agent only reads (a comment, contact or sign-in form's)
/// is said, never handed over: no wait, no line in the Boîte, the site not refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_captcha_on_a_page_the_agent_reads_is_said_never_handed_over() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let out = Out::default();
    let proxy = Proxy::new(checking(tmp, 5.0, 30.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    for (n, kind) in ["recaptcha", "hcaptcha", "turnstile"].iter().enumerate() {
        std::fs::write(tmp.join("page.txt"), widget_page(kind, 3, 4200, false)).unwrap();
        let started = std::time::Instant::now();
        proxy.handle(go(n as i64 + 1, "https://www.example-blog.fr/article")).await;
        assert!(started.elapsed() < Duration::from_secs(3), "{kind}: never waited");
        let lines = said(&out);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].starts_with("A captcha (") && lines[1].contains("never answer it yourself"), "{lines:?}");
        assert!(!lines[1].starts_with(browser_mcp::CHECK_STOP_START));
    }
    assert!(!tmp.join(browser_mcp::CHECK_FILE).exists(), "nothing for the person");
    // reading it again, typing into its fields: as ever
    proxy.handle(tool(9, "type_text", json!({"instance_id": "i1", "selector": "#message", "text": "Bonjour"}))).await;
    assert!(!said(&out)[0].starts_with(browser_mcp::CHECK_REFUSED_START));
    assert_eq!(navigations(tmp).len(), 3);
    proxy.stop_child().await;
}

/// B1: sending the form a captcha guards, the captcha not answered: not sent (the click never reaches the page), the
/// person asked; a widget alone on its page is a wall, handed over from its load.
#[tokio::test(flavor = "multi_thread")]
async fn sending_a_form_whose_captcha_is_not_answered_is_handed_over_and_a_lone_widget_is_a_wall() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let out = Out::default();
    let proxy = Proxy::new(checking(tmp, 0.0, 1.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    std::fs::write(tmp.join("page.txt"), widget_page("hcaptcha", 3, 4200, false)).unwrap();
    proxy.handle(go(1, "https://www.example-shop.fr/contact")).await;
    assert_eq!(said(&out).len(), 2);
    // the agent sends it: the look before its click says the button sends the widget's form
    std::fs::write(tmp.join("page.txt"), widget_page("hcaptcha", 3, 4200, true)).unwrap();
    proxy.handle(tool(2, "click_element", json!({"instance_id": "i1", "selector": "form button"}))).await;
    let lines = said(&out);
    assert!(lines[0].starts_with(browser_mcp::CHECK_FORM_START) && lines[0].contains("hCaptcha"), "{lines:?}");
    assert!(lines[0].contains(browser_mcp::CHECK_STOP_START), "{lines:?}");
    assert!(!calls(tmp).iter().any(|c| c["name"] == "click_element"), "the click never reached the page");
    let note: Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.join(browser_mcp::CHECK_FILE)).unwrap()).unwrap();
    assert_eq!(note["kind"].as_str(), Some("hCaptcha"));
    // Enter in one of its fields: the same, not pressed
    proxy.handle(tool(3, "press_key", json!({"instance_id": "i1", "key": "Enter"}))).await;
    assert!(said(&out)[0].starts_with(browser_mcp::CHECK_REFUSED_START));
    // another site whose page is only a widget (a « verify you are human » page): handed over from its load
    std::fs::remove_file(tmp.join(browser_mcp::CHECK_FILE)).unwrap();
    std::fs::write(tmp.join("page.txt"), widget_page("turnstile", 0, 120, false)).unwrap();
    proxy.handle(go(4, "https://www.example-wall.fr/")).await;
    let lines = said(&out);
    assert!(lines[1].starts_with(browser_mcp::CHECK_STOP_START) && lines[1].contains("Cloudflare Turnstile"), "{lines:?}");
    // a click on a page whose form has no captcha left: as ever
    std::fs::write(tmp.join("page.txt"), widget_page("hcaptcha", 3, 4200, false).replace("[\"hcaptcha\"]", "[]"))
        .unwrap();
    std::fs::write(tmp.join("ended"), "").unwrap();
    proxy.handle(tool(5, "click_element", json!({"instance_id": "i1", "selector": "form button"}))).await;
    assert_eq!(said(&out), vec!["did click_element".to_string()]);
    proxy.stop_child().await;
}

/// B2: only a tool that loads or reads a page is looked at; a script's answer or the tabs' list that say the words of
/// an interstitial are never a check.
#[tokio::test(flavor = "multi_thread")]
async fn other_tools_answers_are_never_read_as_a_check() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let out = Out::default();
    let proxy = Proxy::new(checking(tmp, 5.0, 30.0), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(go(1, "https://www.example.org/")).await;
    let started = std::time::Instant::now();
    proxy.handle(tool(2, "execute_script", json!({"instance_id": "i1", "script": "document.title"}))).await;
    proxy.handle(tool(3, "list_tabs", json!({}))).await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(said(&out).len(), 1);
    assert!(!tmp.join(browser_mcp::CHECK_FILE).exists());
    proxy.stop_child().await;
}

/// R1: the wait ends before Hermes gives up on the call (its timeout, less a margin, from when Hermes sent it), and the
/// other conversations' calls meanwhile hear that a check waits for the person.
#[tokio::test(flavor = "multi_thread")]
async fn the_wait_ends_before_hermes_gives_up_and_the_others_hear_why() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    std::fs::write(tmp.join("page.txt"), INTERSTITIAL).unwrap();
    let mut settings = checking(tmp, 0.0, 600.0);
    settings.insert("hermes_timeout_seconds".into(), json!(browser_mcp::HERMES_MARGIN + 3.0));
    settings.insert("call_seconds".into(), json!(1.0));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    let started = std::time::Instant::now();
    let first = {
        let proxy = proxy.clone();
        tokio::spawn(async move { proxy.handle(go(1, "https://www.decathlon.fr/a")).await })
    };
    tokio::time::sleep(Duration::from_millis(800)).await;
    proxy.handle(go(2, "https://www.example.org/")).await;
    let busy = out.messages().iter().find(|m| m["id"] == json!(2)).cloned().unwrap();
    assert!(busy.to_string().contains("waiting for your person to pass a bot check"), "{busy}");
    first.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let last = out.messages().iter().rev().find(|m| m["id"] == json!(1)).cloned().unwrap();
    assert!(last.to_string().contains(browser_mcp::CHECK_STOP_START), "{last}");
    proxy.stop_child().await;
}

/// R4: the site a check is kept for is the registrable name on the Public Suffix List, its private section included.
#[test]
fn a_checks_site_is_its_registrable_name() {
    assert_eq!(browser_mcp::check_site("www.decathlon.fr"), "decathlon.fr");
    assert_eq!(browser_mcp::check_site("shop.example.co.uk"), "example.co.uk");
    assert_eq!(browser_mcp::check_site("myshop.pages.dev"), "myshop.pages.dev");
    assert_eq!(browser_mcp::check_site("alex.github.io"), "alex.github.io");
    assert_eq!(browser_mcp::check_site("boutique.myshopify.com"), "boutique.myshopify.com");
    assert_eq!(browser_mcp::check_site("127.0.0.1"), "127.0.0.1");
}

/// The look's JSON read (review 1): a whole page by its marks or its title only, a widget present or blocking.
#[test]
fn the_looks_verdict() {
    use browser_mcp::{probe_verdict, Verdict};
    assert_eq!(probe_verdict(INTERSTITIAL), Some(Verdict::Blocking("Cloudflare challenge")));
    let titled = r#"{"title": "Vérification de sécurité", "marks": [], "widgets": [], "inputs": 0, "length": 50}"#;
    assert_eq!(probe_verdict(titled), Some(Verdict::Blocking("Cloudflare challenge")));
    let worded = r#"{"title": "Blog", "marks": [], "widgets": [], "inputs": 0, "length": 50, "body": "Just a moment..."}"#;
    assert_eq!(probe_verdict(worded), Some(Verdict::Clear));
    let datadome = r#"{"title": "leboncoin", "marks": ["captcha-delivery.com"], "widgets": []}"#;
    assert_eq!(probe_verdict(datadome), Some(Verdict::Blocking("DataDome")));
    assert_eq!(probe_verdict(&widget_page("recaptcha", 2, 3000, false)), Some(Verdict::Present("reCAPTCHA")));
    assert_eq!(probe_verdict(&widget_page("recaptcha", 2, 3000, true)), Some(Verdict::Blocking("reCAPTCHA")));
    assert_eq!(probe_verdict(&widget_page("turnstile", 0, 300, false)), Some(Verdict::Blocking("Cloudflare Turnstile")));
    assert_eq!(probe_verdict(&widget_page("turnstile", 0, 5000, false)), Some(Verdict::Present("Cloudflare Turnstile")));
    assert_eq!(probe_verdict("<title>Just a moment...</title>"), None);
    let script = browser_mcp::check_probe_script(browser_mcp::Acted::Selector("form \"x\" button"));
    assert!(script.contains("bipagents-bot-check") && script.contains(r#"d.querySelector("form \"x\" button")"#));
    assert!(browser_mcp::check_probe_script(browser_mcp::Acted::Focused { enter: true }).contains("const submit = true ||"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tasks_own_browser_stops_at_once_nobody_to_hand_it_to() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    std::fs::write(tmp.join("page.txt"), INTERSTITIAL).unwrap();
    let mut settings = checking(tmp, 0.0, 30.0);
    settings.insert("agent".into(), json!("vie--t1"));
    settings.insert("task".into(), json!({"agent": "vie", "job": "t1"}));
    let out = Out::default();
    let proxy = Proxy::new(settings, Box::new(out.clone()));
    proxy.handle(init()).await;
    let started = std::time::Instant::now();
    proxy.handle(go(1, "https://www.decathlon.fr/p/velo")).await;
    assert!(started.elapsed() < Duration::from_secs(5), "no wait");
    let last = said(&out).last().unwrap().clone();
    assert!(last.starts_with(browser_mcp::CHECK_STOP_START) && last.contains("report"), "{last}");
    assert!(!tmp.join(browser_mcp::CHECK_FILE).exists());
    proxy.handle(go(2, "https://www.decathlon.fr/p/velo")).await;
    assert!(said(&out)[0].starts_with(browser_mcp::CHECK_REFUSED_START));
    proxy.stop_child().await;
}
