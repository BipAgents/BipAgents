//! The Carnet's categories (plan « Le sommaire », brick S1; contracts/carnet/sommaire.md §1–2): a label on each page,
//! record, task, document and line — never a folder, nothing moves. Personal ids `people, dates, money, home, papers,
//! trips, work, other, health`, family ones apart (S6). Filed by the bridge at each write, by words, without a model
//! and without a token (`decide`: the kind first, then the person's rules, then the 5-language table of
//! lexicon::CATEGORY_WORDS — a title's head before what follows its first preposition, a word the table does not know
//! by its neighbours of search_words.rs; carnet-categories, 22/11); an agent may say one (`notebook_write.category`);
//! the person's correction always wins
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
/// Corrections that can still be undone: the 20 latest, and all of today's (« l'historique d'aujourd'hui », S2) up
/// to `UNDO_TODAY_MAX`.
pub const UNDO_KEPT: usize = 20;
pub const UNDO_TODAY_MAX: usize = 100;
/// What this bridge's Carnet can do of the sommaire (contracts/carnet/sommaire.json `capabilities`), in the home's
/// `sommaire`: a new app uses a route only when it is listed (S1: the categories on today's answers, « Ranger
/// ailleurs »; S2 `sorted`: the card « Ton Carnet est rangé par catégories », today's re-filings, the toast; S3
/// `index`: the phone's search copy; S4 `summary`, `category`, `kept`, `searched`: the sommaire ready to show,
/// summary.rs; S6 `family`: the family Carnet's sommaire, family_carnet.rs; then S5 `spaces`).
pub const CAPABILITIES: [&str; 9] =
    ["categories", "categorize", "sorted", "index", "summary", "category", "kept", "searched", "family"];
/// The person's corrections of records, tasks, documents and lines kept.
const MARKS_KEPT: usize = 2000;
/// An item is in « Dates » when its date is from this many days ago …
pub const DATED_BEFORE: i64 = 90;
/// … to this many days ahead (a passport's expiry, a warranty's end).
pub const DATED_AFTER: i64 = 730;

// How much each part of a page counts for its words' categories: the title most.
const TITLE: f64 = 5.0;
/// A title's words after its first preposition (« Dépenses | de Noël », « Kredit | für die Küche », « Seguro | de
/// hogar »): what its head is about, counting a little less than the head (carnet-categories, 22/11).
const MODIFIER: f64 = 4.0;
const ALIASES: f64 = 4.0;
const SUMMARY: f64 = 2.0;
/// The « En bref » once the title or an alias said something: it refines, it does not turn the page over
/// (carnet-categories: « Dépenses de Noël · Cadeaux et repas » is Argent).
const SUMMARY_AFTER_TITLE: f64 = 1.0;
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
/// Words that say little of what a page is (review 1, R1: « Ménage ce week-end », « Kids this weekend » are not
/// trips): they count `WEAK` of a word.
const WEAK_STEMS: &[&str] = &["weekend", "week end"];
const WEAK: f64 = 0.5;
/// Below this (as an entry's first category), the words placed a page narrowly (`narrow_in`).
const NARROW_BELOW: f64 = 4.0;
/// A runner-up this close to the first category: placed narrowly too.
const NARROW_CLOSE: f64 = 0.9;
/// An amount in the « En bref » (« 25 € la séance »): a hint for Argent, never enough alone (carnet-categories: an
/// amount filed a babysitter's or a piano lesson's page in Argent; the blind set had 12 « Argent » too many).
const AMOUNT: f64 = 0.6;
/// A word the table does not know, read through its neighbours (search_words.rs, plan §3.2: « toubib » is
/// « médecin »): this share of a word of the table.
const NEIGHBOUR: f64 = 0.5;

/// The words that end a title's head (« Dépenses | de Noël », « Gift ideas | for Mum », « Kredit | für die Küche »,
/// « Hipoteca | del piso », « Afscheid | van Ingrid »), folded: the five languages' prepositions (French and Spanish
/// « en » is Dutch « and »: the head ends there too). Never a title's first word.
const PREPOSITIONS: &[&str] = &[
    "de", "du", "des", "d", "pour", "a", "au", "aux", "avec", "chez", "en", "sur", "dans", "apres", "avant", "par",
    "sans", "vers", "entre", "of", "for", "to", "with", "at", "in", "on", "from", "by", "about", "before", "after",
    "fur", "von", "vom", "mit", "nach", "zu", "zum", "zur", "bei", "beim", "im", "am", "an", "auf", "aus", "vor",
    "uber", "ab", "bis", "ins", "del", "para", "con", "al", "por", "sin", "sobre", "desde", "hasta", "van", "voor",
    "met", "naar", "bij", "op", "aan", "uit", "om", "over", "tot", "door",
];

/// Between categories with the same points, what kind of note it is before what it is about (carnet-categories,
/// 22/11: « Dishwasher warranty » is Papiers, « Train to see Gran » Voyages, « Christmas spending » Argent; the
/// sommaire's order put Maison and Personnes first), « Autres » last.
const TIES: [&str; 7] = ["papers", "money", "trips", "work", "home", "people", "other"];

fn tie(category: &str) -> usize {
    TIES.iter().position(|c| *c == category).unwrap_or(TIES.len())
}

/// Neighbours that never file a word (carnet-categories): « rezept », « receta », « recept » are a prescription too;
/// a car's garage and its inspection, an itinerary's « planning », a warranty's « broken », a shift's « cita »
/// (an appointment) would pull what is not theirs.
const NEIGHBOUR_NEVER_THROUGH: &[&str] = &[
    "rezept", "receta", "recept", "garage", "tuv", "itv", "itineraire", "itinerary", "reiseplan", "garantie", "warranty",
    "garantia", "altpapier", "turno",
];

/// Words whose neighbours never file them: a note (« Note 03 », a bill's neighbour), a ticket, a quote, a room or
/// nights (a hotel's, a child's), a « planning », a call or a meeting that may be the family's, « Marie » the name,
/// German « Mann » and « Frau », English « kind » and « order », French « mot », a school that may be a gym.
const NEIGHBOUR_NEVER: &[&str] = &[
    "note", "notes", "addition", "ticket", "tickets", "quote", "quotes", "college", "gymnasium", "room", "rooms",
    "chambre", "chambres", "kamer", "kamers", "nights", "nuits", "nachte", "noches", "nachten", "planning", "programme",
    "program", "programm", "schedule", "route", "call", "reunion", "reunions", "equipe", "team", "present", "presents",
    "kind", "mot", "order", "due", "post", "pass", "power", "contador", "cita", "citas", "consulta", "marie", "gym",
    "workout", "mann", "frau", "vrouw", "mujer", "man", "mate", "mates", "bon", "fuel", "petrol", "diesel", "essence",
    "remote", "kurs", "gran", "stage", "stages",
];

/// Dutch health words (lexicon.rs `touches_health` has none yet: review 1, R4): never filed by a neighbour, never a
/// neighbour that files (the neighbours' health groups, search_words.rs).
const DUTCH_HEALTH: &[&str] = &[
    "dokter", "huisarts", "arts", "doc", "tandarts", "tanden", "gebit", "afspraak", "afspraken", "consult", "fysio",
    "fysiotherapeut", "fysiotherapie", "opticien", "bril", "lenzen", "oogarts", "allergie", "allergisch", "allergieen",
    "intolerantie", "recept", "medicijn", "medicijnen", "apotheek", "pillen", "vaccinatie", "prik", "vaccin", "inenting",
    "ziekenhuis", "kliniek", "spoedeisende hulp", "eerste hulp",
];

/// Words that make someone a work contact (a person page: people and work), folded.
const COLLEAGUE: &[&str] = &[
    "collegue", "collegues", "colleague", "colleagues", "kollege", "kollegin", "kollegen", "colega", "colegas",
    "collega", "collegas", "manager", "client", "cliente", "clients", "customer", "kunde", "kundin", "klant",
];

/// The categories' names as the screens show them (contracts/carnet/sommaire.json `titles_proposed`, the same table:
/// checked by `the_sommaire_contract_holds_together`), for fr, en, de, es, nl (Alex, 22/11: « Personnes », the other
/// French names stand for now; the other languages to be read by a speaker).
pub const TITLES: [(&str, [&str; 5]); 13] = [
    ("people", ["Personnes", "People", "Personen", "Personas", "Personen"]),
    ("dates", ["Dates", "Dates", "Termine", "Fechas", "Datums"]),
    ("money", ["Argent", "Money", "Geld", "Dinero", "Geld"]),
    ("home", ["Maison", "Home", "Zuhause", "Casa", "Huis"]),
    ("papers", ["Papiers", "Papers", "Papiere", "Papeles", "Papieren"]),
    ("trips", ["Voyages", "Trips", "Reisen", "Viajes", "Reizen"]),
    ("work", ["Travail", "Work", "Arbeit", "Trabajo", "Werk"]),
    ("other", ["Autres", "Other", "Sonstiges", "Otros", "Overig"]),
    ("health", ["Santé", "Health", "Gesundheit", "Salud", "Gezondheid"]),
    ("shopping", ["Courses", "Shopping", "Einkäufe", "Compras", "Boodschappen"]),
    ("holidays", ["Vacances", "Holidays", "Ferien", "Vacaciones", "Vakanties"]),
    ("school", ["École", "School", "Schule", "Colegio", "School"]),
    ("menus", ["Menus", "Menus", "Essen", "Menús", "Menu's"]),
];

/// A category's name in `lang` (English for another language; the id itself for an unknown one).
pub fn title(lang: &str, id: &str) -> String {
    let column = match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        "nl" => 4,
        _ => 1,
    };
    TITLES.iter().find(|(c, _)| *c == id).map(|(_, t)| t[column].to_string()).unwrap_or_else(|| id.to_string())
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
    // a list, or one written by hand in Obsidian as a plain value (`categories: money`)
    let written = match page.front.get("categories") {
        Some(Value::String(one)) => vec![one.clone()],
        other => super::strings(other),
    };
    let cats = valid(&written, &PERSON_MAY_GIVE)?;
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

/// A stem of the table: its text, whether it is a whole word only, its entry.
type Stem = (&'static str, bool, usize);

/// The table's one-word stems by their first two letters.
static STEMS: LazyLock<HashMap<String, Vec<Stem>>> = LazyLock::new(|| {
    let mut out: HashMap<String, Vec<Stem>> = HashMap::new();
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

/// A match of a text's words: its categories, how much it counts, the word (a rule's, a phrase or the text's word)
/// and where it starts (its first word's place among the text's folded words).
struct Match {
    categories: Vec<String>,
    factor: f64,
    word: String,
    at: usize,
}

/// Where `phrase` (folded words) stands among `words`, none of its words taken yet: each place.
fn places(words: &[&str], taken: &[bool], phrase: &[&str]) -> Vec<usize> {
    if phrase.is_empty() || phrase.len() > words.len() {
        return Vec::new();
    }
    (0..=words.len() - phrase.len())
        .filter(|at| phrase.iter().enumerate().all(|(k, p)| !taken[at + k] && words[at + k] == *p))
        .collect()
}

/// A word's entry of the table: its longest stem — a stem at the word's start, or the head of a compound
/// (« Mietvertrag », « Stromrechnung », « huurcontract »: a stem of 5 letters or more after 3 or more): the one ending
/// last wins (a compound's last part says what it is), then the longest.
fn stem_entry(word: &str) -> Option<(usize, &'static str)> {
    // folded text is ASCII (`fold`): two bytes are two letters
    let key = word.get(..2).unwrap_or(word);
    let prefix = STEMS
        .get(key)
        .into_iter()
        .flatten()
        .filter(|(body, exact, _)| if *exact { word == *body } else { word.starts_with(body) })
        .map(|(body, _, i)| (body.len(), body.len(), *i, *body));
    let heads = (3..word.len().saturating_sub(4)).filter(|at| word.is_char_boundary(*at)).flat_map(|at| {
        let rest = &word[at..];
        COMPOUND_HEADS
            .get(rest.get(..2).unwrap_or(rest))
            .into_iter()
            .flatten()
            .filter(move |(body, _)| rest.starts_with(body))
            .map(move |(body, i)| (at + body.len(), body.len(), *i, *body))
    });
    prefix.chain(heads).max_by_key(|(end, len, i, _)| (*end, *len, std::cmp::Reverse(*i))).map(|(_, _, i, body)| (i, body))
}

/// The table's entry for a text of one or more folded words: a phrase it is, else its first word's stem.
fn table_entry(text: &str) -> Option<usize> {
    let padded = format!(" {text} ");
    PHRASES
        .iter()
        .find(|(p, _, _)| padded.contains(p.as_str()))
        .map(|(_, _, i)| *i)
        .or_else(|| text.split(' ').find_map(|w| stem_entry(w).map(|(i, _)| i)))
}

/// The words the table does not know that their neighbours file (search_words.rs, the five languages together: a
/// Dutch person's Carnet is in English words): the first neighbour the table places decides; never a word that might
/// touch health (Santé stays the private part, decision 6), never through `NEIGHBOUR_NEVER_THROUGH`.
static NEIGHBOURS: LazyLock<HashMap<String, usize>> = LazyLock::new(|| {
    let mut merged: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
    for lang in super::search_words::LANGS {
        for (word, near) in super::search_words::WORDS.neighbours(lang) {
            let all = merged.entry(word.clone()).or_default();
            for n in near {
                if !all.contains(n) {
                    all.push(n.clone());
                }
            }
        }
    }
    let mut out = HashMap::new();
    for (word, near) in merged {
        let health = |w: &str| lexicon::touches_health(w) || DUTCH_HEALTH.contains(&w);
        let never = NEIGHBOUR_NEVER.contains(&word.as_str()) || health(&word) || near.iter().any(|n| health(n));
        if word.contains(' ') || never || table_entry(&word).is_some() {
            continue;
        }
        for n in near {
            if NEIGHBOUR_NEVER_THROUGH.contains(&n.as_str()) {
                break;
            }
            if let Some(i) = table_entry(&n) {
                out.insert(word, i);
                break;
            }
        }
    }
    out
});

/// The words' tables made now, before a caller holds the Carnet's lock (the neighbours read the five languages'
/// groups once, a few tenths of a second: measured as the S2 pass's longest hold of the lock, review 1 R5).
pub fn warm() {
    LazyLock::force(&STEMS);
    LazyLock::force(&PHRASES);
    LazyLock::force(&COMPOUND_HEADS);
    LazyLock::force(&NEIGHBOURS);
}

/// What `text`'s words say, each match where it starts — the person's rules first (their word counts double), then
/// the table's phrases, then each word's longest stem, then a word the table does not know by its neighbours.
fn matches(text: &str, rules: &[Rule]) -> Vec<Match> {
    let folded = fold(text);
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    let mut taken = vec![false; words.len()];
    let mut out = Vec::new();
    let take = |phrase: &str, categories: Vec<String>, factor: f64, taken: &mut [bool], out: &mut Vec<Match>| {
        let parts: Vec<&str> = phrase.split(' ').collect();
        let found = places(&words, taken, &parts);
        let Some(first) = found.first() else { return };
        out.push(Match { categories, factor, word: phrase.to_string(), at: *first });
        for at in found {
            for k in 0..parts.len() {
                taken[at + k] = true;
            }
        }
    };
    // a rule or a phrase looked for only when the text holds it (most texts hold none: a page's 40 facts each read)
    let padded = format!(" {folded} ");
    for r in rules.iter().filter(|r| padded.contains(&format!(" {} ", r.word))) {
        take(&r.word, r.categories.clone(), RULE, &mut taken, &mut out);
    }
    for (_, phrase, i) in PHRASES.iter().filter(|(p, _, _)| padded.contains(p.as_str())) {
        let factor = if WEAK_STEMS.contains(phrase) { WEAK } else { 1.0 };
        take(phrase, strings(lexicon::CATEGORY_WORDS[*i].0), factor, &mut taken, &mut out);
    }
    for (at, word) in words.iter().enumerate().filter(|(at, _)| !taken[*at]) {
        let (entry, factor) = match stem_entry(word) {
            Some((i, stem)) => (Some(i), if WEAK_STEMS.contains(&stem) { WEAK } else { 1.0 }),
            None => (NEIGHBOURS.get(*word).copied(), NEIGHBOUR),
        };
        if let Some(i) = entry {
            out.push(Match { categories: strings(lexicon::CATEGORY_WORDS[i].0), factor, word: word.to_string(), at });
        }
    }
    out
}

/// How many of a title's first folded words are its head: up to its first preposition (never its first word).
fn head_len(title: &str) -> usize {
    let folded = fold(title);
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    words.iter().skip(1).position(|w| PREPOSITIONS.contains(w)).map_or(words.len(), |at| at + 1)
}

/// The table's phrases (« entretien annuel »), padded (« ␣entretien annuel␣ »), with their entry.
static PHRASES: LazyLock<Vec<(String, &'static str, usize)>> = LazyLock::new(|| {
    let mut out = Vec::new();
    for (i, (_, stems)) in lexicon::CATEGORY_WORDS.iter().enumerate() {
        for stem in stems.iter().filter(|s| s.contains(' ')) {
            let body = stem.trim_end_matches('=');
            out.push((format!(" {body} "), body, i));
        }
    }
    out
});

/// The table's stems of 5 letters or more (not whole words only), by their first two letters: what a compound may end
/// with.
static COMPOUND_HEADS: LazyLock<HashMap<String, Vec<(&'static str, usize)>>> = LazyLock::new(|| {
    let mut out: HashMap<String, Vec<(&'static str, usize)>> = HashMap::new();
    for (i, (_, stems)) in lexicon::CATEGORY_WORDS.iter().enumerate() {
        for stem in stems.iter().filter(|s| !s.contains(' ') && !s.ends_with('=') && s.len() >= 5) {
            out.entry(stem.chars().take(2).collect()).or_default().push((*stem, i));
        }
    }
    out
});

/// The points of each category, and the word that gave each its most.
#[derive(Default)]
struct Points {
    total: HashMap<String, f64>,
    best: HashMap<String, (f64, String)>,
    /// The points of each category as an entry's first one (a bill's « money » aside): how sure the words are.
    lead: HashMap<String, f64>,
    /// A rule of the person's matched.
    ruled: bool,
    /// The points each category got from the title's words.
    titled: HashMap<String, f64>,
}

impl Points {
    fn add(&mut self, category: &str, points: f64, word: &str) {
        self.add_as(category, points, word, true);
    }

    /// `lead`: the match's first category (else an entry's second one, which counts in the total only).
    fn add_as(&mut self, category: &str, points: f64, word: &str, lead: bool) {
        if lead {
            *self.lead.entry(category.to_string()).or_default() += points;
        }
        *self.total.entry(category.to_string()).or_default() += points;
        let best = self.best.entry(category.to_string()).or_insert((0.0, String::new()));
        if points > best.0 {
            *best = (points, word.to_string());
        }
    }

    fn text(&mut self, text: &str, weight: f64, rules: &[Rule]) {
        self.weighed(text, rules, |_| weight);
    }

    /// `text`'s matches, each weighed by where it starts.
    fn weighed(&mut self, text: &str, rules: &[Rule], weight: impl Fn(usize) -> f64) {
        for m in matches(text, rules) {
            self.ruled |= m.factor == RULE;
            for (n, c) in m.categories.iter().enumerate() {
                self.add_as(c, weight(m.at) * m.factor * if n == 0 { 1.0 } else { SECOND }, &m.word, n == 0);
            }
        }
    }

    /// A title: its head (up to its first preposition) counts `TITLE`, what follows `MODIFIER`.
    fn title(&mut self, title: &str, rules: &[Rule]) {
        let head = head_len(title);
        let before = self.lead.clone();
        self.weighed(title, rules, |at| if at < head { TITLE } else { MODIFIER });
        for (c, p) in &self.lead {
            self.titled.insert(c.clone(), p - before.get(c).copied().unwrap_or(0.0));
        }
    }

    /// The categories, the best first (among equals, what kind of note it is: `TIES`), and the word that decided.
    /// « Autres » first (a hobby, a book, a series: the table's own entry) is « Autres » alone; never second.
    fn result(&self) -> (Vec<String>, Option<String>) {
        let mut ranked: Vec<(&String, &f64)> = self.total.iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal).then(tie(a.0).cmp(&tie(b.0))));
        // a hobby's word never beats what another word of the title says (review 1, R2: « Photos du mariage » is
        // Personnes, « Cours de piano » stays in « Autres »)
        if ranked.first().is_some_and(|(c, _)| c.as_str() == "other")
            && self.titled.iter().any(|(c, p)| c != "other" && *p >= MODIFIER)
        {
            ranked.retain(|(c, _)| c.as_str() != "other");
        }
        let Some((top, points)) = ranked.first().map(|(c, p)| ((*c).clone(), **p)) else {
            return (vec!["other".into()], None);
        };
        if points < ENOUGH {
            return (vec!["other".into()], None);
        }
        let mut cats = vec![top.clone()];
        if let Some((second, p)) = ranked.iter().skip(1).find(|(c, _)| top != "other" && c.as_str() != "other") {
            if **p >= BESIDE * points {
                cats.push((*second).clone());
            }
        }
        let word = self.best.get(&top).map(|(_, w)| w.clone()).filter(|w| !w.is_empty());
        (cats, word)
    }
}

/// A note's categories by its words alone, and the word that decided (the family Carnet's, family_carnet.rs: no kind,
/// no group, no work agent): `title` read as a page's title, `body` as its lines.
pub fn of_words(title: &str, body: &str, lang: &str, rules: &[Rule]) -> (Vec<String>, Option<String>) {
    by_words(&Page { front: Dict::new(), title: title.to_string(), body: body.to_string() }, lang, rules)
}

/// A subject's or an area's categories from its words: the title, its aliases, its « En bref », its labels and facts,
/// its « Chiffres et budget » and its amounts (lang: the Carnet's).
fn by_words(page: &Page, lang: &str, rules: &[Rule]) -> (Vec<String>, Option<String>) {
    scored(page, lang, rules).result()
}

/// The points of a page's words (`by_words`).
fn scored(page: &Page, lang: &str, rules: &[Rule]) -> Points {
    let brief = super::words(lang).brief;
    let mut points = Points::default();
    points.title(&page.title, rules);
    points.text(&page.aliases().join(" "), ALIASES, rules);
    let summary = super::summary_of(&page.body, brief);
    points.text(&summary, if points.total.is_empty() { SUMMARY } else { SUMMARY_AFTER_TITLE }, rules);
    if super::news::mentions_money(&summary) {
        points.add("money", AMOUNT, "");
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
        points.add_as(&category, total.min(FACTS_CAP), &word, false);
        let lead = facts.lead.get(&category).copied().unwrap_or(0.0);
        *points.lead.entry(category).or_default() += lead.min(FACTS_CAP);
    }
    points.ruled |= facts.ruled;
    points
}

/// Whether the words placed a page narrowly (carnet-categories, 22/11: what the night model reads on the one-time
/// sort, categories_pass.rs `candidates_once`): filed by its words (not by its kind, its group or a work agent), no rule
/// of the person's matched (they apply first), not « Autres » (sent as such), and either nothing strong placed it —
/// its first category has less than `NARROW_BELOW` points as an entry's first one (a table word of the title's head
/// counts 5, after its preposition 4: so only its first line, its lines, a neighbour or an amount did) — or another
/// category is as strong as it (`NARROW_CLOSE` of its points or more, a bill's « money » beside « papers » aside).
pub fn narrow_in(ctx: &Ctx, part: Part, path: &str, page: &Page) -> bool {
    if part != Part::Main {
        return false;
    }
    let w = super::words(ctx.lang());
    let kind = page.kind();
    if matches!(kind.as_str(), "self" | "trip" | "tasks" | "documents" | "records" | "person")
        || path == w.inbox_path()
        || path == w.tasks_path()
        || groups::group_relation(&page.title).is_some()
    {
        return false;
    }
    let points = scored(page, ctx.lang(), ctx.rules());
    let (cats, _) = points.result();
    if points.ruled || cats == ["other"] {
        return false;
    }
    let top = points.lead.get(&cats[0]).copied().unwrap_or(0.0);
    let runner = points.lead.iter().filter(|(c, _)| **c != cats[0] && c.as_str() != "other").map(|(_, p)| *p).fold(0.0, f64::max);
    top < NARROW_BELOW || runner >= NARROW_CLOSE * top
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

/// What filing reads once a request, the first time a page needs it: the Carnet's language and the person's rules
/// (state.json). A page whose categories are written needs neither (Review 1, R1: /pages, /tasks and /search read
/// them once, not twice a page).
pub struct Ctx<'a> {
    pub config: &'a Config,
    read: std::cell::OnceCell<(String, Vec<Rule>)>,
}

impl<'a> Ctx<'a> {
    pub fn new(config: &'a Config) -> Self {
        Ctx { config, read: std::cell::OnceCell::new() }
    }

    fn read(&self) -> &(String, Vec<Rule>) {
        self.read.get_or_init(|| (super::language(self.config), rules(self.config)))
    }

    pub fn lang(&self) -> &str {
        &self.read().0
    }

    fn rules(&self) -> &[Rule] {
        &self.read().1
    }
}

/// The bridge's categories for a page, and the word that decided (None: its kind, its group or its type did):
/// the private part is Santé; « Toi » and people are Gens (a colleague also Travail); the group pages by their
/// relation; trips; records pages by type; the tasks page and the inbox « Autres »; a page a work agent (`writer`)
/// makes, Travail; the others by their words (the person's rules first).
pub fn decide(config: &Config, part: Part, path: &str, page: &Page, writer: Option<&str>) -> (Vec<String>, Option<String>) {
    decide_in(&Ctx::new(config), part, path, page, writer)
}

pub fn decide_in(ctx: &Ctx, part: Part, path: &str, page: &Page, writer: Option<&str>) -> (Vec<String>, Option<String>) {
    if part == Part::Health {
        return (vec!["health".into()], None);
    }
    let config = ctx.config;
    let lang = ctx.lang().to_string();
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
    by_words(page, &lang, ctx.rules())
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
    of_page_in(&Ctx::new(config), part, path, page)
}

pub fn of_page_in(ctx: &Ctx, part: Part, path: &str, page: &Page) -> Filed {
    if part == Part::Health {
        return Filed { categories: vec!["health".into()], by: "auto".into(), stored: false };
    }
    match stored(page) {
        Some((categories, by)) => Filed { categories, by, stored: true },
        None => Filed { categories: decide_in(ctx, part, path, page, None).0, by: "auto".into(), stored: false },
    }
}

/// An agent's `category` written on the page (valid ones of `AGENT_MAY_GIVE`), unless the person chose: whether it
/// changed.
pub fn choose(page: &mut Page, chosen: &[String], who: &Who) -> bool {
    let Some(cats) = valid(chosen, &AGENT_MAY_GIVE).filter(|_| who.agent.is_some()) else { return false };
    if hand_written(page) {
        return false;
    }
    let now = stored(page);
    if now.as_ref().is_some_and(|(_, by)| by == "person") || now == Some((cats.clone(), "agent".into())) {
        return false;
    }
    set_front(page, &cats, "agent");
    true
}

/// At a write of a page of the shared part (an agent's notebook_write, the app's new page): its categories written
/// when it has none yet (or only « Autres » from the bridge, filed again). A page already filed keeps its categories;
/// `categories` written by hand that the bridge cannot read are never touched. Who writes counts only for a page this
/// write makes (`created`: a work agent's new page is Travail; its first write on « Maison » does not make it so).
/// Whether they changed.
pub fn on_write(config: &Config, part: Part, path: &str, page: &mut Page, who: &Who, created: bool) -> bool {
    if part != Part::Main || hand_written(page) {
        return false;
    }
    let now = stored(page);
    if now.as_ref().is_some_and(|(c, by)| !(by == "auto" && c == &["other"])) {
        return false;
    }
    let writer = if created { who.agent.as_deref() } else { None };
    let (cats, _) = decide(config, part, path, page, writer);
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

/// A date a note carries: a yearly one (a birthday, a wedding anniversary: month and day), or a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Yearly(u32, u32),
    On(NaiveDate),
}

impl When {
    /// Its next day from `today` (a yearly one's next occurrence).
    pub fn next(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            When::On(d) => Some(d),
            When::Yearly(m, d) => match NaiveDate::from_ymd_opt(today.year(), m, d) {
                Some(this) if this >= today => Some(this),
                _ => (1..=8).find_map(|n| NaiveDate::from_ymd_opt(today.year() + n, m, d)),
            },
        }
    }

    /// In « Dates » on `today`: a yearly date always (its next occurrence is within the year), a day within the window.
    pub fn dated(self, today: NaiveDate) -> bool {
        match self {
            When::Yearly(..) => true,
            When::On(d) => in_window(d, today),
        }
    }

    /// The index's fields (contracts/carnet/sommaire.json `IndexItem`): `date` (a day) and `yearly` (« MM-DD »).
    pub fn fields(when: Option<When>) -> (Value, Value) {
        match when {
            Some(When::On(d)) => (json!(d.format("%Y-%m-%d").to_string()), Value::Null),
            Some(When::Yearly(m, d)) => (Value::Null, json!(format!("{m:02}-{d:02}"))),
            None => (Value::Null, Value::Null),
        }
    }
}

/// The Dutch words of a yearly date (mod.rs `YEARLY` has the four other languages').
static DUTCH_YEARLY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"verjaardag|jarig|trouwdag|huwelijksdag").expect("valid regex"));

/// « 1/2 tasse », « 3/4 » : a number over another of one digit each, without a year and without a word that says a
/// day before it, is not a date here; « 14/11 », « 14/2 », « le 3/11 », « le 3/4 » are (review 2 of S1, R11).
static FRACTION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(\b(?:le|du|au|on|the|am|vom|bis|el|del|op|tot|until|jusqu au|avant le|apres le)\s+)?\b(\d{1,2})/(\d{1,2})\b(/\d{4})?")
        .expect("valid regex")
});

/// The date a note's text carries. Yearly when the text says so (a birthday, an anniversary, a wedding: mod.rs
/// `YEARLY`, or its label); a day with its year as written; a day without a year (« devis reçu le 3 octobre ») is
/// the one nearest to when it was noted (`noted`, else `today`), never every year.
pub fn when_of(text: &str, noted: Option<NaiveDate>, today: NaiveDate) -> Option<When> {
    let mut plain = tk::ascii_fold(&text.to_lowercase());
    plain = SPOKEN.replace_all(&plain, "$1 $2").into_owned();
    for (nl, en) in DUTCH_MONTHS.iter() {
        plain = nl.replace_all(&plain, *en).into_owned();
    }
    plain = FRACTION
        .replace_all(&plain, |c: &regex::Captures| {
            if c.get(1).is_some() || c.get(4).is_some() || c[2].len() == 2 || c[3].len() == 2 {
                c[0].to_string()
            } else {
                " ".to_string()
            }
        })
        .into_owned();
    let (month, day, year) = super::date_in(&plain)?;
    let birthday = lexicon::split_label(text).and_then(|(l, _)| lexicon::label_of(&l)).is_some_and(|l| l.id == "birthday");
    if super::YEARLY.is_match(&plain) || DUTCH_YEARLY.is_match(&plain) || birthday {
        return NaiveDate::from_ymd_opt(2000, month, day).map(|_| When::Yearly(month, day));
    }
    if let Some(y) = year {
        return NaiveDate::from_ymd_opt(y, month, day).map(When::On);
    }
    let around = noted.unwrap_or(today);
    [around.year() - 1, around.year(), around.year() + 1]
        .into_iter()
        .filter_map(|y| NaiveDate::from_ymd_opt(y, month, day))
        .min_by_key(|d| (*d - around).num_days().abs())
        .map(When::On)
}

/// The next day of the date a text carries (tests, the fake bridge).
pub fn date_of(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    when_of(text, None, today)?.next(today)
}

pub fn dated(text: &str, noted: Option<NaiveDate>, today: NaiveDate) -> bool {
    when_of(text, noted, today).is_some_and(|w| w.dated(today))
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
    marks_with_papers(&super::read_state(config))
}

fn marks_in(state: &Dict) -> Dict {
    state.get("category_marks").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// [`marks`] and, below the person's, the tasks the bridge made for a paper's expiry (papers.rs: no page, but a
/// paper's): Papiers.
fn marks_with_papers(state: &Dict) -> Dict {
    let mut m = marks_in(state);
    for ids in state.get("papers").and_then(Value::as_object).into_iter().flat_map(|p| p.values()) {
        for id in ids.as_array().into_iter().flatten().filter_map(Value::as_str) {
            m.entry(task_key(id)).or_insert(json!({"categories": ["papers"], "by": "auto"}));
        }
    }
    m
}

/// Who filed a marked item: the person, or the bridge (a paper's tasks).
fn marked_by(marks: &Dict, key: &str) -> &'static str {
    match marks.get(key).and_then(|m| m.get("by")).and_then(Value::as_str) {
        Some("auto") => "auto",
        _ => "person",
    }
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

/// A document's mark: its agent and its file's name hashed (the state never keeps a file's name: « Oublie ça »).
pub fn document_key(agent: &str, file: &str) -> String {
    format!("doc:{agent}:{}", &super::rev(file)[..12])
}

/// A page's key: its id, else (a page made by hand, no id yet) a hash of its path.
pub fn page_key(page: &Page, path: &str) -> String {
    let id = page.id();
    if id.is_empty() { format!("f{}", &super::rev(path)[..10]) } else { id }
}

/// A line's key: its page's key and its text folded, hashed (never the text).
pub fn line_key(page: &str, text: &str) -> String {
    let (text, _) = super::split_attribution(text.trim().trim_start_matches("- "));
    format!("line:{page}:{}", &super::rev(&fold(&text))[..10])
}

/// The Item ids of a page's lines, in order: their key, and « :2 », « :3 »… for the same line said again.
pub fn line_ids(page: &str, texts: &[String]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    texts
        .iter()
        .map(|t| {
            let key = line_key(page, t);
            let n = seen.entry(key.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { key } else { format!("{key}:{n}") }
        })
        .collect()
}

/// « 250g », « 100km », « 1,5l », « 2x »: a quantity, not a reference.
static QUANTITY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)^\d+(?:[.,]\d+)?(?:g|kg|mg|km|m|cm|mm|ml|cl|dl|l|h|min|mn|ko|mo|go|kb|mb|gb|tb|w|kw|kwh|v|j|e|x)$")
        .expect("valid regex")
});

/// « 12h30 »: a time, not a reference.
static TIME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\d{1,2}h\d{0,2}$").expect("valid regex"));

/// Whether a text holds a reference or a number that never goes to the phone's copy: what the offline copy leaves out
/// (offline.rs `numbered`: an ID number, a word of 6 characters or more with 3 digits or more), and a code of letters
/// and digits (« HR-55821 », « CSG-2291 », « ABC123 ») — not a price, a date, a year or a time.
pub fn holds_reference(text: &str) -> bool {
    // « 250g », « 100km », « 1,5l »: quantities, never references (review 2 of S1, R11)
    let kept: Vec<&str> = text
        .split_whitespace()
        .filter(|w| !QUANTITY.is_match(w.trim_matches(|c: char| !c.is_alphanumeric() && c != ',' && c != '.')))
        .collect();
    let text = kept.join(" ");
    if super::offline::numbered(&text) {
        return true;
    }
    text.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        let digits = w.chars().filter(|c| c.is_ascii_digit()).count();
        digits >= 3 && w.chars().any(char::is_alphabetic) && !TIME.is_match(&w.to_lowercase())
    })
}

/// A record's line for the phone (the index, contracts/carnet/sommaire.json `records`): its values without its
/// number, its booking reference or its source, and no value that holds a reference.
pub fn record_line(t: &records::Kind, record: &Dict) -> String {
    t.fields
        .iter()
        .filter(|f| !["number", "reference", "source"].contains(&f.id))
        .map(|f| tk::str_or_empty(record.get(f.id)))
        .filter(|v| !v.is_empty() && !holds_reference(v))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The date a record is in « Dates » by: its next one (a warranty's end, a renewal, an end, an expiry, a booking's
/// day), never a purchase's day.
pub fn record_when(t: &records::Kind, record: &Dict) -> Option<When> {
    let text = tk::str_or_empty(record.get(t.next));
    let (m, d, y) = super::date_in(&text)?;
    NaiveDate::from_ymd_opt(y?, m, d).map(When::On)
}

/// A record's categories (by its type; a purchase with a warranty also Papiers), by whom, and whether it is dated.
pub fn of_record(t: &records::Kind, record: &Dict, marks: &Dict, today: NaiveDate) -> (Vec<String>, String, bool) {
    let dated = record_when(t, record).is_some_and(|w| w.dated(today));
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
pub fn of_task(ctx: &Ctx, task: &Task, pages: &[(String, Page)], marks: &Dict, today: NaiveDate) -> Vec<String> {
    let cats = marked(marks, &task_key(&task.id)).unwrap_or_else(|| match task_page(task, pages) {
        Some((path, page)) => of_page_in(ctx, task.part, path, page).categories,
        None if task.part == Part::Health => vec!["health".into()],
        None => vec!["other".into()],
    });
    let due = NaiveDate::parse_from_str(&task.due, "%Y-%m-%d").ok();
    with_dates(&cats, due.is_some_and(|d| in_window(d, today)))
}

/// A document's categories (GET /v1/notebook/documents' item), by whom, and the word of its name that decided: the
/// person's choice; the page it is linked to or named on; its name's words; else Papiers (Alex's decision 4).
pub fn of_document(ctx: &Ctx, doc: &Value, pages: &[(String, Page)], marks: &Dict) -> (Vec<String>, String, Option<String>) {
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
            return (of_page_in(ctx, part, path, page).categories, "auto".into(), None);
        }
    }
    let name = tk::str_or_empty(doc.get("name"));
    let stem = std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name);
    let named = Page::new("subject", &stem);
    match by_words(&named, ctx.lang(), ctx.rules()) {
        (cats, word) if cats != ["other"] => (cats, "auto".into(), word),
        _ => (vec!["papers".into()], "auto".into(), None),
    }
}

/// A line's categories: its page's (`page_categories`), the person's own choice first, `dates` when it carries a date.
/// `noted`: when the line was noted (its attribution's day, YYYY-MM-DD), when `text` comes without it.
pub fn of_line(page_categories: &[String], page: &str, text: &str, noted: &str, marks: &Dict, today: NaiveDate) -> Vec<String> {
    let cats = marked(marks, &line_key(page, text)).unwrap_or_else(|| page_categories.to_vec());
    with_dates(&cats, line_when(text, noted, today).is_some_and(|w| w.dated(today)))
}

/// A line's date (its text's, never its attribution's « — *Léa, 2026-10-22* », which says when it was noted: the
/// day a date without a year is read near).
pub fn line_when(text: &str, noted: &str, today: NaiveDate) -> Option<When> {
    let (plain, by) = super::split_attribution(text.trim().trim_start_matches("- "));
    let noted = by.map(|(_, d)| d).unwrap_or_else(|| noted.to_string());
    when_of(&plain, NaiveDate::parse_from_str(&noted, "%Y-%m-%d").ok(), today)
}

/// The documents' list with each one's categories (`categories`, `categories_by`), for the apps.
pub fn with_documents(config: &Config, documents: Vec<Value>, part: Part) -> Vec<Value> {
    if documents.is_empty() {
        return documents;
    }
    let ctx = Ctx::new(config);
    let pages = super::all_pages(config, part);
    let marks = marks(config);
    documents
        .into_iter()
        .map(|mut d| {
            let (cats, by, _) = if part == Part::Health {
                (vec!["health".to_string()], "auto".to_string(), None)
            } else {
                of_document(&ctx, &d, &pages, &marks)
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
pub fn task_view(ctx: &Ctx, task: &Task, pages: &[(String, Page)], marks: &Dict, today: NaiveDate) -> Value {
    let mut v = task.view();
    v["categories"] = json!(of_task(ctx, task, pages, marks, today));
    v
}

// -- the person's correction (« Ranger ailleurs ») -------------------------------------------------------------------

/// What the person files elsewhere: a page (its path), a record, a document, a task, a line of a page, or an Item's
/// id (the index's, a search result's: contracts/carnet/sommaire.json `Item`).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Page { path: String },
    Record { kind: String, id: String },
    Document { agent: String, file: String },
    Task(String),
    Line { path: String, text: String },
    Item(String),
}

impl Target {
    pub fn view(&self) -> Value {
        match self {
            Target::Page { path } => json!({"path": path, "private": false}),
            Target::Record { kind, id } => json!({"record": {"type": kind, "id": id}}),
            Target::Document { agent, file } => json!({"document": {"agent": agent, "file": file}}),
            Target::Task(id) => json!({"task": id}),
            Target::Line { path, text } => json!({"line": {"path": path, "text": text}}),
            Target::Item(id) => json!({"item": id}),
        }
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
    /// The page's `categories` were written by hand (in Obsidian) in a way the bridge does not read: left as they are.
    HandWritten,
}

impl Refused {
    pub fn code(&self) -> &'static str {
        match self {
            Refused::Unknown => "category_unknown",
            Refused::Count => "category_count",
            Refused::NotGiven => "category_not_given",
            Refused::NotFound => "not_found",
            Refused::HandWritten => "categories_by_hand",
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

/// Whether a page has `categories` the bridge cannot read (written by hand: « [[Livres]] », three values…): the
/// bridge never touches them.
pub fn hand_written(page: &Page) -> bool {
    page.front.contains_key("categories") && stored(page).is_none()
}

pub(crate) fn line_on(page: &Page, text: &str) -> Option<String> {
    let wanted = fold(&super::split_attribution(text.trim().trim_start_matches("- ")).0);
    page.sections().into_iter().flat_map(|(_, f)| f).find(|f| fold(&f.text) == wanted).map(|f| f.text)
}

fn page_by_key(config: &Config, key: &str) -> Option<(String, Page)> {
    super::all_pages(config, Part::Main).into_iter().find(|(path, page)| page_key(page, path) == key)
}

/// An Item's id resolved to what it names (None: nothing of that id any more).
pub(crate) fn resolve(config: &Config, id: &str) -> Option<Target> {
    if let Some(key) = id.strip_prefix("page:") {
        return page_by_key(config, key).map(|(path, _)| Target::Page { path });
    }
    if let Some(rest) = id.strip_prefix("line:") {
        let (key, _) = rest.split_once(':')?;
        let (path, page) = page_by_key(config, key)?;
        let texts: Vec<String> = page.sections().into_iter().flat_map(|(_, f)| f).map(|f| f.text).collect();
        let ids = line_ids(key, &texts);
        let i = ids.iter().position(|x| x == id)?;
        return Some(Target::Line { path, text: texts[i].clone() });
    }
    if let Some(task) = id.strip_prefix("task:") {
        return Some(Target::Task(task.into()));
    }
    if let Some(record) = id.strip_prefix("record:") {
        let t = records::TYPES.iter().find(|t| {
            records::records(config, t).into_iter().chain(records::archived_records(config, t)).any(|r| tk::str_or_empty(r.get("id")) == record)
        })?;
        return Some(Target::Record { kind: t.id.into(), id: record.into() });
    }
    if id.starts_with("doc:") {
        return super::documents(config, Part::Main).iter().find_map(|d| {
            let (agent, file) = (tk::str_or_empty(d.get("agent")), tk::str_or_empty(d.get("file")));
            (document_key(&agent, &file) == id).then_some(Target::Document { agent, file })
        });
    }
    None
}

/// « Ranger ailleurs »: `target` filed in `given` (1 or 2 of `PERSON_MAY_GIVE`) by the person. A page gets
/// `categories_by: person` (and, when the bridge itself had filed it by a word, the rule « word → these categories »
/// for the next writes); a record, task, document or line a mark in the state. The state keeps keys only (a page's,
/// a line's hash, a file's hash), never a text, a path or a file's name (« Oublie ça »). The answer says what it was
/// and how to undo.
pub fn categorize(config: &Config, target: &Target, given: &[String], who: &Who) -> anyhow::Result<Result<Value, Refused>> {
    let cats = match checked(given) {
        Ok(c) => c,
        Err(r) => return Ok(Err(r)),
    };
    let _guard = super::lock();
    let target = match target {
        Target::Item(id) => match resolve(config, id) {
            Some(t) => t,
            None => return Ok(Err(Refused::NotFound)),
        },
        t => t.clone(),
    };
    let today = chrono::Local::now().date_naive();
    let marks = marks(config);
    // what it was, its key in the state, the word that decided it (pages and documents filed by the bridge only)
    let (before, key, word, item): (Filed, String, Option<String>, String) = match &target {
        Target::Page { path } => {
            if super::checked(path).is_err() {
                return Ok(Err(Refused::NotFound));
            }
            let Some(text) = super::read_page_text(config, Part::Main, path) else { return Ok(Err(Refused::NotFound)) };
            let mut page = Page::parse(&text, "");
            if hand_written(&page) {
                return Ok(Err(Refused::HandWritten));
            }
            let before = of_page(config, Part::Main, path, &page);
            let (auto, word) = decide(config, Part::Main, path, &page, None);
            // (a page made by hand keeps its key from its path: its header gets the categories' lines only, never an
            // id: S2 review 1, R10; review 1 of S3)
            let key = format!("page:{}", page_key(&page, path));
            // its header lines only: a page written by hand keeps its frontmatter as it is (S2 review 1, R10)
            let after = super::categories_pass::with_header(&text, &cats, "person").unwrap_or_else(|| {
                set_front(&mut page, &cats, "person");
                page.render()
            });
            if after != text {
                super::commit(config, Part::Main, path, Some(&text), Some(&after), who, ACTION, "")?;
            }
            let word = word.filter(|_| auto != cats && before.by == "auto");
            (before, key.clone(), word, key)
        }
        Target::Record { kind, id } => {
            let Some(t) = records::kind(kind) else { return Ok(Err(Refused::NotFound)) };
            let all = records::records(config, t).into_iter().chain(records::archived_records(config, t));
            let Some(record) = all.into_iter().find(|r| tk::str_or_empty(r.get("id")) == *id) else {
                return Ok(Err(Refused::NotFound));
            };
            let (c, by, _) = of_record(t, &record, &marks, today);
            (Filed { categories: c, stored: by == "person", by }, record_key(id), None, record_key(id))
        }
        Target::Document { agent, file } => {
            let docs = super::documents(config, Part::Main);
            let Some(doc) = docs.iter().find(|d| tk::str_or_empty(d.get("agent")) == *agent && tk::str_or_empty(d.get("file")) == *file) else {
                return Ok(Err(Refused::NotFound));
            };
            let pages = super::all_pages(config, Part::Main);
            let (c, by, word) = of_document(&Ctx::new(config), doc, &pages, &marks);
            let word = word.filter(|_| c != cats && by == "auto");
            let key = document_key(agent, file);
            (Filed { categories: c, stored: by == "person", by }, key.clone(), word, key)
        }
        Target::Task(id) => {
            let Some(task) = super::tasks(config, Part::Main).into_iter().find(|t| t.id == *id) else {
                return Ok(Err(Refused::NotFound));
            };
            let pages = super::all_pages(config, Part::Main);
            let c: Vec<String> = of_task(&Ctx::new(config), &task, &pages, &marks, today).into_iter().filter(|c| c != "dates").collect();
            let by = if marked(&marks, &task_key(id)).is_some() { marked_by(&marks, &task_key(id)) } else { "auto" };
            (Filed { categories: c, stored: by == "person", by: by.into() }, task_key(id), None, task_key(id))
        }
        Target::Line { path, text } => {
            let Some(page_text) = super::checked(path).ok().and_then(|_| super::read_page_text(config, Part::Main, path)) else {
                return Ok(Err(Refused::NotFound));
            };
            let page = Page::parse(&page_text, "");
            let Some(line) = line_on(&page, text) else { return Ok(Err(Refused::NotFound)) };
            let key = line_key(&page_key(&page, path), &line);
            let c = marked(&marks, &key).unwrap_or_else(|| of_page(config, Part::Main, path, &page).categories);
            let by = if marked(&marks, &key).is_some() { "person" } else { "auto" };
            (Filed { categories: c, stored: by == "person", by: by.into() }, key.clone(), None, key)
        }
        Target::Item(_) => return Ok(Err(Refused::NotFound)),
    };
    let is_page = key.starts_with("page:");
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
        if !is_page {
            let mut m = marks_in(state);
            m.shift_remove(&key);
            m.insert(key.clone(), json!({"categories": cats, "at": super::today()}));
            if m.len() > MARKS_KEPT {
                let extra = m.len() - MARKS_KEPT;
                m = m.into_iter().skip(extra).collect();
            }
            state.insert("category_marks".into(), Value::Object(m));
        }
        // one undo a target: the newest (an older one would put back what this one replaced)
        let mut list: Vec<Value> = state.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
        list.retain(|e| tk::str_or_empty(e.get("key")) != key);
        list.insert(0, json!({"id": undo, "key": key, "set": cats, "before": {"categories": before.categories,
                              "categories_by": before.by, "stored": before.stored},
                              "rule": word, "rule_before": rule_before, "at": super::today(),
                              "categories": cats, "time": tk::now_iso_seconds()}));
        keep_undo(&mut list, &super::today());
        state.insert("category_undo".into(), Value::Array(list));
    })?;
    let mut shown = target.view();
    shown["item"] = json!(item);
    Ok(Ok(json!({"target": shown, "categories": cats, "categories_by": "person",
                 "before": {"categories": before.categories, "categories_by": before.by},
                 "learnt": learnt, "undo": undo, "toast": Value::Null})))
}

/// The corrections kept for « Annuler » (the newest first): the `UNDO_KEPT` latest, and today's up to
/// `UNDO_TODAY_MAX` (each re-filing of the day stays undoable from its list, S2).
pub(crate) fn keep_undo(list: &mut Vec<Value>, today: &str) {
    let mut n = 0;
    list.retain(|e| {
        n += 1;
        n <= UNDO_KEPT || (n <= UNDO_TODAY_MAX && tk::str_or_empty(e.get("at")) == today)
    });
}

/// The history's action of a correction.
pub const ACTION: &str = "categorize";

/// « Annuler »: the correction `id` undone (the categories as they were, the rule it taught forgotten). None: no
/// such correction any more (20 newer ones, a newer one on the same target, or undone already). A page whose
/// categories changed since (by hand, by another correction) is left as it is now.
pub fn undo(config: &Config, id: &str, who: &Who) -> anyhow::Result<Option<Value>> {
    let _guard = super::lock();
    let state = super::read_state(config);
    let list: Vec<Value> = state.get("category_undo").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(entry) = list.iter().find(|e| tk::str_or_empty(e.get("id")) == id).cloned() else { return Ok(None) };
    let key = tk::str_or_empty(entry.get("key"));
    let before = &entry["before"];
    let was = valid(&super::strings(before.get("categories")), &PERSON_MAY_GIVE).unwrap_or_else(|| vec!["other".into()]);
    let was_by = tk::str_or_empty(before.get("categories_by"));
    let was_by = if was_by.is_empty() { "auto".to_string() } else { was_by };
    let stored_before = before.get("stored").and_then(Value::as_bool).unwrap_or(false);
    let set = valid(&super::strings(entry.get("set")), &PERSON_MAY_GIVE).unwrap_or_default();
    let rule = entry.get("rule").and_then(Value::as_str).map(str::to_string);
    let rule_before = valid(&super::strings(entry.get("rule_before")), &PERSON_MAY_GIVE);
    // a page gone since: nothing to undo on it any more (review 2 of S1, R12: `undo_gone`, never the old state);
    // what the correction taught and its undo are forgotten all the same (review 1 of S3, R6)
    let page = key.strip_prefix("page:").and_then(|page_id| page_by_key(config, page_id));
    let gone = key.starts_with("page:") && page.is_none();
    super::update_state(config, |state| {
        if !key.starts_with("page:") {
            let mut m = marks_in(state);
            if stored_before {
                m.insert(key.clone(), json!({"categories": was, "at": super::today()}));
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
    if gone {
        return Ok(None);
    }
    let mut target = json!({"item": key});
    let mut now = Filed { categories: was.clone(), by: was_by.clone(), stored: stored_before };
    // the page as it was, read once the rule is forgotten (a page the bridge had not written categories on is read
    // with its words again); untouched when its categories are no longer the ones this correction set
    if key.starts_with("page:") {
        if let Some((path, mut page)) = page {
            target = json!({"path": path, "private": false, "item": key});
            if stored(&page) == Some((set.clone(), "person".into())) {
                // its header lines only, as the correction wrote them (S2 review 1, R10)
                let old = super::read_page_text(config, Part::Main, &path).unwrap_or_default();
                let lines = if stored_before {
                    super::categories_pass::with_header(&old, &was, &was_by)
                } else {
                    super::categories_pass::without_header(&old)
                };
                let after = lines.unwrap_or_else(|| {
                    if stored_before {
                        set_front(&mut page, &was, &was_by);
                    } else {
                        clear_front(&mut page);
                    }
                    page.render()
                });
                if after != old {
                    super::commit(config, Part::Main, &path, Some(&old), Some(&after), who, ACTION, "")?;
                }
                let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                page = Page::parse(&after, &stem);
            }
            now = of_page(config, Part::Main, &path, &page);
        }
    }
    Ok(Some(json!({"target": target, "categories": now.categories, "categories_by": now.by, "unlearnt": rule})))
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

    /// Review 1 (R6): 50 page titles a language that are not the demo's (contracts/carnet/category-titles.json, written
    /// before measuring, never tuned on): the bridge's first category is the one meant.
    #[test]
    fn titles_outside_the_demo() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/carnet/category-titles.json");
        let cases: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let mut report = Vec::new();
        for (lang, list) in cases["cases"].as_object().unwrap() {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            let (mut first, mut among, mut misses) = (0, 0, Vec::new());
            let list = list.as_array().unwrap();
            for case in list {
                let (title, want) = (case["title"].as_str().unwrap(), case["category"].as_str().unwrap());
                let got = decided(&config, &page_in(lang, "subject", title, "", ""));
                first += usize::from(got[0] == want);
                among += usize::from(got.iter().any(|c| c == want));
                if got[0] != want {
                    misses.push(format!("{title} → {}", got.join("+")));
                }
            }
            let n = list.len() as f64;
            report.push((lang.clone(), first as f64 / n, among as f64 / n, misses));
        }
        for (lang, first, among, misses) in &report {
            eprintln!("TITLES {lang}: first {:.0} %, among {:.0} %, misses {misses:?}", 100.0 * first, 100.0 * among);
        }
        for (lang, first, _, _) in &report {
            assert!(*first >= FLOOR, "{lang}: {:.0} % (floor {:.0} %)", 100.0 * first, 100.0 * FLOOR);
        }
    }

    /// The floor the held-out titles may not go below (measured on 22/11: see the status; 76–82 % with S1's table,
    /// 92–98 % with carnet-categories').
    const FLOOR: f64 = 0.88;

    /// The categories' development set (contracts/carnet/category-dev-set.json, carnet-categories 22/11: written for
    /// this work without the blind set): each page read as the blind set's measure reads it, a new subject page with its
    /// title and its first line as its « En bref ». Per language: the first category right, the one meant among the
    /// two, « Autres » on a real category, the traps (printed with --nocapture). The floors: the table's measure.
    #[test]
    fn the_dev_set_is_filed_where_a_person_looks() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/carnet/category-dev-set.json");
        let set: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let (mut all_n, mut all_first, mut all_among) = (0, 0, 0);
        let mut lines = Vec::new();
        let mut low = Vec::new();
        for (lang, cases) in set["cases"].as_object().unwrap() {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            let (mut n, mut first, mut among, mut real, mut autres, mut traps, mut traps_right) = (0, 0, 0, 0, 0, 0, 0);
            let mut misses = Vec::new();
            for case in cases.as_array().unwrap() {
                let (title, line) = (case["title"].as_str().unwrap(), case["first_line"].as_str().unwrap_or(""));
                let want = crate::notebook::strings(case.get("expected"));
                let got = decided(&config, &page_in(lang, "subject", title, line, ""));
                n += 1;
                first += usize::from(got[0] == want[0]);
                among += usize::from(got.contains(&want[0]));
                if want[0] != "other" {
                    real += 1;
                    autres += usize::from(got == ["other"]);
                }
                if case["trap"].as_bool() == Some(true) {
                    traps += 1;
                    traps_right += usize::from(got.contains(&want[0]));
                }
                if got[0] != want[0] {
                    misses.push(format!("{title} → {} (want {})", got.join("+"), want.join("+")));
                }
            }
            let pct = |x: usize, of: usize| 100.0 * x as f64 / of.max(1) as f64;
            lines.push(format!(
                "DEV {lang}: n {n}, first {:.1} %, among the two {:.1} %, « Autres » on a real category {:.1} %, traps {traps_right}/{traps}; misses: {}",
                pct(first, n), pct(among, n), pct(autres, real), misses.join(" · ")
            ));
            if pct(first, n) < 100.0 * DEV_FLOOR_EACH {
                low.push(lang.clone());
            }
            (all_n, all_first, all_among) = (all_n + n, all_first + first, all_among + among);
        }
        let all = all_first as f64 / all_n as f64;
        lines.push(format!("DEV all: n {all_n}, first {:.1} %, among the two {:.1} %", 100.0 * all, 100.0 * all_among as f64 / all_n as f64));
        // what filing one page by its words costs (title and first line; the table and neighbours made once before)
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let pages: Vec<Page> = set["cases"]["fr"].as_array().unwrap().iter()
            .map(|c| page_in("fr", "subject", c["title"].as_str().unwrap(), c["first_line"].as_str().unwrap_or(""), "")).collect();
        let _ = decided(&config, &pages[0]);
        let ctx = Ctx::new(&config);
        let started = std::time::Instant::now();
        for _ in 0..20 {
            for p in &pages {
                std::hint::black_box(decide_in(&ctx, Part::Main, "Sujets/x.md", p, None));
            }
        }
        lines.push(format!("DEV cost: {:.1} µs a page (title and first line)", started.elapsed().as_secs_f64() * 1e6 / (20 * pages.len()) as f64));
        eprintln!("{}", lines.join("\n"));
        assert!(all >= DEV_FLOOR && low.is_empty(), "{}", lines.join("\n"));
    }

    /// The development set's floors (first category right): all languages together, and each language (22/11: S1's
    /// table 66.9 %, 61–77 %; carnet-categories' 95.3 %, 92.5–97.1 %; after its review 1: 94.7 %, 92.5–97.1 %; floors at
    /// that measure, review 2 R11).
    const DEV_FLOOR: f64 = 0.94;
    const DEV_FLOOR_EACH: f64 = 0.92;

    /// Words a person writes that the table did not know (« Autres » before, carnet-categories): everyday words of
    /// each category in the five languages, their forms (plurals, verbs), German and Dutch compounds, phrases.
    #[test]
    fn everyday_words_and_their_forms() {
        for (lang, title, want) in [
            ("fr", "Tondeuse en panne", "home"),
            ("fr", "Rembourser Sophie", "money"),
            ("fr", "Procuration de papa", "papers"),
            ("fr", "Note de frais du salon", "work"),
            ("fr", "Billets de train", "trips"),
            ("fr", "Obsèques de tante Rose", "people"),
            ("en", "Hedge and lawn", "home"),
            ("en", "Pocket money", "money"),
            ("en", "Parking permit form", "papers"),
            ("en", "Night shifts in March", "work"),
            ("en", "Lift passes", "trips"),
            ("en", "Nephew's christening", "people"),
            ("de", "Dachboden entrümpeln", "home"),
            ("de", "Taschengeld", "money"),
            ("de", "Geburtsurkunde beantragen", "papers"),
            ("de", "Praktikum im Sommer", "work"),
            ("de", "Mietwagen in Portugal", "trips"),
            ("de", "Beerdigung", "people"),
            ("es", "Pintar la habitación", "home"),
            ("es", "Pagar la deuda", "money"),
            ("es", "Empadronamiento", "papers"),
            ("es", "Turnos de noche", "work"),
            ("es", "Billetes de tren", "trips"),
            ("es", "Bautizo del sobrino", "people"),
            ("nl", "Zolder en kelder", "home"),
            ("nl", "Zakgeld", "money"),
            ("nl", "Uittreksel aanvragen", "papers"),
            ("nl", "Vacature teamleider", "work"),
            ("nl", "Huurauto", "trips"),
            ("nl", "Begrafenis", "people"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            assert_eq!(decided(&config, &page_in(lang, "subject", title, "", ""))[0], want, "{lang} {title}");
        }
    }

    /// « Maison » and « Argent » pull no more than their share (the blind set's misses, 22/11: 18 « Maison » and 12
    /// « Argent » too many): a holiday rental is a trip, a ski pass too; an amount alone files nothing; a hobby stays
    /// in « Autres » whatever its first line says; the title's head (« Dépenses de Noël », « Kredit für die Küche »)
    /// decides between its words.
    #[test]
    fn maison_and_argent_pull_no_more_than_their_share() {
        for (lang, title, line, want) in [
            ("fr", "Gîte à Biarritz", "Maison près de la plage, du 3 au 17 août.", &["trips"][..]),
            ("fr", "Dépenses de Noël", "Cadeaux et repas : 300 € au plus.", &["money", "people"]),
            ("fr", "Cours de piano", "Le mardi, 25 € la séance.", &["other"]),
            ("fr", "Assurance de la maison", "", &["papers", "home"]),
            ("en", "Holiday cottage in Wales", "Three nights in May.", &["trips"]),
            ("en", "Choir", "Rehearsal at the house of Ann, bring £5.", &["other"]),
            ("de", "Ferienwohnung am See", "Zwei Wochen im Juli.", &["trips"]),
            ("de", "Kredit für die Küche", "Raten bis August.", &["money", "home"]),
            ("de", "Jubiläum von Opa", "Feier im Gasthaus.", &["people"]),
            ("es", "Casa rural en Cantabria", "Cuatro noches en abril.", &["trips"]),
            ("es", "Forfait de esquí", "Comprarlo antes de enero.", &["trips"]),
            ("es", "Seguro de hogar", "", &["papers", "home"]),
            ("nl", "Vakantiehuis in Frankrijk", "Twee weken in juli.", &["trips"]),
            ("nl", "Gitaarles", "Elke week 20 euro.", &["other"]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            assert_eq!(decided(&config, &page_in(lang, "subject", title, line, "")), want, "{lang} {title}");
        }
        // German « nicht » is no niece: a first line saying « not » files nothing
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "de");
        assert_eq!(decided(&config, &page_in("de", "subject", "Ideen", "Noch nicht entschieden.", "")), ["other"]);
    }

    /// Review 1 of carnet-categories (R1, R2, R3, R7, R8): the weekend says little; a hobby's word never beats a real
    /// category another word of the title gives; the false friends and too wide words the review found file nothing
    /// wrong. Written from the review and the development set, never from a blind set.
    #[test]
    fn the_reviews_titles_go_where_a_person_looks() {
        let mut wrong = Vec::new();
        for (lang, title, line, want) in [
            // R1: the weekend
            ("fr", "Ménage ce week-end", "", "home"),
            ("fr", "Chez mamie ce week-end", "", "people"),
            ("en", "Garden this weekend", "", "home"),
            ("en", "Kids this weekend", "", "people"),
            ("nl", "Tuin dit weekend", "", "home"),
            ("fr", "Week-end à Lisbonne", "", "trips"),
            // R2: a hobby's word at the head
            ("fr", "Photos du mariage de Julie", "", "people"),
            ("fr", "Sortie au zoo avec les enfants", "", "people"),
            ("es", "Libro de familia", "", "papers"),
            ("de", "Schachteln für den Umzug", "", "home"),
            ("fr", "Cours de piano", "Le mardi, 25 € la séance.", "other"),
            // R8: Spanish feminines
            ("es", "La inquilina del segundo", "", "home"),
            ("es", "Jubilada desde enero", "", "money"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            let got = decided(&config, &page_in(lang, "subject", title, line, ""));
            if got[0] != want {
                wrong.push(format!("{lang} {title} → {got:?} (want {want})"));
            }
        }
        // R3, R7: never in the category the false friend points to
        for (lang, title, line, never) in [
            ("fr", "Stage de poney", "", "work"),
            ("fr", "Emmener la voiture au garage", "", "home"),
            ("fr", "Compte rendu IRM", "", "money"),
            ("fr", "Compte rendu du conseil de classe", "", "work"),
            ("fr", "Salon de l'auto", "", "home"),
            ("fr", "Prélèvement sanguin", "", "money"),
            ("fr", "Disneyland", "", "money"),
            ("fr", "Colis livré chez la voisine", "", "other"),
            ("es", "Gran Canaria en abril", "", "people"),
            ("es", "Tener en cuenta", "", "money"),
            ("es", "Planta baja", "", "papers"),
            ("es", "Turno del pediatra", "", "work"),
            ("es", "Chequeo médico", "", "money"),
            ("nl", "Overleg met de buren", "", "work"),
            ("nl", "Zorgdeclaratie", "", "work"),
            ("nl", "Enkel verstuikt", "", "people"),
            ("de", "Betriebskosten", "", "work"),
            ("de", "Heiratsantrag", "", "papers"),
            ("de", "Akten vernichten", "", "people"),
            ("de", "Hochzeitsfotograf", "", "other"),
            ("de", "Familie Zimmermann", "", "home"),
            ("de", "Wohnung in Berlin", "", "trips"),
            ("en", "Pea and mint soup", "", "money"),
            ("en", "Chore rota", "", "work"),
            ("en", "Meter reading for the energy supplier", "", "other"),
            ("en", "Concrete for the patio", "", "trips"),
            ("en", "Permission slip", "", "work"),
            ("en", "Interpreter", "", "money"),
            ("en", "Cookies for the bake sale", "", "home"),
            ("fr", "Idées", "Un bon resto à Lyon.", "money"),
            ("es", "Plantilla del informe", "", "home"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let config = config_in(&dir, lang);
            let (got, word) = decide(&config, Part::Main, "Sujets/x.md", &page_in(lang, "subject", title, line, ""), None);
            if got.iter().any(|c| c == never) {
                wrong.push(format!("{lang} {title} → {got:?} by {word:?} (never {never})"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// Review 1 (R4): no word of the neighbours' health groups (search_words.rs, the five languages: the groups before
    /// each language's birthday group) is filed by a neighbour.
    #[test]
    fn no_health_word_is_filed_by_a_neighbour() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/notebook/search_words.rs");
        let source = std::fs::read_to_string(file).unwrap();
        for lang in ["FR", "EN", "DE", "ES", "NL"] {
            let block = &source[source.find(&format!("const NEIGHBOURS_{lang}: &[&str] = &[")).unwrap()..];
            let block = &block[..block.find("\n];").unwrap()];
            let mut groups = 0;
            for line in block.lines().skip(1) {
                let Some(group) = line.trim().strip_prefix('"').and_then(|l| l.strip_suffix("\",")) else { continue };
                if ["anniv", "birthday", "geburtstag", "cumple", "verjaardag"].iter().any(|b| group.contains(b)) {
                    break;
                }
                groups += 1;
                for word in group.split('|') {
                    assert!(!NEIGHBOURS.contains_key(&fold(word)), "{lang}: {word} filed by a neighbour");
                }
            }
            assert!(groups >= 5, "{lang}: {groups} health groups read");
        }
    }

    /// The person's rule still wins over the table's new words (a word they filed elsewhere), and a hobby's word is
    /// learnt like any other.
    #[test]
    fn a_rule_wins_over_the_new_words() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let p = page("subject", "Cours de piano", "Le mardi.", "");
        assert_eq!(decide(&config, Part::Main, "Sujets/Cours de piano.md", &p, None), (vec!["other".to_string()], Some("piano".to_string())));
        update_state(&config, |s| {
            s.insert("category_rules".into(), json!([{"word": "piano", "categories": ["people"], "at": "2026-11-22"}]));
        })
        .unwrap();
        assert_eq!(decided(&config, &p), ["people"]);
        let q = page("subject", "Billets de train", "", "");
        update_state(&config, |s| {
            s.insert("category_rules".into(), json!([{"word": "train", "categories": ["work"], "at": "2026-11-22"}]));
        })
        .unwrap();
        assert_eq!(decided(&config, &q), ["work"]);
    }

    #[test]
    fn other_words_and_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let one = |title: &str, summary: &str| decided(&config, &page("subject", title, summary, ""));
        assert_eq!(one("Assurance auto", ""), ["papers"]);
        assert_eq!(one("Factures EDF", ""), ["papers", "money"]);
        // a letter is a paper, the syndic what it is about (carnet-categories: the title's head first)
        assert_eq!(one("Lettre au syndic", ""), ["papers", "home"]);
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
    }

    /// Review 1 (B1): a work agent's page is Travail when it makes it; its first write on a page that has no
    /// categories yet (written before the sommaire) files it by its words, not under Travail for good.
    #[test]
    fn a_work_agents_page_is_travail_only_when_it_makes_it() {
        use crate::notebook::{note, part_root, read_page_text, write_file};
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        crate::agent_groups::set_group(&config, "kerfy", "Pro").unwrap();
        let pro = Who::agent(&config, "kerfy");
        let lancement = Page::new("subject", "Lancement");
        assert_eq!(decide(&config, Part::Main, "Sujets/Lancement.md", &lancement, Some("kerfy")).0, ["work"]);
        assert_eq!(decide(&config, Part::Main, "Sujets/Lancement.md", &lancement, None).0, ["other"]);
        let person = Page::new("person", "Claire Durand");
        assert_eq!(decide(&config, Part::Main, "Personnes/Claire Durand.md", &person, Some("kerfy")).0, ["people", "work"]);
        // a page written before the sommaire (no categories), then by the work agent
        write_file(&part_root(&config, Part::Main).join("Domaines/Maison.md"), "---\ntype: area\n---\n# Maison\n\n## Habitudes et règles\n- Poubelles le lundi soir\n").unwrap();
        write_file(&part_root(&config, Part::Main).join("Personnes/Claire Durand.md"), "---\ntype: person\n---\n# Claire Durand\n\n## Qui c'est\n- Ta sœur\n").unwrap();
        let fact = |f: &str| vec![f.to_string()];
        note(&config, Part::Main, "area", "Maison", "habits", &fact("Chaudière : entretien en octobre"), &[], "", "", &pro).unwrap();
        note(&config, Part::Main, "person", "Claire Durand", "work", &fact("Infirmière"), &[], "", "", &pro).unwrap();
        note(&config, Part::Main, "subject", "Lancement", "status", &fact("Maquette prête"), &[], "", "", &pro).unwrap();
        let read = |p: &str| Page::parse(&read_page_text(&config, Part::Main, p).unwrap(), "");
        assert_eq!(stored(&read("Domaines/Maison.md")).unwrap().0, ["home"]);
        assert_eq!(stored(&read("Personnes/Claire Durand.md")).unwrap().0, ["people"]);
        assert_eq!(stored(&read("Sujets/Lancement.md")).unwrap().0, ["work"]);
    }

    /// Review 1 (B4): `categories` written by hand that the bridge cannot read are never touched.
    #[test]
    fn hand_written_categories_are_left_as_they_are() {
        use crate::notebook::{note, part_root, read_page_text, write_file};
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let (agent, you) = (Who::agent(&config, "home"), Who::you(&config));
        for (name, value) in [("Lectures", "categories:\n  - \"[[Livres]]\"\n"), ("Jardin", "categories:\n  - home\n  - money\n  - work\n")] {
            let path = format!("Sujets/{name}.md");
            write_file(&part_root(&config, Part::Main).join(&path), &format!("---\ntype: subject\n{value}---\n# {name}\n")).unwrap();
            note(&config, Part::Main, "subject", name, "status", &["En cours".to_string()], &[], "", "", &agent).unwrap();
            crate::notebook::note_with(&config, Part::Main, "subject", name, "status", &["Encore".to_string()], &[], "", "",
                                       &["money".to_string()], &agent).unwrap();
            let text = read_page_text(&config, Part::Main, &path).unwrap();
            assert!(text.contains(value) && !text.contains("categories_by"), "{text}");
            let page = Page::parse(&text, "");
            assert!(hand_written(&page));
            assert_eq!(of_page(&config, Part::Main, &path, &page).by, "auto");
            let refused = categorize(&config, &Target::Page { path: path.clone() }, &["money".to_string()], &you).unwrap();
            assert_eq!(refused, Err(Refused::HandWritten));
        }
    }

    #[test]
    fn dates_are_computed_from_the_texts() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let on = |text: &str| dated(text, None, today);
        // yearly when the text says so: at its next occurrence, always in Dates
        assert_eq!(when_of("Anniversaire : 14 mars 1990", None, today), Some(When::Yearly(3, 14)));
        assert_eq!(date_of("Anniversaire : 14 mars 1990", today), Some(day(2027, 3, 14)));
        assert_eq!(when_of("Cumpleaños: 23 de diciembre", None, today), Some(When::Yearly(12, 23)));
        assert_eq!(when_of("Verjaardag : 14 maart", None, today), Some(When::Yearly(3, 14)));
        assert_eq!(When::fields(Some(When::Yearly(3, 14))), (Value::Null, json!("03-14")));
        // a day without a year that says nothing yearly: the one nearest to when it was noted, not every year
        assert_eq!(when_of("Devis reçu le 3 octobre", Some(day(2026, 10, 5)), today), Some(When::On(day(2026, 10, 3))));
        assert!(on("Devis reçu le 3 octobre"));
        assert!(!dated("Devis reçu le 3 octobre", Some(day(2025, 10, 5)), today));
        assert_eq!(when_of("Entretien annuel le 27 novembre", None, today), Some(When::On(day(2026, 11, 27))));
        assert_eq!(when_of("Jaargesprek op 3 januari", None, today), Some(When::On(day(2027, 1, 3))));
        // days with their year
        assert!(on("Vol le 2027-03-01") && on("Garantie jusqu’au 2028-10-01") && on("Rendu le 2026-09-01"));
        assert!(!on("Acheté le 2024-01-10") && !on("Expire le 2030-01-01"));
        // not dates
        assert!(!on("Poubelles le lundi soir") && !on("Avril, 12 jours : Tokyo, Kyoto"));
        assert_eq!(when_of("1/2 tasse de sucre", None, today), None);
        assert_eq!(when_of("Rendez-vous le 14/11", Some(day(2026, 11, 2)), today), Some(When::On(day(2026, 11, 14))));
        // a line: its own date, read near its attribution's day (never the attribution's)
        assert_eq!(line_when("- Devis le 3 octobre — *Léa, 2026-10-05*", "", today), Some(When::On(day(2026, 10, 3))));
        assert_eq!(line_when("Rien de daté — *Léa, 2026-10-05*", "", today), None);
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
        assert!(on_write(&config, Part::Main, path, &mut p, &agent, true));
        assert_eq!(stored(&p), Some((vec!["home".to_string()], "auto".to_string())));
        // filed already: kept, whatever its words become
        p.body = "## En bref\nLe budget : 9 000 €, le prêt de la banque.\n".into();
        assert!(!on_write(&config, Part::Main, path, &mut p, &agent, true));
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
        assert!(!on_write(&config, Part::Main, path, &mut p, &agent, true));
        // in the frontmatter, before `updated`: readable in Obsidian, read back the same
        p.touch();
        let text = p.render();
        assert!(text.contains("categories:\n  - home\ncategories_by: person\nupdated: "), "{text}");
        assert_eq!(stored(&Page::parse(&text, "")), Some((vec!["home".to_string()], "person".to_string())));
        // filed in « Autres » by the bridge: filed again at the next write
        let mut q = page("subject", "Divers", "", "");
        assert!(on_write(&config, Part::Main, "Sujets/Divers.md", &mut q, &agent, true));
        assert_eq!(stored(&q).unwrap().0, ["other"]);
        q.body = "## En bref\nLe devis du plombier pour la salle de bain.\n".into();
        assert!(on_write(&config, Part::Main, "Sujets/Divers.md", &mut q, &agent, true));
        assert_eq!(stored(&q).unwrap().0, ["home"]);
        // a hand-written value that is not a category: read as the bridge's
        let mut r = page("subject", "Voyage Japon", "", "");
        r.front.insert("categories".into(), json!("Money"));
        assert_eq!(stored(&r), Some((vec!["money".to_string()], "auto".to_string())));
        r.front.insert("categories".into(), json!(["maison"]));
        assert_eq!(stored(&r), None);
        assert_eq!(of_page(&config, Part::Main, "Sujets/Voyage Japon.md", &r).categories, ["trips"]);
        // the private part is Santé: nothing written there
        let mut h = page("area", "Santé", "", "");
        assert!(!on_write(&config, Part::Health, "Domaines/Santé.md", &mut h, &agent, true));
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

    /// Review 1 (R2, R3, B2): a rule is learnt only from the bridge's own filing; one undo a target, the newest; the
    /// state keeps keys, never a line's text, a page's path or a file's name.
    #[test]
    fn corrections_learn_from_the_bridge_only_keep_one_undo_and_no_text() {
        use crate::notebook::{note, note_with, read_state};
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir, "fr");
        let (agent, you) = (Who::agent(&config, "home"), Who::you(&config));
        let give = |c: &str| vec![c.to_string()];
        // filed by an agent, then corrected by the person: nothing learnt
        note_with(&config, Part::Main, "subject", "Rénovation cuisine", "status", &give("Devis reçu"), &[], "", "",
                  &give("home"), &agent).unwrap();
        let page = Target::Page { path: "Sujets/Rénovation cuisine.md".into() };
        let first = categorize(&config, &page, &give("money"), &you).unwrap().unwrap();
        assert_eq!((first["before"]["categories_by"].clone(), first["learnt"].clone()), (json!("agent"), Value::Null));
        assert!(rules(&config).is_empty());
        // corrected again: the first correction can no longer be undone, the second puts back the first's
        let second = categorize(&config, &page, &give("work"), &you).unwrap().unwrap();
        assert!(undo(&config, first["undo"].as_str().unwrap(), &you).unwrap().is_none());
        let back = undo(&config, second["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!((back["categories"].clone(), back["categories_by"].clone()), (json!(["money"]), json!("person")));
        // a line, by the Item id the phone has; the state holds no text
        note(&config, Part::Main, "subject", "Voyage Japon", "next", &give("Réserver les vols pour Osaka"), &[], "", "", &agent).unwrap();
        let (path, japan) = crate::notebook::all_pages(&config, Part::Main).into_iter().find(|(_, p)| p.title == "Voyage Japon").unwrap();
        let key = page_key(&japan, &path);
        let ids = line_ids(&key, &["Réserver les vols pour Osaka".to_string()]);
        let done = categorize(&config, &Target::Item(ids[0].clone()), &give("money"), &you).unwrap().unwrap();
        assert_eq!(done["target"]["item"], json!(ids[0]));
        assert_eq!(categorize(&config, &Target::Item(format!("{}0", ids[0])), &give("money"), &you).unwrap(), Err(Refused::NotFound));
        let page_item = categorize(&config, &Target::Item(format!("page:{key}")), &give("work"), &you).unwrap().unwrap();
        assert_eq!(page_item["target"]["path"], json!(path));
        let state = serde_json::to_string(&read_state(&config)).unwrap();
        assert!(!state.contains("Osaka") && !state.contains("Voyage") && !state.contains("cuisine"), "{state}");
        // the same line said twice on a page: two ids
        let twice = line_ids("p1", &["Lait".to_string(), "Pain".to_string(), "lait".to_string()]);
        assert_eq!((twice[2].strip_suffix(":2"), twice[1] != twice[0]), (Some(twice[0].as_str()), true));
        // a page without an id (made in Obsidian): a key from its path
        assert!(page_key(&Page::parse("# Notes\n", "Notes"), "Notes.md").starts_with('f'));
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
        // a purchase is dated by its warranty's end only, never by its day (Review 1, R5)
        assert!(!of_record(purchase, &without, &none, today).2);
        // the phone's line of a record: no number, no reference, no source (Review 1, B5)
        let contract = rec::kind("contract").unwrap();
        let (insured, _) = rec::upsert(&config, contract, "", &fields(&[("what", "Assurance habitation".into()),
            ("provider", "Assureur Horizon".into()), ("number", "HAB-448120".into()), ("end", day(95)),
            ("source", "Avis d’échéance CSG-2291".into())]), false, &agent).unwrap();
        let line = record_line(contract, &insured);
        assert!(line.starts_with("Assurance habitation · Assureur Horizon") && !line.contains("448120") && !line.contains("2291"), "{line}");
        let booking_ref = rec::kind("booking").unwrap();
        let (with_ref, _) = rec::upsert(&config, booking_ref, "", &fields(&[("date", day(30)), ("what", "Hôtel Rossio".into()),
            ("reference", "HR-55821".into())]), false, &agent).unwrap();
        assert!(!record_line(booking_ref, &with_ref).contains("55821"));
        for reference in ["réf. HR-55821", "Avis CSG-2291", "Contrat HAB-448120", "Code ABC123"] {
            assert!(holds_reference(reference), "{reference}");
        }
        for plain in ["8 900 €", "15.000 €", "Vol le 2026-12-04", "Rendez-vous à 12h30", "Entre 2026 et 2027", "Format A4"] {
            assert!(!holds_reference(plain), "{plain}");
        }
        let booking = rec::kind("booking").unwrap();
        let (vol, _) = rec::upsert(&config, booking, "", &fields(&[("date", format!("{} 10:30", day(12))), ("what", "Vol Lisbonne".into())]),
                                   false, &agent).unwrap();
        assert_eq!(of_record(booking, &vol, &none, today), (vec!["trips".into()], "auto".into(), true));
        // tasks: their page's, else « Autres »; Dates when due
        note(&config, Part::Main, "subject", "Voyage Japon", "next", &["Réserver les vols".to_string()], &[], "", "", &agent).unwrap();
        let linked = add_task(&config, Part::Main, "Réserver les vols", &day(4), "Voyage Japon", "asked", "", &agent).unwrap().task;
        let alone = add_task(&config, Part::Main, "Trier les photos", "", "", "asked", "", &agent).unwrap().task;
        let pages = all_pages(&config, Part::Main);
        assert_eq!(of_task(&Ctx::new(&config), &linked, &pages, &none, today), ["trips", "dates"]);
        assert_eq!(of_task(&Ctx::new(&config), &alone, &pages, &none, today), ["other"]);
        // documents: their page's, else their name's words, else Papiers
        let doc = |name: &str, subject: Option<&str>| json!({"agent": "home", "file": name, "name": name,
            "subjects": subject.map(|p| vec![json!({"path": p})]).unwrap_or_default()});
        assert_eq!(of_document(&Ctx::new(&config), &doc("Programme.pptx", Some("Sujets/Voyage Japon.md")), &pages, &none).0, ["trips"]);
        assert_eq!(of_document(&Ctx::new(&config), &doc("Facture EDF.pdf", None), &pages, &none),
                   (vec!["papers".into(), "money".into()], "auto".into(), Some("facture".into())));
        assert_eq!(of_document(&Ctx::new(&config), &doc("IMG_2041.jpg", None), &pages, &none).0, ["papers"]);
        // lines: their page's, Dates when they carry a date
        assert_eq!(of_line(&["people".to_string()], "p1", "Anniversaire : 14 mars — *Léa, 2026-10-01*", "", &none, today), ["people", "dates"]);
        // the person files them elsewhere: kept in the state, undone as well
        let give = |c: &str| vec![c.to_string()];
        let id = tk::str_or_empty(with.get("id"));
        let done = categorize(&config, &Target::Record { kind: "purchase".into(), id: id.clone() }, &give("home"), &you).unwrap().unwrap();
        assert_eq!(done["learnt"], Value::Null);
        assert_eq!(of_record(purchase, &with, &marks(&config), today).0, ["home"]);
        let t = categorize(&config, &Target::Task(alone.id.clone()), &give("home"), &you).unwrap().unwrap();
        assert_eq!(of_task(&Ctx::new(&config), &alone, &pages, &marks(&config), today), ["home"]);
        let japan = all_pages(&config, Part::Main).into_iter().find(|(_, p)| p.title == "Voyage Japon").unwrap();
        let line = Target::Line { path: japan.0.clone(), text: "Réserver les vols".into() };
        categorize(&config, &line, &give("money"), &you).unwrap().unwrap();
        assert_eq!(of_line(&["trips".to_string()], &japan.1.id(), "- Réserver les vols — *Home, 2026-11-22*", "", &marks(&config), today), ["money"]);
        let gone = Target::Line { path: japan.0.clone(), text: "Rien de tel".into() };
        assert_eq!(categorize(&config, &gone, &give("money"), &you).unwrap(), Err(Refused::NotFound));
        assert_eq!(categorize(&config, &Target::Record { kind: "purchase".into(), id: "r000000".into() }, &give("home"), &you).unwrap(),
                   Err(Refused::NotFound));
        undo(&config, t["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!(of_task(&Ctx::new(&config), &alone, &pages, &marks(&config), today), ["other"]);
        undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
        assert_eq!(of_record(purchase, &with, &marks(&config), today).0, ["money", "papers"]);
        // the pages of the records keep their type's categories
        assert_eq!(of_page(&config, Part::Main, "Fiches/Achats.md", &all_pages(&config, Part::Main).into_iter()
            .find(|(p, _)| p == "Fiches/Achats.md").unwrap().1).categories, ["money", "papers"]);
    }
}
