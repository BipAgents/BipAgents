//! The sommaire ready to show (plan « Le sommaire » §1, §3.5, §3.6, brick S4; contracts/carnet/sommaire.md §3,
//! sommaire.json `Summary`, `CategoryPage`, `KeptList`): what the apps show, the bridge counting everything.
//!
//!     GET    /v1/notebook/summary?space=&lang=                       the sommaire (`Summary`)
//!     GET    /v1/notebook/category/{id}?space=&filter=&lang=&private= a category's page (`CategoryPage`)
//!     GET    /v1/notebook/kept?space=                                the kept searches (`KeptList`)
//!     POST   /v1/notebook/kept                                       {q, title?, space?, category?} | {id, title}
//!     DELETE /v1/notebook/kept?id=
//!     POST   /v1/notebook/searched                                   {q, category?, space?} → 204
//!
//! Everything is counted on the phone's copy (index.rs; Santé is never in it): a category's `count` is the pages its
//! screen lists (structure.rs, Alex 26/11: the people in Personnes, the subjects elsewhere, never a line), `total` each
//! note once. Santé's line is locked: no count, no word; its page is read behind the app's lock only (`private=1`),
//! kept nowhere.
//!
//! - **Kept searches** (`state.json` `kept_searches`): the words as typed (a date phrase too: « à payer ce mois-ci »,
//!   counted again each day), a title the person may change, 6 a space; never a number or a reference (the same rule
//!   as the copy), never in Santé. They go with the phone's copy (`kept`, whole, its version moving with them).
//! - **Searches made** (`searched.json`, beside `state.json`, never read by anything an agent reaches, never in the
//!   copy: the phone's copy is not made again for a search): 3 a category and a space, the newest first, the same
//!   words once; without a category, under the category of the search's best result (nothing found: not kept);
//!   never Santé, never a number or a reference, never a single letter. They are « Cherché ici » on a category's
//!   page and the sommaire's grey words.
//! - **« Oublie ça »** (forget.rs `apply`) takes with it the kept searches and the searches made whose words (or a kept
//!   one's title) are what it forgets, in every space: the rule of an agent's memory line (`Target::hit`, folded, whole
//!   words; a person's first name too when their page is forgotten whole).
//! - **Spaces** (`all`, `perso`, `pro`; S5, spaces.rs): a note's space is its page's (`space: pro`) or the person's
//!   mark; `perso` is what is not Pro. The selector (`spaces`): the setting « Séparer Perso, Pro et Famille » on,
//!   Perso then Pro (S6 adds Famille); off, none. A view of the kept searches: `all` every space's, `perso` Perso's
//!   and those kept with the setting off (`all`), `pro` Pro's; 6 a view. The searches made: each space its own.
//!   Santé is in no space: its line is in `all` and `perso`, its page ignores the space asked.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::categories as c;
use super::finder::Finder;
use super::{fold, lexicon, Part};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// The contract's version of the answers (`Summary.v`, `CategoryPage.v`).
pub const V: u64 = 1;
/// Kept searches a space (plan §3.5).
pub const KEPT_MAX: usize = 6;
/// Searches made kept a category (« Cherché ici »).
pub const SEARCHED_KEPT: usize = 3;
/// A category page's latest notes.
pub const RECENT: usize = 20;
/// The grey words of a category's line.
pub const HINTS: usize = 3;
/// The spaces a request may ask (sommaire.json `SpaceAsked`).
pub const SPACES: [&str; 3] = ["all", "perso", "pro"];
/// A kept search's words and its title, at most (characters).
pub const Q_MAX: usize = 200;
pub const TITLE_MAX: usize = 80;
const KEPT_KEY: &str = "kept_searches";
/// Each category's sub-filters (sommaire.json `categories.filters`), in their order: only those that mean something
/// for a list of pages (Alex 26/11: Papiers' Factures, Contrats, Garanties; the others' are gone with the lines).
pub const FILTERS: [(&str, &[&str]); 1] = [("papers", &["invoices", "contracts", "warranties"])];

/// The sub-filters' names (contracts/carnet/sommaire.json `titles_proposed`, the same table: checked by
/// `the_sub_filter_titles_are_the_contracts`) in fr, en, de, es, nl. Visible text: proposed, for Alex.
const FILTER_TITLES: [(&str, [&str; 5]); 3] = [
    ("invoices", ["Factures", "Invoices", "Rechnungen", "Facturas", "Facturen"]),
    ("contracts", ["Contrats", "Contracts", "Verträge", "Contratos", "Contracten"]),
    ("warranties", ["Garanties", "Warranties", "Garantien", "Garantías", "Garanties"]),
];

fn column(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        "nl" => 4,
        _ => 1,
    }
}

/// A sub-filter's name in `lang` (English for another language).
pub fn filter_title(lang: &str, id: &str) -> String {
    FILTER_TITLES.iter().find(|(f, _)| *f == id).map(|(_, t)| t[column(lang)].to_string()).unwrap_or_else(|| id.to_string())
}

/// The sommaire's own texts (visible text: the plan's in French, the others proposed, for Alex).
struct Texts {
    carnet: &'static str,
    all_notes: &'static str,
    bar_all: &'static str,
    /// « {category} »: the category's name.
    bar_in: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { carnet: "Carnet", all_notes: "Tout le Carnet", bar_all: "Cherche dans tout le Carnet", bar_in: "Cherche dans {category}" },
        "de" => Texts { carnet: "Notizbuch", all_notes: "Das ganze Notizbuch", bar_all: "Im ganzen Notizbuch suchen", bar_in: "In {category} suchen" },
        "es" => Texts { carnet: "Cuaderno", all_notes: "Todo el Cuaderno", bar_all: "Buscar en todo el Cuaderno", bar_in: "Buscar en {category}" },
        "nl" => Texts { carnet: "Notitieboek", all_notes: "Het hele notitieboek", bar_all: "Zoeken in het hele notitieboek", bar_in: "Zoeken in {category}" },
        _ => Texts { carnet: "Notebook", all_notes: "The whole Notebook", bar_all: "Search the whole Notebook", bar_in: "Search in {category}" },
    }
}

/// Why a request is refused (contracts/carnet/sommaire.json `routes[].errors`; no text yet: the apps show their own
/// until Alex approves the texts, as S1's).
#[derive(Debug, PartialEq)]
pub enum Refused {
    SpaceUnknown,
    CategoryUnknown,
    FilterUnknown,
    HealthLocked,
    CategoryNotGiven,
    KeptFull,
    KeptUnknown,
    KeptEmpty,
    NumberRefused,
    /// `state.json` could not be written.
    NotWritten,
}

impl Refused {
    pub fn code(&self) -> &'static str {
        match self {
            Refused::SpaceUnknown => "space_unknown",
            Refused::CategoryUnknown => "category_unknown",
            Refused::FilterUnknown => "filter_unknown",
            Refused::HealthLocked => "health_locked",
            Refused::CategoryNotGiven => "category_not_given",
            Refused::KeptFull => "kept_full",
            Refused::KeptUnknown => "kept_unknown",
            Refused::KeptEmpty => "kept_empty",
            Refused::NumberRefused => "number_refused",
            Refused::NotWritten => "not_written",
        }
    }

    /// Its HTTP status (a category of the path that does not exist: 404, by the route).
    pub fn status(&self) -> u16 {
        match self {
            Refused::KeptUnknown => 404,
            Refused::NotWritten => 500,
            Refused::HealthLocked => 403,
            Refused::KeptFull => 409,
            _ => 400,
        }
    }
}

/// The space asked (`?space=`, a body's `space`): none or empty is `all`.
pub fn space_asked(asked: Option<&str>) -> Result<&'static str, Refused> {
    match asked.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok("all"),
        Some(s) => SPACES.iter().find(|x| **x == s).copied().ok_or(Refused::SpaceUnknown),
    }
}

fn in_space(f: &Finder, at: usize, space: &str) -> bool {
    space == "all" || f.items[at].space() == space
}

fn narrowed(space: &str) -> Option<&str> {
    (space != "all").then_some(space)
}

fn titles_lang(langs: &[String]) -> String {
    langs.last().cloned().unwrap_or_else(|| "en".into())
}

// -- the notes of each category ---------------------------------------------------------------------------------------

/// The copy's notes of `space` by category (`dates` as of `today`, each note once a category), and how many in all.
fn by_category(f: &Finder, space: &str, today: NaiveDate) -> (HashMap<String, Vec<usize>>, usize) {
    let mut out: HashMap<String, Vec<usize>> = HashMap::new();
    let mut total = 0;
    for at in 0..f.items.len() {
        if !in_space(f, at, space) {
            continue;
        }
        total += 1;
        let mut seen: Vec<String> = Vec::new();
        for cat in f.categories(at, today) {
            if !seen.contains(&cat) {
                out.entry(cat.clone()).or_default().push(at);
                seen.push(cat);
            }
        }
    }
    (out, total)
}

/// `ats` the latest first: the newest noted first (a page has no day: after, in the copy's order); in `dates`, what
/// is still to come first (the soonest first), then the past (the latest first).
fn latest(f: &Finder, ats: &[usize], category: &str, today: NaiveDate) -> Vec<usize> {
    let mut out = ats.to_vec();
    if category == "dates" {
        out.sort_by_key(|at| match f.day(*at, today) {
            Some(d) if d >= today => (0, (d - today).num_days(), *at),
            Some(d) => (1, (today - d).num_days(), *at),
            None => (2, 0, *at),
        });
    } else {
        out.sort_by(|a, b| f.items[*b].noted.cmp(&f.items[*a].noted).then(a.cmp(b)));
    }
    out
}

/// The grey words: the category's latest searches, else the titles of its pages last written in (Dates: the soonest),
/// 3 at most.
fn hint(made: &serde_json::Map<String, Value>, rows: &[super::structure::Listed], space: &str, category: &str) -> String {
    let made: Vec<String> = made
        .get(&searched_key(space, category))
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|s| s["q"].as_str().map(str::to_string)).take(HINTS).collect())
        .unwrap_or_default();
    if !made.is_empty() {
        return made.join(", ");
    }
    let mut ordered: Vec<&super::structure::Listed> = rows.iter().collect();
    if category != "dates" {
        ordered.sort_by(|a, b| b.latest.cmp(&a.latest));
    }
    let mut titles: Vec<&str> = Vec::new();
    for r in ordered {
        let t = r.title.as_str();
        if !t.is_empty() && !titles.contains(&t) {
            titles.push(t);
            if titles.len() == HINTS {
                break;
            }
        }
    }
    titles.join(", ")
}

// -- the moment's card ------------------------------------------------------------------------------------------------

/// One of today's cards, the first that waits: « Ton Carnet est rangé par catégories » (S2), the question of the day
/// and a follow-up still open today (ask.rs, followups.rs), the misplaced points (misplaced.rs), the week's review
/// (review.rs). Each as its own route shows it (`Card {kind, card}`); answered by its own route.
fn card(config: &Config, shown: &str, now: f64) -> Value {
    if super::categories_pass::card_waits(config) {
        let pages = super::all_pages(config, Part::Main);
        if let Some(v) = super::categories_pass::card(config, shown, &pages) {
            return json!({"kind": "sorted", "card": v});
        }
    }
    let state = super::read_state(config);
    let day = super::ask::day_of(now);
    let cards = state.get("cards").and_then(Value::as_array).cloned().unwrap_or_default();
    // never one that speaks of health (its question, its choices, its task): the sommaire is outside the lock (review 1
    // of S4, R2)
    let healthy = |c: &Dict| {
        let mut texts = super::strings(c.get("chips"));
        texts.extend(["question", "title", "label"].iter().map(|k| tk::str_or_empty(c.get(*k))));
        !texts.iter().any(|t| lexicon::touches_health(t))
    };
    for kind in ["ask", "follow"] {
        let open = cards.iter().rev().filter_map(Value::as_object).find(|c| {
            tk::str_or_empty(c.get("card")) == kind
                && tk::str_or_empty(c.get("state")) == "open"
                && tk::str_or_empty(c.get("day")) == day
                && healthy(c)
        });
        let view = open.and_then(|c| if kind == "ask" { super::ask::view(c, shown, now) } else { super::followups::view(c, shown) });
        if let Some(v) = view {
            return json!({"kind": kind, "card": v});
        }
    }
    if let Some(v) = super::misplaced::card(config, shown) {
        return json!({"kind": "misplaced", "card": v});
    }
    match super::review::view(config, shown, now) {
        Ok(Some(r)) => json!({"kind": "review", "card": r["card"]}),
        _ => Value::Null,
    }
}

// -- GET /v1/notebook/summary -----------------------------------------------------------------------------------------

/// The sommaire (`Summary`) of `space` on `today`, its texts in the app's language (`langs`' last).
pub fn summary_at(config: &Config, space: &str, langs: &[String], today: NaiveDate, now: f64) -> Value {
    let shown = titles_lang(langs);
    let t = texts(&shown);
    let (f, version, epoch) = super::index::current(config, langs);
    let (_, total) = by_category(&f, space, today);
    let made = read_searched(config);
    let mut lines: Vec<Value> = Vec::new();
    for id in c::PERSONAL {
        if id == "health" {
            // locked: no count, no word (Santé is never in the copy); not in Pro
            if space != "pro" {
                lines.push(json!({"id": id, "title": c::title(&shown, id), "count": null, "hint": "", "locked": true}));
            }
            continue;
        }
        // what its screen lists (structure.rs, Alex 26/11): its pages, people in Personnes, subjects elsewhere
        let rows = super::structure::pages_in(&f, id, space, today);
        if rows.is_empty() {
            continue;
        }
        // « Toi » is the first line of « Personnes »; else the first listed (Dates: the soonest)
        let first = rows.first().map(|r| r.at);
        let mut line = json!({"id": id, "title": c::title(&shown, id), "count": rows.len(),
                              "hint": hint(&made, &rows, space, id), "locked": false});
        if let Some(at) = first {
            line["first"] = super::index::item_view(&f, at, today);
        }
        lines.push(line);
    }
    json!({"v": V, "lang": shown, "title": t.carnet, "space": space, "spaces": selector(config, &f, space, &shown),
           "kept": kept_with_counts(config, &f, space, today),
           "card": if space == "pro" { Value::Null } else { card(config, &shown, now) },
           "total": {"count": total, "title": t.all_notes}, "categories": lines,
           "bar": {"placeholder": t.bar_all}, "version": version, "epoch": epoch})
}

/// The selector (`Summary.spaces`). The setting « Séparer Perso, Pro et Famille » off: « Moi · Famille » for a member
/// of a family, else none (S6, family_carnet.rs). On: « Perso », then « Pro » when there is a Pro agent or a Pro note
/// (S5, spaces.rs), then « Famille » for a member of a family.
fn selector(config: &Config, f: &Finder, space: &str, lang: &str) -> Value {
    if !super::spaces::on(config) {
        return crate::family_carnet::spaces(config, lang, space);
    }
    let mut choices = super::spaces::choices(config, f, space, lang);
    if crate::family_carnet::in_family(config) {
        if let Some(list) = choices.as_array_mut() {
            list.push(crate::family_carnet::family_choice(lang));
        }
    }
    choices
}

// -- GET /v1/notebook/category/{id} -----------------------------------------------------------------------------------

/// The words of a note's title and line, folded.
fn words_of(f: &Finder, at: usize) -> Vec<String> {
    let e = &f.items[at];
    fold(&format!("{} {}", e.title, e.line)).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn starts(words: &[String], prefixes: &[&str]) -> bool {
    words.iter().any(|w| prefixes.iter().any(|p| w.starts_with(p)))
}

fn record_type(f: &Finder, at: usize) -> &str {
    f.items[at].open.get("record").and_then(|r| r.get("type")).and_then(Value::as_str).unwrap_or("")
}

/// Whether the note at `at` is in sub-filter `id` (the 1.1 apps' « Les plus récents »: contracts/carnet/sommaire.md
/// §3, change 30): the records by their type, the other notes by the words of their title and line (5 languages).
fn in_filter(id: &str, f: &Finder, at: usize) -> bool {
    let e = &f.items[at];
    let kind = record_type(f, at);
    match id {
        "invoices" => e.kind != "record" && starts(&words_of(f, at), &["factur", "factuur", "invoice", "rechnung"]),
        "contracts" => kind == "contract" || (e.kind != "record" && starts(&words_of(f, at), &["contrat", "contract", "vertrag", "contrato"])),
        "warranties" => kind == "purchase" || (e.kind != "record" && starts(&words_of(f, at), &["garanti", "warrant", "guarant", "gewahrleist"])),
        _ => false,
    }
}

/// A category's page (`CategoryPage`): its pages ready to show (`pages`, `groups`: structure.rs, Alex 26/11) and
/// their count, Papiers' sub-filters counted in pages, « Cherché ici » (3); `recent`, the latest 20 notes (of
/// `filter` when given), for the 1.1 apps that list them. Santé (`health`) only behind the app's lock (`private`):
/// read now, kept nowhere, no sub-filter, no search kept.
pub fn category_at(
    config: &Config,
    id: &str,
    space: &str,
    filter: Option<&str>,
    langs: &[String],
    private: bool,
    today: NaiveDate,
) -> Result<Value, Refused> {
    if !c::PERSONAL.contains(&id) {
        return Err(Refused::CategoryUnknown);
    }
    let filters: &[&str] = FILTERS.iter().find(|(cat, _)| *cat == id).map(|(_, f)| *f).unwrap_or(&[]);
    let filter = filter.map(str::trim).filter(|f| !f.is_empty());
    if filter.is_some_and(|f| !filters.contains(&f)) {
        return Err(Refused::FilterUnknown);
    }
    let shown = titles_lang(langs);
    let title = c::title(&shown, id);
    let bar = json!({"placeholder": texts(&shown).bar_in.replace("{category}", &title)});
    if id == "health" {
        if !private {
            return Err(Refused::HealthLocked);
        }
        // (Santé is in no space: the whole of it, whatever the space asked; S5)
        let f = super::index::health_finder(config, langs);
        let ats: Vec<usize> = (0..f.items.len()).collect();
        let recent: Vec<Value> = latest(&f, &ats, id, today).into_iter().take(RECENT).map(|at| super::index::item_view(&f, at, today)).collect();
        // its pages (the private part's: every one of them, by name)
        let rows = super::structure::pages_in(&f, "health", "all", today);
        let pages: Vec<Value> = rows.iter().map(|r| super::structure::entry_view(&f, r, today)).collect();
        return Ok(json!({"v": V, "id": id, "title": title, "count": pages.len(), "space": space, "filters": [],
                         "filter": null, "searched": [], "recent": recent, "pages": pages, "groups": [], "bar": bar}));
    }
    let f = super::index::finder(config, langs);
    let (mut members, _) = by_category(&f, space, today);
    let ats = members.remove(id).unwrap_or_default();
    let rows = super::structure::pages_in(&f, id, space, today);
    let counted: Vec<Value> = filters
        .iter()
        .map(|fid| {
            let n = rows.iter().filter(|r| super::structure::in_papers_filter(&f, r, fid)).count();
            json!({"id": fid, "title": filter_title(&shown, fid), "count": n})
        })
        .collect();
    let shown_rows: Vec<&super::structure::Listed> = match filter {
        Some(fid) => rows.iter().filter(|r| super::structure::in_papers_filter(&f, r, fid)).collect(),
        None => rows.iter().collect(),
    };
    let pages: Vec<Value> = shown_rows.iter().map(|r| super::structure::entry_view(&f, r, today)).collect();
    let listed: Vec<usize> = match filter {
        Some(fid) => ats.iter().copied().filter(|at| in_filter(fid, &f, *at)).collect(),
        None => ats.clone(),
    };
    let recent: Vec<Value> =
        latest(&f, &listed, id, today).into_iter().take(RECENT).map(|at| super::index::item_view(&f, at, today)).collect();
    let groups = super::structure::groups_view(&rows, &shown);
    Ok(json!({"v": V, "id": id, "title": title, "count": rows.len(), "space": space, "filters": counted,
              "filter": filter, "searched": searched_in(config, space, id), "recent": recent, "pages": pages,
              "groups": groups, "bar": bar}))
}

// -- kept searches (state.json `kept_searches`) ------------------------------------------------------------------------

fn kept_all(state: &Dict) -> Vec<Dict> {
    state.get(KEPT_KEY).and_then(Value::as_array).map(|l| l.iter().filter_map(|k| k.as_object().cloned()).collect()).unwrap_or_default()
}

fn kept_space(k: &Dict) -> String {
    let s = tk::str_or_empty(k.get("space"));
    if s.is_empty() { "all".into() } else { s }
}

fn kept_category(k: &Dict) -> Option<String> {
    k.get("category").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string)
}

/// The kept searches of the view of `space` (`Kept`; spaces.rs `kept_in_view`: `all` every space's, `perso` also
/// those kept with the setting off), six at most (the newest: after the setting changed a view may hold more, none
/// is dropped; review 1 of S5, R2), each with what it finds now in that view (`count`: `all` of the sommaire's
/// search).
fn kept_with_counts(config: &Config, f: &Finder, space: &str, today: NaiveDate) -> Vec<Value> {
    let all = kept_all(&super::read_state(config));
    let shown: Vec<&Dict> = all.iter().filter(|k| super::spaces::kept_in_view(&kept_space(k), space)).collect();
    let skip = shown.len().saturating_sub(KEPT_MAX);
    shown
        .into_iter()
        .skip(skip)
        .map(|k| {
            let q = tk::str_or_empty(k.get("q"));
            let category = kept_category(k);
            let count = f.search(&q, today, category.as_deref(), narrowed(space)).counts.get("all").copied().unwrap_or(0);
            let shown_space = if kept_space(k) == "all" { Value::Null } else { json!(kept_space(k)) };
            json!({"id": k.get("id"), "q": q, "title": tk::str_or_empty(k.get("title")), "count": count,
                   "space": shown_space, "category": category})
        })
        .collect()
}

/// `GET /v1/notebook/kept` (`KeptList`).
pub fn kept_at(config: &Config, space: &str, langs: &[String], today: NaiveDate) -> Value {
    let f = super::index::finder(config, langs);
    json!({"kept": kept_with_counts(config, &f, space, today), "max": KEPT_MAX})
}

/// The kept searches as the phone's copy carries them (`KeptShort`: every space's, the words and the title; a key at
/// its default left out). index.rs moves the copy's version when they change.
pub fn kept_short(config: &Config) -> Value {
    let list: Vec<Value> = kept_all(&super::read_state(config))
        .iter()
        .map(|k| {
            let mut v = json!({"id": k.get("id"), "q": k.get("q"), "title": k.get("title")});
            if let Some(cat) = kept_category(k) {
                v["category"] = json!(cat);
            }
            if kept_space(k) != "all" {
                v["space"] = json!(kept_space(k));
            }
            v
        })
        .collect();
    json!(list)
}

fn chars(text: &str, most: usize) -> String {
    text.chars().take(most).collect()
}

/// A search kept (« Garder »): its words as typed, its title (the words when none), in `space`, in one category when
/// made from its page. The same words again in the same place (its view): kept once; 6 a view. Returns the space.
pub fn keep(config: &Config, q: &str, title: Option<&str>, space: &str, category: Option<&str>) -> Result<String, Refused> {
    let category = category.map(str::trim).filter(|c| !c.is_empty());
    match category {
        Some("health") => return Err(Refused::CategoryNotGiven),
        Some(cat) if !c::PERSONAL.contains(&cat) => return Err(Refused::CategoryUnknown),
        _ => {}
    }
    let q = chars(q.trim(), Q_MAX);
    if fold(&q).trim().is_empty() {
        return Err(Refused::KeptEmpty);
    }
    if super::index::reference(&q) {
        return Err(Refused::NumberRefused);
    }
    let title = title.map(|t| chars(t.trim(), TITLE_MAX)).filter(|t| !t.is_empty()).unwrap_or_else(|| q.clone());
    if super::index::reference(&title) {
        return Err(Refused::NumberRefused);
    }
    let _guard = super::lock();
    let state = super::read_state(config);
    let mut all = kept_all(&state);
    let shown = |k: &Dict| super::spaces::kept_in_view(&kept_space(k), space);
    let same = |k: &Dict| shown(k) && fold(&tk::str_or_empty(k.get("q"))) == fold(&q) && kept_category(k).as_deref() == category;
    if all.iter().any(same) {
        return Ok(space.to_string());
    }
    if all.iter().filter(|k| shown(k)).count() >= KEPT_MAX {
        return Err(Refused::KeptFull);
    }
    let mut k = Dict::new();
    k.insert("id".into(), json!(format!("k{}", crate::py::token_hex(3))));
    k.insert("q".into(), json!(q));
    k.insert("title".into(), json!(title));
    k.insert("space".into(), json!(space));
    k.insert("category".into(), json!(category));
    k.insert("at".into(), json!(tk::now_iso_seconds()));
    all.push(k);
    super::update_state(config, |s| {
        s.insert(KEPT_KEY.into(), json!(all));
    })
    .map_err(|_| Refused::NotWritten)?;
    Ok(space.to_string())
}

/// A kept search renamed (appui long). Returns its space.
pub fn rename(config: &Config, id: &str, title: &str) -> Result<String, Refused> {
    let title = chars(title.trim(), TITLE_MAX);
    if super::index::reference(&title) {
        return Err(Refused::NumberRefused);
    }
    let _guard = super::lock();
    let mut all = kept_all(&super::read_state(config));
    let Some(k) = all.iter_mut().find(|k| tk::str_or_empty(k.get("id")) == id) else { return Err(Refused::KeptUnknown) };
    let title = if title.is_empty() { tk::str_or_empty(k.get("q")) } else { title };
    k.insert("title".into(), json!(title));
    let space = kept_space(k);
    super::update_state(config, |s| {
        s.insert(KEPT_KEY.into(), json!(all));
    })
    .map_err(|_| Refused::NotWritten)?;
    Ok(space)
}

/// A kept search deleted. Returns its space.
pub fn drop_kept(config: &Config, id: &str) -> Result<String, Refused> {
    let _guard = super::lock();
    let mut all = kept_all(&super::read_state(config));
    let Some(at) = all.iter().position(|k| tk::str_or_empty(k.get("id")) == id) else { return Err(Refused::KeptUnknown) };
    let space = kept_space(&all.remove(at));
    super::update_state(config, |s| {
        s.insert(KEPT_KEY.into(), json!(all));
    })
    .map_err(|_| Refused::NotWritten)?;
    Ok(space)
}

/// « Oublie ça » (forget.rs `apply`, under the Carnet's lock, in its `state.json` change): the kept searches of every
/// space whose words or title `gone` says (folded as the search folds them); how many left. The copy's version moves
/// with them (index.rs), so the phones drop them at their next read.
pub fn forget_kept(state: &mut Dict, gone: &dyn Fn(&str) -> bool) -> usize {
    let all = kept_all(state);
    let left: Vec<Dict> =
        all.iter().filter(|k| !gone(&tk::str_or_empty(k.get("q"))) && !gone(&tk::str_or_empty(k.get("title")))).cloned().collect();
    let n = all.len() - left.len();
    if n > 0 {
        state.insert(KEPT_KEY.into(), json!(left));
    }
    n
}

// -- searches made (searched.json) ------------------------------------------------------------------------------------

static SEARCHED: Mutex<()> = Mutex::new(());

/// Where the searches made are kept: beside `state.json`, never a page, never the state (whose change makes the
/// phone's copy again), never read by an agent's tool.
pub fn searched_path(config: &Config) -> PathBuf {
    super::root(config).join("searched.json")
}

fn read_searched(config: &Config) -> serde_json::Map<String, Value> {
    match tk::read_text(&searched_path(config)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

fn searched_key(space: &str, category: &str) -> String {
    format!("{space}/{category}")
}

/// « Cherché ici »: the latest searches made in `category` of `space` (`[Searched]`, the newest first).
fn searched_in(config: &Config, space: &str, category: &str) -> Vec<Value> {
    if category == "health" {
        return Vec::new();
    }
    read_searched(config).get(&searched_key(space, category)).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn write_searched(config: &Config, all: serde_json::Map<String, Value>) -> std::io::Result<()> {
    let path = searched_path(config);
    std::fs::create_dir_all(super::root(config))?;
    let temporary = path.with_extension("tmp");
    crate::py::write_with_mode(&temporary.to_string_lossy(), Value::Object(all).to_string().as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &path)
}

/// « Oublie ça » (forget.rs `apply`): the searches made, in every space and category, whose words `gone` says (« Cherché
/// ici », the grey words); how many left.
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
        all.retain(|_, list| list.as_array().is_some_and(|l| !l.is_empty()));
        write_searched(config, all)?;
    }
    Ok(n)
}

/// Searches made dropped by the person (`DELETE /v1/notebook/searched`; review 1 of S4, B2: one whose words are
/// nowhere in the Carnet, that « Oublie ça » cannot find): the one of these words (`q`, folded) or all, in `space`
/// (none: every space) and `category` (none: every one). How many.
pub fn drop_searched(config: &Config, q: Option<&str>, space: Option<&str>, category: Option<&str>) -> std::io::Result<usize> {
    let folded = q.map(fold).map(|f| f.split_whitespace().collect::<Vec<_>>().join(" "));
    let _guard = SEARCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_searched(config);
    let mut n = 0;
    for (key, list) in all.iter_mut() {
        let (s, c) = key.split_once('/').unwrap_or((key.as_str(), ""));
        if space.is_some_and(|x| x != s) || category.is_some_and(|x| x != c) {
            continue;
        }
        if let Value::Array(items) = list {
            let before = items.len();
            items.retain(|i| {
                folded.as_ref().is_some_and(|f| {
                    fold(i["q"].as_str().unwrap_or_default()).split_whitespace().collect::<Vec<_>>().join(" ") != *f
                })
            });
            n += before - items.len();
        }
    }
    if n > 0 {
        all.retain(|_, list| list.as_array().is_some_and(|l| !l.is_empty()));
        write_searched(config, all)?;
    }
    Ok(n)
}

/// A search made (`POST /v1/notebook/searched`), kept for « Cherché ici » and the grey words. Whether it was kept.
pub fn searched_at(config: &Config, q: &str, category: Option<&str>, space: &str, langs: &[String], today: NaiveDate) -> Result<bool, Refused> {
    let category = category.map(str::trim).filter(|c| !c.is_empty());
    if let Some(cat) = category {
        if !c::PERSONAL.contains(&cat) {
            return Err(Refused::CategoryUnknown);
        }
    }
    let q = chars(q.trim(), Q_MAX);
    // never Santé, a number or a reference, a single letter
    if category == Some("health") || fold(&q).trim().chars().count() < 2 || super::index::reference(&q) {
        return Ok(false);
    }
    let category = match category {
        Some(cat) => cat.to_string(),
        None => {
            let f = super::index::finder(config, langs);
            match f.search(&q, today, None, narrowed(space)).groups.first() {
                Some(g) => g.category.clone(),
                None => return Ok(false),
            }
        }
    };
    if category == "health" {
        return Ok(false);
    }
    let _guard = SEARCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_searched(config);
    let key = searched_key(space, &category);
    let mut list: Vec<Value> = all.get(&key).and_then(Value::as_array).cloned().unwrap_or_default();
    let folded = fold(&q);
    list.retain(|s| fold(s["q"].as_str().unwrap_or_default()) != folded);
    list.insert(0, json!({"q": q, "at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()}));
    list.truncate(SEARCHED_KEPT);
    all.insert(key, json!(list));
    Ok(write_searched(config, all).is_ok())
}
