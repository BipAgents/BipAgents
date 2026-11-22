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
//! Everything is counted on the phone's copy (index.rs: the same notes, the same rule as the search's chips; Santé is
//! never in it): a category's `count` is its notes (`dates` as of today), `total` each note once. Santé's line is
//! locked: no count, no word; its page is read behind the app's lock only (`private=1`), kept nowhere.
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
//! - **Spaces** (`all`, `perso`, `pro`): until S5 no note is Pro: `perso` is the whole Carnet and `pro` nothing; the
//!   selector (`spaces`) stays empty until S5 (Perso · Pro) and S6 (Famille).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use super::categories as c;
use super::finder::Finder;
use super::{fold, Part};
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
/// Each category's sub-filters (sommaire.json `categories.filters`), in their order.
pub const FILTERS: [(&str, &[&str]); 4] = [
    ("papers", &["invoices", "contracts", "warranties"]),
    ("money", &["to_pay", "subscriptions", "purchases"]),
    ("dates", &["this_week", "this_month", "later"]),
    ("people", &["close", "others"]),
];

/// The sub-filters' names (contracts/carnet/sommaire.json `titles_proposed`, the same table: checked by
/// `the_sub_filter_titles_are_the_contracts`) in fr, en, de, es, nl. Visible text: proposed, for Alex.
const FILTER_TITLES: [(&str, [&str; 5]); 11] = [
    ("invoices", ["Factures", "Invoices", "Rechnungen", "Facturas", "Facturen"]),
    ("contracts", ["Contrats", "Contracts", "Verträge", "Contratos", "Contracten"]),
    ("warranties", ["Garanties", "Warranties", "Garantien", "Garantías", "Garanties"]),
    ("to_pay", ["À payer", "To pay", "Zu zahlen", "Por pagar", "Te betalen"]),
    ("subscriptions", ["Abonnements", "Subscriptions", "Abos", "Suscripciones", "Abonnementen"]),
    ("purchases", ["Achats", "Purchases", "Käufe", "Compras", "Aankopen"]),
    ("this_week", ["Cette semaine", "This week", "Diese Woche", "Esta semana", "Deze week"]),
    ("this_month", ["Ce mois-ci", "This month", "Diesen Monat", "Este mes", "Deze maand"]),
    ("later", ["Plus tard", "Later", "Später", "Más adelante", "Later"]),
    ("close", ["Proches", "Close ones", "Nahestehende", "Cercanos", "Naasten"]),
    ("others", ["Autres", "Others", "Andere", "Otros", "Anderen"]),
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

/// The grey words: the category's latest searches, else its latest titles (a line's is its page's), 3 at most.
fn hint(made: &serde_json::Map<String, Value>, f: &Finder, ordered: &[usize], space: &str, category: &str) -> String {
    let made: Vec<String> = made
        .get(&searched_key(space, category))
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|s| s["q"].as_str().map(str::to_string)).take(HINTS).collect())
        .unwrap_or_default();
    if !made.is_empty() {
        return made.join(", ");
    }
    let mut titles: Vec<&str> = Vec::new();
    for at in ordered {
        let t = f.items[*at].title.as_str();
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
    for kind in ["ask", "follow"] {
        let open = cards.iter().rev().filter_map(Value::as_object).find(|c| {
            tk::str_or_empty(c.get("card")) == kind && tk::str_or_empty(c.get("state")) == "open" && tk::str_or_empty(c.get("day")) == day
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
    let (f, version) = super::index::current(config, langs);
    let (members, total) = by_category(&f, space, today);
    let me = super::words(&super::language(config)).self_path();
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
        let Some(ats) = members.get(id).filter(|a| !a.is_empty()) else { continue };
        let ordered = latest(&f, ats, id, today);
        // « Toi » is the first line of « Personnes »; else the latest
        let first = match id {
            "people" => ats.iter().copied().find(|at| f.items[*at].kind == "page" && f.items[*at].path.as_deref() == Some(me.as_str())),
            _ => None,
        }
        .or_else(|| ordered.first().copied());
        let mut line = json!({"id": id, "title": c::title(&shown, id), "count": ats.len(),
                              "hint": hint(&made, &f, &ordered, space, id), "locked": false});
        if let Some(at) = first {
            line["first"] = super::index::item_view(&f, at, today);
        }
        lines.push(line);
    }
    json!({"v": V, "lang": shown, "title": t.carnet, "space": space, "spaces": [],
           "kept": kept_with_counts(config, &f, space, today),
           "card": if space == "pro" { Value::Null } else { card(config, &shown, now) },
           "total": {"count": total, "title": t.all_notes}, "categories": lines,
           "bar": {"placeholder": t.bar_all}, "version": version})
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

/// The last day of `today`'s week (Sunday) and of its month.
fn week_and_month_ends(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let sunday = today + Duration::days(6 - i64::from(today.weekday().num_days_from_monday()));
    let next = if today.month() == 12 {
        NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
    };
    (sunday, next.map(|d| d - Duration::days(1)).unwrap_or(today))
}

/// Whether the note at `at` is in sub-filter `id` (contracts/carnet/sommaire.md §3, change 30): the records by their
/// type, the other notes by the words of their title and line (5 languages); Dates by their day still to come;
/// Personnes by the close ones (`close`: « Toi », the pages marked « proche », their lines).
fn in_filter(id: &str, f: &Finder, at: usize, today: NaiveDate, close: &HashSet<String>) -> bool {
    let e = &f.items[at];
    let kind = record_type(f, at);
    let coming = || f.day(at, today).filter(|d| *d >= today);
    let (sunday, month_end) = week_and_month_ends(today);
    match id {
        "invoices" => e.kind != "record" && starts(&words_of(f, at), &["factur", "factuur", "invoice", "rechnung"]),
        "contracts" => kind == "contract" || (e.kind != "record" && starts(&words_of(f, at), &["contrat", "contract", "vertrag", "contrato"])),
        "warranties" => kind == "purchase" || (e.kind != "record" && starts(&words_of(f, at), &["garanti", "warrant", "guarant", "gewahrleist"])),
        // what is still to pay: an open task, a renewal to come, a line or a document with a day to come
        "to_pay" => match e.kind.as_str() {
            "task" => !e.done,
            "record" => kind == "subscription" && coming().is_some(),
            _ => coming().is_some(),
        },
        "subscriptions" => {
            kind == "subscription" || (e.kind != "record" && {
                let w = words_of(f, at);
                starts(&w, &["abonn", "subscri", "suscrip"]) || w.iter().any(|x| x == "abo" || x == "abos")
            })
        }
        "purchases" => kind == "purchase" || (e.kind != "record" && starts(&words_of(f, at), &["achat", "achet", "purchase", "bought", "gekauft", "compra", "aankoop", "gekocht"])),
        "this_week" => coming().is_some_and(|d| d <= sunday),
        "this_month" => coming().is_some_and(|d| d <= month_end),
        "later" => coming().is_some_and(|d| d > month_end),
        "close" => close.contains(&e.id) || e.page.as_ref().is_some_and(|p| close.contains(p)),
        "others" => !(close.contains(&e.id) || e.page.as_ref().is_some_and(|p| close.contains(p))),
        _ => false,
    }
}

/// The close ones' pages (« Toi », a person marked « proche »): their Item ids.
fn close_pages(config: &Config) -> HashSet<String> {
    super::all_pages(config, Part::Main)
        .iter()
        .filter(|(_, p)| p.close() || p.kind() == "self")
        .map(|(path, p)| format!("page:{}", c::page_key(p, path)))
        .collect()
}

/// A category's page (`CategoryPage`): its sub-filters with their counts, « Cherché ici » (3), the latest (20, of
/// `filter` when given). Santé (`health`) only behind the app's lock (`private`): read now, kept nowhere, no
/// sub-filter, no search kept.
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
        let f = super::index::health_finder(config, langs);
        let ats: Vec<usize> = (0..f.items.len()).filter(|at| in_space(&f, *at, space)).collect();
        let recent: Vec<Value> = latest(&f, &ats, id, today).into_iter().take(RECENT).map(|at| super::index::item_view(&f, at, today)).collect();
        return Ok(json!({"v": V, "id": id, "title": title, "count": ats.len(), "space": space, "filters": [],
                         "filter": null, "searched": [], "recent": recent, "bar": bar}));
    }
    let f = super::index::finder(config, langs);
    let (mut members, _) = by_category(&f, space, today);
    let ats = members.remove(id).unwrap_or_default();
    let close = if id == "people" { close_pages(config) } else { HashSet::new() };
    let counted: Vec<Value> = filters
        .iter()
        .map(|fid| {
            let n = ats.iter().filter(|at| in_filter(fid, &f, **at, today, &close)).count();
            json!({"id": fid, "title": filter_title(&shown, fid), "count": n})
        })
        .collect();
    let listed: Vec<usize> = match filter {
        Some(fid) => ats.iter().copied().filter(|at| in_filter(fid, &f, *at, today, &close)).collect(),
        None => ats.clone(),
    };
    // a sub-filter of days: the soonest first
    let order_as = if matches!(filter, Some("this_week" | "this_month" | "later")) { "dates" } else { id };
    let recent: Vec<Value> =
        latest(&f, &listed, order_as, today).into_iter().take(RECENT).map(|at| super::index::item_view(&f, at, today)).collect();
    Ok(json!({"v": V, "id": id, "title": title, "count": ats.len(), "space": space, "filters": counted,
              "filter": filter, "searched": searched_in(config, space, id), "recent": recent, "bar": bar}))
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

/// The kept searches of `space` (`Kept`), each with what it finds now (`count`: `all` of the sommaire's search).
fn kept_with_counts(config: &Config, f: &Finder, space: &str, today: NaiveDate) -> Vec<Value> {
    kept_all(&super::read_state(config))
        .iter()
        .filter(|k| kept_space(k) == space)
        .map(|k| {
            let q = tk::str_or_empty(k.get("q"));
            let category = kept_category(k);
            let count = f.search(&q, today, category.as_deref(), narrowed(space)).counts.get("all").copied().unwrap_or(0);
            let shown_space = if space == "all" { Value::Null } else { json!(space) };
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
/// made from its page. The same words again in the same place: kept once. Returns the space.
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
    let same = |k: &Dict| kept_space(k) == space && fold(&tk::str_or_empty(k.get("q"))) == fold(&q) && kept_category(k).as_deref() == category;
    if all.iter().any(same) {
        return Ok(space.to_string());
    }
    if all.iter().filter(|k| kept_space(k) == space).count() >= KEPT_MAX {
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
