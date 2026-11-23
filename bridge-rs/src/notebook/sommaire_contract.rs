//! The Carnet « Le sommaire » contract checked (contracts/carnet/sommaire.json) and its fake bridge written
//! (contracts/carnet/fake-bridge/<lang>/): one answer per route for the demo Carnet, dated 2026-11-22, for the iPhone
//! and Android agents to build on before the bridge answers for real. The categories in them are the bridge's (S1:
//! categories.rs on the demo, the search bench's way of writing it); `categorize.json` and `categorize-undo.json` are
//! the bridge's own answers; the rest (index, groups, summary, a category's page, kept searches, the family's) is
//! assembled from those categories in the contract's shape until S3/S4/S6 make them.
//!
//!     BIPAGENTS_WRITE_FAKE_BRIDGE=1 cargo test --lib write_fake_bridge -- --ignored     write the files again
//!
//! `the_sommaire_contract_holds_together` (in `cargo test`): every type named is defined, the categories are
//! categories.rs's, every example has its route's type (no key the contract does not name), and the fake bridge's
//! files are what the bridge's code makes (ids and the run's day aside).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};

use super::categories as c;
use super::{fold, records, Part};
use crate::Dict;

const LANGS: [&str; 2] = ["fr", "en"];

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(relative)
}

fn read(relative: &str) -> Value {
    let file = repo(relative);
    serde_json::from_str(&std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))).unwrap()
}

pub(super) fn contract() -> Value {
    read("contracts/carnet/sommaire.json")
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

// -- the small type language ----------------------------------------------------------------------------------------

pub(super) fn check(v: &Value, ty: &Value, types: &serde_json::Map<String, Value>, at: &str, errors: &mut Vec<String>) {
    match ty {
        Value::Object(fields) => {
            let Some(o) = v.as_object() else {
                errors.push(format!("{at}: an object, not {v}"));
                return;
            };
            for (key, t) in fields {
                let (name, optional) = match key.strip_suffix('?') {
                    Some(n) => (n, true),
                    None => (key.as_str(), false),
                };
                match o.get(name) {
                    Some(x) => check(x, t, types, &format!("{at}.{name}"), errors),
                    None if !optional => errors.push(format!("{at}: no `{name}`")),
                    None => {}
                }
            }
            for key in o.keys() {
                if !fields.contains_key(key) && !fields.contains_key(&format!("{key}?")) {
                    errors.push(format!("{at}: `{key}` is not in the contract"));
                }
            }
        }
        Value::String(t) => {
            if let Some(inner) = t.strip_suffix('?') {
                if !v.is_null() {
                    check(v, &json!(inner), types, at, errors);
                }
                return;
            }
            let fine = match t.as_str() {
                "string" => v.is_string(),
                "int" => v.is_i64() || v.is_u64(),
                "number" => v.is_number(),
                "bool" => v.is_boolean(),
                "any" => true,
                _ if t.starts_with('[') && t.ends_with(']') => {
                    match v.as_array() {
                        Some(items) => {
                            for (i, x) in items.iter().enumerate() {
                                check(x, &json!(&t[1..t.len() - 1]), types, &format!("{at}[{i}]"), errors);
                            }
                        }
                        None => errors.push(format!("{at}: a list, not {v}")),
                    }
                    true
                }
                _ if t.starts_with("map:") => {
                    match v.as_object() {
                        Some(o) => {
                            for (k, x) in o {
                                check(x, &json!(&t[4..]), types, &format!("{at}.{k}"), errors);
                            }
                        }
                        None => errors.push(format!("{at}: an object, not {v}")),
                    }
                    true
                }
                _ if t.starts_with("enum:") => v.as_str().is_some_and(|x| t[5..].split('|').any(|e| e == x)),
                name => match types.get(name) {
                    Some(def) => {
                        check(v, def, types, at, errors);
                        true
                    }
                    None => {
                        errors.push(format!("{at}: no type {name}"));
                        true
                    }
                },
            };
            if !fine {
                errors.push(format!("{at}: not {t}: {v}"));
            }
        }
        other => errors.push(format!("{at}: a type cannot be {other}")),
    }
}

/// The type names a type uses.
fn named(ty: &Value, out: &mut Vec<String>) {
    match ty {
        Value::Object(fields) => fields.values().for_each(|t| named(t, out)),
        Value::String(t) => {
            let t = t.trim_end_matches('?');
            let t = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(t);
            let t = t.strip_prefix("map:").unwrap_or(t).trim_end_matches('?');
            let t = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(t);
            if t.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
                out.push(t.to_string());
            }
        }
        _ => {}
    }
}

// -- the fake bridge ---------------------------------------------------------------------------------------------------

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 22).unwrap()
}

fn titles(lang: &str) -> Value {
    contract()["categories"]["titles_proposed"][lang].clone()
}

fn title(lang: &str, id: &str) -> String {
    s(&titles(lang)[id])
}

/// The demo Carnet of `lang` as the bridge files it: its index (`IndexItem`s, the main part only). The index never
/// depends on the day it is made: no `dates` among the categories (the phone adds it: `dated`, `date`, `yearly`).
fn demo_index(config: &crate::config::Config, lang: &str) -> Vec<Value> {
    let lang_store = super::language(config);
    let w = super::words(&lang_store);
    let marks = Dict::new();
    let mut items: Vec<Value> = Vec::new();
    let main = super::all_pages(config, Part::Main);
    let without_dates = |cats: Vec<String>| -> Vec<String> { cats.into_iter().filter(|c| c != "dates").collect() };
    for (path, page) in &main {
        let kind = page.kind();
        if kind == "tasks" || kind == "records" || path == &w.inbox_path() {
            continue;
        }
        let filed = c::of_page(config, Part::Main, path, page).categories;
        let key = c::page_key(page, path);
        let page_id = format!("page:{key}");
        let archived = w.archived(path);
        let title = if kind == "self" { super::words(lang).me.to_string() } else { page.title.clone() };
        let aliases = page.aliases();
        let summary = super::summary_of(&page.body, w.brief);
        items.push(json!({"id": page_id, "kind": "page", "part": "main", "path": path, "title": title,
                          "line": crate::tools_knowledge::head(&summary, 240), "categories": filed, "space": "perso",
                          "date": null, "yearly": null, "source": title, "who": "", "noted": null, "archived": archived,
                          "label": null, "answer": null, "page": null, "open": {"path": path},
                          "fold": fold(&format!("{title} {} {summary}", aliases.join(" "))), "aliases": aliases}));
        let sections: Vec<(String, Vec<super::Fact>)> =
            page.sections().into_iter().filter(|(name, _)| fold(name) != fold(w.brief)).collect();
        let texts: Vec<String> = sections.iter().flat_map(|(_, f)| f.iter().map(|x| x.text.clone())).collect();
        let ids = c::line_ids(&key, &texts);
        let mut n = 0;
        for (section, facts) in &sections {
            let sid = super::template::resolve(&kind, section);
            for f in facts {
                // never a reference or a number, never an archived mail's link (as the offline copy: offline.rs)
                if c::holds_reference(&f.text) || f.text.contains("[[archive:") {
                    n += 1;
                    continue;
                }
                let when = c::line_when(&f.text, &f.date, today());
                let (date, yearly) = c::When::fields(when);
                let (label, answer) = match super::lexicon::split_label(&f.text) {
                    Some((l, a)) => (json!(l), json!(a)),
                    None => (Value::Null, Value::Null),
                };
                items.push(json!({"id": ids[n], "kind": "line", "part": "main", "path": path, "title": title,
                                  "line": crate::tools_knowledge::head(&f.text, 240),
                                  "categories": without_dates(c::of_line(&filed, &key, &f.text, &f.date, &marks, today())),
                                  "space": "perso", "date": date, "yearly": yearly, "source": section, "who": f.who,
                                  "noted": if f.date.is_empty() { Value::Null } else { json!(f.date) }, "archived": archived,
                                  "label": label, "answer": answer, "page": page_id,
                                  "open": {"path": path, "section": sid}, "fold": fold(&f.text)}));
                n += 1;
            }
        }
    }
    let tasks_title = w.tasks;
    for t in super::open_tasks(config, Part::Main) {
        let cats = without_dates(c::of_task(&c::Ctx::new(config), &t, &main, &marks, today()));
        let plain: String = t.title.split(" [[").next().unwrap_or(&t.title).to_string();
        items.push(json!({"id": format!("task:{}", t.id), "kind": "task", "part": "main", "path": w.tasks_path(),
                          "title": plain, "line": plain, "categories": cats, "space": "perso",
                          "date": if t.due.is_empty() { Value::Null } else { json!(t.due) }, "yearly": null,
                          "source": tasks_title, "who": t.who, "noted": today().to_string(), "archived": t.archived,
                          "label": null, "answer": null, "page": null, "open": {"task": t.id}, "fold": fold(&plain),
                          "done": false}));
    }
    for t in &records::TYPES {
        for r in records::records(config, t) {
            let (cats, _, _) = c::of_record(t, &r, &marks, today());
            let name = records::summary_head(t, &r);
            let (date, yearly) = c::When::fields(c::record_when(t, &r));
            let id = crate::tools_knowledge::str_or_empty(r.get("id"));
            let line = c::record_line(t, &r);
            items.push(json!({"id": format!("record:{id}"), "kind": "record", "part": "main",
                              "path": records::path_of(&lang_store, t), "title": name, "line": line, "categories": cats,
                              "space": "perso", "date": date, "yearly": yearly,
                              "source": t.titles[super::lang_index(&lang_store)], "who": "", "noted": null,
                              "archived": false, "label": null, "answer": null, "page": null,
                              "open": {"record": {"type": t.id, "id": id}}, "fold": fold(&line)}));
        }
    }
    let demo = read("App/Resources/Demo/notebook-demo.json")[lang].clone();
    for d in demo["documents"].as_array().into_iter().flatten() {
        let subjects: Vec<Value> = d["subjects"].as_array().into_iter().flatten().map(|p| json!({"path": p})).collect();
        let view = json!({"agent": d["agent"], "file": d["name"], "name": d["name"], "subjects": subjects,
                          "linked": d["linked"].as_str().map(|p| json!({"path": p}))});
        let (cats, _, _) = c::of_document(&c::Ctx::new(config), &view, &main, &marks);
        let when = (today() - Duration::days(d["days_ago"].as_i64().unwrap_or(0))).to_string();
        let (agent, file) = (s(&d["agent"]), s(&d["name"]));
        items.push(json!({"id": c::document_key(&agent, &file), "kind": "document", "part": "main", "path": null,
                          "title": file, "line": file, "categories": cats, "space": "perso", "date": null, "yearly": null,
                          "source": agent, "who": agent, "noted": when, "archived": false, "label": null, "answer": null,
                          "page": null, "open": {"document": {"agent": agent, "file": file}}, "fold": fold(&file)}));
    }
    items
}

/// Easter's day in year `y` (the Gregorian computus, Meeus/Jones/Butcher).
fn easter_of(y: i32) -> NaiveDate {
    let (a, b, c) = (y % 19, y / 100, y % 100);
    let (d, e) = (b / 4, b % 4);
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let (i, k) = (c / 4, c % 4);
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(y, month as u32, day as u32).unwrap()
}

/// An index item as the bridge sends it: every key at its default left out (null, false, an empty text or list,
/// `part: main`, `space: perso`).
fn compact(item: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (k, v) in item.as_object().unwrap() {
        let default = match v {
            Value::Null => true,
            Value::Bool(b) => !b,
            Value::String(t) => t.is_empty() || (k == "part" && t == "main") || (k == "space" && t == "perso"),
            Value::Array(a) => a.is_empty(),
            _ => false,
        };
        if !default {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// An index item as the bridge's live answers show it (the search, a category's page): `dates` among its categories
/// on the day it is asked, no `fold`.
fn live(index: &Value) -> Value {
    let mut i = index.clone();
    let when = match (i["date"].as_str(), i["yearly"].as_str()) {
        (Some(d), _) => NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().map(c::When::On),
        (None, Some(md)) => md.split_once('-').and_then(|(m, d)| Some(c::When::Yearly(m.parse().ok()?, d.parse().ok()?))),
        _ => None,
    };
    if when.is_some_and(|w| w.dated(today())) && !has(&i, "dates") {
        let mut cats: Vec<String> = i["categories"].as_array().unwrap().iter().map(s).collect();
        cats.push("dates".into());
        i["categories"] = json!(cats);
    }
    if let Some(o) = i.as_object_mut() {
        for k in ["fold", "aliases", "done"] {
            o.remove(k);
        }
    }
    i
}

/// The index's tables for the phone (drafts until S3: a few neighbours and date phrases a language).
fn tables(lang: &str) -> (Value, Value) {
    let neighbours = match lang {
        "fr" => json!({"toubib": ["medecin"], "note": ["facture"], "bagnole": ["voiture"], "aniv": ["anniversaire"],
                       "bail": ["location", "loyer"]}),
        _ => json!({"doc": ["doctor"], "bill": ["invoice"], "bday": ["birthday"], "lease": ["rent"], "flat": ["apartment"]}),
    };
    let phrases = match lang {
        "fr" => json!([
            {"words": "la semaine prochaine", "from": {"unit": "week", "n": 1, "edge": "start"}, "to": {"unit": "week", "n": 1, "edge": "end"}},
            {"words": "ce mois ci", "from": {"unit": "day", "n": 0}, "to": {"unit": "month", "n": 0, "edge": "end"}},
            {"words": "avant noel", "from": {"unit": "day", "n": 0}, "to": {"unit": "feast", "feast": "christmas_eve"}},
            {"words": "il y a un mois", "from": {"unit": "month", "n": -1, "edge": "start"}, "to": {"unit": "month", "n": -1, "edge": "end"}}]),
        _ => json!([
            {"words": "next week", "from": {"unit": "week", "n": 1, "edge": "start"}, "to": {"unit": "week", "n": 1, "edge": "end"}},
            {"words": "this month", "from": {"unit": "day", "n": 0}, "to": {"unit": "month", "n": 0, "edge": "end"}},
            {"words": "before christmas", "from": {"unit": "day", "n": 0}, "to": {"unit": "feast", "feast": "christmas_eve"}},
            {"words": "a month ago", "from": {"unit": "month", "n": -1, "edge": "start"}, "to": {"unit": "month", "n": -1, "edge": "end"}}]),
    };
    // rules, never days (the copy does not change with the day): a fixed day, or days from Easter
    let easter: serde_json::Map<String, Value> =
        (2026..2036).map(|y| (y.to_string(), json!(easter_of(y).format("%Y-%m-%d").to_string()))).collect();
    let dates = json!({"lang": lang, "phrases": phrases,
                       "feasts": {"christmas_eve": {"month": 12, "day": 24}, "christmas": {"month": 12, "day": 25},
                                  "new_year": {"month": 1, "day": 1}, "easter": {"easter": 0},
                                  "ascension": {"easter": 39}, "pentecost": {"easter": 49}},
                       "easter": easter});
    (neighbours, dates)
}

fn first(v: &Value) -> String {
    v["categories"].as_array().and_then(|c| c.first()).map(s).unwrap_or_default()
}

fn has(v: &Value, cat: &str) -> bool {
    v["categories"].as_array().is_some_and(|c| c.iter().any(|x| x == cat))
}

fn kept(lang: &str, items: &[Value]) -> Vec<Value> {
    let (q, cat) = if lang == "fr" { ("à payer ce mois-ci", "money") } else { ("to pay this month", "money") };
    let count = items.iter().filter(|i| has(i, cat) && has(i, "dates")).count();
    vec![json!({"id": "k1a2b3c", "q": q, "title": q, "count": count, "space": "perso", "category": cat})]
}

/// Every fake answer of `lang` (file name → answer), the categorize ones by the bridge itself.
fn fake_bridge(lang: &str) -> BTreeMap<String, Value> {
    let dir = tempfile::tempdir().unwrap();
    let config = super::tests_config(dir.path());
    super::search_bench::write_demo_for(&config, lang, today());
    // and a page the person just made in the app: no « En bref » yet, so its item has no `line`
    let garden = if lang == "fr" { "Jardin" } else { "Garden" };
    super::new_page(&config, Part::Main, "subject", garden, &super::Who::you(&config)).unwrap().unwrap();
    let items = demo_index(&config, lang);
    // the live answers (search, sommaire, a category's page): Dates on the bridge's day
    let shown: Vec<Value> = items.iter().map(live).collect();
    let (neighbours, dates) = tables(lang);
    let mut out = BTreeMap::new();
    let stop: Vec<&str> = super::lexicon::STOP.to_vec();
    let kept_full = kept(lang, &shown);
    let kept_short: Vec<Value> = kept_full.iter().map(|k| json!({"id": k["id"], "q": k["q"], "title": k["title"]})).collect();
    let sent: Vec<Value> = items.iter().map(compact).collect();
    out.insert("index.json".into(), json!({"v": 1, "epoch": "e7k2m9", "version": 42, "since": null, "full": true, "lang": lang,
        "made": "2026-11-22T07:00:00Z", "limits": {"lines": 20000, "bytes": 8000000}, "truncated": false,
        "online_only": null, "dated": {"before": 90, "after": 730}, "items": sent, "removed": [], "stop": stop,
        "neighbours": neighbours, "dates": dates, "kept": kept_short}));
    let changed: Vec<Value> = items.iter().filter(|i| i["kind"] == "task").take(1).map(compact).collect();
    let gone: Vec<Value> = items.iter().filter(|i| i["kind"] == "line").take(1).map(|i| i["id"].clone()).collect();
    out.insert("index-since.json".into(), json!({"v": 1, "epoch": "e7k2m9", "version": 43, "since": 42, "full": false, "lang": lang,
        "made": "2026-11-22T07:05:00Z", "limits": {"lines": 20000, "bytes": 8000000}, "truncated": false,
        "online_only": null, "dated": {"before": 90, "after": 730}, "items": changed, "removed": gone, "stop": stop,
        "neighbours": neighbours, "dates": dates, "kept": kept_short}));
    // « aniv » typed (« bday »): what it finds once the phone's corrector reads « anniv… » (« birthday »), grouped by
    // their first category
    let wanted: &[&str] = if lang == "fr" { &["aniv", "anniv"] } else { &["bday", "birthday"] };
    let hits: Vec<Value> = items
        .iter()
        .filter(|i| s(&i["fold"]).split(' ').any(|w| wanted.iter().any(|q| w.starts_with(q))))
        .map(live)
        .collect();
    // a result listed once, under its first category; counted in each of its categories (the chips filter on any)
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for h in &hits {
        let cat = first(h);
        match groups.iter_mut().find(|(g, _)| *g == cat) {
            Some((_, list)) => list.push(h),
            None => groups.push((cat, vec![h])),
        }
    }
    let mut counts = serde_json::Map::new();
    counts.insert("all".into(), json!(hits.len()));
    for id in c::PERSONAL {
        let n = hits.iter().filter(|h| has(h, id)).count();
        if n > 0 {
            counts.insert(id.to_string(), json!(n));
        }
    }
    out.insert("search-aniv.json".into(), json!({
        "groups": groups.iter().map(|(g, list)| json!({"category": g, "title": title(lang, g), "count": list.len(),
            "top": list.iter().take(2).map(|i| live(i)).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "counts": counts, "online": null}));
    // the sommaire: each category's count and grey words (its latest titles), Santé locked, « Autres » when not empty
    let line = |id: &str, space: Option<&str>| -> Option<Value> {
        let mine: Vec<&Value> = shown
            .iter()
            .filter(|i| has(i, id) && space.is_none_or(|sp| i["space"] == sp))
            .collect();
        if id == "health" {
            return Some(json!({"id": id, "title": title(lang, id), "count": null, "hint": "", "locked": true}));
        }
        if mine.is_empty() {
            return None;
        }
        let mut names: Vec<String> = Vec::new();
        for i in &mine {
            let t = s(&i["title"]);
            if !names.contains(&t) && names.len() < 3 {
                names.push(t);
            }
        }
        Some(json!({"id": id, "title": title(lang, id), "count": mine.len(), "hint": names.join(", "), "locked": false,
                    "first": live(mine[0])}))
    };
    let spaces = |on: bool, selected: &str| -> Value {
        let ids: &[&str] = if on { &["perso", "pro", "family"] } else { &["all", "family"] };
        let names = |id: &str| match (lang, id) {
            ("fr", "all") => "Moi", ("fr", "perso") => "Perso", ("fr", "pro") => "Pro", ("fr", "family") => "Famille",
            (_, "all") => "Me", (_, "perso") => "Personal", (_, "pro") => "Work", _ => "Family",
        };
        json!(ids.iter().map(|id| json!({"id": id, "title": names(id), "selected": *id == selected})).collect::<Vec<_>>())
    };
    let (carnet, all_notes, bar) = if lang == "fr" {
        ("Carnet", "Tout le Carnet", "Cherche dans tout le Carnet")
    } else {
        ("Notebook", "All the Notebook", "Search the whole Notebook")
    };
    let lines: Vec<Value> = c::PERSONAL.iter().filter_map(|id| line(id, None)).collect();
    out.insert("summary.json".into(), json!({"v": 1, "lang": lang, "title": carnet, "space": "all", "spaces": spaces(false, "all"),
        "kept": kept_full, "card": null, "total": {"count": items.len(), "title": all_notes}, "categories": lines,
        "bar": {"placeholder": bar}, "version": 42}));
    // « Séparer Perso, Pro et Famille » on, the Pro tab: the pages of `space: pro` (S5; here the work area's)
    let pro_items: Vec<Value> = shown
        .iter()
        .map(|i| {
            let mut i = i.clone();
            if has(&i, "work") {
                i["space"] = json!("pro");
            }
            i
        })
        .collect();
    let pro_lines: Vec<Value> = c::PERSONAL
        .iter()
        .filter_map(|id| {
            let mine: Vec<&Value> = pro_items.iter().filter(|i| has(i, id) && i["space"] == "pro").collect();
            if *id == "health" || mine.is_empty() {
                return None;
            }
            let hint: Vec<String> = mine.iter().map(|i| s(&i["title"])).fold(Vec::new(), |mut a, t| {
                if !a.contains(&t) && a.len() < 3 {
                    a.push(t);
                }
                a
            });
            Some(json!({"id": id, "title": title(lang, id), "count": mine.len(), "hint": hint.join(", "), "locked": false,
                        "first": live(mine[0])}))
        })
        .collect();
    let pro_total = pro_items.iter().filter(|i| i["space"] == "pro").count();
    out.insert("summary-pro.json".into(), json!({"v": 1, "lang": lang, "title": carnet, "space": "pro", "spaces": spaces(true, "pro"),
        "kept": [], "card": null, "total": {"count": pro_total, "title": all_notes}, "categories": pro_lines,
        "bar": {"placeholder": bar}, "version": 42}));
    // a category's page: Papiers, its sub-filters by the records' types and the documents' names
    let papers: Vec<&Value> = shown.iter().filter(|i| has(i, "papers")).collect();
    let count = |f: &dyn Fn(&Value) -> bool| papers.iter().filter(|i| f(i)).count();
    let filters = vec![
        json!({"id": "invoices", "title": title(lang, "invoices"), "count": count(&|i: &Value| fold(&s(&i["line"])).contains("factur") || fold(&s(&i["line"])).contains("invoice") || fold(&s(&i["line"])).contains("devis") || fold(&s(&i["line"])).contains("quote"))}),
        json!({"id": "contracts", "title": title(lang, "contracts"), "count": count(&|i: &Value| i["open"]["record"]["type"] == "contract")}),
        json!({"id": "warranties", "title": title(lang, "warranties"), "count": count(&|i: &Value| i["open"]["record"]["type"] == "purchase")}),
    ];
    let mut recent: Vec<Value> = papers.iter().map(|i| live(i)).collect();
    recent.sort_by_key(|i| std::cmp::Reverse(s(&i["noted"])));
    recent.truncate(10);
    let searched = if lang == "fr" { "garantie lave-linge" } else { "washing machine warranty" };
    out.insert("category-papers.json".into(), json!({"v": 1, "id": "papers", "title": title(lang, "papers"), "count": papers.len(),
        "space": "all", "filters": filters, "filter": null,
        "searched": [{"q": searched, "at": "2026-11-20T18:02:00Z"}], "recent": recent,
        "bar": {"placeholder": if lang == "fr" { format!("Cherche dans {}", title(lang, "papers")) } else { format!("Search in {}", title(lang, "papers")) }}}));
    out.insert("kept.json".into(), json!({"kept": kept_full, "max": 6}));
    // S2, the bridge's own: the pass over the demo (its categories written in the pages' headers, the same as above),
    // then the home's card
    assert!(super::categories_pass::sort(&config).unwrap().done);
    let pages = super::all_pages(&config, Part::Main);
    out.insert("home-sorted.json".into(), json!({"sorted": super::categories_pass::card(&config, lang, &pages)}));
    // the bridge's own answers: « Ranger ailleurs » on the kitchen's page (its toast), today's list, and its undo
    let kitchen = |i: &&Value| ["cuisine", "kitchen"].iter().any(|w| fold(&s(&i["title"])).contains(w));
    let path = items.iter().find(|i| i["kind"] == "page" && kitchen(i)).map(|i| s(&i["path"])).unwrap();
    let you = super::Who::you(&config);
    let done = c::categorize(&config, &c::Target::Page { path }, &["money".to_string()], &you).unwrap().unwrap();
    let mut refiled = super::categories_pass::today(&config, lang);
    for i in refiled["items"].as_array_mut().unwrap() {
        i["at"] = json!("2026-11-22T10:00:00+01:00"); // (the run's time)
    }
    out.insert("categorize-today.json".into(), refiled);
    let undone = c::undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
    out.insert("categorize.json".into(), super::categories_pass::with_toast(done, lang));
    out.insert("categorize-undo.json".into(), undone);
    // the family's Carnet (S6): the demo written as `family-notebook.json`, filed, copied and counted by the bridge's
    // own code (family_carnet.rs); the copy's tables stay the drafts (the parity cases are written on them)
    let family = read("App/Resources/Demo/family-notebook-demo.json")[lang].clone();
    let made = today().and_hms_opt(7, 0, 0).unwrap().and_utc().timestamp() as f64;
    let ago = |v: &Value| made - 3600.0 * v["hours_ago"].as_f64().unwrap_or(0.0);
    let lists: Vec<Value> = family["lists"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let items: Vec<Value> = l["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| {
                    let due = i["due_in"].as_i64().map(|d| (today() + Duration::days(d)).to_string());
                    json!({"id": i["id"], "text": i["text"], "done": i["done"].as_bool().unwrap_or(false), "due": due,
                           "by": i["by"], "at": ago(i)})
                })
                .collect();
            json!({"id": l["id"], "title": l["title"], "kind": l["kind"], "items": items, "updated": made, "by": "Marie"})
        })
        .collect();
    let pages: Vec<Value> = family["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| json!({"id": p["id"], "title": p["title"], "text": p["text"], "by": p["by"], "updated": ago(p)}))
        .collect();
    let fam_kept = if lang == "fr" { "courses" } else { "shopping" };
    let mut data = json!({"lists": lists, "pages": pages, "updated": made,
                          "kept": [{"id": "k9f8e7d", "q": fam_kept, "title": fam_kept, "by_household": "self"}]})
    .as_object()
    .cloned()
    .unwrap();
    crate::family_carnet::file_all(&mut data);
    std::fs::write(crate::family_agent::notebook_file(&config), Value::Object(data.clone()).to_string()).unwrap();
    let fam_sent: Vec<Value> = crate::family_carnet::entries(&data).iter().map(super::index::sent).collect();
    out.insert("family-index.json".into(), json!({"v": 1, "epoch": "e3p8q1", "version": 7, "since": null, "full": true, "lang": lang,
        "made": "2026-11-22T07:00:00Z", "dated": {"before": 90, "after": 730}, "items": fam_sent, "removed": [], "stop": stop, "neighbours": tables(lang).0,
        "dates": tables(lang).1, "kept": [{"id": "k9f8e7d", "q": fam_kept, "title": fam_kept}]}));
    let names: Vec<String> = ["Marie", "Léa", "Tom"].iter().map(|n| n.to_string()).collect();
    let mut summary = crate::family_carnet::summary_at(&config, &[lang.to_string()], today(), &names);
    summary["version"] = json!(7);
    summary["epoch"] = json!("e3p8q1");
    out.insert("family-summary.json".into(), summary);
    out
}

/// Ids made at random (pages, tasks, records, undo) replaced by stable ones, in order of appearance.
fn steady(files: &BTreeMap<String, Value>) -> BTreeMap<String, String> {
    let patterns = [
        (regex::Regex::new(r"\bp[0-9a-f]{10}\b").unwrap(), "p"),
        (regex::Regex::new(r"\bt[0-9a-f]{6,16}\b").unwrap(), "t"),
        (regex::Regex::new(r"\br[0-9a-f]{6}\b").unwrap(), "r"),
        (regex::Regex::new(r"\bu[0-9a-f]{12}\b").unwrap(), "u"),
        (regex::Regex::new(r#"(line:[^:"]+:)[0-9a-f]{10}"#).unwrap(), "line"),
        (regex::Regex::new(r"doc:([a-z]+):[0-9a-f]{12}").unwrap(), "doc"),
    ];
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (name, v) in files {
        let mut text = serde_json::to_string_pretty(v).unwrap();
        for (re, mark) in &patterns {
            text = re
                .replace_all(&text, |m: &regex::Captures| {
                    let n = seen.len() + 1;
                    seen.entry(m[0].to_string())
                        .or_insert_with(|| match *mark {
                            "p" => format!("p{n:010x}"),
                            "t" => format!("t{n:06x}"),
                            "r" => format!("r{n:06x}"),
                            "u" => format!("u{n:012x}"),
                            "doc" => format!("doc:{}:{n:012x}", &m[1]),
                            _ => format!("{}{n:010x}", &m[1]),
                        })
                        .clone()
                })
                .into_owned();
        }
        out.insert(name.clone(), text + "\n");
    }
    out
}

#[test]
#[ignore = "writes contracts/carnet/fake-bridge (BIPAGENTS_WRITE_FAKE_BRIDGE=1)"]
fn write_fake_bridge() {
    if std::env::var("BIPAGENTS_WRITE_FAKE_BRIDGE").ok().as_deref() != Some("1") {
        return;
    }
    for lang in LANGS {
        let dir = repo(&format!("contracts/carnet/fake-bridge/{lang}"));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in steady(&fake_bridge(lang)) {
            std::fs::write(dir.join(name), text).unwrap();
        }
    }
}

#[test]
fn the_sommaire_contract_holds_together() {
    let contract = contract();
    let types = contract["types"].as_object().unwrap();
    // every type named is defined
    let mut names = Vec::new();
    for t in types.values() {
        named(t, &mut names);
    }
    for r in contract["routes"].as_array().unwrap() {
        named(&r["answer"], &mut names);
    }
    for n in names.iter().filter(|n| n.chars().all(|c| c.is_ascii_alphanumeric())) {
        assert!(types.contains_key(n) || n.contains(' '), "no type {n}");
    }
    // the categories are the bridge's
    let list = |k: &str| -> Vec<String> { contract["categories"][k].as_array().unwrap().iter().map(s).collect() };
    assert_eq!(list("personal"), c::PERSONAL);
    assert_eq!(list("family"), c::FAMILY);
    assert_eq!(list("agent_may_give"), c::AGENT_MAY_GIVE);
    assert_eq!(list("person_may_give"), c::PERSON_MAY_GIVE);
    assert_eq!(contract["by"], json!(c::BY));
    assert_eq!(contract["categories"]["at_most"], json!(c::AT_MOST));
    for lang in ["fr", "en", "de", "es", "nl"] {
        let t = &contract["categories"]["titles_proposed"][lang];
        for id in c::PERSONAL.iter().chain(c::FAMILY.iter()) {
            assert!(t[*id].is_string(), "{lang}: no title for {id}");
            // the bridge's own table (the toasts, the card) says the same
            assert_eq!(json!(c::title(lang, id)), t[*id], "{lang} {id}");
        }
    }
    // every example has its route's type; the made ones too (the generator follows the contract)
    for lang in LANGS {
        let made = fake_bridge(lang);
        for r in contract["routes"].as_array().unwrap() {
            let Some(example) = r["example"].as_str() else { continue };
            let example = example.replace("/fr/", &format!("/{lang}/"));
            let file = repo(&format!("contracts/carnet/{example}"));
            let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            let v: Value = serde_json::from_str(&text).unwrap();
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let mut errors = Vec::new();
            check(&v, &r["answer"], types, &example, &mut errors);
            check(&made[&name], &r["answer"], types, &format!("made {example}"), &mut errors);
            assert!(errors.is_empty(), "{errors:#?}");
        }
        // the index's categories are the bridge's (Dates aside: they follow the day it runs)
        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(repo(&format!("contracts/carnet/fake-bridge/{lang}/index.json"))).unwrap(),
        )
        .unwrap();
        let filed = |index: &Value| -> BTreeMap<String, Vec<String>> {
            index["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| {
                    let cats = i["categories"].as_array().unwrap().iter().map(s).filter(|c| c != "dates").collect();
                    (format!("{} {} {}", s(&i["kind"]), s(&i["title"]), s(&i["line"])), cats)
                })
                .collect()
        };
        assert_eq!(filed(&file), filed(&made["index.json"]), "{lang}: fake-bridge/index.json is out of date (BIPAGENTS_WRITE_FAKE_BRIDGE=1)");
        assert_eq!(file["items"].as_array().unwrap().len(), made["index.json"]["items"].as_array().unwrap().len());
    }
    // the additive keys name only routes the 1.0 apps read
    for route in contract["additive"].as_object().unwrap().keys() {
        assert!(route.starts_with("GET /v1/notebook"), "{route}");
    }
}
