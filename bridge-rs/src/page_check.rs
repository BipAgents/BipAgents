//! The layout check of an agent's page (Alex 20/11: cards on two thirds of the phone, tags cut at the card's edge, a
//! plan's boxes reading « Bas… »). Right after `create_page` saves a page, the bridge opens it in a headless Chromium
//! at a phone's width (390 px) and a tablet's (820 px), measures from the DOM what a person would see broken —
//! something past the screen's edge, text cut off, text on top of other text, an empty block, a script error — and
//! says it in the tool's result, so the agent fixes it in the same turn. No model call; the page is saved and shown
//! whatever the check finds; at most one « fix it » per page within [`RETRY_WINDOW`]; skipped when no Chromium is
//! there (a self-hosted bridge without the agents' browser) or when it takes longer than [`DEADLINE`].
//!
//! Chromium, the first found: `BIP_PAGE_CHROME` (a command run like Chromium, e.g. the build server's bip-chrome
//! wrapper), one on the host, else the agents' browser image (`[browser] image`) in a throwaway container with no
//! network at all. Each width is an iframe of a small frame page of ours (one browser, one load for both); the page's
//! copy carries `assets/page_check.js`, which posts its findings to the frame; `--dump-dom` prints them.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::Config;

/// The script the checked copy carries first in its head.
pub const SCRIPT: &str = include_str!("../assets/page_check.js");
/// The widths a page is checked at: an iPhone's (390 px; most phones are 360–430) and an iPad's.
pub const WIDTHS: [u32; 2] = [390, 820];
/// The whole check, browser start included; past it, the page goes without (Alex: under 2 s).
pub const DEADLINE: Duration = Duration::from_millis(2500);
/// One « fix it and call again » per page in this time; after it, the problems are only named.
pub const RETRY_WINDOW: f64 = 15.0 * 60.0;
/// A runner that failed (no image, a crash) is not tried again for this long.
const FAILED_PAUSE: Duration = Duration::from_secs(600);

/// What one width's measure found (page_check.js `out`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Found {
    pub width: u32,
    /// The document's width (more than `width`: the page scrolls sideways).
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub wide: Vec<String>,
    #[serde(default)]
    pub cut: Vec<String>,
    #[serde(default)]
    pub overlap: Vec<String>,
    #[serde(default)]
    pub empty: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// How Chromium is started.
#[derive(Debug, Clone, PartialEq)]
pub enum Runner {
    /// A Chromium binary, or a command run like one (`BIP_PAGE_CHROME`).
    Binary(String),
    /// The agents' browser image, with podman or docker.
    Image { engine: String, image: String },
}

const HOST_BROWSERS: [&str; 5] = ["chromium", "chromium-browser", "google-chrome-stable", "google-chrome", "chrome"];
const MAC_BROWSERS: [&str; 3] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
];

/// The runner for this bridge, None: no check here. `BIP_PAGE_CHECK=off` turns the check off.
pub fn runner(config: &Config) -> Option<Runner> {
    if std::env::var("BIP_PAGE_CHECK").is_ok_and(|v| matches!(v.as_str(), "off" | "0" | "no")) {
        return None;
    }
    if let Ok(command) = std::env::var("BIP_PAGE_CHROME") {
        if !command.trim().is_empty() {
            return Some(Runner::Binary(command));
        }
    }
    let engine = || crate::browsers::runtime(config);
    if let Ok(image) = std::env::var("BIP_PAGE_CHECK_IMAGE") {
        return engine().map(|engine| Runner::Image { engine, image });
    }
    for name in HOST_BROWSERS {
        if let Some(path) = crate::browsers::pyx::which(name) {
            return Some(Runner::Binary(path));
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(path) = MAC_BROWSERS.iter().find(|p| Path::new(p).exists()) {
            return Some(Runner::Binary(path.to_string()));
        }
    }
    if config.browser.enabled && cfg!(target_os = "linux") {
        return engine().map(|engine| Runner::Image { engine, image: config.browser.image.clone() });
    }
    None
}

static FAILED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The page's copy for the check: the script first in its head (before the kit and the page's own scripts, so their
/// errors are heard), and the call last (after the kit's and the page's own drawing at DOMContentLoaded).
pub fn checked_copy(locked: &str) -> String {
    let script = format!("<script>{}</script>", SCRIPT.replace("</script", "<\\/script"));
    let head = &*crate::pages::HEAD;
    let mut copy = match locked.find(head.as_str()) {
        Some(at) => format!("{}{script}{}", &locked[..at + head.len()], &locked[at + head.len()..]),
        None => format!("{script}{locked}"),
    };
    copy.push_str("\n<script>document.addEventListener(\"DOMContentLoaded\",function(){window.__bipCheck&&__bipCheck()})</script>\n");
    copy
}

/// The frame page: one iframe per width, their findings written in `<pre id="bip-check">` once all came.
pub fn frame() -> String {
    let frames: String = WIDTHS
        .iter()
        .map(|w| format!("<iframe src=\"p.html\" style=\"width:{w}px;height:900px;border:0\"></iframe>"))
        .collect();
    format!(
        "<!doctype html><meta charset=\"utf-8\"><body style=\"margin:0\"><pre id=\"bip-check\"></pre>{frames}<script>\
var got=[];addEventListener(\"message\",function(e){{got.push(e.data);document.getElementById(\"bip-check\")\
.textContent=JSON.stringify(got)}})</script>"
    )
}

/// Chromium's arguments for the frame at `url`, its profile in `profile`.
fn chrome_args(profile: &Path, url: &str) -> Vec<String> {
    [
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--disable-crash-reporter",
        "--disable-breakpad",
        "--no-first-run",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-sync",
        "--mute-audio",
        // nothing reaches the network, whatever the page asks (it has its own policy too)
        "--proxy-server=socks5://127.0.0.1:9",
        "--host-resolver-rules=MAP * ~NOTFOUND",
        "--window-size=1300,1000",
        "--virtual-time-budget=1000",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([format!("--user-data-dir={}", profile.display()), "--dump-dom".into(), url.to_string()])
    .collect()
}

/// The command for `runner`, the check's folder `dir` (the frame and the page in `dir/c`, the profile `dir`).
pub fn command(runner: &Runner, dir: &Path, name: &str) -> Command {
    let url = format!("file://{}/c/f.html", dir.display());
    match runner {
        Runner::Binary(binary) => {
            let mut c = Command::new(binary);
            c.args(chrome_args(dir, &url));
            c
        }
        Runner::Image { engine, image } => {
            let podman = Path::new(engine).file_name().is_some_and(|n| n == "podman");
            let dir_s = dir.display().to_string();
            let mut c = Command::new(engine);
            c.args(["run", "--rm", "--pull=never", "--name", name, "--network", "none", "--cap-drop", "ALL"]);
            c.args(["--security-opt", "no-new-privileges", "--memory", "1g", "--shm-size", "256m", "-e", "HOME=/tmp"]);
            if podman {
                // the image runs as its user bip (1000): the host user is them (as browsers.rs)
                c.args(["--userns", "keep-id:uid=1000,gid=1000"]);
            } else {
                // SAFETY: getuid/getgid cannot fail.
                let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
                c.args(["--user", &format!("{uid}:{gid}")]);
            }
            // the image's own policies refuse file:// pages (browser/policies.json): none in this throwaway one
            c.args(["-v", &format!("{dir_s}/empty:/etc/opt/chrome/policies/managed:ro,Z")]);
            c.args(["-v", &format!("{dir_s}/empty:/etc/chromium/policies/managed:ro,Z")]);
            // the pages read only; the profile in the container's own /tmp, gone with it
            c.args(["-v", &format!("{dir_s}/c:{dir_s}/c:ro,Z"), "--entrypoint", "sh", image, "-c"]);
            c.arg("exec \"$(command -v google-chrome-stable || command -v chromium-browser || command -v chromium)\" \"$@\"");
            c.arg("sh");
            c.args(chrome_args(Path::new("/tmp/profile"), &url));
            c
        }
    }
}

/// What Chromium printed: the findings in the frame's `<pre id="bip-check">` (HTML-escaped text).
pub fn parse(dump: &str) -> Option<Vec<Found>> {
    let start = dump.find("<pre id=\"bip-check\">")? + "<pre id=\"bip-check\">".len();
    let end = start + dump[start..].find("</pre>")?;
    let text =
        dump[start..end].replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", "\u{a0}").replace("&amp;", "&");
    let mut found: Vec<Found> = serde_json::from_str(&text).ok()?;
    found.sort_by_key(|f| f.width);
    Some(found)
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The check of a page as kept (`locked`). None: not checked (no Chromium, too slow, it failed).
pub fn check(config: &Config, locked: &str) -> Option<Vec<Found>> {
    let runner = runner(config)?;
    let key = format!("{runner:?}");
    if FAILED.lock().ok()?.get(&key).is_some_and(|at| at.elapsed() < FAILED_PAUSE) {
        return None;
    }
    let started = Instant::now();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("bip-page-check-{}-{n}", std::process::id());
    let dir = std::env::temp_dir().join(&name);
    let result = run(&runner, &dir, &name, locked);
    let _ = std::fs::remove_dir_all(&dir);
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(found) => {
            tracing::info!(ms = ms, widths = found.len(), "page layout checked");
            Some(found)
        }
        Err(Skip::Slow) => {
            tracing::info!(ms = ms, "page layout check: too slow, skipped");
            None
        }
        Err(Skip::Broken) => {
            // no Chromium after all (no image, it crashed): not tried again for a while
            tracing::info!(ms = ms, runner = %key, "page layout check: the browser did not answer, skipped");
            if let Ok(mut failed) = FAILED.lock() {
                failed.insert(key, Instant::now());
            }
            None
        }
    }
}

enum Skip {
    Slow,
    Broken,
}

fn run(runner: &Runner, dir: &Path, name: &str, locked: &str) -> Result<Vec<Found>, Skip> {
    let pages: PathBuf = dir.join("c");
    let written = std::fs::create_dir_all(&pages)
        .and_then(|_| std::fs::create_dir_all(dir.join("empty")))
        .and_then(|_| std::fs::write(pages.join("p.html"), checked_copy(locked)))
        .and_then(|_| std::fs::write(pages.join("f.html"), frame()));
    written.map_err(|_| Skip::Broken)?;
    let mut child = command(runner, dir, name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Skip::Broken)?;
    let mut out = child.stdout.take().ok_or(Skip::Broken)?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + DEADLINE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                if let Runner::Image { engine, .. } = runner {
                    let _ = Command::new(engine)
                        .args(["rm", "-f", name])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                return Err(Skip::Slow);
            }
        }
    }
    let dump = reader.join().map_err(|_| Skip::Broken)?;
    parse(&dump).filter(|found| !found.is_empty()).ok_or(Skip::Broken)
}

/// The problems in plain words, one width after the other (a problem seen at both widths said once, at the first).
pub fn problems(found: &[Found]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    for f in found {
        let w = f.width;
        let mut add = |kind: &str, item: &str| {
            // the same element at both widths (its width aside): once
            let same =
                item.trim_end_matches(" px").trim_end_matches(|c: char| c.is_ascii_digit()).trim_end().to_string();
            if !said.contains(&same) {
                said.push(same);
                lines.push(format!("{kind} at {w} px: {item}"));
            }
        };
        for item in &f.wide {
            add("past the screen's edge", item);
        }
        if f.wide.is_empty() && f.page > w + 2 {
            add("the page scrolls sideways", &format!("{} px wide", f.page));
        }
        for item in &f.cut {
            add("text cut off", item);
        }
        for item in &f.overlap {
            add("text on top of other text", item);
        }
        for item in &f.empty {
            add("empty block", item);
        }
        for item in &f.errors {
            add("script error", item);
        }
    }
    lines.truncate(8);
    lines
}

static ASKED: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The note for the tool's result ("" when the page is fine or was not checked). `now`: seconds.
pub fn note(agent: &str, page: &str, found: Option<&[Found]>, now: f64) -> String {
    let Some(found) = found else { return String::new() };
    let lines = problems(found);
    let key = format!("{agent}/{page}");
    let mut asked = match ASKED.lock() {
        Ok(asked) => asked,
        Err(_) => return String::new(),
    };
    if lines.is_empty() {
        asked.remove(&key);
        return String::new();
    }
    let list = lines.join("; ");
    if asked.get(&key).is_some_and(|at| now - at < RETRY_WINDOW) {
        return format!("\nLayout check: still {list}. The page is shown as it is; leave it unless the person asks.");
    }
    asked.insert(key, now);
    format!(
        "\nLayout check (the page is shown already): {list}. Fix it now: let text wrap (no white-space:nowrap, fixed \
widths or overflow:hidden on words), stack boxes that don't fit side by side, then call create_page again with page \
\"{page}\" (once)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(width: u32) -> Found {
        Found { width, page: width, ..Default::default() }
    }

    #[test]
    fn the_findings_are_read_from_the_dump() {
        let dump = "<!DOCTYPE html><html><head></head><body><pre id=\"bip-check\">[{\"width\":820,\"page\":820},\
{\"width\":390,\"page\":697,\"wide\":[\"&lt;span.chip&gt; « Digue &amp; plage » 666 px\"]}]</pre><iframe></iframe></body></html>";
        let got = parse(dump).unwrap();
        assert_eq!(got.iter().map(|f| f.width).collect::<Vec<_>>(), vec![390, 820]);
        assert_eq!(got[0].wide, vec!["<span.chip> « Digue & plage » 666 px".to_string()]);
        assert!(parse("<pre id=\"bip-check\"></pre>").is_none());
        assert!(parse("nothing").is_none());
    }

    #[test]
    fn a_fine_page_gets_no_note_and_an_unchecked_one_neither() {
        assert_eq!(note("a", "fine", Some(&[found(390), found(820)]), 0.0), "");
        assert_eq!(note("a", "fine", None, 0.0), "");
        assert_eq!(note("a", "fine", Some(&[]), 0.0), "");
    }

    #[test]
    fn a_broken_page_asks_for_one_fix_then_only_names_what_is_left() {
        let mut phone = found(390);
        phone.page = 697;
        phone.wide = vec!["<span.chip> « Digue et plage » 666 px".into()];
        phone.cut = vec!["<div.box> « Bas » ".into()];
        let mut tablet = found(820);
        tablet.cut = phone.cut.clone();
        tablet.errors = vec!["Uncaught ReferenceError: x is not defined".into()];
        let both = [phone, tablet];
        let first = note("coach", "cote-belge", Some(&both), 1000.0);
        assert!(first.contains("past the screen's edge at 390 px: <span.chip> « Digue et plage » 666 px"), "{first}");
        assert!(first.contains("text cut off at 390 px"), "{first}");
        assert!(!first.contains("text cut off at 820 px"), "said once: {first}");
        assert!(first.contains("script error at 820 px"), "{first}");
        assert!(first.contains("call create_page again with page \"cote-belge\""), "{first}");
        // the retry still broken: named, never asked again
        let second = note("coach", "cote-belge", Some(&both), 1100.0);
        assert!(second.contains("still") && !second.contains("call create_page"), "{second}");
        // another page of the same agent: its own retry
        assert!(note("coach", "menus", Some(&both), 1100.0).contains("call create_page"));
        // fixed: forgotten, a later break asks again
        assert_eq!(note("coach", "cote-belge", Some(&[found(390)]), 1200.0), "");
        assert!(note("coach", "cote-belge", Some(&both), 1300.0).contains("call create_page"));
    }

    #[test]
    fn a_page_wider_than_the_screen_with_no_culprit_still_says_it_scrolls() {
        let mut phone = found(390);
        phone.page = 450;
        assert_eq!(problems(&[phone]), vec!["the page scrolls sideways at 390 px: 450 px wide".to_string()]);
    }

    #[test]
    fn the_checked_copy_hears_errors_before_the_kit_and_measures_after_it() {
        let (locked, _) = crate::pages::lock_with("<h1>Hi</h1><script>Bip.bars('#c',{})</script>", "Hi", None);
        let copy = checked_copy(&locked);
        let at = |t: &str| copy.find(t).unwrap_or_else(|| panic!("{t} missing"));
        assert!(at("Content-Security-Policy") < at("window.__bipCheck = check"), "the policy first, always");
        assert!(at("window.__bipCheck = check") < at("id=\"bip-kit-js\""));
        assert!(at("Bip.bars") < copy.rfind("__bipCheck()").unwrap());
        assert!(!locked.contains("__bipCheck"), "the person's page never carries it");
    }

    #[test]
    fn the_container_has_no_network_and_no_file_policy() {
        let dir = Path::new("/tmp/bip-page-check-1-0");
        let c =
            command(&Runner::Image { engine: "/usr/bin/podman".into(), image: "bipagents-browser".into() }, dir, "n");
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let joined = args.join(" ");
        assert!(joined.contains("--network none"), "{joined}");
        assert!(joined.contains("--pull=never") && joined.contains("--rm"));
        assert!(joined.contains("/tmp/bip-page-check-1-0/empty:/etc/opt/chrome/policies/managed:ro,Z"));
        assert!(joined.contains("--proxy-server=socks5://127.0.0.1:9"));
        assert!(joined.ends_with("--dump-dom file:///tmp/bip-page-check-1-0/c/f.html"), "{joined}");
    }

    /// With a real Chromium (the build server: `BIP_PAGE_CHECK_IMAGE=<the agents' browser image>`, or
    /// `BIP_PAGE_CHROME`): Alex's page of 20/11 as a model wrote it (its own pills that never wrap, its own boxes that
    /// cut their words) is caught at 390 px, the same page in the kit's parts passes, each check within the deadline.
    #[test]
    fn a_real_browser_catches_the_broken_trip_page() {
        if std::env::var("BIP_PAGE_CHECK_IMAGE").is_err() && std::env::var("BIP_PAGE_CHROME").is_err() {
            eprintln!("no BIP_PAGE_CHECK_IMAGE / BIP_PAGE_CHROME: skipped");
            return;
        }
        let mut config = Config::with_key("k".repeat(40));
        config.browser.enabled = true;
        let broken = "<html lang=\"fr\"><style>.pills{display:flex;gap:6px}.pill{white-space:nowrap;padding:4px 10px;\
border-radius:99px;background:var(--tint)}.steps{display:flex;gap:8px}.steps div{flex:1;min-width:0;overflow:hidden;\
white-space:nowrap;text-overflow:ellipsis;background:var(--tint);padding:8px}</style><h1>Côte belge en train</h1>\
<div class=\"card\"><div class=\"pills\"><span class=\"pill\">Digue et plage (longues balades en hiver)</span><span \
class=\"pill\">Mu.ZEE (musée d'art moderne, gratuit le premier dimanche)</span></div></div><div class=\"steps\">\
<div>Bastogne</div><div>J1 Bruxelles-Midi</div><div>Ostende (via Courtrai)</div><div>J2 La Panne</div></div>\
<script>missing()</script>";
        let fixed = "<html lang=\"fr\"><h1>Côte belge en train</h1><div class=\"card\"><div class=\"row\"><span \
class=\"chip\">Digue et plage (longues balades en hiver)</span><span class=\"chip\">Mu.ZEE (musée d'art moderne, \
gratuit le premier dimanche)</span></div></div><div class=\"flow\"><div>Bastogne</div><div>J1 Bruxelles-Midi</div>\
<div>Ostende (via Courtrai)</div><div>J2 La Panne</div></div><pre class=\"diagram\">Bastogne -> J1 Bruxelles-Midi
J1 Bruxelles-Midi -> Ostende (via Courtrai)
J1 Bruxelles-Midi -> Bruges (arrêt déjeuner)
J1 Bruxelles-Midi -> Gand (Saint-Bavon)</pre>";
        for (html, bad) in [(broken, true), (fixed, false)] {
            let (locked, _) = crate::pages::lock_with(html, "Côte belge", Some("home"));
            let started = Instant::now();
            let found = check(&config, &locked).expect("checked");
            let took = started.elapsed();
            let lines = problems(&found);
            eprintln!("{took:?} {lines:#?}");
            assert!(took < Duration::from_secs(2), "{took:?}");
            assert_eq!(found.iter().map(|f| f.width).collect::<Vec<_>>(), WIDTHS.to_vec());
            if bad {
                let all = lines.join("\n");
                assert!(all.contains("past the screen's edge at 390 px: <span.pill> « Mu.ZEE"), "{all}");
                assert!(!all.contains("820 px: <span.pill>"), "said once: {all}");
                assert!(all.contains("text cut off at 390 px: <div> « Ostende (via Courtrai) »"), "{all}");
                assert!(all.contains("script error"), "{all}");
            } else {
                assert!(lines.is_empty(), "{lines:#?}");
            }
        }
    }

    #[test]
    fn the_frame_holds_one_iframe_per_width() {
        let f = frame();
        for w in WIDTHS {
            assert!(f.contains(&format!("width:{w}px")));
        }
    }
}
