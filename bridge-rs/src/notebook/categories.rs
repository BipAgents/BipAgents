//! The Carnet's categories (plan « Le sommaire », brick S1; contracts/carnet/sommaire.md §1–2): a label on each page,
//! record, task, document and line — never a folder, nothing moves. Personal ids `people, dates, money, home, papers,
//! trips, work, other, health`, family ones apart (S6). Filed by the bridge at each write, by words, without a model
//! and without a token (`decide`: the kind first, then the person's rules, then the 5-language table of
//! lexicon::CATEGORY_WORDS); an agent may say one (`notebook_write.category`); the person's correction always wins
//! (`categories_by: person`) and teaches the next writes (`category_rules`). `dates` is computed, never stored;
//! `health` is the private part.
//!
//! On a page: `categories` (1 or 2) and `categories_by` (auto, agent, person) in its frontmatter; a page without them
//! is read with the categories `decide` gives now (stored at its next write, and by S2's pass). A record, task,
//! document or line has no frontmatter: theirs come from their page or type, the person's correction kept in
//! `state.json` `category_marks`.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use super::{assign, fold, groups, lexicon, records, Page, Part, Task, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// The personal Carnet's categories, in the sommaire's order.
pub const PERSONAL: [&str; 9] = ["people", "dates", "money", "home", "papers", "trips", "work", "other", "health"];
/// The family Carnet's (S6).
pub const FAMILY: [&str; 9] = ["shopping", "people", "dates", "home", "papers", "holidays", "school", "menus", "other"];
/// What an agent may say (`notebook_write.category`): never `dates` (computed), `health` (the private part) or
/// `other` (what nothing places).
pub const AGENT_MAY_GIVE: [&str; 6] = ["people", "money", "home", "papers", "trips", "work"];
/// What the person may choose (« Ranger ailleurs »), and what a page may hold.
pub const PERSON_MAY_GIVE: [&str; 7] = ["people", "money", "home", "papers", "trips", "work", "other"];
pub const BY: [&str; 3] = ["auto", "agent", "person"];
pub const AT_MOST: usize = 2;
/// Rules learned from the person's corrections kept (the most recent first).
pub const RULES_KEPT: usize = 200;
/// Corrections that can still be undone.
pub const UNDO_KEPT: usize = 20;
/// The person's corrections of records, tasks, documents and lines kept.
const MARKS_KEPT: usize = 2000;
/// An item is in « Dates » when its date is from this many days ago …
const DATED_BEFORE: i64 = 90;
/// … to this many days ahead (a passport's expiry, a warranty's end).
const DATED_AFTER: i64 = 730;

// How much each part of a page counts for its words' categories: the title most.
const TITLE: f64 = 5.0;
const ALIASES: f64 = 4.0;
const SUMMARY: f64 = 2.0;
const LABEL: f64 = 1.0;
const FACT: f64 = 0.5;
/// The facts together count this much at most for one category (a long page is not filed by its volume).
const FACTS_CAP: f64 = 2.0;
/// The facts read for words.
const FACTS_READ: usize = 40;
/// A table entry's second category counts this share of its first.
const SECOND: f64 = 0.9;
/// A word the person filed (a rule) counts this much more than the table.
const RULE: f64 = 2.0;
/// Below this, nothing places the page: « Autres ».
const ENOUGH: f64 = 1.0;
/// A second category when it scores this share of the first.
const BESIDE: f64 = 0.75;

/// Words that make someone a work contact (a person page: people and work), folded.
const COLLEAGUE: &[&str] = &[
    "collegue", "collegues", "colleague", "colleagues", "kollege", "kollegin", "kollegen", "colega", "colegas",
    "collega", "collegas", "manager", "client", "cliente", "clients", "customer", "kunde", "kundin", "klant",
];

fn order(category: &str) -> usize {
    PERSONAL.iter().chain(FAMILY.iter()).position(|c| *c == category).unwrap_or(99)
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|c| c.to_string()).collect()
}

/// `given` as categories when they are 1 or 2 of `allowed` (lower case, each once), else None.
pub fn valid(given: &[String], allowed: &[&str]) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for c in given {
        let c = c.trim().to_lowercase();
        if !allowed.contains(&c.as_str()) {
            return None;
        }
        if !out.contains(&c) {
            out.push(c);
        }
    }
    (!out.is_empty() && out.len() <= AT_MOST).then_some(out)
}

/// The categories written on a page (its frontmatter) and by whom, when they are valid ones.
pub fn stored(page: &Page) -> Option<(Vec<String>, String)> {
    let cats = valid(&super::strings(page.front.get("categories")), &PERSON_MAY_GIVE)?;
    let by = tk::str_or_empty(page.front.get("categories_by"));
    Some((cats, if BY.contains(&by.as_str()) { by } else { "auto".into() }))
}

/// The page's frontmatter with `categories` and `categories_by` (before `updated`, readable in Obsidian).
pub fn set_front(page: &mut Page, categories: &[String], by: &str) {
    let mut front = Dict::new();
    let mut placed = false;
    for (k, v) in std::mem::take(&mut page.front) {
        if k == "categories" || k == "categories_by" {
            continue;
        }
        if k == "updated" && !placed {
            front.insert("categories".into(), json!(categories));
            front.insert("categories_by".into(), json!(by));
            placed = true;
        }
        front.insert(k, v);
    }
    if !placed {
        front.insert("categories".into(), json!(categories));
        front.insert("categories_by".into(), json!(by));
    }
    page.front = front;
}

/// The page without categories (read again as the bridge's).
pub fn clear_front(page: &mut Page) {
    page.front.remove("categories");
    page.front.remove("categories_by");
}

// -- the words ------------------------------------------------------------------------------------------------------

/// The table's one-word stems by their first two letters: (stem, whole word only, entry).
static STEMS: LazyLock<HashMap<String, Vec<(&'static str, bool, usize)>>> = LazyLock::new(|| {
    let mut out: HashMap<String, Vec<(&'static str, bool, usize)>> = HashMap::new();
    for (i, (_, stems)) in lexicon::CATEGORY_WORDS.iter().enumerate() {
        for stem in stems.iter().filter(|s| !s.contains(' ')) {
            let (body, exact) = match stem.strip_suffix('=') {
                Some(b) => (b, true),
                None => (*stem, false),
            };
            out.entry(body.chars().take(2).collect()).or_default().push((body, exact, i));
        }
    }
    out
});

/// A rule the person taught: a word (folded) and the categories they gave a page it decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub word: String,
    pub categories: Vec<String>,
}

pub fn rules(config: &Config) -> Vec<Rule> {
    rules_in(&super::read_state(config))
}

fn rules_in(state: &Dict) -> Vec<Rule> {
    state
        .get("category_rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let word = fold(&tk::str_or_empty(r.get("word")));
            let cats = valid(&super::strings(r.get("categories")), &PERSON_MAY_GIVE)?;
            (!word.is_empty()).then_some(Rule { word, categories: cats })
        })
        .collect()
}

/// What `text`'s words say: (categories, how much a match counts, the word) for each match — the person's rules
/// first (their word counts double), then the table's phrases, then each word's longest stem.
fn matches(text: &str, rules: &[Rule]) -> Vec<(Vec<String>, f64, String)> {
    let mut padded = format!(" {} ", fold(text));
    let mut out = Vec::new();
    for r in rules {
        let p = format!(" {} ", r.word);
        if padded.contains(&p) {
            out.push((r.categories.clone(), RULE, r.word.clone()));
            padded = padded.replace(&p, " ");
        }
    }
    for (cats, stems) in lexicon::CATEGORY_WORDS {
        for stem in stems.iter().filter(|s| s.contains(' ')) {
            let body = stem.trim_end_matches('=');
            let p = format!(" {body} ");
            if padded.contains(&p) {
                out.push((strings(cats), 1.0, body.to_string()));
                padded = padded.replace(&p, " ");
            }
        }
    }
    for word in padded.split_whitespace() {
        let key: String = word.chars().take(2).collect();
        let best = STEMS
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|(body, exact, _)| if *exact { word == *body } else { word.starts_with(body) })
            .max_by_key(|(body, _, i)| (body.len(), std::cmp::Reverse(*i)));
        if let Some((_, _, i)) = best {
            out.push((strings(lexicon::CATEGORY_WORDS[*i].0), 1.0, word.to_string()));
        }
    }
    out
}

/// The points of each category, and the word that gave each its most.
#[derive(Default)]
struct Points {
    total: HashMap<String, f64>,
    best: HashMap<String, (f64, String)>,
}

impl Points {
    fn add(&mut self, category: &str, points: f64, word: &str) {
        *self.total.entry(category.to_string()).or_default() += points;
        let best = self.best.entry(category.to_string()).or_insert((0.0, String::new()));
        if points > best.0 {
            *best = (points, word.to_string());
        }
    }

    fn text(&mut self, text: &str, weight: f64, rules: &[Rule]) {
        for (cats, factor, word) in matches(text, rules) {
            for (n, c) in cats.iter().enumerate() {
                self.add(c, weight * factor * if n == 0 { 1.0 } else { SECOND }, &word);
            }
        }
    }

    /// The categories, the best first (the sommaire's order among equals), and the word that decided.
    fn result(&self) -> (Vec<String>, Option<String>) {
        let mut ranked: Vec<(&String, &f64)> = self.total.iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal).then(order(a.0).cmp(&order(b.0))));
        let Some((top, points)) = ranked.first().map(|(c, p)| ((*c).clone(), **p)) else {
            return (vec!["other".into()], None);
        };
        if points < ENOUGH {
            return (vec!["other".into()], None);
        }
        let mut cats = vec![top.clone()];
        if let Some((second, p)) = ranked.get(1) {
            if **p >= BESIDE * points {
                cats.push((*second).clone());
            }
        }
        let word = self.best.get(&top).map(|(_, w)| w.clone()).filter(|w| !w.is_empty());
        (cats, word)
    }
}

/// A subject's or an area's categories from its words: the title, its aliases, its « En bref », its labels and facts,
/// its « Chiffres et budget » and its amounts (lang: the Carnet's).
fn by_words(page: &Page, lang: &str, rules: &[Rule]) -> (Vec<String>, Option<String>) {
    let brief = super::words(lang).brief;
    let mut points = Points::default();
    points.text(&page.title, TITLE, rules);
    points.text(&page.aliases().join(" "), ALIASES, rules);
    let summary = super::summary_of(&page.body, brief);
    points.text(&summary, SUMMARY, rules);
    if super::news::mentions_money(&summary) {
        points.add("money", ENOUGH, "");
    }
    let mut facts = Points::default();
    let kind = page.kind();
    let mut read = 0;
    for (section, list) in page.sections() {
        if fold(&section) == fold(brief) {
            continue;
        }
        if !list.is_empty() && super::template::resolve(&kind, &section) == Some("budget") {
            points.add("money", LABEL, "budget");
        }
        for fact in list.iter().take(FACTS_READ.saturating_sub(read)) {
            read += 1;
            match lexicon::split_label(&fact.text) {
                Some((label, value)) => {
                    facts.text(&label, LABEL, rules);
                    facts.text(&value, FACT, rules);
                }
                None => facts.text(&fact.text, FACT, rules),
            }
            if super::news::mentions_money(&fact.text) {
                facts.add("money", FACT, "");
            }
        }
    }
    for (category, total) in facts.total {
        let (_, word) = facts.best.get(&category).cloned().unwrap_or_default();
        points.add(&category, total.min(FACTS_CAP), &word);
    }
    points.result()
}

/// A records page's categories, by its type.
pub fn of_records_type(t: Option<&str>) -> Vec<String> {
    strings(match t {
        Some("purchase") => &["money", "papers"],
        Some("subscription") => &["money"],
        Some("contract" | "paper") => &["papers"],
        Some("booking") => &["trips"],
        _ => &["other"],
    })
}

fn colleague(page: &Page, lang: &str) -> bool {
    let brief = super::words(lang).brief;
    let mut text = super::summary_of(&page.body, brief);
    for (section, list) in page.sections() {
        if super::template::resolve("person", &section) == Some("who") || super::template::resolve("person", &section) == Some("work") {
            for f in list {
                text.push(' ');
                text.push_str(&f.text);
            }
        }
    }
    let padded = format!(" {} ", fold(&text));
    COLLEAGUE.iter().any(|w| padded.contains(&format!(" {w} ")))
}

/// The bridge's categories for a page, and the word that decided (None: its kind, its group or its type did):
/// the private part is Santé; « Toi » and people are Gens (a colleague also Travail); the group pages by their
/// relation; trips; records pages by type; the tasks page and the inbox « Autres »; a page a work agent (`writer`)
/// makes, Travail; the others by their words (the person's rules first).
pub fn decide(config: &Config, part: Part, path: &str, page: &Page, writer: Option<&str>) -> (Vec<String>, Option<String>) {
    if part == Part::Health {
        return (vec!["health".into()], None);
    }
    let lang = super::language(config);
    let w = super::words(&lang);
    let kind = page.kind();
    let fixed = |list: &[&str]| (strings(list), None);
    match kind.as_str() {
        "self" => return fixed(&["people"]),
        "trip" => return fixed(&["trips"]),
        "tasks" | "documents" => return fixed(&["other"]),
        "records" => return (of_records_type(records::kind_of_page(page).map(|t| t.id)), None),
        _ => {}
    }
    if path == w.inbox_path() || path == w.tasks_path() {
        return fixed(&["other"]);
    }
    let work_agent = writer.is_some_and(|a| assign::is_work_agent(config, a));
    if kind == "person" {
        return if work_agent || colleague(page, &lang) { fixed(&["people", "work"]) } else { fixed(&["people"]) };
    }
    match groups::group_relation(&page.title) {
        Some("work") => return fixed(&["work", "people"]),
        Some(_) => return fixed(&["people"]),
        None => {}
    }
    if work_agent {
        return fixed(&["work"]);
    }
    by_words(page, &lang, &rules(config))
}

/// A page's categories as the apps show them.
#[derive(Debug, Clone, PartialEq)]
pub struct Filed {
    pub categories: Vec<String>,
    pub by: String,
    /// Written on the page (else computed now, stored at its next write).
    pub stored: bool,
}

impl Filed {
    pub fn view(&self) -> Value {
        json!({"categories": self.categories, "categories_by": self.by})
    }
}

/// A page's categories: those written on it, else the bridge's now.
pub fn of_page(config: &Config, part: Part, path: &str, page: &Page) -> Filed {
    if part == Part::Health {
        return Filed { categories: vec!["health".into()], by: "auto".into(), stored: false };
    }
    match stored(page) {
        Some((categories, by)) => Filed { categories, by, stored: true },
        None => Filed { categories: decide(config, part, path, page, None).0, by: "auto".into(), stored: false },
    }
}

/// An agent's `category` written on the page (valid ones of `AGENT_MAY_GIVE`), unless the person chose: whether it
/// changed.
pub fn choose(page: &mut Page, chosen: &[String], who: &Who) -> bool {
    let Some(cats) = valid(chosen, &AGENT_MAY_GIVE).filter(|_| who.agent.is_some()) else { return false };
    let now = stored(page);
    if now.as_ref().is_some_and(|(_, by)| by == "person") || now == Some((cats.clone(), "agent".into())) {
        return false;
    }
    set_front(page, &cats, "agent");
    true
}

/// At a write of a page of the shared part (an agent's notebook_write, the app's new page): its categories written
/// when it has none yet (or only « Autres » from the bridge, filed again). A page already filed keeps its categories.
/// Whether they changed.
pub fn on_write(config: &Config, part: Part, path: &str, page: &mut Page, who: &Who) -> bool {
    if part != Part::Main {
        return false;
    }
    let now = stored(page);
    if now.as_ref().is_some_and(|(c, by)| !(by == "auto" && c == &["other"])) {
        return false;
    }
    let (cats, _) = decide(config, part, path, page, who.agent.as_deref());
    if now == Some((cats.clone(), "auto".into())) {
        return false;
    }
    set_front(page, &cats, "auto");
    true
}

// -- dates ----------------------------------------------------------------------------------------------------------

/// Whether a day is in « Dates »: from 90 days ago to two years ahead.
pub fn in_window(day: NaiveDate, today: NaiveDate) -> bool {
    let n = (day - today).num_days();
    (-DATED_BEFORE..=DATED_AFTER).contains(&n)
}

/// Dutch months (the Carnet's other dates have theirs in mod.rs `MONTHS`), and « 23 de diciembre » said the Spanish
/// way: read as « 23 diciembre » here only (the « Dates à venir » of today keep their own reading).
static SPOKEN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(\d{1,2})\s+de\s+([a-z]{3,10})\b").expect("valid regex"));
static DUTCH_MONTHS: LazyLock<Vec<(regex::Regex, &'static str)>> = LazyLock::new(|| {
    [("januari", "january"), ("februari", "february"), ("maart", "march"), ("mei", "may"), ("augustus", "august")]
        .into_iter()
        .map(|(nl, en)| (regex::Regex::new(&format!(r"\b{nl}\b")).expect("valid regex"), en))
        .collect()
});

/// The date a text carries, a yearly one (a birthday, a date without a year) at its next occurrence.
pub fn date_of(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let mut plain = tk::ascii_fold(&text.to_lowercase());
    plain = SPOKEN.replace_all(&plain, "$1 $2").into_owned();
    for (nl, en) in DUTCH_MONTHS.iter() {
        plain = nl.replace_all(&plain, *en).into_owned();
    }
    let (month, day, year) = super::date_in(&plain)?;
    let yearly = year.is_none() || super::YEARLY.is_match(text);
    if yearly {
        let this = NaiveDate::from_ymd_opt(today.year(), month, day);
        match this {
            Some(d) if d >= today => Some(d),
            _ => NaiveDate::from_ymd_opt(today.year() + 1, month, day),
        }
    } else {
        NaiveDate::from_ymd_opt(year?, month, day)
    }
}

pub fn dated(text: &str, today: NaiveDate) -> bool {
    date_of(text, today).is_some_and(|d| in_window(d, today))
}

/// `categories` with `dates` when the item is dated.
pub fn with_dates(categories: &[String], dated: bool) -> Vec<String> {
    let mut out = categories.to_vec();
    if dated && !out.iter().any(|c| c == "dates") {
        out.push("dates".into());
    }
    out
}

// -- records, tasks, documents, lines --------------------------------------------------------------------------------

/// The person's corrections of records, tasks, documents and lines (`category_marks`).
pub fn marks(config: &Config) -> Dict {
    marks_in(&super::read_state(config))
}

fn marks_in(state: &Dict) -> Dict {
    state.get("category_marks").and_then(Value::as_object).cloned().unwrap_or_default()
}

fn marked(marks: &Dict, key: &str) -> Option<Vec<String>> {
    valid(&super::strings(marks.get(key).and_then(|m| m.get("categories"))), &PERSON_MAY_GIVE)
}

pub fn record_key(id: &str) -> String {
    format!("record:{id}")
}

pub fn task_key(id: &str) -> String {
    format!("task:{id}")
}

pub fn document_key(agent: &str, file: &str) -> String {
    format!("doc:{agent}/{file}")
}

/// A line's key: its page (its id, else its path) and its text folded.
pub fn line_key(page: &str, text: &str) -> String {
    let (text, _) = super::split_attribution(text.trim().trim_start_matches("- "));
    format!("line:{page}:{}", &super::rev(&fold(&text))[..10])
}

/// A record's categories (by its type; a purchase with a warranty also Papiers), by whom, and whether it is dated.
pub fn of_record(t: &records::Kind, record: &Dict, marks: &Dict, today: NaiveDate) -> (Vec<String>, String, bool) {
    let dated = t
        .fields
        .iter()
        .filter(|f| records::is_date_field(f.id))
        .any(|f| dated(&tk::str_or_empty(record.get(f.id)), today));
    if let Some(cats) = marked(marks, &record_key(&tk::str_or_empty(record.get("id")))) {
        return (cats, "person".into(), dated);
    }
    let cats = if t.id == "purchase" && tk::str_or_empty(record.get("warranty_end")).trim().is_empty() {
        vec!["money".to_string()]
    } else {
        of_records_type(Some(t.id))
    };
    (cats, "auto".into(), dated)
}

/// The page a task belongs to: its first `[[link]]`.
fn task_page<'a>(task: &Task, pages: &'a [(String, Page)]) -> Option<&'a (String, Page)> {
    let link = task.links.first()?;
    pages.iter().find(|(path, p)| p.named(link) || std::path::Path::new(path).file_stem().is_some_and(|s| s.to_string_lossy() == *link))
}

/// A task's categories: its page's (none: « Autres »), the person's own choice first, `dates` when it is due within
/// the window.
pub fn of_task(config: &Config, task: &Task, pages: &[(String, Page)], marks: &Dict, today: NaiveDate) -> Vec<String> {
    let cats = marked(marks, &task_key(&task.id)).unwrap_or_else(|| match task_page(task, pages) {
        Some((path, page)) => of_page(config, task.part, path, page).categories,
        None if task.part == Part::Health => vec!["health".into()],
        None => vec!["other".into()],
    });
    let due = NaiveDate::parse_from_str(&task.due, "%Y-%m-%d").ok();
    with_dates(&cats, due.is_some_and(|d| in_window(d, today)))
}

/// A document's categories (GET /v1/notebook/documents' item), by whom, and the word of its name that decided: the
/// person's choice; the page it is linked to or named on; its name's words; else Papiers (Alex's decision 4).
pub fn of_document(config: &Config, doc: &Value, pages: &[(String, Page)], marks: &Dict) -> (Vec<String>, String, Option<String>) {
    let key = document_key(&tk::str_or_empty(doc.get("agent")), &tk::str_or_empty(doc.get("file")));
    if let Some(cats) = marked(marks, &key) {
        return (cats, "person".into(), None);
    }
    let health = tk::str_or_empty(doc.get("part")) == "health";
    let linked = doc.get("linked").and_then(|l| l.get("path")).and_then(Value::as_str).map(str::to_string);
    let named = doc.get("subjects").and_then(Value::as_array).into_iter().flatten();
    let named = named.filter_map(|s| s.get("path").and_then(Value::as_str).map(str::to_string));
    for path in linked.into_iter().chain(named) {
        if let Some((path, page)) = pages.iter().find(|(p, _)| *p == path) {
            let part = if health { Part::Health } else { Part::Main };
            return (of_page(config, part, path, page).categories, "auto".into(), None);
        }
    }
    let name = tk::str_or_empty(doc.get("name"));
    let stem = std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name);
    let named = Page::new("subject", &stem);
    match by_words(&named, &super::language(config), &rules(config)) {
        (cats, word) if cats != ["other"] => (cats, "auto".into(), word),
        _ => (vec!["papers".into()], "auto".into(), None),
    }
}

/// A line's categories: its page's (`page_categories`), the person's own choice first, `dates` when it carries a date.
pub fn of_line(page_categories: &[String], page: &str, text: &str, marks: &Dict, today: NaiveDate) -> Vec<String> {
    let cats = marked(marks, &line_key(page, text)).unwrap_or_else(|| page_categories.to_vec());
    // the line's own date, never its attribution's (« — *Léa, 2026-10-22* »)
    let (plain, _) = super::split_attribution(text.trim().trim_start_matches("- "));
    with_dates(&cats, dated(&plain, today))
}

/// The documents' list with each one's categories (`categories`, `categories_by`), for the apps.
pub fn with_documents(config: &Config, documents: Vec<Value>, part: Part) -> Vec<Value> {
    let pages = super::all_pages(config, part);
    let marks = marks(config);
    documents
        .into_iter()
        .map(|mut d| {
            let (cats, by, _) = if part == Part::Health {
                (vec!["health".to_string()], "auto".to_string(), None)
            } else {
                of_document(config, &d, &pages, &marks)
            };
            if let Some(o) = d.as_object_mut() {
                o.insert("categories".into(), json!(cats));
                o.insert("categories_by".into(), json!(by));
            }
            d
        })
        .collect()
}

/// A task's view with its categories.
pub fn task_view(config: &Config, task: &Task, pages: &[(String, Page)], marks: &Dict, today: NaiveDate) -> Value {
    let mut v = task.view();
    v["categories"] = json!(of_task(config, task, pages, marks, today));
    v
}

// -- the person's correction (« Ranger ailleurs ») -------------------------------------------------------------------

/// What the person files elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Page { path: String },
    Record { kind: String, id: String },
    Document { agent: String, file: String },
    Task(String),
    Line { path: String, text: String },
}

impl Target {
    pub fn view(&self) -> Value {
        match self {
            Target::Page { path } => json!({"path": path, "private": false}),
            Target::Record { kind, id } => json!({"record": {"type": kind, "id": id}}),
            Target::Document { agent, file } => json!({"document": {"agent": agent, "file": file}}),
            Target::Task(id) => json!({"task": id}),
            Target::Line { path, text } => json!({"line": {"path": path, "text": text}}),
        }
    }

    fn from_view(v: &Value) -> Option<Target> {
        if let Some(path) = v.get("path").and_then(Value::as_str) {
            return Some(Target::Page { path: path.into() });
        }
        if let Some(r) = v.get("record") {
            return Some(Target::Record { kind: tk::str_or_empty(r.get("type")), id: tk::str_or_empty(r.get("id")) });
        }
        if let Some(d) = v.get("document") {
            return Some(Target::Document { agent: tk::str_or_empty(d.get("agent")), file: tk::str_or_empty(d.get("file")) });
        }
        if let Some(t) = v.get("task").and_then(Value::as_str) {
            return Some(Target::Task(t.into()));
        }
        let l = v.get("line")?;
        Some(Target::Line { path: tk::str_or_empty(l.get("path")), text: tk::str_or_empty(l.get("text")) })
    }
}

/// Why a correction is refused (the route's codes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refused {
    /// Not a category.
    Unknown,
    /// None, or more than two.
    Count,
    /// `dates` (computed) or `health` (the private part).
    NotGiven,
    /// No such page, record, task, document or line.
    NotFound,
}

impl Refused {
    pub fn code(&self) -> &'static str {
        match self {
            Refused::Unknown => "category_unknown",
            Refused::Count => "category_count",
            Refused::NotGiven => "category_not_given",
            Refused::NotFound => "not_found",
        }
    }
}

fn checked(given: &[String]) -> Result<Vec<String>, Refused> {
    let given: Vec<String> = given.iter().map(|c| c.trim().to_lowercase()).collect();
    if given.iter().any(|c| !PERSONAL.contains(&c.as_str())) {
        return Err(Refused::Unknown);
    }
    if given.iter().any(|c| !PERSON_MAY_GIVE.contains(&c.as_str())) {
        return Err(Refused::NotGiven);
    }
    valid(&given, &PERSON_MAY_GIVE).ok_or(Refused::Count)
}

fn line_on(page: &Page, text: &str) -> Option<String> {
    let wanted = fold(&super::split_attribution(text.trim().trim_start_matches("- ")).0);
    page.sections().into_iter().flat_map(|(_, f)| f).find(|f| fold(&f.text) == wanted).map(|f| f.text)
}

/// « Ranger ailleurs »: `target` filed in `given` (1 or 2 of `PERSON_MAY_GIVE`) by the person. A page gets
/// `categories_by: person` (and, when the bridge had decided by a word, the rule « word → these categories » for the
/// next writes); a record, task, document or line a mark in the state. The answer says what it was and how to undo.
pub fn categorize(config: &Config, target: &Target, given: &[String], who: &Who) -> anyhow::Result<Result<Value, Refused>> {
    let cats = match checked(given) {
        Ok(c) => c,
        Err(r) => return Ok(Err(r)),
    };
    let _guard = super::lock();
    let today = chrono::Local::now().date_naive();
    let marks = marks(config);
    // what it was, its mark's key (none for a page), the word that decided it
    let (before, key, word): (Filed, Option<String>, Option<String>) = match target {
        Target::Page { path } => {
            if super::checked(path).is_err() {
                return Ok(Err(Refused::NotFound));
            }
            let Some(text) = super::read_page_text(config, Part::Main, path) else { return Ok(Err(Refused::NotFound)) };
            let mut page = Page::parse(&text, "");
            let before = of_page(config, Part::Main, path, &page);
            let (auto, word) = decide(config, Part::Main, path, &page, None);
            set_front(&mut page, &cats, "person");
            let after = page.render();
            if after != text {
                super::commit(config, Part::Main, path, Some(&text), Some(&after), who, ACTION, &cats.join(", "))?;
            }
            (before, None, word.filter(|_| auto != cats))
        }
        Target::Record { kind, id } => {
            let Some(t) = records::kind(kind) else { return Ok(Err(Refused::NotFound)) };
            let all = records::records(config, t).into_iter().chain(records::archived_records(config, t));
            let Some(record) = all.into_iter().find(|r| tk::str_or_empty(r.get("id")) == *id) else {
                return Ok(Err(Refused::NotFound));
            };
            let (c, by, _) = of_record(t, &record, &marks, today);
            (Filed { categories: c, stored: by == "person", by }, Some(record_key(id)), None)
        }
        Target::Document { agent, file } => {
            let docs = super::documents(config, Part::Main);
            let Some(doc) = docs.iter().find(|d| tk::str_or_empty(d.get("agent")) == *agent && tk::str_or_empty(d.get("file")) == *file) else {
                return Ok(Err(Refused::NotFound));
            };
            let pages = super::all_pages(config, Part::Main);
            let (c, by, word) = of_document(config, doc, &pages, &marks);
            let word = word.filter(|_| c != cats);
            (Filed { categories: c, stored: by == "person", by }, Some(document_key(agent, file)), word)
        }
        Target::Task(id) => {
            let Some(task) = super::tasks(config, Part::Main).into_iter().find(|t| t.id == *id) else {
                return Ok(Err(Refused::NotFound));
            };
            let pages = super::all_pages(config, Part::Main);
            let c: Vec<String> = of_task(config, &task, &pages, &marks, today).into_iter().filter(|c| c != "dates").collect();
            let by = if marked(&marks, &task_key(id)).is_some() { "person" } else { "auto" };
            (Filed { categories: c, stored: by == "person", by: by.into() }, Some(task_key(id)), None)
        }
        Target::Line { path, text } => {
            let Some(page_text) = super::checked(path).ok().and_then(|_| super::read_page_text(config, Part::Main, path)) else {
                return Ok(Err(Refused::NotFound));
            };
            let page = Page::parse(&page_text, "");
            let Some(line) = line_on(&page, text) else { return Ok(Err(Refused::NotFound)) };
            let id = if page.id().is_empty() { path.clone() } else { page.id() };
            let key = line_key(&id, &line);
            let c = marked(&marks, &key).unwrap_or_else(|| of_page(config, Part::Main, path, &page).categories);
            let by = if marked(&marks, &key).is_some() { "person" } else { "auto" };
            (Filed { categories: c, stored: by == "person", by: by.into() }, Some(key), None)
        }
    };
    let undo = format!("u{}", crate::py::token_hex(6));
    let learnt = word.as_ref().map(|w| json!({"word": w, "categories": cats}));
    super::update_state(config, |state| {
        let rule_before = word.as_ref().and_then(|w| rules_in(state).into_iter().find(|r| r.word == *w)).map(|r| r.categories);
        if let Some(w) = &word {
            let mut list: Vec<Value> = state.get("category_rules").and_then(Value::as_array).cloned().unwrap_or_default();
            list.retain(|r| fold(&tk::str_or_empty(r.get("word"))) != *w);
            list.insert(0, json!({"word": w, "categories": cats, "at": super::today()}));
            list.truncate(RULES_KEPT);
            state.insert("category_rules".into(), Value::Array(list));
        }
        if let Some(key) = &key {
            let mut m = marks_in(state);
            m.shift_remove(key);
            m.insert(key.clone(), json!({"categories": cats, "at": super::today()}));
            if m.len() > MARKS_KEPT {
                let extra = m.len() - MARKS_KEPT;
                m = m.into_iter().skip(extra).collect();
            }
            state.insert("category_marks".into(), Value::Object(m));
        }
        let mut list: Vec<Value> = state.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
        list.insert(0, json!({"id": undo, "target": target.view(), "before": {"categories": before.categories,
                              "categories_by": before.by, "stored": before.stored},
                              "rule": word, "rule_before": rule_before, "at": super::today()}));
        list.truncate(UNDO_KEPT);
        state.insert("category_undo".into(), Value::Array(list));
    })?;
    Ok(Ok(json!({"target": target.view(), "categories": cats, "categories_by": "person",
                 "before": {"categories": before.categories, "categories_by": before.by},
                 "learnt": learnt, "undo": undo, "toast": Value::Null})))
}

/// The history's action of a correction.
pub const ACTION: &str = "categorize";

/// « Annuler »: the correction `id` undone (the categories as they were, the rule it taught forgotten). None: no
/// such correction any more (20 newer ones, or undone already).
pub fn undo(config: &Config, id: &str, who: &Who) -> anyhow::Result<Option<Value>> {
    let _guard = super::lock();
    let state = super::read_state(config);
    let list: Vec<Value> = state.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(entry) = list.iter().find(|e| tk::str_or_empty(e.get("id")) == id).cloned() else { return Ok(None) };
    let Some(target) = entry.get("target").and_then(Target::from_view) else { return Ok(None) };
    let before = &entry["before"];
    let was = valid(&super::strings(before.get("categories")), &PERSON_MAY_GIVE).unwrap_or_else(|| vec!["other".into()]);
    let was_by = tk::str_or_empty(before.get("categories_by"));
    let stored = before.get("stored").and_then(Value::as_bool).unwrap_or(false);
    let mut now = Filed { categories: was.clone(), by: if was_by.is_empty() { "auto".into() } else { was_by.clone() }, stored };
    let rule = entry.get("rule").and_then(Value::as_str).map(str::to_string);
    let rule_before = valid(&super::strings(entry.get("rule_before")), &PERSON_MAY_GIVE);
    super::update_state(config, |state| {
        let key = match &target {
            Target::Page { .. } => None,
            Target::Record { id, .. } => Some(record_key(id)),
            Target::Document { agent, file } => Some(document_key(agent, file)),
            Target::Task(id) => Some(task_key(id)),
            Target::Line { path, text } => {
                let page = super::read_page_text(config, Part::Main, path).map(|t| Page::parse(&t, "")).unwrap_or_default();
                let id = if page.id().is_empty() { path.clone() } else { page.id() };
                Some(line_key(&id, text))
            }
        };
        if let Some(key) = key {
            let mut m = marks_in(state);
            if stored {
                m.insert(key, json!({"categories": was, "at": super::today()}));
            } else {
                m.shift_remove(&key);
            }
            state.insert("category_marks".into(), Value::Object(m));
        }
        if let Some(w) = &rule {
            let mut list: Vec<Value> = state.get("category_rules").and_then(Value::as_array).cloned().unwrap_or_default();
            list.retain(|r| fold(&tk::str_or_empty(r.get("word"))) != *w);
            if let Some(c) = &rule_before {
                list.insert(0, json!({"word": w, "categories": c, "at": super::today()}));
            }
            state.insert("category_rules".into(), Value::Array(list));
        }
        let mut list: Vec<Value> = state.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
        list.retain(|e| tk::str_or_empty(e.get("id")) != id);
        state.insert("category_undo".into(), Value::Array(list));
    })?;
    // the page as it was, read once the rule is forgotten (a page the bridge had not written categories on is read
    // with its words again)
    if let Target::Page { path } = &target {
        if let Some(text) = super::read_page_text(config, Part::Main, path) {
            let mut page = Page::parse(&text, "");
            if stored {
                set_front(&mut page, &was, &now.by);
            } else {
                clear_front(&mut page);
            }
            let after = page.render();
            if after != text {
                super::commit(config, Part::Main, path, Some(&text), Some(&after), who, ACTION, &was.join(", "))?;
            }
            now = of_page(config, Part::Main, path, &page);
        }
    }
    Ok(Some(json!({"target": target.view(), "categories": now.categories, "categories_by": now.by, "unlearnt": rule})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::{tests_config, update_state};

    fn config_in(dir: &tempfile::TempDir, lang: &str) -> Config {
        let config = tests_config(dir.path());
        update_state(&config, |s| {
            s.insert("language".into(), json!(if lang == "nl" { "en" } else { lang }));
        })
        .unwrap();
        config
    }

    /// A page as the bridge writes it in a French Carnet (`summary`: its « En bref »).
    fn page(kind: &str, title: &str, summary: &str, body: &str) -> Page {
        page_in("fr", kind, title, summary, body)
    }

    fn page_in(lang: &str, kind: &str, title: &str, summary: &str, body: &str) -> Page {
        let brief = crate::notebook::words(if lang == "nl" { "en" } else { lang }).brief;
        let mut p = Page::new(kind, title);
        p.body = if summary.is_empty() { body.to_string() } else { format!("## {brief}\n{summary}\n\n{body}") };
        p
    }

    fn decided(config: &Config, p: &Page) -> Vec<String> {
        decide(config, Part::Main, "Sujets/x.md", p, None).0
    }

    /// The demo's subjects and areas, in the five languages (App/Resources/Demo, Dutch from the bench's fixture).
    #[test]
    fn each_language_files_the_demo_subjects_and_areas() {
        let budget = "## Chiffres et budget\n- Budget : 15 000 € au maximum\n";
        let cases: [(&str, &str, &str, &str, &[&str]); 45] = [
            ("fr", "subject", "Rénovation cuisine", "Devis de Marc reçu : 8 900 €.", &["home"]),
            ("en", "subject", "Kitchen renovation", "Marc’s quote is in: €8,900.", &["home"]),
            ("de", "subject", "Küchenrenovierung", "Marcs Angebot ist da: 8.900 €.", &["home"]),
            ("es", "subject", "Reforma de la cocina", "Llegó el presupuesto de Marc: 8.900 €.", &["home"]),
            ("nl", "subject", "Keukenrenovatie", "De offerte van Marc is binnen: € 8.900.", &["home"]),
            ("fr", "subject", "Voyage Japon", "Avril, 12 jours : Tokyo, Kyoto, Osaka.", &["trips"]),
            ("en", "subject", "Japan trip", "April, 12 days: Tokyo, Kyoto, Osaka.", &["trips"]),
            ("de", "subject", "Japanreise", "April, 12 Tage: Tokio, Kyoto, Osaka.", &["trips"]),
            ("es", "subject", "Viaje a Japón", "Abril, 12 días: Tokio, Kioto, Osaka.", &["trips"]),
            ("nl", "subject", "Reis naar Japan", "April, 12 dagen: Tokio, Kyoto, Osaka.", &["trips"]),
            ("fr", "subject", "Anniversaire d’Inès", "Inès fête ses 8 ans samedi.", &["people"]),
            ("en", "subject", "Inès’s birthday", "Inès turns 8 on Saturday.", &["people"]),
            ("de", "subject", "Inès’ Geburtstag", "Inès wird am Samstag 8.", &["people"]),
            ("es", "subject", "Cumpleaños de Inès", "Inès cumple 8 años el sábado.", &["people"]),
            ("nl", "subject", "Verjaardag van Inès", "Inès wordt 8 op zaterdag.", &["people"]),
            ("fr", "subject", "Impôts 2026", "Déclaration en ligne à vérifier.", &["money", "papers"]),
            ("en", "subject", "Taxes 2026", "Online tax return to check.", &["money", "papers"]),
            ("de", "subject", "Steuern 2026", "Die Online-Steuererklärung prüfen.", &["money", "papers"]),
            ("es", "subject", "Impuestos 2026", "Revisar la declaración en línea.", &["money", "papers"]),
            ("nl", "subject", "Belastingen 2026", "Online belastingaangifte nakijken.", &["money", "papers"]),
            ("fr", "subject", "Changer de voiture", "Une hybride d’occasion, 15 000 € au maximum.", &["money"]),
            ("en", "subject", "New car", "A used hybrid, €15,000 at most.", &["money"]),
            ("de", "subject", "Neues Auto", "Ein gebrauchter Hybrid, höchstens 15.000 €.", &["money"]),
            ("es", "subject", "Cambiar de coche", "Un híbrido de segunda mano, 15.000 € como máximo.", &["money"]),
            ("nl", "subject", "Nieuwe auto", "Een tweedehands hybride, maximaal € 15.000.", &["money"]),
            ("fr", "area", "Famille", "Ta sœur Claire et ses enfants.", &["people"]),
            ("en", "area", "Family", "Your sister Claire and her children.", &["people"]),
            ("de", "area", "Familie", "Deine Schwester Claire und ihre Kinder.", &["people"]),
            ("es", "area", "Familia", "Tu hermana Claire y sus hijos.", &["people"]),
            ("nl", "area", "Gezin", "Je zus Claire en haar kinderen.", &["people"]),
            ("fr", "area", "Maison", "L’appartement, les travaux de la cuisine et les voisins.", &["home"]),
            ("en", "area", "Home", "The flat, the kitchen work and the neighbours.", &["home"]),
            ("de", "area", "Zuhause", "Die Wohnung, die Küchenarbeiten und die Nachbarn.", &["home"]),
            ("es", "area", "Casa", "El piso, la obra de la cocina y los vecinos.", &["home"]),
            ("nl", "area", "Huis", "Het appartement, de keukenverbouwing en de buren.", &["home"]),
            ("fr", "area", "Finances", "Le budget du mois et les contrats à renouveler.", &["money"]),
            ("en", "area", "Money", "The month’s budget and the contracts to renew.", &["money"]),
            ("de", "area", "Finanzen", "Das Monatsbudget und die Verträge zum Verlängern.", &["money"]),
            ("es", "area", "Finanzas", "El presupuesto del mes y los contratos por renovar.", &["money"]),
            ("nl", "area", "Geld", "Het budget van de maand en de contracten.", &["money"]),
            ("fr", "area", "Travail", "Ton poste chez Atelier Lumière et ton entretien annuel.", &["work"]),
            ("en", "area", "Work", "Your job at Atelier Lumière and your annual review.", &["work"]),
            ("de", "area", "Arbeit", "Deine Stelle bei Atelier Lumière und dein Jahresgespräch.", &["work"]),
            ("es", "area", "Trabajo", "Tu puesto en Atelier Lumière y tu entrevista anual.", &["work"]),
            ("nl", "area", "Werk", "Je baan bij Atelier Lumière en je jaargesprek.", &["work"]),
        ];
        for (lang, kind, title, summary, want) in cases {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            let body = if title.contains("voiture") || title.contains("car") || title.contains("Auto")
                || title.contains("coche") || title.contains("auto")
            {
                budget
            } else {
                ""
            };
            let got = decided(&config, &page_in(lang, kind, title, summary, body));
            assert_eq!(got, want, "{lang} {title}");
        }
    }

    #[test]
    fn other_words_and_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let one = |title: &str, summary: &str| decided(&config, &page("subject", title, summary, ""));
        assert_eq!(one("Assurance auto", ""), ["papers"]);
        assert_eq!(one("Factures EDF", ""), ["papers", "money"]);
        assert_eq!(one("Lettre au syndic", ""), ["home", "papers"]);
        assert_eq!(one("Abonnement salle de sport", ""), ["money"]);
        assert_eq!(one("Lectures de l’hiver", "Romans à lire"), ["other"]);
        assert_eq!(one("Jazz", ""), ["other"]);
        assert_eq!(one("Lessen Nederlands", ""), ["other"]);
        // a health word files nothing: the shared Carnet's line stays where its page is (decision 6)
        assert_eq!(one("Dentiste", ""), ["other"]);
    }

    #[test]
    fn the_kind_decides_before_the_words() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let at = |part: Part, p: &Page| decide(&config, part, "x.md", p, None).0;
        assert_eq!(at(Part::Main, &page("person", "Marc Durand", "Ton beau-frère, menuisier.", "")), ["people"]);
        assert_eq!(at(Part::Main, &page("person", "Paul Martin", "Ton collègue chez Atelier Lumière.", "")), ["people", "work"]);
        assert_eq!(at(Part::Main, &page("self", "Toi", "Tu vis à Nantes, tu voyages beaucoup.", "")), ["people"]);
        assert_eq!(at(Part::Main, &page("trip", "Week-end à Lisbonne", "Budget : 600 €", "")), ["trips"]);
        assert_eq!(at(Part::Main, &page("area", "Amis", "", "")), ["people"]);
        assert_eq!(at(Part::Main, &page("area", "Famille élargie", "", "")), ["people"]);
        assert_eq!(at(Part::Main, &page("area", "Collègues", "", "")), ["work", "people"]);
        assert_eq!(at(Part::Health, &page("area", "Santé", "Allergie à la pénicilline", "")), ["health"]);
        let mut records = Page::new("records", "Achats");
        records.front.insert("records".into(), json!("purchase"));
        assert_eq!(at(Part::Main, &records), ["money", "papers"]);
        records.front.insert("records".into(), json!("subscription"));
        assert_eq!(at(Part::Main, &records), ["money"]);
        records.front.insert("records".into(), json!("contract"));
        assert_eq!(at(Part::Main, &records), ["papers"]);
        records.front.insert("records".into(), json!("booking"));
        assert_eq!(at(Part::Main, &records), ["trips"]);
        let tasks = Page::new("tasks", "Tâches");
        assert_eq!(at(Part::Main, &tasks), ["other"]);
        // a page a work agent makes (its group « Pro »): Travail
        let pro = Page::new("subject", "Lancement");
        assert_eq!(decide(&config, Part::Main, "Sujets/Lancement.md", &pro, None).0, ["other"]);
    }

    #[test]
    fn dates_are_computed_from_the_texts() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        assert!(dated("Anniversaire : 14 mars", today));
        assert_eq!(date_of("Anniversaire : 14 mars 1990", today), NaiveDate::from_ymd_opt(2027, 3, 14));
        assert!(dated("Entretien annuel le 27 novembre", today));
        assert!(dated("Vol le 2027-03-01", today));
        assert!(dated("Garantie jusqu’au 2028-10-01", today));
        assert!(dated("Rendu le 2026-09-01", today));
        assert!(!dated("Acheté le 2024-01-10", today));
        assert!(!dated("Expire le 2030-01-01", today));
        assert!(!dated("Poubelles le lundi soir", today));
        assert!(!dated("Avril, 12 jours : Tokyo, Kyoto", today));
        assert!(dated("Cumpleaños: 23 de diciembre", today));
        assert!(dated("Verjaardag : 14 maart", today));
        assert!(dated("Jaargesprek op 27 november", today));
        assert_eq!(with_dates(&["people".to_string()], true), ["people", "dates"]);
        assert_eq!(with_dates(&["people".to_string()], false), ["people"]);
    }

    #[test]
    fn a_page_keeps_its_categories_an_agent_may_say_one_and_the_person_wins() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let (agent, you) = (Who::agent(&config, "home"), Who::you(&config));
        let path = "Sujets/Rénovation cuisine.md";
        let mut p = page("subject", "Rénovation cuisine", "Devis reçu.", "");
        assert!(on_write(&config, Part::Main, path, &mut p, &agent));
        assert_eq!(stored(&p), Some((vec!["home".to_string()], "auto".to_string())));
        // filed already: kept, whatever its words become
        p.body = "## En bref\nLe budget : 9 000 €, le prêt de la banque.\n".into();
        assert!(!on_write(&config, Part::Main, path, &mut p, &agent));
        // an agent says it (never dates, health or other, two at most, only an agent)
        for wrong in [vec!["dates"], vec!["health"], vec!["other"], vec!["money", "home", "work"], vec![]] {
            let wrong: Vec<String> = wrong.into_iter().map(String::from).collect();
            assert!(!choose(&mut p, &wrong, &agent), "{wrong:?}");
        }
        assert!(!choose(&mut p, &["money".to_string()], &you));
        assert!(choose(&mut p, &["Money".to_string(), "home".to_string()], &agent));
        assert_eq!(stored(&p), Some((vec!["money".to_string(), "home".to_string()], "agent".to_string())));
        // the person's choice: no agent and no write changes it
        set_front(&mut p, &["home".to_string()], "person");
        assert!(!choose(&mut p, &["money".to_string()], &agent));
        assert!(!on_write(&config, Part::Main, path, &mut p, &agent));
        // in the frontmatter, before `updated`: readable in Obsidian, read back the same
        p.touch();
        let text = p.render();
        assert!(text.contains("categories:\n  - home\ncategories_by: person\nupdated: "), "{text}");
        assert_eq!(stored(&Page::parse(&text, "")), Some((vec!["home".to_string()], "person".to_string())));
        // filed in « Autres » by the bridge: filed again at the next write
        let mut q = page("subject", "Divers", "", "");
        assert!(on_write(&config, Part::Main, "Sujets/Divers.md", &mut q, &agent));
        assert_eq!(stored(&q).unwrap().0, ["other"]);
        q.body = "## En bref\nLe devis du plombier pour la salle de bain.\n".into();
        assert!(on_write(&config, Part::Main, "Sujets/Divers.md", &mut q, &agent));
        assert_eq!(stored(&q).unwrap().0, ["home"]);
        // a hand-written value that is not a category: read as the bridge's
        let mut r = page("subject", "Voyage Japon", "", "");
        r.front.insert("categories".into(), json!(["maison"]));
        assert_eq!(stored(&r), None);
        assert_eq!(of_page(&config, Part::Main, "Sujets/Voyage Japon.md", &r).categories, ["trips"]);
        // the private part is Santé: nothing written there
        let mut h = page("area", "Santé", "", "");
        assert!(!on_write(&config, Part::Health, "Domaines/Santé.md", &mut h, &agent));
        assert_eq!(of_page(&config, Part::Health, "Domaines/Santé.md", &h).categories, ["health"]);
    }

    #[test]
    fn a_correction_wins_teaches_the_next_write_and_undo_forgets_it() {
        use crate::notebook::{note, read_page_text, recent_changes};
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let (agent, you) = (Who::agent(&config, "home"), Who::you(&config));
        let facts = |f: &str| vec![f.to_string()];
        note(&config, Part::Main, "subject", "Rénovation cuisine", "status", &facts("Devis reçu"), &[], "", "", &agent).unwrap();
        let path = "Sujets/Rénovation cuisine.md".to_string();
        let read = |p: &str| Page::parse(&read_page_text(&config, Part::Main, p).unwrap(), "");
        assert_eq!(stored(&read(&path)), Some((vec!["home".to_string()], "auto".to_string())));
        let page = Target::Page { path: path.clone() };
        let give = |c: &[&str]| c.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(categorize(&config, &page, &give(&["dates"]), &you).unwrap(), Err(Refused::NotGiven));
        assert_eq!(categorize(&config, &page, &give(&["health"]), &you).unwrap(), Err(Refused::NotGiven));
        assert_eq!(categorize(&config, &page, &give(&["maison"]), &you).unwrap(), Err(Refused::Unknown));
        assert_eq!(categorize(&config, &page, &give(&[]), &you).unwrap(), Err(Refused::Count));
        assert_eq!(categorize(&config, &page, &give(&["home", "money", "work"]), &you).unwrap(), Err(Refused::Count));
        let nowhere = Target::Page { path: "Sujets/Rien.md".into() };
        assert_eq!(categorize(&config, &nowhere, &give(&["home"]), &you).unwrap(), Err(Refused::NotFound));
        let done = categorize(&config, &page, &give(&["money"]), &you).unwrap().unwrap();
        assert_eq!(done["before"], json!({"categories": ["home"], "categories_by": "auto"}));
        assert_eq!(done["learnt"], json!({"word": "renovation", "categories": ["money"]}));
        assert_eq!((done["categories"].clone(), done["categories_by"].clone()), (json!(["money"]), json!("person")));
        assert_eq!(stored(&read(&path)), Some((vec!["money".to_string()], "person".to_string())));
        // an agent writes there again, even saying another category: the person's stays
        crate::notebook::note_with(&config, Part::Main, "subject", "Rénovation cuisine", "status", &facts("Travaux en mars"),
                                   &[], "", "", &give(&["home"]), &agent).unwrap();
        assert_eq!(stored(&read(&path)), Some((vec!["money".to_string()], "person".to_string())));
        // the next page that word decides goes where the person put the first one
        note(&config, Part::Main, "subject", "Rénovation salle de bain", "status", &facts("Devis demandé"), &[], "", "", &agent).unwrap();
        assert_eq!(stored(&read("Sujets/Rénovation salle de bain.md")).unwrap().0, ["money"]);
        assert_eq!(rules(&config), [Rule { word: "renovation".into(), categories: vec!["money".into()] }]);
        // not news: never in « Nouveau »
        assert!(recent_changes(&config, Part::Main, 20).iter().all(|e| e["action"] != ACTION));
        // undone: as it was, the rule forgotten; once
        let undone = undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!((undone["categories"].clone(), undone["categories_by"].clone()), (json!(["home"]), json!("auto")));
        assert_eq!(undone["unlearnt"], json!("renovation"));
        assert_eq!(stored(&read(&path)), Some((vec!["home".to_string()], "auto".to_string())));
        assert!(rules(&config).is_empty());
        assert!(undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().is_none());
        // a page written without categories (by hand, or before the sommaire): undone, it is read with its words again
        let old = "Sujets/Rénovation garage.md";
        crate::notebook::write_file(&crate::notebook::part_root(&config, Part::Main).join(old),
                                    "---\ntype: subject\n---\n# Rénovation garage\n").unwrap();
        let done = categorize(&config, &Target::Page { path: old.into() }, &give(&["work"]), &you).unwrap().unwrap();
        assert_eq!(done["before"], json!({"categories": ["home"], "categories_by": "auto"}));
        let undone = undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!((undone["categories"].clone(), undone["categories_by"].clone()), (json!(["home"]), json!("auto")));
        assert_eq!(stored(&read(old)), None);
    }

    #[test]
    fn records_tasks_documents_and_lines_follow_their_page_or_type() {
        use crate::notebook::{add_task, all_pages, note, records as rec};
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let (agent, you) = (Who::agent(&config, "home"), Who::you(&config));
        let today = chrono::Local::now().date_naive();
        let day = |n: i64| (today + chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
        let fields = |pairs: &[(&str, String)]| pairs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Dict>();
        let purchase = rec::kind("purchase").unwrap();
        let (with, _) = rec::upsert(&config, purchase, "", &fields(&[("item", "Lave-linge".into()), ("date", day(-100)),
                                                                     ("warranty_end", day(600))]), false, &agent).unwrap();
        let (without, _) = rec::upsert(&config, purchase, "", &fields(&[("item", "Places de cinéma".into()), ("date", day(-400)),
                                                                        ("class", "ticket".into())]), false, &agent).unwrap();
        let none = Dict::new();
        assert_eq!(of_record(purchase, &with, &none, today), (vec!["money".into(), "papers".into()], "auto".into(), true));
        assert_eq!(of_record(purchase, &without, &none, today).0, ["money"]);
        let booking = rec::kind("booking").unwrap();
        let (vol, _) = rec::upsert(&config, booking, "", &fields(&[("date", format!("{} 10:30", day(12))), ("what", "Vol Lisbonne".into())]),
                                   false, &agent).unwrap();
        assert_eq!(of_record(booking, &vol, &none, today), (vec!["trips".into()], "auto".into(), true));
        // tasks: their page's, else « Autres »; Dates when due
        note(&config, Part::Main, "subject", "Voyage Japon", "next", &["Réserver les vols".to_string()], &[], "", "", &agent).unwrap();
        let linked = add_task(&config, Part::Main, "Réserver les vols", &day(4), "Voyage Japon", "asked", "", &agent).unwrap().task;
        let alone = add_task(&config, Part::Main, "Trier les photos", "", "", "asked", "", &agent).unwrap().task;
        let pages = all_pages(&config, Part::Main);
        assert_eq!(of_task(&config, &linked, &pages, &none, today), ["trips", "dates"]);
        assert_eq!(of_task(&config, &alone, &pages, &none, today), ["other"]);
        // documents: their page's, else their name's words, else Papiers
        let doc = |name: &str, subject: Option<&str>| json!({"agent": "home", "file": name, "name": name,
            "subjects": subject.map(|p| vec![json!({"path": p})]).unwrap_or_default()});
        assert_eq!(of_document(&config, &doc("Programme.pptx", Some("Sujets/Voyage Japon.md")), &pages, &none).0, ["trips"]);
        assert_eq!(of_document(&config, &doc("Facture EDF.pdf", None), &pages, &none),
                   (vec!["papers".into(), "money".into()], "auto".into(), Some("facture".into())));
        assert_eq!(of_document(&config, &doc("IMG_2041.jpg", None), &pages, &none).0, ["papers"]);
        // lines: their page's, Dates when they carry a date
        assert_eq!(of_line(&["people".to_string()], "p1", "Anniversaire : 14 mars — *Léa, 2026-10-01*", &none, today), ["people", "dates"]);
        // the person files them elsewhere: kept in the state, undone as well
        let give = |c: &str| vec![c.to_string()];
        let id = tk::str_or_empty(with.get("id"));
        let done = categorize(&config, &Target::Record { kind: "purchase".into(), id: id.clone() }, &give("home"), &you).unwrap().unwrap();
        assert_eq!(done["learnt"], Value::Null);
        assert_eq!(of_record(purchase, &with, &marks(&config), today).0, ["home"]);
        let t = categorize(&config, &Target::Task(alone.id.clone()), &give("home"), &you).unwrap().unwrap();
        assert_eq!(of_task(&config, &alone, &pages, &marks(&config), today), ["home"]);
        let japan = all_pages(&config, Part::Main).into_iter().find(|(_, p)| p.title == "Voyage Japon").unwrap();
        let line = Target::Line { path: japan.0.clone(), text: "Réserver les vols".into() };
        categorize(&config, &line, &give("money"), &you).unwrap().unwrap();
        assert_eq!(of_line(&["trips".to_string()], &japan.1.id(), "- Réserver les vols — *Home, 2026-11-22*", &marks(&config), today), ["money"]);
        let gone = Target::Line { path: japan.0.clone(), text: "Rien de tel".into() };
        assert_eq!(categorize(&config, &gone, &give("money"), &you).unwrap(), Err(Refused::NotFound));
        assert_eq!(categorize(&config, &Target::Record { kind: "purchase".into(), id: "r000000".into() }, &give("home"), &you).unwrap(),
                   Err(Refused::NotFound));
        undo(&config, t["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!(of_task(&config, &alone, &pages, &marks(&config), today), ["other"]);
        undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!(of_record(purchase, &with, &marks(&config), today).0, ["money", "papers"]);
        // the pages of the records keep their type's categories
        assert_eq!(of_page(&config, Part::Main, "Fiches/Achats.md", &all_pages(&config, Part::Main).into_iter()
            .find(|(p, _)| p == "Fiches/Achats.md").unwrap().1).categories, ["money", "papers"]);
    }
}
