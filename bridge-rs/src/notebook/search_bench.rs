//! The Carnet search bench (plan « Le sommaire », brick S0): `contracts/carnet/search-bench.json` replayed against the
//! search as it is (no change to it). Each language's demo Carnet (App/Resources/Demo, Dutch from the bench's own
//! fixture) is written into a temporary Carnet, then every case runs what `GET /v1/notebook/search` runs (api.rs
//! `search`: `tools::answers`, then `search`, then the tasks holding every word), in that order. The family Carnet has no
//! search on the bridge: its cases run the phone's rule (HermesKit `FamilyCarnet.search`: every word, case and accents
//! aside, the items then the pages). Documents, agent pages and connected drives are not searched today: their cases
//! miss by construction, they are there for S3.
//!
//!     cargo test --lib search_bench -- --ignored --nocapture       the report (BIPAGENTS_BENCH_OUT=<file>: as JSON too)

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use super::{fold, part_root, records, update_state, write_file, Page, Part, Who};
use crate::config::Config;
use crate::Dict;

const LANGS: [&str; 5] = ["fr", "en", "de", "es", "nl"];
/// Each query this many times; its time is the median.
const RUNS: usize = 5;

fn read(relative: &str) -> Value {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(relative);
    serde_json::from_str(&std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))).unwrap()
}

fn bench() -> Value {
    read("contracts/carnet/search-bench.json")
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

// -- dates as the demo writes them -----------------------------------------------------------------------------------

const MONTHS: [[&str; 12]; 5] = [
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
    ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    ["januari", "februari", "maart", "april", "mei", "juni", "juli", "augustus", "september", "oktober", "november", "december"],
];

fn spoken(lang: &str, d: NaiveDate) -> String {
    let i = LANGS.iter().position(|l| *l == lang).unwrap_or(1);
    let month = MONTHS[i][d.month0() as usize];
    match lang {
        "en" => format!("{month} {}", d.day()),
        "de" => format!("{}. {month}", d.day()),
        "es" => format!("{} de {month}", d.day()),
        _ => format!("{} {month}", d.day()),
    }
}

/// `{date+N}` → the day in words, `{day±N}` → YYYY-MM-DD (the demo's two marks).
fn fill(text: &str, lang: &str, today: NaiveDate) -> String {
    let re = regex::Regex::new(r"\{(date|day)([+-]\d+)\}").unwrap();
    re.replace_all(text, |c: &regex::Captures| {
        let d = today + Duration::days(c[2].parse::<i64>().unwrap());
        if &c[1] == "date" {
            spoken(lang, d)
        } else {
            d.format("%Y-%m-%d").to_string()
        }
    })
    .into_owned()
}

// -- the demo Carnet written as the bridge keeps it --------------------------------------------------------------------

/// What the written Carnet holds, to read the results back as the bench's ids.
#[derive(Default)]
struct Catalogue {
    /// page path → its id (page:…)
    pages: HashMap<String, String>,
    /// (id, page path, folded text)
    facts: Vec<(String, String, String)>,
    /// (id, folded title)
    tasks: Vec<(String, String)>,
    /// (id, records page path, folded key value)
    records: Vec<(String, String, String)>,
    tasks_path: String,
    warnings: Vec<String>,
}

fn who(agent: &str) -> Who {
    let name = if agent == "you" { "toi".to_string() } else { agent[..1].to_uppercase() + &agent[1..] };
    Who { by: agent.into(), name: name.clone(), agent: Some(name), phone: None }
}

#[allow(clippy::too_many_arguments)]
fn write_page(
    config: &Config,
    cat: &mut Catalogue,
    part: Part,
    key: &str,
    p: &Value,
    kind: &str,
    lang: &str,
    today: NaiveDate,
) {
    let path = s(&p["path"]);
    let mut page = Page::new(kind, &s(&p["title"]));
    page.front.insert("aliases".into(), p.get("aliases").cloned().unwrap_or(json!([])));
    if p["close"].as_bool() == Some(true) {
        page.front.insert("close".into(), json!(true));
    }
    if let Some(c) = p["contact"].as_str() {
        page.front.insert("contact".into(), json!(c));
    }
    let brief = super::words(&super::language(config)).brief;
    let mut body = format!("## {brief}\n{}\n", fill(&s(&p["summary"]), lang, today));
    for section in p["sections"].as_array().into_iter().flatten() {
        body.push_str(&format!("\n## {}\n", s(&section["title"])));
        for (n, f) in section["facts"].as_array().into_iter().flatten().enumerate() {
            let text = fill(&s(&f["text"]), lang, today);
            let date = today - Duration::days(f["days_ago"].as_i64().unwrap_or(0));
            let by = who(&s(&f["agent"]));
            body.push_str(&format!("- {text}{}\n", super::attribution(&by, &date.format("%Y-%m-%d").to_string())));
            cat.facts.push((format!("fact:{key}/{}/{n}", s(&section["id"])), path.clone(), fold(&text)));
        }
    }
    page.body = body;
    write_file(&part_root(config, part).join(&path), &page.render()).unwrap();
    cat.pages.insert(path, format!("page:{key}"));
}

/// The language's demo (or the Dutch fixture) written into `config`'s Carnet.
fn write_demo(config: &Config, lang: &str, today: NaiveDate) -> Catalogue {
    let b = bench();
    let (demo, trip) = if lang == "nl" {
        (b["fixtures"]["nl"]["personal"].clone(), b["fixtures"]["nl"]["trip"].clone())
    } else {
        (read("App/Resources/Demo/notebook-demo.json")[lang].clone(), read("App/Resources/Demo/notebook-demo-trip.json")[lang].clone())
    };
    // today's bridge has no Dutch: a Dutch person's Carnet is in its English words (words() falls back to en)
    update_state(config, |st| {
        st.insert("language".into(), json!(if lang == "nl" { "en" } else { lang }));
    })
    .unwrap();
    let mut cat = Catalogue::default();
    let keys: Vec<String> = b["pages"].as_array().unwrap().iter().map(s).collect();
    for (i, p) in demo["pages"].as_array().unwrap().iter().enumerate() {
        write_page(config, &mut cat, Part::Main, &keys[i], p, &s(&p["kind"]), lang, today);
    }
    write_page(config, &mut cat, Part::Main, "me", &demo["me"], "self", lang, today);
    write_page(config, &mut cat, Part::Health, "health", &demo["health"][0], "area", lang, today);
    write_page(config, &mut cat, Part::Main, "trip", &trip["trip"], "trip", lang, today);
    // tasks, by the bridge's own code
    let by = who("daily");
    for t in demo["tasks"].as_array().unwrap() {
        let due = t["due_in"].as_i64().map(|d| (today + Duration::days(d)).format("%Y-%m-%d").to_string()).unwrap_or_default();
        let link = t["links"].get(0).map(s).unwrap_or_default();
        let title = s(&t["title"]);
        match super::add_task(config, Part::Main, &title, &due, &link, "asked", "", &by) {
            Ok(added) => {
                if t["done_ago"].is_number() {
                    let _ = super::set_task_done(config, &[Part::Main], &added.task.id, true, &by, false);
                }
                cat.tasks.push((format!("task:{}", s(&t["id"])), fold(&title)));
            }
            Err(e) => cat.warnings.push(format!("task {title}: {e:#}")),
        }
    }
    cat.tasks_path = super::all_pages(config, Part::Main)
        .into_iter()
        .find(|(_, p)| p.kind() == "tasks" || p.title == super::words(&super::language(config)).tasks)
        .map(|(path, _)| path)
        .unwrap_or_default();
    // records, by the bridge's own code
    for r in trip["records"].as_array().unwrap() {
        let Some(t) = records::kind(&s(&r["type"])) else { continue };
        for it in r["items"].as_array().unwrap() {
            let mut fields = Dict::new();
            for (k, v) in it["values"].as_object().unwrap() {
                let v = fill(&s(v), lang, today);
                if !v.is_empty() {
                    fields.insert(k.clone(), json!(v));
                }
            }
            let key = ["item", "service", "what", "document"]
                .iter()
                .find_map(|k| fields.get(*k).map(|v| fold(&s(v))))
                .unwrap_or_default();
            match records::upsert(config, t, "", &fields, false, &by) {
                Ok(_) => {
                    let path = records::path_of(&super::language(config), t);
                    cat.pages.entry(path.clone()).or_insert_with(|| format!("page:records_{}", t.id));
                    cat.records.push((format!("record:{}", s(&it["id"])), path, key));
                }
                Err(e) => cat.warnings.push(format!("record {key}: {e:#}")),
            }
        }
    }
    if !cat.tasks_path.is_empty() {
        cat.pages.insert(cat.tasks_path.clone(), "page:tasks".into());
    }
    cat
}

// -- one search, as the route makes it ---------------------------------------------------------------------------------

/// A result as the app shows it: the item it is (for its category) and every bench id it shows.
#[derive(Debug, Clone)]
struct Shown {
    primary: String,
    covers: Vec<String>,
    health_line: bool,
}

fn personal(config: &Config, cat: &Catalogue, part: Part, q: &str) -> Vec<Shown> {
    let mut out: Vec<Shown> = Vec::new();
    for a in super::tools::answers(config, &[part], q) {
        let page = cat.pages.get(&a.path).cloned().unwrap_or_else(|| format!("page:?{}", a.path));
        let answer = fold(&a.answer);
        let mut covers: Vec<String> =
            cat.facts.iter().filter(|(_, p, t)| *p == a.path && t.contains(&answer)).map(|(id, _, _)| id.clone()).collect();
        let primary = covers.first().cloned().unwrap_or_else(|| page.clone());
        covers.push(page);
        out.push(Shown { primary, covers, health_line: super::lexicon::is_health(&a.answer) });
    }
    for (_, path, _, lines) in super::search(config, &[part], q, "").into_iter().take(50) {
        let page = cat.pages.get(&path).cloned().unwrap_or_else(|| format!("page:?{path}"));
        let folded: Vec<String> = lines.iter().map(|l| fold(l)).collect();
        let shows = |t: &str| !t.is_empty() && folded.iter().any(|l| l.contains(t));
        let mut covers: Vec<String> = Vec::new();
        covers.extend(cat.facts.iter().filter(|(_, p, t)| *p == path && shows(t)).map(|(id, _, _)| id.clone()));
        if path == cat.tasks_path {
            covers.extend(cat.tasks.iter().filter(|(_, t)| shows(t)).map(|(id, _)| id.clone()));
        }
        covers.extend(cat.records.iter().filter(|(_, p, k)| *p == path && shows(k)).map(|(id, _, _)| id.clone()));
        let primary = if path == cat.tasks_path || page.starts_with("page:records_") {
            covers.first().cloned().unwrap_or_else(|| page.clone())
        } else {
            page.clone()
        };
        covers.push(page);
        out.push(Shown { primary, covers, health_line: lines.iter().any(|l| super::lexicon::is_health(l)) });
    }
    let (wanted, _) = super::lexicon::query(q);
    for t in super::tasks(config, part) {
        if wanted.is_empty() || !wanted.iter().all(|x| fold(&t.title).contains(x.as_str())) {
            continue;
        }
        let title = fold(&t.title);
        let id = cat.tasks.iter().find(|(_, x)| *x == title).map(|(id, _)| id.clone()).unwrap_or_else(|| "task:?".into());
        out.push(Shown { primary: id.clone(), covers: vec![id], health_line: super::lexicon::is_health(&t.title) });
    }
    out
}

/// The family Carnet's search today: the phone's (HermesKit `FamilyCarnet.search`).
fn family(data: &Value, q: &str) -> Vec<Shown> {
    let wanted: Vec<String> = q.split_whitespace().map(fold).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let holds = |text: &str| {
        let t = fold(text);
        wanted.iter().all(|w| t.contains(w.as_str()))
    };
    let mut out = Vec::new();
    for list in data["lists"].as_array().into_iter().flatten() {
        for item in list["items"].as_array().into_iter().flatten() {
            if holds(&s(&item["text"])) {
                let id = format!("fam:list:{}/{}", s(&list["id"]), s(&item["id"]));
                out.push(Shown { primary: id.clone(), covers: vec![id], health_line: false });
            }
        }
    }
    for page in data["pages"].as_array().into_iter().flatten() {
        if holds(&format!("{}\n{}", s(&page["title"]), s(&page["text"]))) {
            let id = format!("fam:page:{}", s(&page["id"]));
            out.push(Shown { primary: id.clone(), covers: vec![id], health_line: false });
        }
    }
    out
}

// -- the windows of the date cases -----------------------------------------------------------------------------------

/// A window as days from today, both ends in.
fn window(name: &str, today: NaiveDate) -> (i64, i64) {
    let wd = today.weekday().num_days_from_monday() as i64;
    let next = |month: u32, day: u32| {
        let this = NaiveDate::from_ymd_opt(today.year(), month, day).unwrap();
        if this >= today { this } else { NaiveDate::from_ymd_opt(today.year() + 1, month, day).unwrap() }
    };
    match name {
        "next_week" => (7 - wd, 13 - wd),
        "this_weekend" => (0, 6 - wd),
        "this_month" => {
            let first_next = if today.month() == 12 {
                NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
            }
            .unwrap();
            (0, (first_next - today).num_days() - 1)
        }
        "december" => {
            let end = next(12, 31);
            let start = NaiveDate::from_ymd_opt(end.year(), 12, 1).unwrap().max(today);
            ((start - today).num_days(), (end - today).num_days())
        }
        "before_christmas" => (0, (next(12, 24) - today).num_days()),
        other => panic!("unknown window {other}"),
    }
}

/// The ids that answer a case (its window applied).
fn expected(case: &Value, items: &serde_json::Map<String, Value>, today: NaiveDate) -> Vec<String> {
    let list: Vec<String> = case["expect"].as_array().unwrap().iter().map(s).collect();
    let Some(w) = case["window"].as_str() else { return list };
    let (a, b) = window(w, today);
    let inside = |id: &str| items[id]["offset"].as_i64().is_some_and(|o| o >= a && o <= b);
    let mut out = Vec::new();
    for e in list {
        if let Some(rest) = e.strip_prefix("*dated") {
            let prefix = match rest {
                ":task" => "task:",
                ":fam" => "fam:",
                _ => "",
            };
            for id in items.keys() {
                let personal = !id.starts_with("fam:") && !id.starts_with("drive:") && !id.starts_with("doc:");
                let kind_ok = if prefix.is_empty() { personal && !id.contains("health") } else { id.starts_with(prefix) };
                if kind_ok && inside(id) {
                    out.push(id.clone());
                }
            }
        } else if inside(&e) {
            out.push(e);
        }
    }
    out
}

// -- the measurements --------------------------------------------------------------------------------------------------

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[i]
}

#[derive(Default)]
struct Tally {
    n: usize,
    hit1: usize,
    hit5: usize,
    with_category: usize,
    wrong_category: usize,
    empty: usize,
    ms: Vec<f64>,
}

impl Tally {
    fn view(&self) -> Value {
        let mut ms = self.ms.clone();
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |x: usize, n: usize| if n == 0 { Value::Null } else { json!((1000.0 * x as f64 / n as f64).round() / 10.0) };
        json!({"n": self.n, "hit1": pct(self.hit1, self.n), "hit5": pct(self.hit5, self.n),
               "wrong_category": pct(self.wrong_category, self.with_category), "no_result": pct(self.empty, self.n),
               "p50_ms": (percentile(&ms, 50.0) * 100.0).round() / 100.0, "p95_ms": (percentile(&ms, 95.0) * 100.0).round() / 100.0})
    }
}

fn gz(bytes: &[u8]) -> usize {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap().len()
}

/// The Carnet's size on disk (the main part, what the phone's copy would come from), and the copies' sizes.
fn sizes(config: &Config, lang: &str, cat: &Catalogue) -> Value {
    let (mut files, mut lines, mut bytes, mut facts) = (0usize, 0usize, 0usize, 0usize);
    let mut index: Vec<Value> = Vec::new();
    for (path, page) in super::all_pages(config, Part::Main) {
        let text = page.render();
        files += 1;
        lines += text.lines().count();
        bytes += text.len();
        for line in page.body.lines().filter(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with('|')) {
            facts += 1;
            let (fact, by) = super::split_attribution(line.trim_start().trim_start_matches("- "));
            let (who, date) = by.unwrap_or_default();
            // the copy the plan describes (§3.1): one line per point, its words folded by the bridge
            index.push(json!({"id": format!("{}#{facts}", page.id()), "kind": page.kind(), "path": path, "title": page.title,
                              "line": crate::tools_knowledge::head(&fact, 240), "categories": ["home"], "space": "",
                              "date": date, "source": "", "who": who, "noted": date, "fold": fold(&fact)}));
        }
    }
    let index = serde_json::to_vec(&index).unwrap();
    let (offline, _) = super::offline::snapshot(config, lang, crate::py::time_time());
    let offline = serde_json::to_vec(&offline).unwrap();
    json!({"pages": files, "lines": lines, "bytes": bytes, "points": facts, "tasks": cat.tasks.len(), "records": cat.records.len(),
           "index_prototype_bytes": index.len(), "index_prototype_gzip": gz(&index),
           "offline_copy_bytes": offline.len(), "offline_copy_gzip": gz(&offline)})
}

/// Today's search on a Carnet grown to `lines` lines (the demo's pages copied): its time per query.
fn at_scale(lang: &str, today: NaiveDate, queries: &[String], target: usize) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let config = super::tests_config(dir.path());
    let cat = write_demo(&config, lang, today);
    let originals = super::all_pages(&config, Part::Main);
    let per_copy: usize = originals.iter().map(|(_, p)| p.render().lines().count()).sum();
    let copies = target.div_ceil(per_copy.max(1));
    let root = part_root(&config, Part::Main);
    for i in 0..copies {
        for (path, page) in &originals {
            let name = path.rsplit('/').next().unwrap();
            let mut copy = page.clone();
            copy.title = format!("{} {i}", page.title);
            copy.front.insert("id".into(), json!(super::new_page_id()));
            write_file(&root.join(format!("Archives/copie{i}/{name}")), &copy.render()).unwrap();
        }
    }
    let total: usize = super::all_pages(&config, Part::Main).iter().map(|(_, p)| p.render().lines().count()).sum();
    let bytes: usize = super::all_pages(&config, Part::Main).iter().map(|(_, p)| p.render().len()).sum();
    let mut ms: Vec<f64> = Vec::new();
    for q in queries {
        let mut runs: Vec<f64> = (0..3)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(personal(&config, &cat, Part::Main, q));
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ms.push(runs[1]);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    json!({"lines": total, "bytes": bytes, "queries": queries.len(),
           "p50_ms": (percentile(&ms, 50.0) * 10.0).round() / 10.0, "p95_ms": (percentile(&ms, 95.0) * 10.0).round() / 10.0})
}

fn report() -> Value {
    let b = bench();
    let items = b["items"].as_object().unwrap().clone();
    let today = chrono::Local::now().date_naive();
    let mut all = Tally::default();
    let mut by: BTreeMap<String, Tally> = BTreeMap::new();
    let mut misses: Vec<Value> = Vec::new();
    let mut sizes_by: BTreeMap<String, Value> = BTreeMap::new();
    let (mut skipped, mut refused_ok, mut refused_n, mut health_leaks, mut health_lines) = (0, 0, 0, 0, 0);
    let mut warnings: Vec<String> = Vec::new();
    for lang in LANGS {
        let dir = tempfile::tempdir().unwrap();
        let config = super::tests_config(dir.path());
        let cat = write_demo(&config, lang, today);
        warnings.extend(cat.warnings.iter().map(|w| format!("{lang}: {w}")));
        let fam = if lang == "nl" {
            b["fixtures"]["nl"]["family"].clone()
        } else {
            read("App/Resources/Demo/family-notebook-demo.json")[lang].clone()
        };
        sizes_by.insert(lang.into(), {
            let mut v = sizes(&config, lang, &cat);
            v["family_bytes"] = json!(serde_json::to_vec(&fam).unwrap().len());
            v
        });
        for case in b["cases"].as_array().unwrap().iter().filter(|c| c["lang"] == lang) {
            let q = s(&case["q"]);
            let is_family = case["carnet"] == "family";
            let health = case["health"].as_bool() == Some(true);
            let part = if health { Part::Health } else { Part::Main };
            let mut times: Vec<f64> = Vec::new();
            let mut shown = Vec::new();
            for _ in 0..RUNS {
                let t = Instant::now();
                shown = if is_family { family(&fam, &q) } else { personal(&config, &cat, part, &q) };
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let ms = times[RUNS / 2];
            if !health && shown.iter().any(|r| r.covers.iter().any(|c| c.contains("health"))) {
                health_leaks += 1;
            }
            health_lines += shown.iter().filter(|r| !health && r.health_line).count();
            if let Some(never) = case["never"].as_array() {
                refused_n += 1;
                if !shown.iter().any(|r| never.iter().any(|n| r.covers.contains(&s(n)))) {
                    refused_ok += 1;
                }
                continue;
            }
            let want = expected(case, &items, today);
            if want.is_empty() {
                skipped += 1;
                continue;
            }
            let rank = shown.iter().position(|r| r.covers.iter().any(|c| want.contains(c)));
            let category = case["category"].as_str();
            let top_cats: Vec<String> = shown
                .first()
                .and_then(|r| items.get(&r.primary))
                .map(|i| i["categories"].as_array().unwrap().iter().map(s).collect())
                .unwrap_or_else(|| if shown.is_empty() { vec![] } else { vec!["other".into()] });
            let wrong = category.is_some() && !shown.is_empty() && !top_cats.iter().any(|c| Some(c.as_str()) == category);
            let carnet = if is_family { "family" } else { "personal" };
            let keys = [
                format!("lang:{lang}"),
                format!("type:{}", s(&case["type"])),
                format!("carnet:{carnet}"),
                format!("{carnet}:{lang}"),
            ];
            let count = |t: &mut Tally| {
                t.n += 1;
                t.hit1 += usize::from(rank == Some(0));
                t.hit5 += usize::from(rank.is_some_and(|r| r < 5));
                t.with_category += usize::from(category.is_some() && !shown.is_empty());
                t.wrong_category += usize::from(wrong);
                t.empty += usize::from(shown.is_empty());
                t.ms.push(ms);
            };
            count(&mut all);
            for k in keys {
                count(by.entry(k).or_default());
            }
            if rank != Some(0) {
                misses.push(json!({"id": case["id"], "type": case["type"], "q": q, "want": want, "rank": rank,
                                   "results": shown.len(), "top": shown.iter().take(3).map(|r| r.primary.clone()).collect::<Vec<_>>(),
                                   "top_category": top_cats, "wrong_category": wrong}));
            }
        }
    }
    let fr_queries: Vec<String> = b["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["lang"] == "fr" && c["carnet"] == "personal" && c["health"].is_null())
        .map(|c| s(&c["q"]))
        .collect();
    json!({
        "run": {"date": today.to_string(), "cases": b["cases"].as_array().unwrap().len(), "skipped_empty_window": skipped,
                "runs_per_query": RUNS, "note": "in-process time of the route's work (no HTTP, no network); build profile of `cargo test`"},
        "all": all.view(),
        "by": by.iter().map(|(k, t)| (k.clone(), t.view())).collect::<serde_json::Map<String, Value>>(),
        "drives_refused_kept_out": format!("{refused_ok}/{refused_n}"),
        "health_leaks": health_leaks,
        "health_looking_lines_in_main_results": health_lines,
        "sizes": sizes_by,
        "scale": [at_scale("fr", today, &fr_queries, 5_000), at_scale("fr", today, &fr_queries, 20_000)],
        "warnings": warnings,
        "misses": misses,
    })
}

#[test]
fn the_bench_file_holds_together() {
    let b = bench();
    let items = b["items"].as_object().unwrap();
    let types: Vec<String> = b["types"].as_array().unwrap().iter().map(s).collect();
    let mut ids = std::collections::HashSet::new();
    let mut per_lang: HashMap<String, usize> = HashMap::new();
    for case in b["cases"].as_array().unwrap() {
        assert!(ids.insert(s(&case["id"])), "same id twice: {}", case["id"]);
        assert!(LANGS.contains(&case["lang"].as_str().unwrap()), "{}", case["id"]);
        assert!(types.contains(&s(&case["type"])), "{}", case["id"]);
        assert!(!s(&case["q"]).trim().is_empty());
        *per_lang.entry(s(&case["lang"])).or_default() += 1;
        for e in case["expect"].as_array().unwrap().iter().chain(case["never"].as_array().into_iter().flatten()) {
            let e = s(e);
            assert!(e.starts_with("*dated") || items.contains_key(&e), "{}: {e}", case["id"]);
        }
        if let Some(w) = case["window"].as_str() {
            window(w, NaiveDate::from_ymd_opt(2026, 11, 21).unwrap());
        }
        let carnet = if case["carnet"] == "family" { "family" } else { "personal" };
        if let Some(c) = case["category"].as_str() {
            assert!(b["categories"][carnet].as_array().unwrap().iter().any(|x| x == c), "{}: {c}", case["id"]);
        }
    }
    assert!(LANGS.iter().all(|l| per_lang.get(*l).copied().unwrap_or(0) >= 40), "{per_lang:?}");
    // the demo written as the bridge keeps it: every page, task and record there (the ids resolve)
    let dir = tempfile::tempdir().unwrap();
    let config = super::tests_config(dir.path());
    let cat = write_demo(&config, "fr", NaiveDate::from_ymd_opt(2026, 11, 21).unwrap());
    assert!(cat.warnings.is_empty(), "{:?}", cat.warnings);
    assert_eq!((cat.tasks.len(), cat.records.len()), (20, 9));
    for (id, _, _) in &cat.facts {
        assert!(items.contains_key(id), "{id}");
    }
}

#[test]
#[ignore = "the search bench's report (cargo test --lib search_bench -- --ignored --nocapture)"]
fn search_bench_report() {
    let report = report();
    if let Ok(out) = std::env::var("BIPAGENTS_BENCH_OUT") {
        std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
    let mut short = report.clone();
    short.as_object_mut().unwrap().remove("misses");
    println!("SEARCH-BENCH {}", serde_json::to_string_pretty(&short).unwrap());
}
