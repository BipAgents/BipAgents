//! « Rangement de l'existant » (plan « Le sommaire », brick S2; contracts/carnet/sommaire.md §2): the Carnet a person
//! had before the sommaire gets its categories, once, at night, without a model and without a token.
//!
//! - **The pass** ([`sort`], in the nightly pass before its model, tidy.rs): every page of the shared part gets
//!   `categories` and `categories_by: auto` in its frontmatter, the categories S1 reads on the fly for it (`decide`).
//!   Nothing moves: no file changes folder or name, and a page's text changes only by those header lines (written
//!   where the bridge writes them, checked by reading the page back; a page the bridge cannot change safely — another
//!   line ending, a frontmatter it cannot read, a `categories` of the person's own — is left as it is). Each page
//!   written is one change of its history (`categorize`, by the « rangement », never in « Nouveau »). The tasks page,
//!   « À ranger » and the documents' index are not notes: left as they are. A page a work agent made (its « create »
//!   in the history) is Travail, as at its creation since S1 (plan §2.2); a page it only wrote on is filed by its
//!   words (S1 review B1).
//! - **Once per version.** `state.json` `categories_v` is the table's version the pass has gone through: a better table
//!   (`VERSION` + 1) re-decides the `auto` pages once, never `agent` nor `person`. Between versions, each night only
//!   the pages without categories (written by hand, or by an older bridge) and those `auto` in « Autres » (the person's
//!   rules may place them now) are looked at again.
//! - **Safe on a big Carnet.** By batches of `BATCH` pages under the Carnet's lock (the agents' writes wait one batch at
//!   most), the language and the rules read once a batch; a page already as it should be is not written again
//!   (idempotent). A pass cut short (the bridge stopped, a crash) starts again after the last batch done
//!   (`categories_pass`: `{v, after}`, `after` the fingerprint of the batch's last path: the state keeps no path).
//! - **What stays in « Autres »** goes to the nightly pass's small model, `NIGHT_MAX` pages a night at most, inside its
//!   monthly cap ([`night`]): the pages no word placed, never the private part, never a page or a line that looks like
//!   health, never an attribution. Its answer is written `categories_by: agent` (a model's choice: the next versions
//!   of the table leave it, the person's correction still wins); a page it was shown is not sent again until its text
//!   changes.
//! - **The card**, once, after the first pass: « Ton Carnet est rangé par catégories · 9 notes dans Autres » with
//!   « Voir » (opens « Autres ») and « OK » ([`card`], `GET /v1/notebook` `sorted`; `POST /v1/notebook/sorted`).
//! - **The person's re-filings** (« Ranger ailleurs », categories.rs `categorize`): the toast and its « Annuler »
//!   ([`with_toast`]), then today's list ([`today`], `GET /v1/notebook/categorize/today`), each still undoable.
//!
//! - **The one-time sort** (carnet-categories, 22/11, Alex: « Ok jusqu'à 200 pages »): once the pass has gone through
//!   the whole Carnet at a new version (S2's first pass, or a better table), the model reads up to `ONCE_MAX` pages
//!   instead of `NIGHT_MAX`: those left in « Autres » first, then those the words placed narrowly
//!   (categories.rs `narrow_in`), by calls of `NIGHT_MAX` pages, inside the same monthly cap (cut down, never past it:
//!   the rest the following nights), once per Carnet and version. Each call is reserved in the state before it goes
//!   (its pages marked as shown, its most cost kept): a bridge stopped midway counts it at its next night and never
//!   sends those pages again.
//!
//! state.json: `categories_v` (int), `categories_pass` `{v, after}` (a pass under way; `after` a fingerprint),
//! `categories_card` `{v, at, done, answer?}`, `categories_tried` (fingerprints of the pages the model was shown: never
//! their path nor their text), `categories_once` `{v, left}` (the one-time sort's pages still allowed),
//! `categories_spent` `{eur, tokens_in, tokens_out, calls, reserved}` (this night's model calls not yet in the month's
//! spending: counted by tidy.rs when it records the night, or at the next night after a stop).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::categories::{self as c, Ctx};
use super::{fold, lexicon, records, tidy, Page, Part, Who, Words};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// The categories table's version (lexicon::CATEGORY_WORDS and categories.rs `decide`): one more re-decides the
/// `auto` pages once. 2: carnet-categories (22/11) — the everyday words of the five languages, « Autres » words, a
/// title's head, the neighbours, an amount alone no longer Argent.
pub const VERSION: i64 = 2;
/// Pages written under one hold of the Carnet's lock.
pub const BATCH: usize = 50;
/// Pages of « Autres » sent to the small model, a night at most.
pub const NIGHT_MAX: usize = 20;
/// What one page of « Autres » sends at most (characters).
const EXCERPT: usize = 400;
/// What the model may answer for them: 20 lines of ≈ 15 tokens, and GLM's « low » thinking, which the provider counts
/// in the same tokens (model_route.rs `add_thinking_floor`; carnet-categories: 400 could end before the 20th line).
const OUT_MAX: i64 = 1000;
/// The pages the model was shown, kept (fingerprints).
const TRIED_KEPT: usize = 2000;
/// The one-time sort after a new version: the pages the model may read, in all (Alex, 22/11).
pub const ONCE_MAX: usize = 200;
/// What a call may cost is reserved with this many characters a token (fewer than `tidy::tokens_of`'s 3.5: the cap is
/// never passed even when the provider counts more tokens than the bridge thinks).
const CHARS_PER_TOKEN_SURE: f64 = 2.5;

const KEY_V: &str = "categories_v";
const KEY_PASS: &str = "categories_pass";
const KEY_CARD: &str = "categories_card";
const KEY_TRIED: &str = "categories_tried";
const KEY_ONCE: &str = "categories_once";
pub(crate) const KEY_SPENT: &str = "categories_spent";

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Sorted {
    /// Pages given categories (their header written).
    pub filed: usize,
    /// Pages left as they are because the bridge cannot change them safely, or holding a `categories` of the
    /// person's own.
    pub skipped: usize,
    /// The whole Carnet went through this version now (`categories_v` set).
    pub done: bool,
    /// The longest hold of the Carnet's lock.
    pub longest_hold: Duration,
}

// -- the header lines ------------------------------------------------------------------------------------------------

/// Where a frontmatter ends, as `Page::parse` reads it: (the end of its YAML, where its closing `---` starts).
fn frontmatter_end(after: &str) -> Option<(usize, usize)> {
    after
        .find("\n---\n")
        .map(|i| (i, i + 1))
        .or_else(|| after.strip_suffix("\n---").map(|a| (a.len(), a.len() + 1)))
        .or(if after.starts_with("---\n") { Some((0, 0)) } else { None })
}

/// The page without `categories` and `categories_by`, to compare.
fn without_ours(page: &Page) -> Dict {
    let mut front = page.front.clone();
    front.shift_remove("categories");
    front.shift_remove("categories_by");
    front
}

/// The page's text with `categories` and `categories_by` in its frontmatter and nothing else changed: the bridge's
/// lines (and a list's items) replaced where they are, else put before `updated:` (where the bridge writes them), else
/// at the end of the frontmatter; a page without one gets one. Checked by reading it back (the same frontmatter
/// otherwise, the same title, the same text). None when the bridge cannot change it safely: a frontmatter it cannot
/// read (another line ending, a byte-order mark, YAML that does not parse, not closed).
pub fn with_header(text: &str, categories: &[String], by: &str) -> Option<String> {
    let ours = format!("categories:\n{}categories_by: {by}\n", categories.iter().map(|c| format!("  - {c}\n")).collect::<String>());
    rewrite(text, Some(ours.as_str()), Some((categories, by)))
}

/// The page's text without `categories` and `categories_by` (« Annuler » after the person's « Ranger ailleurs » on a
/// page that had none), nothing else changed; a frontmatter that held only them is taken out. None as for
/// [`with_header`].
pub fn without_header(text: &str) -> Option<String> {
    rewrite(text, None, None)
}

/// [`with_header`] (`ours`: the lines to put) and [`without_header`] (None), checked by reading the page back
/// (`want`: the categories it must then hold; None: none).
fn rewrite(text: &str, ours: Option<&str>, want: Option<(&[String], &str)>) -> Option<String> {
    // another line ending: left as it is (S2 review 1, R3)
    if text.contains('\r') {
        return None;
    }
    let out = if text.starts_with("---") || text.starts_with('\u{feff}') {
        let after = text.strip_prefix("---\n")?;
        // an empty frontmatter with a rule further down: the parser would read the lines before the rule as its
        // frontmatter, ours would show in the text (S2 review 1, R4)
        if after.starts_with("---\n") && after[4..].contains("\n---\n") {
            return None;
        }
        let (end, close) = frontmatter_end(after)?;
        let yaml = &after[..end];
        if !yaml.trim().is_empty() && !matches!(serde_yaml::from_str::<Value>(yaml), Ok(Value::Object(_))) {
            return None;
        }
        let mut kept: Vec<&str> = Vec::new();
        let mut list = false;
        let mut at: Option<usize> = None;
        for line in yaml.split('\n').filter(|_| !yaml.is_empty()) {
            if line.starts_with("categories:") {
                list = line.trim_end() == "categories:";
                at.get_or_insert(kept.len());
                continue;
            }
            if list && (line.starts_with(' ') || line.starts_with('-')) {
                continue;
            }
            list = false;
            if line.starts_with("categories_by:") {
                at.get_or_insert(kept.len());
                continue;
            }
            kept.push(line);
        }
        let place = at.or_else(|| kept.iter().position(|l| l.starts_with("updated:"))).unwrap_or(kept.len());
        let mut head = String::new();
        for (i, line) in kept.iter().enumerate() {
            if i == place {
                head.push_str(ours.unwrap_or(""));
            }
            head.push_str(line);
            head.push('\n');
        }
        if place >= kept.len() {
            head.push_str(ours.unwrap_or(""));
        }
        if head.is_empty() && ours.is_none() && !yaml.is_empty() {
            // a frontmatter that held only our lines (written by the bridge on a page that had none): taken out
            after[close..].strip_prefix("---").map(|rest| rest.strip_prefix('\n').unwrap_or(rest).to_string())?
        } else {
            format!("---\n{head}{}", &after[close..])
        }
    } else {
        match ours {
            Some(ours) => format!("---\n{ours}---\n{text}"),
            None => text.to_string(),
        }
    };
    let (old, new) = (Page::parse(text, ""), Page::parse(&out, ""));
    let same = without_ours(&old) == without_ours(&new) && old.title == new.title && old.body == new.body;
    let holds = match want {
        Some((categories, by)) => c::stored(&new) == Some((categories.to_vec(), by.to_string())),
        None => !new.front.contains_key("categories") && !new.front.contains_key("categories_by"),
    };
    (same && holds).then_some(out)
}

// -- the pass ------------------------------------------------------------------------------------------------------

/// A path as the state keeps it: a fingerprint, never the path (« Oublie ça » leaves no name in state.json).
fn print_of_path(path: &str) -> String {
    super::rev(path)
}

/// The pages that are not notes: the tasks page, « À ranger », the documents' index.
fn structural(page: &Page, path: &str, w: &Words) -> bool {
    matches!(page.kind().as_str(), "tasks" | "documents") || path == w.inbox_path()
}

/// What the page's frontmatter says of its categories: none yet, the bridge's own (`auto`), the person's or an
/// agent's choice, or a `categories` the bridge did not write (by hand in Obsidian, values it does not read).
#[derive(Debug, PartialEq)]
enum Header {
    None,
    Auto,
    Chosen,
    Theirs,
}

fn header(page: &Page) -> Header {
    let by = page.front.get("categories_by").map(|v| tk::str_or_empty(Some(v)));
    match (page.front.contains_key("categories"), by.as_deref()) {
        (_, Some("person" | "agent")) => Header::Chosen,
        (false, None | Some("auto")) => Header::None,
        (true, Some("auto")) if c::stored(page).is_some() => Header::Auto,
        _ => Header::Theirs,
    }
}

/// The bridge's own « Autres »: `auto` in « Autres », written so.
fn auto_other(page: &Page) -> bool {
    header(page) == Header::Auto && c::stored(page).is_some_and(|(cats, _)| cats == ["other"])
}

/// The agent that made each page of the shared part (its last « create » in the history: the change that wrote it
/// from nothing), read line by line from the log (a big Carnet's log is never held whole). A page gone since (deleted,
/// merged, moved away: a change with no text after it) or made again by the person is no longer that agent's (S2
/// review 1, R6).
fn makers(config: &Config) -> HashMap<String, String> {
    use std::io::BufRead;
    let mut out = HashMap::new();
    let Ok(file) = std::fs::File::open(super::history_dir(config).join("log.jsonl")) else { return out };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        // most lines neither make nor take out a page: skipped before they are read as JSON
        if !line.contains("\"create\"") && !line.contains("\"after\": null") && !line.contains("\"after\":null") {
            continue;
        }
        let Ok(Value::Object(e)) = serde_json::from_str::<Value>(&line) else { continue };
        if tk::str_or_empty(e.get("part")) != "main" {
            continue;
        }
        let path = tk::str_or_empty(e.get("path"));
        let agent = tk::str_or_empty(e.get("agent"));
        if e.get("after").is_none_or(Value::is_null) {
            out.remove(&path);
        } else if tk::str_or_empty(e.get("action")) == "create" {
            if agent.is_empty() {
                out.remove(&path);
            } else {
                out.insert(path, agent);
            }
        }
    }
    out
}

/// The work agent that made the page (plan §2.2, as categories.rs `on_write` at a page's creation: a page a Pro agent
/// makes is Travail; one it only wrote on is not), else None.
fn maker_of(config: &Config, path: &str, makers: &HashMap<String, String>, work: &mut HashMap<String, bool>) -> Option<String> {
    let agent = makers.get(path)?;
    let is_work = *work.entry(agent.clone()).or_insert_with(|| super::assign::is_work_agent(config, agent));
    is_work.then(|| agent.clone())
}

/// One page: Some(true) written, Some(false) as it should be already (or not the bridge's to file), None left alone
/// (the bridge cannot change it safely, or the person wrote a `categories` of their own).
#[allow(clippy::too_many_arguments)]
fn sort_page(
    config: &Config,
    ctx: &Ctx,
    w: &Words,
    path: &str,
    full: bool,
    who: &Who,
    makers: &HashMap<String, String>,
    work: &mut HashMap<String, bool>,
) -> anyhow::Result<Option<bool>> {
    let Some(text) = super::read_page_text(config, Part::Main, path) else { return Ok(Some(false)) };
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let page = Page::parse(&text, &stem);
    if structural(&page, path, w) {
        return Ok(Some(false));
    }
    let now = c::stored(&page);
    match header(&page) {
        Header::Chosen => return Ok(Some(false)),
        Header::Theirs => return Ok(None),
        Header::Auto if !full && !auto_other(&page) => return Ok(Some(false)),
        _ => {}
    }
    let writer = maker_of(config, path, makers, work);
    let (cats, _) = c::decide_in(ctx, Part::Main, path, &page, writer.as_deref());
    if header(&page) == Header::Auto && now == Some((cats.clone(), "auto".into())) {
        return Ok(Some(false));
    }
    let Some(after) = with_header(&text, &cats, "auto") else { return Ok(None) };
    // (no summary: a page's history in the apps 1.0 would show the ids; S1 review R4). A page the bridge cannot write
    // (a read-only file or folder on a self-install) is left and counted: the pass and the night go on (S2 review 1,
    // R1); the next night tries it again.
    if let Err(err) = super::commit(config, Part::Main, path, Some(&text), Some(&after), who, c::ACTION, "") {
        tracing::warn!(path = %path, error = %format!("{err:#}"), "notebook page's categories not written");
        return Ok(None);
    }
    Ok(Some(true))
}

/// The pass, now (the nightly pass calls it; free, no model).
pub fn sort(config: &Config) -> anyhow::Result<Sorted> {
    sort_with(config, VERSION, None)
}

/// The pass for the table's `version`; `stop_after` batches (tests: as if the bridge stopped there).
pub(crate) fn sort_with(config: &Config, version: i64, stop_after: Option<usize>) -> anyhow::Result<Sorted> {
    let state = super::read_state(config);
    let done_v = state.get(KEY_V).and_then(Value::as_i64).unwrap_or(0);
    let full = done_v < version;
    let resume = state
        .get(KEY_PASS)
        .filter(|p| full && p.get("v").and_then(Value::as_i64) == Some(version))
        .map(|p| tk::str_or_empty(p.get("after")));
    let lang = super::language(config);
    let w = super::words(&lang);
    let all = super::all_pages(config, Part::Main);
    let notes = all.iter().filter(|(path, p)| !structural(p, path, w)).count();
    let mut paths: Vec<String> = all.into_iter().map(|(p, _)| p).collect();
    // after the last page done (gone since: from the start again, nothing is written twice)
    if let Some(at) = resume.and_then(|after| paths.iter().position(|p| print_of_path(p) == after)) {
        paths.drain(..=at);
    }
    let who = Who::tidy(config);
    let mut out = Sorted::default();
    let makers = makers(config);
    // the words' tables made before the lock is taken (review 1, R5)
    c::warm();
    let mut work: HashMap<String, bool> = HashMap::new();
    for (n, batch) in paths.chunks(BATCH).enumerate() {
        if stop_after == Some(n) {
            return Ok(out);
        }
        let _guard = super::lock();
        let held = Instant::now();
        let ctx = Ctx::new(config);
        for path in batch {
            match sort_page(config, &ctx, w, path, full, &who, &makers, &mut work)? {
                Some(true) => out.filed += 1,
                Some(false) => {}
                None => out.skipped += 1,
            }
        }
        if full {
            let after = batch.last().map(|p| print_of_path(p)).unwrap_or_default();
            super::update_state(config, |s| {
                s.insert(KEY_PASS.into(), json!({"v": version, "after": after}));
            })?;
        }
        out.longest_hold = out.longest_hold.max(held.elapsed());
    }
    if full {
        let _guard = super::lock();
        super::update_state(config, |s| {
            s.insert(KEY_V.into(), json!(version));
            s.shift_remove(KEY_PASS);
            // the card, once: the first pass over a Carnet that has notes
            if done_v == 0 && notes > 0 && !s.contains_key(KEY_CARD) {
                s.insert(KEY_CARD.into(), json!({"v": version, "at": super::today(), "done": false}));
            }
            // the model's one-time sort, once per version
            if s.get(KEY_ONCE).and_then(|o| o.get("v")).and_then(Value::as_i64) != Some(version) {
                s.insert(KEY_ONCE.into(), json!({"v": version, "left": ONCE_MAX}));
            }
        })?;
        out.done = true;
    }
    if out.filed > 0 || out.skipped > 0 {
        tracing::info!(filed = out.filed, skipped = out.skipped, done = out.done, "notebook pages given their categories");
    }
    Ok(out)
}

// -- the card ------------------------------------------------------------------------------------------------------

/// The card's words: title, one note, several, « Voir », « OK » (fr: the plan's; the others proposed, for Alex).
fn card_texts(lang: &str) -> (&'static str, &'static str, &'static str, &'static str, &'static str) {
    match lang {
        "fr" => ("Ton Carnet est rangé par catégories", "1 note dans {other}", "{n} notes dans {other}", "Voir", "OK"),
        "de" => ("Dein Notizbuch ist nach Kategorien geordnet", "1 Notiz in {other}", "{n} Notizen in {other}", "Ansehen", "OK"),
        "es" => ("Tu Cuaderno está ordenado por categorías", "1 nota en {other}", "{n} notas en {other}", "Ver", "OK"),
        _ => ("Your Notebook is sorted into categories", "1 note in {other}", "{n} notes in {other}", "See", "OK"),
    }
}

/// The notes in « Autres » (pages, their lines, tasks, records, documents: how the sommaire counts; `pages`: the
/// shared part's).
pub fn other_count(config: &Config, pages: &[(String, Page)]) -> usize {
    let ctx = Ctx::new(config);
    let w = super::words(ctx.lang());
    let marks = c::marks(config);
    let today = chrono::Local::now().date_naive();
    let other = |cats: &[String]| cats.iter().any(|c| c == "other");
    let mut n = 0;
    for (path, page) in pages {
        if structural(page, path, w) || page.kind() == "records" {
            continue;
        }
        let filed = c::stored(page).map(|(cats, _)| cats).unwrap_or_else(|| c::decide_in(&ctx, Part::Main, path, page, None).0);
        if other(&filed) {
            n += 1;
        }
        let key = c::page_key(page, path);
        for (section, facts) in page.sections() {
            if fold(&section) == fold(w.brief) {
                continue;
            }
            n += facts.iter().filter(|f| other(&c::of_line(&filed, &key, &f.text, &f.date, &marks, today))).count();
        }
    }
    n += super::open_tasks(config, Part::Main).iter().filter(|t| other(&c::of_task(&ctx, t, pages, &marks, today))).count();
    for t in &records::TYPES {
        n += records::records(config, t).iter().filter(|r| other(&c::of_record(t, r, &marks, today).0)).count();
    }
    // a document is in « Autres » only when the person put it there (else its page's, its name's, or Papiers)
    n + marks.iter().filter(|(k, m)| k.starts_with("doc:") && other(&super::strings(m.get("categories")))).count()
}

/// The card's count of « Autres » by Carnet, kept while the Carnet does not change: the 1.0 apps never answer the
/// card, and the home would count every line again at each opening (S2 review 1, R2).
static OTHER: LazyLock<Mutex<HashMap<PathBuf, (String, usize)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What says the Carnet changed for the count: its history's length (every write of the bridge) and the person's
/// marks (a record, task, line or document filed elsewhere: state.json, no history).
fn carnet_print(config: &Config) -> String {
    let log = std::fs::metadata(super::history_dir(config).join("log.jsonl")).map(|m| m.len()).unwrap_or(0);
    let marks = serde_json::to_string(&c::marks(config)).unwrap_or_default();
    format!("{log}:{}", super::rev(&marks))
}

/// [`other_count`], counted again only when the Carnet changed.
fn other_count_kept(config: &Config, pages: &[(String, Page)]) -> usize {
    let (key, print) = (super::root(config), carnet_print(config));
    let kept = OTHER.lock().ok().and_then(|m| m.get(&key).filter(|(p, _)| *p == print).map(|(_, n)| *n));
    kept.unwrap_or_else(|| {
        let n = other_count(config, pages);
        if let Ok(mut m) = OTHER.lock() {
            m.insert(key, (print, n));
        }
        n
    })
}

/// The card, ready to show (`shown`: the app's language), until the person answers it; `pages`: the shared part's.
pub fn card(config: &Config, shown: &str, pages: &[(String, Page)]) -> Option<Value> {
    let state = super::read_state(config);
    let card = state.get(KEY_CARD)?;
    if card.get("done").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let n = other_count_kept(config, pages);
    let (title, one, many, see, ok) = card_texts(shown);
    let other = c::title(shown, "other");
    let text = match n {
        0 => Value::Null,
        1 => json!(one.replace("{other}", &other)),
        _ => json!(many.replace("{n}", &n.to_string()).replace("{other}", &other)),
    };
    let see = if n == 0 { Value::Null } else { json!(see) };
    Some(json!({"title": title, "text": text, "other": n, "see": see, "ok": ok, "open": {"category": "other"}}))
}

/// The card answered (`ok`, or `see`: « Voir » opened « Autres »): not shown again.
pub fn card_done(config: &Config, action: &str) -> anyhow::Result<()> {
    let _guard = super::lock();
    super::update_state(config, |s| {
        if let Some(Value::Object(card)) = s.get_mut(KEY_CARD) {
            card.insert("done".into(), json!(true));
            card.insert("answer".into(), json!(action));
            card.insert("answered".into(), json!(super::today()));
        }
    })?;
    Ok(())
}

// -- what stays in « Autres »: the nightly model ------------------------------------------------------------------

pub const INSTRUCTIONS: &str = "You file notes of a person's notebook (their « Carnet ») in categories. Each note \
below is a page no keyword placed, or one a weak keyword did: its number, its title, then a few of its lines. Give \
each note the one or two categories it clearly belongs to, from: people (family, friends, the people they know), money \
(budget, bills, purchases, subscriptions, bank, savings), home (the house or flat, repairs, garden, household, \
neighbours), papers (administration, contracts, insurance, ID and official documents, taxes), trips (travel, \
holidays, journeys), work (their job, colleagues, clients, professional projects); or other alone (hobbies, books, \
films, music, sport, learning, what none of these is). When you are not sure, leave the note out. Never health. \
Answer with JSON lines only, one per note you file, nothing else:
{\"n\": <the note's number>, \"categories\": [\"home\"]}";

/// A page of « Autres » for the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub path: String,
    /// Its fingerprint (path and text): not sent again while the page stays as it is.
    pub print: String,
    pub excerpt: String,
}

fn print_of(path: &str, text: &str) -> String {
    super::rev(&format!("{path}\n{text}"))
}

/// What the model reads of a page: its title and kind, its aliases, its « En bref » and its lines, without their
/// attributions and without anything that might touch health (lexicon.rs `touches_health`: what is held back),
/// `EXCERPT` characters at most.
fn excerpt(page: &Page, w: &Words) -> String {
    let kind = page.kind();
    let mut parts: Vec<String> = Vec::new();
    let aliases: Vec<String> = page.aliases().into_iter().filter(|a| !lexicon::touches_health(a)).collect();
    if !aliases.is_empty() {
        parts.push(format!("also {}", aliases.join(", ")));
    }
    let brief = tk::squeeze(&super::summary_of(&page.body, w.brief));
    if !brief.is_empty() && !lexicon::touches_health(&brief) {
        parts.push(brief);
    }
    for (section, facts) in page.sections() {
        if fold(&section) == fold(w.brief) {
            continue;
        }
        for f in facts {
            if !f.text.trim().is_empty() && !lexicon::touches_health(&f.text) {
                parts.push(tk::squeeze(&f.text));
            }
        }
    }
    let head = format!("{} ({})", page.title, if kind.is_empty() { "page" } else { kind.as_str() });
    let full = if parts.is_empty() { head } else { format!("{head} — {}", parts.join(" · ")) };
    tk::head(&full, EXCERPT).to_string()
}

/// The pages the model may place now: `auto` in « Autres » only, never one that looks like health, never sent before
/// as they are, `limit` at most (in the pages' order).
pub fn candidates(config: &Config, limit: usize) -> Vec<Candidate> {
    walk(config, limit, false)
}

/// The one-time sort's pages (`ONCE_MAX` in all): those of [`candidates`] first, then the `auto` pages the words placed
/// narrowly (categories.rs `narrow_in`: never by the person's rules, never by a page's kind) whose header still says
/// what the words say, `limit` at most.
pub fn candidates_once(config: &Config, limit: usize) -> Vec<Candidate> {
    walk(config, limit, true)
}

fn walk(config: &Config, limit: usize, narrow_too: bool) -> Vec<Candidate> {
    let tried: HashSet<String> = super::strings(super::read_state(config).get(KEY_TRIED)).into_iter().collect();
    let lang = super::language(config);
    let w = super::words(&lang);
    let ctx = Ctx::new(config);
    let (mut other, mut narrow) = (Vec::new(), Vec::new());
    for (path, page) in super::all_pages(config, Part::Main) {
        if other.len() >= limit {
            break;
        }
        if structural(&page, &path, w) || lexicon::touches_health(&page.title) {
            continue;
        }
        // in « Autres » because no word placed it; never one a hobby's word put there on purpose (review 1, B1)
        let is_other = auto_other(&page) && c::decide_in(&ctx, Part::Main, &path, &page, None).1.is_none();
        let is_narrow = !is_other
            && narrow_too
            && narrow.len() < limit
            && header(&page) == Header::Auto
            && c::stored(&page).is_some_and(|(cats, _)| cats == c::decide_in(&ctx, Part::Main, &path, &page, None).0)
            && c::narrow_in(&ctx, Part::Main, &path, &page);
        if !is_other && !is_narrow {
            continue;
        }
        let Some(text) = super::read_page_text(config, Part::Main, &path) else { continue };
        let print = print_of(&path, &text);
        if !tried.contains(&print) {
            let cand = Candidate { path, print, excerpt: excerpt(&page, w) };
            if is_other { other.push(cand) } else { narrow.push(cand) }
        }
    }
    other.extend(narrow);
    other.truncate(limit);
    other
}

/// The model's input: the numbered notes.
pub fn night_input(candidates: &[Candidate]) -> String {
    candidates.iter().enumerate().map(|(i, c)| format!("{}. {}", i + 1, c.excerpt)).collect::<Vec<_>>().join("\n")
}

/// The model's lines applied, all the candidates marked as shown. Kept as `agent` only when the model is sure: it is
/// told to leave out a note it cannot place clearly and to answer « other » when none of the categories fits; a line
/// counts only when it gives 1–2 of `agent_may_give` (never « other », « dates », « health ») for a page still the
/// bridge's (`auto`). Anything else leaves the page as the words placed it. Returns the pages filed.
pub fn apply_answer(config: &Config, candidates: &[Candidate], answer: &str) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let who = Who::tidy(config);
    let mut filed = 0;
    // one object a line, as asked; or one JSON array, maybe in a code fence (S2 review 1, R9)
    let bare = answer.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let ops: Vec<Value> = match serde_json::from_str::<Value>(bare) {
        Ok(Value::Array(items)) => items,
        _ => answer.lines().filter_map(|l| serde_json::from_str::<Value>(l.trim().trim_matches(',')).ok()).collect(),
    };
    for op in ops {
        let Value::Object(op) = op else { continue };
        let n = match op.get("n") {
            Some(Value::Number(x)) => x.as_u64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        };
        let Some(cand) = n.and_then(|n| (n as usize).checked_sub(1)).and_then(|i| candidates.get(i)) else { continue };
        let Some(cats) = c::valid(&super::strings(op.get("categories")), &c::AGENT_MAY_GIVE) else { continue };
        let Some(text) = super::read_page_text(config, Part::Main, &cand.path) else { continue };
        // still the bridge's (the person or an agent may have filed it since)
        if header(&Page::parse(&text, "")) != Header::Auto {
            continue;
        }
        let Some(after) = with_header(&text, &cats, "agent") else { continue };
        match super::commit(config, Part::Main, &cand.path, Some(&text), Some(&after), &who, c::ACTION, "") {
            Ok(_) => filed += 1,
            Err(err) => tracing::warn!(path = %cand.path, error = %format!("{err:#}"), "notebook page's categories not written"),
        }
    }
    super::update_state(config, |s| {
        let mut list = super::strings(s.get(KEY_TRIED));
        for cand in candidates {
            if !list.contains(&cand.print) {
                list.push(cand.print.clone());
            }
        }
        let over = list.len().saturating_sub(TRIED_KEPT);
        list.drain(..over);
        s.insert(KEY_TRIED.into(), json!(list));
    })?;
    Ok(filed)
}

/// A call's body: the instructions, the numbered notes, the provider's routing and GLM's thinking floor.
pub(crate) fn request_body(config: &Config, base: &str, model: &str, user: &str) -> Value {
    let mut body = json!({"model": model, "temperature": 0, "max_tokens": OUT_MAX,
                          "messages": [{"role": "system", "content": INSTRUCTIONS}, {"role": "user", "content": user}]});
    crate::model_route::add_to_body(&config.model_provider_order, base, &mut body);
    body
}

/// The one-time sort's pages still allowed, when it is open for this version.
fn once_left(config: &Config) -> Option<usize> {
    let state = super::read_state(config);
    let once = state.get(KEY_ONCE)?;
    let left = once.get("left").and_then(Value::as_u64).unwrap_or(0) as usize;
    (once.get("v").and_then(Value::as_i64) == Some(VERSION) && left > 0).then_some(left)
}

/// How many of `rest`'s first `NIGHT_MAX` pages one call may send: inside `[notebook] max_input_chars`, and so that
/// its most cost (its input counted at `CHARS_PER_TOKEN_SURE`, `OUT_MAX` out) stays inside `left` euros.
fn fitting(config: &Config, rest: &[Candidate], left: f64) -> usize {
    let room = if config.notebook.price_in > 0.0 {
        let euros = left - tidy::cost(config, 0, OUT_MAX);
        (euros.max(0.0) * 1_000_000.0 / config.notebook.price_in * CHARS_PER_TOKEN_SURE) as usize
    } else {
        usize::MAX
    };
    let room = config.notebook.max_input_chars.min(room).saturating_sub(INSTRUCTIONS.len());
    let mut size = 0;
    rest.iter()
        .take(NIGHT_MAX)
        .take_while(|c| {
            size += c.excerpt.len() + 6;
            size <= room
        })
        .count()
}

/// A call's most cost: its input at `CHARS_PER_TOKEN_SURE`, `OUT_MAX` out.
fn most_cost(config: &Config, user: &str) -> f64 {
    let tokens = ((INSTRUCTIONS.len() + user.len()) as f64 / CHARS_PER_TOKEN_SURE).ceil() as i64;
    tidy::cost(config, tokens, OUT_MAX)
}

/// Before a call: its pages marked as shown, the one-time sort's allowance taken, its most cost reserved — one write,
/// so that a bridge stopped during the call neither sends them again nor forgets what it may have spent.
fn reserve(config: &Config, batch: &[Candidate], most: f64, once: bool) -> anyhow::Result<()> {
    let _guard = super::lock();
    super::update_state(config, |s| {
        let mut list = super::strings(s.get(KEY_TRIED));
        for cand in batch {
            if !list.contains(&cand.print) {
                list.push(cand.print.clone());
            }
        }
        let over = list.len().saturating_sub(TRIED_KEPT);
        list.drain(..over);
        s.insert(KEY_TRIED.into(), json!(list));
        if once {
            if let Some(Value::Object(o)) = s.get_mut(KEY_ONCE) {
                let left = o.get("left").and_then(Value::as_u64).unwrap_or(0).saturating_sub(batch.len() as u64);
                o.insert("left".into(), json!(left));
            }
        }
        let mut spent = s.get(KEY_SPENT).and_then(Value::as_object).cloned().unwrap_or_default();
        spent.insert("reserved".into(), json!(most));
        s.insert(KEY_SPENT.into(), Value::Object(spent));
    })?;
    Ok(())
}

/// After a call: what it cost replaces its reservation (`ok`: it answered; else its pages may go again).
fn settle(config: &Config, batch: &[Candidate], tokens_in: i64, tokens_out: i64, eur: f64, ok: bool, once: bool) -> anyhow::Result<()> {
    let _guard = super::lock();
    super::update_state(config, |s| {
        let mut spent = s.get(KEY_SPENT).and_then(Value::as_object).cloned().unwrap_or_default();
        let num = |m: &Dict, k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let int = |m: &Dict, k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
        spent.insert("eur".into(), json!(num(&spent, "eur") + eur));
        spent.insert("tokens_in".into(), json!(int(&spent, "tokens_in") + tokens_in));
        spent.insert("tokens_out".into(), json!(int(&spent, "tokens_out") + tokens_out));
        spent.insert("calls".into(), json!(int(&spent, "calls") + 1));
        spent.insert("reserved".into(), json!(0.0));
        s.insert(KEY_SPENT.into(), Value::Object(spent));
        if !ok {
            let prints: HashSet<&str> = batch.iter().map(|c| c.print.as_str()).collect();
            let list: Vec<String> = super::strings(s.get(KEY_TRIED)).into_iter().filter(|p| !prints.contains(p.as_str())).collect();
            s.insert(KEY_TRIED.into(), json!(list));
            if once {
                if let Some(Value::Object(o)) = s.get_mut(KEY_ONCE) {
                    let left = o.get("left").and_then(Value::as_u64).unwrap_or(0) + batch.len() as u64;
                    o.insert("left".into(), json!(left));
                }
            }
        }
    })?;
    Ok(())
}

/// A night that stopped before tidy.rs recorded it (the bridge stopped, a crash): what its calls cost, and what one
/// under way may have, added to the month's spending now — never twice (tidy.rs `run` clears it as it records a night).
fn recover(config: &Config) -> anyhow::Result<()> {
    let state = super::read_state(config);
    let Some(spent) = state.get(KEY_SPENT).and_then(Value::as_object).cloned() else { return Ok(()) };
    let _guard = super::lock();
    super::update_state(config, |s| {
        let mut month = tidy::spending_in(s);
        add_spent(&mut month, &spent, true);
        tidy::set_spending(s, month);
        s.shift_remove(KEY_SPENT);
    })?;
    Ok(())
}

/// `spent` (`categories_spent`) added to a month's spending; `reserved`: a call under way counted at its most.
pub(crate) fn add_spent(month: &mut Dict, spent: &Dict, reserved: bool) {
    let num = |m: &Dict, k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let int = |m: &Dict, k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
    let eur = num(month, "eur") + num(spent, "eur") + if reserved { num(spent, "reserved") } else { 0.0 };
    month.insert("eur".into(), json!((eur * 1_000_000.0).round() / 1_000_000.0));
    month.insert("tokens_in".into(), json!(int(month, "tokens_in") + int(spent, "tokens_in")));
    month.insert("tokens_out".into(), json!(int(month, "tokens_out") + int(spent, "tokens_out")));
    month.insert("model_calls".into(), json!(int(month, "model_calls") + int(spent, "calls")));
}

/// The nightly model's turn for « Autres » (tidy.rs `run`, after its own call, before the month's spending is
/// counted): `NIGHT_MAX` pages a night, or the one-time sort's `ONCE_MAX` after a new version, by calls of `NIGHT_MAX`,
/// each inside what is left of the monthly cap; tokens and cost added to `record` (`categorized_by_model`;
/// `categories_reason` when it did not call or stopped). The one-time sort closes once its pages are all sent.
pub async fn night(config: &Config, http: &reqwest::Client, record: &mut Dict) -> anyhow::Result<()> {
    recover(config)?;
    let once = once_left(config);
    let c = config.clone();
    let mut rest = tokio::task::spawn_blocking(move || match once {
        Some(left) => candidates_once(&c, left),
        None => candidates(&c, NIGHT_MAX),
    })
    .await?;
    let close = |config: &Config| {
        let _guard = super::lock();
        super::update_state(config, |s| {
            if let Some(Value::Object(o)) = s.get_mut(KEY_ONCE) {
                o.insert("left".into(), json!(0));
            }
        })
    };
    if rest.is_empty() {
        if once.is_some() {
            close(config)?;
        }
        return Ok(());
    }
    let num = |r: &Dict, k: &str| r.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let int = |r: &Dict, k: &str| r.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut filed = 0;
    while !rest.is_empty() {
        // this run's own calls are not in the month's spending yet
        let spent = tidy::spending(config).get("eur").and_then(Value::as_f64).unwrap_or(0.0) + num(record, "eur");
        let left = config.notebook.monthly_cap - spent;
        let fit = if left > 0.0 { fitting(config, &rest, left) } else { 0 };
        if fit == 0 {
            record.insert("categories_reason".into(), json!("monthly cap reached"));
            break;
        }
        let Some((base, model, key)) = tidy::model(config) else {
            record.insert("categories_reason".into(), json!("no model to call"));
            break;
        };
        let batch: Vec<Candidate> = rest.drain(..fit).collect();
        let user = night_input(&batch);
        reserve(config, &batch, most_cost(config, &user), once.is_some())?;
        let body = request_body(config, &base, &model, &user);
        let sent = http
            .post(format!("{base}/chat/completions"))
            .header("Authorization", format!("Bearer {key}"))
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await;
        let resp = match sent {
            Ok(r) => r,
            Err(err) => {
                // no answer: nothing known spent, its pages go again
                settle(config, &batch, 0, 0, 0.0, false, once.is_some())?;
                return Err(err.into());
            }
        };
        // (the run's model calls: the pass's own, if it called, and these; S2 review 1, R9)
        let calls = record.get("model_calls").and_then(Value::as_i64).unwrap_or(i64::from(record.get("model_called") == Some(&json!(true))));
        record.insert("model_calls".into(), json!(calls + 1));
        record.insert("model_called".into(), json!(true));
        record.entry("model").or_insert(json!(model));
        let status = resp.status().as_u16();
        let data: Value = serde_json::from_slice(&resp.bytes().await.unwrap_or_default()).unwrap_or(Value::Null);
        let usage = data.get("usage").cloned().unwrap_or(json!({}));
        let tokens_in = usage.get("prompt_tokens").and_then(Value::as_i64).unwrap_or_else(|| tidy::tokens_of(INSTRUCTIONS.len() + user.len()));
        let tokens_out = usage.get("completion_tokens").and_then(Value::as_i64).unwrap_or(0);
        let cost = tidy::cost(config, tokens_in, tokens_out);
        let eur = num(record, "eur") + cost;
        let (all_in, all_out) = (int(record, "tokens_in") + tokens_in, int(record, "tokens_out") + tokens_out);
        record.insert("tokens_in".into(), json!(all_in));
        record.insert("tokens_out".into(), json!(all_out));
        record.insert("eur".into(), json!((eur * 1_000_000.0).round() / 1_000_000.0));
        settle(config, &batch, tokens_in, tokens_out, cost, status < 400, once.is_some())?;
        if status >= 400 {
            // not marked as shown: tried again next night
            record.insert("categories_reason".into(), json!(format!("the model answered {status}")));
            break;
        }
        let answer = data["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        let c = config.clone();
        filed += tokio::task::spawn_blocking(move || apply_answer(&c, &batch, &answer)).await??;
    }
    if once.is_some() && rest.is_empty() && !record.contains_key("categories_reason") {
        // all its pages sent (they were at most what it still allowed): the one-time sort is over
        close(config)?;
    }
    if filed > 0 {
        record.insert("categorized_by_model".into(), json!(filed));
    }
    Ok(())
}

// -- the person's re-filings: the toast, then today's list -------------------------------------------------------

/// The toast's words (one category, two) and « Annuler » (the plan's « Annuler »; the rest proposed, for Alex).
fn toast_words(lang: &str) -> (&'static str, &'static str, &'static str) {
    match lang {
        "fr" => ("Rangé dans {a}.", "Rangé dans {a} et {b}.", "Annuler"),
        "de" => ("In {a} abgelegt.", "In {a} und {b} abgelegt.", "Rückgängig"),
        "es" => ("Guardado en {a}.", "Guardado en {a} y {b}.", "Deshacer"),
        _ => ("Filed in {a}.", "Filed in {a} and {b}.", "Undo"),
    }
}

/// « Rangé dans Argent. » (« … et Maison » for two; Dates, computed, never named).
pub fn toast(lang: &str, categories: &[String]) -> String {
    let (one, two, _) = toast_words(lang);
    let names: Vec<String> = categories.iter().filter(|c| *c != "dates").map(|c| c::title(lang, c)).collect();
    match names.as_slice() {
        [a] => one.replace("{a}", a),
        [a, b, ..] => two.replace("{a}", a).replace("{b}", b),
        [] => String::new(),
    }
}

/// « Annuler ».
pub fn undo_title(lang: &str) -> &'static str {
    toast_words(lang).2
}

/// `categorize`'s answer with its toast and its « Annuler » in the app's language.
pub fn with_toast(mut view: Value, lang: &str) -> Value {
    let cats = super::strings(view.get("categories"));
    view["toast"] = json!(toast(lang, &cats));
    view["undo_title"] = json!(undo_title(lang));
    view
}

/// What a re-filing is, as the list shows it: the page's title, the line, the task's, the record's or the document's
/// name, read now (the state keeps only its key). None when it is no longer there (deleted, forgotten, edited): not
/// listed.
fn title_of(config: &Config, target: &c::Target) -> Option<String> {
    match target {
        c::Target::Page { path } => {
            let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            Some(Page::parse(&super::read_page_text(config, Part::Main, path)?, &stem).title)
        }
        c::Target::Line { path, text } => c::line_on(&Page::parse(&super::read_page_text(config, Part::Main, path)?, ""), text),
        c::Target::Task(id) => {
            let task = super::tasks(config, Part::Main).into_iter().find(|t| t.id == *id)?;
            Some(task.title.split(" [[").next().unwrap_or(&task.title).trim().to_string())
        }
        c::Target::Record { kind, id } => {
            let t = records::kind(kind)?;
            let all = records::records(config, t).into_iter().chain(records::archived_records(config, t));
            let found = all.into_iter().find(|r| tk::str_or_empty(r.get("id")) == *id)?;
            Some(records::summary_head(t, &found))
        }
        c::Target::Document { file, .. } => Some(file.clone()),
        c::Target::Item(_) => None,
    }
}

/// What a re-filing's key names now, and what the list shows of it (the pages read once for all of them).
fn named(config: &Config, key: &str, pages: &HashMap<String, (String, Page)>) -> Option<(c::Target, String)> {
    if let Some(k) = key.strip_prefix("page:") {
        let (path, page) = pages.get(k)?;
        return Some((c::Target::Page { path: path.clone() }, page.title.clone()));
    }
    if let Some(rest) = key.strip_prefix("line:") {
        let (k, _) = rest.split_once(':')?;
        let (path, page) = pages.get(k)?;
        let texts: Vec<String> = page.sections().into_iter().flat_map(|(_, f)| f).map(|f| f.text).collect();
        let i = c::line_ids(k, &texts).iter().position(|x| x == key)?;
        return Some((c::Target::Line { path: path.clone(), text: texts[i].clone() }, texts[i].clone()));
    }
    let target = c::resolve(config, key)?;
    let title = title_of(config, &target)?;
    Some((target, title))
}

/// Today's re-filings still undoable, the newest first: what it is (`title`), where it went (`categories`, `text`:
/// the toast's words), what it was (`before`), its `undo`.
pub fn today(config: &Config, lang: &str) -> Value {
    let state = super::read_state(config);
    let day = super::today();
    let entries: Vec<&Value> = state
        .get("category_undo")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| tk::str_or_empty(e.get("at")) == day)
        .collect();
    let pages: HashMap<String, (String, Page)> = if entries.is_empty() {
        HashMap::new()
    } else {
        super::all_pages(config, Part::Main).into_iter().map(|(path, page)| (c::page_key(&page, &path), (path, page))).collect()
    };
    let mut items: Vec<Value> = Vec::new();
    for e in entries {
        let key = tk::str_or_empty(e.get("key"));
        let Some(cats) = c::valid(&super::strings(e.get("categories")), &c::PERSON_MAY_GIVE) else { continue };
        let Some((target, title)) = named(config, &key, &pages) else { continue };
        let mut shown = target.view();
        shown["item"] = json!(key);
        let before = &e["before"];
        items.push(json!({"undo": e["id"], "target": shown, "title": title, "categories": cats,
                          "before": {"categories": before["categories"], "categories_by": before["categories_by"]},
                          "at": e.get("time").cloned().unwrap_or_else(|| e["at"].clone()), "text": toast(lang, &cats)}));
    }
    json!({"items": items, "undo_title": undo_title(lang)})
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;
    use crate::notebook::{all_pages, history, note, part_root, read_page_text, read_state, tests_config, update_state, write_file};

    fn config_in(dir: &tempfile::TempDir, lang: &str) -> Config {
        let config = tests_config(dir.path());
        update_state(&config, |s| {
            s.insert("language".into(), json!(lang));
        })
        .unwrap();
        config
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Every file under `dir` (relative path → bytes as text).
    fn files_in(dir: &Path) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                    out.insert(rel, String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned());
                }
            }
        }
        out
    }

    /// A text without the sommaire's header lines (`categories:` and its items, `categories_by:`), an empty
    /// frontmatter left by them taken out too: what must be left identical.
    fn without_header(text: &str) -> String {
        let Some(after) = text.strip_prefix("---\n") else { return text.to_string() };
        let Some(end) = after.find("\n---\n") else { return text.to_string() };
        let (yaml, rest) = (&after[..end], &after[end + 5..]);
        let mut kept: Vec<&str> = Vec::new();
        let mut in_list = false;
        for line in yaml.split('\n') {
            if line.starts_with("categories:") {
                in_list = line.trim_end() == "categories:";
                continue;
            }
            if in_list && (line.starts_with("  ") || line.starts_with('-')) {
                continue;
            }
            in_list = false;
            if line.starts_with("categories_by:") {
                continue;
            }
            kept.push(line);
        }
        if kept.iter().all(|l| l.is_empty()) {
            return rest.to_string();
        }
        format!("---\n{}\n---\n{rest}", kept.join("\n"))
    }

    /// The Carnet as it was before the sommaire: every page of the shared part without the header lines.
    fn as_before_the_sommaire(config: &Config) {
        let root = part_root(config, Part::Main);
        for (rel, text) in files_in(&root) {
            if rel.ends_with(".md") {
                std::fs::write(root.join(&rel), without_header(&text)).unwrap();
            }
        }
    }

    fn stored_of(config: &Config, path: &str) -> Option<(Vec<String>, String)> {
        c::stored(&Page::parse(&read_page_text(config, Part::Main, path).unwrap(), ""))
    }

    #[test]
    fn only_the_header_lines_change() {
        let home = strings(&["home"]);
        // a page the bridge wrote: exactly where S1 writes them (set_front, render)
        let mut p = Page::new("subject", "Rénovation cuisine");
        p.body = "## En bref\nDevis reçu.\n\n## Où on en est\n- Devis reçu — *Home, 2026-11-01*\n".into();
        p.touch();
        let text = p.render();
        let mut q = p.clone();
        c::set_front(&mut q, &home, "auto");
        assert_eq!(with_header(&text, &home, "auto").unwrap(), q.render());
        // written by hand without a frontmatter: one is added, the rest as it was
        assert_eq!(with_header("# Jazz\n\nDu jazz le jeudi.\n", &strings(&["other"]), "auto").unwrap(),
                   "---\ncategories:\n  - other\ncategories_by: auto\n---\n# Jazz\n\nDu jazz le jeudi.\n");
        assert_eq!(with_header("Pas de titre\n", &home, "auto").unwrap(),
                   "---\ncategories:\n  - home\ncategories_by: auto\n---\nPas de titre\n");
        // the bridge's lines replaced where they are
        let filed = "---\ntype: subject\ncategories:\n  - other\ncategories_by: auto\nupdated: 2026-11-01\n---\n# X\n";
        assert_eq!(with_header(filed, &strings(&["home", "money"]), "auto").unwrap(),
                   "---\ntype: subject\ncategories:\n  - home\n  - money\ncategories_by: auto\nupdated: 2026-11-01\n---\n# X\n");
        // no `updated`: at the end of the frontmatter
        assert_eq!(with_header("---\ntype: subject\naliases: []\n---\n# X\n", &home, "auto").unwrap(),
                   "---\ntype: subject\naliases: []\ncategories:\n  - home\ncategories_by: auto\n---\n# X\n");
        // a hand-written frontmatter: its comments, spaces and order kept byte for byte
        let hand = "---\ntype:   subject   # mine\ntags: [a, b]\nupdated: 2026-11-01\n---\n\n# X\nbody\n\n---\nnot a frontmatter\n";
        assert_eq!(with_header(hand, &home, "agent").unwrap(),
                   "---\ntype:   subject   # mine\ntags: [a, b]\ncategories:\n  - home\ncategories_by: agent\nupdated: 2026-11-01\n---\n\n# X\nbody\n\n---\nnot a frontmatter\n");
        // what the bridge cannot change safely: left alone
        // (review 1, R3: a page with CRLF line endings, with or without a frontmatter; R4: an empty frontmatter with
        // a rule further down, whose « key: value » lines the parser would read as one)
        for odd in ["# X\r\nbody\r\n", "---\n---\n# Réunion\nDate: 12 mars\n\n---\nNotes\n",
                    "---\r\ntype: subject\r\n---\r\n# X\r\n", "\u{feff}---\ntype: subject\n---\n# X\n",
                    "---\ntype: [\n---\n# X\n", "---\ntype: subject\n# X\n", "--- \ntype: subject\n---\n# X\n"] {
            assert_eq!(with_header(odd, &home, "auto"), None, "{odd:?}");
        }
        // read back as written, and the rest of the page the same
        let after = with_header(hand, &home, "agent").unwrap();
        assert_eq!(c::stored(&Page::parse(&after, "")), Some((home.clone(), "agent".into())));
        assert_eq!(without_header(&after), hand);
    }

    #[test]
    fn the_demo_carnet_is_sorted_once_and_nothing_moves() {
        for lang in ["fr", "en", "de", "es", "nl"] {
            let dir = tempfile::tempdir().unwrap();
            let config = tests_config(dir.path());
            crate::notebook::search_bench::write_demo_for(&config, lang, chrono::Local::now().date_naive());
            as_before_the_sommaire(&config);
            let root = crate::notebook::root(&config);
            let carnet = |c: &Config| files_in(&part_root(c, Part::Main));
            let private = files_in(&part_root(&config, Part::Health));
            let before = carnet(&config);
            // what S1 reads on the fly for each page: what the pass writes
            let read: BTreeMap<String, Vec<String>> = all_pages(&config, Part::Main)
                .iter()
                .map(|(path, p)| (path.clone(), c::of_page(&config, Part::Main, path, p).categories))
                .collect();
            let sorted = sort(&config).unwrap();
            assert!(sorted.done && sorted.filed > 10 && sorted.skipped == 0, "{lang}: {sorted:?}");
            let after = carnet(&config);
            // nothing moved, nothing added or removed; each page changed by its header lines only
            assert_eq!(before.keys().collect::<Vec<_>>(), after.keys().collect::<Vec<_>>(), "{lang}");
            for (path, text) in &after {
                assert_eq!(without_header(text), before[path], "{lang} {path}");
            }
            assert_eq!(files_in(&part_root(&config, Part::Health)), private, "{lang}: the private part is never touched");
            // the categories S1 read on the fly, now written (the tasks page, the inbox and the documents' index aside)
            let w = crate::notebook::words(&crate::notebook::language(&config));
            for (path, cats) in &read {
                let page = Page::parse(&read_page_text(&config, Part::Main, path).unwrap(), "");
                let structural = ["tasks", "documents"].contains(&page.kind().as_str()) || *path == w.inbox_path();
                if structural {
                    assert_eq!(&after[path], &before[path], "{lang} {path}");
                } else {
                    assert_eq!(c::stored(&page), Some((cats.clone(), "auto".to_string())), "{lang} {path}");
                }
            }
            // one change each in their history, by the « rangement », never in « Nouveau »
            let log = history(&config, Some(Part::Main), None, usize::MAX);
            let written: Vec<&Dict> = log.iter().filter(|e| e["action"] == c::ACTION).collect();
            assert_eq!(written.len(), sorted.filed, "{lang}");
            assert!(written.iter().all(|e| e["by"] == "nightly"), "{lang}");
            assert!(crate::notebook::recent_changes(&config, Part::Main, 50).iter().all(|e| e["action"] != c::ACTION));
            // once: the next night writes nothing, the files stay as they are
            let again = sort(&config).unwrap();
            assert_eq!((again.filed, again.done), (0, false), "{lang}");
            assert_eq!(carnet(&config), after, "{lang}");
            let state = read_state(&config);
            assert_eq!(state.get(KEY_V), Some(&json!(VERSION)));
            assert!(state.get(KEY_PASS).is_none());
            assert!(root.join("state.json").exists());
        }
    }

    #[test]
    fn the_persons_and_the_agents_choices_are_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        let write = |path: &str, text: &str| write_file(&root.join(path), text).unwrap();
        let pages = [
            ("Sujets/Rénovation cuisine.md", "---\ntype: subject\ncategories:\n  - money\ncategories_by: person\n---\n# Rénovation cuisine\n"),
            ("Sujets/Voyage Japon.md", "---\ntype: subject\ncategories:\n  - work\ncategories_by: agent\n---\n# Voyage Japon\n"),
            // written by hand in Obsidian: the person's, whether the bridge reads them or not
            ("Sujets/Impôts.md", "---\ntype: subject\ncategories: money\n---\n# Impôts\n"),
            ("Sujets/Lectures.md", "---\ntype: subject\ncategories:\n  - \"[[Livres]]\"\n  - romans\n  - hiver\n---\n# Lectures\n"),
            ("Sujets/Jardin potager.md", "---\ntype: subject\ncategories_by: person\n---\n# Jardin potager\n"),
        ];
        for (path, text) in pages {
            write(path, text);
        }
        // the bridge's own: re-decided at a new version only
        write("Domaines/Maison.md", "---\ntype: area\ncategories:\n  - work\ncategories_by: auto\n---\n# Maison\n\n## En bref\nL'appartement et les travaux.\n");
        write("Sujets/Jazz.md", "---\ntype: subject\ncategories:\n  - other\ncategories_by: auto\n---\n# Jazz\n");
        let first = sort(&config).unwrap();
        assert!(first.done);
        for (path, text) in pages {
            assert_eq!(read_page_text(&config, Part::Main, path).unwrap(), text, "{path}");
        }
        assert_eq!(stored_of(&config, "Domaines/Maison.md"), Some((strings(&["home"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Jazz.md"), Some((strings(&["other"]), "auto".into())));
        // between versions: an `auto` page keeps its categories; one in « Autres » is filed by a rule the person taught
        write("Domaines/Maison.md", "---\ntype: area\ncategories:\n  - work\ncategories_by: auto\n---\n# Maison\n\n## En bref\nL'appartement et les travaux.\n");
        update_state(&config, |s| {
            s.insert("category_rules".into(), json!([{"word": "jazz", "categories": ["home"], "at": "2026-11-22"}]));
        })
        .unwrap();
        let night = sort(&config).unwrap();
        assert_eq!((night.filed, night.done), (1, false));
        assert_eq!(stored_of(&config, "Domaines/Maison.md"), Some((strings(&["work"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Jazz.md"), Some((strings(&["home"]), "auto".into())));
        // a better table: the `auto` pages once more, never the person's nor the agents'
        let next = sort_with(&config, VERSION + 1, None).unwrap();
        assert!(next.done && next.filed == 1, "{next:?}");
        assert_eq!(stored_of(&config, "Domaines/Maison.md"), Some((strings(&["home"]), "auto".into())));
        for (path, text) in pages {
            assert_eq!(read_page_text(&config, Part::Main, path).unwrap(), text, "{path}");
        }
        assert_eq!(read_state(&config).get(KEY_V), Some(&json!(VERSION + 1)));
    }

    /// carnet-categories (22/11): the better table is version 2. A Carnet the pass went through at version 1 is
    /// re-decided once, `auto` pages only (a page in « Maison » by its first line's « maison » goes to Voyages); the
    /// agent's and the person's choices stay, the person's rule still wins, nothing moves and the card is not shown
    /// again.
    #[test]
    fn the_better_table_resorts_the_auto_pages_once() {
        assert_eq!(VERSION, 2);
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        let write = |path: &str, text: &str| write_file(&root.join(path), text).unwrap();
        let page = |cats: &str, by: &str, title: &str, brief: &str| {
            format!("---\ntype: subject\ncategories:\n  - {cats}\ncategories_by: {by}\n---\n# {title}\n\n## En bref\n{brief}\n")
        };
        // as version 1 left them
        write("Sujets/Gîte à Biarritz.md", &page("home", "auto", "Gîte à Biarritz", "Maison près de la plage, en août."));
        write("Sujets/Cours de piano.md", &page("money", "auto", "Cours de piano", "Le mardi, 25 € la séance."));
        write("Sujets/Billets de train.md", &page("other", "auto", "Billets de train", "Pour Lyon."));
        let chosen = [
            ("Sujets/Gîte en Ardèche.md", page("home", "person", "Gîte en Ardèche", "Maison près de la rivière.")),
            ("Sujets/Location de ski.md", page("money", "agent", "Location de ski", "Forfaits à réserver.")),
        ];
        for (path, text) in &chosen {
            write(path, text);
        }
        update_state(&config, |s| {
            s.insert(KEY_V.into(), json!(1));
            s.insert(KEY_CARD.into(), json!({"v": 1, "at": "2026-11-20", "done": true, "answer": "ok"}));
            s.insert("category_rules".into(), json!([{"word": "piano", "categories": ["people"], "at": "2026-11-21"}]));
        })
        .unwrap();
        let before = files_in(&root);
        let sorted = sort(&config).unwrap();
        assert!(sorted.done && sorted.filed == 3, "{sorted:?}");
        assert_eq!(stored_of(&config, "Sujets/Gîte à Biarritz.md"), Some((strings(&["trips"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Cours de piano.md"), Some((strings(&["people"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Billets de train.md"), Some((strings(&["trips"]), "auto".into())));
        for (path, text) in &chosen {
            assert_eq!(&read_page_text(&config, Part::Main, path).unwrap(), text, "{path}");
        }
        let after = files_in(&root);
        assert_eq!(after.keys().filter(|k| k.ends_with(".md")).collect::<Vec<_>>(),
                   before.keys().filter(|k| k.ends_with(".md")).collect::<Vec<_>>());
        for (path, text) in &after {
            if path.ends_with(".md") {
                assert_eq!(without_header(text), without_header(&before[path]), "{path}");
            }
        }
        let state = read_state(&config);
        assert_eq!(state.get(KEY_V), Some(&json!(2)));
        assert_eq!(state.get(KEY_CARD).and_then(|c| c.get("v")), Some(&json!(1)));
        // once: the next night writes nothing
        assert_eq!(sort(&config).unwrap().filed, 0);
    }

    #[test]
    fn a_page_a_work_agent_made_is_work_not_one_it_wrote_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir, "fr");
        for (id, name, category) in [("kerfy", "Kerfy Studio", Some("work")), ("quotidien", "Quotidien", None)] {
            let agent = crate::config::AgentConfig {
                name: id.into(),
                display_name: name.into(),
                category: category.map(str::to_string),
                ..Default::default()
            };
            config.agents.insert(id.into(), agent);
        }
        let (pro, daily) = (Who::agent(&config, "kerfy"), Who::agent(&config, "quotidien"));
        note(&config, Part::Main, "subject", "Lancement", "status", &strings(&["Site en ligne"]), &[], "", "", &pro).unwrap();
        // made by the daily agent, then written on by the work agent (S1 review B1: not Travail for that)
        note(&config, Part::Main, "area", "Maison", "matters", &strings(&["Poubelles le lundi"]), &[], "", "", &daily).unwrap();
        note(&config, Part::Main, "area", "Maison", "matters", &strings(&["Bureau dans la chambre"]), &[], "", "", &pro).unwrap();
        // written by hand in Obsidian: no maker, its words
        write_file(&part_root(&config, Part::Main).join("Sujets/Lancement presse.md"),
                   "---\ntype: subject\n---\n# Lancement presse\n\n## Où on en est\n- Presse le 3 — *Kerfy Studio, 2026-11-02*\n").unwrap();
        // made by the work agent, deleted, then made again by the person at the same path (review 1, R6): theirs
        note(&config, Part::Main, "subject", "Bilan", "status", &strings(&["Rapport livré"]), &[], "", "", &pro).unwrap();
        crate::notebook::delete_page(&config, Part::Main, "Sujets/Bilan.md", &Who::you(&config)).unwrap();
        crate::notebook::new_page(&config, Part::Main, "subject", "Bilan", &Who::you(&config)).unwrap().unwrap();
        // made by the work agent, then deleted and written again by hand
        note(&config, Part::Main, "subject", "Atelier", "status", &strings(&["Lancé"]), &[], "", "", &pro).unwrap();
        crate::notebook::delete_page(&config, Part::Main, "Sujets/Atelier.md", &Who::you(&config)).unwrap();
        write_file(&part_root(&config, Part::Main).join("Sujets/Atelier.md"), "---\ntype: subject\n---\n# Atelier\n").unwrap();
        as_before_the_sommaire(&config);
        sort(&config).unwrap();
        assert_eq!(stored_of(&config, "Sujets/Bilan.md").unwrap().0, ["other"]);
        assert_eq!(stored_of(&config, "Sujets/Atelier.md").unwrap().0, ["other"]);
        assert_eq!(stored_of(&config, "Sujets/Lancement.md"), Some((strings(&["work"]), "auto".into())));
        assert_eq!(stored_of(&config, "Domaines/Maison.md"), Some((strings(&["home"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Lancement presse.md"), Some((strings(&["other"]), "auto".into())));
    }

    /// `n` pages of the shared part, `lines` facts each, written as an older bridge did (no categories).
    fn synthetic(config: &Config, n: usize, lines: usize) {
        let root = part_root(config, Part::Main);
        let words = ["cuisine", "voyage", "facture", "contrat", "collègue", "jardin", "banque", "concert", "lecture", "assurance"];
        for i in 0..n {
            let (folder, kind) = match i % 4 {
                0 => ("Personnes", "person"),
                1 | 2 => ("Sujets", "subject"),
                _ => ("Domaines", "area"),
            };
            let word = words[i % words.len()];
            let mut text = format!("---\nid: p{i:010x}\ntype: {kind}\naliases: []\nupdated: 2026-10-01\n---\n# {word} {i:05}\n\n## En bref\nÀ propos de {word}.\n\n## Autres\n");
            for j in 0..lines {
                text.push_str(&format!("- Ligne {j} sur {word} et le reste, notée le 2026-10-01 — *Quotidien, 2026-10-01*\n"));
            }
            write_file(&root.join(format!("{folder}/{word} {i:05}.md")), &text).unwrap();
        }
    }

    #[test]
    fn a_pass_cut_short_starts_again_after_the_last_batch() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        synthetic(&config, 6 * BATCH, 3);
        let cut = sort_with(&config, VERSION, Some(2)).unwrap();
        assert_eq!((cut.filed, cut.done), (2 * BATCH, false));
        let state = read_state(&config);
        let paths: Vec<String> = all_pages(&config, Part::Main).into_iter().map(|(p, _)| p).collect();
        assert_eq!(state[KEY_PASS], json!({"v": VERSION, "after": print_of_path(&paths[2 * BATCH - 1])}));
        assert!(state.get(KEY_V).is_none() && state.get(KEY_CARD).is_none());
        let rest = sort(&config).unwrap();
        assert_eq!((rest.filed, rest.done), (4 * BATCH, true));
        // each page written once, none twice
        let log = history(&config, Some(Part::Main), None, usize::MAX);
        let mut seen: HashMap<String, usize> = HashMap::new();
        for e in log.iter().filter(|e| e["action"] == c::ACTION) {
            *seen.entry(tk::str_or_empty(e.get("path"))).or_default() += 1;
        }
        assert_eq!(seen.len(), 6 * BATCH);
        assert!(seen.values().all(|n| *n == 1));
        assert!(paths.iter().all(|p| stored_of(&config, p).is_some()));
        assert!(read_state(&config).get(KEY_PASS).is_none());
    }

    fn memory_kb(key: &str) -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }

    /// A Carnet of 20 000 lines (1 000 pages of 20): one pass, then the next nights' (nothing to write). Times and
    /// memory printed (`--nocapture`); the bounds are a debug build's, loose.
    #[test]
    fn a_carnet_of_twenty_thousand_lines_in_one_pass() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        synthetic(&config, 1000, 20);
        let rss = memory_kb("VmRSS:");
        let started = Instant::now();
        let first = sort(&config).unwrap();
        let took = started.elapsed();
        assert!(first.done && first.filed == 1000, "{first:?}");
        let started = Instant::now();
        let again = sort(&config).unwrap();
        let next = started.elapsed();
        assert_eq!(again.filed, 0);
        eprintln!("S2 pass, 20 000 lines: {took:?} (lock held {:?} at most), next night {next:?}; RSS {:?} → {:?} kB, peak {:?} kB",
                  first.longest_hold, rss, memory_kb("VmRSS:"), memory_kb("VmHWM:"));
        assert!(took < Duration::from_secs(120), "{took:?}");
        assert!(first.longest_hold < Duration::from_secs(10), "{:?}", first.longest_hold);
        assert!(next < Duration::from_secs(60), "{next:?}");
    }

    /// Out of the demo (S1 review R6): Carnets of page titles the demo never had, as an older bridge left them, sorted
    /// by the pass (only their headers change), and the measure — where the bridge files each against where a person
    /// would look (printed with `--nocapture`). Two held-out sets, both written before measuring: S1's
    /// (contracts/carnet/category-titles.json) and S2's (categories-out-of-demo.json, with the review's traps). The
    /// floors are today's table's: a change that files fewer of them where a person looks, or more of them in a wrong
    /// category (neither that one nor « Autres »), fails.
    #[test]
    fn the_pass_over_pages_from_outside_the_demo() {
        let mut lines = Vec::new();
        for (file, floor, ceiling) in OUT_OF_DEMO {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/carnet").join(file);
            let set: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let (mut all_where, mut all_wrong, mut all_n) = (0, 0, 0);
            for lang in ["fr", "en", "de", "es", "nl"] {
                let dir = tempfile::tempdir().unwrap();
                // a Dutch person's Carnet is in English words (the bridge has no Dutch folders)
                let config = config_in(&dir, if lang == "nl" { "en" } else { lang });
                let root = part_root(&config, Part::Main);
                let w = crate::notebook::words(&crate::notebook::language(&config));
                let cases = set["cases"][lang].as_array().unwrap();
                let path_of = |title: &str| format!("{}/{}.md", w.subjects, crate::notebook::file_name(title));
                for case in cases {
                    let title = case["title"].as_str().unwrap();
                    write_file(&root.join(path_of(title)), &format!("---\ntype: subject\naliases: []\n---\n# {title}\n")).unwrap();
                }
                let before = files_in(&root);
                assert_eq!(before.len(), cases.len(), "{file} {lang}: two titles share a file");
                let sorted = sort(&config).unwrap();
                assert_eq!((sorted.filed, sorted.skipped), (cases.len(), 0), "{file} {lang}");
                for (path, text) in files_in(&root) {
                    assert_eq!(without_header(&text), before[&path], "{file} {lang} {path}");
                }
                let (mut at, mut wrong, mut misses) = (0, 0, Vec::new());
                for case in cases {
                    let title = case["title"].as_str().unwrap();
                    // S2's set: `want` (where a person looks first, maybe a second); S1's: `category`
                    let want = match case.get("want") {
                        Some(w) => crate::notebook::strings(Some(w)),
                        None => vec![case["category"].as_str().unwrap().to_string()],
                    };
                    let (got, by) = stored_of(&config, &path_of(title)).unwrap();
                    assert_eq!(by, "auto");
                    if got.contains(&want[0]) {
                        at += 1;
                        continue;
                    }
                    // in « Autres » it waits for the night's model; elsewhere it is filed wrong
                    if got != ["other"] {
                        wrong += 1;
                    }
                    misses.push(format!("{title} → {} (want {})", got.join("+"), want.join("+")));
                }
                (all_where, all_wrong, all_n) = (all_where + at, all_wrong + wrong, all_n + cases.len());
                lines.push(format!("{file} {lang}: {at}/{}, {wrong} filed wrong; missed: {}", cases.len(), misses.join(" · ")));
            }
            lines.push(format!("{file}: {all_where}/{all_n} where a person looks, {all_wrong} filed wrong"));
            assert!(all_where >= floor && all_wrong <= ceiling, "{}", lines.join("\n"));
        }
        eprintln!("S2 out of the demo:\n{}", lines.join("\n"));
    }

    /// The held-out sets, the pages filed where a person looks at least, and in a wrong category at most (of 250 each):
    /// today's table's measure (22/11, with S1's compound words: S1's set 214 and 4, S2's 197 and 14; carnet-categories'
    /// table, version 2: 240 and 2, 235 and 9; after its review 1: 239 and 2 — « overleg » taken out, a false friend —,
    /// 236 and 8).
    const OUT_OF_DEMO: [(&str, usize, usize); 2] = [("category-titles.json", 239, 2), ("categories-out-of-demo.json", 236, 8)];

    /// A page written by hand (no history) stays the person's once the pass wrote its header: the next nights neither
    /// fold it into its group (groups.rs, a light person page) nor take it out when it is empty (archive.rs).
    #[test]
    fn a_page_written_by_hand_stays_the_persons_after_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        write_file(&root.join("Personnes/Léa.md"),
                   "---\ntype: person\naliases: []\n---\n# Léa\n\n## Dates\n- Anniversaire : 12 mars — *Wellness, 2026-10-01*\n").unwrap();
        write_file(&root.join("Sujets/Idées.md"), "---\ntype: subject\naliases: []\n---\n# Idées\n").unwrap();
        assert_eq!(sort(&config).unwrap().filed, 2);
        let sorted = files_in(&root);
        let today = chrono::Local::now().date_naive();
        crate::notebook::archive::run(&config, today).unwrap();
        assert_eq!(files_in(&root), sorted);
        // the rule itself unchanged: the same light page an agent wrote goes to its group
        note(&config, Part::Main, "person", "Paul", "dates", &strings(&["Anniversaire : 3 mai"]), &[], "", "", &Who::agent(&config, "home")).unwrap();
        assert!(root.join("Personnes/Paul.md").exists());
        crate::notebook::archive::run(&config, today).unwrap();
        assert!(!root.join("Personnes/Paul.md").exists() && root.join("Personnes/Léa.md").exists() && root.join("Sujets/Idées.md").exists());
    }

    /// Review 1, B1: the person's « Ranger ailleurs » on a page an agent wrote makes it theirs, as before S2: the next
    /// nights neither fold it into its group (a light person page) nor take it out (an empty page). The pass's and an
    /// agent's category lines do not.
    #[test]
    fn a_page_the_person_filed_stays_theirs_at_night() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        let (agent, you) = (Who::agent(&config, "home"), Who::you(&config));
        for name in ["Paul", "Hugo"] {
            note(&config, Part::Main, "person", name, "dates", &strings(&["Anniversaire : 3 mai"]), &[], "", "", &agent).unwrap();
        }
        for title in ["Idées", "Brouillon"] {
            crate::notebook::new_page(&config, Part::Main, "subject", title, &agent).unwrap().unwrap();
        }
        assert!(sort(&config).unwrap().done);
        for path in ["Personnes/Paul.md", "Sujets/Idées.md"] {
            c::categorize(&config, &c::Target::Page { path: path.into() }, &strings(&["work"]), &you).unwrap().unwrap();
        }
        crate::notebook::archive::run(&config, chrono::Local::now().date_naive()).unwrap();
        assert!(root.join("Personnes/Paul.md").exists() && root.join("Sujets/Idées.md").exists());
        assert_eq!(stored_of(&config, "Personnes/Paul.md"), Some((strings(&["work"]), "person".into())));
        // the agent's own, only filed by the pass: as before S2 (folded, taken out)
        assert!(!root.join("Personnes/Hugo.md").exists() && !root.join("Sujets/Brouillon.md").exists());
    }

    /// Review 1, R1: a page the bridge cannot write (a read-only folder on a self-install) is left and counted, the
    /// pass goes on and ends: the rest of the night is not lost, night after night.
    #[cfg(unix)]
    #[test]
    fn a_page_the_bridge_cannot_write_is_left_not_the_night() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        for path in ["Sujets/Jazz.md", "Domaines/Maison.md"] {
            write_file(&root.join(path), &format!("---\ntype: subject\n---\n# {}\n", &path[path.find('/').unwrap() + 1..path.len() - 3])).unwrap();
        }
        let locked = root.join("Sujets");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = std::fs::write(locked.join("probe"), "").is_ok(); // (root writes anywhere)
        let sorted = sort(&config);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            return;
        }
        let sorted = sorted.unwrap();
        assert_eq!((sorted.filed, sorted.skipped, sorted.done), (1, 1, true), "{sorted:?}");
        assert_eq!(stored_of(&config, "Domaines/Maison.md").unwrap().0, ["home"]);
        // the next night writes it
        assert_eq!(sort(&config).unwrap().filed, 1);
    }

    /// Review 1, R2: the card's count is not made again at each opening of the Carnet (the 1.0 apps never answer the
    /// card): kept until the Carnet changes (its history, the person's marks).
    #[test]
    fn the_cards_count_is_kept_until_the_carnet_changes() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let agent = Who::agent(&config, "home");
        note(&config, Part::Main, "subject", "Jazz", "status", &strings(&["Coltrane d'abord"]), &[], "", "", &agent).unwrap();
        note(&config, Part::Main, "subject", "Rénovation cuisine", "status", &strings(&["Devis reçu"]), &[], "", "", &agent).unwrap();
        sort(&config).unwrap();
        let count = |c: &Config| card(c, "fr", &all_pages(c, Part::Main)).unwrap()["other"].as_u64().unwrap();
        assert_eq!(count(&config), 2);
        // written outside the bridge (no history): the count kept
        let jazz = part_root(&config, Part::Main).join("Sujets/Jazz.md");
        std::fs::write(&jazz, std::fs::read_to_string(&jazz).unwrap() + "- Puis Mingus\n").unwrap();
        assert_eq!(count(&config), 2);
        // a change of the Carnet: counted again
        note(&config, Part::Main, "subject", "Jazz", "status", &strings(&["Et Monk"]), &[], "", "", &agent).unwrap();
        assert_eq!(count(&config), 4);
        // the person puts a line in « Autres » (a mark, no history): counted again
        let line = c::Target::Line { path: "Sujets/Rénovation cuisine.md".into(), text: "Devis reçu".into() };
        c::categorize(&config, &line, &strings(&["other"]), &Who::you(&config)).unwrap().unwrap();
        assert_eq!(count(&config), 5);
    }

    /// Review 1, R10: « Ranger ailleurs » on a page written by hand changes its header lines only (its comments, its
    /// spacing kept), and « Annuler » takes them out again: the page as it was, byte for byte.
    #[test]
    fn a_refiling_and_its_undo_change_the_header_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        let hand = "---\ntype:   subject   # mine\ntags: [a, b]\n---\n\n# Jazz\nColtrane.\n";
        write_file(&root.join("Sujets/Jazz.md"), hand).unwrap();
        let bare = "# Lectures\nRomans.\n";
        write_file(&root.join("Sujets/Lectures.md"), bare).unwrap();
        let you = Who::you(&config);
        let home = strings(&["home"]);
        let done = c::categorize(&config, &c::Target::Page { path: "Sujets/Jazz.md".into() }, &home, &you).unwrap().unwrap();
        assert_eq!(read_page_text(&config, Part::Main, "Sujets/Jazz.md").unwrap(), with_header(hand, &home, "person").unwrap());
        c::undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!(read_page_text(&config, Part::Main, "Sujets/Jazz.md").unwrap(), hand);
        let done = c::categorize(&config, &c::Target::Page { path: "Sujets/Lectures.md".into() }, &home, &you).unwrap().unwrap();
        assert_eq!(read_page_text(&config, Part::Main, "Sujets/Lectures.md").unwrap(), with_header(bare, &home, "person").unwrap());
        c::undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!(read_page_text(&config, Part::Main, "Sujets/Lectures.md").unwrap(), bare);
    }

    #[test]
    fn the_card_once_with_what_is_in_other() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        // an empty Carnet: nothing to announce
        assert!(sort(&config).unwrap().done);
        assert_eq!(card(&config, "fr", &all_pages(&config, Part::Main)), None);
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let agent = Who::agent(&config, "home");
        let facts = |list: &[&str]| strings(list);
        note(&config, Part::Main, "subject", "Rénovation cuisine", "status", &facts(&["Devis reçu"]), &[], "", "", &agent).unwrap();
        note(&config, Part::Main, "subject", "Jazz", "status", &facts(&["Coltrane d'abord", "Puis Mingus", "Et Monk"]), &[], "", "", &agent).unwrap();
        note(&config, Part::Main, "subject", "Divers", "status", &facts(&["Idée en vrac", "Autre idée"]), &[], "", "", &agent).unwrap();
        crate::notebook::add_task(&config, Part::Main, "Trier les photos", "", "", "asked", "", &agent).unwrap();
        as_before_the_sommaire(&config);
        let you = Who::you(&config);
        let line = c::Target::Line { path: "Sujets/Rénovation cuisine.md".into(), text: "Devis reçu".into() };
        c::categorize(&config, &line, &strings(&["other"]), &you).unwrap().unwrap();
        assert!(sort(&config).unwrap().done);
        let pages = all_pages(&config, Part::Main);
        // Jazz and its 3 lines, Divers and its 2, the task without a page, the line the person put there
        assert_eq!(other_count(&config, &pages), 9);
        assert_eq!(card(&config, "fr", &pages), Some(json!({"title": "Ton Carnet est rangé par catégories",
            "text": "9 notes dans Autres", "other": 9, "see": "Voir", "ok": "OK", "open": {"category": "other"}})));
        assert_eq!(card(&config, "en", &pages).unwrap()["text"], "9 notes in Other");
        assert_eq!(card(&config, "de", &pages).unwrap()["title"], "Dein Notizbuch ist nach Kategorien geordnet");
        assert_eq!(card(&config, "es", &pages).unwrap()["see"], "Ver");
        card_done(&config, "ok").unwrap();
        assert_eq!(card(&config, "fr", &pages), None);
        // once: a later version shows none
        assert!(sort_with(&config, VERSION + 1, None).unwrap().done);
        assert_eq!(card(&config, "fr", &all_pages(&config, Part::Main)), None);
        // nothing in « Autres »: the card without its second line nor « Voir »
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        note(&config, Part::Main, "subject", "Rénovation cuisine", "status", &facts(&["Devis reçu"]), &[], "", "", &agent).unwrap();
        sort(&config).unwrap();
        assert_eq!(card(&config, "fr", &all_pages(&config, Part::Main)), Some(json!({"title": "Ton Carnet est rangé par catégories",
            "text": null, "other": 0, "see": null, "ok": "OK", "open": {"category": "other"}})));
        assert_eq!(card(&config, "fr", &[]).unwrap()["other"], 0);
        // one note (made by the bridge: the count is kept until the Carnet changes)
        crate::notebook::new_page(&config, Part::Main, "subject", "Divers", &agent).unwrap().unwrap();
        assert_eq!(card(&config, "fr", &all_pages(&config, Part::Main)).unwrap()["text"], "1 note dans Autres");
    }

    #[test]
    fn what_stays_in_other_goes_to_the_night_model_twenty_a_night() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let root = part_root(&config, Part::Main);
        for i in 0..25 {
            // (review 1, R7: an alias that looks like health is never sent either)
            write_file(&root.join(format!("Sujets/Note {i:02}.md")),
                       &format!("---\ntype: subject\naliases:\n  - Allergie aux noix {i}\n---\n# Note {i:02}\n\n## En bref\nQuelque chose {i}.\n\n## Autres\n- Allergique aux noix\n- Ligne {i} — *Quotidien, 2026-11-01*\n")).unwrap();
        }
        // never sent: a page that looks like health, one the person filed in « Autres », the tasks and the inbox
        write_file(&root.join("Sujets/Dentiste.md"), "---\ntype: subject\n---\n# Dentiste\n").unwrap();
        write_file(&root.join("Sujets/Bazar.md"), "---\ntype: subject\ncategories:\n  - other\ncategories_by: person\n---\n# Bazar\n").unwrap();
        let agent = Who::agent(&config, "home");
        crate::notebook::add_task(&config, Part::Main, "Trier les photos", "", "", "asked", "", &agent).unwrap();
        sort(&config).unwrap();
        let first = candidates(&config, NIGHT_MAX);
        assert_eq!(first.len(), NIGHT_MAX);
        assert_eq!(first[0].path, "Sujets/Note 00.md");
        assert!(first.iter().all(|c| c.path.starts_with("Sujets/Note ")), "{first:?}");
        let input = night_input(&first);
        assert!(input.starts_with("1. Note 00 (subject)"), "{input}");
        assert!(input.contains("\n20. Note 19 (subject)") && input.contains("Ligne 3"), "{input}");
        assert!(!input.to_lowercase().contains("noix"), "a health line is never sent: {input}");
        assert!(first.iter().all(|c| tk::chars(&c.excerpt) <= EXCERPT));
        // the person files one meanwhile: never overridden
        c::categorize(&config, &c::Target::Page { path: "Sujets/Note 04.md".into() }, &strings(&["money"]), &Who::you(&config)).unwrap().unwrap();
        let answer = "{\"n\": 1, \"categories\": [\"home\"]}\n{\"n\": 2, \"categories\": [\"dates\"]}\n{\"n\": 3, \"categories\": [\"health\"]}\nnot json\n\
                      {\"n\": 99, \"categories\": [\"home\"]}\n{\"n\": \"4\", \"categories\": [\"work\", \"money\"]}\n{\"n\": 5, \"categories\": [\"home\"]}\n\
                      {\"n\": 6, \"categories\": [\"home\", \"money\", \"work\"]}";
        assert_eq!(apply_answer(&config, &first, answer).unwrap(), 2);
        assert_eq!(stored_of(&config, "Sujets/Note 00.md"), Some((strings(&["home"]), "agent".into())));
        assert_eq!(stored_of(&config, "Sujets/Note 03.md"), Some((strings(&["work", "money"]), "agent".into())));
        assert_eq!(stored_of(&config, "Sujets/Note 04.md"), Some((strings(&["money"]), "person".into())));
        assert_eq!(stored_of(&config, "Sujets/Note 01.md"), Some((strings(&["other"]), "auto".into())));
        // the next night: the 5 not shown yet, then none
        let second = candidates(&config, NIGHT_MAX);
        assert_eq!(second.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
                   ["Sujets/Note 20.md", "Sujets/Note 21.md", "Sujets/Note 22.md", "Sujets/Note 23.md", "Sujets/Note 24.md"]);
        // (review 1, R9) an answer as one JSON array, in a code fence: read too
        let fenced = "```json\n[{\"n\": 1, \"categories\": [\"home\"]}, {\"n\": 2, \"categories\": [\"trips\"]}]\n```";
        assert_eq!(apply_answer(&config, &second, fenced).unwrap(), 2);
        assert_eq!(stored_of(&config, "Sujets/Note 21.md"), Some((strings(&["trips"]), "agent".into())));
        assert!(candidates(&config, NIGHT_MAX).is_empty());
        // a page whose text changed is shown again; the state keeps fingerprints, never a text
        let changed = read_page_text(&config, Part::Main, "Sujets/Note 01.md").unwrap() + "- Une ligne de plus\n";
        write_file(&root.join("Sujets/Note 01.md"), &changed).unwrap();
        assert_eq!(candidates(&config, NIGHT_MAX).iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), ["Sujets/Note 01.md"]);
        let tried = read_state(&config)[KEY_TRIED].clone();
        assert!(tried.as_array().unwrap().iter().all(|t| t.as_str().is_some_and(|t| t.len() == 16 && t.chars().all(|ch| ch.is_ascii_hexdigit()))),
                "{tried}");
        // the model's writes: in the page's history, by the « rangement »
        let log = history(&config, Some(Part::Main), Some("Sujets/Note 00.md"), 10);
        assert_eq!((log[0]["action"].clone(), log[0]["by"].clone()), (json!(c::ACTION), json!("nightly")));
    }

    #[test]
    fn a_refiling_says_where_and_stays_undoable_all_day() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let agent = Who::agent(&config, "home");
        let you = Who::you(&config);
        for title in ["Rénovation cuisine", "Voyage Japon", "Impôts 2026"] {
            note(&config, Part::Main, "subject", title, "status", &strings(&["Une ligne"]), &[], "", "", &agent).unwrap();
        }
        let kitchen = c::Target::Page { path: "Sujets/Rénovation cuisine.md".into() };
        let done = c::categorize(&config, &kitchen, &strings(&["money"]), &you).unwrap().unwrap();
        let shown = with_toast(done.clone(), "fr");
        assert_eq!((shown["toast"].clone(), shown["undo_title"].clone()), (json!("Rangé dans Argent."), json!("Annuler")));
        assert_eq!(shown["undo"], done["undo"]);
        assert_eq!(toast("fr", &strings(&["money", "home"])), "Rangé dans Argent et Maison.");
        assert_eq!(toast("en", &strings(&["money"])), "Filed in Money.");
        assert_eq!(toast("de", &strings(&["money", "home"])), "In Geld und Zuhause abgelegt.");
        assert_eq!(toast("es", &strings(&["trips"])), "Guardado en Viajes.");
        assert_eq!((undo_title("en"), undo_title("de"), undo_title("es")), ("Undo", "Rückgängig", "Deshacer"));
        // today's list: what it is, where it went, its undo
        let list = today(&config, "fr");
        assert_eq!(list["undo_title"], "Annuler");
        let first = &list["items"][0];
        assert_eq!((first["undo"].clone(), first["title"].clone(), first["text"].clone(), first["categories"].clone()),
                   (done["undo"].clone(), json!("Rénovation cuisine"), json!("Rangé dans Argent."), json!(["money"])));
        assert_eq!(first["before"], json!({"categories": ["home"], "categories_by": "auto"}));
        assert_eq!(first["target"], done["target"]);
        assert_eq!(first["target"]["path"], "Sujets/Rénovation cuisine.md");
        // a whole day of re-filings: the first stays undoable after 30 more
        for i in 0..30 {
            let title = format!("Page {i:02}");
            note(&config, Part::Main, "subject", &title, "status", &strings(&["Une ligne"]), &[], "", "", &agent).unwrap();
            let page = c::Target::Page { path: format!("Sujets/{title}.md") };
            c::categorize(&config, &page, &strings(&["work"]), &you).unwrap().unwrap();
        }
        let japan = c::Target::Page { path: "Sujets/Voyage Japon.md".into() };
        assert_eq!(today(&config, "fr")["items"].as_array().unwrap().len(), 31);
        assert!(c::undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().is_some());
        assert_eq!(today(&config, "fr")["items"].as_array().unwrap().len(), 30);
        // yesterday's beyond the 20 latest: no longer kept
        update_state(&config, |s| {
            for e in s.get_mut("category_undo").unwrap().as_array_mut().unwrap() {
                e["at"] = json!("2026-01-01");
            }
        })
        .unwrap();
        c::categorize(&config, &japan, &strings(&["work"]), &you).unwrap().unwrap();
        assert_eq!(read_state(&config)["category_undo"].as_array().unwrap().len(), c::UNDO_KEPT);
        assert_eq!(today(&config, "fr")["items"].as_array().unwrap().len(), 1);
        // a line re-filed, then gone from its page (forgotten, edited): not listed
        let line = c::Target::Line { path: "Sujets/Impôts 2026.md".into(), text: "Une ligne".into() };
        c::categorize(&config, &line, &strings(&["money"]), &you).unwrap().unwrap();
        assert_eq!(today(&config, "fr")["items"][0]["title"], "Une ligne");
        let text = read_page_text(&config, Part::Main, "Sujets/Impôts 2026.md").unwrap();
        let gone: String = text.lines().filter(|l| !l.contains("Une ligne")).map(|l| format!("{l}\n")).collect();
        write_file(&part_root(&config, Part::Main).join("Sujets/Impôts 2026.md"), &gone).unwrap();
        assert!(today(&config, "fr")["items"].as_array().unwrap().iter().all(|i| i["title"] != "Une ligne"));
    }

    // -- the one-time sort after a new version (carnet-categories, 22/11) ----------------------------------------------

    /// A fake small model: each numbered note of the input answered `home` (« other » for « Truc 000 » to « Truc 009 »);
    /// its usage what the bridge counts (characters / 3.5 in, 15 tokens a note out). Each input kept.
    async fn fake_model(seen: Arc<Mutex<Vec<String>>>) -> String {
        use axum::extract::State;
        use axum::routing::post;
        async fn chat(State(seen): State<Arc<Mutex<Vec<String>>>>, axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
            let system = body["messages"][0]["content"].as_str().unwrap_or("").to_string();
            let user = body["messages"][1]["content"].as_str().unwrap_or("").to_string();
            seen.lock().unwrap().push(user.clone());
            let numbers: Vec<(usize, bool)> = user
                .lines()
                .filter_map(|l| l.split_once(". ").and_then(|(n, rest)| Some((n.parse().ok()?, rest.starts_with("Truc 00")))))
                .collect();
            let lines: Vec<String> = numbers
                .iter()
                .map(|(n, other)| json!({"n": n, "categories": [if *other { "other" } else { "home" }]}).to_string())
                .collect();
            axum::Json(json!({"choices": [{"message": {"content": lines.join("\n")}}],
                              "usage": {"prompt_tokens": ((system.len() + user.len()) as f64 / 3.5).ceil() as i64,
                                        "completion_tokens": 15 * numbers.len()}}))
        }
        let app = axum::Router::new().route("/v1/chat/completions", post(chat)).with_state(seen);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{address}/v1")
    }

    use std::sync::Arc;

    /// A night as tidy.rs records it: the « Autres » turn, then the run's spending saved.
    async fn night_recorded(config: &Config) -> Dict {
        let mut record = Dict::new();
        night(config, &reqwest::Client::new(), &mut record).await.unwrap();
        tidy::save_run(config, &record);
        record
    }

    fn eur(config: &Config) -> f64 {
        tidy::spending(config)["eur"].as_f64().unwrap()
    }

    /// The notes each input carried, by their title.
    fn titles_sent(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock().unwrap().iter().flat_map(|u| u.lines().map(|l| l.split_once(". ").unwrap().1.split(" (").next().unwrap().to_string()).collect::<Vec<_>>()).collect()
    }

    /// A Carnet as S2's first pass leaves it: 150 pages no word placed, 70 the words placed by their first line only
    /// (narrowly), 10 placed surely, one that looks like health, one the person filed in « Autres », one a rule of
    /// theirs placed, one a hobby's word put in « Autres » on purpose.
    fn carnet_for_the_model(config: &Config) {
        let root = part_root(config, Part::Main);
        let page = |title: &str, brief: &str| format!("---\ntype: subject\n---\n# {title}\n\n## En bref\n{brief}\n");
        for i in 0..150 {
            write_file(&root.join(format!("Sujets/Note {i:03}.md")), &page(&format!("Note {i:03}"), &format!("Quelque chose {i}."))).unwrap();
        }
        for i in 0..70 {
            write_file(&root.join(format!("Sujets/Truc {i:03}.md")), &format!("{}\n## Autres\n- Allergique aux noix\n",
                page(&format!("Truc {i:03}"), "Le jardin."))).unwrap();
        }
        for i in 0..10 {
            write_file(&root.join(format!("Sujets/Rénovation cuisine {i}.md")), &page(&format!("Rénovation cuisine {i}"), "Devis reçu.")).unwrap();
        }
        write_file(&root.join("Sujets/Dentiste.md"), &page("Dentiste", "Mardi.")).unwrap();
        write_file(&root.join("Sujets/Bazar.md"), "---\ntype: subject\ncategories:\n  - other\ncategories_by: person\n---\n# Bazar\n").unwrap();
        write_file(&root.join("Sujets/Bidule.md"), &page("Bidule", "Le jardin.")).unwrap();
        write_file(&root.join("Sujets/Cours de piano.md"), &page("Cours de piano", "Le mardi, 25 € la séance.")).unwrap();
        update_state(config, |s| {
            s.insert("category_rules".into(), json!([{"word": "bidule", "categories": ["work"], "at": "2026-11-22"}]));
        })
        .unwrap();
    }

    fn with_model(dir: &tempfile::TempDir, base: &str) -> Config {
        let mut config = config_in(dir, "fr");
        config.notebook.base_url = base.into();
        config.notebook.key = Some("sk-lab".into());
        config.notebook.model = "small-eu".into();
        config
    }

    #[test]
    fn narrowly_placed_pages_are_those_the_words_hardly_placed() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let ctx = Ctx::new(&config);
        let narrow = |title: &str, brief: &str, kind: &str| {
            let mut p = Page::new(kind, title);
            p.body = format!("## En bref\n{brief}\n");
            c::narrow_in(&ctx, Part::Main, "Sujets/x.md", &p)
        };
        // only the first line, a neighbour, or two title words as strong as each other
        assert!(narrow("Truc", "Le jardin.", "subject"));
        assert!(narrow("Toutou", "", "subject"));
        assert!(narrow("Voisins et jardin", "", "subject"));
        // a table word of the title's head, a hobby (« Autres »), a person, a trip: not narrow
        assert!(!narrow("Rénovation cuisine", "Le devis.", "subject"));
        assert!(!narrow("Dépenses de Noël", "", "subject"));
        assert!(!narrow("Factures EDF", "", "subject"));
        assert!(!narrow("Cours de piano", "", "subject"));
        assert!(!narrow("Marc Durand", "Le jardin.", "person"));
        assert!(!narrow("Week-end", "Le jardin.", "trip"));
        assert!(!c::narrow_in(&ctx, Part::Health, "Sujets/x.md", &Page::new("subject", "Truc")));
        // a rule of the person's placed it: it applies first, never the model
        update_state(&config, |s| {
            s.insert("category_rules".into(), json!([{"word": "truc", "categories": ["work"], "at": "2026-11-22"}]));
        })
        .unwrap();
        let ctx = Ctx::new(&config);
        let mut p = Page::new("subject", "Truc");
        p.body = "## En bref\nLe jardin.\n".into();
        assert!(!c::narrow_in(&ctx, Part::Main, "Sujets/x.md", &p));
    }

    #[tokio::test]
    async fn the_one_time_sort_reads_up_to_200_pages_by_twenty_inside_the_cap() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = fake_model(seen.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let config = with_model(&dir, &base);
        carnet_for_the_model(&config);
        let c2 = config.clone();
        assert!(tokio::task::spawn_blocking(move || sort(&c2).unwrap()).await.unwrap().done);
        assert_eq!(read_state(&config)[KEY_ONCE], json!({"v": VERSION, "left": ONCE_MAX}));
        let run = night_recorded(&config).await;
        // 150 in « Autres », then 50 of the narrow ones: 10 calls of 20
        let inputs = seen.lock().unwrap().clone();
        assert_eq!(inputs.len(), 10, "{run:?}");
        assert!(inputs.iter().all(|u| u.lines().count() == NIGHT_MAX));
        let sent = titles_sent(&seen);
        assert_eq!(sent.len(), ONCE_MAX);
        assert_eq!(sent.iter().filter(|t| t.starts_with("Note ")).count(), 150);
        assert_eq!(sent.iter().filter(|t| t.starts_with("Truc ")).count(), 50);
        assert_eq!(sent.iter().collect::<HashSet<_>>().len(), ONCE_MAX, "a page sent twice");
        // never: a page placed surely, one that looks like health, the person's, one their rule placed; no health line
        assert!(sent.iter().all(|t| !t.starts_with("Rénovation") && !["Dentiste", "Bazar", "Bidule", "Cours de piano"].contains(&t.as_str())),
                "{sent:?}");
        assert!(inputs.iter().all(|u| !u.to_lowercase().contains("noix")));
        // what one page sends: its excerpt, `EXCERPT` characters at most
        assert!(inputs.iter().flat_map(|u| u.lines()).all(|l| tk::chars(l) <= EXCERPT + 5));
        // kept only when the model gives a real category: its « other » leaves the words' (Truc 000 to 009)
        assert_eq!(run["categorized_by_model"], json!(190));
        assert_eq!(stored_of(&config, "Sujets/Note 000.md"), Some((strings(&["home"]), "agent".into())));
        assert_eq!(stored_of(&config, "Sujets/Truc 000.md"), Some((strings(&["home"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Truc 010.md"), Some((strings(&["home"]), "agent".into())));
        assert_eq!(stored_of(&config, "Sujets/Cours de piano.md"), Some((strings(&["other"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Truc 069.md"), Some((strings(&["home"]), "auto".into())));
        assert_eq!(stored_of(&config, "Sujets/Bidule.md"), Some((strings(&["work"]), "auto".into())));
        // within the cap, in the month's spending once (the run's record and the reckoning agree)
        assert_eq!(run["model_calls"], json!(10));
        assert!((eur(&config) - run["eur"].as_f64().unwrap()).abs() < 1e-9 && eur(&config) <= config.notebook.monthly_cap);
        assert!(!read_state(&config).contains_key(KEY_SPENT));
        // once per version: the next nights are the usual 20 of « Autres », none left untried here
        assert_eq!(read_state(&config)[KEY_ONCE]["left"], json!(0));
        night_recorded(&config).await;
        assert_eq!(seen.lock().unwrap().len(), 10);
        let c2 = config.clone();
        tokio::task::spawn_blocking(move || sort(&c2).unwrap()).await.unwrap();
        assert_eq!(read_state(&config)[KEY_ONCE]["left"], json!(0));
    }

    #[tokio::test]
    async fn the_one_time_sort_never_passes_the_cap_and_goes_on_the_next_months_nights() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = fake_model(seen.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = with_model(&dir, &base);
        config.notebook.monthly_cap = 0.0025;
        carnet_for_the_model(&config);
        let c2 = config.clone();
        tokio::task::spawn_blocking(move || sort(&c2).unwrap()).await.unwrap();
        let run = night_recorded(&config).await;
        let first = seen.lock().unwrap().len();
        assert!((1..10).contains(&first), "{first} calls");
        assert_eq!(run["categories_reason"], json!("monthly cap reached"));
        assert!(eur(&config) <= config.notebook.monthly_cap, "{}", eur(&config));
        assert_eq!(read_state(&config)[KEY_ONCE]["left"].as_u64().unwrap() as usize, ONCE_MAX - titles_sent(&seen).len());
        // the same month: nothing more
        night_recorded(&config).await;
        assert_eq!(seen.lock().unwrap().len(), first);
        assert!(eur(&config) <= config.notebook.monthly_cap);
        // the next month: it goes on, with pages not sent yet
        update_state(&config, |s| {
            s["tidy"]["spend"]["month"] = json!("2000-01");
        })
        .unwrap();
        night_recorded(&config).await;
        assert!(seen.lock().unwrap().len() > first);
        assert!(eur(&config) <= config.notebook.monthly_cap);
        let sent = titles_sent(&seen);
        assert_eq!(sent.iter().collect::<HashSet<_>>().len(), sent.len(), "a page sent twice");
    }

    #[tokio::test]
    async fn a_bridge_stopped_midway_spends_nothing_twice_and_sends_nothing_again() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = fake_model(seen.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let config = with_model(&dir, &base);
        carnet_for_the_model(&config);
        let c2 = config.clone();
        tokio::task::spawn_blocking(move || sort(&c2).unwrap()).await.unwrap();
        // stopped during a call: its pages marked as shown and its most cost reserved, nothing else
        let c2 = config.clone();
        let batch = tokio::task::spawn_blocking(move || candidates_once(&c2, NIGHT_MAX)).await.unwrap();
        let most = most_cost(&config, &night_input(&batch));
        reserve(&config, &batch, most, true).unwrap();
        // stopped after its calls, before the night was recorded: their cost is only in the reckoning
        let mut lost = Dict::new();
        night(&config, &reqwest::Client::new(), &mut lost).await.unwrap();
        let spent_then = read_state(&config)[KEY_SPENT].clone();
        assert!(spent_then["eur"].as_f64().unwrap() > 0.0 && (lost["eur"].as_f64().unwrap() - spent_then["eur"].as_f64().unwrap()).abs() < 1e-5);
        // the reserved call was counted at its most when that night began, once
        assert!((eur(&config) - (most * 1_000_000.0).round() / 1_000_000.0).abs() < 1e-9, "{}", eur(&config));
        // the next night counts the lost night once, sends none of their pages again
        let run = night_recorded(&config).await;
        let want = most + spent_then["eur"].as_f64().unwrap() + run.get("eur").and_then(Value::as_f64).unwrap_or(0.0);
        assert!((eur(&config) - want).abs() < 2e-6, "{} {want}", eur(&config));
        let sent = titles_sent(&seen);
        assert_eq!(sent.iter().collect::<HashSet<_>>().len(), sent.len(), "a page sent twice");
        let reserved: HashSet<String> = batch.iter().map(|c| c.excerpt.split(" (").next().unwrap().to_string()).collect();
        assert!(sent.iter().all(|t| !reserved.contains(t)));
        // 200 in all, the reserved call's 20 included: never more
        assert_eq!(sent.len() + batch.len(), ONCE_MAX);
        // every page's header whole: its categories read back
        for (path, _) in all_pages(&config, Part::Main) {
            let text = read_page_text(&config, Part::Main, &path).unwrap();
            assert!(c::stored(&Page::parse(&text, "")).is_some(), "{path}");
        }
    }

    #[tokio::test]
    async fn the_local_model_costs_nothing_and_reads_the_same_pages() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = fake_model(seen.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = with_model(&dir, &base);
        (config.notebook.price_in, config.notebook.price_out) = (0.0, 0.0);
        carnet_for_the_model(&config);
        let c2 = config.clone();
        tokio::task::spawn_blocking(move || sort(&c2).unwrap()).await.unwrap();
        let run = night_recorded(&config).await;
        assert_eq!((titles_sent(&seen).len(), run["eur"].clone(), eur(&config)), (ONCE_MAX, json!(0.0), 0.0));
    }
}
