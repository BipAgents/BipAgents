//! Tests of summary.rs (brick S4): the sommaire, a category's page, the kept searches, the searches made.

use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use super::summary::{self as sm, Refused};
use crate::config::Config;
use crate::notebook::{self as nb, categories as c, Part};

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn demo(lang: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let config = nb::tests_config(dir.path());
    nb::search_bench::write_demo_for(&config, lang, today());
    (dir, config)
}

fn fr() -> Vec<String> {
    vec!["fr".to_string()]
}

fn now() -> f64 {
    crate::py::time_time()
}

fn line<'a>(summary: &'a Value, id: &str) -> Option<&'a Value> {
    summary["categories"].as_array().unwrap().iter().find(|l| l["id"] == id)
}

/// The real answer has its contract type (contracts/carnet/sommaire.json `types`).
fn typed(v: &Value, ty: &str) {
    let contract = nb::sommaire_contract::contract();
    let types = contract["types"].as_object().unwrap();
    let mut errors = Vec::new();
    nb::sommaire_contract::check(v, &json!(ty), types, ty, &mut errors);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// What the copy counts in each category (`dates` as of today), each note once in `total`.
fn counted(config: &Config, space: &str) -> (usize, std::collections::HashMap<String, usize>) {
    let f = nb::index::finder(config, &fr());
    let mut counts = std::collections::HashMap::new();
    let mut total = 0;
    for at in 0..f.items.len() {
        if space != "all" && f.items[at].space() != space {
            continue;
        }
        total += 1;
        let mut seen = HashSet::new();
        for cat in f.categories(at, today()) {
            if seen.insert(cat.clone()) {
                *counts.entry(cat).or_insert(0) += 1;
            }
        }
    }
    (total, counts)
}

fn santé(config: &Config) {
    let private = nb::part_root(config, Part::Health).join("Domaines");
    std::fs::create_dir_all(&private).unwrap();
    std::fs::write(private.join("Santé.md"), "---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n- Prednisolone le matin — *Wellness, 2026-11-01*\n").unwrap();
}

#[test]
fn the_summary_is_ready_to_show() {
    let (_dir, config) = demo("fr");
    santé(&config);
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    typed(&s, "Summary");
    assert_eq!((s["v"].clone(), s["lang"].clone(), s["title"].clone(), s["space"].clone()), (json!(1), json!("fr"), json!("Carnet"), json!("all")));
    // no selector before S5 (Perso · Pro) and S6 (Famille): the app keeps its « Moi | Famille » of today
    assert_eq!(s["spaces"], json!([]));
    assert_eq!(s["kept"], json!([]));
    let (total, counts) = counted(&config, "all");
    assert_eq!(s["total"], json!({"count": total, "title": "Tout le Carnet"}));
    assert_eq!(s["bar"], json!({"placeholder": "Cherche dans tout le Carnet"}));
    // the categories in the sommaire's order, an empty one hidden, Santé last and locked (no count, no word)
    let ids: Vec<String> = s["categories"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap().to_string()).collect();
    let order: Vec<String> = c::PERSONAL.iter().map(|x| x.to_string()).filter(|x| ids.contains(x)).collect();
    assert_eq!(ids, order);
    assert_eq!(ids.last().map(String::as_str), Some("health"));
    assert_eq!(line(&s, "health").unwrap(), &json!({"id": "health", "title": "Santé", "count": null, "hint": "", "locked": true}));
    for l in s["categories"].as_array().unwrap().iter().filter(|l| l["id"] != "health") {
        let id = l["id"].as_str().unwrap();
        assert_eq!(l["count"], json!(counts[id]), "{id}");
        assert_eq!((l["locked"].clone(), l["title"].clone()), (json!(false), json!(c::title("fr", id))));
        // the grey words: the latest titles of the category (no search made yet), three at most
        let hint = l["hint"].as_str().unwrap();
        assert!(!hint.is_empty() && hint.split(", ").count() <= 3, "{id}: {hint}");
        assert!(l["first"]["categories"].as_array().unwrap().iter().any(|x| x == id), "{id}: {}", l["first"]);
    }
    for id in counts.keys().filter(|k| *k != "health") {
        assert!(ids.contains(id), "{id} not shown");
    }
    // « Toi » is the first line of « Personnes »
    let people = line(&s, "people").unwrap();
    assert_eq!((people["first"]["kind"].clone(), people["first"]["title"].clone()), (json!("page"), json!("Toi")));
    // the copy's version: the app knows whether its copy is the one counted
    let f = nb::index::answer(&config, &fr(), None, None, "").0;
    assert_eq!(s["version"], f["version"]);
    // with the copy's epoch: a version number alone means nothing after the bridge started again (review 1, R4)
    assert_eq!(s["epoch"], f["epoch"]);
    // Santé: never a count, a word or an item outside its lock
    assert!(!s.to_string().contains("Prednisolone"));
    // in the app's language (titles and texts), the Carnet's notes as they are
    let en = sm::summary_at(&config, "all", &["fr".to_string(), "en".to_string()], today(), now());
    assert_eq!((en["lang"].clone(), en["title"].clone()), (json!("en"), json!("Notebook")));
    assert_eq!(line(&en, "people").unwrap()["title"], json!("People"));
    assert_eq!(en["total"]["count"], s["total"]["count"]);
}

#[test]
fn the_summary_takes_well_under_100_ms_once_the_copy_is_made() {
    let (_dir, config) = demo("fr");
    sm::summary_at(&config, "all", &fr(), today(), now());
    let t = std::time::Instant::now();
    for _ in 0..5 {
        std::hint::black_box(sm::summary_at(&config, "all", &fr(), today(), now()));
    }
    // (the demo; 20 000 lines: `summary_at_scale`, ignored, in the status)
    assert!(t.elapsed().as_millis() < 5 * 100, "{:?}", t.elapsed());
}

#[test]
fn a_space_before_s5() {
    let (_dir, config) = demo("fr");
    // no page is Pro before S5: « perso » is the whole Carnet, « pro » nothing
    let all = sm::summary_at(&config, "all", &fr(), today(), now());
    let perso = sm::summary_at(&config, "perso", &fr(), today(), now());
    assert_eq!(perso["space"], json!("perso"));
    assert_eq!((perso["total"].clone(), perso["categories"].clone()), (all["total"].clone(), all["categories"].clone()));
    let pro = sm::summary_at(&config, "pro", &fr(), today(), now());
    typed(&pro, "Summary");
    assert_eq!((pro["total"]["count"].clone(), pro["categories"].clone(), pro["card"].clone()), (json!(0), json!([]), Value::Null));
    for asked in [None, Some(""), Some("all"), Some("perso"), Some("pro")] {
        assert!(sm::space_asked(asked).is_ok(), "{asked:?}");
    }
    for asked in ["family", "Pro", "x"] {
        assert_eq!(sm::space_asked(Some(asked)), Err(Refused::SpaceUnknown));
    }
}

#[test]
fn a_category_page_has_its_filters_its_searches_and_the_latest() {
    let (_dir, config) = demo("fr");
    santé(&config);
    let page = |id: &str, filter: Option<&str>| sm::category_at(&config, id, "all", filter, &fr(), false, today());
    let papers = page("papers", None).unwrap();
    typed(&papers, "CategoryPage");
    let (_, counts) = counted(&config, "all");
    assert_eq!((papers["id"].clone(), papers["title"].clone(), papers["count"].clone()), (json!("papers"), json!("Papiers"), json!(counts["papers"])));
    assert_eq!((papers["filter"].clone(), papers["searched"].clone()), (Value::Null, json!([])));
    assert_eq!(papers["bar"], json!({"placeholder": "Cherche dans Papiers"}));
    let filters: Vec<(String, String)> = papers["filters"].as_array().unwrap().iter()
        .map(|f| (f["id"].as_str().unwrap().into(), f["title"].as_str().unwrap().into())).collect();
    assert_eq!(filters, [("invoices", "Factures"), ("contracts", "Contrats"), ("warranties", "Garanties")]
        .map(|(a, b)| (a.to_string(), b.to_string())));
    // the latest: the category's own, at most 20, the newest noted first
    let recent = papers["recent"].as_array().unwrap();
    assert!(!recent.is_empty() && recent.len() <= sm::RECENT);
    assert!(recent.iter().all(|i| i["categories"].as_array().unwrap().iter().any(|x| x == "papers")));
    let noted: Vec<String> = recent.iter().filter_map(|i| i["noted"].as_str().map(str::to_string)).collect();
    assert!(noted.windows(2).all(|w| w[0] >= w[1]), "{noted:?}");
    // each sub-filter: its count is what it lists (all of it under 20), each of them in the category
    for id in ["papers", "money", "dates", "people"] {
        let whole = page(id, None).unwrap();
        for f in whole["filters"].as_array().unwrap() {
            let fid = f["id"].as_str().unwrap();
            let narrowed = page(id, Some(fid)).unwrap();
            assert_eq!(narrowed["filter"], json!(fid));
            assert_eq!(narrowed["count"], whole["count"]);
            let listed = narrowed["recent"].as_array().unwrap();
            assert_eq!(json!(listed.len().min(sm::RECENT)), json!(f["count"].as_u64().unwrap().min(sm::RECENT as u64)), "{id}/{fid}");
            assert!(listed.iter().all(|i| i["categories"].as_array().unwrap().iter().any(|x| x == id)), "{id}/{fid}");
        }
    }
    // the demo's records: a contract, a purchase with its warranty
    let count = |p: &Value, fid: &str| p["filters"].as_array().unwrap().iter().find(|f| f["id"] == fid).unwrap()["count"].as_u64().unwrap();
    assert!(count(&papers, "contracts") >= 1 && count(&papers, "warranties") >= 1, "{}", papers["filters"]);
    let money = page("money", None).unwrap();
    assert!(count(&money, "subscriptions") >= 1 && count(&money, "purchases") >= 1, "{}", money["filters"]);
    // Dates: what is still to come, this week (to Sunday), this month (to its last day), later
    let dates = page("dates", None).unwrap();
    let f = nb::index::finder(&config, &fr());
    let t = today();
    let sunday = t + Duration::days(6 - t.weekday().num_days_from_monday() as i64);
    let month_end = NaiveDate::from_ymd_opt(t.year() + i32::from(t.month() == 12), t.month() % 12 + 1, 1).unwrap() - Duration::days(1);
    let days: Vec<NaiveDate> = (0..f.items.len())
        .filter(|at| f.categories(*at, t).iter().any(|x| x == "dates"))
        .filter_map(|at| f.day(at, t))
        .filter(|d| *d >= t)
        .collect();
    assert_eq!(count(&dates, "this_week") as usize, days.iter().filter(|d| **d <= sunday).count());
    assert_eq!(count(&dates, "this_month") as usize, days.iter().filter(|d| **d <= month_end).count());
    assert_eq!(count(&dates, "later") as usize, days.iter().filter(|d| **d > month_end).count());
    let coming = page("dates", Some("later")).unwrap();
    let listed: Vec<&str> = coming["recent"].as_array().unwrap().iter().filter_map(|i| i["date"].as_str()).collect();
    assert!(listed.windows(2).all(|w| w[0] <= w[1]), "the soonest first: {listed:?}");
    // Personnes: the close ones (marked « proche », and « Toi ») and the others, each note in one of them
    let people = page("people", None).unwrap();
    assert_eq!(count(&people, "close") + count(&people, "others"), people["count"].as_u64().unwrap());
    assert!(count(&people, "close") >= 1);
    // categories without sub-filters: none
    assert_eq!(page("home", None).unwrap()["filters"], json!([]));
    // refused: a sub-filter of another category, an unknown category, Santé without its lock
    assert_eq!(page("papers", Some("to_pay")), Err(Refused::FilterUnknown));
    assert_eq!(page("home", Some("x")), Err(Refused::FilterUnknown));
    assert_eq!(page("gens", None), Err(Refused::CategoryUnknown));
    assert_eq!(page("health", None), Err(Refused::HealthLocked));
    // Santé behind the lock: read now, kept nowhere, no sub-filter, no search kept
    let health = sm::category_at(&config, "health", "all", None, &fr(), true, today()).unwrap();
    typed(&health, "CategoryPage");
    assert_eq!((health["filters"].clone(), health["searched"].clone()), (json!([]), json!([])));
    let lines: Vec<&str> = health["recent"].as_array().unwrap().iter().map(|i| i["line"].as_str().unwrap()).collect();
    assert!(lines.contains(&"Prednisolone le matin"), "{lines:?}");
    assert!(health["recent"].as_array().unwrap().iter().all(|i| i["part"] == "health"));
    // Pro before S5: nothing
    let pro = sm::category_at(&config, "papers", "pro", None, &fr(), false, today()).unwrap();
    assert_eq!((pro["count"].clone(), pro["recent"].clone()), (json!(0), json!([])));
}

#[test]
fn kept_searches_six_a_space_with_their_count() {
    let (_dir, config) = demo("fr");
    let f = nb::index::finder(&config, &fr());
    let found = |q: &str, cat: Option<&str>| f.search(q, today(), cat, None).counts["all"];
    let listed = |space: &str| sm::kept_at(&config, space, &fr(), today());
    assert_eq!(listed("all"), json!({"kept": [], "max": 6}));
    let space = sm::keep(&config, "anniversaire", None, "all", None).unwrap();
    assert_eq!(space, "all");
    sm::keep(&config, "à payer ce mois-ci", None, "all", Some("money")).unwrap();
    let kept = listed("all");
    typed(&kept, "KeptList");
    let list = kept["kept"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!((list[0]["q"].clone(), list[0]["title"].clone(), list[0]["space"].clone()), (json!("anniversaire"), json!("anniversaire"), Value::Null));
    assert!(list[0]["id"].as_str().unwrap().starts_with('k'));
    assert_eq!(list[0]["count"], json!(found("anniversaire", None)));
    assert!(list[0]["count"].as_u64().unwrap() >= 2);
    // the words and the date phrase, counted again each day
    assert_eq!((list[1]["category"].clone(), list[1]["count"].clone()), (json!("money"), json!(found("à payer ce mois-ci", Some("money")))));
    // the same search again: kept once
    sm::keep(&config, "  Anniversaire ", None, "all", None).unwrap();
    assert_eq!(listed("all")["kept"].as_array().unwrap().len(), 2);
    // renamed: its title, the same words
    let id = list[0]["id"].as_str().unwrap().to_string();
    sm::rename(&config, &id, "Les anniversaires").unwrap();
    let again = listed("all");
    assert_eq!((again["kept"][0]["title"].clone(), again["kept"][0]["q"].clone()), (json!("Les anniversaires"), json!("anniversaire")));
    // the summary shows them, with their counts
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert_eq!(s["kept"], again["kept"]);
    // six a space
    for q in ["voyage", "maison", "impôts", "marc"] {
        sm::keep(&config, q, None, "all", None).unwrap();
    }
    assert_eq!(sm::keep(&config, "claire", None, "all", None), Err(Refused::KeptFull));
    // another space has its own six (before S5: Pro counts nothing)
    sm::keep(&config, "claire", Some("Claire"), "pro", None).unwrap();
    let pro = listed("pro");
    assert_eq!((pro["kept"][0]["space"].clone(), pro["kept"][0]["count"].clone(), pro["kept"][0]["title"].clone()), (json!("pro"), json!(0), json!("Claire")));
    assert_eq!(listed("all")["kept"].as_array().unwrap().len(), 6);
    // deleted
    assert_eq!(sm::drop_kept(&config, &id).unwrap(), "all");
    assert_eq!(listed("all")["kept"].as_array().unwrap().len(), 5);
    assert_eq!(sm::drop_kept(&config, &id), Err(Refused::KeptUnknown));
    assert_eq!(sm::rename(&config, "k000000", "x"), Err(Refused::KeptUnknown));
    // refused: nothing to search, a number or a reference (never kept, never in the phone's copy), Santé, a category
    // that does not exist
    assert_eq!(sm::keep(&config, "   ", None, "all", None), Err(Refused::KeptEmpty));
    assert_eq!(sm::keep(&config, "dossier HR-55821", None, "all", None), Err(Refused::NumberRefused));
    assert_eq!(sm::keep(&config, "FR76 3000 6000 0112 3456 7890 189", None, "all", None), Err(Refused::NumberRefused));
    assert_eq!(sm::keep(&config, "allergie", None, "all", Some("health")), Err(Refused::CategoryNotGiven));
    assert_eq!(sm::keep(&config, "x", None, "all", Some("gens")), Err(Refused::CategoryUnknown));
    // kept in state.json, the keys only the contract names
    let state = nb::read_state(&config);
    let kept = state["kept_searches"].as_array().unwrap();
    assert_eq!(kept.len(), 6);
    assert!(kept.iter().all(|k| k["id"].is_string() && k["q"].is_string() && k["space"].is_string()));
}

#[test]
fn the_phones_copy_carries_the_kept_searches_and_moves_with_them() {
    let (_dir, config) = demo("fr");
    let (first, etag) = nb::index::answer(&config, &fr(), None, None, "");
    assert_eq!(first["kept"], json!([]));
    sm::keep(&config, "à payer ce mois-ci", None, "all", Some("money")).unwrap();
    let (after, moved) = nb::index::answer(&config, &fr(), Some(first["version"].as_u64().unwrap()), first["epoch"].as_str(), "");
    assert_ne!(moved, etag);
    assert_eq!(after["version"].as_u64().unwrap(), first["version"].as_u64().unwrap() + 1);
    // a diff: no item changed, the kept searches whole (as the tables)
    assert_eq!((after["full"].clone(), after["items"].clone(), after["removed"].clone()), (json!(false), json!([]), json!([])));
    let kept = after["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["q"], json!("à payer ce mois-ci"));
    assert_eq!(kept[0]["category"], json!("money"));
    assert!(kept[0].get("count").is_none() && kept[0].get("space").is_none(), "{}", kept[0]);
    typed(&after, "Index");
    // another space's: its `space` (review 1, R8)
    sm::keep(&config, "claire", None, "pro", None).unwrap();
    let (again, _) = nb::index::answer(&config, &fr(), None, None, "");
    assert_eq!(again["kept"][1], json!({"id": again["kept"][1]["id"], "q": "claire", "title": "claire", "space": "pro"}));
    // renamed to a title holding a number or a reference: refused (never on the phone)
    assert_eq!(sm::rename(&config, &kept_id(&config, "claire"), "Dossier HR-55821"), Err(Refused::NumberRefused));
}

#[test]
fn a_search_made_feeds_searched_here_three_a_category() {
    let (_dir, config) = demo("fr");
    let etag = nb::index::etag(&config, &fr());
    let made = |q: &str, cat: Option<&str>| sm::searched_at(&config, q, cat, "all", &fr(), today()).unwrap();
    assert!(made("garantie lave-linge", Some("papers")));
    let papers = |config: &Config| sm::category_at(config, "papers", "all", None, &fr(), false, today()).unwrap();
    let searched = papers(&config)["searched"].clone();
    assert_eq!(searched.as_array().unwrap().len(), 1);
    assert_eq!(searched[0]["q"], json!("garantie lave-linge"));
    assert!(searched[0]["at"].as_str().unwrap().ends_with('Z'));
    for q in ["contrat", "passeport", "assurance"] {
        made(q, Some("papers"));
    }
    // three, the newest first; the same search again comes back on top, once
    made("Contrat", Some("papers"));
    let qs: Vec<String> = papers(&config)["searched"].as_array().unwrap().iter().map(|s| s["q"].as_str().unwrap().to_string()).collect();
    assert_eq!(qs, ["Contrat", "assurance", "passeport"]);
    // the sommaire's grey words: the category's latest searches first
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert_eq!(line(&s, "papers").unwrap()["hint"], json!("Contrat, assurance, passeport"));
    // without a category: under the category of its best result
    assert!(made("anniversaire de Claire", None));
    let people = sm::category_at(&config, "people", "all", None, &fr(), false, today()).unwrap();
    assert_eq!(people["searched"][0]["q"], json!("anniversaire de Claire"));
    // never kept: what finds nothing (no category), Santé, a number or a reference, a letter
    assert!(!made("zzqqxxw", None));
    assert!(!made("allergie", Some("health")));
    assert!(!made("dossier HR-55821", Some("papers")));
    assert!(!made("a", Some("papers")));
    assert_eq!(sm::searched_at(&config, "x", Some("gens"), "all", &fr(), today()), Err(Refused::CategoryUnknown));
    // kept apart from state.json (its file only): the phone's copy is not made again for a search, its ETag stays
    assert_eq!(nb::index::etag(&config, &fr()), etag);
    let state = std::fs::read_to_string(nb::root(&config).join("state.json")).unwrap_or_default();
    assert!(!state.contains("garantie lave-linge"));
}

#[test]
fn nothing_searched_reaches_the_carnet_the_copy_or_an_agent() {
    let (dir, config) = demo("fr");
    assert!(sm::searched_at(&config, "marc qzxwvyk", Some("people"), "all", &fr(), today()).unwrap());
    let word = "qzxwvyk";
    // the person sees it (« Cherché ici », the grey words); nothing else holds it
    let people = sm::category_at(&config, "people", "all", None, &fr(), false, today()).unwrap();
    assert_eq!(people["searched"][0]["q"], json!("marc qzxwvyk"));
    let (copy, _) = nb::index::answer(&config, &fr(), None, None, "");
    assert!(!copy.to_string().contains(word));
    assert!(nb::tools::answers(&config, &[Part::Main], word).is_empty());
    assert!(nb::search(&config, &[Part::Main], word, "").is_empty());
    // on disk: in its own file only (never a page, the state, the history the agents' tools read)
    let mut holding: Vec<String> = Vec::new();
    let mut stack = vec![dir.path().to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).contains(word)).unwrap_or(false) {
                holding.push(path.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let file = sm::searched_path(&config);
    assert_eq!(holding, [file.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned()]);
}

#[test]
fn oublie_ca_takes_the_searches_with_it() {
    let (_dir, config) = demo("fr");
    // kept and made in every space: Marc's, and others
    for (q, space) in [("marc", "all"), ("anniversaire", "all"), ("Marc", "pro"), ("garantie lave-linge", "all"), ("Marcel", "all")] {
        sm::keep(&config, q, None, space, None).unwrap();
    }
    sm::rename(&config, &kept_id(&config, "anniversaire"), "Les anniversaires").unwrap();
    for (q, cat, space) in [("claire", "people", "all"), ("marc", "people", "all"), ("cadeau pour Marc", "people", "perso"),
                            ("garantie", "papers", "all"), ("marché", "money", "all"), ("MARC", "money", "all")] {
        assert!(sm::searched_at(&config, q, Some(cat), space, &fr(), today()).unwrap(), "{q}");
    }
    let (before, _) = nb::index::answer(&config, &fr(), None, None, "");
    // « Oublie Marc »: his page, forgotten whole
    let request = nb::forget::Request { page: Some("Marc Durand".into()), ..Default::default() };
    let target = nb::forget::target(&config, &request).unwrap();
    nb::forget::apply(&config, &target, "fr", "").unwrap();
    let words = |space: &str| -> Vec<String> {
        sm::kept_at(&config, space, &fr(), today())["kept"].as_array().unwrap().iter().map(|k| k["q"].as_str().unwrap().to_string()).collect()
    };
    // no kept « marc » in any space (accents and case folded as the search folds them); the others untouched
    // (whole words: « Marcel » and « marché » stay)
    assert_eq!(words("all"), ["anniversaire", "garantie lave-linge", "Marcel"]);
    assert_eq!(words("pro"), Vec::<String>::new());
    let searched = |cat: &str, space: &str| -> Vec<String> {
        sm::category_at(&config, cat, space, None, &fr(), false, today()).unwrap()["searched"].as_array().unwrap()
            .iter().map(|s| s["q"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(searched("people", "all"), ["claire"]);
    assert_eq!(searched("people", "perso"), Vec::<String>::new());
    assert_eq!(searched("money", "all"), ["marché"]);
    assert_eq!(searched("papers", "all"), ["garantie"]);
    // the grey words of Personnes: no « marc » left
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert_eq!(line(&s, "people").unwrap()["hint"], json!("claire"));
    assert!(!has_word(&s["kept"].to_string(), "marc"), "{}", s["kept"]);
    // the phone's copy: a new version, its kept searches without Marc's
    let (after, _) = nb::index::answer(&config, &fr(), Some(before["version"].as_u64().unwrap()), before["epoch"].as_str(), "");
    assert!(after["version"].as_u64().unwrap() > before["version"].as_u64().unwrap());
    let kept: Vec<&str> = after["kept"].as_array().unwrap().iter().map(|k| k["q"].as_str().unwrap()).collect();
    assert_eq!(kept, ["anniversaire", "garantie lave-linge", "Marcel"]);
    // nothing of it left on disk in the searches' file nor the state's
    let file = std::fs::read_to_string(sm::searched_path(&config)).unwrap();
    assert!(!has_word(&file, "marc"), "{file}");
    let state = nb::read_state(&config);
    assert!(!has_word(&state["kept_searches"].to_string(), "marc"));
    // a fact forgotten (no page): the searches holding its words, whole words only
    let request = nb::forget::Request { texts: vec!["lave-linge".into()], ..Default::default() };
    if let Ok(target) = nb::forget::target(&config, &request) {
        nb::forget::apply(&config, &target, "fr", "").unwrap();
    }
    assert_eq!(words("all"), ["anniversaire", "Marcel"]);
    assert_eq!(searched("papers", "all"), ["garantie"]);
}

/// Whether folded `text` holds `word` as a whole word.
fn has_word(text: &str, word: &str) -> bool {
    nb::fold(text).split(|c: char| !c.is_alphanumeric()).any(|w| w == word)
}

#[test]
fn a_short_first_name_never_wipes_the_searches() {
    // « La nounou », « Van Damme » forgotten whole: their first word is a stop word; « nounou » is the name (review 1,
    // R1)
    let (_dir, config) = demo("fr");
    for title in ["La nounou", "Van Damme"] {
        nb::new_page(&config, Part::Main, "person", title, &nb::Who::you(&config)).unwrap().unwrap();
    }
    for q in ["la plage", "van gogh", "nounou mercredi", "le marché"] {
        sm::keep(&config, q, None, "all", None).unwrap();
    }
    assert!(sm::searched_at(&config, "la cantine", Some("other"), "all", &fr(), today()).unwrap());
    for page in ["La nounou", "Van Damme"] {
        let request = nb::forget::Request { page: Some(page.into()), ..Default::default() };
        let target = nb::forget::target(&config, &request).unwrap();
        nb::forget::apply(&config, &target, "fr", "").unwrap();
    }
    let words: Vec<String> = sm::kept_at(&config, "all", &fr(), today())["kept"].as_array().unwrap().iter()
        .map(|k| k["q"].as_str().unwrap().to_string()).collect();
    assert_eq!(words, ["la plage", "van gogh", "le marché"]);
    let other = sm::category_at(&config, "other", "all", None, &fr(), false, today()).unwrap();
    assert_eq!(other["searched"][0]["q"], json!("la cantine"));
}

#[test]
fn a_search_made_can_be_dropped() {
    // one whose words are nowhere in the Carnet: « Oublie ça » has nothing to find, this drops it (review 1, B2)
    let (_dir, config) = demo("fr");
    for (q, cat, space) in [("avocat divorce", "papers", "all"), ("garantie", "papers", "all"), ("claire", "people", "all"),
                            ("avocat divorce", "papers", "pro")] {
        assert!(sm::searched_at(&config, q, Some(cat), space, &fr(), today()).unwrap(), "{q}");
    }
    let searched = |cat: &str, space: &str| -> Vec<String> {
        sm::category_at(&config, cat, space, None, &fr(), false, today()).unwrap()["searched"].as_array().unwrap()
            .iter().map(|s| s["q"].as_str().unwrap().to_string()).collect()
    };
    // one search (folded as the search folds), in one space
    assert_eq!(sm::drop_searched(&config, Some("Avocat  Divorce"), Some("all"), None).unwrap(), 1);
    assert_eq!((searched("papers", "all"), searched("papers", "pro")), (vec!["garantie".to_string()], vec!["avocat divorce".to_string()]));
    // in every space
    assert_eq!(sm::drop_searched(&config, Some("avocat divorce"), None, None).unwrap(), 1);
    assert_eq!(searched("papers", "pro"), Vec::<String>::new());
    // a category's
    assert_eq!(sm::drop_searched(&config, None, Some("all"), Some("papers")).unwrap(), 1);
    assert_eq!(searched("people", "all"), ["claire"]);
    // all of them
    assert_eq!(sm::drop_searched(&config, None, None, None).unwrap(), 1);
    assert_eq!(searched("people", "all"), Vec::<String>::new());
    assert_eq!(sm::drop_searched(&config, None, None, None).unwrap(), 0);
}

#[test]
fn the_moments_card_never_speaks_of_sante() {
    let (_dir, config) = demo("fr");
    santé(&config);
    // the misplaced points' card: never Santé's points, whatever their words (review 1, B1)
    nb::misplaced::detect(&config).unwrap();
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert!(!s.to_string().contains("Prednisolone"), "{}", s["card"]);
    // the question of the day about health: never on the sommaire (review 1, R2); another one is
    let day = nb::ask::day_of(now());
    let ask = |id: &str, question: &str| {
        json!({"id": id, "card": "ask", "at": now(), "day": day, "agent": "wellness", "session": "s1", "state": "open",
               "question": question, "chips": ["Oui", "Non"], "path": "Toi.md", "title": "Toi", "kind": "self",
               "section": "about", "label": "x"})
    };
    let health = ask("c1", "Ton allergie à la pénicilline, c'est toujours d'actualité ?");
    nb::update_state(&config, |st| {
        st.insert("cards".into(), json!([health.clone()]));
    })
    .unwrap();
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert!(s["card"].is_null() || s["card"]["kind"] != "ask", "{}", s["card"]);
    assert!(!s.to_string().contains("pénicilline"));
    nb::update_state(&config, |st| {
        st.insert("cards".into(), json!([ask("c2", "Quelle est ta pointure ?"), health.clone()]));
    })
    .unwrap();
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert_eq!((s["card"]["kind"].clone(), s["card"]["card"]["question"].clone()), (json!("ask"), json!("Quelle est ta pointure ?")));
}

fn kept_id(config: &Config, q: &str) -> String {
    let all = nb::read_state(config)["kept_searches"].clone();
    all.as_array().unwrap().iter().find(|k| k["q"] == q).unwrap()["id"].as_str().unwrap().to_string()
}

#[test]
fn the_sub_filter_titles_are_the_contracts() {
    let contract = nb::sommaire_contract::contract();
    for lang in ["fr", "en", "de", "es", "nl"] {
        for (category, filters) in sm::FILTERS {
            assert_eq!(contract["categories"]["filters"][category], json!(filters), "{category}");
            for f in filters {
                assert_eq!(json!(sm::filter_title(lang, f)), contract["categories"]["titles_proposed"][lang][*f], "{lang} {f}");
            }
        }
    }
}

/// The sommaire on a Carnet of about 20 000 lines (the demo copied), as the S3 measure: the copy made once, then the
/// summary, a category's page and the kept searches' counts (`cargo test --lib summary_at_scale -- --ignored
/// --nocapture`).
#[test]
#[ignore = "a measure: cargo test --lib summary_at_scale -- --ignored --nocapture"]
fn summary_at_scale() {
    let (_dir, config) = demo("fr");
    let root = nb::part_root(&config, Part::Main);
    let originals = nb::all_pages(&config, Part::Main);
    let per_copy: usize = originals.iter().map(|(_, p)| p.render().lines().count()).sum();
    for i in 0..20_000usize.div_ceil(per_copy.max(1)) {
        for (path, page) in &originals {
            let name = path.rsplit('/').next().unwrap();
            let mut copy = page.clone();
            copy.title = format!("{} {i}", page.title);
            copy.front.insert("id".into(), json!(nb::new_page_id()));
            let file = root.join(format!("Archives/{i}/{name}"));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, copy.render()).unwrap();
        }
    }
    let lines: usize = nb::all_pages(&config, Part::Main).iter().map(|(_, p)| p.render().lines().count()).sum();
    for q in ["anniversaire", "à payer ce mois-ci", "voyage", "maison", "impôts", "marc"] {
        sm::keep(&config, q, None, "all", None).unwrap();
    }
    for (q, cat) in [("garantie", "papers"), ("claire", "people"), ("cuisine", "home")] {
        sm::searched_at(&config, q, Some(cat), "all", &fr(), today()).unwrap();
    }
    let t = std::time::Instant::now();
    let first = sm::summary_at(&config, "all", &fr(), today(), now());
    let cold = t.elapsed().as_secs_f64() * 1000.0;
    let mut warm: Vec<f64> = Vec::new();
    let mut pages: Vec<f64> = Vec::new();
    for _ in 0..21 {
        let t = std::time::Instant::now();
        std::hint::black_box(sm::summary_at(&config, "all", &fr(), today(), now()));
        warm.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        std::hint::black_box(sm::category_at(&config, "people", "all", None, &fr(), false, today()).unwrap());
        pages.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    warm.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pages.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let r = |x: f64| (x * 10.0).round() / 10.0;
    println!(
        "{}",
        json!({"text_lines": lines, "items": first["total"]["count"], "summary_first_ms_copy_made": r(cold),
               "summary_p50_ms": r(warm[10]), "summary_p95_ms": r(warm[19]), "category_people_p50_ms": r(pages[10]),
               "category_people_p95_ms": r(pages[19])})
    );
}
