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

fn contract() -> Value {
    read("contracts/carnet/sommaire.json")
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

// -- the small type language ----------------------------------------------------------------------------------------

fn check(v: &Value, ty: &Value, types: &serde_json::Map<String, Value>, at: &str, errors: &mut Vec<String>) {
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

/// The demo Carnet of `lang` as the bridge files it: its index (`IndexItem`s, the main part only).
fn demo_index(config: &crate::config::Config, lang: &str) -> Vec<Value> {
    let lang_store = super::language(config);
    let w = super::words(&lang_store);
    let marks = Dict::new();
    let mut items: Vec<Value> = Vec::new();
    let main = super::all_pages(config, Part::Main);
    for (path, page) in &main {
        let kind = page.kind();
        if kind == "tasks" || kind == "records" || path == &w.inbox_path() {
            continue;
        }
        let filed = c::of_page(config, Part::Main, path, page).categories;
        let pid = page.id();
        let date = page.front.get("updated").map(s).filter(|d| !d.is_empty());
        let title = if kind == "self" { super::words(lang).me.to_string() } else { page.title.clone() };
        let aliases = page.aliases();
        let summary = super::summary_of(&page.body, w.brief);
        items.push(json!({"id": format!("page:{pid}"), "kind": "page", "part": "main", "path": path, "title": title,
                          "line": crate::tools_knowledge::head(&summary, 240), "categories": filed, "space": "",
                          "date": date, "source": title, "who": "", "noted": date, "open": {"path": path},
                          "fold": fold(&format!("{title} {} {summary}", aliases.join(" "))), "aliases": aliases}));
        for (section, facts) in page.sections() {
            if fold(&section) == fold(w.brief) {
                continue;
            }
            let id = super::template::resolve(&kind, &section);
            for f in facts {
                let cats = c::of_line(&filed, &pid, &f.text, &marks, today());
                let date = c::date_of(&f.text, today()).filter(|d| c::in_window(*d, today())).map(|d| d.to_string());
                items.push(json!({"id": c::line_key(&pid, &f.text).replacen("line:", "line:", 1), "kind": "line",
                                  "part": "main", "path": path, "title": title, "line": crate::tools_knowledge::head(&f.text, 240),
                                  "categories": cats, "space": "", "date": date, "source": section, "who": f.who,
                                  "noted": if f.date.is_empty() { Value::Null } else { json!(f.date) },
                                  "open": {"path": path, "section": id}, "fold": fold(&f.text)}));
            }
        }
    }
    let tasks_title = w.tasks;
    for t in super::open_tasks(config, Part::Main) {
        let cats = c::of_task(config, &t, &main, &marks, today());
        let plain: String = t.title.split(" [[").next().unwrap_or(&t.title).to_string();
        items.push(json!({"id": format!("task:{}", t.id), "kind": "task", "part": "main", "path": w.tasks_path(),
                          "title": plain, "line": plain, "categories": cats, "space": "",
                          "date": if t.due.is_empty() { Value::Null } else { json!(t.due) }, "source": tasks_title,
                          "who": t.who, "noted": today().to_string(), "open": {"task": t.id}, "fold": fold(&plain),
                          "done": false}));
    }
    for t in &records::TYPES {
        for r in records::records(config, t) {
            let (cats, _, dated) = c::of_record(t, &r, &marks, today());
            let name = records::summary_head(t, &r);
            let next = records::next_date_on(t, &r, today()).map(|d| d.to_string());
            let id = crate::tools_knowledge::str_or_empty(r.get("id"));
            items.push(json!({"id": format!("record:{id}"), "kind": "record", "part": "main",
                              "path": records::path_of(&lang_store, t), "title": name,
                              "line": records::summary_of(t, &r), "categories": c::with_dates(&cats, dated), "space": "",
                              "date": next, "source": t.titles[super::lang_index(&lang_store)], "who": "", "noted": Value::Null,
                              "open": {"record": {"type": t.id, "id": id}}, "fold": fold(&records::summary_of(t, &r))}));
        }
    }
    let demo = read("App/Resources/Demo/notebook-demo.json")[lang].clone();
    for d in demo["documents"].as_array().into_iter().flatten() {
        let subjects: Vec<Value> = d["subjects"].as_array().into_iter().flatten().map(|p| json!({"path": p})).collect();
        let view = json!({"agent": d["agent"], "file": d["name"], "name": d["name"], "subjects": subjects,
                          "linked": d["linked"].as_str().map(|p| json!({"path": p}))});
        let (cats, _, _) = c::of_document(config, &view, &main, &marks);
        let when = (today() - Duration::days(d["days_ago"].as_i64().unwrap_or(0))).to_string();
        let file = s(&d["name"]);
        items.push(json!({"id": format!("doc:{}/{file}", s(&d["agent"])), "kind": "document", "part": "main", "path": Value::Null,
                          "title": file, "line": file, "categories": cats, "space": "", "date": when,
                          "source": s(&d["agent"]), "who": s(&d["agent"]), "noted": when,
                          "open": {"document": {"agent": d["agent"], "file": d["name"]}}, "fold": fold(&file)}));
    }
    items
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
    let dates = json!({"lang": lang, "phrases": phrases,
                       "feasts": {"christmas_eve": "2026-12-24", "christmas": "2026-12-25", "easter": "2027-03-28"}});
    (neighbours, dates)
}

fn item_of(index: &Value) -> Value {
    let mut i = index.clone();
    if let Some(o) = i.as_object_mut() {
        for k in ["fold", "aliases", "done"] {
            o.remove(k);
        }
    }
    i
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
    vec![json!({"id": "k1a2b3c", "q": q, "title": q, "count": count, "space": "", "category": cat})]
}

/// Every fake answer of `lang` (file name → answer), the categorize ones by the bridge itself.
fn fake_bridge(lang: &str) -> BTreeMap<String, Value> {
    let dir = tempfile::tempdir().unwrap();
    let config = super::tests_config(dir.path());
    super::search_bench::write_demo_for(&config, lang, today());
    let items = demo_index(&config, lang);
    let (neighbours, dates) = tables(lang);
    let mut out = BTreeMap::new();
    let stop: Vec<&str> = super::lexicon::STOP.to_vec();
    let kept_full = kept(lang, &items);
    let kept_short: Vec<Value> = kept_full.iter().map(|k| json!({"id": k["id"], "q": k["q"], "title": k["title"]})).collect();
    out.insert("index.json".into(), json!({"v": 1, "version": 42, "since": null, "full": true, "lang": lang,
        "made": "2026-11-22T07:00:00Z", "limits": {"lines": 20000, "bytes": 8000000}, "truncated": false,
        "online_only": null, "items": items, "removed": [], "stop": stop, "neighbours": neighbours, "dates": dates,
        "kept": kept_short}));
    let changed: Vec<Value> = items.iter().filter(|i| i["kind"] == "task").take(1).cloned().collect();
    let gone: Vec<Value> = items.iter().filter(|i| i["kind"] == "line").take(1).map(|i| i["id"].clone()).collect();
    out.insert("index-since.json".into(), json!({"v": 1, "version": 43, "since": 42, "full": false, "lang": lang,
        "made": "2026-11-22T07:05:00Z", "limits": {"lines": 20000, "bytes": 8000000}, "truncated": false,
        "online_only": null, "items": changed, "removed": gone, "stop": stop, "neighbours": neighbours, "dates": dates,
        "kept": kept_short}));
    // « aniv » typed (« bday »): what it finds once the phone's corrector reads « anniv… » (« birthday »), grouped by
    // their first category
    let wanted: &[&str] = if lang == "fr" { &["aniv", "anniv"] } else { &["bday", "birthday"] };
    let hits: Vec<&Value> =
        items.iter().filter(|i| s(&i["fold"]).split(' ').any(|w| wanted.iter().any(|q| w.starts_with(q)))).collect();
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
    for (g, list) in &groups {
        counts.insert(g.clone(), json!(list.len()));
    }
    out.insert("search-aniv.json".into(), json!({
        "groups": groups.iter().map(|(g, list)| json!({"category": g, "title": title(lang, g), "count": list.len(),
            "top": list.iter().take(2).map(|i| item_of(i)).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "counts": counts, "online": null}));
    // the sommaire: each category's count and grey words (its latest titles), Santé locked, « Autres » when not empty
    let line = |id: &str, space: Option<&str>| -> Option<Value> {
        let mine: Vec<&Value> = items
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
                    "first": item_of(mine[0])}))
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
    let pro_items: Vec<Value> = items
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
                        "first": item_of(mine[0])}))
        })
        .collect();
    let pro_total = pro_items.iter().filter(|i| i["space"] == "pro").count();
    out.insert("summary-pro.json".into(), json!({"v": 1, "lang": lang, "title": carnet, "space": "pro", "spaces": spaces(true, "pro"),
        "kept": [], "card": null, "total": {"count": pro_total, "title": all_notes}, "categories": pro_lines,
        "bar": {"placeholder": bar}, "version": 42}));
    // a category's page: Papiers, its sub-filters by the records' types and the documents' names
    let papers: Vec<&Value> = items.iter().filter(|i| has(i, "papers")).collect();
    let count = |f: &dyn Fn(&Value) -> bool| papers.iter().filter(|i| f(i)).count();
    let filters = vec![
        json!({"id": "invoices", "title": title(lang, "invoices"), "count": count(&|i: &Value| fold(&s(&i["line"])).contains("factur") || fold(&s(&i["line"])).contains("invoice") || fold(&s(&i["line"])).contains("devis") || fold(&s(&i["line"])).contains("quote"))}),
        json!({"id": "contracts", "title": title(lang, "contracts"), "count": count(&|i: &Value| i["open"]["record"]["type"] == "contract")}),
        json!({"id": "warranties", "title": title(lang, "warranties"), "count": count(&|i: &Value| i["open"]["record"]["type"] == "purchase")}),
    ];
    let mut recent: Vec<Value> = papers.iter().map(|i| item_of(i)).collect();
    recent.sort_by_key(|i| std::cmp::Reverse(s(&i["noted"])));
    recent.truncate(10);
    let searched = if lang == "fr" { "garantie lave-linge" } else { "washing machine warranty" };
    out.insert("category-papers.json".into(), json!({"v": 1, "id": "papers", "title": title(lang, "papers"), "count": papers.len(),
        "space": "all", "filters": filters, "filter": null,
        "searched": [{"q": searched, "at": "2026-11-20T18:02:00Z"}], "recent": recent,
        "bar": {"placeholder": if lang == "fr" { format!("Cherche dans {}", title(lang, "papers")) } else { format!("Search in {}", title(lang, "papers")) }}}));
    out.insert("kept.json".into(), json!({"kept": kept_full, "max": 6}));
    // the bridge's own answers: « Ranger ailleurs » on the kitchen's page, and its undo
    let kitchen = |i: &&Value| ["cuisine", "kitchen"].iter().any(|w| fold(&s(&i["title"])).contains(w));
    let path = items.iter().find(|i| i["kind"] == "page" && kitchen(i)).map(|i| s(&i["path"])).unwrap();
    let you = super::Who::you(&config);
    let done = c::categorize(&config, &c::Target::Page { path }, &["money".to_string()], &you).unwrap().unwrap();
    let undone = c::undo(&config, done["undo"].as_str().unwrap(), &you).unwrap().unwrap();
    out.insert("categorize.json".into(), done);
    out.insert("categorize-undo.json".into(), undone);
    // the family's Carnet: its lists and pages, their categories from the bench's reference until S6
    let family = read("App/Resources/Demo/family-notebook-demo.json")[lang].clone();
    let bench = read("contracts/carnet/search-bench.json");
    let reference = bench["items"].as_object().unwrap();
    let mut fam: Vec<Value> = Vec::new();
    for list in family["lists"].as_array().into_iter().flatten() {
        for it in list["items"].as_array().into_iter().flatten() {
            let id = format!("fam:list:{}/{}", s(&list["id"]), s(&it["id"]));
            let cats = reference.get(&id).map(|r| r["categories"].clone()).unwrap_or(json!(["other"]));
            let date = it["due_in"].as_i64().map(|d| (today() + Duration::days(d)).to_string());
            fam.push(json!({"id": id, "kind": "list_item", "title": s(&list["title"]), "line": s(&it["text"]), "categories": cats,
                            "date": date, "who": s(&it["by"]), "noted": Value::Null,
                            "open": {"list": s(&list["id"]), "item": s(&it["id"])}, "fold": fold(&s(&it["text"])),
                            "done": it["done"].as_bool().unwrap_or(false)}));
        }
    }
    for p in family["pages"].as_array().into_iter().flatten() {
        let id = format!("fam:page:{}", s(&p["id"]));
        let cats = reference.get(&id).map(|r| r["categories"].clone()).unwrap_or(json!(["other"]));
        fam.push(json!({"id": id, "kind": "page", "title": s(&p["title"]), "line": crate::tools_knowledge::head(&s(&p["text"]), 240),
                        "categories": cats, "date": Value::Null, "who": s(&p["by"]), "noted": Value::Null,
                        "open": {"path": s(&p["id"])}, "fold": fold(&format!("{} {}", s(&p["title"]), s(&p["text"])))}));
    }
    let fam_kept = if lang == "fr" { "courses" } else { "shopping" };
    out.insert("family-index.json".into(), json!({"v": 1, "version": 7, "since": null, "full": true, "lang": lang,
        "made": "2026-11-22T07:00:00Z", "items": fam, "removed": [], "stop": stop, "neighbours": tables(lang).0,
        "dates": tables(lang).1, "kept": [{"id": "k9f8e7d", "q": fam_kept, "title": fam_kept}]}));
    let fam_lines: Vec<Value> = c::FAMILY
        .iter()
        .filter_map(|id| {
            let mine: Vec<&Value> = fam.iter().filter(|i| i["categories"].as_array().is_some_and(|c| c.iter().any(|x| x == id))).collect();
            if mine.is_empty() {
                return None;
            }
            let hint: Vec<String> = mine.iter().take(3).map(|i| s(&i["line"])).collect();
            Some(json!({"id": id, "title": title(lang, id), "count": mine.len(), "hint": hint.join(", "), "locked": false}))
        })
        .collect();
    let shopping = fam.iter().filter(|i| i["categories"].as_array().is_some_and(|c| c.iter().any(|x| x == "shopping"))).count();
    out.insert("family-summary.json".into(), json!({"v": 1, "lang": lang, "title": carnet,
        "with": if lang == "fr" { "Avec Marie, Léa et Tom" } else { "With Marie, Léa and Tom" },
        "kept": [{"id": "k9f8e7d", "q": fam_kept, "title": fam_kept, "count": shopping, "space": ""}],
        "total": {"count": fam.len(), "title": all_notes}, "categories": fam_lines,
        "bar": {"placeholder": if lang == "fr" { "Cherche pour la famille" } else { "Search for the family" }}, "version": 7}));
    out
}

/// Ids made at random (pages, tasks, records, undo) replaced by stable ones, in order of appearance.
fn steady(files: &BTreeMap<String, Value>) -> BTreeMap<String, String> {
    let patterns = [
        (regex::Regex::new(r"\bp[0-9a-f]{10}\b").unwrap(), "p"),
        (regex::Regex::new(r"\bt[0-9a-f]{6,16}\b").unwrap(), "t"),
        (regex::Regex::new(r"\br[0-9a-f]{6}\b").unwrap(), "r"),
        (regex::Regex::new(r"\bu[0-9a-f]{12}\b").unwrap(), "u"),
        (regex::Regex::new(r"line:[^:]+:[0-9a-f]{10}").unwrap(), "line"),
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
                            _ => format!("line:p{n:010x}:{n:010x}"),
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
