//! The phone's search copy of the Carnet (plan « Le sommaire » §3.1, brick S3; contracts/carnet/sommaire.md §3,
//! sommaire.json `Index`): `GET /v1/notebook/index?since=&epoch=`, and the same copy searched by the bridge itself
//! (`GET /v1/notebook/search`'s `groups` and `counts`: finder.rs, the rule of contracts/carnet/search-parity.json).
//!
//! What it holds: the shared part's pages (title, aliases, « En bref »), their lines, the open tasks, the records
//! (never a number, a reference or a source), the documents the agents made or received and their web pages (their
//! name, their kind and the start of their text). Never: the private health part (Santé), a line holding an ID number
//! or a reference (`categories::holds_reference`: offline.rs `numbered`, « HR-55821 »), an archived mail's link,
//! an online file's content. Lines of the shared Carnet that look like health stay (Alex's decision 6).
//!
//! It never depends on the day it is made: no `dates` among the categories (the phone adds it from `date`/`yearly`
//! and `dated`), a day without a year read near when the line was noted (its attribution, else its page's `updated`,
//! else the file's day), never near today; the tables' feasts are rules. So its version moves only with the Carnet.
//!
//! Versions: the bridge keeps, in memory, each Carnet's copy as last sent (each item's hash) under an epoch drawn when
//! it starts keeping it (a bridge started again, a Carnet restored or moved: another epoch, the phone takes the whole
//! copy); each change of the copy is a new version, the last `HISTORY` of them kept as their changed and removed ids.
//! `since` + `epoch` of a version kept: only what changed (`full: false`); else everything (`full: true`). The ETag is
//! `"<epoch>-v<version>-<languages>"`; `If-None-Match` with it: 304. The kept searches (S4, summary.rs) go whole with
//! every answer, as the tables; a change of theirs is a version of its own (no item changed).
//!
//! Limits: 20 000 items and 8 MB of compact JSON. The bridge alone trims (the phone never does): archived lines out
//! first (the oldest noted first), then archived pages, then the other lines (the oldest noted first);
//! `truncated: true` and `online_only` (the bar's « Recherche complète en ligne »).

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use super::categories as c;
use super::finder::{Entry, Finder, Tables};
use super::{fold, records, Page, Part};
use crate::config::Config;
use crate::tools_knowledge as tk;

/// The contract's version of the answer (`Index.v`).
pub const V: u64 = 1;
/// The copy's limits (plan §3.1): items, and bytes of compact JSON as sent (before compression).
pub const LINES_MAX: usize = 20_000;
pub const BYTES_MAX: usize = 8_000_000;
/// Versions whose changes are kept (an older `since` gets the whole copy).
const HISTORY: usize = 64;
/// The words of a document's text in the copy (its start).
const DOCUMENT_TEXT: usize = 2048;
/// A document read for its text at most (bigger: its name and kind only).
const DOCUMENT_MAX: u64 = 20 * 1024 * 1024;

// -- the items ----------------------------------------------------------------------------------------------------------

/// « Budget 2026.xlsx », « Impôts 2026.md »: a real file's extension is no part of a reference; any other ending
/// is (« 12345.AB1 » holds one: review 2 of S3).
static EXTENSION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\.(?:pdf|docx?|odt|rtf|txt|md|html?|xlsx?|ods|csv|numbers|pptx?|odp|key|pages|jpe?g|png|heic|heif|gif|webp|tiff?|mp3|wav|m4a|aac|ogg|opus|flac|mp4|mov|m4v|webm|zip|json)$",
    )
    .expect("valid regex")
});

/// Whether a word (or a text) of the copy holds a reference (categories.rs `holds_reference`, quantities aside), a
/// real file's extension set aside.
pub fn reference(text: &str) -> bool {
    let words: Vec<String> = text.split_whitespace().map(|w| EXTENSION.replace(w, "").into_owned()).collect();
    c::holds_reference(&words.join(" "))
}

/// Whether a path or a file's name holds a reference (« Sujets/Passeport 18AB12345.md », « Facture FR-2026-00123.pdf
/// »): the item is not in the copy at all, as a line that holds one (review 1 of S3, B2: its `path` and `open` would
/// carry it to the phone).
pub fn named_reference(path: &str) -> bool {
    reference(&path.replace(['/', '\\'], " "))
}

/// A text with each word that holds a reference taken out (« Facture FR-2026-00123.pdf » → « Facture … »); None when
/// the text still holds one (a number written in several words): it is not sent at all.
pub fn masked(text: &str) -> Option<String> {
    if !reference(text) {
        return Some(text.to_string());
    }
    let out: Vec<&str> = text.split_whitespace().map(|w| if reference(w) { "…" } else { w }).collect();
    let out = out.join(" ");
    (!reference(&out)).then_some(out)
}

fn day_of(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
}

fn without_dates(cats: Vec<String>) -> Vec<String> {
    cats.into_iter().filter(|c| c != "dates").collect()
}

fn some(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

/// A document's kind in words, so that « présentation Japon » or « tableur budget » find it.
fn kind_words(name: &str, web_page: bool) -> &'static str {
    if web_page {
        return "page web html pagina seite webseite";
    }
    match super::file_type(name) {
        "pdf" => "pdf",
        "image" => "image photo",
        "document" => "document doc docx word texte",
        "spreadsheet" => "tableur tableau spreadsheet sheet excel xlsx tabelle hoja calculo",
        "presentation" => "presentation diaporama slides powerpoint pptx ppt prasentation diapositivas",
        "audio" => "audio",
        "video" => "video",
        _ => "",
    }
}

/// A document's file as read: its path, size and time.
type FileKey = (PathBuf, u64, u64);

/// Documents' texts read once (path, size, modified) → their first words, folded, references taken out.
static DOCUMENT_TEXTS: LazyLock<Mutex<HashMap<FileKey, String>>> = LazyLock::new(Default::default);

static TAGS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?is)<(script|style)\b.*?</(script|style)\s*>|<[^>]*>|&[a-z]+;|&#\d+;").expect("valid regex")
});

fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos() as u64;
    Some((meta.len(), modified))
}

/// Documents waiting to be read, and whether the reader runs (one thread, off the requests' way).
static TO_READ: LazyLock<Mutex<VecDeque<(FileKey, String)>>> = LazyLock::new(Default::default);
static READING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How many documents' texts were read: the copy is made again when a text arrives.
static TEXTS_READ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The tags that end a line of a web page (its paragraphs, titles, cells): read as line ends, so that a number in one
/// paragraph takes only that paragraph out.
static BLOCKS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)<(?:br|/?p|/?div|/?h[1-6]|/?li|/?tr|/?td|/?th|/?section|/?article)\b[^>]*>").expect("valid regex")
});

/// A word of a document kept in the copy: no digits, a year (19xx, 20xx) or a number of one or two digits. A number
/// written in groups (« FR76 3000 6000 0112 », « 4970 1012 3456 7890 ») leaves nothing of itself (review 1 of S3, B1).
fn plain_word(w: &str) -> bool {
    let digits = w.chars().filter(char::is_ascii_digit).count();
    digits == 0
        || (digits == w.len() && (w.len() <= 2 || (w.len() == 4 && (w.starts_with("19") || w.starts_with("20")))))
}

fn read_document(path: &Path, name: &str) -> String {
    let raw = if name.ends_with(".html") || name.ends_with(".htm") {
        std::fs::read_to_string(path)
            .map(|h| TAGS.replace_all(&BLOCKS.replace_all(&h, "\n"), " ").into_owned())
            .unwrap_or_default()
    } else if std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) <= DOCUMENT_MAX {
        match crate::doc_text::document_text(path, name, "") {
            crate::doc_text::Read::Text(t) => t,
            _ => String::new(),
        }
    } else {
        String::new()
    };
    // each raw line as written: none that holds an ID number or a reference (an IBAN, a card or a social security
    // number is read on its line, before folding splits it into words)
    let kept: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty() && !reference(l)).collect();
    let folded = fold(tk::head(&kept.join("\n"), DOCUMENT_TEXT * 2));
    let words: Vec<&str> = folded.split(' ').filter(|w| plain_word(w)).collect();
    tk::head(&words.join(" "), DOCUMENT_TEXT).to_string()
}

/// The start of a document's text (2 KB, folded, references out) when it was read already; else it is read by a
/// thread of its own (a PDF can take seconds) and the copy gets it at its next making.
fn document_text(path: &Path, name: &str) -> String {
    let Some((len, modified)) = stamp(path) else { return String::new() };
    let key = (path.to_path_buf(), len, modified);
    if let Some(text) = DOCUMENT_TEXTS.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return text.clone();
    }
    {
        let mut queue = TO_READ.lock().unwrap_or_else(|e| e.into_inner());
        if !queue.iter().any(|(k, _)| *k == key) {
            queue.push_back((key, name.to_string()));
        }
    }
    if READING.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
        std::thread::spawn(|| loop {
            let next = TO_READ.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            let Some((key, name)) = next else {
                READING.store(false, std::sync::atomic::Ordering::SeqCst);
                // (one queued meanwhile: read by the next call)
                break;
            };
            let text = read_document(&key.0, &name);
            let found = !text.is_empty();
            DOCUMENT_TEXTS.lock().unwrap_or_else(|e| e.into_inner()).insert(key, text);
            // the copy is made again for a text that says something only
            if found {
                TEXTS_READ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
    }
    String::new()
}

/// Where each document of `documents()` is on disk: (agent, file) → its path.
fn document_files(config: &Config) -> HashMap<(String, String), PathBuf> {
    let mut out: HashMap<(String, String), PathBuf> = HashMap::new();
    for agent in config.agents.values() {
        if let Some(root) = agent.upload_dir_host.as_deref().filter(|r| !r.is_empty()) {
            for month in std::fs::read_dir(root).into_iter().flatten().flatten() {
                for file in std::fs::read_dir(month.path()).into_iter().flatten().flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    let shown = match name.split_once('-') {
                        Some((id, rest)) if id.len() == 12 && !rest.is_empty() => rest.to_string(),
                        _ => name.clone(),
                    };
                    // two files of the same name: the newest (the one the copy lists; review 2 of S3)
                    let newer = |p: &Path| stamp(p).map(|(_, m)| m).unwrap_or(0);
                    let path = file.path();
                    match out.get(&(agent.name.clone(), shown.clone())) {
                        Some(kept) if newer(kept) >= newer(&path) => {}
                        _ => {
                            out.insert((agent.name.clone(), shown), path);
                        }
                    }
                }
            }
        }
        for entry in std::fs::read_dir(crate::pages::root(config).join(&agent.name)).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".html") {
                out.insert((agent.name.clone(), name), entry.path());
            }
        }
    }
    out
}

/// A line's anchor day, for a date without a year (review 2 of S1, R10): its attribution's day, else its page's
/// `updated`, else the file's day; never today.
fn anchor(page: &Page, file: &Path) -> Option<NaiveDate> {
    day_of(&tk::str_or_empty(page.front.get("updated"))).or_else(|| {
        let (_, modified) = stamp(file)?;
        chrono::DateTime::from_timestamp((modified / 1_000_000_000) as i64, 0).map(|d| d.date_naive())
    })
}

/// Each page's items as last made: (its root, its path) → (what the page said and what filed it, its items).
type PageItems = HashMap<(PathBuf, String), (u64, Vec<Entry>)>;
static PAGE_ITEMS: LazyLock<Mutex<PageItems>> = LazyLock::new(Default::default);

/// What files the pages besides their own text: the person's rules and marks, the papers' tasks (state.json).
fn state_key(config: &Config) -> String {
    let state = super::read_state(config);
    ["category_rules", "category_marks", "papers", "language"]
        .iter()
        .map(|k| state.get(*k).map(|v| v.to_string()).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// A page's items: itself, then its lines (never a reference, a number or an archived mail's link).
fn page_entries(
    ctx: &c::Ctx,
    part: Part,
    w: &super::Words,
    marks: &serde_json::Map<String, Value>,
    anchor: Option<NaiveDate>,
    path: &String,
    page: &Page,
) -> Vec<Entry> {
    let health = part == Part::Health;
    let mut items: Vec<Entry> = Vec::new();
    let kind = page.kind();
    if ["tasks", "records", "documents"].contains(&kind.as_str()) || *path == w.inbox_path() || named_reference(path) {
        return items;
    }
    let filed = c::of_page_in(ctx, part, path, page);
    if !health && filed.categories.iter().any(|x| x == "health") {
        return items;
    }
    let key = c::page_key(page, path);
    let page_id = format!("page:{key}");
    let archived = w.archived(path);
    let title = if kind == "self" { w.me.to_string() } else { page.title.clone() };
    let Some(title) = masked(&title) else { return items };
    let aliases: Vec<String> = page.aliases().iter().filter_map(|a| masked(a)).collect();
    let summary = masked(&super::summary_of(&page.body, w.brief)).unwrap_or_default();
    let space = tk::str_or_empty(page.front.get("space"));
    let space = (space == "pro").then(|| "pro".to_string());
    items.push(Entry {
        id: page_id.clone(),
        kind: "page".into(),
        part: health.then(|| "health".to_string()),
        path: Some(path.clone()),
        title: title.clone(),
        line: tk::head(&summary, 240).to_string(),
        categories: without_dates(filed.categories.clone()),
        space: space.clone(),
        source: some(title.clone()),
        archived,
        open: json!({"path": path}),
        fold: fold(&format!("{title} {} {summary}", aliases.join(" "))),
        aliases,
        ..Default::default()
    });
    let sections: Vec<(String, Vec<super::Fact>)> =
        page.sections().into_iter().filter(|(name, _)| fold(name) != fold(w.brief)).collect();
    let texts: Vec<String> = sections.iter().flat_map(|(_, f)| f.iter().map(|x| x.text.clone())).collect();
    let ids = c::line_ids(&key, &texts);
    let mut n = 0;
    for (section, facts) in &sections {
        let sid = super::template::resolve(&kind, section);
        for f in facts {
            let id = ids[n].clone();
            n += 1;
            // never a reference or a number, never an archived mail's link (as the offline copy: offline.rs)
            if reference(&f.text) || f.text.contains("[[archive:") {
                continue;
            }
            let noted = day_of(&f.date).or(anchor);
            let when = match noted {
                Some(d) => c::line_when(&f.text, &d.to_string(), d),
                None => None,
            };
            let (date, yearly) = match when {
                Some(c::When::On(d)) => (Some(d.to_string()), None),
                Some(c::When::Yearly(m, d)) => (None, Some(format!("{m:02}-{d:02}"))),
                None => (None, None),
            };
            let (label, answer) = match super::lexicon::split_label(&f.text) {
                Some((l, a)) => (Some(l), Some(a)),
                None => (None, None),
            };
            let cats = without_dates(c::of_line(&filed.categories, &key, &f.text, &f.date, marks, NaiveDate::MIN));
            items.push(Entry {
                id,
                kind: "line".into(),
                part: health.then(|| "health".to_string()),
                path: Some(path.clone()),
                title: title.clone(),
                line: tk::head(&f.text, 240).to_string(),
                categories: cats,
                space: space.clone(),
                date,
                yearly,
                // its section's title as written can hold one too (review 2 of S3, B4)
                source: masked(section).and_then(some),
                who: some(f.who.clone()),
                noted: some(f.date.clone()),
                archived,
                label,
                answer,
                page: Some(page_id.clone()),
                open: json!({"path": path, "section": sid}),
                fold: fold(&f.text),
                ..Default::default()
            });
        }
    }
    items
}

/// Every item of the copy, in the copy's order (pages and their lines, open tasks, records, documents), untrimmed:
/// the shared part only, never Santé, a reference or an archived mail. `documents`: `documents(config, Main)`'s list
/// (the bench gives the demo's).
pub fn entries(config: &Config, documents: &[Value]) -> Vec<Entry> {
    entries_of(config, Part::Main, documents)
}

/// [`entries`] of a part: the shared one (the copy), or Santé's (searched by the bridge behind the app's lock, read
/// at each search and kept nowhere).
fn entries_of(config: &Config, part: Part, documents: &[Value]) -> Vec<Entry> {
    let lang = super::language(config);
    let w = super::words(&lang);
    let ctx = c::Ctx::new(config);
    let marks = c::marks(config);
    let health = part == Part::Health;
    let main = super::all_pages(config, part);
    let root = super::part_root(config, part);
    let mut items: Vec<Entry> = Vec::new();
    // each page's items kept while what it says and what files it (language, rules, the person's marks) stay the
    // same: a write makes the copy again for the pages it touched only (Santé's are read at each search, kept nowhere)
    let filing = hash_of(&(lang.clone(), state_key(config)));
    let mut cache = PAGE_ITEMS.lock().unwrap_or_else(|e| e.into_inner());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    // a page whose path holds a reference (its name, or a folder's: « Dossiers/HR-55821/… ») is left out with its
    // lines: the phone opens a page by its path (`open.path`, `GET /v1/notebook/page?path=`), and a path masked opens
    // nothing; opening by an item's id (A2/D2) would let them in. Counted, never named, in the log.
    let mut named_out = 0usize;
    for (path, page) in &main {
        if !health && named_reference(path) {
            named_out += 1;
            continue;
        }
        let file = root.join(path);
        let anchor = anchor(page, &file);
        if health {
            items.extend(page_entries(&ctx, part, w, &marks, anchor, path, page));
            continue;
        }
        // what the page says as it was read (never its file's size and time, read apart: an agent's write or « Oublie
        // ça » between the two kept the old lines; review 1 of S3, B3), the day its undated lines are read near, and
        // what files it
        let said = hash_of(&(filing, anchor, page.title.as_str(), page.body.as_str(), serde_json::to_string(&page.front).unwrap_or_default()));
        let key = (root.clone(), path.clone());
        seen.insert(path.as_str());
        if let Some((s, kept)) = cache.get(&key) {
            if *s == said {
                items.extend(kept.iter().cloned());
                continue;
            }
        }
        let made = page_entries(&ctx, part, w, &marks, anchor, path, page);
        items.extend(made.iter().cloned());
        cache.insert(key, (said, made));
    }
    if !health {
        cache.retain(|(r, p), _| *r != root || seen.contains(p.as_str()));
    }
    drop(cache);
    if named_out > 0 {
        tracing::info!(pages = named_out, "notebook index: pages left out of the phone's copy, a reference in their path");
    }
    for t in super::open_tasks(config, part) {
        let cats = without_dates(c::of_task(&ctx, &t, &main, &marks, NaiveDate::MIN));
        if !health && cats.iter().any(|x| x == "health") {
            continue;
        }
        let plain: String = t.title.split(" [[").next().unwrap_or(&t.title).trim().to_string();
        let Some(plain) = masked(&plain) else { continue };
        items.push(Entry {
            id: format!("task:{}", t.id),
            kind: "task".into(),
            part: health.then(|| "health".to_string()),
            path: Some(w.tasks_path()),
            title: plain.clone(),
            line: plain.clone(),
            categories: cats,
            date: some(t.due.clone()),
            source: some(w.tasks.to_string()),
            who: some(t.who.clone()),
            noted: some(t.date.clone()),
            archived: t.archived,
            open: json!({"task": t.id}),
            fold: fold(&plain),
            ..Default::default()
        });
    }
    for t in records::TYPES.iter().filter(|_| !health) {
        let active = records::records(config, t).into_iter().map(|r| (r, false));
        let archived = records::archived_records(config, t).into_iter().map(|r| (r, true));
        for (r, archived) in active.chain(archived) {
            let (cats, _, _) = c::of_record(t, &r, &marks, NaiveDate::MIN);
            let id = tk::str_or_empty(r.get("id"));
            let Some(name) = masked(&records::summary_head(t, &r)) else { continue };
            let line = c::record_line(t, &r);
            let (date, _) = c::When::fields(c::record_when(t, &r));
            // what its next date is (« Fin de garantie », « Prochain renouvellement »): « garantie lave-linge »
            let next = t.fields.iter().find(|f| f.id == t.next && !tk::str_or_empty(r.get(f.id)).trim().is_empty());
            let next = next.map(|f| f.titles[super::lang_index(&lang)]).unwrap_or_default();
            items.push(Entry {
                id: format!("record:{id}"),
                kind: "record".into(),
                path: Some(records::path_of(&lang, t)),
                title: name,
                line: tk::head(&line, 240).to_string(),
                categories: without_dates(cats),
                date: date.as_str().map(str::to_string),
                source: some(t.titles[super::lang_index(&lang)].to_string()),
                archived,
                open: json!({"record": {"type": t.id, "id": id}}),
                fold: fold(&format!("{line} {next}")),
                ..Default::default()
            });
        }
    }
    let files = if documents.is_empty() { HashMap::new() } else { document_files(config) };
    // the texts kept: those of the documents listed now (never all dropped at once: they would be read again forever)
    if part == Part::Main {
        let now: std::collections::HashSet<&Path> = files.values().map(PathBuf::as_path).collect();
        DOCUMENT_TEXTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| now.contains(k.0.as_path()));
    }
    let mut listed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in documents {
        if tk::str_or_empty(d.get("part")) == "health" {
            continue;
        }
        let (cats, _, _) = c::of_document(&ctx, d, &main, &marks);
        if cats.iter().any(|x| x == "health") {
            continue;
        }
        let (agent, file) = (tk::str_or_empty(d.get("agent")), tk::str_or_empty(d.get("file")));
        let web_page = d.get("page").is_some_and(|p| !p.is_null()) || tk::str_or_empty(d.get("type")) == "page";
        // a file whose name holds a reference: out (its `open` names it); an agent's web page is named after its
        // title (`pages::slug`): out when its title holds one
        let named = tk::str_or_empty(d.get("name"));
        if (!web_page && named_reference(&file)) || (web_page && reference(&named)) {
            continue;
        }
        // two files of the same name an agent received: one note, the newest (`documents` lists the newest first;
        // review 1 of S3, R8)
        let id = c::document_key(&agent, &file);
        if !listed.insert(id.clone()) {
            continue;
        }
        let Some(name) = masked(&named) else { continue };
        let text = files.get(&(agent.clone(), file.clone())).map(|p| document_text(p, &file)).unwrap_or_default();
        let shown_by = tk::str_or_empty(d.get("agent_name"));
        let by = if shown_by.is_empty() { agent.clone() } else { shown_by };
        items.push(Entry {
            id,
            kind: "document".into(),
            title: name.clone(),
            line: name.clone(),
            categories: without_dates(cats),
            source: some(by.clone()),
            who: some(by),
            noted: day_of(&tk::str_or_empty(d.get("date"))).map(|d| d.to_string()),
            open: json!({"document": {"agent": agent, "file": file}}),
            fold: fold(&format!("{name} {} {text}", kind_words(&file, web_page))),
            ..Default::default()
        });
    }
    items
}

/// Santé's search (`GET /v1/notebook/search?private=1`, behind the app's lock): its pages, lines and tasks read now,
/// searched by the same rule, kept nowhere.
pub fn health_finder(config: &Config, langs: &[String]) -> Finder {
    let year = chrono::Local::now().date_naive().year();
    Finder::new(entries_of(config, Part::Health, &[]), &tables(langs, year))
}

/// An item as sent: compact JSON (its default keys left out).
pub fn sent(e: &Entry) -> Value {
    let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        if o.get("part").and_then(Value::as_str) == Some("main") {
            o.remove("part");
        }
        if o.get("space").and_then(Value::as_str) == Some("perso") {
            o.remove("space");
        }
    }
    v
}

/// The copy kept within `lines` items and `bytes` of compact JSON, by the bridge alone: archived lines out first (the
/// oldest noted first), then archived pages, then the other lines (the oldest noted first). Whether it was cut.
pub fn trim(items: Vec<Entry>, lines: usize, bytes: usize) -> (Vec<Entry>, bool) {
    let sizes: Vec<usize> = items.iter().map(|e| serde_json::to_vec(&sent(e)).map(|b| b.len() + 1).unwrap_or(0)).collect();
    trim_sized(items, sizes, lines, bytes)
}

/// [`trim`] with each item's size as sent already known.
fn trim_sized(items: Vec<Entry>, sizes: Vec<usize>, lines: usize, bytes: usize) -> (Vec<Entry>, bool) {
    // the answer's own keys and tables: a margin
    let mut total: usize = sizes.iter().sum::<usize>() + 64 * 1024;
    let mut count = items.len();
    if count <= lines && total <= bytes {
        return (items, false);
    }
    let mut order: Vec<usize> = (0..items.len()).filter(|i| items[*i].kind == "line" || (items[*i].kind == "page" && items[*i].archived)).collect();
    let rank = |e: &Entry| -> (u8, String) {
        let step = match (e.kind.as_str(), e.archived) {
            ("line", true) => 0,
            ("page", true) => 1,
            _ => 2,
        };
        (step, e.noted.clone().unwrap_or_default())
    };
    order.sort_by(|a, b| rank(&items[*a]).cmp(&rank(&items[*b])).then(a.cmp(b)));
    let mut out = vec![true; items.len()];
    for i in order {
        if count <= lines && total <= bytes {
            break;
        }
        out[i] = false;
        count -= 1;
        total -= sizes[i];
    }
    (items.into_iter().zip(out).filter(|(_, keep)| *keep).map(|(e, _)| e).collect(), true)
}

// -- the tables -----------------------------------------------------------------------------------------------------------

/// The languages whose tables go with the copy: the Carnet's and the app's.
pub fn langs(carnet: &str, asked: Option<&str>) -> Vec<String> {
    let mut out = vec![carnet.to_string()];
    if let Some(a) = asked.map(|a| a.trim().to_lowercase()).filter(|a| a.chars().count() >= 2) {
        let a: String = a.chars().take(2).collect();
        if super::search_words::LANGS.contains(&a.as_str()) && !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// The copy's tables for `langs` (sommaire.json `Index`: `stop`, `neighbours`, `dates`, `dated`).
pub fn tables(langs: &[String], year: i32) -> Tables {
    let words = &super::search_words::WORDS;
    let mut t = Tables { dated: (c::DATED_BEFORE, c::DATED_AFTER), ..Default::default() };
    for lang in langs {
        for w in words.stop(lang) {
            if !t.stop.contains(w) {
                t.stop.push(w.clone());
            }
        }
        for (k, v) in words.neighbours(lang) {
            let list = t.neighbours.entry(k.clone()).or_default();
            for x in v {
                if !list.contains(x) {
                    list.push(x.clone());
                }
            }
        }
        for p in words.phrases(lang) {
            if !t.dates.phrases.iter().any(|q| q.words == p.words) {
                t.dates.phrases.push(p.clone());
            }
        }
    }
    t.dates.lang = langs.first().cloned().unwrap_or_default();
    t.dates.feasts = words.feasts.clone();
    t.dates.easter = (year..year + 10).map(|y| (y.to_string(), super::search_words::easter_of(y).to_string())).collect();
    t
}

pub fn tables_json(t: &Tables) -> (Value, Value, Value, Value) {
    (
        json!(t.stop),
        json!(t.neighbours),
        serde_json::to_value(&t.dates).unwrap_or(Value::Null),
        json!({"before": t.dated.0, "after": t.dated.1}),
    )
}

// -- versions -------------------------------------------------------------------------------------------------------------

/// One Carnet's copy as last made: its epoch, its version, each item's hash, the recent versions' changes, and the
/// copy itself (ready to search: the bridge's own search reads it).
struct Kept {
    epoch: String,
    version: u64,
    hashes: HashMap<String, u64>,
    history: VecDeque<(u64, Vec<String>, Vec<String>)>,
    /// What the Carnet's files were when it was made: the next search makes it again only when they changed.
    files: u64,
    items: Arc<Vec<Entry>>,
    truncated: bool,
    finders: HashMap<Vec<String>, Arc<Finder>>,
    /// The kept searches as sent (`KeptShort`, summary.rs): a change is a new version (no item changed).
    kept: Value,
}

static KEPT: LazyLock<Mutex<HashMap<PathBuf, Kept>>> = LazyLock::new(Default::default);
/// One making of a copy at a time: the sommaire and the index opened together make it once (review 1 of S4, R7).
static MAKING: Mutex<()> = Mutex::new(());

fn hash_of(v: &impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// What the Carnet's files are now (paths, sizes, times): the pages of the shared part, its state, the agents' files
/// and pages. Cheap (no file read): a search makes the copy again only when this moved.
fn files_now(config: &Config) -> u64 {
    let mut seen: Vec<(String, u64, u64)> = Vec::new();
    let walk = |dir: &Path, depth: usize, seen: &mut Vec<(String, u64, u64)>| {
        let mut stack = vec![(dir.to_path_buf(), 0usize)];
        while let Some((d, level)) = stack.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    if level < depth {
                        stack.push((path, level + 1));
                    }
                    continue;
                }
                let modified = meta.modified().ok().and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok());
                seen.push((path.to_string_lossy().into_owned(), meta.len(), modified.map(|m| m.as_nanos() as u64).unwrap_or(0)));
            }
        }
    };
    walk(&super::part_root(config, Part::Main), 3, &mut seen);
    let state = super::root(config).join("state.json");
    if let Some((len, modified)) = stamp(&state) {
        seen.push((state.to_string_lossy().into_owned(), len, modified));
    }
    for agent in config.agents.values() {
        if let Some(root) = agent.upload_dir_host.as_deref().filter(|r| !r.is_empty()) {
            walk(Path::new(root), 1, &mut seen);
        }
        walk(&crate::pages::root(config).join(&agent.name), 0, &mut seen);
        walk(&crate::agent_tools::audio_root(config).join(&agent.name), 0, &mut seen);
    }
    seen.sort();
    hash_of(&(seen, TEXTS_READ.load(std::sync::atomic::Ordering::SeqCst)))
}

/// The copy now (made again when the Carnet's files moved), kept with its version.
fn refreshed(config: &Config) -> (PathBuf, u64) {
    let key = super::root(config);
    let files = files_now(config);
    let unchanged = |files: u64| {
        let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        kept.get(&key).is_some_and(|k| k.files == files)
    };
    if unchanged(files) {
        return (key.clone(), files);
    }
    // another request may be making it: wait for it, then look again
    let _making = MAKING.lock().unwrap_or_else(|e| e.into_inner());
    if unchanged(files) {
        return (key.clone(), files);
    }
    let documents = super::documents(config, Part::Main);
    let made = entries(config, &documents);
    // each item as sent, once: its size (the limits) and its hash (what changed)
    let sent_bytes: Vec<Vec<u8>> = made.iter().map(|e| serde_json::to_vec(&sent(e)).unwrap_or_default()).collect();
    let all: HashMap<&str, u64> = made.iter().zip(&sent_bytes).map(|(e, b)| (e.id.as_str(), hash_of(b))).collect();
    let all: HashMap<String, u64> = all.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let sizes = sent_bytes.iter().map(|b| b.len() + 1).collect();
    drop(sent_bytes);
    let (items, truncated) = trim_sized(made, sizes, LINES_MAX, BYTES_MAX);
    let hashes: HashMap<String, u64> = items.iter().filter_map(|e| all.get(&e.id).map(|h| (e.id.clone(), *h))).collect();
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let k = kept.entry(key.clone()).or_insert_with(|| Kept {
        epoch: format!("e{}", crate::py::token_hex(4)),
        version: 0,
        hashes: HashMap::new(),
        history: VecDeque::new(),
        files: 0,
        items: Arc::new(Vec::new()),
        truncated: false,
        finders: HashMap::new(),
        kept: json!([]),
    });
    let changed: Vec<String> =
        items.iter().filter(|e| k.hashes.get(&e.id) != hashes.get(&e.id)).map(|e| e.id.clone()).collect();
    let removed: Vec<String> = k.hashes.keys().filter(|id| !hashes.contains_key(*id)).cloned().collect();
    // (the kept searches are in state.json: a change of theirs makes the copy again, and is a version of its own)
    let kept_now = super::summary::kept_short(config);
    if k.version == 0 || !changed.is_empty() || !removed.is_empty() || k.truncated != truncated || k.kept != kept_now {
        k.version += 1;
        k.history.push_back((k.version, changed, removed));
        while k.history.len() > HISTORY {
            k.history.pop_front();
        }
        k.hashes = hashes;
        k.items = Arc::new(items);
        k.truncated = truncated;
        k.finders.clear();
        k.kept = kept_now;
    }
    k.files = files;
    (key, files)
}

/// The bridge's own search over the copy (the same rule as the phones): the copy made again only when the Carnet
/// changed, its finder kept for these tables' languages.
pub fn finder(config: &Config, langs: &[String]) -> Arc<Finder> {
    current(config, langs).0
}

/// [`finder`], and the copy's version and epoch (the sommaire's `version` and `epoch`, summary.rs: a version alone
/// means nothing after the bridge started again; review 1 of S4, R4).
pub fn current(config: &Config, langs: &[String]) -> (Arc<Finder>, u64, String) {
    let (key, _) = refreshed(config);
    let (items, version, epoch) = {
        let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        let k = &kept[&key];
        if let Some(f) = k.finders.get(langs) {
            return (f.clone(), k.version, k.epoch.clone());
        }
        (k.items.clone(), k.version, k.epoch.clone())
    };
    let year = chrono::Local::now().date_naive().year();
    let finder = Arc::new(Finder::shared(items.clone(), &tables(langs, year)));
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = kept.get_mut(&key) {
        if Arc::ptr_eq(&k.items, &items) {
            k.finders.insert(langs.to_vec(), finder.clone());
        }
    }
    (finder, version, epoch)
}


/// The copy made from scratch and made ready to search (the bench: what the first search after a change costs).
#[cfg(test)]
pub fn rebuilt_for_bench(config: &Config, langs: &[String]) -> Finder {
    let documents = super::documents(config, Part::Main);
    let (items, _) = trim(entries(config, &documents), LINES_MAX, BYTES_MAX);
    Finder::new(items, &tables(langs, chrono::Local::now().date_naive().year()))
}

/// The copy's ETag: its epoch, its version and the languages of its tables (`"<epoch>-v<version>-fr.en"`: the app's
/// language changed, its tables come; review 1 of S3, R2).
fn etag_of(k: &Kept, langs: &[String]) -> String {
    format!("\"{}-v{}-{}\"", k.epoch, k.version, langs.join("."))
}

/// The copy's ETag now (the copy made again first when the Carnet changed): a 304 is answered without making the
/// answer (review 1 of S3, R4).
pub fn etag(config: &Config, langs: &[String]) -> String {
    let (key, _) = refreshed(config);
    let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    etag_of(&kept[&key], langs)
}

/// [`answer`] of one space (`?space=`, S5).
pub fn answer_in(
    config: &Config,
    langs: &[String],
    since: Option<u64>,
    epoch: Option<&str>,
    online_only: &str,
    _space: &str,
) -> (Value, String) {
    answer(config, langs, since, epoch, online_only)
}

/// [`etag`] of one space's copy (S5).
pub fn etag_in(config: &Config, langs: &[String], _space: &str) -> String {
    etag(config, langs)
}

/// `GET /v1/notebook/index`: (the answer, its ETag). `since`/`epoch`: what the phone has.
pub fn answer(config: &Config, langs: &[String], since: Option<u64>, epoch: Option<&str>, online_only: &str) -> (Value, String) {
    let (key, _) = refreshed(config);
    let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let k = &kept[&key];
    let etag = etag_of(k, langs);
    // a diff: the phone's version is one of this epoch's kept ones
    let first_kept = k.history.front().map(|h| h.0).unwrap_or(k.version);
    let diff = match (since, epoch) {
        (Some(s), Some(e)) if e == k.epoch && s <= k.version && s + 1 >= first_kept => Some(s),
        _ => None,
    };
    let (items, removed, full) = match diff {
        Some(s) => {
            // (sets: a diff of many items stays linear; review 1 of S3, R10)
            let mut changed: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let mut removed: Vec<&str> = Vec::new();
            for (_, ch, rm) in k.history.iter().filter(|h| h.0 > s) {
                changed.extend(ch.iter().map(String::as_str));
                removed.extend(rm.iter().map(String::as_str));
            }
            let now: std::collections::HashSet<&str> = k.items.iter().map(|e| e.id.as_str()).collect();
            let items: Vec<Value> = k.items.iter().filter(|e| changed.contains(e.id.as_str())).map(sent).collect();
            let mut gone: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let removed: Vec<String> =
                removed.into_iter().filter(|id| !now.contains(id) && gone.insert(id)).map(str::to_string).collect();
            (items, removed, false)
        }
        None => (k.items.iter().map(sent).collect(), Vec::new(), true),
    };
    let year = chrono::Local::now().date_naive().year();
    let (stop, neighbours, dates, dated) = tables_json(&tables(langs, year));
    let body = json!({
        "v": V, "epoch": k.epoch, "version": k.version, "since": if full { Value::Null } else { json!(since) },
        "full": full, "lang": langs.first().cloned().unwrap_or_default(), "made": tk::now_iso_seconds(),
        "limits": {"lines": LINES_MAX, "bytes": BYTES_MAX}, "truncated": k.truncated,
        "online_only": if k.truncated { json!(online_only) } else { Value::Null },
        "dated": dated, "items": items, "removed": removed, "stop": stop, "neighbours": neighbours, "dates": dates,
        "kept": k.kept,
    });
    (body, etag)
}

/// The bar's text when the copy was cut (visible text: for Alex).
pub fn online_only(lang: &str) -> &'static str {
    match lang {
        "fr" => "Recherche complète en ligne",
        "de" => "Vollständige Suche online",
        "es" => "Búsqueda completa en línea",
        "nl" => "Volledig zoeken online",
        _ => "Full search online",
    }
}

/// A search result as the apps show it (sommaire.json `Item`): every key, `dates` among the categories as of `today`.
pub fn item_view(f: &Finder, at: usize, today: NaiveDate) -> Value {
    let e = &f.items[at];
    json!({"id": e.id, "kind": e.kind, "part": e.part.clone().unwrap_or_else(|| "main".into()), "path": e.path,
           "title": e.title, "line": e.line, "categories": f.categories(at, today), "space": e.space(),
           "date": e.date, "yearly": e.yearly, "source": e.source.clone().unwrap_or_default(),
           "who": e.who.clone().unwrap_or_default(), "noted": e.noted, "archived": e.archived, "label": e.label,
           "answer": e.answer, "page": e.page, "open": e.open})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::{tests_config, write_file, Who};

    fn config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let config = tests_config(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    fn page(config: &Config, part: Part, path: &str, text: &str) {
        write_file(&super::super::part_root(config, part).join(path), text).unwrap();
    }

    fn texts(items: &[Entry]) -> String {
        items.iter().map(|e| format!("{} {} {} {}", e.title, e.line, e.fold, e.aliases.join(" "))).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_copy_holds_no_santé_no_reference_no_archived_mail() {
        let (_d, config) = config();
        page(&config, Part::Health, "Domaines/Santé.md", "---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n- Traitement du soir — *Daily, 2026-11-01*\n");
        page(&config, Part::Main, "Personnes/Marc Durand.md", "---\nid: p0000000001\ntype: person\naliases: [Marc]\n---\n# Marc Durand\n\n## En bref\nTon beau-frère, dossier HR-55821.\n\n## Goûts et préférences\n- Allergique aux noix — *Daily, 2026-11-01*\n- Numéro client CSG-2291 — *Daily, 2026-11-01*\n- Le mail [[archive:m1]] — *Daily, 2026-11-01*\n- 250g de farine pour son gâteau — *Daily, 2026-11-01*\n");
        let you = Who::you(&config);
        super::super::add_task(&config, Part::Main, "Payer la facture FR-2026-00123", "", "", "asked", "", &you).unwrap();
        let docs = vec![json!({"agent": "daily", "agent_name": "Daily", "file": "Facture FR-2026-00123.pdf",
                               "name": "Facture FR-2026-00123.pdf", "type": "pdf", "date": "2026-11-02T10:00:00+00:00",
                               "subjects": []}),
                        json!({"agent": "health", "file": "Prise de sang.pdf", "name": "Prise de sang.pdf", "part": "health"})];
        let items = entries(&config, &docs);
        let all = texts(&items);
        for never in ["Traitement", "HR-55821", "CSG", "archive", "FR-2026", "00123", "Prise de sang", "traitement"] {
            assert!(!all.contains(never), "{never} in the copy:\n{all}");
        }
        // the shared Carnet's health-looking line stays (Alex's decision 6), a quantity is not a reference (R11)
        assert!(all.contains("Allergique aux noix"), "{all}");
        assert!(all.contains("250g de farine"), "{all}");
        // the « En bref », the task and the document keep their other words (R9)
        let marc = items.iter().find(|e| e.id == "page:p0000000001").unwrap();
        assert_eq!(marc.line, "Ton beau-frère, dossier …");
        assert!(items.iter().any(|e| e.kind == "task" && e.title == "Payer la facture …"), "{all}");
        // a document whose file's name holds a reference: out, as a line (review 1 of S3, B2: its `open` names it)
        assert!(!items.iter().any(|e| e.kind == "document"), "{all}");
        assert!(items.iter().all(|e| !e.categories.iter().any(|c| c == "health" || c == "dates")));
    }

    #[test]
    fn the_copy_never_depends_on_the_day() {
        let (_d, config) = config();
        // a line typed in Obsidian (no attribution), a day without a year: read near the page's `updated` (R10)
        // (review 1 of S3, R5: an `updated` far from today, so that today's day would give another year)
        page(&config, Part::Main, "Sujets/Devis.md", "---\nid: p0000000002\ntype: subject\nupdated: 2025-03-01\n---\n# Devis\n\n## Où on en est\n- Devis reçu le 3 octobre\n- Rendez-vous le 3/11\n- 1/2 tasse de sucre\n- Anniversaire de mariage : 14 juin\n");
        let items = entries(&config, &[]);
        let by_line = |t: &str| items.iter().find(|e| e.line == t).unwrap().clone();
        assert_eq!(by_line("Devis reçu le 3 octobre").date.as_deref(), Some("2024-10-03"));
        assert_eq!(by_line("Rendez-vous le 3/11").date.as_deref(), Some("2024-11-03"));
        assert_eq!(by_line("1/2 tasse de sucre").date, None);
        assert_eq!(by_line("Anniversaire de mariage : 14 juin").yearly.as_deref(), Some("06-14"));
        assert_eq!(entries(&config, &[]), items, "the same Carnet, the same copy");
    }

    #[test]
    fn the_bridge_alone_trims_archived_lines_first() {
        let line = |id: &str, archived: bool, noted: &str| Entry {
            id: id.into(), kind: "line".into(), archived, noted: Some(noted.into()), fold: "x".into(), ..Default::default()
        };
        let items = vec![
            Entry { id: "page:a".into(), kind: "page".into(), archived: true, ..Default::default() },
            line("new", false, "2026-11-01"),
            line("old", false, "2020-01-01"),
            line("arch-new", true, "2026-01-01"),
            line("arch-old", true, "2019-01-01"),
            Entry { id: "task:1".into(), kind: "task".into(), ..Default::default() },
        ];
        let ids = |(v, cut): (Vec<Entry>, bool)| (v.into_iter().map(|e| e.id).collect::<Vec<_>>(), cut);
        assert!(!ids(trim(items.clone(), 6, BYTES_MAX)).1);
        assert_eq!(ids(trim(items.clone(), 5, BYTES_MAX)), (["page:a", "new", "old", "arch-new", "task:1"].iter().map(|s| s.to_string()).collect(), true));
        assert_eq!(ids(trim(items.clone(), 3, BYTES_MAX)).0, ["new", "old", "task:1"]);
        assert_eq!(ids(trim(items.clone(), 2, BYTES_MAX)).0, ["new", "task:1"]);
        // the bytes too
        let (kept, cut) = trim(items, 100, 64 * 1024 + 200);
        assert!(cut && kept.len() < 6 && kept.iter().any(|e| e.id == "task:1"));
    }

    #[test]
    fn versions_diffs_and_epochs() {
        let (_d, config) = config();
        page(&config, Part::Main, "Sujets/Jardin.md", "---\nid: p0000000003\ntype: subject\n---\n# Jardin\n\n## Où on en est\n- Tailler la haie — *Daily, 2026-11-01*\n");
        page(&config, Part::Main, "Sujets/Cuisine.md", "---\nid: p0000000004\ntype: subject\n---\n# Cuisine\n\n## Où on en est\n- Devis reçu — *Daily, 2026-11-01*\n");
        let langs = vec!["fr".to_string()];
        let (first, etag) = answer(&config, &langs, None, None, "");
        assert_eq!((first["full"].as_bool(), first["version"].as_u64()), (Some(true), Some(1)));
        let epoch = first["epoch"].as_str().unwrap().to_string();
        assert_eq!(etag, format!("\"{epoch}-v1-fr\""));
        // nothing changed: the same version, nothing to send
        let (same, etag2) = answer(&config, &langs, Some(1), Some(&epoch), "");
        assert_eq!((etag2, same["full"].as_bool(), same["items"].as_array().unwrap().len()), (etag, Some(false), 0));
        // a line added, a page gone: only those
        std::thread::sleep(std::time::Duration::from_millis(20));
        page(&config, Part::Main, "Sujets/Jardin.md", "---\nid: p0000000003\ntype: subject\n---\n# Jardin\n\n## Où on en est\n- Tailler la haie — *Daily, 2026-11-01*\n- Planter des tulipes — *Daily, 2026-11-02*\n");
        std::fs::remove_file(super::super::part_root(&config, Part::Main).join("Sujets/Cuisine.md")).unwrap();
        let (diff, _) = answer(&config, &langs, Some(1), Some(&epoch), "");
        assert_eq!(diff["version"], 2);
        assert_eq!(diff["full"], false);
        assert_eq!(diff["since"], 1);
        let changed: Vec<&str> = diff["items"].as_array().unwrap().iter().map(|i| i["line"].as_str().unwrap_or("")).collect();
        assert_eq!(changed, ["Planter des tulipes"]);
        let removed: Vec<&str> = diff["removed"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()).collect();
        assert!(removed.contains(&"page:p0000000004") && removed.iter().all(|r| r.contains("p0000000004")), "{removed:?}");
        // the tables are whole in a diff too
        assert!(diff["neighbours"].as_object().is_some_and(|n| n.len() > 100) && diff["dates"]["phrases"].is_array());
        // another epoch, a version ahead, nothing given: everything
        for (since, e) in [(Some(1), Some("e0000")), (Some(9), Some(epoch.as_str())), (None, None)] {
            let (all, _) = answer(&config, &langs, since, e, "");
            assert_eq!(all["full"], true, "{since:?} {e:?}");
            assert_eq!(all["items"].as_array().unwrap().len(), 3);
        }
    }

    /// A page made by hand (no id) keeps its key from its path when the person files it or a line of it: its header
    /// gets the categories' lines only (S2 review 1, R10), never an id; an undo on it once it is gone is gone (review 2
    /// of S1, R12).
    #[test]
    fn a_page_made_by_hand_keeps_its_key_when_it_is_filed() {
        let (_d, config) = config();
        let hand = "---\ntype: subject\n---\n# Atelier\n\n## Où on en est\n- Vernis à commander\n";
        page(&config, Part::Main, "Sujets/Atelier.md", hand);
        let file = super::super::part_root(&config, Part::Main).join("Sujets/Atelier.md");
        let you = Who::you(&config);
        let line = c::Target::Line { path: "Sujets/Atelier.md".into(), text: "Vernis à commander".into() };
        let done = c::categorize(&config, &line, &["money".to_string()], &you).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), hand, "a line's correction is kept in the state only");
        let item = done["target"]["item"].as_str().unwrap().to_string();
        assert!(item.starts_with("line:f"), "{done}");
        let copy = entries(&config, &[]);
        let filed = copy.iter().find(|e| e.line == "Vernis à commander").unwrap();
        assert_eq!((filed.id.clone(), filed.categories.clone()), (item, vec!["money".to_string()]));
        let pg = c::Target::Page { path: "Sujets/Atelier.md".into() };
        let done = c::categorize(&config, &pg, &["home".to_string()], &you).unwrap().unwrap();
        assert!(done["target"]["item"].as_str().unwrap().starts_with("page:f"), "{done}");
        assert!(Page::parse(&std::fs::read_to_string(&file).unwrap(), "Atelier").id().is_empty());
        std::fs::remove_file(&file).unwrap();
        assert!(c::undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().is_none());
    }

    /// Review 1 of S3, B1: a number written in groups (an IBAN, a card, a social security number) is an ID number on
    /// its raw line: never in a document's words, whatever the words around it.
    #[test]
    fn numbers_in_groups_never_reach_the_copy_from_a_document() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            ("rib.txt", "Relevé d'identité bancaire\nTitulaire : Marie Martin\nIBAN : FR76 3000 6000 0112 3456 7890 189\nBIC : AGRIFRPP\n"),
            ("paie.txt", "Bulletin de paie d'octobre\nNuméro de sécurité sociale : 1 85 05 78 006 084 36\nSalaire net : 2 100 €\n"),
            ("pg0123456789.html", "<html><body><h1>Budget du voyage</h1><p>Payé avec la carte 4970 1012 3456 7890</p><p>Hôtel à Kyoto</p></body></html>"),
        ];
        let mut read = String::new();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            read.push_str(&read_document(&path, name));
            read.push(' ');
        }
        for never in ["fr76", "3000", "6000", "0112", "3456", "7890", "4970", "1012", "006 084", " 084 "] {
            assert!(!format!(" {read} ").contains(never), "{never} in the copy: {read}");
        }
        for kept in ["titulaire", "salaire", "budget du voyage", "kyoto"] {
            assert!(read.contains(kept), "{kept} not in: {read}");
        }
    }

    /// Review 1 of S3, B2: a reference never reaches the phone by a path or a file's name either (`path`, `open`).
    #[test]
    fn a_reference_never_reaches_the_copy_by_a_path_or_a_file() {
        let (_d, config) = config();
        page(&config, Part::Main, "Sujets/Passeport 18AB12345.md", "---\ntype: subject\n---\n# Passeport\n\n## Où on en est\n- Renouveler avant l'été\n");
        page(&config, Part::Main, "Sujets/Impôts 2026.md", "---\ntype: subject\n---\n# Impôts 2026\n\n## Où on en est\n- Déclaration reçue\n");
        let docs = vec![
            json!({"agent": "daily", "agent_name": "Daily", "file": "Facture FR-2026-00123.pdf", "name": "Facture FR-2026-00123.pdf",
                   "type": "pdf", "date": "2026-11-02T10:00:00+00:00", "subjects": []}),
            json!({"agent": "daily", "agent_name": "Daily", "file": "Budget 2026.xlsx", "name": "Budget 2026.xlsx",
                   "type": "spreadsheet", "date": "2026-11-02T10:00:00+00:00", "subjects": []}),
        ];
        let items = entries(&config, &docs);
        let all = serde_json::to_string(&items.iter().map(sent).collect::<Vec<_>>()).unwrap();
        for never in ["18AB12345", "FR-2026", "00123"] {
            assert!(!all.contains(never), "{never} in the copy: {all}");
        }
        // a year is no reference: those stay
        assert!(all.contains("Impôts 2026.md") && all.contains("Budget 2026.xlsx"), "{all}");
    }

    /// Review 2 of S3, B4: a line's `source` is its section's title, as written: a reference in it is masked too.
    #[test]
    fn a_reference_in_a_sections_title_never_reaches_the_copy() {
        let (_d, config) = config();
        page(&config, Part::Main, "Sujets/Papiers.md", "---\nid: p0000000009\ntype: subject\n---\n# Papiers\n\n## Passeport 18AB12345\n- Expire en 2031\n");
        let items = entries(&config, &[]);
        let all = serde_json::to_string(&items.iter().map(sent).collect::<Vec<_>>()).unwrap();
        assert!(!all.contains("18AB12345"), "{all}");
        let line = items.iter().find(|e| e.line == "Expire en 2031").unwrap();
        assert_eq!(line.source.as_deref(), Some("Passeport …"));
    }

    /// Review 2 of S3: a file's extension is set aside in a path or a file's name only, never in a line's words (« le
    /// code 12345.AB1 » holds a reference); only a real file's extension.
    #[test]
    fn an_extension_hides_no_reference() {
        assert!(reference("Code client 12345.AB1"));
        assert!(named_reference("Dossier 12345.AB1"));
        assert!(!named_reference("Budget 2026.xlsx") && !named_reference("Sujets/Impôts 2026.md"));
        assert!(named_reference("Facture FR-2026-00123.pdf"));
        assert!(!reference("Voir le devis.pdf"));
    }

    /// Review 1 of S3, B3: a page's items are kept by what the page says, never by its file's size and time alone
    /// (an agent's write or « Oublie ça » between the read and the look at the file left the old line in the copy).
    #[test]
    fn a_page_rewritten_is_read_again_whatever_its_size_and_time() {
        let (_d, config) = config();
        let file = super::super::part_root(&config, Part::Main).join("Sujets/Jardin.md");
        let text = |line: &str| format!("---\nid: p0000000005\ntype: subject\n---\n# Jardin\n\n## Où on en est\n- {line}\n");
        page(&config, Part::Main, "Sujets/Jardin.md", &text("Tailler la haie"));
        let when = std::fs::metadata(&file).unwrap().modified().unwrap();
        assert!(entries(&config, &[]).iter().any(|e| e.line == "Tailler la haie"));
        // the same length, the same time: another line
        std::fs::write(&file, text("Planter un lila")).unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
        let lines: Vec<String> = entries(&config, &[]).into_iter().filter(|e| e.kind == "line").map(|e| e.line).collect();
        assert_eq!(lines, ["Planter un lila"]);
    }

    /// Review 1 of S3, R2: the tables follow the app's language, so its ETag says the languages too.
    #[test]
    fn the_etag_says_the_languages_of_the_tables() {
        let (_d, config) = config();
        page(&config, Part::Main, "Sujets/Jardin.md", "---\nid: p0000000006\ntype: subject\n---\n# Jardin\n");
        let (_, fr) = answer(&config, &["fr".to_string()], None, None, "");
        let (_, fr_en) = answer(&config, &["fr".to_string(), "en".to_string()], None, None, "");
        assert_ne!(fr, fr_en);
        assert!(fr.ends_with("-fr\"") && fr_en.ends_with("-fr.en\""), "{fr} {fr_en}");
    }

    /// Review 1 of S3, R7: a language asked of two letters that are not ASCII never breaks the route.
    #[test]
    fn a_language_of_any_letters_is_read() {
        assert_eq!(langs("fr", Some("aé")), ["fr"]);
        assert_eq!(langs("fr", Some("NL-be")), ["fr", "nl"]);
        assert_eq!(langs("en", Some("é")), ["en"]);
    }

    /// Review 1 of S3, R8: two files of the same name an agent received are one note of the copy (the newest), never
    /// two items of the same id.
    #[test]
    fn two_files_of_the_same_name_are_one_item() {
        let (_d, config) = config();
        let doc = |date: &str| json!({"agent": "daily", "agent_name": "Daily", "file": "image.jpg", "name": "image.jpg",
                                      "type": "image", "date": date, "subjects": []});
        let items = entries(&config, &[doc("2026-11-02T10:00:00+00:00"), doc("2026-10-02T10:00:00+00:00")]);
        let docs: Vec<&Entry> = items.iter().filter(|e| e.kind == "document").collect();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].noted.as_deref(), Some("2026-11-02"));
    }

    /// Review 1 of S3, R9: each language's stop words only (an English Carnet keeps « van », « bin », « hay », « met »,
    /// « comment »; a German one « Mais »).
    #[test]
    fn the_stop_words_are_each_languages() {
        let stop = |lang: &str| tables(&[lang.to_string()], 2026).stop;
        let en = stop("en");
        for kept in ["van", "bin", "hay", "met", "comment", "mais", "die", "het"] {
            assert!(!en.contains(&kept.to_string()), "en drops {kept}");
        }
        assert!(en.contains(&"the".to_string()) && en.contains(&"um".to_string()));
        assert!(!stop("de").contains(&"mais".to_string()) && stop("de").contains(&"der".to_string()));
        assert!(stop("fr").contains(&"le".to_string()) && stop("fr").contains(&"euh".to_string()));
        assert!(stop("nl").contains(&"het".to_string()) && !stop("nl").contains(&"the".to_string()));
        // the app's language adds its own
        let both = tables(&["fr".to_string(), "en".to_string()], 2026).stop;
        assert!(both.contains(&"le".to_string()) && both.contains(&"the".to_string()));
    }

    /// Review 1 of S3, R6: « Annuler » on a page gone since forgets what the correction taught, then says it is gone.
    #[test]
    fn an_undo_on_a_page_gone_forgets_its_rule() {
        let (_d, config) = config();
        page(&config, Part::Main, "Sujets/Cuisine.md", "---\nid: p0000000007\ntype: subject\n---\n# Cuisine\n\n## Où on en est\n- Devis reçu\n");
        let you = Who::you(&config);
        let target = c::Target::Page { path: "Sujets/Cuisine.md".into() };
        let done = c::categorize(&config, &target, &["money".to_string()], &you).unwrap().unwrap();
        let word = done["learnt"]["word"].as_str().unwrap().to_string();
        let undo = done["undo"].as_str().unwrap().to_string();
        std::fs::remove_file(super::super::part_root(&config, Part::Main).join("Sujets/Cuisine.md")).unwrap();
        assert!(c::undo(&config, &undo, &you).unwrap().is_none());
        let state = super::super::read_state(&config);
        let rules = state.get("category_rules").cloned().unwrap_or_default().to_string();
        let undos = state.get("category_undo").cloned().unwrap_or_default().to_string();
        assert!(!rules.contains(&format!("\"{word}\"")), "{rules}");
        assert!(!undos.contains(&undo), "{undos}");
    }

    #[test]
    fn the_neighbours_and_date_phrases_are_there_in_five_languages() {
        for lang in super::super::search_words::LANGS {
            let t = tables(&[lang.to_string()], 2026);
            assert!(t.neighbours.len() >= 250, "{lang}: {} neighbours", t.neighbours.len());
            assert!(t.dates.phrases.len() >= 15, "{lang}: {} date phrases", t.dates.phrases.len());
            for p in &t.dates.phrases {
                for end in [&p.from, &p.to] {
                    if let Some(f) = &end.feast {
                        assert!(t.dates.feasts.contains_key(f), "{lang}: {} names {f}", p.words);
                    }
                }
            }
        }
    }
}
