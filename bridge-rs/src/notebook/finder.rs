//! The Carnet « Le sommaire » search (plan piste 9 §3.1–3.3, contracts/carnet/sommaire.md §3 « The phone's search »),
//! the rule written once for the bridge and the two phones: contracts/carnet/search-parity.json, played here
//! (`the_search_follows_the_parity_file`), by HermesKit's CarnetSearch and Android's CarnetSearch (its twin, step for
//! step: this file follows it so that the three give the same ids in the same order).
//!
//! It runs over a copy of the Carnet (`GET /v1/notebook/index`'s items, index.rs): the bridge builds the same copy for
//! its own search (`GET /v1/notebook/search`'s `groups` and `counts`) and the phones search theirs as the person types.
//!
//! - the query folded (`fold`); a date phrase of the copy's `dates` (the longest first) becomes an interval of days;
//!   a neighbour of several words (« salle de gym ») one term; the words left without the stop words (unless nothing
//!   else is left), each with its neighbours (other spellings, found as themselves only); 12 terms at most;
//! - a word typed is found as itself, as the start of a word from 3 letters, with one typo from 5 letters and two from
//!   8 (optimal string alignment), never a typo towards a word under 4 letters;
//! - an item's words: its `fold` (its own), and around it its title, its source, its aliases and its page's;
//! - kept: the category, the space and the interval first, then the items holding every term when some do;
//! - ranked: archived last; the title or an alias itself; every term; a direct answer (its `label`); the number of
//!   terms and how closely; the most recently noted; still to come; the copy's order;
//! - grouped under each item's first category (`other` when none), in the order of their best result.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Duration, Months, NaiveDate};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fold;

/// The most terms a query looks for (a long dictated sentence is cut).
pub const MAX_TERMS: usize = 12;
/// The kinds of notes the personal copy holds (sommaire.json `IndexItem`), and the family's (`FamilyIndexItem`).
pub const KINDS: [&str; 5] = ["page", "line", "task", "record", "document"];
pub const FAMILY_KINDS: [&str; 2] = ["page", "list_item"];

// -- the copy ---------------------------------------------------------------------------------------------------------

/// An item of the copy (`IndexItem`, compact: a key left out is its default).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub line: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yearly: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noted: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(default)]
    pub open: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fold: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
    /// A page's own kind (`person`, `self`, `subject`, `area`, `trip`): the category screens list the people apart
    /// (structure.rs, Alex 26/11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_kind: Option<String>,
    /// A person's link to the person as their page says it (« Beau-frère », « Voisine »); a group page's (« Amis »)
    /// none. `group`: the group the category screen lists them under (`family`, `friends`, `work`, `others`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// A person marked « proche ».
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
    /// The icon the person chose for a page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Entry {
    /// Its space (`perso` when none).
    pub fn space(&self) -> &str {
        self.space.as_deref().filter(|s| !s.is_empty()).unwrap_or("perso")
    }
}

/// One end of a date phrase (`DateEnd`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DateEnd {
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feast: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekday: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DatePhrase {
    pub words: String,
    pub from: DateEnd,
    pub to: DateEnd,
}

/// A feast as a rule (`Feast`): a fixed day, or days from Easter.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Feast {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easter: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DatePhrases {
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub phrases: Vec<DatePhrase>,
    #[serde(default)]
    pub feasts: IndexMap<String, Feast>,
    #[serde(default)]
    pub easter: IndexMap<String, String>,
}

/// The copy's tables the search reads: stop words, neighbours, date phrases, the `dated` window.
#[derive(Debug, Clone, Default)]
pub struct Tables {
    pub stop: Vec<String>,
    pub neighbours: IndexMap<String, Vec<String>>,
    pub dates: DatePhrases,
    /// Days back and ahead an item with a `date` is in `dates` for.
    pub dated: (i64, i64),
}

/// A copy (`Index` or `FamilyIndex`) as the phone keeps it: Santé never there, only its own kinds (the parity cases'
/// `allowed`).
pub fn read_copy(copy: &Value, family: bool) -> (Vec<Entry>, Tables) {
    let kinds: &[&str] = if family { &FAMILY_KINDS } else { &KINDS };
    let items: Vec<Entry> = copy["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| serde_json::from_value::<Entry>(i.clone()).ok())
        .filter(|e| allowed(e, kinds))
        .collect();
    let strings = |v: &Value| -> Vec<String> {
        v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
    };
    let neighbours = copy["neighbours"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), strings(v)))
        .collect();
    let dates = serde_json::from_value(copy["dates"].clone()).unwrap_or_default();
    let dated = (
        copy["dated"]["before"].as_i64().unwrap_or(super::categories::DATED_BEFORE),
        copy["dated"]["after"].as_i64().unwrap_or(super::categories::DATED_AFTER),
    );
    (items, Tables { stop: strings(&copy["stop"]), neighbours, dates, dated })
}

/// Whether an item may be in a copy: the main part, a kind the copy holds, never filed in Santé, never an archived
/// mail's link.
pub fn allowed(e: &Entry, kinds: &[&str]) -> bool {
    e.part.as_deref().unwrap_or("main") == "main"
        && kinds.contains(&e.kind.as_str())
        && !e.categories.iter().any(|c| c == "health")
        && !e.line.contains("[[archive:")
}

// -- days -------------------------------------------------------------------------------------------------------------

/// `YYYY-MM-DD` → `YYYYMMDD` (0: none).
fn key(day: Option<&str>) -> i32 {
    let Some(day) = day else { return 0 };
    let b = day.as_bytes();
    if b.len() < 10 {
        return 0;
    }
    let mut value: i32 = 0;
    for (i, c) in b.iter().take(10).enumerate() {
        if i == 4 || i == 7 {
            continue;
        }
        if !c.is_ascii_digit() {
            return 0;
        }
        value = value * 10 + i32::from(c - b'0');
    }
    value
}

fn key_of(d: NaiveDate) -> i32 {
    d.year() * 10000 + d.month() as i32 * 100 + d.day() as i32
}

fn day_of_key(k: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(k / 10000, (k / 100 % 100) as u32, (k % 100) as u32)
}

/// A yearly `MM-DD` → `MMDD` (0: none).
fn month_day(md: Option<&str>) -> i32 {
    let Some(md) = md else { return 0 };
    let b = md.as_bytes();
    if b.len() != 5 || b[2] != b'-' {
        return 0;
    }
    let (Ok(m), Ok(d)) = (md[..2].parse::<i32>(), md[3..].parse::<i32>()) else { return 0 };
    if (1..=12).contains(&m) && (1..=31).contains(&d) { m * 100 + d } else { 0 }
}

fn leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// A yearly `MMDD` in `year`, `YYYYMMDD` (29 February: the 28th in a year without one).
fn on_year(year: i32, md: i32) -> i32 {
    year * 10000 + if md == 229 && !leap(year) { 228 } else { md }
}

/// Whether a yearly `MMDD` falls in `from..=to` in one of its years.
fn yearly_in(md: i32, from: i32, to: i32) -> bool {
    if from > to || md == 0 {
        return false;
    }
    (from / 10000..=to / 10000).any(|y| (from..=to).contains(&on_year(y, md)))
}

fn plus_months(d: NaiveDate, n: i64) -> Option<NaiveDate> {
    if n >= 0 {
        d.checked_add_months(Months::new(n as u32))
    } else {
        d.checked_sub_months(Months::new((-n) as u32))
    }
}

fn plus_years(d: NaiveDate, n: i64) -> Option<NaiveDate> {
    plus_months(d, n * 12)
}

/// The day one end of a date phrase names, from `today`: `day` today + n; `week` the ISO week n from this one, its
/// `start` or `end` (no edge: today + 7n); `month` and `year` the same; `feast` its next occurrence from today
/// ([`next_feast`]); `weekday` the coming one (1 Monday … 7 Sunday, today included) + n weeks. n within ±1 000.
pub fn day(end: &DateEnd, today: NaiveDate, feasts: &IndexMap<String, Feast>, easter: &IndexMap<String, String>) -> Option<NaiveDate> {
    let n = end.n.unwrap_or(0).clamp(-1000, 1000);
    let weekday = i64::from(today.weekday().number_from_monday());
    match end.unit.as_str() {
        "day" => Some(today + Duration::days(n)),
        "week" => {
            let monday = today + Duration::days(1 - weekday) + Duration::weeks(n);
            match end.edge.as_deref() {
                Some("start") => Some(monday),
                Some("end") => Some(monday + Duration::days(6)),
                _ => Some(today + Duration::weeks(n)),
            }
        }
        "month" => {
            let first = today.with_day(1)?;
            match end.edge.as_deref() {
                Some("start") => plus_months(first, n),
                Some("end") => plus_months(first, n + 1).map(|d| d - Duration::days(1)),
                _ => plus_months(today, n),
            }
        }
        "year" => {
            let first = NaiveDate::from_ymd_opt(today.year(), 1, 1)?;
            match end.edge.as_deref() {
                Some("start") => plus_years(first, n),
                Some("end") => plus_years(first, n + 1).map(|d| d - Duration::days(1)),
                _ => plus_years(today, n),
            }
        }
        "feast" => next_feast(feasts.get(end.feast.as_deref()?)?, today, easter),
        "weekday" => {
            let wanted = end.weekday.filter(|w| (1..=7).contains(w))?;
            Some(today + Duration::days((wanted - weekday + 7) % 7 + 7 * n))
        }
        _ => None,
    }
}

/// A feast's next occurrence from `today` (today included): a fixed day this year or a later one (29 February: the
/// next leap year's), or Easter's day of the first year that falls on or after today, + its days. None: none (no
/// Easter given that far).
pub fn next_feast(rule: &Feast, today: NaiveDate, easter: &IndexMap<String, String>) -> Option<NaiveDate> {
    if let Some(days) = rule.easter {
        let mut years: Vec<(&String, &String)> = easter.iter().collect();
        years.sort_by(|a, b| a.0.cmp(b.0));
        return years
            .into_iter()
            .filter_map(|(_, d)| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())
            .map(|d| d + Duration::days(days))
            .find(|d| *d >= today);
    }
    let m = rule.month.filter(|m| (1..=12).contains(m))?;
    let longest = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m as usize - 1];
    let d = rule.day.filter(|d| (1..=longest).contains(d))?;
    for year in today.year()..=today.year() + 8 {
        if m == 2 && d == 29 && !leap(year) {
            continue;
        }
        let on = NaiveDate::from_ymd_opt(year, m, d)?;
        if on >= today {
            return Some(on);
        }
    }
    None
}

// -- typos -------------------------------------------------------------------------------------------------------------

/// Letters added, removed, changed or two side by side swapped (optimal string alignment), `most + 1` as soon as it is
/// past `most`.
pub fn distance(a: &str, b: &str, most: usize) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > most {
        return most + 1;
    }
    if n == 0 || m == 0 {
        return n.max(m);
    }
    let width = n + 1;
    let mut two_back = vec![0usize; width];
    let mut back: Vec<usize> = (0..width).collect();
    let mut here = vec![0usize; width];
    for i in 1..=m {
        here[0] = i;
        let mut smallest = i;
        for j in 1..=n {
            let cost = usize::from(a[j - 1] != b[i - 1]);
            let mut value = (back[j] + 1).min(here[j - 1] + 1).min(back[j - 1] + cost);
            if i > 1 && j > 1 && a[j - 1] == b[i - 2] && a[j - 2] == b[i - 1] {
                value = value.min(two_back[j - 2] + 1);
            }
            here[j] = value;
            smallest = smallest.min(value);
        }
        if smallest > most {
            return most + 1;
        }
        std::mem::swap(&mut two_back, &mut back);
        std::mem::swap(&mut back, &mut here);
    }
    back[n]
}

/// A folded word's letters and digits as bits: a word with more than `most` letters the other lacks is further than
/// `most`.
fn letter_bits(word: &str) -> u64 {
    let mut bits = 0u64;
    for c in word.bytes() {
        if c.is_ascii_lowercase() {
            bits |= 1 << (c - b'a');
        } else if c.is_ascii_digit() {
            bits |= 1 << (c - b'0' + 26);
        }
    }
    bits
}

// -- the search -------------------------------------------------------------------------------------------------------

/// What a query looks for: `folded` the whole query, `plain` its words without the stop words, the date phrase nor
/// the neighbours of several words; each term its spellings (itself first, then its neighbours), each a list of words.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub folded: String,
    pub terms: Vec<Vec<Vec<String>>>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub plain: String,
}

/// A category's block: its count and its two best (positions in the items).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub category: String,
    pub count: usize,
    pub top: Vec<usize>,
}

/// Every item found, best first (`order`: positions in the items); `counts`: `all` and one per category (the chips).
#[derive(Debug, Clone)]
pub struct Found {
    pub query: Query,
    pub order: Vec<usize>,
    pub groups: Vec<Group>,
    pub counts: IndexMap<String, usize>,
}

/// A result's rank, the larger first: not archived, its title or an alias, every term, a direct answer, how many
/// terms, how closely, when it was noted, still to come.
type Rank = (bool, bool, bool, bool, u32, u32, i32, bool);

/// The copy made ready to search: its words, where each one is, the items' days.
pub struct Finder {
    /// The copy's items (shared with the copy kept by index.rs: never held twice).
    pub items: std::sync::Arc<Vec<Entry>>,
    words: Vec<String>,
    word_id: HashMap<String, u32>,
    /// Per word: the item's position doubled, + 1 when the word is only around it (title, source, aliases).
    postings: Vec<Vec<u32>>,
    /// Word ids in the words' order: the starts of words.
    sorted: Vec<u32>,
    /// Word ids by length (4 to 47 letters): the typos.
    by_length: Vec<Vec<u32>>,
    letters: Vec<u64>,
    titles: Vec<String>,
    aliases: Vec<Vec<String>>,
    labels: Vec<Vec<u32>>,
    noted: Vec<i32>,
    dated: Vec<i32>,
    yearly: Vec<i32>,
    archived: Vec<bool>,
    window: (i64, i64),
    stop: HashSet<String>,
    neighbours: HashMap<String, Vec<Vec<String>>>,
    phrases_of_words: Vec<(String, Vec<Vec<String>>)>,
    date_phrases: Vec<(String, DateEnd, DateEnd)>,
    feasts: IndexMap<String, Feast>,
    easter: IndexMap<String, String>,
}

/// Texts folded once (a page's title and its sections' names are every line's).
#[derive(Default)]
struct Folded {
    texts: HashMap<String, String>,
    ids: HashMap<String, Vec<u32>>,
}

impl Folded {
    fn fold(&mut self, text: &str) -> String {
        if let Some(f) = self.texts.get(text) {
            return f.clone();
        }
        let f = fold(text);
        self.texts.insert(text.to_string(), f.clone());
        f
    }
}

fn split(text: &str) -> Vec<String> {
    text.split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

impl Finder {
    pub fn new(items: Vec<Entry>, tables: &Tables) -> Finder {
        Finder::shared(std::sync::Arc::new(items), tables)
    }

    /// [`Finder::new`] on items kept elsewhere too (index.rs's copy).
    pub fn shared(items: std::sync::Arc<Vec<Entry>>, tables: &Tables) -> Finder {
        let n = items.len();
        let mut f = Finder {
            items: Default::default(),
            words: Vec::new(),
            word_id: HashMap::new(),
            postings: Vec::new(),
            sorted: Vec::new(),
            by_length: vec![Vec::new(); 48],
            letters: Vec::new(),
            titles: Vec::with_capacity(n),
            aliases: Vec::with_capacity(n),
            labels: Vec::with_capacity(n),
            noted: Vec::with_capacity(n),
            dated: Vec::with_capacity(n),
            yearly: Vec::with_capacity(n),
            archived: Vec::with_capacity(n),
            window: tables.dated,
            stop: HashSet::new(),
            neighbours: HashMap::new(),
            phrases_of_words: Vec::new(),
            date_phrases: Vec::new(),
            feasts: tables.dates.feasts.clone(),
            easter: tables.dates.easter.clone(),
        };
        let mut cache = Folded::default();
        // a line's page: its aliases count for the line
        let mut page_aliases: HashMap<&str, &Vec<String>> = HashMap::new();
        for item in items.iter() {
            if item.kind == "page" && !item.aliases.is_empty() {
                page_aliases.insert(item.id.as_str(), &item.aliases);
            }
        }
        for (position, item) in items.iter().enumerate() {
            let position = position as u32;
            for w in item.fold.split(' ').filter(|w| !w.is_empty()) {
                let id = f.intern(w);
                f.post(id, position, false);
            }
            for id in f.around_ids(&mut cache, &item.title) {
                f.post(id, position, true);
            }
            if let Some(source) = &item.source {
                for id in f.around_ids(&mut cache, source) {
                    f.post(id, position, true);
                }
            }
            let mut named: Vec<String> = item.aliases.clone();
            if let Some(more) = item.page.as_deref().and_then(|p| page_aliases.get(p)) {
                named.extend(more.iter().cloned());
            }
            for alias in &named {
                for id in f.around_ids(&mut cache, alias) {
                    f.post(id, position, true);
                }
            }
            f.titles.push(cache.fold(&item.title));
            let mut folded_aliases: Vec<String> = Vec::new();
            for a in &named {
                let a = cache.fold(a);
                if !folded_aliases.contains(&a) {
                    folded_aliases.push(a);
                }
            }
            f.aliases.push(folded_aliases);
            let labels = match &item.label {
                Some(l) => f.around_ids(&mut cache, l),
                None => Vec::new(),
            };
            f.labels.push(labels);
            // (a year before 1900 is never noted)
            let noted = key(item.noted.as_deref());
            f.noted.push(if noted / 10000 >= 1900 { noted } else { 0 });
            f.dated.push(key(item.date.as_deref()));
            f.yearly.push(month_day(item.yearly.as_deref()));
            f.archived.push(item.archived);
        }
        // a page has no attribution of its own: its latest line's (by the line's `page`, else its path)
        let mut latest: HashMap<&str, i32> = HashMap::new();
        for (at, item) in items.iter().enumerate() {
            if item.kind == "page" {
                continue;
            }
            if let Some(p) = item.page.as_deref().or(item.path.as_deref()) {
                let e = latest.entry(p).or_insert(0);
                if f.noted[at] > *e {
                    *e = f.noted[at];
                }
            }
        }
        for (at, item) in items.iter().enumerate() {
            if item.kind == "page" && f.noted[at] == 0 {
                if let Some(n) = latest.get(item.id.as_str()).or_else(|| item.path.as_deref().and_then(|p| latest.get(p))) {
                    f.noted[at] = *n;
                }
            }
        }
        let mut sorted: Vec<u32> = (0..f.words.len() as u32).collect();
        sorted.sort_by(|a, b| f.words[*a as usize].cmp(&f.words[*b as usize]));
        f.sorted = sorted;
        for (id, word) in f.words.iter().enumerate() {
            if (4..=47).contains(&word.len()) {
                f.by_length[word.len()].push(id as u32);
            }
        }
        f.letters = f.words.iter().map(|w| letter_bits(w)).collect();
        f.stop = tables.stop.iter().map(|s| fold(s)).collect();
        let mut several: Vec<(String, Vec<Vec<String>>)> = Vec::new();
        for (k, values) in &tables.neighbours {
            let said = fold(k);
            let alternatives: Vec<Vec<String>> = values.iter().map(|v| split(&fold(v))).filter(|a| !a.is_empty()).collect();
            if said.is_empty() || alternatives.is_empty() {
                continue;
            }
            if said.contains(' ') {
                several.push((said, alternatives));
            } else {
                f.neighbours.entry(said).or_default().extend(alternatives);
            }
        }
        several.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        f.phrases_of_words = several;
        let mut phrases: Vec<(String, DateEnd, DateEnd)> = tables
            .dates
            .phrases
            .iter()
            .map(|p| (fold(&p.words), p.from.clone(), p.to.clone()))
            .filter(|p| !p.0.is_empty())
            .collect();
        phrases.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
        f.date_phrases = phrases;
        f.items = items;
        f
    }

    fn post(&mut self, id: u32, position: u32, around: bool) {
        let list = &mut self.postings[id as usize];
        if list.last().is_none_or(|last| last >> 1 != position) {
            list.push(position << 1 | u32::from(around));
        }
    }

    /// A title's, a source's, an alias's or a label's words: each text folded and looked up once.
    fn around_ids(&mut self, cache: &mut Folded, text: &str) -> Vec<u32> {
        if let Some(ids) = cache.ids.get(text) {
            return ids.clone();
        }
        let folded = cache.fold(text);
        let ids: Vec<u32> = folded.split(' ').filter(|w| !w.is_empty()).map(|w| self.intern(w)).collect();
        cache.ids.insert(text.to_string(), ids.clone());
        ids
    }

    /// About how many bytes it holds in memory (the bench: the bridge's RAM for one Carnet's copy).
    pub fn footprint(&self) -> usize {
        let text = |t: &str| t.len() + 24;
        let items: usize = self
            .items
            .iter()
            .map(|e| {
                text(&e.id) + text(&e.title) + text(&e.line) + text(&e.fold) + e.path.as_deref().map_or(0, text)
                    + e.categories.iter().map(|c| text(c)).sum::<usize>() + 200
            })
            .sum();
        let words: usize = self.words.iter().map(|w| text(w) * 2 + 8 + 24).sum();
        let postings: usize = self.postings.iter().map(|p| p.len() * 4 + 24).sum();
        let per_item: usize = self.titles.iter().map(|t| text(t)).sum::<usize>() + self.items.len() * (4 * 3 + 1 + 24 * 2);
        items + words + postings + per_item
    }

    fn intern(&mut self, word: &str) -> u32 {
        if let Some(id) = self.word_id.get(word) {
            return *id;
        }
        let id = self.words.len() as u32;
        self.words.push(word.to_string());
        self.word_id.insert(word.to_string(), id);
        self.postings.push(Vec::new());
        id
    }

    /// What `text` looks for on `today`.
    pub fn query(&self, text: &str, today: NaiveDate) -> Query {
        let folded = fold(text);
        let mut padded = format!(" {folded} ");
        let (mut from, mut to) = (None, None);
        for (words, start, end) in &self.date_phrases {
            let wanted = format!(" {words} ");
            if !padded.contains(&wanted) {
                continue;
            }
            let (Some(a), Some(b)) = (day(start, today, &self.feasts, &self.easter), day(end, today, &self.feasts, &self.easter)) else {
                continue;
            };
            // a phrase whose interval would end before it starts is not used (its words stay)
            if a > b {
                continue;
            }
            (from, to) = (Some(a), Some(b));
            padded = padded.replace(&wanted, " ");
            break;
        }
        let mut terms: Vec<Vec<Vec<String>>> = Vec::new();
        for (words, alternatives) in &self.phrases_of_words {
            let wanted = format!(" {words} ");
            if !padded.contains(&wanted) {
                continue;
            }
            let mut term = vec![split(words)];
            term.extend(alternatives.iter().cloned());
            terms.push(term);
            padded = padded.replace(&wanted, " ");
        }
        let said = split(&padded);
        let mut kept: Vec<String> = said.iter().filter(|w| !self.stop.contains(*w)).cloned().collect();
        if kept.is_empty() && terms.is_empty() && from.is_none() {
            kept = said;
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for word in &kept {
            if !seen.insert(word.as_str()) {
                continue;
            }
            let mut term = vec![vec![word.clone()]];
            term.extend(self.neighbours.get(word).cloned().unwrap_or_default());
            terms.push(term);
        }
        terms.truncate(MAX_TERMS);
        Query { folded, terms, from, to, plain: kept.join(" ") }
    }

    /// One word against the copy's words, as (word id, quality): itself 3; typed by the person (not a neighbour), also
    /// the start of a word 2 (from 3 letters) and a typo 1.
    fn found(&self, word: &str, typed: bool) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let exact = self.word_id.get(word).copied();
        if let Some(id) = exact {
            out.push((id, 3));
        }
        if !typed {
            return out;
        }
        if word.len() >= 3 {
            let start = self.sorted.partition_point(|id| self.words[*id as usize].as_str() < word);
            for id in &self.sorted[start..] {
                if !self.words[*id as usize].starts_with(word) {
                    break;
                }
                if Some(*id) != exact {
                    out.push((*id, 2));
                }
            }
        }
        let most = if word.len() >= 8 { 2 } else if word.len() >= 5 { 1 } else { 0 };
        if most == 0 || word.len() >= 46 {
            return out;
        }
        let mine = letter_bits(word);
        for length in 4.max(word.len() - most)..=47.min(word.len() + most) {
            for id in &self.by_length[length] {
                if Some(*id) == exact {
                    continue;
                }
                let theirs = self.letters[*id as usize];
                if (mine & !theirs).count_ones() as usize > most || (theirs & !mine).count_ones() as usize > most {
                    continue;
                }
                let candidate = &self.words[*id as usize];
                if distance(word, candidate, most) <= most && !candidate.starts_with(word) {
                    out.push((*id, 1));
                }
            }
        }
        out
    }

    fn window(&self, today: NaiveDate) -> (i32, i32) {
        (key_of(today - Duration::days(self.window.0)), key_of(today + Duration::days(self.window.1)))
    }

    /// An item's day as of `day` (`YYYYMMDD`): its date, or a yearly one's next occurrence, today included; 0: none.
    fn day_of(&self, at: usize, day: i32) -> i32 {
        if self.dated[at] != 0 {
            return self.dated[at];
        }
        if self.yearly[at] == 0 {
            return 0;
        }
        let year = day / 10000;
        if on_year(year, self.yearly[at]) >= day { on_year(year, self.yearly[at]) } else { on_year(year + 1, self.yearly[at]) }
    }

    fn in_dates(&self, at: usize, from: i32, to: i32) -> bool {
        self.yearly[at] != 0 || (self.dated[at] != 0 && (from..=to).contains(&self.dated[at]))
    }

    /// An item's categories as of `today`: the copy's, and `dates` when it is dated (what the screens show).
    pub fn categories(&self, at: usize, today: NaiveDate) -> Vec<String> {
        let (from, to) = self.window(today);
        let mut cats = self.items[at].categories.clone();
        if !cats.iter().any(|c| c == "dates") && self.in_dates(at, from, to) {
            cats.push("dates".into());
        }
        cats
    }

    /// An item's day as of `today` (a yearly one's next occurrence).
    pub fn day(&self, at: usize, today: NaiveDate) -> Option<NaiveDate> {
        match self.day_of(at, key_of(today)) {
            0 => None,
            k => day_of_key(k),
        }
    }

    /// Whether `words` (folded) are the item's title or one of its aliases.
    fn is_title(&self, at: usize, words: &str) -> bool {
        !words.is_empty() && (self.titles[at] == words || self.aliases[at].iter().any(|a| a == words))
    }

    /// What `text` finds on `today`: in one category (any of an item's, `dates` as of today) and one space when given.
    pub fn search(&self, text: &str, today: NaiveDate, category: Option<&str>, space: Option<&str>) -> Found {
        let query = self.query(text, today);
        let terms = &query.terms;
        let interval = query.from.zip(query.to).map(|(a, b)| (key_of(a), key_of(b)));
        let day = key_of(today);
        let (window_from, window_to) = self.window(today);
        let mut candidates: Vec<usize> = Vec::new();
        let mut mask: Vec<u32> = Vec::new();
        let mut closeness: Vec<u32> = Vec::new();
        let mut matched = vec![false; self.words.len()];
        if terms.is_empty() {
            if interval.is_none() {
                let mut counts = IndexMap::new();
                counts.insert("all".to_string(), 0);
                return Found { query, order: Vec::new(), groups: Vec::new(), counts };
            }
            candidates = (0..self.items.len()).filter(|at| self.day_of(*at, day) != 0).collect();
        } else {
            mask = vec![0; self.items.len()];
            closeness = vec![0; self.items.len()];
            let mut best = vec![0u32; self.items.len()];
            let mut touched: Vec<usize> = Vec::new();
            for (number, term) in terms.iter().enumerate() {
                touched.clear();
                for (spelling, spelled) in term.iter().enumerate() {
                    if spelled.len() == 1 {
                        for (id, q) in self.found(&spelled[0], spelling == 0) {
                            let quality = 2 * if spelling == 0 { q } else { q.min(2) };
                            matched[id as usize] = true;
                            for p in &self.postings[id as usize] {
                                let at = (p >> 1) as usize;
                                let value = quality + 1 - (p & 1);
                                if best[at] == 0 {
                                    touched.push(at);
                                }
                                best[at] = best[at].max(value);
                            }
                        }
                    } else {
                        // several words, each as itself
                        let ids: Vec<u32> = spelled.iter().filter_map(|w| self.word_id.get(w).copied()).collect();
                        if ids.len() != spelled.len() {
                            continue;
                        }
                        let positions = |id: u32| -> HashSet<u32> { self.postings[id as usize].iter().map(|p| p >> 1).collect() };
                        let mut common = positions(ids[0]);
                        for id in &ids[1..] {
                            let other = positions(*id);
                            common.retain(|p| other.contains(p));
                        }
                        let mut common: Vec<u32> = common.into_iter().collect();
                        common.sort_unstable();
                        for at in common {
                            let at = at as usize;
                            if best[at] == 0 {
                                touched.push(at);
                            }
                            best[at] = best[at].max(4);
                        }
                    }
                }
                for at in &touched {
                    let at = *at;
                    if mask[at] == 0 {
                        candidates.push(at);
                    }
                    mask[at] |= 1 << number;
                    closeness[at] += best[at];
                    best[at] = 0;
                }
            }
        }
        // first the category, the space and the interval; then, among those, every term when some hold them all
        let any_space = space.is_none_or(|s| s == "all");
        candidates.retain(|at| {
            let at = *at;
            let item = &self.items[at];
            interval.is_none_or(|(low, high)| {
                if self.yearly[at] != 0 { yearly_in(self.yearly[at], low, high) } else { (low..=high).contains(&self.dated[at]) }
            }) && category.is_none_or(|c| item.categories.iter().any(|x| x == c) || (c == "dates" && self.in_dates(at, window_from, window_to)))
                && (any_space || Some(item.space()) == space)
        });
        if !terms.is_empty() {
            let every = (1u32 << terms.len()) - 1;
            if candidates.iter().any(|at| mask[*at] == every) {
                candidates.retain(|at| mask[*at] == every);
            }
        }
        // ranked: the larger first, then the copy's order
        let mut ranked: Vec<(Rank, usize)> = candidates
            .iter()
            .map(|at| {
                let at = *at;
                let item = &self.items[at];
                let title = item.kind != "line" && (self.is_title(at, &query.folded) || self.is_title(at, &query.plain));
                let (every, label, count, close) = if terms.is_empty() {
                    (false, false, 0, 0)
                } else {
                    let count = mask[at].count_ones();
                    (
                        count as usize == terms.len(),
                        self.labels[at].iter().any(|id| matched[*id as usize]),
                        count,
                        closeness[at].min(4095),
                    )
                };
                let n = self.noted[at];
                let noted = if n == 0 { 0 } else { (n / 10000 - 1900) * 372 + n / 100 % 100 * 31 + n % 100 };
                let coming = if item.kind == "task" { !item.done } else { self.day_of(at, day) >= day };
                ((!self.archived[at], title, every, label, count, close, noted, coming), at)
            })
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let order: Vec<usize> = ranked.into_iter().map(|(_, at)| at).collect();
        // a note counts in each of its categories (`dates` as of today too), once in `all`, listed under its first
        let mut groups: Vec<Group> = Vec::new();
        let mut place: HashMap<String, usize> = HashMap::new();
        let mut counts: IndexMap<String, usize> = IndexMap::new();
        counts.insert("all".into(), order.len());
        for at in &order {
            let cats = &self.items[*at].categories;
            let mut seen: Vec<&str> = Vec::new();
            for c in cats {
                if !seen.contains(&c.as_str()) {
                    seen.push(c);
                    *counts.entry(c.clone()).or_insert(0) += 1;
                }
            }
            let dated = self.in_dates(*at, window_from, window_to);
            if !cats.iter().any(|c| c == "dates") && dated {
                *counts.entry("dates".into()).or_insert(0) += 1;
            }
            // a note without a category of its own (the family Carnet's date with no word of one, S6): under Dates
            // when dated, else « Autres »
            if cats.is_empty() && !dated {
                *counts.entry("other".into()).or_insert(0) += 1;
            }
            let fallback = if dated { "dates" } else { "other" };
            let cat = cats.first().cloned().unwrap_or_else(|| fallback.into());
            match place.get(&cat) {
                Some(g) => {
                    groups[*g].count += 1;
                    if groups[*g].top.len() < 2 {
                        groups[*g].top.push(*at);
                    }
                }
                None => {
                    place.insert(cat.clone(), groups.len());
                    groups.push(Group { category: cat, count: 1, top: vec![*at] });
                }
            }
        }
        Found { query, order, groups, counts }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;

    fn repo(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(relative)
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
    }

    fn s(v: &Value) -> String {
        v.as_str().unwrap_or_default().to_string()
    }

    fn date(v: &Value) -> NaiveDate {
        NaiveDate::parse_from_str(v.as_str().unwrap(), "%Y-%m-%d").unwrap()
    }

    /// contracts/carnet/search-parity.json: the typos, the date ends, then every case on its copy, as the phones play
    /// them (HermesKit CarnetSearchTests.theSearchFollowsTheParityFile, Android CarnetSearchTest).
    #[test]
    fn the_search_follows_the_parity_file() {
        let file = repo("contracts/carnet/search-parity.json");
        let parity = read(&file);
        let dir = file.parent().unwrap();
        let mut errors: Vec<String> = Vec::new();
        for t in parity["typos"].as_array().unwrap() {
            let (a, b, d) = (s(&t[0]), s(&t[1]), t[2].as_u64().unwrap() as usize);
            if distance(&a, &b, 9) != d {
                errors.push(format!("typo {a} → {b}: {} not {d}", distance(&a, &b, 9)));
            }
        }
        for case in parity["dates"].as_array().unwrap() {
            let end: DateEnd = serde_json::from_value(case["end"].clone()).unwrap();
            let feasts: IndexMap<String, Feast> = serde_json::from_value(case.get("feasts").cloned().unwrap_or(json!({}))).unwrap();
            let easter: IndexMap<String, String> = serde_json::from_value(case.get("easter").cloned().unwrap_or(json!({}))).unwrap();
            let got = day(&end, date(&case["today"]), &feasts, &easter).map(|d| d.to_string());
            if got.as_deref() != case["day"].as_str() {
                errors.push(format!("date {case}: {got:?}"));
            }
        }
        let mut copies: HashMap<String, Finder> = HashMap::new();
        for case in parity["cases"].as_array().unwrap() {
            let name = s(&case["index"]);
            let finder = copies.entry(name.clone()).or_insert_with(|| {
                let (items, tables) = read_copy(&read(&dir.join(&name)), name.contains("family"));
                Finder::new(items, &tables)
            });
            let found = finder.search(
                case["q"].as_str().unwrap_or_default(),
                date(&case["today"]),
                case["category"].as_str(),
                case["space"].as_str(),
            );
            let ids: Vec<String> = found.order.iter().map(|at| finder.items[*at].id.clone()).collect();
            let what = s(&case["id"]);
            let mut fail = |text: String| errors.push(format!("{what}: {text} (found {ids:?})"));
            if let Some(first) = case["first"].as_str() {
                if ids.first().map(String::as_str) != Some(first) {
                    fail(format!("first {first}"));
                }
            }
            if let Some(top) = case["top"].as_array() {
                let want: Vec<String> = top.iter().map(s).collect();
                if ids.iter().take(want.len()).cloned().collect::<Vec<_>>() != want {
                    fail(format!("top {want:?}"));
                }
            }
            for i in case["includes"].as_array().into_iter().flatten() {
                if !ids.contains(&s(i)) {
                    fail(format!("includes {i}"));
                }
            }
            for e in case["excludes"].as_array().into_iter().flatten() {
                if ids.contains(&s(e)) {
                    fail(format!("excludes {e}"));
                }
            }
            if let Some(want) = case["found"].as_array() {
                let want: HashSet<String> = want.iter().map(s).collect();
                if ids.iter().cloned().collect::<HashSet<_>>() != want {
                    fail(format!("found {want:?}"));
                }
            }
            if let Some(count) = case["count"].as_u64() {
                if ids.len() as u64 != count {
                    fail(format!("count {count}"));
                }
            }
            if let Some(groups) = case["groups"].as_array() {
                let want: Vec<String> = groups.iter().map(s).collect();
                let got: Vec<String> = found.groups.iter().take(want.len()).map(|g| g.category.clone()).collect();
                if got != want {
                    fail(format!("groups {want:?}, got {got:?}"));
                }
            }
            if let Some(counts) = case["counts"].as_object() {
                for (k, v) in counts {
                    if found.counts.get(k).map(|n| *n as u64) != v.as_u64() {
                        fail(format!("counts.{k} {v}, got {:?}", found.counts.get(k)));
                    }
                }
            }
            if let Some(interval) = case["interval"].as_array() {
                let got = vec![json!(found.query.from.map(|d| d.to_string())), json!(found.query.to.map(|d| d.to_string()))];
                if &got != interval {
                    fail(format!("interval {interval:?}, got {got:?}"));
                }
            }
            if let Some(terms) = case.get("terms") {
                if &json!(found.query.terms) != terms {
                    fail(format!("terms {terms}, got {}", json!(found.query.terms)));
                }
            }
        }
        assert!(errors.is_empty(), "{} of the parity file's cases differ:\n{}", errors.len(), errors.join("\n"));
    }

    #[test]
    fn santé_never_reaches_a_copy() {
        let copy = json!({"items": [
            {"id": "a", "kind": "line", "fold": "traitement", "open": {}},
            {"id": "b", "kind": "line", "part": "health", "fold": "traitement", "open": {}},
            {"id": "c", "kind": "line", "categories": ["health"], "fold": "traitement", "open": {}},
            {"id": "d", "kind": "line", "line": "[[archive:x]]", "fold": "traitement", "open": {}},
            {"id": "e", "kind": "list_item", "fold": "traitement", "open": {}}]});
        let (items, tables) = read_copy(&copy, false);
        let finder = Finder::new(items, &tables);
        let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        let ids: Vec<&str> = finder.search("traitement", today, None, None).order.iter().map(|at| finder.items[*at].id.as_str()).collect();
        assert_eq!(ids, ["a"]);
    }
}
