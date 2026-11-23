//! The family Carnet's sommaire (plan « Le sommaire » §4, brick S6; contracts/carnet/sommaire.md §3d,
//! sommaire.json `FamilySummary`, `FamilyIndex`, `FamilyCategoryPage`, `FamilyCategorized`): the household's shared
//! Carnet (`<data>/family-notebook.json` on the place that holds the family's agents, family_agent.rs) gets the same
//! sommaire as the person's own, relayed by the members' bridges as today's family routes.
//!
//!     GET    /v1/family-agent/notebook/summary?lang=               the sommaire (`FamilySummary`)
//!     GET    /v1/family-agent/notebook/category/{id}?lang=         a category's page (`FamilyCategoryPage`)
//!     GET    /v1/family-agent/notebook/search?q=&category=&lang=   `{groups, counts}` (`FamilySearch`)
//!     GET    /v1/family-agent/notebook/index?since=&epoch=&lang=   the phone's copy (`FamilyIndex`, ETag, 304, gzip)
//!     GET    /v1/family-agent/notebook/kept                        the household's kept searches (`KeptList`)
//!     POST   /v1/family-agent/notebook/kept                        {q, title?, category?} | {id, title}
//!     DELETE /v1/family-agent/notebook/kept?id=
//!     POST   /v1/family-agent/notebook/searched                    {q, category?, private?} → 204; DELETE ?q=&category=
//!     POST   /v1/family-agent/notebook/categorize                  {page} | {list} | {list, item} | {item}, categories
//!     POST   /v1/family-agent/notebook/categorize/undo             {undo}
//!
//! Between the bridges (the household key; the holder answers, `served` as today's family routes): the same under
//! `/family/fa/notebook/…` (`index` answers `{etag, index}`, the index null when the phone's ETag (`has`) is the one).
//!
//! - **Categories** (the family's: `shopping, people, dates, home, papers, holidays, school, menus, other`): filed by
//!   the bridge at each write (family_agent.rs `apply`: the apps' writes, the members', the family agent's — its tools
//!   unchanged), by words and without a model: the person's rules first (her corrections), then the family's words
//!   (`WORDS`: Courses, École, Menus, Vacances in five languages), then the person's Carnet's table (categories.rs
//!   `of_words`: Argent is Papiers here, Voyages Vacances, Travail Autres). A page and a list of things keep theirs in
//!   `family-notebook.json` (`categories`, `categories_by`); a list's items are its list's; an item of a list of tasks
//!   or of dates has its own words' (a date with none: in Dates only); Dates is computed. The person's correction
//!   (`categories_by: person`) is never changed and teaches the next notes (`category_rules`, its word). No nightly
//!   model: the family Carnet is small, each member's writing, and the holder's model is one person's (status-carnet-s6.md).
//! - **The copy** (`FamilyIndex`): each list item and each page; never a line holding a number or a reference (the
//!   person's copy's rule); versions and epoch in memory as index.rs (another holder, another epoch).
//! - **Kept searches**: common to the household, 6 at most, in `family-notebook.json` `kept` (with who kept them, for
//!   « Oublie ça »); in the copy, the copy's version moving with them.
//! - **Searches made** (« Cherché ici », the grey words): each person's own, kept by her own bridge
//!   (`<data>/family-searched.json`), never sent to the holder nor to another member, never read by an agent's tool.
//! - **Leaving**: a person with no family Carnet any more (taken out of the family's agent, a link removed, its holder
//!   gone) gets `left: true` from the index (epoch `left`, nothing): her phone drops its copy; her searches made are
//!   erased (not while a handover waits for her to take the Carnet over: they go on with it).

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{Datelike, NaiveDate, TimeZone};
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::notebook::categories::{self as c, Rule, When};
use crate::notebook::finder::{Entry, Finder};
use crate::notebook::{fold, index};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

/// The contract's version of the answers.
pub const V: u64 = 1;
/// What a person may give a family note (`dates` is computed; the family has no Santé, no Argent, no Travail).
pub const FAMILY_MAY_GIVE: [&str; 8] = ["shopping", "people", "home", "papers", "holidays", "school", "menus", "other"];
/// The household's kept searches (plan §3.5).
pub const KEPT_MAX: usize = 6;
/// Searches made kept a category (« Cherché ici »).
pub const SEARCHED_KEPT: usize = 3;
pub const RECENT: usize = 20;
pub const HINTS: usize = 3;
pub const Q_MAX: usize = 200;
pub const TITLE_MAX: usize = 80;
/// The epoch of a copy that is no more (`left: true`).
pub const LEFT: &str = "left";
const RULES_KEPT: usize = 200;
const UNDO_KEPT: usize = 20;
const LINE_MAX: usize = 240;
const HISTORY: usize = 64;

// -- the family's words ------------------------------------------------------------------------------------------------

/// The family's own words, folded (fr, en, de, es, nl): a stem starts a word (`=`: the whole word); a stem of 5 letters
/// or more also ends a compound (« Weihnachtsmenü »). A phrase (with a space) as it is. The rest comes from the
/// person's Carnet's table (categories.rs), its ids made the family's.
#[rustfmt::skip]
const WORDS: &[(&str, &[&str])] = &[
    ("shopping", &[
        "courses=", "liste de courses", "supermarch", "epicerie", "acheter", "shopping", "groceries", "grocery", "buy=",
        "einkauf", "lebensmittel", "kaufen", "supermarkt", "compras=", "compra=", "comprar", "supermercado", "boodschap",
        "kopen=",
    ]),
    ("school", &[
        "ecole", "scolaire", "rentree", "classe=", "maitresse", "instit", "cantine", "devoirs", "lycee", "creche",
        "garderie", "periscolaire", "cartable", "fournitures", "inscri", "kermesse", "judo", "karate", "school",
        "teacher", "homework", "kindergarten", "nursery", "daycare", "sign up", "enrol", "schule", "lehrer",
        "hausaufgab", "kita=", "klasse", "anmeld", "colegio", "escuela", "profesor", "deberes", "guarderia", "matricul",
        "apunt", "leraar", "juf=", "huiswerk", "kinderopvang", "inschrijv",
    ]),
    ("menus", &[
        "menu", "recette", "repas", "diner=", "dejeuner", "traiteur", "cuisiner", "recipe", "meal", "dinner", "lunch",
        "caterer", "catering", "cateraar", "rezept", "essen=", "abendessen", "mittagessen", "kochen", "speiseplan",
        "receta", "comida", "cena=", "cenas=", "almuerzo", "cocinar", "recept", "maaltijd", "avondeten", "eten=",
        "koken",
    ]),
    ("holidays", &[
        "vacance", "conges", "gite", "camping", "colonie", "holiday", "vacation", "urlaub", "ferien", "vacacion",
        "vakantie",
    ]),
];

/// The family's words in `text`: each category's points (`weight` a word) and its first word, the best first.
fn family_words(text: &str, weight: f64) -> Vec<(String, f64, String)> {
    let folded = fold(text);
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    let padded = format!(" {folded} ");
    let mut out: Vec<(String, f64, String)> = Vec::new();
    let mut add = |cat: &str, word: &str| match out.iter_mut().find(|(c, _, _)| c == cat) {
        Some(e) => e.1 += weight,
        None => out.push((cat.to_string(), weight, word.to_string())),
    };
    let mut taken = vec![false; words.len()];
    for (cat, stems) in WORDS {
        for stem in stems.iter().filter(|s| s.contains(' ')) {
            if padded.contains(&format!(" {stem} ")) {
                add(cat, stem);
                for part in stem.split(' ') {
                    if let Some(at) = words.iter().position(|w| *w == part) {
                        taken[at] = true;
                    }
                }
            }
        }
    }
    for (at, word) in words.iter().enumerate() {
        if taken[at] {
            continue;
        }
        let hit = WORDS.iter().find(|(_, stems)| {
            stems.iter().filter(|s| !s.contains(' ')).any(|stem| match stem.strip_suffix('=') {
                Some(body) => *word == body,
                None => {
                    word.starts_with(stem)
                        || (stem.len() >= 5 && word.len() >= stem.len() + 3 && word[3..].contains(stem))
                }
            })
        });
        if let Some((cat, _)) = hit {
            add(cat, word);
        }
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// A personal category as the family's (Argent: Papiers; Voyages: Vacances; Travail, Santé: Autres).
fn as_family(category: &str) -> &'static str {
    match category {
        "people" => "people",
        "home" => "home",
        "papers" | "money" => "papers",
        "trips" => "holidays",
        "shopping" => "shopping",
        "school" => "school",
        "menus" => "menus",
        "holidays" => "holidays",
        _ => "other",
    }
}

/// The person's Carnet's table on a family note's words, made the family's (« Autres » left out), and its word.
fn personal(title: &str, text: &str) -> (Vec<String>, Option<String>) {
    let (cats, word) = c::of_words(title, text, "fr", &[]);
    let mut out: Vec<String> = Vec::new();
    for cat in cats.iter().map(|c| as_family(c)) {
        if cat != "other" && !out.iter().any(|c| c == cat) {
            out.push(cat.to_string());
        }
    }
    (out, word)
}

/// A family note's categories by its words, and the word that decided: the person's rules first (her word, her
/// categories), then its title (the family's words, then the person's table), then its text; else « Autres ».
pub fn decide(title: &str, text: &str, rules: &[Rule]) -> (Vec<String>, Option<String>) {
    let padded = format!(" {} ", fold(&format!("{title} {text}")));
    if let Some(r) = rules.iter().find(|r| !r.word.is_empty() && padded.contains(&format!(" {} ", r.word))) {
        return (r.categories.clone(), Some(r.word.clone()));
    }
    // the title: a family word says what the note is (alone: « Courses » is not also Papiers because the person's
    // table counts groceries as spending); else the person's table on the title
    if let Some((cat, _, word)) = family_words(title, 5.0).into_iter().next() {
        return (vec![cat], Some(fold(&word)));
    }
    let (mine, word) = personal(title, "");
    if !mine.is_empty() {
        return (mine, word.map(|w| fold(&w)));
    }
    // then the text: the person's table on the whole note, else the family's words in its lines
    if !text.trim().is_empty() {
        let (mine, word) = personal(title, text);
        if !mine.is_empty() {
            return (mine, word.map(|w| fold(&w)));
        }
        if let Some((cat, _, word)) = family_words(text, 1.0).into_iter().next() {
            return (vec![cat], Some(fold(&word)));
        }
    }
    (vec!["other".into()], None)
}

// -- what family-notebook.json keeps --------------------------------------------------------------------------------------

fn str_of(v: &Value, key: &str) -> String {
    tk::str_or_empty(v.get(key))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// `given` as family categories: 1 or 2 of `FAMILY_MAY_GIVE`, each once.
fn valid(given: &[String]) -> Option<Vec<String>> {
    c::valid(given, &FAMILY_MAY_GIVE)
}

/// The categories written on a page, a list or an item, and by whom.
pub fn stored(v: &Value) -> Option<(Vec<String>, String)> {
    let cats = valid(&strings(v.get("categories")))?;
    let by = str_of(v, "categories_by");
    Some((cats, if c::BY.contains(&by.as_str()) { by } else { "auto".into() }))
}

fn set(v: &mut Value, categories: &[String], by: &str) {
    if let Some(o) = v.as_object_mut() {
        o.insert("categories".into(), json!(categories));
        o.insert("categories_by".into(), json!(by));
    }
}

fn unset(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        o.shift_remove("categories");
        o.shift_remove("categories_by");
    }
}

/// The person's rules (her corrections' words), the most recent first.
pub fn rules_of(data: &Dict) -> Vec<Rule> {
    data.get("category_rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let word = fold(&str_of(r, "word"));
            let cats = valid(&strings(r.get("categories")))?;
            (!word.is_empty()).then_some(Rule { word, categories: cats })
        })
        .collect()
}

fn needs_filing(v: &Value) -> bool {
    match stored(v) {
        None => !v.get("categories").is_some_and(|c| !c.is_null()) || v.get("categories_by").is_none(),
        Some((cats, by)) => by == "auto" && cats == ["other"],
    }
}

/// At each write (family_agent.rs `apply`): the pages and the lists of things without categories (or only the
/// bridge's « Autres ») filed by their words. A note already filed keeps its categories; the person's never change.
pub fn file_all(data: &mut Dict) {
    let rules = rules_of(data);
    if let Some(Value::Array(pages)) = data.get_mut("pages") {
        for page in pages.iter_mut().filter(|p| needs_filing(p)) {
            let (cats, _) = decide(&str_of(page, "title"), &str_of(page, "text"), &rules);
            set(page, &cats, "auto");
        }
    }
    if let Some(Value::Array(lists)) = data.get_mut("lists") {
        for list in lists.iter_mut().filter(|l| str_of(l, "kind") == "list" || str_of(l, "kind").is_empty()) {
            if needs_filing(list) {
                let (cats, _) = decide(&str_of(list, "title"), "", &rules);
                set(list, &cats, "auto");
            }
        }
    }
}

/// A page or a list renamed (family_agent.rs `apply`): the bridge's own categories go, filed again by the new title;
/// the person's stay.
pub fn renamed(v: &mut Value, title: &str) {
    if !title.is_empty() && str_of(v, "title") != title && stored(v).is_some_and(|(_, by)| by == "auto") {
        unset(v);
    }
}

/// A list's categories: its own (a list of things), none for a list of tasks or of dates.
fn list_categories(list: &Value, rules: &[Rule]) -> Vec<String> {
    match str_of(list, "kind").as_str() {
        "tasks" | "dates" => Vec::new(),
        _ => stored(list).map(|(c, _)| c).unwrap_or_else(|| decide(&str_of(list, "title"), "", rules).0),
    }
}

/// An item's categories: the person's, else its list's (a list of things), else its own words' (a task: « Autres »
/// when none; a date with none: in Dates only).
fn item_categories(
    list: &Value,
    list_cats: &[String],
    item: &Value,
    rules: &[Rule],
) -> (Vec<String>, String, Option<String>) {
    if let Some((cats, by)) = stored(item) {
        return (cats, by, None);
    }
    match str_of(list, "kind").as_str() {
        "tasks" => {
            let (cats, word) = decide(&str_of(item, "text"), "", rules);
            (cats, "auto".into(), word)
        }
        "dates" => {
            let (cats, word) = decide(&str_of(item, "text"), "", rules);
            (cats.into_iter().filter(|c| c != "other").collect(), "auto".into(), word)
        }
        _ => (list_cats.to_vec(), "auto".into(), None),
    }
}

/// A page's categories, by whom, and the word that decided.
fn page_categories(page: &Value, rules: &[Rule]) -> (Vec<String>, String, Option<String>) {
    match stored(page) {
        Some((cats, by)) => (cats, by, None),
        None => {
            let (cats, word) = decide(&str_of(page, "title"), &str_of(page, "text"), rules);
            (cats, "auto".into(), word)
        }
    }
}

// -- the copy --------------------------------------------------------------------------------------------------------------

fn day_of(at: f64) -> Option<String> {
    (at > 0.0).then(|| chrono::Local.timestamp_opt(at as i64, 0).single()).flatten().map(|d| d.date_naive().to_string())
}

/// A text's words for the copy, folded: never a word of digits but a year or one or two digits.
fn folded_words(text: &str) -> String {
    fold(text)
        .split(' ')
        .filter(|w| {
            !w.chars().all(|c| c.is_ascii_digit())
                || w.len() <= 2
                || (w.len() == 4 && (w.starts_with("19") || w.starts_with("20")))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A day of the family (`due`) for a text: every year when the text says so (a birthday), else that day.
fn when(text: &str, due: &str) -> (Option<String>, Option<String>) {
    let Ok(day) = NaiveDate::parse_from_str(due, "%Y-%m-%d") else { return (None, None) };
    let probe = format!("{text} {:02}/{:02}", day.day(), day.month());
    match c::when_of(&probe, Some(day), day) {
        Some(When::Yearly(..)) => (None, Some(format!("{:02}-{:02}", day.month(), day.day()))),
        _ => (Some(due.to_string()), None),
    }
}

/// The family Carnet's notes as the phone's copy holds them (sommaire.json `FamilyIndexItem`): each list item
/// (`fam:list:<list>/<item>`) and each page (`fam:page:<page>`), their categories without Dates (the phone adds it from
/// `date` / `yearly`); never a line holding a number or a reference.
pub fn entries(data: &Dict) -> Vec<Entry> {
    entries_at(data).into_iter().map(|(e, _)| e).collect()
}

/// [`entries`], each with when it was written (the latest first in the sommaire).
fn entries_at(data: &Dict) -> Vec<(Entry, f64)> {
    let rules = rules_of(data);
    let mut out = Vec::new();
    for list in data.get("lists").and_then(Value::as_array).into_iter().flatten() {
        let lid = str_of(list, "id");
        let Some(title) = index::masked(&str_of(list, "title")) else { continue };
        let cats = list_categories(list, &rules);
        for item in list.get("items").and_then(Value::as_array).into_iter().flatten() {
            // a line holding a number or a reference is never in the copy (the person's copy's rule); a title is masked
            let text = str_of(item, "text");
            if index::reference(&text) {
                continue;
            }
            let (categories, _, _) = item_categories(list, &cats, item, &rules);
            let (date, yearly) = when(&text, &str_of(item, "due"));
            let at = item.get("at").and_then(Value::as_f64).unwrap_or(0.0);
            let iid = str_of(item, "id");
            out.push((
                Entry {
                    id: format!("fam:list:{lid}/{iid}"),
                    kind: "list_item".into(),
                    title: title.clone(),
                    line: text.chars().take(LINE_MAX).collect(),
                    categories,
                    date,
                    yearly,
                    who: Some(str_of(item, "by")).filter(|b| !b.is_empty()),
                    noted: day_of(at),
                    open: json!({"list": lid, "item": iid}),
                    fold: folded_words(&text),
                    done: item.get("done").is_some_and(py::truthy),
                    ..Default::default()
                },
                at,
            ));
        }
    }
    for page in data.get("pages").and_then(Value::as_array).into_iter().flatten() {
        let Some(title) = index::masked(&str_of(page, "title")) else { continue };
        let kept: Vec<String> = str_of(page, "text")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !index::reference(l))
            .map(str::to_string)
            .collect();
        let text = kept.join("\n");
        let at = page.get("updated").and_then(Value::as_f64).unwrap_or(0.0);
        let pid = str_of(page, "id");
        out.push((
            Entry {
                id: format!("fam:page:{pid}"),
                kind: "page".into(),
                title: title.clone(),
                line: text.chars().take(LINE_MAX).collect(),
                categories: page_categories(page, &rules).0,
                who: Some(str_of(page, "by")).filter(|b| !b.is_empty()),
                noted: day_of(at),
                open: json!({"path": pid}),
                fold: folded_words(&format!("{title} {text}")),
                ..Default::default()
            },
            at,
        ));
    }
    out
}

/// The kept searches as the copy carries them (`KeptShort`).
fn kept_short(data: &Dict) -> Value {
    json!(kept_all(data)
        .iter()
        .map(|k| {
            let mut v = json!({"id": k.get("id"), "q": k.get("q"), "title": k.get("title")});
            if let Some(cat) = k.get("category").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                v["category"] = json!(cat);
            }
            v
        })
        .collect::<Vec<_>>())
}

// -- versions (as index.rs: in memory, an epoch a holder) ------------------------------------------------------------------

struct Kept {
    epoch: String,
    version: u64,
    hashes: HashMap<String, u64>,
    history: VecDeque<(u64, Vec<String>, Vec<String>)>,
    file: u64,
    items: Arc<Vec<Entry>>,
    at: Arc<Vec<f64>>,
    kept: Value,
    finders: HashMap<Vec<String>, Arc<Finder>>,
}

static KEPT: LazyLock<Mutex<HashMap<PathBuf, Kept>>> = LazyLock::new(Default::default);

fn hash_of(v: &impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn kept_lock() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Kept>> {
    KEPT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The copy now (made again when `family-notebook.json` changed), with its version.
fn refreshed(config: &Config) -> PathBuf {
    let path = crate::family_agent::notebook_file(config);
    let bytes = std::fs::read(&path).unwrap_or_default();
    let file = hash_of(&bytes);
    if kept_lock().get(&path).is_some_and(|k| k.file == file) {
        return path;
    }
    let data: Dict =
        serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let made = entries_at(&data);
    let hashes: HashMap<String, u64> = made
        .iter()
        .map(|(e, _)| (e.id.clone(), hash_of(&serde_json::to_vec(&index::sent(e)).unwrap_or_default())))
        .collect();
    let kept_now = kept_short(&data);
    let mut kept = kept_lock();
    let k = kept.entry(path.clone()).or_insert_with(|| Kept {
        epoch: format!("f{}", py::token_hex(4)),
        version: 0,
        hashes: HashMap::new(),
        history: VecDeque::new(),
        file: 0,
        items: Arc::new(Vec::new()),
        at: Arc::new(Vec::new()),
        kept: json!([]),
        finders: HashMap::new(),
    });
    let (items, at): (Vec<Entry>, Vec<f64>) = made.into_iter().unzip();
    let changed: Vec<String> =
        items.iter().filter(|e| k.hashes.get(&e.id) != hashes.get(&e.id)).map(|e| e.id.clone()).collect();
    let removed: Vec<String> = k.hashes.keys().filter(|id| !hashes.contains_key(*id)).cloned().collect();
    if k.version == 0 || !changed.is_empty() || !removed.is_empty() || k.kept != kept_now {
        k.version += 1;
        k.history.push_back((k.version, changed, removed));
        while k.history.len() > HISTORY {
            k.history.pop_front();
        }
        k.hashes = hashes;
        k.items = Arc::new(items);
        k.finders.clear();
        k.kept = kept_now;
    }
    // (when each note was written moves without its copy: an item ticked the same day)
    k.at = Arc::new(at);
    k.file = file;
    path
}

/// The copy ready to search (the same rule as the phones), its version and epoch, and when each note was written.
fn current(config: &Config, langs: &[String]) -> (Arc<Finder>, u64, String, Arc<Vec<f64>>) {
    let path = refreshed(config);
    let (items, version, epoch, at) = {
        let kept = kept_lock();
        let k = &kept[&path];
        if let Some(f) = k.finders.get(langs) {
            return (f.clone(), k.version, k.epoch.clone(), k.at.clone());
        }
        (k.items.clone(), k.version, k.epoch.clone(), k.at.clone())
    };
    let finder =
        Arc::new(Finder::shared(items.clone(), &index::tables(langs, chrono::Local::now().date_naive().year())));
    if let Some(k) = kept_lock().get_mut(&path) {
        if Arc::ptr_eq(&k.items, &items) {
            k.finders.insert(langs.to_vec(), finder.clone());
        }
    }
    (finder, version, epoch, at)
}

fn etag_of(epoch: &str, version: u64, langs: &[String]) -> String {
    format!("\"{epoch}-v{version}-{}\"", langs.join("."))
}

/// The copy's ETag now.
pub fn etag(config: &Config, langs: &[String]) -> String {
    let path = refreshed(config);
    let kept = kept_lock();
    let k = &kept[&path];
    etag_of(&k.epoch, k.version, langs)
}

fn tables_of(langs: &[String]) -> (Value, Value, Value, Value) {
    index::tables_json(&index::tables(langs, chrono::Local::now().date_naive().year()))
}

/// `GET …/index`: (the copy, its ETag). `since`/`epoch`: what the phone has.
pub fn index_answer(config: &Config, langs: &[String], since: Option<u64>, epoch: Option<&str>) -> (Value, String) {
    let path = refreshed(config);
    let kept = kept_lock();
    let k = &kept[&path];
    let first_kept = k.history.front().map(|h| h.0).unwrap_or(k.version);
    let diff = match (since, epoch) {
        (Some(s), Some(e)) if e == k.epoch && s <= k.version && s + 1 >= first_kept => Some(s),
        _ => None,
    };
    let (items, removed, full): (Vec<Value>, Vec<String>, bool) = match diff {
        Some(s) => {
            let mut changed: HashSet<&str> = HashSet::new();
            let mut gone: Vec<&str> = Vec::new();
            for (_, ch, rm) in k.history.iter().filter(|h| h.0 > s) {
                changed.extend(ch.iter().map(String::as_str));
                gone.extend(rm.iter().map(String::as_str));
            }
            let now: HashSet<&str> = k.items.iter().map(|e| e.id.as_str()).collect();
            let items = k.items.iter().filter(|e| changed.contains(e.id.as_str())).map(index::sent).collect();
            let mut seen = HashSet::new();
            let removed =
                gone.into_iter().filter(|id| !now.contains(id) && seen.insert(*id)).map(str::to_string).collect();
            (items, removed, false)
        }
        None => (k.items.iter().map(index::sent).collect(), Vec::new(), true),
    };
    let (stop, neighbours, dates, dated) = tables_of(langs);
    let body = json!({"v": V, "epoch": k.epoch, "version": k.version, "since": if full { Value::Null } else { json!(since) },
                      "full": full, "lang": langs.first().cloned().unwrap_or_default(), "made": tk::now_iso_seconds(),
                      "dated": dated, "items": items, "removed": removed, "stop": stop, "neighbours": neighbours,
                      "dates": dates, "kept": k.kept});
    (body, etag_of(&k.epoch, k.version, langs))
}

/// The copy of a person who has no family Carnet (any more): nothing, `left: true`, its own epoch: the phone drops
/// its copy (its file too).
pub fn left_answer(langs: &[String]) -> Value {
    let (stop, neighbours, dates, dated) = tables_of(langs);
    json!({"v": V, "epoch": LEFT, "version": 0, "since": null, "full": true,
           "lang": langs.first().cloned().unwrap_or_default(), "made": tk::now_iso_seconds(), "dated": dated,
           "items": [], "removed": [], "stop": stop, "neighbours": neighbours, "dates": dates, "kept": [], "left": true})
}

// -- the sommaire --------------------------------------------------------------------------------------------------------

struct Texts {
    carnet: &'static str,
    all_notes: &'static str,
    bar: &'static str,
    bar_in: &'static str,
    with: &'static str,
    and: &'static str,
}

/// The family sommaire's texts (visible: « Carnet », « Tout le Carnet », « Cherche dans … » are S4's; « Avec … » and
/// « Cherche pour la famille » the plan's; the other languages proposed, for Alex).
fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts {
            carnet: "Carnet",
            all_notes: "Tout le Carnet",
            bar: "Cherche pour la famille",
            bar_in: "Cherche dans {category}",
            with: "Avec {names}",
            and: "et",
        },
        "de" => Texts {
            carnet: "Notizbuch",
            all_notes: "Das ganze Notizbuch",
            bar: "Für die Familie suchen",
            bar_in: "In {category} suchen",
            with: "Mit {names}",
            and: "und",
        },
        "es" => Texts {
            carnet: "Cuaderno",
            all_notes: "Todo el Cuaderno",
            bar: "Buscar para la familia",
            bar_in: "Buscar en {category}",
            with: "Con {names}",
            and: "y",
        },
        "nl" => Texts {
            carnet: "Notitieboek",
            all_notes: "Het hele notitieboek",
            bar: "Zoeken voor het gezin",
            bar_in: "Zoeken in {category}",
            with: "Met {names}",
            and: "en",
        },
        _ => Texts {
            carnet: "Notebook",
            all_notes: "The whole Notebook",
            bar: "Search for the family",
            bar_in: "Search in {category}",
            with: "With {names}",
            and: "and",
        },
    }
}

/// « Avec Marie, Léa et Tom » (empty: nobody else).
pub fn with_names(lang: &str, names: &[String]) -> String {
    let t = texts(lang);
    let list = match names {
        [] => return String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} {} {last}", rest.join(", "), t.and),
    };
    t.with.replace("{names}", &list)
}

fn shown_of(langs: &[String]) -> String {
    langs.last().cloned().unwrap_or_else(|| "en".into())
}

/// The notes by category (`dates` as of today), and how many.
fn by_category(f: &Finder, today: NaiveDate) -> HashMap<String, Vec<usize>> {
    let mut out: HashMap<String, Vec<usize>> = HashMap::new();
    for at in 0..f.items.len() {
        let mut seen: Vec<String> = Vec::new();
        let mut cats = f.categories(at, today);
        if cats.is_empty() {
            // a date of the family with no word of a category, out of Dates' window: « Autres » (as the search groups it)
            cats.push("other".into());
        }
        for cat in cats {
            if !seen.contains(&cat) {
                out.entry(cat.clone()).or_default().push(at);
                seen.push(cat);
            }
        }
    }
    out
}

/// The newest written first; in Dates what is to come first (the soonest first), then the past.
fn latest(f: &Finder, when: &[f64], ats: &[usize], category: &str, today: NaiveDate) -> Vec<usize> {
    let mut out = ats.to_vec();
    if category == "dates" {
        out.sort_by_key(|at| match f.day(*at, today) {
            Some(d) if d >= today => (0, (d - today).num_days(), *at),
            Some(d) => (1, (today - d).num_days(), *at),
            None => (2, 0, *at),
        });
    } else {
        out.sort_by(|a, b| when[*b].partial_cmp(&when[*a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
    }
    out
}

/// A note as the screens show it (`FamilyIndexItem` without `fold`, a key at its default left out, its categories as
/// of `today`).
fn item_view(f: &Finder, at: usize, today: NaiveDate) -> Value {
    let mut v = index::sent(&f.items[at]);
    if let Some(o) = v.as_object_mut() {
        o.remove("fold");
        o.insert("categories".into(), json!(f.categories(at, today)));
    }
    v
}

/// What a note shows in the grey words: an item its line, a page its title.
fn shown_text(e: &Entry) -> &str {
    if e.kind == "page" {
        &e.title
    } else {
        &e.line
    }
}

/// The family's sommaire (`FamilySummary`): its texts in the app's language (`langs`' last), `with` the others.
pub fn summary_at(config: &Config, langs: &[String], today: NaiveDate, others: &[String]) -> Value {
    let shown = shown_of(langs);
    let t = texts(&shown);
    let (f, version, epoch, when) = current(config, langs);
    let members = by_category(&f, today);
    let mut lines = Vec::new();
    for id in c::FAMILY {
        let Some(ats) = members.get(id).filter(|a| !a.is_empty()) else { continue };
        let ordered = latest(&f, &when, ats, id, today);
        let mut hint: Vec<&str> = Vec::new();
        for at in &ordered {
            let x = shown_text(&f.items[*at]);
            if !x.is_empty() && !hint.contains(&x) {
                hint.push(x);
                if hint.len() == HINTS {
                    break;
                }
            }
        }
        let mut line = json!({"id": id, "title": c::title(&shown, id), "count": ats.len(), "hint": hint.join(", "), "locked": false});
        if let Some(at) = ordered.first() {
            line["first"] = item_view(&f, *at, today);
        }
        lines.push(line);
    }
    json!({"v": V, "lang": shown, "title": t.carnet, "with": with_names(&shown, others),
           "kept": kept_counted(config, &f, today, None), "total": {"count": f.items.len(), "title": t.all_notes},
           "categories": lines, "bar": {"placeholder": t.bar}, "version": version, "epoch": epoch})
}

/// A category's page (`FamilyCategoryPage`): its count, the latest 20 (Dates: what comes first); « Cherché ici » is
/// the asking person's, filled by her own bridge.
pub fn category_at(config: &Config, id: &str, langs: &[String], today: NaiveDate) -> Result<Value, Refused> {
    if !c::FAMILY.contains(&id) {
        return Err(Refused::CategoryUnknown);
    }
    let shown = shown_of(langs);
    let (f, _, _, when) = current(config, langs);
    let ats = by_category(&f, today).remove(id).unwrap_or_default();
    let recent: Vec<Value> =
        latest(&f, &when, &ats, id, today).into_iter().take(RECENT).map(|at| item_view(&f, at, today)).collect();
    let title = c::title(&shown, id);
    Ok(json!({"v": V, "id": id, "title": title, "count": ats.len(), "searched": [], "recent": recent,
              "bar": {"placeholder": texts(&shown).bar_in.replace("{category}", &title)}}))
}

/// The family's search (`FamilySearch`): by category, the two best of each, the chips' counts.
pub fn search_at(
    config: &Config,
    q: &str,
    category: Option<&str>,
    langs: &[String],
    today: NaiveDate,
) -> Result<Value, Refused> {
    if category.is_some_and(|x| !c::FAMILY.contains(&x)) {
        return Err(Refused::CategoryUnknown);
    }
    let shown = shown_of(langs);
    let (f, _, _, _) = current(config, langs);
    let found = f.search(q, today, category, None);
    let groups: Vec<Value> = found
        .groups
        .iter()
        .map(|g| {
            json!({"category": g.category, "title": c::title(&shown, &g.category), "count": g.count,
                        "top": g.top.iter().map(|at| item_view(&f, *at, today)).collect::<Vec<_>>()})
        })
        .collect();
    Ok(json!({"groups": groups, "counts": found.counts}))
}

/// The category of a search's best result (a search made without one: kept under it).
pub fn best_category(config: &Config, q: &str, langs: &[String], today: NaiveDate) -> Option<String> {
    let (f, _, _, _) = current(config, langs);
    f.search(q, today, None, None).groups.first().map(|g| g.category.clone())
}

// -- refusals ----------------------------------------------------------------------------------------------------------

/// Why a request is refused (the codes of S1 and S4, and `no_family`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refused {
    CategoryUnknown,
    CategoryCount,
    CategoryNotGiven,
    NotFound,
    UndoGone,
    KeptFull,
    KeptUnknown,
    KeptEmpty,
    NumberRefused,
    NotWritten,
    NoFamily,
    /// A list of tasks or of dates: no category of its own (its items have theirs; review 1 of S6, R6b).
    ListNotFiled,
}

impl Refused {
    pub fn code(&self) -> &'static str {
        match self {
            Refused::CategoryUnknown => "category_unknown",
            Refused::CategoryCount => "category_count",
            Refused::CategoryNotGiven => "category_not_given",
            Refused::NotFound => "not_found",
            Refused::UndoGone => "undo_gone",
            Refused::KeptFull => "kept_full",
            Refused::KeptUnknown => "kept_unknown",
            Refused::KeptEmpty => "kept_empty",
            Refused::NumberRefused => "number_refused",
            Refused::NotWritten => "not_written",
            Refused::NoFamily => "no_family",
            Refused::ListNotFiled => "list_not_filed",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Refused::NotFound | Refused::UndoGone | Refused::KeptUnknown | Refused::NoFamily => 404,
            Refused::KeptFull => 409,
            Refused::NotWritten => 500,
            _ => 400,
        }
    }

    /// The answer (`{"detail": {"code", "text"}}`): no text but `number_refused`'s (S4's), as S1's codes.
    pub fn error(&self, lang: &str) -> HttpError {
        let text = match (self, lang) {
            (Refused::NumberRefused, "fr") => "Les numéros ne sont jamais gardés.",
            (Refused::NumberRefused, "de") => "Nummern werden nie gespeichert.",
            (Refused::NumberRefused, "es") => "Los números nunca se guardan.",
            (Refused::NumberRefused, _) => "Numbers are never kept.",
            _ => "",
        };
        HttpError::json(self.status(), json!({"code": self.code(), "text": text}))
    }
}

// -- kept searches: the household's (family-notebook.json `kept`) ------------------------------------------------------

fn kept_all(data: &Dict) -> Vec<Dict> {
    data.get("kept")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|k| k.as_object().cloned()).collect())
        .unwrap_or_default()
}

/// Who kept a search: the household (its id on this place; `self` for this place's own person). Never a name.
pub const SELF: &str = "self";

fn kept_by(k: &Dict) -> String {
    tk::str_or_empty(k.get("by_household"))
}

/// The household's kept searches, each with what it finds now; `mine` (for `caller`, between the bridges only): the
/// ones that household kept (review 1 of S6, B1: a member's « Oublie ça » matches them on her own place).
fn kept_counted(config: &Config, f: &Finder, today: NaiveDate, caller: Option<&str>) -> Vec<Value> {
    kept_all(&crate::family_agent::notebook_data(config))
        .iter()
        .map(|k| {
            let q = tk::str_or_empty(k.get("q"));
            let category = k.get("category").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string);
            let count = f.search(&q, today, category.as_deref(), None).counts.get("all").copied().unwrap_or(0);
            let mut v = json!({"id": k.get("id"), "q": q, "title": tk::str_or_empty(k.get("title")), "count": count,
                               "space": null, "category": category});
            if let Some(caller) = caller {
                v["mine"] = json!(kept_by(k) == caller);
            }
            v
        })
        .collect()
}

/// `KeptList`: the household's kept searches, each with what it finds now.
pub fn kept_at(config: &Config, langs: &[String], today: NaiveDate) -> Value {
    kept_for(config, langs, today, None)
}

fn kept_for(config: &Config, langs: &[String], today: NaiveDate, caller: Option<&str>) -> Value {
    let (f, _, _, _) = current(config, langs);
    json!({"kept": kept_counted(config, &f, today, caller), "max": KEPT_MAX})
}

/// The households whose person uses this place's family Carnet now (its family agents' members).
fn members_now(config: &Config) -> HashSet<String> {
    crate::family_agent::owns(config).iter().flat_map(|a| crate::agent_shares::shared_with(config, a)).collect()
}

/// The kept searches of a person who left (taken out of the family agents, a link removed: review 1 of S6, R3) go:
/// only this place's own person's and its members' stay. How many went.
pub fn prune_kept(config: &Config) -> usize {
    let members = members_now(config);
    let mut gone = 0;
    let done = crate::family_agent::change_data(config, |data| {
        let all = kept_all(data);
        let left: Vec<Dict> =
            all.iter().filter(|k| kept_by(k) == SELF || members.contains(&kept_by(k))).cloned().collect();
        gone = all.len() - left.len();
        if gone > 0 {
            data.insert("kept".into(), json!(left));
        }
    });
    if let Err(e) = done {
        tracing::warn!(error = %e, "family Carnet: the kept searches of who left not taken out");
    }
    gone
}

/// The kept searches as the handover carries them to `receiver` (review 1 of S6, R3): the receiver's become her own
/// (`self`), the leaving holder's own stay behind, the others' go as they are.
pub fn handed_kept(kept: Option<&Value>, receiver: &str) -> Value {
    let mut out = Vec::new();
    for k in kept.and_then(Value::as_array).into_iter().flatten() {
        let mut k = k.clone();
        match k.get("by_household").and_then(Value::as_str) {
            Some(SELF) | None => continue,
            Some(h) if h == receiver => k["by_household"] = json!(SELF),
            _ => {}
        }
        out.push(k);
    }
    json!(out)
}

fn chars(text: &str, most: usize) -> String {
    text.chars().take(most).collect()
}

fn write_kept(config: &Config, change: impl FnOnce(&mut Vec<Dict>) -> Result<(), Refused>) -> Result<(), Refused> {
    let done = crate::family_agent::change_data(config, |data| {
        let mut all = kept_all(data);
        let out = change(&mut all);
        if out.is_ok() {
            data.insert("kept".into(), json!(all));
        }
        out
    });
    done.map_err(|_| Refused::NotWritten)?
}

/// A search kept (« Garder ») for the household, by the household `by` (`self`: this place's person): the same words
/// once; 6 at most.
pub fn keep(config: &Config, q: &str, title: Option<&str>, category: Option<&str>, by: &str) -> Result<(), Refused> {
    let category = category.map(str::trim).filter(|c| !c.is_empty());
    if category.is_some_and(|x| !c::FAMILY.contains(&x)) {
        return Err(Refused::CategoryUnknown);
    }
    let q = chars(q.trim(), Q_MAX);
    if fold(&q).trim().is_empty() {
        return Err(Refused::KeptEmpty);
    }
    let title = title.map(|t| chars(t.trim(), TITLE_MAX)).filter(|t| !t.is_empty()).unwrap_or_else(|| q.clone());
    if index::reference(&q) || index::reference(&title) {
        return Err(Refused::NumberRefused);
    }
    write_kept(config, |all| {
        let same = |k: &Dict| {
            fold(&tk::str_or_empty(k.get("q"))) == fold(&q)
                && k.get("category").and_then(Value::as_str).filter(|c| !c.is_empty()) == category
        };
        if all.iter().any(same) {
            return Ok(());
        }
        if all.len() >= KEPT_MAX {
            return Err(Refused::KeptFull);
        }
        let k = json!({"id": format!("k{}", py::token_hex(3)), "q": q, "title": title, "category": category,
                       "by_household": by, "at": tk::now_iso_seconds()});
        all.push(k.as_object().cloned().unwrap_or_default());
        Ok(())
    })
}

/// A kept search renamed (an empty title: its words).
pub fn rename_kept(config: &Config, id: &str, title: &str) -> Result<(), Refused> {
    let title = chars(title.trim(), TITLE_MAX);
    if index::reference(&title) {
        return Err(Refused::NumberRefused);
    }
    write_kept(config, |all| {
        let k = all.iter_mut().find(|k| tk::str_or_empty(k.get("id")) == id).ok_or(Refused::KeptUnknown)?;
        let title = if title.is_empty() { tk::str_or_empty(k.get("q")) } else { title };
        k.insert("title".into(), json!(title));
        Ok(())
    })
}

/// A kept search deleted.
pub fn drop_kept(config: &Config, id: &str) -> Result<(), Refused> {
    write_kept(config, |all| {
        let at = all.iter().position(|k| tk::str_or_empty(k.get("id")) == id).ok_or(Refused::KeptUnknown)?;
        all.remove(at);
        Ok(())
    })
}

/// « Oublie ça » on the holder (forget.rs `family`): this place's own person's kept searches whose words or title say
/// it (the others' are theirs, as their lines; a member's go by her own bridge: `forget_at_holder`); how many left.
pub fn forget_kept(data: &mut Dict, gone: &dyn Fn(&str) -> bool) -> usize {
    let all = kept_all(data);
    let mine = |k: &Dict| kept_by(k) == SELF;
    let left: Vec<Dict> = all
        .iter()
        .filter(|k| !(mine(k) && (gone(&tk::str_or_empty(k.get("q"))) || gone(&tk::str_or_empty(k.get("title"))))))
        .cloned()
        .collect();
    let n = all.len() - left.len();
    if n > 0 {
        data.insert("kept".into(), json!(left));
    }
    n
}

// -- searches made: each person's own (<data>/family-searched.json) -------------------------------------------------------

static SEARCHED: Mutex<()> = Mutex::new(());

/// Where this person's searches made in the family Carnet are kept: her own place, never another member's.
pub fn searched_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("family-searched.json")
}

fn read_searched(config: &Config) -> Map<String, Value> {
    match tk::read_text(&searched_path(config)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn write_searched(config: &Config, all: Map<String, Value>) -> std::io::Result<()> {
    let path = searched_path(config);
    if all.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let temporary = path.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), Value::Object(all).to_string().as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &path)
}

/// « Cherché ici » of `category`: this person's latest searches there, the newest first.
pub fn searched_in(config: &Config, category: &str) -> Vec<Value> {
    read_searched(config).get(category).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Whether a search may be kept at all: never a single letter, a number or a reference.
pub fn may_keep(q: &str) -> bool {
    fold(q).trim().chars().count() >= 2 && !index::reference(q)
}

/// A search made, kept under `category` (3 a category, the newest first, the same words once). Whether it was kept.
pub fn searched_add(config: &Config, q: &str, category: &str) -> bool {
    let q = chars(q.trim(), Q_MAX);
    if !may_keep(&q) || !c::FAMILY.contains(&category) {
        return false;
    }
    let _guard = SEARCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_searched(config);
    let mut list: Vec<Value> = all.get(category).and_then(Value::as_array).cloned().unwrap_or_default();
    let folded = fold(&q);
    list.retain(|s| fold(s["q"].as_str().unwrap_or_default()) != folded);
    list.insert(0, json!({"q": q, "at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()}));
    list.truncate(SEARCHED_KEPT);
    all.insert(category.to_string(), json!(list));
    write_searched(config, all).is_ok()
}

/// Searches made dropped by the person: the one of these words, else all; in `category`, else every one.
pub fn searched_drop(config: &Config, q: Option<&str>, category: Option<&str>) -> std::io::Result<usize> {
    let folded = q.map(fold);
    let _guard = SEARCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_searched(config);
    let mut n = 0;
    for (cat, list) in all.iter_mut() {
        if category.is_some_and(|c| c != cat) {
            continue;
        }
        if let Value::Array(items) = list {
            let before = items.len();
            items.retain(|i| folded.as_ref().is_some_and(|f| fold(i["q"].as_str().unwrap_or_default()) != *f));
            n += before - items.len();
        }
    }
    all.retain(|_, l| l.as_array().is_some_and(|l| !l.is_empty()));
    write_searched(config, all)?;
    Ok(n)
}

/// « Oublie ça » (forget.rs `apply`, on any place): this person's searches made in the family Carnet that say it.
pub fn forget_searched(config: &Config, gone: &dyn Fn(&str) -> bool) -> std::io::Result<usize> {
    let _guard = SEARCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_searched(config);
    let mut n = 0;
    for list in all.values_mut() {
        if let Value::Array(items) = list {
            let before = items.len();
            items.retain(|s| !gone(s["q"].as_str().unwrap_or_default()));
            n += before - items.len();
        }
    }
    if n > 0 {
        all.retain(|_, l| l.as_array().is_some_and(|l| !l.is_empty()));
        write_searched(config, all)?;
    }
    Ok(n)
}

/// The person has no family Carnet any more: her searches made in it go.
pub fn erase_searched(config: &Config) {
    let _guard = SEARCHED.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = std::fs::remove_file(searched_path(config)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = %e, "family Carnet: the searches made not erased");
        }
    }
}

/// The asking person's own searches in a sommaire: a category's grey words are her 3 latest searches there.
pub fn with_own_hints(config: &Config, summary: &mut Value) {
    let mine = read_searched(config);
    if let Some(Value::Array(lines)) = summary.get_mut("categories") {
        for line in lines.iter_mut() {
            let qs: Vec<String> = mine
                .get(&str_of(line, "id"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| s["q"].as_str().map(str::to_string))
                .take(HINTS)
                .collect();
            if !qs.is_empty() {
                line["hint"] = json!(qs.join(", "));
            }
        }
    }
}

// -- « Ranger ailleurs » (the person's correction) ---------------------------------------------------------------------

/// What a correction names.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Page(String),
    List(String),
    Item(String, String),
}

impl Target {
    pub fn of(body: &Dict) -> Option<Target> {
        let s = |k: &str| tk::str_or_empty(body.get(k)).trim().to_string();
        let (page, list, item) = (s("page"), s("list"), s("item"));
        if !page.is_empty() {
            return Some(Target::Page(page.strip_prefix("fam:page:").unwrap_or(&page).to_string()));
        }
        if let Some(rest) = item.strip_prefix("fam:list:") {
            let (l, i) = rest.split_once('/')?;
            return Some(Target::Item(l.to_string(), i.to_string()));
        }
        if let Some(rest) = item.strip_prefix("fam:page:") {
            return Some(Target::Page(rest.to_string()));
        }
        match (list.is_empty(), item.is_empty()) {
            (false, false) => Some(Target::Item(list, item)),
            (false, true) => Some(Target::List(list)),
            _ => None,
        }
    }

    fn view(&self) -> Value {
        match self {
            Target::Page(p) => json!({"page": p}),
            Target::List(l) => json!({"list": l}),
            Target::Item(l, i) => json!({"list": l, "item": i}),
        }
    }
}

fn checked(given: &[String]) -> Result<Vec<String>, Refused> {
    let given: Vec<String> = given.iter().map(|c| c.trim().to_lowercase()).collect();
    if given.iter().any(|c| !c::FAMILY.contains(&c.as_str())) {
        return Err(Refused::CategoryUnknown);
    }
    if given.iter().any(|c| !FAMILY_MAY_GIVE.contains(&c.as_str())) {
        return Err(Refused::CategoryNotGiven);
    }
    valid(&given).ok_or(Refused::CategoryCount)
}

/// The note a target names in `data` (mutable), its list when an item, and what it has now (categories, by, the
/// word that decided, whether a correction of it may teach).
/// A note found for a correction: itself, its categories now, by whom, the word that decided, whether correcting it
/// may teach.
type FoundNote<'a> = (&'a mut Value, Vec<String>, String, Option<String>, bool);

fn found<'a>(data: &'a mut Dict, target: &Target, rules: &[Rule]) -> Option<FoundNote<'a>> {
    match target {
        Target::Page(id) => {
            let pages = data.get_mut("pages")?.as_array_mut()?;
            let page = pages.iter_mut().find(|p| str_of(p, "id") == *id)?;
            let (cats, by, word) = page_categories(page, rules);
            let word = word.or_else(|| decide(&str_of(page, "title"), &str_of(page, "text"), rules).1);
            Some((page, cats, by, word, true))
        }
        Target::List(id) => {
            let lists = data.get_mut("lists")?.as_array_mut()?;
            let list = lists.iter_mut().find(|l| str_of(l, "id") == *id)?;
            let cats = list_categories(list, rules);
            let by = stored(list).map(|(_, b)| b).unwrap_or_else(|| "auto".into());
            let word = decide(&str_of(list, "title"), "", rules).1;
            Some((list, cats, by, word, true))
        }
        Target::Item(lid, iid) => {
            let lists = data.get_mut("lists")?.as_array_mut()?;
            let list = lists.iter_mut().find(|l| str_of(l, "id") == *lid)?;
            let list_view = list.clone();
            let cats = list_categories(&list_view, rules);
            let item = list.get_mut("items")?.as_array_mut()?.iter_mut().find(|i| str_of(i, "id") == *iid)?;
            let (now, by, word) = item_categories(&list_view, &cats, item, rules);
            // an item of a list of things is filed by its list: correcting it teaches nothing
            let teaches = matches!(str_of(&list_view, "kind").as_str(), "tasks" | "dates");
            Some((item, now, by, word, teaches))
        }
    }
}

/// « Ranger ailleurs » on a family note, by the person `by`: her categories (`categories_by: person`), never changed
/// by a write; when the bridge had filed it by a word, that word files the next notes her way (`category_rules`, 200
/// at most, the most recent first). One undo (`category_undo`, the 20 latest).
pub fn categorize(config: &Config, target: &Target, given: &[String], lang: &str) -> Result<Value, Refused> {
    let cats = checked(given)?;
    let out = crate::family_agent::change_data(config, |data| -> Result<Value, Refused> {
        let rules = rules_of(data);
        let (note, before, before_by, word, teaches) = found(data, target, &rules).ok_or(Refused::NotFound)?;
        if matches!(target, Target::List(_)) && matches!(str_of(note, "kind").as_str(), "tasks" | "dates") {
            return Err(Refused::ListNotFiled);
        }
        let had = note.get("categories").cloned();
        let had_by = note.get("categories_by").cloned();
        set(note, &cats, "person");
        let learnt = word.filter(|_| teaches && before_by == "auto" && before != cats);
        let mut rules_now: Vec<Value> =
            data.get("category_rules").and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some(w) = &learnt {
            rules_now.retain(|r| fold(&str_of(r, "word")) != *w);
            rules_now.insert(0, json!({"word": w, "categories": cats}));
            rules_now.truncate(RULES_KEPT);
            data.insert("category_rules".into(), json!(rules_now));
        }
        let undo = format!("u{}", py::token_hex(6));
        let mut undos: Vec<Value> = data.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
        undos.retain(|u| u.get("target") != Some(&target.view()));
        undos.insert(0, json!({"id": undo, "target": target.view(), "had": had, "had_by": had_by, "rule": learnt}));
        undos.truncate(UNDO_KEPT);
        data.insert("category_undo".into(), json!(undos));
        Ok(json!({"target": target.view(), "categories": cats, "categories_by": "person",
                  "before": {"categories": before, "categories_by": before_by}, "undo": undo,
                  "learnt": learnt.map(|w| json!({"word": w, "categories": cats}))}))
    });
    let view = out.map_err(|_| Refused::NotWritten)??;
    Ok(crate::notebook::categories_pass::with_toast(view, lang))
}

/// A correction undone: the note as it was (filed again by the bridge when it had nothing written), the rule it
/// taught forgotten. 404 `undo_gone` once undone, or when the note is gone.
pub fn undo(config: &Config, id: &str) -> Result<Value, Refused> {
    let out = crate::family_agent::change_data(config, |data| -> Result<Value, Refused> {
        let mut undos: Vec<Value> = data.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
        let at = undos.iter().position(|u| str_of(u, "id") == id).ok_or(Refused::UndoGone)?;
        let u = undos.remove(at);
        data.insert("category_undo".into(), json!(undos));
        let target = u["target"].as_object().and_then(Target::of).ok_or(Refused::UndoGone)?;
        if let Some(word) = u.get("rule").and_then(Value::as_str) {
            if let Some(Value::Array(rules)) = data.get_mut("category_rules") {
                rules.retain(|r| str_of(r, "word") != word);
            }
        }
        {
            let rules = rules_of(data);
            let (note, _, _, _, _) = found(data, &target, &rules).ok_or(Refused::UndoGone)?;
            unset(note);
            if let Some(o) = note.as_object_mut() {
                for (k, v) in [("categories", &u["had"]), ("categories_by", &u["had_by"])] {
                    if !v.is_null() {
                        o.insert(k.into(), v.clone());
                    }
                }
            }
        }
        // as at a write: what has nothing written is filed by the bridge
        file_all(data);
        let rules = rules_of(data);
        let (_, now, by, _, _) = found(data, &target, &rules).ok_or(Refused::UndoGone)?;
        Ok(json!({"target": target.view(), "categories": now, "categories_by": by}))
    });
    out.map_err(|_| Refused::NotWritten)?
}

// -- who: the holder, the members --------------------------------------------------------------------------------------

/// The family's members but `caller` (a household id; None: the holder's own person, who is not listed), by name: the
/// holder's person first when a member asks, then the households its family agents are for.
pub fn others(config: &Config, holder_name: &str, caller: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if caller.is_some() && !holder_name.is_empty() {
        names.push(holder_name.to_string());
    }
    let linked = crate::households::Households::new(&crate::households::path_for(config));
    let mut seen: Vec<String> = Vec::new();
    for agent in crate::family_agent::owns(config) {
        for h in crate::agent_shares::shared_with(config, &agent) {
            if caller == Some(h.as_str()) || seen.contains(&h) {
                continue;
            }
            let name = linked
                .get(&h)
                .and_then(|e| e.get("name").map(py::py_str))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| h.clone());
            seen.push(h);
            names.push(name);
        }
    }
    names
}

/// Whether this person uses a family Carnet (her place holds it for at least one other member, or a linked
/// household's): the sommaire's selector shows « Famille ».
pub fn in_family(config: &Config) -> bool {
    let own = crate::family_agent::owns(config);
    if !own.is_empty() {
        return own.iter().any(|a| !crate::agent_shares::shared_with(config, a).is_empty());
    }
    crate::family_agent::uses_a_family_agent(config)
}

/// The person's own sommaire's selector (`Summary.spaces`) until S5's « Perso · Pro »: « Moi · Famille » for a member
/// of a family, else none (the app's own until S6). Visible text: proposed, for Alex.
pub fn spaces(config: &Config, lang: &str, space: &str) -> Value {
    if !in_family(config) {
        return json!([]);
    }
    let (me, family) = match lang {
        "fr" => ("Moi", "Famille"),
        "de" => ("Ich", "Familie"),
        "es" => ("Yo", "Familia"),
        "nl" => ("Ik", "Familie"),
        _ => ("Me", "Family"),
    };
    json!([{"id": "all", "title": me, "selected": space != "pro"}, {"id": "family", "title": family, "selected": false}])
}

/// The family Carnet's language (its agent's, else the place's) and the app's: the copy's tables.
fn langs_of(config: &Config, q: &QueryMap) -> Vec<String> {
    index::langs(&crate::family_agent::carnet_language(config), q.get("lang"))
}

/// The app's language for the texts (`?lang=`, else the family Carnet's).
fn shown_lang(config: &Config, q: &QueryMap) -> String {
    shown_of(&langs_of(config, q))
}

// -- the routes ----------------------------------------------------------------------------------------------------------

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| HttpError::from(anyhow::Error::from(e)))
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn body_of(bytes: &[u8]) -> ApiResult<Dict> {
    if bytes.is_empty() {
        return Ok(Dict::new());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

fn query(q: &QueryMap, keys: &[&str]) -> Vec<(String, String)> {
    keys.iter().filter_map(|k| q.get(k).map(|v| (k.to_string(), v.to_string()))).collect()
}

/// Where the family Carnet is for this person: here (her place holds it), at a linked household's, or nowhere.
enum Holder {
    Here,
    At(String),
    None,
}

fn holder(config: &Config) -> Holder {
    match crate::family_agent::holder_of(config) {
        Some(None) => Holder::Here,
        Some(Some(h)) => Holder::At(h),
        None => Holder::None,
    }
}

/// A call to the holder's place (`/family/fa/notebook…`, its household key): (status, JSON); None when it does not
/// answer.
async fn hop(
    state: &AppState,
    household: &str,
    method: reqwest::Method,
    path: &str,
    query: &[(String, String)],
    body: Option<Value>,
) -> Option<(u16, Value)> {
    let entry = state.households.get(household)?;
    hop_to(state, &entry, method, path, query, body).await
}

/// [`hop`] with the household's entry as it is (or was: a link just removed, `leave_holder`).
async fn hop_to(
    state: &AppState,
    entry: &Dict,
    method: reqwest::Method,
    path: &str,
    query: &[(String, String)],
    body: Option<Value>,
) -> Option<(u16, Value)> {
    let url = format!("{}/family/fa/notebook{path}", crate::family_relay::reach_url(entry));
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let http = crate::door_tls::household_client(&state.http, entry);
    let mut request = http.request(method, url).query(query).timeout(Duration::from_secs(15)).bearer_auth(key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.ok()?;
    let status = resp.status().as_u16();
    Some((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
}

/// What the holder answered, for the app: 404 « this household is in none of its family agents » is `no_family`
/// (she was taken out, or left); another 404 a holder of before S6 (`family_sommaire_unavailable`: the app shows
/// today's family Carnet); no answer 502 as today's family routes.
enum Hopped {
    Answer(u16, Value),
    Out,
    Old,
}

async fn hop_checked(
    state: &AppState,
    household: &str,
    method: reqwest::Method,
    path: &str,
    query: &[(String, String)],
    body: Option<Value>,
) -> ApiResult<Hopped> {
    match hop(state, household, method, path, query, body).await {
        None => Err(HttpError::new(502, "their side does not answer right now: try again later")),
        Some((404, v)) if v.get("detail").and_then(Value::as_str) == Some(crate::family_agent::NOT_SERVED) => {
            Ok(Hopped::Out)
        }
        // the holder removed the link (its handover may never have come): out of the family (review 1 of S6, R1)
        Some((401, v)) if v.get("detail").and_then(Value::as_str) == Some(crate::family_agent::UNKNOWN_HOUSEHOLD) => {
            Ok(Hopped::Out)
        }
        // never a refusal of the holder's to the app as it is (the app would say its own server refused it)
        Some((401 | 403, _)) => Err(HttpError::new(502, "their side does not answer right now: try again later")),
        Some((404, v)) if !v.get("detail").is_some_and(Value::is_object) => Ok(Hopped::Old),
        Some((status, v)) => Ok(Hopped::Answer(status, v)),
    }
}

/// This household's own kept searches on the holder's place (`mine`), those `take` wants deleted there; whether the
/// holder answered every call. Only ids and the kept searches' own words travel: never what the person forgets.
async fn drop_own_kept(state: &AppState, entry: &Dict, take: &(dyn Fn(&str, &str) -> bool + Sync)) -> bool {
    let asked = vec![("mine".to_string(), "1".to_string())];
    let Some((200, list)) = hop_to(state, entry, reqwest::Method::GET, "/kept", &asked, None).await else {
        return false;
    };
    let mut reached = true;
    for k in list["kept"].as_array().into_iter().flatten() {
        if k.get("mine").and_then(Value::as_bool) != Some(true) || !take(&str_of(k, "q"), &str_of(k, "title")) {
            continue;
        }
        let id = vec![("id".to_string(), str_of(k, "id"))];
        reached &= hop_to(state, entry, reqwest::Method::DELETE, "/kept", &id, None).await.is_some();
    }
    reached
}

/// « Oublie ça » confirmed on a member's place (notebook api.rs `forget_confirm`; review 1 of S6, B1): her own kept
/// searches on the holder's place that say it go (matched here, by `says`: the forgotten words stay on her place).
/// Whether the holder was reached (false: the app says it, nothing is kept to try again). Her lines there: not reached
/// (the holder applies its own « Oublie ça »; relaying the words is Alex's decision, status-carnet-s6.md).
pub async fn forget_at_holder(state: &AppState, says: &(dyn Fn(&str) -> bool + Sync)) -> bool {
    let Holder::At(household) = holder(&state.config()) else { return true };
    let Some(entry) = state.households.get(&household) else { return false };
    drop_own_kept(state, &entry, &|q, title| says(q) || says(title)).await
}

/// A member removed the link to the household holding her family Carnet (family_agent.rs `after_unlink`; review 1 of
/// S6, R3): her bridge takes her kept searches off that place as she goes (best effort: the holder also drops them
/// when it sees her gone).
pub fn leave_holder(state: &AppState, entry: Dict) {
    let state2 = state.clone();
    state.spawn(async move {
        let done = drop_own_kept(&state2, &entry, &|_, _| true).await;
        tracing::info!(done = done, "family Carnet: her kept searches taken off the place she left");
    });
}

/// A category of the path that is not the family's: 404 (S4's rule for `/category/{id}`).
fn unknown_category() -> HttpError {
    HttpError::json(404, json!({"code": Refused::CategoryUnknown.code(), "text": ""}))
}

fn old_holder() -> HttpError {
    HttpError::json(404, json!({"code": "family_sommaire_unavailable", "text": ""}))
}

/// No family Carnet for this person (any more): her searches made in it go, unless a handover waits for her.
fn no_family(config: &Config, lang: &str) -> HttpError {
    if !crate::family_agent::handover_waits(config) {
        erase_searched(config);
    }
    Refused::NoFamily.error(lang)
}

fn answered(status: u16, v: Value) -> Response {
    web::json_response(status, &v)
}

/// `GET /v1/family-agent/notebook/summary`.
async fn summary(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let mut view = match holder(&config) {
        Holder::None => return Err(no_family(&config, &shown)),
        Holder::Here => {
            let langs = langs_of(&config, &q);
            let name = state.households.name();
            let c2 = config.clone();
            blocking(move || summary_at(&c2, &langs, today(), &others(&c2, &name, None))).await?
        }
        Holder::At(h) => {
            match hop_checked(&state, &h, reqwest::Method::GET, "/summary", &query(&q, &["lang"]), None).await? {
                Hopped::Answer(200, v) => v,
                Hopped::Answer(status, v) => return Ok(answered(status, v)),
                Hopped::Out => return Err(no_family(&config, &shown)),
                Hopped::Old => return Err(old_holder()),
            }
        }
    };
    with_own_hints(&config, &mut view);
    Ok(web::ok(view))
}

/// `GET /v1/family-agent/notebook/category/{id}`.
async fn category(State(state): State<AppState>, UrlPath(id): UrlPath<String>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    if !c::FAMILY.contains(&id.as_str()) {
        return Err(unknown_category());
    }
    let mut view = match holder(&config) {
        Holder::None => return Err(no_family(&config, &shown)),
        Holder::Here => {
            let langs = langs_of(&config, &q);
            let (c2, id2) = (config.clone(), id.clone());
            blocking(move || category_at(&c2, &id2, &langs, today())).await?.map_err(|r| r.error(&shown))?
        }
        Holder::At(h) => {
            match hop_checked(&state, &h, reqwest::Method::GET, &format!("/category/{id}"), &query(&q, &["lang"]), None)
                .await?
            {
                Hopped::Answer(200, v) => v,
                Hopped::Answer(status, v) => return Ok(answered(status, v)),
                Hopped::Out => return Err(no_family(&config, &shown)),
                Hopped::Old => return Err(old_holder()),
            }
        }
    };
    view["searched"] = json!(searched_in(&config, &id));
    Ok(web::ok(view))
}

/// `GET /v1/family-agent/notebook/search?q=&category=`.
async fn search(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let words = web::query_str(&q, "q", true, Some(Q_MAX))?.unwrap_or_default();
    let category = web::query_str(&q, "category", false, Some(20))?.filter(|c| !c.is_empty());
    match holder(&config) {
        Holder::None => Err(no_family(&config, &shown)),
        Holder::Here => {
            let langs = langs_of(&config, &q);
            let v = blocking(move || search_at(&config, &words, category.as_deref(), &langs, today())).await?;
            v.map(web::ok).map_err(|r| r.error(&shown))
        }
        Holder::At(h) => {
            match hop_checked(&state, &h, reqwest::Method::GET, "/search", &query(&q, &["q", "category", "lang"]), None)
                .await?
            {
                Hopped::Answer(status, v) => Ok(answered(status, v)),
                Hopped::Out => Err(no_family(&config, &shown)),
                Hopped::Old => Err(old_holder()),
            }
        }
    }
}

fn since_of(q: &QueryMap) -> ApiResult<Option<u64>> {
    match web::query_str(q, "since", false, Some(20))? {
        Some(s) => s
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| HttpError::json(400, json!({"code": "since_invalid", "text": ""}))),
        None => Ok(None),
    }
}

/// The copy as the phone gets it: 304 when it has it, gzip when it accepts it, the ETag.
fn index_response(body: Option<Value>, etag: &str, headers: &HeaderMap) -> ApiResult<Response> {
    let gzip = headers
        .get("accept-encoding")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().split(';').next() == Some("gzip")));
    let mut response = match body {
        None => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = axum::http::StatusCode::NOT_MODIFIED;
            r
        }
        Some(body) if gzip => {
            use std::io::Write as _;
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&serde_json::to_vec(&body).map_err(anyhow::Error::from)?).map_err(anyhow::Error::from)?;
            let mut r = Response::new(Body::from(e.finish().map_err(anyhow::Error::from)?));
            r.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
            r.headers_mut().insert("content-encoding", HeaderValue::from_static("gzip"));
            r
        }
        Some(body) => web::ok(body),
    };
    if let Ok(v) = HeaderValue::from_str(etag) {
        response.headers_mut().insert("etag", v);
    }
    response.headers_mut().insert("vary", HeaderValue::from_static("accept-encoding"));
    Ok(response)
}

/// The copy here: (None: the phone has it; else the answer) and its ETag.
fn index_here(
    config: &Config,
    langs: &[String],
    since: Option<u64>,
    epoch: Option<&str>,
    has: Option<&str>,
) -> (Option<Value>, String) {
    let now = etag(config, langs);
    if has == Some(now.as_str()) {
        return (None, now);
    }
    let (body, etag) = index_answer(config, langs, since, epoch);
    (Some(body), etag)
}

/// `GET /v1/family-agent/notebook/index?since=&epoch=&lang=`.
async fn index_route(State(state): State<AppState>, q: QueryMap, headers: HeaderMap) -> ApiResult<Response> {
    let config = state.config();
    let since = since_of(&q)?;
    let epoch = web::query_str(&q, "epoch", false, Some(40))?;
    let has = headers.get("if-none-match").and_then(|v| v.to_str().ok()).map(|v| v.trim().to_string());
    let langs = langs_of(&config, &q);
    let left = |config: &Config, langs: &[String]| -> ApiResult<Response> {
        if !crate::family_agent::handover_waits(config) {
            erase_searched(config);
        }
        let tag = format!("\"{LEFT}\"");
        if has.as_deref() == Some(tag.as_str()) {
            return index_response(None, &tag, &headers);
        }
        index_response(Some(left_answer(langs)), &tag, &headers)
    };
    match holder(&config) {
        Holder::None => left(&config, &langs),
        Holder::Here => {
            let (c2, l2, has2) = (config.clone(), langs.clone(), has.clone());
            let (body, etag) = blocking(move || index_here(&c2, &l2, since, epoch.as_deref(), has2.as_deref())).await?;
            index_response(body, &etag, &headers)
        }
        Holder::At(h) => {
            let mut asked = query(&q, &["since", "epoch", "lang"]);
            if let Some(has) = &has {
                asked.push(("has".into(), has.clone()));
            }
            match hop_checked(&state, &h, reqwest::Method::GET, "/index", &asked, None).await? {
                Hopped::Answer(200, v) => {
                    let etag = str_of(&v, "etag");
                    let body = v.get("index").cloned().filter(|i| !i.is_null());
                    index_response(body, &etag, &headers)
                }
                Hopped::Answer(status, v) => Ok(answered(status, v)),
                Hopped::Out => left(&config, &langs),
                Hopped::Old => Err(old_holder()),
            }
        }
    }
}

/// `GET /v1/family-agent/notebook/kept`.
async fn kept_list(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    kept_relayed(state, q, reqwest::Method::GET, None).await
}

async fn kept_change(State(state): State<AppState>, q: QueryMap, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    kept_relayed(state, q, reqwest::Method::POST, Some(body)).await
}

async fn kept_drop(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    kept_relayed(state, q, reqwest::Method::DELETE, None).await
}

/// One change of the kept searches (`body`: POST; `?id=`: DELETE) by the household `by`, then the list (`mine` for
/// `caller`, between the bridges, when asked).
fn kept_here(
    config: &Config,
    method: &reqwest::Method,
    body: Option<&Dict>,
    id: Option<&str>,
    by: &str,
    langs: &[String],
    caller: Option<&str>,
) -> Result<Value, Refused> {
    match (method.as_str(), body) {
        ("POST", Some(b)) => {
            let s = |k: &str| b.get(k).and_then(Value::as_str).map(str::to_string);
            match (s("id"), s("q")) {
                (Some(id), _) => rename_kept(config, &id, s("title").as_deref().unwrap_or(""))?,
                (None, Some(words)) => keep(config, &words, s("title").as_deref(), s("category").as_deref(), by)?,
                (None, None) => return Err(Refused::KeptEmpty),
            }
        }
        ("DELETE", _) => drop_kept(config, id.unwrap_or(""))?,
        _ => {}
    }
    Ok(kept_for(config, langs, today(), caller))
}

async fn kept_relayed(
    state: AppState,
    q: QueryMap,
    method: reqwest::Method,
    body: Option<Dict>,
) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let id = web::query_str(&q, "id", method == reqwest::Method::DELETE, Some(20))?;
    match holder(&config) {
        Holder::None => Err(no_family(&config, &shown)),
        Holder::Here => {
            let langs = langs_of(&config, &q);
            let v =
                blocking(move || kept_here(&config, &method, body.as_ref(), id.as_deref(), SELF, &langs, None)).await?;
            v.map(web::ok).map_err(|r| r.error(&shown))
        }
        Holder::At(h) => {
            // (who keeps is the household the holder knows by its key: no name is sent)
            match hop_checked(&state, &h, method, "/kept", &query(&q, &["id", "lang"]), body.map(Value::Object)).await?
            {
                Hopped::Answer(status, v) => Ok(answered(status, v)),
                Hopped::Out => Err(no_family(&config, &shown)),
                Hopped::Old => Err(old_holder()),
            }
        }
    }
}

/// `POST /v1/family-agent/notebook/searched {q, category?, private?}` → 204: kept by this person's own bridge.
async fn searched(State(state): State<AppState>, q: QueryMap, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let body = body_of(&body)?;
    let words: String = tk::str_or_empty(body.get("q")).chars().take(Q_MAX).collect();
    let category = Some(tk::str_or_empty(body.get("category")).trim().to_string()).filter(|c| !c.is_empty());
    if category.as_deref().is_some_and(|x| !c::FAMILY.contains(&x)) {
        return Err(Refused::CategoryUnknown.error(&shown));
    }
    // out of the family: nothing kept, even with its category (review 1 of S6, R6a)
    let here = match holder(&config) {
        Holder::None => return Err(no_family(&config, &shown)),
        Holder::Here => true,
        Holder::At(_) => false,
    };
    if body.get("private").is_some_and(py::truthy) || !may_keep(&words) {
        return Ok(web::no_content());
    }
    // without a category: under its best result on the holder's own place; on a member's, kept nowhere and asked
    // of nobody (review 1 of S6, R2: her words never leave her place; the app sends its best group's category)
    let category = match category {
        Some(c) => Some(c),
        None if here => {
            let (c2, w2, langs) = (config.clone(), words.clone(), langs_of(&config, &q));
            blocking(move || best_category(&c2, &w2, &langs, today())).await?
        }
        None => None,
    };
    if let Some(category) = category {
        blocking(move || searched_add(&config, &words, &category)).await?;
    }
    Ok(web::no_content())
}

/// `DELETE /v1/family-agent/notebook/searched?q=&category=` → 204.
async fn searched_dropped(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let words = web::query_str(&q, "q", false, Some(Q_MAX))?.filter(|w| !w.trim().is_empty());
    let category = web::query_str(&q, "category", false, Some(20))?.filter(|c| !c.trim().is_empty());
    if category.as_deref().is_some_and(|x| !c::FAMILY.contains(&x)) {
        return Err(Refused::CategoryUnknown.error(&shown));
    }
    blocking(move || searched_drop(&config, words.as_deref(), category.as_deref()))
        .await?
        .map_err(anyhow::Error::from)?;
    Ok(web::no_content())
}

/// `POST /v1/family-agent/notebook/categorize` and `/categorize/undo`.
async fn categorize_route(State(state): State<AppState>, q: QueryMap, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    correction(state, q, "/categorize", body).await
}

async fn undo_route(State(state): State<AppState>, q: QueryMap, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    correction(state, q, "/categorize/undo", body).await
}

fn correction_here(config: &Config, path: &str, body: &Dict, lang: &str) -> Result<Value, Refused> {
    if path == "/categorize/undo" {
        return undo(config, &tk::str_or_empty(body.get("undo")));
    }
    let target = Target::of(body).ok_or(Refused::NotFound)?;
    categorize(config, &target, &strings(body.get("categories")), lang)
}

async fn correction(state: AppState, q: QueryMap, path: &'static str, body: Dict) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_lang(&config, &q);
    match holder(&config) {
        Holder::None => Err(no_family(&config, &shown)),
        Holder::Here => {
            let s2 = shown.clone();
            let v = blocking(move || correction_here(&config, path, &body, &s2)).await?;
            v.map(web::ok).map_err(|r| r.error(&shown))
        }
        Holder::At(h) => {
            match hop_checked(&state, &h, reqwest::Method::POST, path, &query(&q, &["lang"]), Some(Value::Object(body)))
                .await?
            {
                Hopped::Answer(status, v) => Ok(answered(status, v)),
                Hopped::Out => Err(no_family(&config, &shown)),
                Hopped::Old => Err(old_holder()),
            }
        }
    }
}

/// Every answer of the family Carnet's sommaire is kept by no HTTP cache (sommaire.md §0).
async fn stored_nowhere(response: Response) -> Response {
    crate::notebook::api::no_store(response)
}

/// For the apps (mounted under /v1 with family_agent.rs's, bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/family-agent/notebook/summary", get(summary))
        .route("/family-agent/notebook/category/{id}", get(category))
        .route("/family-agent/notebook/search", get(search))
        .route("/family-agent/notebook/index", get(index_route))
        .route("/family-agent/notebook/kept", get(kept_list).post(kept_change).delete(kept_drop))
        .route("/family-agent/notebook/searched", post(searched).delete(searched_dropped))
        .route("/family-agent/notebook/categorize", post(categorize_route))
        .route("/family-agent/notebook/categorize/undo", post(undo_route))
        .layer(axum::middleware::map_response(stored_nowhere))
}

// -- between the bridges: the holder answers its members --------------------------------------------------------------

/// The calling member (a household in one of this place's family agents), else 404 as today's family routes.
fn caller(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    crate::family_agent::served_household(state, headers)
}

async fn served_summary(State(state): State<AppState>, headers: HeaderMap, q: QueryMap) -> ApiResult<Response> {
    let household = caller(&state, &headers)?;
    let config = state.config();
    let langs = langs_of(&config, &q);
    let name = state.households.name();
    Ok(web::ok(
        blocking(move || summary_at(&config, &langs, today(), &others(&config, &name, Some(&household)))).await?,
    ))
}

async fn served_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    q: QueryMap,
) -> ApiResult<Response> {
    caller(&state, &headers)?;
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let langs = langs_of(&config, &q);
    blocking(move || category_at(&config, &id, &langs, today())).await?.map(web::ok).map_err(|r| match r {
        Refused::CategoryUnknown => unknown_category(),
        r => r.error(&shown),
    })
}

async fn served_search(State(state): State<AppState>, headers: HeaderMap, q: QueryMap) -> ApiResult<Response> {
    caller(&state, &headers)?;
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let words = web::query_str(&q, "q", true, Some(Q_MAX))?.unwrap_or_default();
    let category = web::query_str(&q, "category", false, Some(20))?.filter(|c| !c.is_empty());
    let langs = langs_of(&config, &q);
    blocking(move || search_at(&config, &words, category.as_deref(), &langs, today()))
        .await?
        .map(web::ok)
        .map_err(|r| r.error(&shown))
}

/// `GET /family/fa/notebook/index?since=&epoch=&lang=&has=`: `{etag, index}` (the index null when `has` is it).
async fn served_index(State(state): State<AppState>, headers: HeaderMap, q: QueryMap) -> ApiResult<Response> {
    caller(&state, &headers)?;
    let config = state.config();
    let since = since_of(&q)?;
    let epoch = web::query_str(&q, "epoch", false, Some(40))?;
    let has = web::query_str(&q, "has", false, Some(120))?;
    let langs = langs_of(&config, &q);
    let (body, etag) = blocking(move || index_here(&config, &langs, since, epoch.as_deref(), has.as_deref())).await?;
    Ok(web::ok(json!({"etag": etag, "index": body})))
}

async fn served_kept(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: QueryMap,
    method: axum::http::Method,
    body: Bytes,
) -> ApiResult<Response> {
    let household = caller(&state, &headers)?;
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let body = body_of(&body)?;
    let id = web::query_str(&q, "id", false, Some(20))?;
    let langs = langs_of(&config, &q);
    let method = reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    // `mine=1`: which ones this household kept (its « Oublie ça », its leaving: matched on its own place)
    let mine = matches!(q.get("mine"), Some("1")).then(|| household.clone());
    let v =
        blocking(move || kept_here(&config, &method, Some(&body), id.as_deref(), &household, &langs, mine.as_deref()))
            .await?;
    v.map(web::ok).map_err(|r| r.error(&shown))
}

async fn served_correction(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: QueryMap,
    uri: axum::http::Uri,
    body: Bytes,
) -> ApiResult<Response> {
    caller(&state, &headers)?;
    let config = state.config();
    let shown = shown_lang(&config, &q);
    let body = body_of(&body)?;
    let path = if uri.path().ends_with("/undo") { "/categorize/undo" } else { "/categorize" };
    let s2 = shown.clone();
    blocking(move || correction_here(&config, path, &body, &s2)).await?.map(web::ok).map_err(|r| r.error(&shown))
}

/// What a member's bridge may call (its household key). Mounted at the root with family_agent.rs's.
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/fa/notebook/summary", get(served_summary))
        .route("/family/fa/notebook/category/{id}", get(served_category))
        .route("/family/fa/notebook/search", get(served_search))
        .route("/family/fa/notebook/index", get(served_index))
        .route("/family/fa/notebook/kept", get(served_kept).post(served_kept).delete(served_kept))
        .route("/family/fa/notebook/categorize", post(served_correction))
        .route("/family/fa/notebook/categorize/undo", post(served_correction))
}

#[cfg(test)]
#[path = "family_carnet_tests.rs"]
mod tests;
