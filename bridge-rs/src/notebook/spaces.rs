//! Perso and Pro (plan « Le sommaire » §2.3, brick S5; the choices of 17/11; contracts/carnet/sommaire.md §3c).
//!
//! - **The setting** « Séparer Perso, Pro et Famille » (Réglages › Carnet): `state.json` `separate_spaces`, off at
//!   first. Off, everything is together, as before. It decides two things only: where an agent of the person's Pro
//!   group makes its new pages, and the sommaire's selector (`spaces`). Switched off again, nothing is lost: the
//!   pages keep their space, everything shows together.
//! - **A page's space**: `space: pro` in its frontmatter (readable in Obsidian), nothing for Perso. Its lines, its
//!   tasks (their first `[[link]]`, as their categories) and the documents linked to it or named on it follow it. A
//!   record, a task, a document or a single line the person files in another space keeps a mark in `state.json`
//!   `space_marks` (keys only, never a text, a path or a file's name: « Oublie ça »).
//! - **A Pro agent** (assign.rs `is_work_agent`: its group says Pro, or it is the work Bip), the setting on: a page
//!   it **makes** (a person, a subject, an area, a trip) is Pro; never « Toi », the pages that are not notes (tasks,
//!   « À ranger », records, documents) nor the private part. A page that exists keeps its space: nothing moves by
//!   itself (8 B). It reads everything (7 B): no new rule of reading.
//! - **« Ranger dans… Perso / Pro »**: `POST /v1/notebook/categorize` with `space`, alone (from the page) or with
//!   `categories` (« Ranger ailleurs »): one gesture, one undo (`/categorize/undo`), one line of today's re-filings.
//! - **Santé** is in no space: its pages never get one (one written by hand is not read), it is never in Pro, its
//!   page and its search ignore the space asked.
//! - **Nothing changes space without the person** (review 1, 23/11): no merge of two pages of different spaces
//!   (mod.rs `merge_locked`), the trips split from a page in its space (trip_split.rs); « Toi » never in Pro.

use serde_json::{json, Value};

use super::categories::{self as c, Target};
use super::finder::Finder;
use super::{records, Page, Part, Task, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// The spaces a note may be in (Perso: nothing written).
pub const SPACES: [&str; 2] = ["perso", "pro"];
/// The setting's key in `state.json` (and in `GET/PATCH /v1/notebook/settings`).
pub const SETTING: &str = "separate_spaces";
const MARKS: &str = "space_marks";
const UNDO: &str = "space_undo";
/// The person's marks of records, tasks, documents and lines kept.
const MARKS_KEPT: usize = 2000;

/// Why « Ranger dans… » is refused (the route's codes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refused {
    Category(c::Refused),
    /// Not `perso` nor `pro`.
    SpaceUnknown,
    /// « Toi » in Pro: the person's own page is always Perso (17/11; review 1 of S5, R4).
    SpaceNotGiven,
}

impl Refused {
    pub fn code(&self) -> &'static str {
        match self {
            Refused::Category(r) => r.code(),
            Refused::SpaceUnknown => "space_unknown",
            Refused::SpaceNotGiven => "space_not_given",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Refused::Category(c::Refused::NotFound) => 404,
            Refused::Category(c::Refused::HandWritten) => 409,
            _ => 400,
        }
    }
}

// -- the setting ------------------------------------------------------------------------------------------------------

/// « Séparer Perso, Pro et Famille » (off at first).
pub fn on(config: &Config) -> bool {
    on_in(&super::read_state(config))
}

pub fn on_in(state: &Dict) -> bool {
    state.get(SETTING).and_then(Value::as_bool).unwrap_or(false)
}

/// The setting changed (under the Carnet's lock; `PATCH /v1/notebook/settings` writes it in its own change).
pub fn set_on(config: &Config, on: bool) -> std::io::Result<()> {
    let _guard = super::lock();
    super::update_state(config, |s| {
        s.insert(SETTING.into(), json!(on));
    })
    .map(|_| ())
}

/// A space's name in `lang` (the words of the home's groups of agents: « Perso », « Pro »; Dutch proposed).
pub fn title(lang: &str, space: &str) -> &'static str {
    let pro = space == "pro";
    match lang {
        "fr" => if pro { "Pro" } else { "Perso" },
        "de" => if pro { "Arbeit" } else { "Privat" },
        "es" => if pro { "Trabajo" } else { "Personal" },
        "nl" => if pro { "Werk" } else { "Privé" },
        _ => if pro { "Work" } else { "Personal" },
    }
}

/// The space a request or a body asks to file in (`perso`, `pro`).
pub fn asked(space: &str) -> Result<&'static str, Refused> {
    let space = space.trim().to_lowercase();
    SPACES.iter().find(|s| **s == space).copied().ok_or(Refused::SpaceUnknown)
}

// -- a page's space ---------------------------------------------------------------------------------------------------

/// The space written on a page (`space: pro`, in any case; anything else is Perso). Santé's pages: none (Perso).
pub fn of_page(part: Part, page: &Page) -> &'static str {
    if part == Part::Health {
        return "perso";
    }
    match page.front.get("space").and_then(Value::as_str) {
        Some(s) if s.trim().eq_ignore_ascii_case("pro") => "pro",
        _ => "perso",
    }
}

/// The page's frontmatter with `space: pro` (after `categories_by` when there, else before `updated`), or without
/// `space` for Perso.
pub fn set_front(page: &mut Page, space: &str) {
    if space != "pro" {
        page.front.shift_remove("space");
        return;
    }
    let after_by = page.front.contains_key("categories_by");
    let mut front = Dict::new();
    let mut placed = false;
    for (k, v) in std::mem::take(&mut page.front) {
        if k == "space" {
            continue;
        }
        if !placed && !after_by && k == "updated" {
            front.insert("space".into(), json!("pro"));
            placed = true;
        }
        let by = k == "categories_by";
        front.insert(k, v);
        if !placed && after_by && by {
            front.insert("space".into(), json!("pro"));
            placed = true;
        }
    }
    if !placed {
        front.insert("space".into(), json!("pro"));
    }
    page.front = front;
}

/// Where a frontmatter ends, as `Page::parse` reads it: (the end of its YAML, where its closing `---` starts).
fn frontmatter_end(after: &str) -> Option<(usize, usize)> {
    after
        .find("\n---\n")
        .map(|i| (i, i + 1))
        .or_else(|| after.strip_suffix("\n---").map(|a| (a.len(), a.len() + 1)))
        .or(if after.starts_with("---\n") { Some((0, 0)) } else { None })
}

/// The page's text with `space: pro` in its frontmatter (`space` = `pro`) or without its `space` line (`perso`), and
/// nothing else changed: the line put where it was, else after `categories_by:`, else before `updated:`, else at the
/// end; a page without a frontmatter gets one, one that held only `space` loses it. Checked by reading it back (the
/// same frontmatter otherwise, the same title, the same text). None when the bridge cannot change it safely (another
/// line ending, a byte-order mark, YAML that does not parse).
pub fn with_header(text: &str, space: &str) -> Option<String> {
    if text.contains('\r') || text.starts_with('\u{feff}') {
        return None;
    }
    let ours = (space == "pro").then_some("space: pro\n");
    let out = match text.strip_prefix("---\n") {
        Some(after) => {
            // an empty frontmatter with a rule further down: its lines would show in the text (as categories_pass)
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
                if line.starts_with("space:") {
                    list = line.trim_end() == "space:";
                    at.get_or_insert(kept.len());
                    continue;
                }
                if list && (line.starts_with(' ') || line.starts_with('-')) {
                    continue;
                }
                list = false;
                kept.push(line);
            }
            let place = at
                .or_else(|| kept.iter().position(|l| l.starts_with("categories_by:")).map(|i| i + 1))
                .or_else(|| kept.iter().position(|l| l.starts_with("updated:")))
                .unwrap_or(kept.len());
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
                // a frontmatter that held only `space`: taken out
                after[close..].strip_prefix("---").map(|rest| rest.strip_prefix('\n').unwrap_or(rest).to_string())?
            } else {
                format!("---\n{head}{}", &after[close..])
            }
        }
        None if text.starts_with("---") => return None,
        None => match ours {
            Some(ours) => format!("---\n{ours}---\n{text}"),
            None => text.to_string(),
        },
    };
    let (old, new) = (Page::parse(text, ""), Page::parse(&out, ""));
    let without = |p: &Page| {
        let mut f = p.front.clone();
        f.shift_remove("space");
        f
    };
    let same = without(&old) == without(&new) && old.title == new.title && old.body == new.body;
    (same && of_page(Part::Main, &new) == space).then_some(out)
}

/// At a write of a page: a page a Pro agent **makes** (`created`), the setting on, is Pro (a person, a subject, an
/// area, a trip; never « Toi », a page that is not a note, the private part). Whether it changed.
pub fn on_write(config: &Config, part: Part, path: &str, page: &mut Page, who: &Who, created: bool) -> bool {
    if !created || part != Part::Main || page.front.contains_key("space") {
        return false;
    }
    let Some(agent) = who.agent.as_deref() else { return false };
    if !["person", "subject", "area", "trip"].contains(&page.kind().as_str()) {
        return false;
    }
    let w = super::words(&super::language(config));
    if path == w.inbox_path() || path == w.tasks_path() || path == w.self_path() {
        return false;
    }
    if !super::assign::is_work_agent(config, agent) || !on(config) {
        return false;
    }
    set_front(page, "pro");
    true
}

// -- records, tasks, documents, lines ---------------------------------------------------------------------------------

/// The person's marks (`space_marks`: key → its space).
pub fn marks(config: &Config) -> Dict {
    marks_in(&super::read_state(config))
}

fn marks_in(state: &Dict) -> Dict {
    state.get(MARKS).and_then(Value::as_object).cloned().unwrap_or_default()
}

fn marked(marks: &Dict, key: &str) -> Option<&'static str> {
    match marks.get(key).and_then(|m| m.get("space")).and_then(Value::as_str) {
        Some("pro") => Some("pro"),
        Some("perso") => Some("perso"),
        _ => None,
    }
}

/// A line's space: the person's mark for it, else its page's.
pub fn of_line(page_space: &str, line_key: &str, marks: &Dict) -> &'static str {
    marked(marks, line_key).unwrap_or(if page_space == "pro" { "pro" } else { "perso" })
}

/// The page a task belongs to: its first `[[link]]` (as its categories).
fn task_page<'a>(task: &Task, pages: &'a [(String, Page)]) -> Option<&'a (String, Page)> {
    let link = task.links.first()?;
    pages.iter().find(|(path, p)| p.named(link) || std::path::Path::new(path).file_stem().is_some_and(|s| s.to_string_lossy() == *link))
}

/// A task's space: the person's mark, else its page's, else Perso.
pub fn of_task(task: &Task, pages: &[(String, Page)], marks: &Dict) -> &'static str {
    if task.part == Part::Health {
        return "perso";
    }
    marked(marks, &c::task_key(&task.id))
        .or_else(|| task_page(task, pages).map(|(_, p)| of_page(Part::Main, p)))
        .unwrap_or("perso")
}

/// A record's space: the person's mark, else Perso.
pub fn of_record(id: &str, marks: &Dict) -> &'static str {
    marked(marks, &c::record_key(id)).unwrap_or("perso")
}

/// A document's space: the person's mark, else the page it is linked to or named on, else Perso.
pub fn of_document(doc: &Value, pages: &[(String, Page)], marks: &Dict) -> &'static str {
    let key = c::document_key(&tk::str_or_empty(doc.get("agent")), &tk::str_or_empty(doc.get("file")));
    if let Some(s) = marked(marks, &key) {
        return s;
    }
    if tk::str_or_empty(doc.get("part")) == "health" {
        return "perso";
    }
    let linked = doc.get("linked").and_then(|l| l.get("path")).and_then(Value::as_str).map(str::to_string);
    let named = doc.get("subjects").and_then(Value::as_array).into_iter().flatten();
    let named = named.filter_map(|s| s.get("path").and_then(Value::as_str).map(str::to_string));
    for path in linked.into_iter().chain(named) {
        if let Some((_, page)) = pages.iter().find(|(p, _)| *p == path) {
            return of_page(Part::Main, page);
        }
    }
    "perso"
}

// -- « Ranger dans… » -------------------------------------------------------------------------------------------------

/// What a target is now: its key (the state's, the Item's id) and its space. None: not in the shared part.
fn found(config: &Config, target: &Target) -> Option<(String, &'static str)> {
    let marks = marks(config);
    match target {
        Target::Page { path } => {
            super::checked(path).ok()?;
            let page = Page::parse(&super::read_page_text(config, Part::Main, path)?, "");
            Some((format!("page:{}", c::page_key(&page, path)), of_page(Part::Main, &page)))
        }
        Target::Line { path, text } => {
            super::checked(path).ok()?;
            let page = Page::parse(&super::read_page_text(config, Part::Main, path)?, "");
            let line = c::line_on(&page, text)?;
            let key = c::line_key(&c::page_key(&page, path), &line);
            let space = of_line(of_page(Part::Main, &page), &key, &marks);
            Some((key, space))
        }
        Target::Task(id) => {
            let task = super::tasks(config, Part::Main).into_iter().find(|t| t.id == *id)?;
            let pages = super::all_pages(config, Part::Main);
            Some((c::task_key(id), of_task(&task, &pages, &marks)))
        }
        Target::Record { kind, id } => {
            let t = records::kind(kind)?;
            let mut all = records::records(config, t).into_iter().chain(records::archived_records(config, t));
            all.any(|r| tk::str_or_empty(r.get("id")) == *id).then(|| (c::record_key(id), of_record(id, &marks)))
        }
        Target::Document { agent, file } => {
            let docs = super::documents(config, Part::Main);
            let doc = docs.iter().find(|d| tk::str_or_empty(d.get("agent")) == *agent && tk::str_or_empty(d.get("file")) == *file)?;
            let pages = super::all_pages(config, Part::Main);
            Some((c::document_key(agent, file), of_document(doc, &pages, &marks)))
        }
        Target::Item(_) => None,
    }
}

/// The space a target has without a mark of its own (its page's, Perso for a record).
fn inherited(config: &Config, target: &Target) -> &'static str {
    match target {
        Target::Line { path, .. } => {
            super::read_page_text(config, Part::Main, path).map(|t| of_page(Part::Main, &Page::parse(&t, ""))).unwrap_or("perso")
        }
        Target::Task(id) => {
            let pages = super::all_pages(config, Part::Main);
            super::tasks(config, Part::Main)
                .into_iter()
                .find(|t| t.id == *id)
                .and_then(|t| task_page(&t, &pages).map(|(_, p)| of_page(Part::Main, p)))
                .unwrap_or("perso")
        }
        Target::Document { agent, file } => {
            let docs = super::documents(config, Part::Main);
            let pages = super::all_pages(config, Part::Main);
            docs.iter()
                .find(|d| tk::str_or_empty(d.get("agent")) == *agent && tk::str_or_empty(d.get("file")) == *file)
                .map(|d| of_document(d, &pages, &Dict::new()))
                .unwrap_or("perso")
        }
        _ => "perso",
    }
}

/// What a target's categories are now (an answer that changes only its space shows them as they are).
fn filed_now(config: &Config, target: &Target) -> (Vec<String>, String) {
    let marks = c::marks(config);
    let ctx = c::Ctx::new(config);
    let by_mark = |key: &str| match marks.get(key).and_then(|m| m.get("by")).and_then(Value::as_str) {
        Some("auto") => "auto".to_string(),
        Some(_) => "person".to_string(),
        None if marks.contains_key(key) => "person".to_string(),
        None => "auto".to_string(),
    };
    let undated = |cats: Vec<String>| -> Vec<String> { cats.into_iter().filter(|c| c != "dates").collect() };
    match target {
        Target::Page { path } => super::read_page_text(config, Part::Main, path)
            .map(|t| {
                let f = c::of_page(config, Part::Main, path, &Page::parse(&t, ""));
                (f.categories, f.by)
            })
            .unwrap_or_default(),
        Target::Line { path, text } => {
            let Some(page) = super::read_page_text(config, Part::Main, path).map(|t| Page::parse(&t, "")) else { return Default::default() };
            let key = c::page_key(&page, path);
            let filed = c::of_page(config, Part::Main, path, &page);
            let cats = undated(c::of_line(&filed.categories, &key, text, "", &marks, chrono::NaiveDate::MIN));
            (cats, by_mark(&c::line_key(&key, text)))
        }
        Target::Task(id) => {
            let pages = super::all_pages(config, Part::Main);
            let Some(task) = super::tasks(config, Part::Main).into_iter().find(|t| t.id == *id) else { return Default::default() };
            (undated(c::of_task(&ctx, &task, &pages, &marks, chrono::NaiveDate::MIN)), by_mark(&c::task_key(id)))
        }
        Target::Record { kind, id } => {
            let Some(t) = records::kind(kind) else { return Default::default() };
            let mut all = records::records(config, t).into_iter().chain(records::archived_records(config, t));
            let Some(r) = all.find(|r| tk::str_or_empty(r.get("id")) == *id) else { return Default::default() };
            let (cats, by, _) = c::of_record(t, &r, &marks, chrono::NaiveDate::MIN);
            (undated(cats), by)
        }
        Target::Document { agent, file } => {
            let docs = super::documents(config, Part::Main);
            let pages = super::all_pages(config, Part::Main);
            let Some(doc) = docs.iter().find(|d| tk::str_or_empty(d.get("agent")) == *agent && tk::str_or_empty(d.get("file")) == *file) else {
                return Default::default();
            };
            let (cats, by, _) = c::of_document(&ctx, doc, &pages, &marks);
            (cats, by)
        }
        Target::Item(_) => Default::default(),
    }
}

/// A page's whole text with `space` (its header's line only when the bridge can; else the page written again).
fn page_with(text: &str, space: &str) -> String {
    with_header(text, space).unwrap_or_else(|| {
        let mut page = Page::parse(text, "");
        set_front(&mut page, space);
        page.render()
    })
}

/// Sets `space` on `target` (found: its `key`, its space `before`) by the person, its undo kept under `undo`; the
/// Carnet's lock held by the caller. A page already written with it (with its categories, in one change) is not
/// written again.
#[allow(clippy::too_many_arguments)]
fn set_locked(config: &Config, target: &Target, key: &str, space: &str, before: &str, undo: &str, who: &Who) -> anyhow::Result<()> {
    let state = super::read_state(config);
    let mark_before = marks_in(&state).get(key).cloned();
    if let Target::Page { path } = target {
        if let Some(text) = super::read_page_text(config, Part::Main, path) {
            let after = page_with(&text, space);
            if after != text {
                super::commit(config, Part::Main, path, Some(&text), Some(&after), who, c::ACTION, "")?;
            }
        }
    }
    let is_page = key.starts_with("page:");
    let own = if is_page { "perso" } else { inherited(config, target) };
    super::update_state(config, |state| {
        if !is_page {
            let mut m = marks_in(state);
            m.shift_remove(key);
            // a mark only where the space is not the one it would have anyway (its page's)
            if space != own {
                m.insert(key.to_string(), json!({"space": space, "at": super::today()}));
            }
            if m.len() > MARKS_KEPT {
                let extra = m.len() - MARKS_KEPT;
                m = m.into_iter().skip(extra).collect();
            }
            state.insert(MARKS.into(), Value::Object(m));
        }
        let mut list: Vec<Value> = state.get(UNDO).and_then(Value::as_array).cloned().unwrap_or_default();
        list.retain(|e| tk::str_or_empty(e.get("key")) != key);
        list.insert(0, json!({"id": undo, "key": key, "set": space, "before": before, "mark_before": mark_before,
                              "at": super::today(), "time": tk::now_iso_seconds()}));
        c::keep_undo(&mut list, &super::today());
        state.insert(UNDO.into(), Value::Array(list));
    })?;
    Ok(())
}

/// Whether a target is « Toi » (the person's own page: always Perso, 17/11).
fn is_self(config: &Config, target: &Target) -> bool {
    let Target::Page { path } = target else { return false };
    let w = super::words(&super::language(config));
    *path == w.self_path()
        || super::read_page_text(config, Part::Main, path).is_some_and(|t| Page::parse(&t, "").kind() == "self")
}

/// « Rangé dans Pro. », « Rangé dans Argent et Pro. » (the categories first, Dates never named; visible text, the
/// pattern of S2's toast, for Alex).
pub fn toast(lang: &str, categories: &[String], space: &str) -> String {
    let mut names: Vec<String> = categories.iter().filter(|c| *c != "dates").map(|c| c::title(lang, c)).collect();
    names.push(title(lang, space).to_string());
    let and = match lang {
        "fr" => "et",
        "de" => "und",
        "es" => "y",
        "nl" => "en",
        _ => "and",
    };
    let joined = match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} {and} {last}", rest.join(", ")),
        None => String::new(),
    };
    let pattern = match lang {
        "fr" => "Rangé dans {a}.",
        "de" => "In {a} abgelegt.",
        "es" => "Guardado en {a}.",
        "nl" => "Ingedeeld bij {a}.",
        _ => "Filed in {a}.",
    };
    pattern.replace("{a}", &joined)
}

/// « Ranger ailleurs » (S1, `categories`) and « Ranger dans… Perso / Pro » (`space`), alone or together: one gesture,
/// under one lock, one change of a page, one undo. Without a space, S1's answer exactly; with one, `space` and
/// `before.space` added, and the toast saying it. « Toi » is never put in Pro (`space_not_given`).
pub fn categorize(
    config: &Config,
    target: &Target,
    categories: &[String],
    space: Option<&str>,
    lang: &str,
    who: &Who,
) -> anyhow::Result<Result<Value, Refused>> {
    let space = match space.map(asked) {
        None => {
            return Ok(c::categorize(config, target, categories, who)?
                .map(|v| super::categories_pass::with_toast(v, lang))
                .map_err(Refused::Category));
        }
        Some(Err(r)) => return Ok(Err(r)),
        Some(Ok(s)) => s,
    };
    let _guard = super::lock();
    let target = match target {
        Target::Item(id) => match c::resolve(config, id) {
            Some(t) => t,
            None => return Ok(Err(Refused::Category(c::Refused::NotFound))),
        },
        t => t.clone(),
    };
    let Some((key, before)) = found(config, &target) else { return Ok(Err(Refused::Category(c::Refused::NotFound))) };
    if space == "pro" && is_self(config, &target) {
        return Ok(Err(Refused::SpaceNotGiven));
    }
    let mut view = if categories.is_empty() {
        let (cats, by) = filed_now(config, &target);
        let mut shown = target.view();
        shown["item"] = json!(key);
        json!({"target": shown, "categories": cats, "categories_by": by,
               "before": {"categories": cats, "categories_by": by}, "learnt": null,
               "undo": format!("u{}", crate::py::token_hex(6)), "toast": null})
    } else {
        // a page: its categories and its space in one change of its text (review 1 of S5, R3)
        let header = |t: &str| page_with(t, space);
        let header: Option<&dyn Fn(&str) -> String> = matches!(target, Target::Page { .. }).then_some(&header);
        match c::categorize_locked(config, &target, categories, who, header)? {
            Ok(v) => v,
            Err(r) => return Ok(Err(Refused::Category(r))),
        }
    };
    let undo = tk::str_or_empty(view.get("undo"));
    set_locked(config, &target, &key, space, before, &undo, who)?;
    let cats = if categories.is_empty() { Vec::new() } else { super::strings(view.get("categories")) };
    view["space"] = json!(space);
    view["before"]["space"] = json!(before);
    view["toast"] = json!(toast(lang, &cats, space));
    view["undo_title"] = json!(super::categories_pass::undo_title(lang));
    Ok(Ok(view))
}

/// A page of the shared part by its key.
fn page_by_key(config: &Config, key: &str) -> Option<(String, Page)> {
    super::all_pages(config, Part::Main).into_iter().find(|(path, page)| c::page_key(page, path) == key)
}

/// The space part of an undo (`entry`, of `space_undo`): the space as it was (when it is still the one set), the mark
/// as it was; the Carnet's lock held by the caller. None: its page is gone. (the target as the answer shows it, its
/// space now)
fn undo_space_locked(config: &Config, id: &str, entry: &Value, who: &Who) -> anyhow::Result<Option<(Value, String)>> {
    let key = tk::str_or_empty(entry.get("key"));
    let set = if tk::str_or_empty(entry.get("set")) == "pro" { "pro" } else { "perso" };
    let before = if tk::str_or_empty(entry.get("before")) == "pro" { "pro" } else { "perso" };
    let page = key.strip_prefix("page:").and_then(|k| page_by_key(config, k));
    let mut target = json!({"item": key});
    let mut now = before.to_string();
    if let Some((path, p)) = &page {
        target = json!({"path": path, "private": false, "item": key});
        // untouched when its space is no longer the one this gesture set (changed by hand, or again; or put back
        // already, with its categories, in one change)
        if of_page(Part::Main, p) == set {
            let text = super::read_page_text(config, Part::Main, path).unwrap_or_default();
            let after = page_with(&text, before);
            if after != text {
                super::commit(config, Part::Main, path, Some(&text), Some(&after), who, c::ACTION, "")?;
            }
        } else {
            now = of_page(Part::Main, p).to_string();
        }
    }
    let gone = key.starts_with("page:") && page.is_none();
    super::update_state(config, |state| {
        if !key.starts_with("page:") {
            // (one undo a target, the newest: the mark now is this gesture's)
            let mut m = marks_in(state);
            m.shift_remove(&key);
            if let Some(mark) = entry.get("mark_before").filter(|v| v.is_object()) {
                m.insert(key.clone(), mark.clone());
            }
            state.insert(MARKS.into(), Value::Object(m));
        }
        let mut list: Vec<Value> = state.get(UNDO).and_then(Value::as_array).cloned().unwrap_or_default();
        list.retain(|e| tk::str_or_empty(e.get("id")) != id);
        state.insert(UNDO.into(), Value::Array(list));
    })?;
    if gone {
        return Ok(None);
    }
    if !key.starts_with("page:") {
        if let Some(t) = c::resolve(config, &key) {
            target = t.view();
            target["item"] = json!(key);
            now = found(config, &t).map(|(_, s)| s.to_string()).unwrap_or(now);
        }
    }
    Ok(Some((target, now)))
}

/// « Annuler » after « Ranger ailleurs » or « Ranger dans… »: the categories and the space as they were, under one lock
/// and, for a page, in one change of its text (S1's answer, `space` added when the gesture changed it). None: nothing
/// of that id any more.
pub fn undo(config: &Config, id: &str, _lang: &str, who: &Who) -> anyhow::Result<Option<Value>> {
    let _guard = super::lock();
    let state = super::read_state(config);
    let entry = state.get(UNDO).and_then(Value::as_array).into_iter().flatten().find(|e| tk::str_or_empty(e.get("id")) == id).cloned();
    // the page's space put back with its categories (one change), when it is still the one this gesture set
    let back = entry.as_ref().and_then(|e| {
        let key = tk::str_or_empty(e.get("key"));
        let (_, page) = page_by_key(config, key.strip_prefix("page:")?)?;
        let set = if tk::str_or_empty(e.get("set")) == "pro" { "pro" } else { "perso" };
        (of_page(Part::Main, &page) == set).then(|| if tk::str_or_empty(e.get("before")) == "pro" { "pro" } else { "perso" })
    });
    let header = |t: &str| page_with(t, back.unwrap_or("perso"));
    let header: Option<&dyn Fn(&str) -> String> = back.is_some().then_some(&header);
    let categories = c::undo_locked(config, id, who, header)?;
    let space = match &entry {
        Some(e) => undo_space_locked(config, id, e, who)?,
        None => None,
    };
    Ok(match (categories, space) {
        (Some(mut v), Some((_, now))) => {
            v["space"] = json!(now);
            Some(v)
        }
        (Some(v), None) => Some(v),
        (None, Some((target, now))) => {
            let t = c::resolve(config, tk::str_or_empty(target.get("item")).as_str());
            let (cats, by) = t.as_ref().map(|t| filed_now(config, t)).unwrap_or_default();
            Some(json!({"target": target, "categories": cats, "categories_by": if by.is_empty() { "auto".to_string() } else { by },
                        "unlearnt": null, "space": now}))
        }
        (None, None) => None,
    })
}

/// What a re-filing names now, for today's list: its target, its title (None: gone).
fn named(config: &Config, key: &str) -> Option<(Target, String)> {
    let target = c::resolve(config, key)?;
    let title = match &target {
        Target::Page { path } => {
            let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            Page::parse(&super::read_page_text(config, Part::Main, path)?, &stem).title
        }
        Target::Line { text, .. } => text.clone(),
        Target::Task(id) => {
            let task = super::tasks(config, Part::Main).into_iter().find(|t| t.id == *id)?;
            task.title.split(" [[").next().unwrap_or(&task.title).trim().to_string()
        }
        Target::Record { kind, id } => {
            let t = records::kind(kind)?;
            let mut all = records::records(config, t).into_iter().chain(records::archived_records(config, t));
            records::summary_head(t, &all.find(|r| tk::str_or_empty(r.get("id")) == *id)?)
        }
        Target::Document { file, .. } => file.clone(),
        Target::Item(_) => return None,
    };
    Some((target, title))
}

/// Today's re-filings (S2's list), the spaces with them: a gesture that set one says it (`space`, its text the
/// toast's); one that only set a space is listed too. The newest first.
pub fn today(config: &Config, lang: &str) -> Value {
    let mut view = super::categories_pass::today(config, lang);
    let state = super::read_state(config);
    let day = super::today();
    let entries: Vec<Value> =
        state.get(UNDO).and_then(Value::as_array).into_iter().flatten().filter(|e| tk::str_or_empty(e.get("at")) == day).cloned().collect();
    if entries.is_empty() {
        return view;
    }
    let mut items: Vec<Value> = view["items"].as_array().cloned().unwrap_or_default();
    for e in entries {
        let set = if tk::str_or_empty(e.get("set")) == "pro" { "pro" } else { "perso" };
        if let Some(item) = items.iter_mut().find(|i| i["undo"] == e["id"]) {
            let cats = super::strings(item.get("categories"));
            item["space"] = json!(set);
            item["text"] = json!(toast(lang, &cats, set));
            continue;
        }
        let key = tk::str_or_empty(e.get("key"));
        let Some((target, title)) = named(config, &key) else { continue };
        let (cats, by) = filed_now(config, &target);
        let mut shown = target.view();
        shown["item"] = json!(key);
        items.push(json!({"undo": e["id"], "target": shown, "title": title, "categories": cats,
                          "before": {"categories": cats, "categories_by": by},
                          "at": e.get("time").cloned().unwrap_or_else(|| e["at"].clone()), "text": toast(lang, &[], set),
                          "space": set}));
    }
    items.sort_by_key(|i| std::cmp::Reverse(tk::str_or_empty(i.get("at"))));
    view["items"] = json!(items);
    view
}

// -- what the sommaire shows ------------------------------------------------------------------------------------------

/// S5's part of the selector (`Summary.spaces`): the setting off, none; on, Perso, then Pro when the person has a Pro
/// agent or a Pro note, `asked` selected (`all`: none). summary.rs `selector` adds S6's « Famille » (family_carnet.rs).
pub fn choices(config: &Config, f: &Finder, asked: &str, lang: &str) -> Value {
    if !on(config) {
        return json!([]);
    }
    let mut agents: Vec<String> = config.agents.values().map(|a| a.name.clone()).collect();
    agents.extend(crate::agent_groups::all(config).keys().cloned());
    let pro = agents.iter().any(|a| super::assign::is_work_agent(config, a)) || f.items.iter().any(|e| e.space() == "pro");
    let mut out = vec![json!({"id": "perso", "title": title(lang, "perso"), "selected": asked == "perso"})];
    if pro {
        out.push(json!({"id": "pro", "title": title(lang, "pro"), "selected": asked == "pro"}));
    }
    json!(out)
}

/// Whether a kept search of `kept` (its space: `all` when kept with the setting off) shows in the view of `view`:
/// `all` every space's (everything together: switching off loses nothing), `perso` Perso's and those kept before
/// the setting (`all`: Perso is the tab the app opens on), `pro` Pro's.
pub fn kept_in_view(kept: &str, view: &str) -> bool {
    match view {
        "all" => true,
        "perso" => kept == "perso" || kept == "all",
        v => kept == v,
    }
}
