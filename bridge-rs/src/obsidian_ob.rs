//! Obsidian Headless (`ob`, npm `obsidian-headless`, Obsidian's own client of Obsidian Sync) run by the bridge for the
//! person's Obsidian connection (obsidian.rs). Facts read from the package itself (0.0.14, its README and `cli.js`):
//!
//! - `ob login` asks « Email: », « Password: » and, when the account has it, « 2FA code: » on its terminal (or takes
//!   `--email`/`--password`/`--mfa` on its command line, which we never use for the password: a command line is
//!   readable by the machine's other processes). Without a terminal it reads its whole standard input for the first
//!   question only, so a second question (the code) would wait forever: hence a **pseudo-terminal** of our own
//!   ([`Dialog`], `openpty`), the answers typed when each question shows, the password dropped from memory as soon as
//!   it is typed. It keeps its session token in `$XDG_CONFIG_HOME/obsidian-headless/auth_token` (0600), here
//!   `<data>/obsidian/state/.config/…`, never anywhere else; `ob logout` ends it.
//! - `ob sync-list-remote --json` → `{vaults: [{id, name, region}], shared: [...]}`.
//! - `ob sync-setup --vault <id> --path <dir> --device-name <name>` asks « End-to-end encryption password: » for an
//!   end-to-end encrypted vault (same terminal), then says « Vault configured successfully! », or « Failed to
//!   validate password. » / « Setup failed: ». It keeps the vault's key under `…/obsidian-headless/sync/<id>/`.
//! - `ob sync --path <dir>` syncs once, both ways (`--continuous` would keep a process per vault: not used, the bridge
//!   syncs on a schedule and after each write instead); « Another sync instance is already running » when two meet.
//! - `ob sync-config --path <dir> --configs <list>`: which of the vault's settings come too (only the core plugins'
//!   here: the daily notes' folder and format; never the community plugins' code).
//! - `ob sync-unlink --path <dir>` forgets the vault's key; `ob logout`.
//!
//! **Where it runs**: Node ≥ 22 is not on the places (nor in the agents' sandbox image), and the package's licence is
//! « UNLICENSED » (Obsidian's), so it is never in our images nor our repository: the place builds its own small image
//! the first time the person connects (`prepare`: `node:22-bookworm-slim` + `npm install -g obsidian-headless@<pin>`,
//! as the person would install it), `localhost/bipagents-obsidian:<pin>`, and runs each `ob` command in a short-lived
//! container of it (rootless podman, as the connectors' runs: no capabilities, read-only root, small limits), with
//! only two folders: its state and the vault's copy. `[obsidian] command = ["ob"]` runs an `ob` already installed
//! instead (a self-install that has Node), and the tests run a fake one the same way.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Notify;
use zeroize::Zeroizing;

/// The version of Obsidian Headless the bridge installs (read before each change: its questions are matched here).
pub const PACKAGE_VERSION: &str = "0.0.14";
pub const IMAGE: &str = "localhost/bipagents-obsidian:0.0.14";
/// What this machine's copy of the vault is called in Obsidian's version history.
pub const DEVICE_NAME: &str = "BipAgents";
/// The settings that come with the vault: the core plugins' (daily notes, templates), never the community plugins'.
pub const CONFIGS: &str = "core-plugin,core-plugin-data";
const CONTAINERFILE: &str = "FROM docker.io/library/node:22-bookworm-slim\n\
RUN npm install -g --no-fund --no-audit obsidian-headless@0.0.14 && ob --help > /dev/null\n\
ENTRYPOINT [\"ob\"]\n";
/// How long the image may take to build (a node image and one npm package).
const PREPARE_TIMEOUT: Duration = Duration::from_secs(900);
/// A question that waits for the person (a code, a vault's password): how long its `ob` is kept waiting.
pub const FLOW_SECONDS: u64 = 300;
/// One question's wait (the service answers a sign-in in a second or two).
const STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// A command without questions (a list, the settings).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
/// A sync (the first one brings the whole vault).
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(600);
pub const FIRST_SYNC_TIMEOUT: Duration = Duration::from_secs(1800);
const OUTPUT_KEPT: usize = 64 * 1024;

pub const Q_EMAIL: &str = "Email:";
pub const Q_PASSWORD: &str = "Password:";
pub const Q_CODE: &str = "2FA code:";
pub const Q_VAULT_PASSWORD: &str = "End-to-end encryption password:";

/// The two folders an `ob` run gets.
#[derive(Debug, Clone)]
pub struct Paths {
    pub state: PathBuf,
    pub vault: PathBuf,
}

/// How `ob` is started here: in its container, or a command given in the settings.
#[derive(Debug, Clone, Default)]
pub struct Runner {
    /// Empty: podman and `IMAGE`.
    pub command: Vec<String>,
    pub podman: String,
}

impl Runner {
    pub fn of(settings: &crate::obsidian::Settings) -> Runner {
        Runner { command: settings.command.clone(), podman: settings.podman.clone() }
    }

    fn contained(&self) -> bool {
        self.command.is_empty()
    }

    /// The command line of `ob <args>`: `{vault}` in an argument is the vault's folder as `ob` sees it.
    pub fn argv(&self, paths: &Paths, tty: bool, args: &[&str]) -> Vec<String> {
        self.argv_named(paths, tty, args, "bip-obsidian")
    }

    /// `argv` with the container's name (`name`: so a run killed on the bridge's side is removed too: a killed
    /// `podman run` leaves its container running).
    pub fn argv_named(&self, paths: &Paths, tty: bool, args: &[&str], name: &str) -> Vec<String> {
        let vault_seen = if self.contained() { "/vault".to_string() } else { paths.vault.to_string_lossy().into_owned() };
        let args = args.iter().map(|a| a.replace("{vault}", &vault_seen));
        if !self.contained() {
            return self.command.iter().cloned().chain(args).collect();
        }
        let mut argv: Vec<String> =
            vec![self.podman.clone(), "run".into(), "--rm".into(), "-i".into(), format!("--name={name}")];
        if tty {
            argv.push("-t".into());
        }
        for a in [
            "--cap-drop=all",
            "--security-opt=no-new-privileges",
            "--read-only",
            "--tmpfs=/tmp:rw,size=64m",
            "--memory=768m",
            "--pids-limit=256",
            "--log-driver=none",
            "-e",
            "HOME=/state",
            "-e",
            "XDG_CONFIG_HOME=/state/.config",
            "-w",
            "/vault",
        ] {
            argv.push(a.into());
        }
        argv.push("-v".into());
        argv.push(format!("{}:/state", paths.state.display()));
        argv.push("-v".into());
        argv.push(format!("{}:/vault", paths.vault.display()));
        argv.push(IMAGE.into());
        argv.extend(args);
        argv
    }

    /// The variables of a run: a direct command keeps its state in the same folder.
    fn env(&self, paths: &Paths) -> Vec<(String, String)> {
        if self.contained() {
            return Vec::new();
        }
        vec![
            ("HOME".into(), paths.state.to_string_lossy().into_owned()),
            ("XDG_CONFIG_HOME".into(), paths.state.join(".config").to_string_lossy().into_owned()),
        ]
    }
}

// -- the image ---------------------------------------------------------------------------------------------------------

/// Where the image stands on this place.
#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Missing,
    Preparing,
    Ready,
    Failed(String),
}

impl Tool {
    pub fn word(&self) -> &'static str {
        match self {
            Tool::Missing => "missing",
            Tool::Preparing => "preparing",
            Tool::Ready => "ready",
            Tool::Failed(_) => "failed",
        }
    }
}

static TOOL: LazyLock<Mutex<HashMap<String, Tool>>> = LazyLock::new(Default::default);

fn tools() -> std::sync::MutexGuard<'static, HashMap<String, Tool>> {
    TOOL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The image's state for this place (`place`: its data folder), as last seen.
pub fn tool_state(place: &str, runner: &Runner) -> Tool {
    if !runner.contained() {
        return Tool::Ready;
    }
    tools().get(place).cloned().unwrap_or(Tool::Missing)
}

/// Checks whether the image is there (at the start, and before a sign-in).
pub async fn check(place: &str, runner: &Runner) -> Tool {
    if !runner.contained() {
        return Tool::Ready;
    }
    if matches!(tool_state(place, runner), Tool::Preparing) {
        return Tool::Preparing;
    }
    let there = tokio::process::Command::new(&runner.podman)
        .args(["image", "exists", IMAGE])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    let mut table = tools();
    let now = if there {
        Tool::Ready
    } else {
        match table.get(place) {
            Some(Tool::Failed(why)) => Tool::Failed(why.clone()),
            _ => Tool::Missing,
        }
    };
    table.insert(place.to_string(), now.clone());
    now
}

/// Marks the image as being prepared (false: it already is).
pub fn mark_preparing(place: &str) -> bool {
    let mut table = tools();
    if matches!(table.get(place), Some(Tool::Preparing)) {
        return false;
    }
    table.insert(place.to_string(), Tool::Preparing);
    true
}

/// Builds the image (`mark_preparing` first; in the background: the app shows « Préparation… »). Its state after.
pub async fn prepare_marked(place: String, runner: Runner, work: PathBuf) -> Tool {
    let built = build(&runner, &work).await;
    let now = match built {
        Ok(()) => Tool::Ready,
        Err(why) => {
            tracing::warn!(target: "bipbridge::obsidian", error = %why, "Obsidian's tool not prepared");
            Tool::Failed(why)
        }
    };
    tools().insert(place, now.clone());
    now
}

async fn build(runner: &Runner, work: &Path) -> Result<(), String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    std::fs::write(work.join("Containerfile"), CONTAINERFILE).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(&runner.podman);
    command
        .args(["build", "--pull=missing", "-t", IMAGE, "-f"])
        .arg(work.join("Containerfile"))
        .arg(work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().map_err(|e| format!("podman can't start ({e})"))?;
    match tokio::time::timeout(PREPARE_TIMEOUT, child.wait_with_output()).await {
        Err(_) => Err("the preparation took too long".into()),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => Err(tail(&String::from_utf8_lossy(&out.stderr), 400)),
    }
}

fn tail(text: &str, chars: usize) -> String {
    let all: Vec<char> = text.trim().chars().collect();
    all[all.len().saturating_sub(chars)..].iter().collect()
}

// -- a command without questions ---------------------------------------------------------------------------------------

/// What a run of `ob` gave.
#[derive(Debug, Clone)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// `ob <args>`, no terminal, its standard input closed.
pub async fn run(runner: &Runner, paths: &Paths, args: &[&str], timeout: Duration) -> Result<Ran, String> {
    let name = container_name();
    let argv = runner.argv_named(paths, false, args, &name);
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .envs(runner.env(paths))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().map_err(|e| format!("Obsidian's tool can't start ({e})"))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => {
            runner.remove(&name).await;
            Err("Obsidian's tool didn't answer in time".into())
        }
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(out)) => Ok(Ran {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: tail(&String::from_utf8_lossy(&out.stderr), 2000),
        }),
    }
}

// -- a command with questions: a pseudo-terminal -----------------------------------------------------------------------

/// How a wait for a question ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Waited {
    /// The n-th of the texts waited for showed.
    Found(usize),
    /// `ob` ended: whether it succeeded, what it said since the last question.
    Ended(bool, String),
    Timeout,
}

/// One `ob` run on a pseudo-terminal of ours: what it writes is read as it comes, an answer is typed when its question
/// shows.
/// A run's container name, unique.
fn container_name() -> String {
    format!("bip-obsidian-{}", crate::py::token_hex(6))
}

impl Runner {
    /// The container `name` gone (a run stopped by the bridge: its `podman run` killed, the container not).
    pub async fn remove(&self, name: &str) {
        if !self.contained() {
            return;
        }
        let _ = tokio::process::Command::new(&self.podman)
            .args(["rm", "-f", "-t", "2", name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

pub struct Dialog {
    child: tokio::process::Child,
    writer: std::fs::File,
    out: Arc<Mutex<String>>,
    notify: Arc<Notify>,
    seen: usize,
    runner: Runner,
    name: String,
}

fn openpty() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty writes two descriptors it opened; no name, settings or size given.
    let done = unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null())
    };
    if done != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors are ours alone from here.
    Ok(unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) })
}

impl Dialog {
    /// `ob <args>` started on a new pseudo-terminal.
    pub fn start(runner: &Runner, paths: &Paths, args: &[&str]) -> std::io::Result<Dialog> {
        let name = container_name();
        let argv = runner.argv_named(paths, true, args, &name);
        let (master, slave) = openpty()?;
        let mut command = tokio::process::Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .envs(runner.env(paths))
            .env("TERM", "dumb")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave))
            .kill_on_drop(true);
        // SAFETY: only async-signal-safe calls between fork and exec: a new session whose terminal is the pty.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let child = command.spawn()?;
        let reader_fd = master.try_clone()?;
        let writer = std::fs::File::from(master);
        let out = Arc::new(Mutex::new(String::new()));
        let notify = Arc::new(Notify::new());
        let (o, n) = (out.clone(), notify.clone());
        std::thread::spawn(move || {
            let mut file = std::fs::File::from(reader_fd);
            let mut buffer = [0u8; 4096];
            loop {
                match file.read(&mut buffer) {
                    Ok(0) | Err(_) => break, // EIO once the terminal's last user is gone
                    Ok(n_read) => {
                        let mut text = o.lock().unwrap_or_else(|e| e.into_inner());
                        text.push_str(&String::from_utf8_lossy(&buffer[..n_read]));
                        if text.len() > OUTPUT_KEPT * 2 {
                            let cut = text.len() - OUTPUT_KEPT;
                            let cut = (cut..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
                            text.drain(..cut);
                        }
                    }
                }
                n.notify_waiters();
            }
            n.notify_waiters();
        });
        Ok(Dialog { child, writer, out, notify, seen: 0, runner: runner.clone(), name })
    }

    fn text(&self) -> String {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Waits until one of `texts` shows after what was already read, or `ob` ends, or `timeout`.
    pub async fn wait_for(&mut self, texts: &[&str], timeout: Duration) -> Waited {
        let deadline = Instant::now() + timeout;
        loop {
            let wake = self.notify.notified();
            let all = self.text();
            let fresh = all.get(self.seen.min(all.len())..).unwrap_or("");
            let first = texts.iter().enumerate().filter_map(|(i, t)| fresh.find(t).map(|at| (at, i, t.len()))).min();
            if let Some((at, i, len)) = first {
                self.seen = self.seen.min(all.len()) + at + len;
                return Waited::Found(i);
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                tokio::time::sleep(Duration::from_millis(100)).await; // its last words
                let all = self.text();
                let fresh = all.get(self.seen.min(all.len())..).unwrap_or("").to_string();
                return Waited::Ended(status.success(), fresh);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Waited::Timeout;
            }
            let _ = tokio::time::timeout(left.min(Duration::from_millis(250)), wake).await;
        }
    }

    /// Types `answer` then Enter, apart (`ob` reads its terminal raw and takes Enter only as a key of its own).
    pub async fn answer(&mut self, answer: &str) -> std::io::Result<()> {
        tokio::time::sleep(Duration::from_millis(150)).await; // its terminal switched to raw after the question
        self.writer.write_all(answer.as_bytes())?;
        self.writer.flush()?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.writer.write_all(b"\r")?;
        self.writer.flush()
    }

    /// What `ob` said since the last question, cut.
    pub fn said(&self) -> String {
        let all = self.text();
        tail(all.get(self.seen.min(all.len())..).unwrap_or(""), 600)
    }

    pub async fn stop(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
        self.runner.remove(&self.name).await;
        self.name.clear();
    }
}

impl Drop for Dialog {
    /// A dialog dropped without `stop` (a sign-in nobody finished): its container removed in the background.
    fn drop(&mut self) {
        if self.name.is_empty() || !self.runner.contained() {
            return;
        }
        let (podman, name) = (self.runner.podman.clone(), std::mem::take(&mut self.name));
        std::thread::spawn(move || {
            let _ = std::process::Command::new(podman)
                .args(["rm", "-f", "-t", "2", &name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
    }
}

// -- the steps ---------------------------------------------------------------------------------------------------------

/// Where a sign-in or a vault's setup stands.
pub enum Step {
    /// Signed in as (name, e-mail).
    SignedIn(String, String),
    /// The vault is set up on this place.
    Configured,
    /// `ob` waits for the person's code (sign-in) or the vault's password (setup).
    Waiting(Box<Dialog>),
    /// Refused: a code (`wrong_login`, `wrong_code`, `wrong_vault_password`, `not_found`, `failed`) and `ob`'s words.
    Refused(&'static str, String),
}

/// « Logged in as Alex (alex@example.org) » → (name, e-mail).
pub fn signed_in_as(text: &str) -> Option<(String, String)> {
    let line = text.lines().find_map(|l| l.split("Logged in as ").nth(1))?.trim();
    let (name, email) = line.rsplit_once(" (")?;
    Some((name.trim().to_string(), email.trim_end_matches(')').trim().to_string()))
}

/// What a refused sign-in's words mean.
fn refusal_of(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if lower.contains("2fa code is incorrect") || lower.contains("2fa") && lower.contains("incorrect") {
        "wrong_code"
    } else if lower.contains("login failed") || lower.contains("password") || lower.contains("credential") {
        "wrong_login"
    } else {
        "failed"
    }
}

/// `ob login`: the e-mail, then the password (`password` dropped once typed), then the code when asked and given.
pub async fn login(runner: &Runner, paths: &Paths, email: &str, password: Zeroizing<String>, code: Option<&str>) -> Step {
    // a session of another account would be kept by `ob login`: ended first
    let _ = run(runner, paths, &["logout"], COMMAND_TIMEOUT).await;
    let mut dialog = match Dialog::start(runner, paths, &["login"]) {
        Ok(d) => d,
        Err(e) => return Step::Refused("failed", format!("Obsidian's tool can't start ({e})")),
    };
    for (question, answer) in [(Q_EMAIL, email), (Q_PASSWORD, password.as_str())] {
        match dialog.wait_for(&[question], STEP_TIMEOUT).await {
            Waited::Found(_) => {
                if let Err(e) = dialog.answer(answer).await {
                    dialog.stop().await;
                    return Step::Refused("failed", e.to_string());
                }
            }
            Waited::Ended(_, said) => return Step::Refused(refusal_of(&said), said),
            Waited::Timeout => {
                dialog.stop().await;
                return Step::Refused("failed", "Obsidian didn't answer in time".into());
            }
        }
    }
    drop(password);
    login_result(dialog, code).await
}

/// After the password (or the code): signed in, refused, or the code asked.
pub async fn login_result(mut dialog: Dialog, code: Option<&str>) -> Step {
    let mut code = code.map(str::to_string);
    loop {
        match dialog.wait_for(&[Q_CODE, "Logged in as", "Login failed"], STEP_TIMEOUT).await {
            Waited::Found(0) => match code.take() {
                Some(c) => {
                    if let Err(e) = dialog.answer(c.trim()).await {
                        dialog.stop().await;
                        return Step::Refused("failed", e.to_string());
                    }
                }
                None => return Step::Waiting(Box::new(dialog)),
            },
            Waited::Found(1) => {
                let _ = dialog.wait_for(&["\n"], Duration::from_secs(5)).await;
                let text = dialog.text();
                let found = text.rsplit("Logged in as ").next().map(|rest| format!("Logged in as {rest}"));
                let (name, email) = found.as_deref().and_then(signed_in_as).unwrap_or_default();
                dialog.stop().await;
                return Step::SignedIn(name, email);
            }
            Waited::Found(_) => {
                let _ = dialog.wait_for(&["\n"], Duration::from_secs(5)).await;
                let said = dialog.said();
                dialog.stop().await;
                return Step::Refused(refusal_of(&format!("Login failed {said}")), said);
            }
            Waited::Ended(_, said) => return Step::Refused(refusal_of(&said), said),
            Waited::Timeout => {
                dialog.stop().await;
                return Step::Refused("failed", "Obsidian didn't answer in time".into());
            }
        }
    }
}

/// The code typed into a waiting sign-in.
pub async fn login_code(mut dialog: Dialog, code: &str) -> Step {
    if let Err(e) = dialog.answer(code.trim()).await {
        dialog.stop().await;
        return Step::Refused("failed", e.to_string());
    }
    login_result(dialog, None).await
}

/// One remote vault.
#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub shared: bool,
}

/// The vaults of the account signed in.
pub async fn vaults(runner: &Runner, paths: &Paths) -> Result<Vec<Remote>, String> {
    let ran = run(runner, paths, &["sync-list-remote", "--json"], COMMAND_TIMEOUT).await?;
    if !ran.ok {
        return Err(if ran.stderr.contains("No account logged in") { "signed_out".into() } else { ran.stderr });
    }
    parse_vaults(&ran.stdout).ok_or_else(|| "Obsidian's list of vaults can't be read".to_string())
}

pub fn parse_vaults(text: &str) -> Option<Vec<Remote>> {
    let data: Value = serde_json::from_str(text.trim()).ok()?;
    let mut out = Vec::new();
    for (key, shared) in [("vaults", false), ("shared", true)] {
        for v in data.get(key).and_then(Value::as_array).cloned().unwrap_or_default() {
            let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            if !text("id").is_empty() {
                out.push(Remote { id: text("id"), name: text("name"), shared });
            }
        }
    }
    Some(out)
}

/// `ob sync-setup` for `vault`: configured, refused, or its password asked.
pub async fn setup(runner: &Runner, paths: &Paths, vault: &str, password: Option<Zeroizing<String>>) -> Step {
    let args = ["sync-setup", "--vault", vault, "--path", "{vault}", "--device-name", DEVICE_NAME];
    let dialog = match Dialog::start(runner, paths, &args) {
        Ok(d) => d,
        Err(e) => return Step::Refused("failed", format!("Obsidian's tool can't start ({e})")),
    };
    setup_result(dialog, password).await
}

pub async fn setup_result(mut dialog: Dialog, mut password: Option<Zeroizing<String>>) -> Step {
    loop {
        let texts = [Q_VAULT_PASSWORD, "configured successfully", "Failed to validate password", "not found", "failed"];
        match dialog.wait_for(&texts, STEP_TIMEOUT).await {
            Waited::Found(0) => match password.take() {
                Some(p) => {
                    if let Err(e) = dialog.answer(p.as_str()).await {
                        dialog.stop().await;
                        return Step::Refused("failed", e.to_string());
                    }
                }
                None => return Step::Waiting(Box::new(dialog)),
            },
            Waited::Found(1) => {
                let _ = dialog.wait_for(&["sync"], Duration::from_secs(10)).await;
                dialog.stop().await;
                return Step::Configured;
            }
            Waited::Found(2) => {
                dialog.stop().await;
                return Step::Refused("wrong_vault_password", String::new());
            }
            Waited::Found(3) => {
                dialog.stop().await;
                return Step::Refused("not_found", String::new());
            }
            Waited::Found(_) | Waited::Ended(false, _) => {
                let said = dialog.said();
                dialog.stop().await;
                return Step::Refused("failed", said);
            }
            Waited::Ended(true, _) => return Step::Configured,
            Waited::Timeout => {
                dialog.stop().await;
                return Step::Refused("failed", "Obsidian didn't answer in time".into());
            }
        }
    }
}

/// The vault's password typed into a waiting setup.
pub async fn setup_password(mut dialog: Dialog, password: Zeroizing<String>) -> Step {
    if let Err(e) = dialog.answer(password.as_str()).await {
        dialog.stop().await;
        return Step::Refused("failed", e.to_string());
    }
    drop(password);
    setup_result(dialog, None).await
}

/// Only the core plugins' settings come with the vault (the daily notes' folder and format).
pub async fn configure(runner: &Runner, paths: &Paths) -> Result<(), String> {
    let ran = run(runner, paths, &["sync-config", "--path", "{vault}", "--configs", CONFIGS], COMMAND_TIMEOUT).await?;
    if ran.ok {
        Ok(())
    } else {
        Err(ran.stderr)
    }
}

/// How a sync ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Synced {
    Done,
    /// The account's session ended (« No account logged in »): the person signs in again.
    SignedOut,
    /// Another sync of the vault is running.
    Busy,
    Failed(String),
}

/// `ob sync` once.
pub async fn sync(runner: &Runner, paths: &Paths, timeout: Duration) -> Synced {
    match run(runner, paths, &["sync", "--path", "{vault}"], timeout).await {
        Err(why) => Synced::Failed(why),
        Ok(ran) if ran.ok => Synced::Done,
        Ok(ran) => {
            let said = format!("{}\n{}", ran.stdout, ran.stderr);
            if said.contains("No account logged in") || said.contains("Run \"ob login\"") {
                Synced::SignedOut
            } else if said.contains("already running") {
                Synced::Busy
            } else {
                Synced::Failed(tail(&said, 300))
            }
        }
    }
}

/// The vault forgotten on this place and the account signed out (`ob sync-unlink`, `ob logout`).
pub async fn disconnect(runner: &Runner, paths: &Paths) {
    let _ = run(runner, paths, &["sync-unlink", "--path", "{vault}"], COMMAND_TIMEOUT).await;
    let _ = run(runner, paths, &["logout"], COMMAND_TIMEOUT).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_container_gets_only_its_two_folders_and_the_password_is_never_on_the_line() {
        let paths = Paths { state: "/d/obsidian/state".into(), vault: "/d/obsidian/vault".into() };
        let runner = Runner { command: Vec::new(), podman: "podman".into() };
        let argv = runner.argv(&paths, true, &["sync", "--path", "{vault}"]);
        assert_eq!(&argv[..6], ["podman", "run", "--rm", "-i", "--name=bip-obsidian", "-t"]);
        assert!(argv.contains(&"/d/obsidian/state:/state".to_string()));
        assert!(argv.contains(&"/d/obsidian/vault:/vault".to_string()));
        assert!(argv.contains(&"--cap-drop=all".to_string()) && argv.contains(&"--read-only".to_string()));
        assert_eq!(argv[argv.len() - 4..], [IMAGE, "sync", "--path", "/vault"]);
        assert_eq!(argv.iter().filter(|a| a.starts_with("-v")).count(), 2);
        let direct = Runner { command: vec!["ob".into()], podman: "podman".into() };
        assert_eq!(direct.argv(&paths, false, &["sync", "--path", "{vault}"]), ["ob", "sync", "--path", "/d/obsidian/vault"]);
        let env = direct.env(&paths);
        assert!(env.contains(&("XDG_CONFIG_HOME".into(), "/d/obsidian/state/.config".into())));
    }

    #[test]
    fn obsidians_words_are_read() {
        assert_eq!(signed_in_as("Logged in as Alex B (alex@example.org)\n"), Some(("Alex B".into(), "alex@example.org".into())));
        assert_eq!(signed_in_as("nothing"), None);
        assert_eq!(refusal_of("Login failed: Ye { error: '2FA code is incorrect' }"), "wrong_code");
        assert_eq!(refusal_of("Login failed: Ye { error: 'Login failed, please check your email and password' }"), "wrong_login");
        assert_eq!(refusal_of("ECONNREFUSED"), "failed");
        let list = r#"{"vaults":[{"id":"v1","name":"Perso","region":"eu"}],"shared":[{"id":"v2","name":"Famille","region":"eu"}]}"#;
        assert_eq!(parse_vaults(list).unwrap(), vec![
            Remote { id: "v1".into(), name: "Perso".into(), shared: false },
            Remote { id: "v2".into(), name: "Famille".into(), shared: true },
        ]);
        assert!(parse_vaults("Fetching vaults...").is_none());
    }

    /// Obsidian's real `ob` in the place's image asks its questions on our terminal (run on a machine with podman and
    /// the image: `cargo test --lib obsidian_ob -- --ignored`). Stops at the password: nothing reaches Obsidian.
    #[tokio::test]
    #[ignore = "needs podman and localhost/bipagents-obsidian:0.0.14"]
    async fn the_real_ob_asks_its_questions_on_our_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { state: dir.path().join("state"), vault: dir.path().join("vault") };
        std::fs::create_dir_all(&paths.state).unwrap();
        std::fs::create_dir_all(&paths.vault).unwrap();
        let runner = Runner { command: Vec::new(), podman: "podman".into() };
        let mut dialog = Dialog::start(&runner, &paths, &["login"]).unwrap();
        assert_eq!(dialog.wait_for(&[Q_EMAIL], Duration::from_secs(60)).await, Waited::Found(0), "{}", dialog.text());
        dialog.answer("nobody@example.invalid").await.unwrap();
        assert_eq!(dialog.wait_for(&[Q_PASSWORD], Duration::from_secs(30)).await, Waited::Found(0), "{}", dialog.text());
        dialog.stop().await;
        let left = std::process::Command::new("podman")
            .args(["ps", "-a", "--filter", "name=bip-obsidian-", "--format", "{{.Names}}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&left.stdout).trim(), "", "a stopped dialog leaves no container");
        let listed = run(&runner, &paths, &["sync-list-remote", "--json"], Duration::from_secs(60)).await.unwrap();
        assert!(!listed.ok && listed.stderr.contains("No account logged in"), "{listed:?}");
    }
}
