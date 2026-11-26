//! The Carnet's structure as the category screens show it (Alex, 26/11: « Personnes > liste de personnes > Sujets…
//! Maison > liste des sujets > liste des notes »): a category lists its **pages** — the people in Personnes, the
//! subjects elsewhere —, each with its number of notes; a tap opens the page (its sections, their notes). Never a
//! line, a task or a file on its own (a file named « photo.jpg » is never a row): a page is in a category by its own
//! categories (its header's `categories`, the bridge's filing), never by one of its lines; in Dates by a date still to
//! come (`next`, shown on its row), never by a line that only reads like one. The records' pages (Achats,
//! Abonnements…) are listed by their records' categories, each counting the records it has there.
//!
//! - **Personnes**: « Toi » first, then the people by their link (their page's « Qui c'est », else its « En bref »:
//!   « Ton beau-frère » → `link` « Beau-frère », group `family`), Famille, Amis, Travail, Autres; then the other
//!   pages filed there (Sujets). The group pages (« Amis », « Famille élargie »…) under their group.
//! - **The other categories**: their pages by name (archived last); Sujets then Personnes when both are there.
//! - **Dates**: the pages with a date to come (a yearly one's next, a day within `AHEAD`), the soonest first.
//! - Headers (`groups`) only when the list has two groups or more.
//!
//! Counted on the phone's copy (index.rs), as the sommaire: the phones make the same list from their copy without a
//! network (HermesKit's and Android's `CarnetSommaire`, checked on contracts/carnet/fake-bridge/<lang>/category-*.json).

use std::collections::HashMap;

use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};

use super::finder::Finder;
use super::Page;
use crate::tools_knowledge as tk;

/// How far ahead a date puts its page in Dates (days; the copy's `dated.after`).
pub const AHEAD: i64 = 730;

/// The groups' order (Personnes' people, then what else is filed there; `people` in another category).
pub const GROUPS: [&str; 6] = ["family", "friends", "work", "others", "subjects", "people"];

/// The groups' names (visible text, proposed for Alex) in fr, en, de, es, nl.
const GROUP_TITLES: [(&str, [&str; 5]); 6] = [
    ("family", ["Famille", "Family", "Familie", "Familia", "Familie"]),
    ("friends", ["Amis", "Friends", "Freunde", "Amigos", "Vrienden"]),
    ("work", ["Travail", "Work", "Arbeit", "Trabajo", "Werk"]),
    ("others", ["Autres", "Others", "Andere", "Otros", "Anderen"]),
    ("subjects", ["Sujets", "Subjects", "Themen", "Temas", "Onderwerpen"]),
    ("people", ["Personnes", "People", "Personen", "Personas", "Personen"]),
];

/// A group's name in `lang` (English for another language).
pub fn group_title(lang: &str, id: &str) -> String {
    let column = match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        "nl" => 4,
        _ => 1,
    };
    GROUP_TITLES.iter().find(|(g, _)| *g == id).map(|(_, t)| t[column].to_string()).unwrap_or_else(|| id.to_string())
}

// -- a person's link --------------------------------------------------------------------------------------------------

/// The words of a link (folded, hyphens kept) and their group, in fr, en, de, es, nl.
const LINKS: &[(&str, &str)] = &[
    // family
    ("mere", "family"), ("maman", "family"), ("pere", "family"), ("papa", "family"), ("frere", "family"),
    ("soeur", "family"), ("fils", "family"), ("fille", "family"), ("mari", "family"), ("femme", "family"),
    ("epouse", "family"), ("epoux", "family"), ("conjoint", "family"), ("conjointe", "family"), ("compagnon", "family"),
    ("compagne", "family"), ("grand-mere", "family"), ("grand-pere", "family"), ("mamie", "family"), ("papi", "family"),
    ("petit-fils", "family"), ("petite-fille", "family"), ("oncle", "family"), ("tante", "family"), ("cousin", "family"),
    ("cousine", "family"), ("neveu", "family"), ("niece", "family"), ("beau-frere", "family"), ("belle-soeur", "family"),
    ("beau-pere", "family"), ("belle-mere", "family"), ("gendre", "family"), ("belle-fille", "family"),
    ("parrain", "family"), ("marraine", "family"), ("filleul", "family"), ("filleule", "family"),
    ("mother", "family"), ("mum", "family"), ("mom", "family"), ("father", "family"), ("dad", "family"),
    ("brother", "family"), ("sister", "family"), ("son", "family"), ("daughter", "family"), ("husband", "family"),
    ("wife", "family"), ("grandmother", "family"), ("grandfather", "family"), ("grandma", "family"), ("grandpa", "family"),
    ("grandson", "family"), ("granddaughter", "family"), ("uncle", "family"), ("aunt", "family"), ("nephew", "family"),
    ("brother-in-law", "family"), ("sister-in-law", "family"), ("father-in-law", "family"), ("mother-in-law", "family"),
    ("son-in-law", "family"), ("daughter-in-law", "family"), ("godfather", "family"), ("godmother", "family"),
    ("godson", "family"), ("goddaughter", "family"), ("stepmother", "family"), ("stepfather", "family"),
    ("mutter", "family"), ("mama", "family"), ("vater", "family"), ("bruder", "family"), ("schwester", "family"),
    ("sohn", "family"), ("tochter", "family"), ("ehemann", "family"), ("ehefrau", "family"), ("oma", "family"),
    ("opa", "family"), ("grossmutter", "family"), ("grossvater", "family"), ("enkel", "family"), ("enkelin", "family"),
    ("onkel", "family"), ("neffe", "family"), ("nichte", "family"), ("schwager", "family"), ("schwagerin", "family"),
    ("schwiegermutter", "family"), ("schwiegervater", "family"), ("pate", "family"), ("patin", "family"),
    ("madre", "family"), ("padre", "family"), ("hermano", "family"), ("hermana", "family"), ("hijo", "family"),
    ("hija", "family"), ("marido", "family"), ("esposo", "family"), ("esposa", "family"), ("abuela", "family"),
    ("abuelo", "family"), ("nieto", "family"), ("nieta", "family"), ("tio", "family"), ("tia", "family"),
    ("primo", "family"), ("prima", "family"), ("sobrino", "family"), ("sobrina", "family"), ("cunado", "family"),
    ("cunada", "family"), ("suegro", "family"), ("suegra", "family"), ("yerno", "family"), ("nuera", "family"),
    ("padrino", "family"), ("madrina", "family"), ("ahijado", "family"), ("ahijada", "family"),
    ("moeder", "family"), ("vader", "family"), ("broer", "family"), ("zus", "family"), ("zuster", "family"),
    ("zoon", "family"), ("dochter", "family"), ("echtgenoot", "family"), ("echtgenote", "family"), ("kleinzoon", "family"),
    ("kleindochter", "family"), ("oom", "family"), ("neef", "family"), ("nicht", "family"), ("zwager", "family"),
    ("schoonzus", "family"), ("schoonmoeder", "family"), ("schoonvader", "family"),
    // friends
    ("ami", "friends"), ("amie", "friends"), ("copain", "friends"), ("copine", "friends"), ("pote", "friends"),
    ("friend", "friends"), ("freund", "friends"), ("freundin", "friends"), ("kumpel", "friends"), ("amigo", "friends"),
    ("amiga", "friends"), ("vriend", "friends"), ("vriendin", "friends"),
    // work
    ("collegue", "work"), ("chef", "work"), ("cheffe", "work"), ("patron", "work"), ("patronne", "work"),
    ("manager", "work"), ("client", "work"), ("cliente", "work"), ("associe", "work"), ("associee", "work"),
    ("colleague", "work"), ("boss", "work"), ("coworker", "work"), ("co-worker", "work"), ("kollege", "work"),
    ("kollegin", "work"), ("chefin", "work"), ("kunde", "work"), ("kundin", "work"), ("colega", "work"), ("companero", "work"), ("companera", "work"),
    ("jefe", "work"), ("jefa", "work"), ("socio", "work"), ("socia", "work"), ("collega", "work"), ("baas", "work"),
    ("klant", "work"),
    // others
    ("voisin", "others"), ("voisine", "others"), ("neighbour", "others"), ("neighbor", "others"),
    ("nachbar", "others"), ("nachbarin", "others"), ("vecino", "others"), ("vecina", "others"), ("buurman", "others"),
    ("buurvrouw", "others"), ("buur", "others"), ("connaissance", "others"), ("acquaintance", "others"),
    ("bekannte", "others"), ("bekannter", "others"), ("conocido", "others"), ("conocida", "others"), ("kennis", "others"),
];

/// The words that end a German or Dutch compound and still say the link (« Lauffreund », « Arbeitskollegin »).
const COMPOUND_ENDS: &[&str] =
    &["freund", "freundin", "kollege", "kollegin", "nachbar", "nachbarin", "vriend", "vriendin", "collega"];

/// The person's own « your » (a link is said after it: « Ton beau-frère », « Deine Nachbarin »).
const YOURS: &[&str] =
    &["ton", "ta", "tes", "your", "dein", "deine", "deinen", "deinem", "deiner", "tu", "tus", "je", "jouw", "uw"];

/// A word as the link table reads it: lower case, without accents (« sœur » → « soeur »), hyphens kept.
fn plain(word: &str) -> String {
    tk::ascii_fold(&word.to_lowercase().replace('œ', "oe").replace('æ', "ae").replace('ß', "ss"))
}

/// A word of a text without what sticks to it (punctuation, links' brackets, « l’ »).
fn bare(token: &str) -> &str {
    let t = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
    t.rsplit(['’', '\'']).next().unwrap_or(t).trim_matches(|c: char| !c.is_alphanumeric())
}

fn link_group(word: &str) -> Option<&'static str> {
    let w = plain(word);
    if let Some((_, g)) = LINKS.iter().find(|(l, _)| *l == w) {
        return Some(g);
    }
    COMPOUND_ENDS
        .iter()
        .find(|end| w.len() > end.len() + 2 && w.ends_with(*end))
        .and_then(|end| LINKS.iter().find(|(l, _)| l == end).map(|(_, g)| *g))
}

/// The link a text says at its head (« Ton beau-frère, marié à … » → « Beau-frère », « Voisine du 3e » →
/// « Voisine »): a link word within three words after the person's « your », or the text's first word. Further in, a
/// link is someone else's (« Le mari de Claire, ta sœur »).
fn said(text: &str) -> Option<(String, &'static str)> {
    let words: Vec<&str> = text.split_whitespace().map(bare).filter(|w| !w.is_empty()).collect();
    let shown = |w: &str| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    let first = words.first()?;
    if YOURS.contains(&plain(first).as_str()) {
        return words.iter().skip(1).take(3).find_map(|x| link_group(x).map(|g| (shown(x), g)));
    }
    link_group(first).map(|g| (shown(first), g))
}

/// A group page's or a family area's group (« Amis » → `friends`, « Famille » → `family`), else none.
fn page_group(title: &str) -> Option<&'static str> {
    match super::groups::group_relation(title) {
        Some("family") => Some("family"),
        Some("friend") => Some("friends"),
        Some("work") => Some("work"),
        Some(_) => Some("others"),
        None => ["famille", "family", "familie", "familia", "gezin"].contains(&super::fold(title).as_str()).then_some("family"),
    }
}

/// What the category screens list a page by: a person's link and group (from « Qui c'est », else « En bref »); a
/// group page's or a family area's group; none for the others.
pub fn link_of(page: &Page, kind: &str) -> (Option<String>, Option<&'static str>) {
    match kind {
        "person" => {
            let sections = page.sections();
            let who: Vec<&str> = sections
                .iter()
                .filter(|(name, _)| super::template::resolve("person", name) == Some("who"))
                .flat_map(|(_, facts)| facts.iter().map(|f| f.text.as_str()))
                .collect();
            let brief: Option<String> =
                super::WORDS.iter().map(|w| super::summary_of(&page.body, w.brief)).find(|s| !s.trim().is_empty());
            let found = who
                .iter()
                .find_map(|t| said(&super::split_attribution(t).0))
                .or_else(|| brief.as_deref().and_then(said));
            match found {
                Some((link, group)) => (Some(link), Some(group)),
                None => (None, None),
            }
        }
        "area" | "subject" => (None, page_group(&page.title)),
        _ => (None, None),
    }
}

// -- the pages of a category ------------------------------------------------------------------------------------------

/// A row of a category's screen: a page of the copy, or a records' page (made of its records).
#[derive(Debug, Clone)]
pub struct Listed {
    /// `page:<key>` (the page's item), `records:<type>`.
    pub id: String,
    /// The page's item, or the records page's first record.
    pub at: usize,
    pub records: Option<String>,
    pub title: String,
    pub path: String,
    pub page_kind: String,
    pub notes: usize,
    /// Its next date (the soonest still to come) and the item that carries it.
    pub next: Option<(NaiveDate, usize)>,
    pub group: Option<String>,
    /// Its newest note's day (the grey words: the latest pages first).
    pub latest: String,
    pub archived: bool,
}

fn in_space(f: &Finder, at: usize, space: &str) -> bool {
    space == "all" || f.items[at].space() == space
}

fn sooner(next: &mut Option<(NaiveDate, usize)>, day: NaiveDate, at: usize) {
    if next.is_none_or(|(d, _)| day < d) {
        *next = Some((day, at));
    }
}

fn path_stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The pages of category `id` in `space` on `today`, in the order shown, each with its group (none: no header).
pub fn pages_in(f: &Finder, id: &str, space: &str, today: NaiveDate) -> Vec<Listed> {
    let until = today + Duration::days(AHEAD);
    let mut rows: Vec<Listed> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    // the records' pages: their records in each category
    let mut in_cat: HashMap<String, usize> = HashMap::new();
    for at in 0..f.items.len() {
        let e = &f.items[at];
        if !in_space(f, at, space) {
            continue;
        }
        match e.kind.as_str() {
            "page" => {
                by_id.insert(e.id.clone(), rows.len());
                rows.push(Listed {
                    id: e.id.clone(),
                    at,
                    records: None,
                    title: e.title.clone(),
                    path: e.path.clone().unwrap_or_default(),
                    page_kind: e.page_kind.clone().unwrap_or_else(|| "subject".into()),
                    notes: 0,
                    next: None,
                    group: e.group.clone(),
                    latest: String::new(),
                    archived: e.archived,
                });
                in_cat.insert(e.id.clone(), usize::from(e.categories.iter().any(|c| c == id)));
            }
            "record" => {
                let kind = e.open.get("record").and_then(|r| r.get("type")).and_then(Value::as_str).unwrap_or("").to_string();
                let key = format!("records:{kind}");
                let row = *by_id.entry(key.clone()).or_insert_with(|| {
                    let path = e.path.clone().unwrap_or_default();
                    rows.push(Listed {
                        id: key.clone(),
                        at,
                        records: Some(kind.clone()),
                        title: path_stem(&path),
                        path,
                        page_kind: "records".into(),
                        notes: 0,
                        next: None,
                        group: None,
                        latest: String::new(),
                        archived: false,
                    });
                    rows.len() - 1
                });
                let r = &mut rows[row];
                if e.noted.as_deref().unwrap_or("") > r.latest.as_str() {
                    r.latest = e.noted.clone().unwrap_or_default();
                }
                if let Some(day) = f.day(at, today).filter(|d| *d >= today && *d <= until) {
                    sooner(&mut r.next, day, at);
                }
                // a records' page counts, in a category, the records it has there
                if id == "dates" {
                    r.notes += 1;
                } else if e.categories.iter().any(|c| c == id) {
                    *in_cat.entry(key).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    // the lines: their page's notes, its latest day, its next date
    for at in 0..f.items.len() {
        let e = &f.items[at];
        if e.kind != "line" || !in_space(f, at, space) {
            continue;
        }
        let Some(&row) = e.page.as_ref().and_then(|p| by_id.get(p)) else { continue };
        let r = &mut rows[row];
        r.notes += 1;
        if e.noted.as_deref().unwrap_or("") > r.latest.as_str() {
            r.latest = e.noted.clone().unwrap_or_default();
        }
        if let Some(day) = f.day(at, today).filter(|d| *d >= today && *d <= until) {
            sooner(&mut r.next, day, at);
        }
    }
    let mut out: Vec<Listed> = rows
        .into_iter()
        .filter_map(|mut r| {
            if id == "dates" {
                return r.next.is_some().then_some(r);
            }
            let n = in_cat.get(&r.id).copied().unwrap_or(0);
            if n == 0 {
                return None;
            }
            if r.records.is_some() {
                r.notes = n;
            }
            Some(r)
        })
        .collect();
    let folded = |r: &Listed| super::fold(&r.title);
    if id == "dates" {
        out.sort_by(|a, b| a.next.map(|n| n.0).cmp(&b.next.map(|n| n.0)).then_with(|| folded(a).cmp(&folded(b))));
        for r in &mut out {
            r.group = None;
        }
        return out;
    }
    let person = |r: &Listed| r.page_kind == "person" || r.page_kind == "self";
    for r in &mut out {
        // its next day is Dates' (elsewhere a row says its link and its notes)
        r.next = None;
        r.group = if id == "people" {
            match r.page_kind.as_str() {
                "self" => None,
                "person" => Some(r.group.clone().unwrap_or_else(|| "others".into())),
                _ => Some(r.group.clone().unwrap_or_else(|| "subjects".into())),
            }
        } else if person(r) {
            Some("people".into())
        } else {
            Some("subjects".into())
        };
    }
    let rank = |r: &Listed| match &r.group {
        None => 0,
        Some(g) => 1 + GROUPS.iter().position(|x| x == g).unwrap_or(GROUPS.len()),
    };
    out.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then(a.archived.cmp(&b.archived)).then_with(|| folded(a).cmp(&folded(b)))
    });
    // headers only when the list has two groups or more
    let mut seen: Vec<&str> = Vec::new();
    for r in &out {
        if let Some(g) = r.group.as_deref() {
            if !seen.contains(&g) {
                seen.push(g);
            }
        }
    }
    if seen.len() < 2 {
        for r in &mut out {
            r.group = None;
        }
    }
    out
}

/// The groups of a list, in their order (`groups`: their id and name).
pub fn groups_view(rows: &[Listed], lang: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for r in rows {
        if let Some(g) = r.group.as_deref() {
            if !out.iter().any(|v| v["id"] == g) {
                out.push(json!({"id": g, "title": group_title(lang, g)}));
            }
        }
    }
    out
}

/// What a date is (« Anniversaire », « Lave-linge »): its item's label, a record's title, else its line (60 letters).
fn what(f: &Finder, at: usize) -> String {
    let e = &f.items[at];
    if let Some(label) = e.label.as_deref().filter(|l| !l.is_empty()) {
        return label.to_string();
    }
    if e.kind == "record" {
        return e.title.clone();
    }
    let line = super::LINK.replace_all(&e.line, |c: &regex::Captures| c[1].to_string()).trim().to_string();
    match line.char_indices().nth(60) {
        Some((cut, _)) => format!("{}…", line[..cut].trim_end()),
        None => line,
    }
}

/// A row as the apps show it (`CategoryEntry`).
pub fn entry_view(f: &Finder, r: &Listed, today: NaiveDate) -> Value {
    let e = &f.items[r.at];
    let page = r.records.is_none();
    let mut v = json!({"id": r.id, "kind": if page { "page" } else { "records" }, "page_kind": r.page_kind,
                       "title": r.title, "path": r.path, "notes": r.notes, "group": r.group, "archived": r.archived,
                       "close": page && e.close,
                       "open": if page { json!({"path": r.path}) } else { json!({"path": r.path, "records": r.records}) }});
    if page {
        if let Some(link) = e.link.as_deref() {
            v["link"] = json!(link);
        }
        if let Some(icon) = e.icon.as_deref() {
            v["icon"] = json!(icon);
        }
    }
    if let Some((day, at)) = r.next {
        let yearly = f.items[at].yearly.is_some();
        v["next"] = json!({"date": day.format("%Y-%m-%d").to_string(), "in_days": (day - today).num_days(),
                           "what": what(f, at), "yearly": yearly});
    }
    v
}

/// Whether a row is in one of Papiers' sub-filters (contracts/carnet/sommaire.md §3b): a records' page by its type
/// (Contrats et assurances, Achats for the warranties), a page by the words of its title, aliases and « En bref ».
pub fn in_papers_filter(f: &Finder, r: &Listed, filter: &str) -> bool {
    let words: Vec<&str> = f.items[r.at].fold.split(' ').collect();
    let starts = |p: &[&str]| r.records.is_none() && words.iter().any(|w| p.iter().any(|x| w.starts_with(x)));
    match filter {
        "invoices" => starts(&["factur", "factuur", "invoice", "rechnung"]),
        "contracts" => r.records.as_deref() == Some("contract") || starts(&["contrat", "contract", "vertrag", "contrato"]),
        "warranties" => r.records.as_deref() == Some("purchase") || starts(&["garanti", "warrant", "guarant", "gewahrleist"]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::finder::{Entry, Tables};

    fn person(who: &str, brief: &str) -> Page {
        let mut p = Page::new("person", "Quelqu'un");
        p.body = format!("## En bref\n{brief}\n\n## Qui c'est\n- {who} — *Daily, 2026-11-20*\n");
        p
    }

    #[test]
    fn a_person_s_link_is_the_head_of_who_they_are() {
        let link = |who: &str, brief: &str| link_of(&person(who, brief), "person");
        let said = |l: &str, g: &'static str| (Some(l.to_string()), Some(g));
        assert_eq!(link("Ton beau-frère, marié à [[Claire Durand]]", ""), said("Beau-frère", "family"));
        assert_eq!(link("Ta sœur", ""), said("Sœur", "family"));
        assert_eq!(link("Ton ami de la course à pied, le dimanche matin", ""), said("Ami", "friends"));
        assert_eq!(link("Ta voisine du 3e", ""), said("Voisine", "others"));
        assert_eq!(link("Collègue de l'équipe compta", ""), said("Collègue", "work"));
        assert_eq!(link("Your brother-in-law, married to [[Claire Durand]]", ""), said("Brother-in-law", "family"));
        assert_eq!(link("Your running friend, Sunday mornings", ""), said("Friend", "friends"));
        assert_eq!(link("Dein Lauffreund, sonntagmorgens", ""), said("Lauffreund", "friends"));
        assert_eq!(link("Deine Nachbarin im 3. Stock", ""), said("Nachbarin", "others"));
        assert_eq!(link("Tu cuñado, casado con [[Claire Durand]]", ""), said("Cuñado", "family"));
        assert_eq!(link("Je zwager, getrouwd met [[Claire Durand]]", ""), said("Zwager", "family"));
        // « Qui c'est » first, else « En bref »; someone else's relation is not theirs (« Le mari de Claire, ta sœur »)
        assert_eq!(link("Menuisier", "Ta cousine, à Lyon."), said("Cousine", "family"));
        assert_eq!(link("Menuisier à Nantes", "Le mari de Claire, ta sœur, et le papa d'Inès."), (None, None));
        assert_eq!(link("Le mari de ta sœur", ""), (None, None));
        // a group page, a family area; another page has none
        assert_eq!(link_of(&Page::new("area", "Amis"), "area"), (None, Some("friends")));
        assert_eq!(link_of(&Page::new("area", "Famille élargie"), "area"), (None, Some("family")));
        assert_eq!(link_of(&Page::new("area", "Famille"), "area"), (None, Some("family")));
        assert_eq!(link_of(&Page::new("subject", "Rénovation cuisine"), "subject"), (None, None));
    }

    fn entry(id: &str, kind: &str, title: &str, cats: &[&str]) -> Entry {
        Entry {
            id: id.into(),
            kind: kind.into(),
            title: title.into(),
            path: Some(format!("{title}.md")),
            categories: cats.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        }
    }

    fn line(page: &str, text: &str, cats: &[&str]) -> Entry {
        Entry { page: Some(page.into()), line: text.into(), ..entry(&format!("line:{page}:{text}"), "line", "", cats) }
    }

    #[test]
    fn a_category_lists_pages_never_a_line_a_task_or_a_file() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let diane = Entry { page_kind: Some("person".into()), link: Some("Amie".into()), group: Some("friends".into()),
                            ..entry("page:diane", "page", "Diane", &["people"]) };
        let me = Entry { page_kind: Some("self".into()), ..entry("page:me", "page", "Toi", &["people"]) };
        let items = vec![
            diane,
            me,
            line("page:diane", "Aime le chocolat noir", &["people"]),
            // a line the person filed in Maison: its page stays in Personnes only
            line("page:diane", "Garde les clés de la maison", &["home"]),
            // a file linked to her page (« photo.jpg »), a task about her: never rows
            Entry { open: json!({"document": {"agent": "daily", "file": "photo.jpg"}}), ..entry("doc:daily:1", "document", "photo.jpg", &["people"]) },
            Entry { open: json!({"task": "t1"}), ..entry("task:t1", "task", "Appeler Diane", &["people"]) },
            Entry { page_kind: Some("subject".into()), ..entry("page:jardin", "page", "Jardin", &["home"]) },
        ];
        let f = Finder::new(items, &Tables::default());
        let people = pages_in(&f, "people", "all", today);
        let titles: Vec<&str> = people.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Toi", "Diane"]);
        assert_eq!((people[1].notes, people[1].group.clone()), (2, None));
        let home = pages_in(&f, "home", "all", today);
        assert_eq!(home.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Jardin"]);
        for id in ["people", "home", "papers", "dates", "other"] {
            assert!(pages_in(&f, id, "all", today).iter().all(|r| r.title != "photo.jpg" && r.title != "Appeler Diane"), "{id}");
        }
    }

    #[test]
    fn dates_lists_a_page_by_a_date_to_come_only() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let page = |id: &str, title: &str| Entry { page_kind: Some("person".into()), ..entry(id, "page", title, &["people"]) };
        let dated = |page: &str, text: &str, date: Option<&str>, yearly: Option<&str>| Entry {
            date: date.map(str::to_string),
            yearly: yearly.map(str::to_string),
            label: text.split_once(" : ").map(|(l, _)| l.to_string()),
            ..line(page, text, &["people"])
        };
        let items = vec![
            page("page:marc", "Marc"),
            page("page:diane", "Diane"),
            page("page:lea", "Léa"),
            dated("page:marc", "Anniversaire : 19 décembre", None, Some("12-19")),
            // past: not a date to come
            dated("page:lea", "Vu au concert le 2 novembre", Some("2026-11-02"), None),
            dated("page:diane", "Aime les tulipes", None, None),
        ];
        let f = Finder::new(items, &Tables::default());
        let rows = pages_in(&f, "dates", "all", today);
        assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Marc"]);
        let v = entry_view(&f, &rows[0], today);
        assert_eq!(v["next"], json!({"date": "2026-12-19", "in_days": 23, "what": "Anniversaire", "yearly": true}));
        assert!(rows.iter().all(|r| r.group.is_none()));
    }

    #[test]
    fn a_mark_or_a_size_in_a_line_is_not_a_date() {
        use crate::notebook::categories::when_of;
        let noted = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let when = |t: &str| when_of(t, Some(noted), noted);
        // a line of tastes, a gift idea: never in Dates (Alex 26/11)
        for t in ["Restaurant préféré : Chez Paul, 9/10", "Note du film : 8/10", "Taille de vêtements : 10/12 ans",
                  "Pointure 6/8 years", "15/20 en maths", "Salle de sport ouverte 24/7", "Recette : 1/2 tasse de sucre"] {
            assert_eq!(when(t), None, "{t}");
        }
        // still dates: said as a day, with its year, or a day and a month of two digits
        for t in ["Rendez-vous le 8/10", "Anniversaire : 14/11", "Fête le 3/4", "Paiement 10/12/2026", "Arrivée 14/2"] {
            assert!(when(t).is_some(), "{t}");
        }
    }
}
