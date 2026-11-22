//! The Carnet search bench (plan « Le sommaire », brick S0): `contracts/carnet/search-bench.json` replayed against the
//! search as it is (no change to it). Each language's demo Carnet (App/Resources/Demo, Dutch from the bench's own
//! fixture) is written into a temporary Carnet, then every case runs what `GET /v1/notebook/search` runs (api.rs
//! `search`: `tools::answers`, then `search`, then the tasks holding every word), in that order. The family Carnet has no
//! search on the bridge: its cases run the phone's rule (HermesKit `FamilyCarnet.search`: every word, case and accents
//! aside, the items then the pages). Documents, agent pages and connected drives are not searched today: their cases
//! miss by construction, they are there for S3.
//!
//! Brick S1 (categories.rs) adds the categories the bridge gives each item of the demo, against the bench's own
//! (`items`: the first one, the one the item is grouped under, must be among the bridge's): ≥ 90 % in each language
//! (`the_demo_is_filed_in_its_categories`, in `cargo test`); and the report's « mauvaise catégorie en tête » measured
//! twice: with the bench's categories of the first result (S0's measure: the ranking's, unchanged by S1) and with the
//! bridge's own (before S1: none, every result in « Autres »).
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
    /// fact id → its text as written
    texts: HashMap<String, String>,
}

fn who(agent: &str) -> Who {
    let name = if agent == "you" { "toi".to_string() } else { agent[..1].to_uppercase() + &agent[1..] };
    Who { by: agent.into(), name: name.clone(), agent: Some(name) }
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
            let id = format!("fact:{key}/{}/{n}", s(&section["id"]));
            cat.texts.insert(id.clone(), text.clone());
            cat.facts.push((id, path.clone(), fold(&text)));
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

/// The language's demo written into `config`'s Carnet (sommaire_contract.rs's fake bridge).
pub(super) fn write_demo_for(config: &Config, lang: &str, today: NaiveDate) {
    write_demo(config, lang, today);
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
    hit2: usize,
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
        json!({"n": self.n, "hit1": pct(self.hit1, self.n), "hit2": pct(self.hit2, self.n), "hit5": pct(self.hit5, self.n),
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
    // what /pages costs with every page's categories (none written yet, as before S2): the pages read, then the
    // categories with the language and rules read for each page (S1 as first pushed) or once (review 1, R1)
    let median = |work: &dyn Fn()| -> f64 {
        let mut runs: Vec<f64> = (0..3)
            .map(|_| {
                let t = Instant::now();
                work();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (runs[1] * 10.0).round() / 10.0
    };
    let pages = super::all_pages(&config, Part::Main).len();
    let read = median(&|| {
        std::hint::black_box(super::all_pages(&config, Part::Main));
    });
    let each = median(&|| {
        for (path, page) in super::all_pages(&config, Part::Main) {
            std::hint::black_box(super::categories::of_page(&config, Part::Main, &path, &page));
        }
    });
    let once = median(&|| {
        let ctx = super::categories::Ctx::new(&config);
        for (path, page) in super::all_pages(&config, Part::Main) {
            std::hint::black_box(super::categories::of_page_in(&ctx, Part::Main, &path, &page));
        }
    });
    // brick S3: the copy made once (as the first search after a change does), then each query as the route runs it:
    // the Carnet's files looked at (no read), the copy kept, the search on it
    let langs = vec![lang.to_string()];
    let cold = median(&|| {
        std::hint::black_box(super::index::rebuilt_for_bench(&config, &langs));
    });
    let mut route_ms: Vec<f64> = Vec::new();
    let mut search_ms: Vec<f64> = Vec::new();
    for q in queries {
        let mut runs: Vec<(f64, f64)> = (0..3)
            .map(|_| {
                let t = Instant::now();
                let f = super::index::finder(&config, &langs);
                let t2 = Instant::now();
                std::hint::black_box(f.search(q, today, None, None));
                (t.elapsed().as_secs_f64() * 1000.0, t2.elapsed().as_secs_f64() * 1000.0)
            })
            .collect();
        runs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        route_ms.push(runs[1].0);
        search_ms.push(runs[1].1);
    }
    route_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    search_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // one page written: the copy made again for that page only (each page's items kept, index.rs)
    let (path, page) = super::all_pages(&config, Part::Main).into_iter().find(|(p, _)| p.starts_with("Archives/")).unwrap();
    let mut changed = page.clone();
    changed.body.push_str("\n- Une ligne de plus\n");
    write_file(&root.join(&path), &changed.render()).unwrap();
    let t = Instant::now();
    std::hint::black_box(super::index::finder(&config, &langs));
    let after_write = (t.elapsed().as_secs_f64() * 10000.0).round() / 10.0;
    let finder = super::index::finder(&config, &langs);
    let copy_bytes: usize = finder.items.iter().map(|e| serde_json::to_vec(&super::index::sent(e)).unwrap().len() + 1).sum();
    let s3 = json!({"items": finder.items.len(), "copy_bytes": copy_bytes, "finder_bytes_estimate": finder.footprint(),
                    "copy_made_ms": cold, "copy_after_one_write_ms": after_write, "route_p50_ms": (percentile(&route_ms, 50.0) * 10.0).round() / 10.0,
                    "route_p95_ms": (percentile(&route_ms, 95.0) * 10.0).round() / 10.0,
                    "search_p50_ms": (percentile(&search_ms, 50.0) * 100.0).round() / 100.0,
                    "search_p95_ms": (percentile(&search_ms, 95.0) * 100.0).round() / 100.0});
    json!({"lines": total, "bytes": bytes, "queries": queries.len(), "s3": s3,
           "p50_ms": (percentile(&ms, 50.0) * 10.0).round() / 10.0, "p95_ms": (percentile(&ms, 95.0) * 10.0).round() / 10.0,
           "pages": pages, "pages_read_ms": read, "pages_with_categories_ms_state_each_page": each,
           "pages_with_categories_ms_state_once": once})
}

/// The sommaire's search on a Carnet grown to `items` items of the copy (its limit: 20 000): the copy made, made again
/// after one write, each query as the route runs it (S3 only: today's search takes seconds there).
fn at_scale_s3(lang: &str, today: NaiveDate, queries: &[String], items: usize) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let config = super::tests_config(dir.path());
    write_demo(&config, lang, today);
    let originals = super::all_pages(&config, Part::Main);
    let per_copy = super::index::entries(&config, &[]).iter().filter(|e| e.kind == "page" || e.kind == "line").count();
    let copies = items.div_ceil(per_copy.max(1));
    let root = part_root(&config, Part::Main);
    for i in 0..copies {
        for (path, page) in &originals {
            let name = path.rsplit('/').next().unwrap();
            let mut copy = page.clone();
            copy.title = format!("{} {i}", page.title);
            copy.front.insert("id".into(), json!(super::new_page_id()));
            // (a folder of digits only: « copie123 » would read as a reference, and its pages leave the copy)
            write_file(&root.join(format!("Archives/{i}/{name}")), &copy.render()).unwrap();
        }
    }
    let langs = vec![lang.to_string()];
    let rss_before = rss_kb();
    let t = Instant::now();
    let finder = super::index::finder(&config, &langs);
    let made = t.elapsed().as_secs_f64() * 1000.0;
    // a phone in French and one in English: two engines on the same copy
    let en = super::index::finder(&config, &[lang.to_string(), "en".to_string()]);
    let rss_after = rss_kb();
    drop(en);
    let (path, page) = super::all_pages(&config, Part::Main).into_iter().find(|(p, _)| p.starts_with("Archives/")).unwrap();
    let mut changed = page.clone();
    changed.body.push_str("\n- Une ligne de plus\n");
    write_file(&root.join(&path), &changed.render()).unwrap();
    let t = Instant::now();
    std::hint::black_box(super::index::finder(&config, &langs));
    let after_write = t.elapsed().as_secs_f64() * 1000.0;
    let mut route_ms: Vec<f64> = Vec::new();
    let mut search_ms: Vec<f64> = Vec::new();
    for q in queries {
        let t = Instant::now();
        let f = super::index::finder(&config, &langs);
        let t2 = Instant::now();
        std::hint::black_box(f.search(q, today, None, None));
        route_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        search_ms.push(t2.elapsed().as_secs_f64() * 1000.0);
    }
    route_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    search_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let r = |x: f64| (x * 10.0).round() / 10.0;
    let lines: usize = super::all_pages(&config, Part::Main).iter().map(|(_, p)| p.render().lines().count()).sum();
    let (body, _) = super::index::answer(&config, &langs, None, None, "");
    json!({"text_lines": lines, "items": finder.items.len(), "index_json_bytes": serde_json::to_vec(&body).unwrap().len(),
           "rss_kb_before": rss_before, "rss_kb_after_copy_and_two_engines": rss_after,
           "rss_mb_added": rss_after.zip(rss_before).map(|(a, b)| ((a as f64 - b as f64) / 102.4).round() / 10.0),
           "index_gzip_bytes": gz(&serde_json::to_vec(&body).unwrap()), "finder_bytes_estimate": finder.footprint(),
           "copy_made_ms": r(made), "copy_after_one_write_ms": r(after_write), "route_p50_ms": r(percentile(&route_ms, 50.0)),
           "route_p95_ms": r(percentile(&route_ms, 95.0)), "search_p50_ms": r(percentile(&search_ms, 50.0)),
           "search_p95_ms": r(percentile(&search_ms, 95.0))})
}

/// The process's resident memory (Linux, the build server: `/proc/self/status`), in KB.
fn rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|l| l.strip_prefix("VmRSS:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

/// What an installed 1.0 app's search costs after a write, with 500 documents (review 1 of S3, R3): `documents()`
/// alone, the copy made again (the documents listed, their pages read), and the search route's 1.0 work.
fn with_documents(lang: &str, today: NaiveDate, documents: usize) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let mut config = super::tests_config(dir.path());
    let uploads = dir.path().join("uploads");
    config.agents.insert(
        "daily".into(),
        crate::config::AgentConfig { name: "daily".into(), upload_dir_host: Some(uploads.to_string_lossy().into_owned()), ..Default::default() },
    );
    let cat = write_demo(&config, lang, today);
    let originals = super::all_pages(&config, Part::Main);
    let root = part_root(&config, Part::Main);
    // the demo copied to about 1 000 pages
    for i in 0..48 {
        for (path, page) in &originals {
            let name = path.rsplit('/').next().unwrap();
            let mut copy = page.clone();
            copy.title = format!("{} {i}", page.title);
            copy.front.insert("id".into(), json!(super::new_page_id()));
            write_file(&root.join(format!("Archives/copie{i}/{name}")), &copy.render()).unwrap();
        }
    }
    let month = uploads.join("2026-11");
    std::fs::create_dir_all(&month).unwrap();
    for i in 0..documents {
        std::fs::write(month.join(format!("{:012x}-Facture du mois {i}.txt", i)), format!("Facture {i}\nMerci")).unwrap();
    }
    let median = |work: &dyn Fn()| -> f64 {
        let mut runs: Vec<f64> = (0..3)
            .map(|_| {
                let t = Instant::now();
                work();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (runs[1] * 10.0).round() / 10.0
    };
    let langs = vec![lang.to_string()];
    let listed = median(&|| {
        std::hint::black_box(super::documents(&config, Part::Main));
    });
    let made = median(&|| {
        std::hint::black_box(super::index::rebuilt_for_bench(&config, &langs));
    });
    // what `documents()` did before (review 1 of S3, R3): each page's text folded again for each document
    let pages_read = super::all_pages(&config, Part::Main);
    let before = median(&|| {
        for _ in 0..documents {
            for (_, p) in &pages_read {
                std::hint::black_box(fold(&p.body));
            }
        }
    });
    let today_search = median(&|| {
        std::hint::black_box(personal(&config, &cat, Part::Main, "devis cuisine"));
    });
    let pages = super::all_pages(&config, Part::Main).len();
    json!({"pages": pages, "documents": documents, "documents_listed_ms": listed, "copy_made_again_ms": made,
           "before_each_page_folded_for_each_document_ms": before,
           "a_1_0_search_ms": today_search,
           "note": "a 1.0 app's search makes no copy (review 1 of S3, R3): its cost is today's search only"})
}

// -- the categories (brick S1) -----------------------------------------------------------------------------------------

/// The language's demo documents (and the health part's) as GET /v1/notebook/documents lists them, by bench id.
fn demo_documents(lang: &str) -> Vec<(String, Value, Part)> {
    let b = bench();
    let demo = if lang == "nl" { b["fixtures"]["nl"]["personal"].clone() } else { read("App/Resources/Demo/notebook-demo.json")[lang].clone() };
    let keys: Vec<String> = b["documents"].as_array().unwrap().iter().map(s).collect();
    let mut out = Vec::new();
    let all = demo["documents"].as_array().into_iter().flatten().map(|d| (d, Part::Main));
    let health = demo["health_documents"].as_array().into_iter().flatten().map(|d| (d, Part::Health));
    for (i, (d, part)) in all.chain(health).enumerate() {
        let subjects: Vec<Value> = d["subjects"].as_array().into_iter().flatten().map(|p| json!({"path": p})).collect();
        let linked = d["linked"].as_str().map(|p| json!({"path": p})).unwrap_or(Value::Null);
        let view = json!({"agent": d["agent"], "file": d["name"], "name": d["name"], "subjects": subjects, "linked": linked,
                          "part": part.as_str()});
        out.push((format!("doc:{}", keys.get(i).cloned().unwrap_or_default()), view, part));
    }
    out
}

/// The bridge's categories of every item of the language's demo (pages, lines, tasks, records, documents), by bench id.
fn bridge_categories(config: &Config, cat: &Catalogue, lang: &str, today: NaiveDate) -> BTreeMap<String, Vec<String>> {
    use super::categories as c;
    let mut out = BTreeMap::new();
    let main = super::all_pages(config, Part::Main);
    let health = super::all_pages(config, Part::Health);
    let marks = Dict::new();
    let ctx = c::Ctx::new(config);
    let page_of = |path: &str| -> Option<(Part, &(String, super::Page))> {
        main.iter().find(|(p, _)| p == path).map(|x| (Part::Main, x)).or_else(|| health.iter().find(|(p, _)| p == path).map(|x| (Part::Health, x)))
    };
    let mut filed: HashMap<String, Vec<String>> = HashMap::new();
    for (path, id) in &cat.pages {
        if let Some((part, (path, page))) = page_of(path) {
            let cats = c::of_page_in(&ctx, part, path, page).categories;
            filed.insert(path.clone(), cats.clone());
            out.insert(id.clone(), cats);
        }
    }
    for (id, path, _) in &cat.facts {
        let page = filed.get(path).cloned().unwrap_or_default();
        let text = cat.texts.get(id).cloned().unwrap_or_default();
        out.insert(id.clone(), c::with_dates(&page, c::dated(&text, None, today)));
    }
    let tasks = super::tasks(config, Part::Main);
    for (id, title) in &cat.tasks {
        // its title as written, its page's link after it (« … [[Rénovation cuisine]] »)
        if let Some(t) = tasks.iter().find(|t| fold(&t.title).starts_with(title.as_str())) {
            out.insert(id.clone(), c::of_task(&ctx, t, &main, &marks, today));
        }
    }
    for t in &records::TYPES {
        for r in records::records(config, t).into_iter().chain(records::archived_records(config, t)) {
            let key = ["item", "service", "what", "document"].iter().find_map(|k| r.get(*k).map(|v| fold(&s(v)))).unwrap_or_default();
            if let Some((id, _, _)) = cat.records.iter().find(|(_, _, k)| *k == key) {
                let (cats, _, dated) = c::of_record(t, &r, &marks, today);
                out.insert(id.clone(), c::with_dates(&cats, dated));
            }
        }
    }
    for (id, doc, part) in demo_documents(lang) {
        let pages = if part == Part::Health { &health } else { &main };
        let cats = if part == Part::Health { vec!["health".to_string()] } else { c::of_document(&ctx, &doc, pages, &marks).0 };
        out.insert(id, cats);
    }
    out
}

#[derive(Default)]
struct Filing {
    n: usize,
    /// the bench's first category among the bridge's
    right: usize,
    /// the bridge's first category is the bench's (Dates aside; carnet-categories, 22/11)
    first: usize,
    /// « Autres » alone where the bench has a real category
    other_on_real: usize,
    real: usize,
    /// the same categories (Dates aside)
    exact: usize,
    /// Dates where the bench has it (of those with an offset in the bench)
    dated: usize,
    dated_right: usize,
    misses: Vec<Value>,
}

impl Filing {
    fn view(&self) -> Value {
        let pct = |x: usize, n: usize| if n == 0 { Value::Null } else { json!((1000.0 * x as f64 / n as f64).round() / 10.0) };
        json!({"n": self.n, "right": pct(self.right, self.n), "first": pct(self.first, self.n),
               "other_on_real": pct(self.other_on_real, self.real), "exact": pct(self.exact, self.n),
               "dates_found": pct(self.dated_right, self.dated), "misses": self.misses})
    }
}

/// The bridge's categories against the bench's (`items`), for the personal Carnet's items (the family's are S6's, the
/// connected drives' S3's).
fn filing(config: &Config, cat: &Catalogue, lang: &str, today: NaiveDate) -> (Filing, BTreeMap<String, Vec<String>>) {
    let b = bench();
    let items = b["items"].as_object().unwrap();
    let got = bridge_categories(config, cat, lang, today);
    let mut f = Filing::default();
    for (id, item) in items {
        if id.starts_with("fam:") || id.starts_with("drive:") {
            continue;
        }
        let want: Vec<String> = item["categories"].as_array().unwrap().iter().map(s).collect();
        let Some(have) = got.get(id) else {
            f.n += 1;
            f.misses.push(json!({"id": id, "want": want, "got": null}));
            continue;
        };
        f.n += 1;
        let main = want.iter().find(|c| *c != "dates").cloned().unwrap_or_else(|| want[0].clone());
        let right = have.contains(&main);
        let plain = |l: &[String]| { let mut v: Vec<String> = l.iter().filter(|c| *c != "dates").cloned().collect(); v.sort(); v };
        f.right += usize::from(right);
        let first = have.iter().find(|c| *c != "dates");
        f.first += usize::from(first == Some(&main));
        if main != "other" && main != "health" {
            f.real += 1;
            f.other_on_real += usize::from(first.map(String::as_str) == Some("other"));
        }
        f.exact += usize::from(plain(&want) == plain(have));
        if item.get("offset").is_some() && want.contains(&"dates".to_string()) {
            f.dated += 1;
            f.dated_right += usize::from(have.contains(&"dates".to_string()));
        }
        if !right {
            f.misses.push(json!({"id": id, "want": want, "got": have}));
        }
    }
    (f, got)
}

#[test]
fn the_demo_is_filed_in_its_categories() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
    for lang in LANGS {
        let dir = tempfile::tempdir().unwrap();
        let config = super::tests_config(dir.path());
        let cat = write_demo(&config, lang, today);
        let (f, _) = filing(&config, &cat, lang, today);
        let right = f.right as f64 / f.n as f64;
        assert!(right >= 0.9, "{lang}: {:.1} % in the right category, {:?}", 100.0 * right, f.misses);
    }
}

fn report() -> Value {
    let b = bench();
    let items = b["items"].as_object().unwrap().clone();
    let today = chrono::Local::now().date_naive();
    let mut all = Tally::default();
    let mut by: BTreeMap<String, Tally> = BTreeMap::new();
    let mut filed_by: BTreeMap<String, Value> = BTreeMap::new();
    // « mauvaise catégorie en tête » with the bridge's own categories: before S1 (none: every result « Autres »), after
    let (mut with_cat, mut wrong_none, mut wrong_bridge) = (0usize, 0usize, 0usize);
    // the result that answers, under the category the person means (what S3's groups will show)
    let (mut answered, mut answer_right) = (0usize, 0usize);
    let mut misses: Vec<Value> = Vec::new();
    let mut sizes_by: BTreeMap<String, Value> = BTreeMap::new();
    let (mut skipped, mut refused_ok, mut refused_n, mut health_leaks, mut health_lines) = (0, 0, 0, 0, 0);
    let mut warnings: Vec<String> = Vec::new();
    let (mut s3_all, mut s3_by) = (Tally::default(), BTreeMap::<String, Tally>::new());
    let mut s3_misses: Vec<Value> = Vec::new();
    let (mut s3_refused_ok, mut s3_refused_n, mut s3_health_leaks) = (0, 0, 0);
    for lang in LANGS {
        let dir = tempfile::tempdir().unwrap();
        let config = super::tests_config(dir.path());
        let cat = write_demo(&config, lang, today);
        warnings.extend(cat.warnings.iter().map(|w| format!("{lang}: {w}")));
        let (f, bridge) = filing(&config, &cat, lang, today);
        let mut view = f.view();
        // what reading every item's categories costs (no page has them written yet: all computed, as before S2)
        let mut runs: Vec<f64> = (0..5)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(bridge_categories(&config, &cat, lang, today));
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        view["all_items_ms"] = json!((runs[2] * 100.0).round() / 100.0);
        view["items"] = json!(bridge.len());
        filed_by.insert(lang.into(), view);
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
        let som = sommaire(&config, &cat, lang, today, &fam);
        for case in b["cases"].as_array().unwrap().iter().filter(|c| c["lang"] == lang) {
            // brick S3: the same case on the sommaire's search
            let mut s3_times: Vec<f64> = Vec::new();
            let mut s3_shown = Vec::new();
            for _ in 0..RUNS {
                let t = Instant::now();
                s3_shown = sommaire_shows(&som, case, today);
                s3_times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            s3_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if case["health"].as_bool() != Some(true) && s3_shown.iter().any(|r| r.covers.iter().any(|c| c.contains("health"))) {
                s3_health_leaks += 1;
            }
            if let Some(never) = case["never"].as_array() {
                s3_refused_n += 1;
                if !s3_shown.iter().any(|r| never.iter().any(|n| r.covers.contains(&s(n)))) {
                    s3_refused_ok += 1;
                }
            } else {
                let want = expected(case, &items, today);
                if !want.is_empty() {
                    let rank = s3_shown.iter().position(|r| r.covers.iter().any(|c| want.contains(c)));
                    let category = case["category"].as_str();
                    let top_cats: Vec<String> = s3_shown
                        .first()
                        .and_then(|r| items.get(&r.primary))
                        .map(|i| i["categories"].as_array().unwrap().iter().map(s).collect())
                        .unwrap_or_else(|| if s3_shown.is_empty() { vec![] } else { vec!["other".into()] });
                    let wrong = category.is_some() && !s3_shown.is_empty() && !top_cats.iter().any(|c| Some(c.as_str()) == category);
                    let carnet = if case["carnet"] == "family" { "family" } else { "personal" };
                    let keys = [format!("lang:{lang}"), format!("type:{}", s(&case["type"])), format!("carnet:{carnet}"), format!("{carnet}:{lang}")];
                    let count = |t: &mut Tally| {
                        t.n += 1;
                        t.hit1 += usize::from(rank == Some(0));
                        t.hit2 += usize::from(rank.is_some_and(|r| r < 2));
                        t.hit5 += usize::from(rank.is_some_and(|r| r < 5));
                        t.with_category += usize::from(category.is_some() && !s3_shown.is_empty());
                        t.wrong_category += usize::from(wrong);
                        t.empty += usize::from(s3_shown.is_empty());
                        t.ms.push(s3_times[RUNS / 2]);
                    };
                    count(&mut s3_all);
                    for k in keys {
                        count(s3_by.entry(k).or_default());
                    }
                    if rank != Some(0) {
                        s3_misses.push(json!({"id": case["id"], "type": case["type"], "q": s(&case["q"]), "want": want, "rank": rank,
                                              "results": s3_shown.len(), "top": s3_shown.iter().take(3).map(|r| r.primary.clone()).collect::<Vec<_>>()}));
                    }
                }
            }
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
            if !is_family {
                if let (Some(category), Some(top)) = (category, shown.first()) {
                    with_cat += 1;
                    wrong_none += usize::from(category != "other");
                    let cats = bridge.get(&top.primary).cloned().unwrap_or_else(|| vec!["other".into()]);
                    wrong_bridge += usize::from(!cats.iter().any(|c| c == category));
                }
                if let Some(category) = category {
                    let firsts: Vec<&String> = want.iter().filter(|w| bridge.contains_key(*w)).collect();
                    if !firsts.is_empty() {
                        answered += 1;
                        answer_right += usize::from(firsts.iter().any(|w| bridge[*w].iter().any(|c| c == category)));
                    }
                }
            }
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
                t.hit2 += usize::from(rank.is_some_and(|r| r < 2));
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
        "categories": {
            "filed": filed_by,
            "wrong_category_top_personal": {
                "n": with_cat,
                "bench_categories_s0": by.get("carnet:personal").map(|t| t.view()["wrong_category"].clone()).unwrap_or(Value::Null),
                "bridge_before_s1": (1000.0 * wrong_none as f64 / with_cat.max(1) as f64).round() / 10.0,
                "bridge_after_s1": (1000.0 * wrong_bridge as f64 / with_cat.max(1) as f64).round() / 10.0,
            },
            "answer_in_meant_category": {"n": answered, "after_s1": (1000.0 * answer_right as f64 / answered.max(1) as f64).round() / 10.0,
                                          "before_s1": "0 (no categories: everything « Autres »)"},
        },
        "drives_refused_kept_out": format!("{refused_ok}/{refused_n}"),
        "health_leaks": health_leaks,
        "health_looking_lines_in_main_results": health_lines,
        "sizes": sizes_by,
        "scale": [at_scale("fr", today, &fr_queries, 5_000), at_scale("fr", today, &fr_queries, 20_000)],
        "scale_s3_20000_items": at_scale_s3("fr", today, &fr_queries, 20_000),
        "documents_500": with_documents("fr", today, 500),
        "s3": {
            "note": "brick S3: the same cases on the sommaire's search (index.rs + finder.rs: the copy built by the bridge, the phones' rule); a line is its own result (it no longer counts for its page); `drive` cases: the block of the online files the person allowed (their service's search simulated by the start of a word); time: the search on the copy in memory",
            "all": s3_all.view(),
            "by": s3_by.iter().map(|(k, t)| (k.clone(), t.view())).collect::<serde_json::Map<String, Value>>(),
            "drives_refused_kept_out": format!("{s3_refused_ok}/{s3_refused_n}"),
            "health_leaks": s3_health_leaks,
            "misses": s3_misses,
        },
        "warnings": warnings,
        "misses": misses,
    })
}

// -- brick S3: the sommaire's search (index.rs, finder.rs) -------------------------------------------------------------

/// The demo's documents as `documents()` lists them (the shared part's, with their kind and day), by bench id.
fn listed_documents(lang: &str, today: NaiveDate) -> Vec<(String, Value)> {
    let b = bench();
    let demo = if lang == "nl" { b["fixtures"]["nl"]["personal"].clone() } else { read("App/Resources/Demo/notebook-demo.json")[lang].clone() };
    let keys: Vec<String> = b["documents"].as_array().unwrap().iter().map(s).collect();
    let mut out = Vec::new();
    for (i, d) in demo["documents"].as_array().into_iter().flatten().enumerate() {
        let subjects: Vec<Value> = d["subjects"].as_array().into_iter().flatten().map(|p| json!({"path": p})).collect();
        let linked = d["linked"].as_str().map(|p| json!({"path": p})).unwrap_or(Value::Null);
        let page = d["page"].as_str().map(|p| json!(p)).unwrap_or(Value::Null);
        let file = if d["type"] == "page" { format!("{}.html", s(&d["page"])) } else { s(&d["name"]) };
        let day = today - Duration::days(d["days_ago"].as_i64().unwrap_or(0));
        let view = json!({"agent": d["agent"], "agent_name": d["agent"], "file": file, "name": d["name"], "type": d["type"],
                          "page": page, "subjects": subjects, "linked": linked, "date": format!("{day}T09:00:00+00:00")});
        out.push((format!("doc:{}", keys.get(i).cloned().unwrap_or_default()), view));
    }
    out
}

/// The family Carnet's copy as the phone keeps it (the fake bridge's way, until S6), its categories the bench's.
fn family_entries(data: &Value, today: NaiveDate) -> Vec<super::finder::Entry> {
    use super::finder::Entry;
    let b = bench();
    let reference = b["items"].as_object().unwrap();
    let cats = |id: &str| -> Vec<String> {
        let c: Vec<String> = reference
            .get(id)
            .and_then(|r| r["categories"].as_array())
            .map(|c| c.iter().map(s).filter(|c| c != "dates").collect())
            .unwrap_or_default();
        if c.is_empty() { vec!["other".into()] } else { c }
    };
    let mut out = Vec::new();
    for list in data["lists"].as_array().into_iter().flatten() {
        for it in list["items"].as_array().into_iter().flatten() {
            let id = format!("fam:list:{}/{}", s(&list["id"]), s(&it["id"]));
            let day = it["due_in"].as_i64().map(|d| today + Duration::days(d));
            let (date, yearly) = match (day, it["born"].is_number()) {
                (Some(d), true) => (None, Some(d.format("%m-%d").to_string())),
                (Some(d), false) => (Some(d.to_string()), None),
                _ => (None, None),
            };
            out.push(Entry { categories: cats(&id), id, kind: "list_item".into(), title: s(&list["title"]), line: s(&it["text"]),
                             date, yearly, fold: fold(&s(&it["text"])), done: it["done"].as_bool().unwrap_or(false),
                             ..Default::default() });
        }
    }
    for p in data["pages"].as_array().into_iter().flatten() {
        let id = format!("fam:page:{}", s(&p["id"]));
        out.push(Entry { categories: cats(&id), id, kind: "page".into(), title: s(&p["title"]),
                         line: crate::tools_knowledge::head(&s(&p["text"]), 240).to_string(),
                         fold: fold(&format!("{} {}", s(&p["title"]), s(&p["text"]))), ..Default::default() });
    }
    out
}

/// The sommaire's search on a language's demo: the copy (and Santé's, the family's) ready, each index id → its bench id.
struct Sommaire {
    main: super::finder::Finder,
    health: super::finder::Finder,
    family: super::finder::Finder,
    ids: HashMap<String, String>,
    drives: Vec<(String, String, bool)>,
}

fn sommaire(config: &Config, cat: &Catalogue, lang: &str, today: NaiveDate, fam: &Value) -> Sommaire {
    use super::finder::Finder;
    let docs = listed_documents(lang, today);
    let views: Vec<Value> = docs.iter().map(|(_, v)| v.clone()).collect();
    let langs = vec![lang.to_string()];
    let tables = super::index::tables(&langs, today.year());
    let main = Finder::new(super::index::entries(config, &views), &tables);
    let health = super::index::health_finder(config, &langs);
    let family = Finder::new(family_entries(fam, today), &tables);
    let mut ids: HashMap<String, String> = HashMap::new();
    for f in [&main, &health] {
        for e in f.items.iter() {
            let bench_id = match e.kind.as_str() {
                "page" => e.path.as_ref().and_then(|p| cat.pages.get(p)).cloned(),
                "line" => {
                    let t = fold(&e.line);
                    cat.facts.iter().find(|(_, p, x)| Some(p) == e.path.as_ref() && *x == t).map(|(id, _, _)| id.clone())
                }
                "task" => {
                    let t = fold(&e.title);
                    cat.tasks.iter().find(|(_, x)| *x == t).map(|(id, _)| id.clone())
                }
                "record" => cat.records.iter().find(|(_, _, k)| !k.is_empty() && e.fold.contains(k.as_str())).map(|(id, _, _)| id.clone()),
                "document" => docs.iter().find(|(_, v)| super::categories::document_key(&s(&v["agent"]), &s(&v["file"])) == e.id).map(|(id, _)| id.clone()),
                _ => None,
            };
            if let Some(b) = bench_id {
                ids.insert(e.id.clone(), b);
            }
        }
    }
    let drives = bench()["fixtures"]["drives"][lang]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| (s(&d["id"]), fold(&format!("{} {}", s(&d["name"]), s(&d["text"]))), d["allowed"].as_bool() == Some(true)))
        .collect();
    Sommaire { main, health, family, ids, drives }
}

/// A case as the new search shows it: the copy's results in order (Santé's when open, the family's for its Carnet);
/// a `drive` case: the block « Dans tes fichiers en ligne » (the drives the person allowed, found by their words: the
/// services' own search, simulated here by the start of a word), which arrives on its own.
fn sommaire_shows(som: &Sommaire, case: &Value, today: NaiveDate) -> Vec<Shown> {
    let q = s(&case["q"]);
    if matches!(case["type"].as_str(), Some("drive" | "drive_refused")) {
        let query = som.main.query(&q, today);
        let words: Vec<&String> = query.terms.iter().flatten().flatten().filter(|w| w.len() >= 3).collect();
        return som
            .drives
            .iter()
            .filter(|(_, text, allowed)| *allowed && text.split(' ').any(|t| words.iter().any(|w| t.starts_with(w.as_str()))))
            .map(|(id, _, _)| Shown { primary: id.clone(), covers: vec![id.clone()], health_line: false })
            .collect();
    }
    let finder = if case["carnet"] == "family" {
        &som.family
    } else if case["health"].as_bool() == Some(true) {
        &som.health
    } else {
        &som.main
    };
    finder
        .search(&q, today, None, None)
        .order
        .iter()
        .take(50)
        .map(|at| {
            let e = &finder.items[*at];
            let id = som.ids.get(&e.id).cloned().unwrap_or_else(|| e.id.clone());
            Shown { primary: id.clone(), covers: vec![id], health_line: super::lexicon::is_health(&e.line) }
        })
        .collect()
}

/// The plan's bar for S3 (« banc ≥ 85 % (deux meilleurs justes) »), in `cargo test`: the right result among the two
/// first for 85 % of the bench's cases or more (80 % in each language), Santé never out of its lock, the drives the
/// person did not allow never searched.
#[test]
fn the_sommaire_search_passes_the_bench() {
    let b = bench();
    let items = b["items"].as_object().unwrap().clone();
    let today = chrono::Local::now().date_naive();
    let (mut n, mut two, mut leaks, mut refused) = (0usize, 0usize, 0usize, 0usize);
    let mut by_lang: Vec<String> = Vec::new();
    for lang in LANGS {
        let dir = tempfile::tempdir().unwrap();
        let config = super::tests_config(dir.path());
        let cat = write_demo(&config, lang, today);
        let fam = if lang == "nl" { b["fixtures"]["nl"]["family"].clone() } else { read("App/Resources/Demo/family-notebook-demo.json")[lang].clone() };
        let som = sommaire(&config, &cat, lang, today, &fam);
        let (mut ln, mut ltwo) = (0usize, 0usize);
        for case in b["cases"].as_array().unwrap().iter().filter(|c| c["lang"] == lang) {
            let shown = sommaire_shows(&som, case, today);
            if case["health"].as_bool() != Some(true) && shown.iter().any(|r| r.covers.iter().any(|c| c.contains("health"))) {
                leaks += 1;
            }
            if let Some(never) = case["never"].as_array() {
                refused += usize::from(shown.iter().any(|r| never.iter().any(|x| r.covers.contains(&s(x)))));
                continue;
            }
            let want = expected(case, &items, today);
            if want.is_empty() {
                continue;
            }
            ln += 1;
            ltwo += usize::from(shown.iter().take(2).any(|r| r.covers.iter().any(|c| want.contains(c))));
        }
        n += ln;
        two += ltwo;
        assert!(ltwo as f64 >= 0.8 * ln as f64, "{lang}: {ltwo}/{ln} among the two first");
        by_lang.push(format!("{lang} {ltwo}/{ln}"));
    }
    assert!(two as f64 >= 0.85 * n as f64, "{two}/{n} among the two first ({by_lang:?})");
    assert_eq!((leaks, refused), (0, 0), "Santé out of its lock, a drive not allowed searched");
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

/// The blind set (briefs/carnet-blind-set-1122.json, written without seeing the word table: 60 pages a language),
/// measured once: the bridge's categories for each personal page (a new subject page: its title and first line),
/// and the search, by queries made mechanically from each title (the title; its longest word with two letters side by
/// side swapped; its longest word's first five letters), against a Carnet of the 60 pages. Never tuned on.
///
///     BIPAGENTS_BLIND_SET=<file> cargo test --lib blind_set_report -- --ignored --nocapture
#[test]
#[ignore = "the blind set's report (BIPAGENTS_BLIND_SET=<file>)"]
fn blind_set_report() {
    let Ok(file) = std::env::var("BIPAGENTS_BLIND_SET") else { return };
    let set: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
    let mut out = serde_json::Map::new();
    let pct = |x: usize, n: usize| if n == 0 { Value::Null } else { json!((1000.0 * x as f64 / n as f64).round() / 10.0) };
    let (mut tn, mut tfirst, mut tright, mut texact) = (0, 0, 0, 0);
    let mut search_all: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for lang in LANGS {
        let dir = tempfile::tempdir().unwrap();
        let config = super::tests_config(dir.path());
        update_state(&config, |st| {
            st.insert("language".into(), json!(if lang == "nl" { "en" } else { lang }));
        })
        .unwrap();
        let brief = super::words(&super::language(&config)).brief;
        let (mut n, mut first, mut right, mut exact, mut other_wrong, mut real) = (0, 0, 0, 0, 0, 0);
        let (mut trap_n, mut trap_right) = (0, 0);
        let mut misses: Vec<Value> = Vec::new();
        let pages = set[lang].as_array().unwrap();
        let mut written: Vec<(String, String)> = Vec::new();
        for (i, p) in pages.iter().enumerate() {
            let title = s(&p["title"]);
            let mut page = Page::new("subject", &title);
            page.front.insert("id".into(), json!(format!("p{i:010x}")));
            if let Some(line) = p["first_line"].as_str() {
                page.body = format!("## {brief}\n{line}\n");
            }
            let path = format!("Sujets/{}.md", super::file_name(&title));
            write_file(&part_root(&config, Part::Main).join(&path), &page.render()).unwrap();
            written.push((format!("page:p{i:010x}"), title.clone()));
            if p["carnet"] != "personal" {
                continue;
            }
            let expected: Vec<String> = p["expected"].as_array().unwrap().iter().map(s).collect();
            let (got, _) = super::categories::decide(&config, Part::Main, &path, &page, None);
            n += 1;
            first += usize::from(got.first() == expected.first());
            right += usize::from(got.contains(&expected[0]));
            let mut a = got.clone();
            a.sort();
            let mut b = expected.clone();
            b.sort();
            exact += usize::from(a == b);
            if expected[0] != "other" {
                real += 1;
                other_wrong += usize::from(got == ["other"]);
            }
            if p["tricky"].as_bool() == Some(true) {
                trap_n += 1;
                trap_right += usize::from(got.contains(&expected[0]));
            }
            if !got.contains(&expected[0]) {
                misses.push(json!({"title": title, "expected": expected, "got": got, "tricky": p["tricky"]}));
            }
        }
        tn += n;
        tfirst += first;
        tright += right;
        texact += exact;
        // the search: each title, as typed in three ways
        let langs = vec![lang.to_string()];
        let finder = super::finder::Finder::new(super::index::entries(&config, &[]), &super::index::tables(&langs, today.year()));
        let mut search = serde_json::Map::new();
        for mode in ["title", "swap", "start"] {
            let (mut sn, mut s1, mut s2) = (0, 0, 0);
            for (id, title) in &written {
                let longest = fold(title).split(' ').max_by_key(|w| w.len()).unwrap_or_default().to_string();
                let q = match mode {
                    "title" => title.clone(),
                    "swap" if longest.len() >= 5 => {
                        let mut b: Vec<char> = longest.chars().collect();
                        let m = b.len() / 2;
                        b.swap(m - 1, m);
                        fold(title).replacen(&longest, &b.iter().collect::<String>(), 1)
                    }
                    "start" if longest.len() >= 6 => fold(title).replacen(&longest, &longest[..5], 1),
                    _ => continue,
                };
                let found: Vec<&str> = finder.search(&q, today, None, None).order.iter().map(|at| finder.items[*at].id.as_str()).collect();
                sn += 1;
                s1 += usize::from(found.first() == Some(&id.as_str()));
                s2 += usize::from(found.iter().take(2).any(|f| f == id));
            }
            let e = search_all.entry(mode).or_default();
            e.0 += sn;
            e.1 += s1;
            e.2 += s2;
            search.insert(mode.into(), json!({"n": sn, "hit1": pct(s1, sn), "hit2": pct(s2, sn)}));
        }
        out.insert(lang.into(), json!({"categories": {"n": n, "first_right": pct(first, n), "expected_among_the_two": pct(right, n),
                                        "exact": pct(exact, n), "autres_on_a_real_category": pct(other_wrong, real),
                                        "traps": format!("{trap_right}/{trap_n}"), "misses": misses},
                                       "search": search}));
    }
    out.insert("all".into(), json!({"categories": {"n": tn, "first_right": pct(tfirst, tn), "expected_among_the_two": pct(tright, tn),
                                                   "exact": pct(texact, tn)},
                                    "search": search_all.iter().map(|(k, (n, a, b))| (k.to_string(), json!({"n": n, "hit1": pct(*a, *n), "hit2": pct(*b, *n)}))).collect::<serde_json::Map<String, Value>>()}));
    println!("BLIND-SET {}", serde_json::to_string_pretty(&Value::Object(out)).unwrap());
}
