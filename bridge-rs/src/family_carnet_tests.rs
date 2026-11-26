//! Brick S6: the family Carnet's categories and its copy (the routes and the relay: the black-box suite,
//! bridge/tests_blackbox/test_family_carnet_sommaire.py).

use std::path::PathBuf;

use chrono::{Duration, Local};
use serde_json::{json, Value};

use super::*;
use crate::family_agent::apply;

fn repo(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(path)
}

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(repo(path)).unwrap()).unwrap()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// The demo's family Carnet of `lang` as `family-notebook.json` holds it (its days from today).
fn demo(lang: &str) -> Dict {
    let raw = if lang == "nl" {
        read("contracts/carnet/search-bench.json")["fixtures"]["nl"]["family"].clone()
    } else {
        read("App/Resources/Demo/family-notebook-demo.json")[lang].clone()
    };
    let now = crate::py::time_time();
    let today = Local::now().date_naive();
    let lists: Vec<Value> = raw["lists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let items: Vec<Value> = l["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| {
                    let due = i["due_in"].as_i64().map(|d| (today + Duration::days(d)).to_string());
                    json!({"id": i["id"], "text": i["text"], "done": i["done"].as_bool().unwrap_or(false), "due": due,
                           "by": i["by"], "at": now - 3600.0 * i["hours_ago"].as_f64().unwrap_or(0.0)})
                })
                .collect();
            json!({"id": l["id"], "title": l["title"], "kind": l["kind"], "items": items, "updated": now, "by": "Marie"})
        })
        .collect();
    let pages: Vec<Value> = raw["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            json!({"id": p["id"], "title": p["title"], "text": p["text"], "by": p["by"],
                        "updated": now - 3600.0 * p["hours_ago"].as_f64().unwrap_or(0.0)})
        })
        .collect();
    json!({"lists": lists, "pages": pages, "updated": now}).as_object().cloned().unwrap()
}

#[test]
fn the_family_table_files_the_demo_in_five_languages() {
    // the bench's reference (contracts/carnet/search-bench.json `items`, S0): the same ids in every language
    let bench = read("contracts/carnet/search-bench.json");
    let reference = bench["items"].as_object().unwrap();
    let (mut right, mut all, mut misses) = (0, 0, Vec::new());
    for lang in ["fr", "en", "de", "es", "nl"] {
        let mut data = demo(lang);
        file_all(&mut data);
        let copy = entries(&data);
        assert_eq!(copy.len(), 15, "{lang}: every list item and page");
        for e in &copy {
            let want: Vec<String> =
                reference[&e.id]["categories"].as_array().unwrap().iter().map(s).filter(|c| c != "dates").collect();
            assert!(!e.categories.iter().any(|c| c == "dates" || c == "health"), "{lang} {}: {:?}", e.id, e.categories);
            assert!(e.categories.iter().all(|c| crate::notebook::categories::FAMILY.contains(&c.as_str())));
            all += 1;
            let good = match e.categories.first() {
                None => want.is_empty(),
                Some(first) => want.contains(first),
            };
            if good {
                right += 1;
            } else {
                misses.push(format!("{lang} {} « {} » {:?} (want {want:?})", e.id, e.line, e.categories));
            }
        }
    }
    println!("family table: {right}/{all}; misses: {misses:#?}");
    assert!(right * 10 >= all * 9, "{right}/{all} well filed (90 % wanted): {misses:#?}");
}

#[test]
fn the_familys_words_decide_a_note() {
    let none: Vec<Rule> = Vec::new();
    let first = |title: &str, text: &str| decide(title, text, &none).0.first().cloned().unwrap_or_default();
    assert_eq!(first("Courses", ""), "shopping");
    assert_eq!(first("Einkäufe", ""), "shopping");
    assert_eq!(first("Boodschappen", ""), "shopping");
    assert_eq!(first("Menus de la semaine", ""), "menus");
    assert_eq!(first("Inscrire Tom au judo", ""), "school");
    assert_eq!(first("Rentrée scolaire", "Acheter les fournitures"), "school");
    assert_eq!(first("Vacances de février", "Gîte à Annecy"), "holidays");
    assert_eq!(first("Rendez-vous chaudière", ""), "home");
    assert_eq!(first("Anniversaire de Mamie", ""), "people");
    assert_eq!(first("Assurance habitation", ""), "papers");
    // the personal table's Argent and Travail have no family category: Papiers and Autres
    assert_eq!(first("Facture d'électricité", ""), "papers");
    assert_eq!(first("Xyzzy", ""), "other");
    // the person's rule first: her word, her categories
    let rules = vec![Rule { word: "menus".into(), categories: vec!["home".into()] }];
    assert_eq!(decide("Menus de Noël", "", &rules), (vec!["home".to_string()], Some("menus".to_string())));
}

#[test]
fn the_copy_of_the_family_carnet() {
    let mut data = demo("fr");
    data.insert(
        "pages".into(),
        json!([{"id": "p2", "title": "Codes et numéros utiles", "by": "Marie", "updated": 1.7e9,
                "text": "Wifi : voir la box.\nPasseport de Tom : 18AB12345.\nPlombier : M. Garnier, 06 12 34 56 78."}]),
    );
    file_all(&mut data);
    let copy = entries(&data);
    let by_id = |id: &str| copy.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no {id}: {copy:#?}"));
    let lait = by_id("fam:list:courses/c1");
    assert_eq!((lait.kind.as_str(), lait.title.as_str(), lait.line.as_str()), ("list_item", "Courses", "Lait"));
    assert_eq!(
        (lait.categories.clone(), lait.who.clone(), lait.fold.as_str()),
        (vec!["shopping".to_string()], Some("Marie".into()), "lait")
    );
    assert_eq!(lait.open, json!({"list": "courses", "item": "c1"}));
    assert!(!lait.done && by_id("fam:list:courses/c5").done);
    let today = Local::now().date_naive();
    // a task's day; a birthday every year (its day without the year), never a day of its own
    assert_eq!(by_id("fam:list:taches/t1").date, Some((today + Duration::days(3)).to_string()));
    let mamie = by_id("fam:list:dates/d1");
    assert_eq!(
        (mamie.date.clone(), mamie.yearly.clone()),
        (None, Some((today + Duration::days(6)).format("%m-%d").to_string()))
    );
    let page = by_id("fam:page:p2");
    assert_eq!((page.kind.as_str(), page.title.as_str()), ("page", "Codes et numéros utiles"));
    assert_eq!(page.open, json!({"path": "p2"}));
    // never a number nor a reference (the person's copy's rule: a line holding one is left out; a phone number stays)
    assert!(page.line.starts_with("Wifi : voir la box.\nPlombier : M. Garnier, 06 12 34 56 78."), "{page:?}");
    assert!(!page.line.contains("18AB") && !page.fold.contains("18ab") && !page.fold.contains("passeport"), "{page:?}");
    assert!(page.fold.contains("wifi") && page.fold.contains("codes"));
    assert!(copy.iter().all(|e| e.noted.as_deref().is_some_and(|n| n.len() == 10)));
}

#[test]
fn filing_at_a_write_keeps_the_persons_choice() {
    let mut data = Dict::new();
    let body = |v: Value| v.as_object().cloned().unwrap();
    apply(&mut data, &body(json!({"op": "list_save", "title": "Courses", "kind": "list"})), "Marie", 1.0).unwrap();
    apply(&mut data, &body(json!({"op": "list_save", "title": "Tâches", "kind": "tasks"})), "Marie", 1.0).unwrap();
    apply(
        &mut data,
        &body(json!({"op": "page_save", "title": "Menus de la semaine", "text": "Lundi : pâtes."})),
        "Léa",
        2.0,
    )
    .unwrap();
    let list = data["lists"][0].clone();
    assert_eq!((list["categories"].clone(), list["categories_by"].clone()), (json!(["shopping"]), json!("auto")));
    // a list of tasks or of dates has none of its own: each item has its words'
    assert!(data["lists"][1].get("categories").is_none());
    let page = data["pages"][0].clone();
    assert_eq!((page["categories"].clone(), page["categories_by"].clone()), (json!(["menus"]), json!("auto")));
    // the person's choice: never changed by a write
    data.get_mut("pages").unwrap()[0]["categories"] = json!(["home"]);
    data.get_mut("pages").unwrap()[0]["categories_by"] = json!("person");
    let pid = s(&page["id"]);
    apply(&mut data, &body(json!({"op": "page_save", "id": pid, "title": "Recettes", "text": "Gratin."})), "Léa", 3.0)
        .unwrap();
    assert_eq!(
        (data["pages"][0]["categories"].clone(), data["pages"][0]["categories_by"].clone()),
        (json!(["home"]), json!("person"))
    );
    // the bridge's own: filed again when the title changes
    let lid = s(&list["id"]);
    apply(&mut data, &body(json!({"op": "list_save", "id": lid, "title": "Menus"})), "Marie", 4.0).unwrap();
    assert_eq!(data["lists"][0]["categories"], json!(["menus"]));
    // today's view of the Carnet (the 1.0 apps, the agent's read) shows none of it
    let view = crate::family_agent::view_for_tests(&data);
    assert!(!view.to_string().contains("categor"), "{view}");
}

/// Review 1 of S6, R4: « Oublie ça » writes the family Carnet under its own lock, by a temporary file of its own:
/// the members' and the agent's writes at the same moment are never lost, the file never read half written.
#[test]
fn oublie_ca_and_the_familys_writes_never_lose_one_another() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::notebook::tests_config(dir.path());
    crate::notebook::update_state(&config, |s| {
        s.insert("language".into(), json!("fr"));
    })
    .unwrap();
    let body = |v: Value| v.as_object().cloned().unwrap();
    let made = crate::family_agent::change(
        &config,
        &body(json!({"op": "list_save", "title": "Courses", "kind": "list"})),
        "Marie",
    )
    .unwrap()
    .unwrap();
    let lid = s(&made["lists"][0]["id"]);
    std::thread::scope(|scope| {
        for t in 0..4 {
            let (config, lid) = (&config, lid.clone());
            scope.spawn(move || {
                for i in 0..25 {
                    let add = body(json!({"op": "item_add", "list": lid, "text": format!("lait {t}-{i}")}));
                    crate::family_agent::change(config, &add, "Marie").unwrap().unwrap();
                }
            });
        }
        let (config, lid) = (&config, lid.clone());
        scope.spawn(move || {
            for i in 0..15 {
                let add = body(json!({"op": "item_add", "list": lid, "text": format!("cadeau {i} pour Marc")}));
                crate::family_agent::change(config, &add, "Alex").unwrap().unwrap();
                let request =
                    crate::notebook::forget::Request { texts: vec!["Marc".into()], page: None, private: false };
                let target = crate::notebook::forget::target(config, &request).unwrap();
                crate::notebook::forget::apply(config, &target, "fr", "Alex").unwrap();
            }
        });
    });
    let text = std::fs::read_to_string(std::path::Path::new(&config.data_dir()).join("family-notebook.json")).unwrap();
    let data: Value = serde_json::from_str(&text).expect("never half written");
    let texts: Vec<String> = data["lists"][0]["items"].as_array().unwrap().iter().map(|i| s(&i["text"])).collect();
    assert_eq!(texts.iter().filter(|t| t.starts_with("lait ")).count(), 100, "a member's write lost: {texts:?}");
    assert!(!texts.iter().any(|t| t.contains("Marc")), "{texts:?}");
}

/// The family Carnet's screens have the person's structure (Alex 26/11): a category lists its pages and lists, each
/// with its count, never its items one by one; the sommaire counts them.
#[test]
fn a_family_category_lists_its_pages_and_lists() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::notebook::tests_config(dir.path());
    let mut data = demo("fr");
    file_all(&mut data);
    let file = crate::family_agent::notebook_file(&config);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, Value::Object(data.clone()).to_string()).unwrap();
    let today = Local::now().date_naive();
    let fr = vec!["fr".to_string()];
    let screen = |id: &str| category_at(&config, id, &fr, today).unwrap();
    let rows = |v: &Value| -> Vec<(String, String, u64)> {
        v["pages"].as_array().unwrap().iter().map(|p| (s(&p["title"]), s(&p["kind"]), p["notes"].as_u64().unwrap())).collect()
    };
    // Courses: the list once, with the things still to buy (« Café » is bought)
    let shopping = screen("shopping");
    let list = rows(&shopping);
    assert_eq!(list, [("Courses".to_string(), "list".to_string(), 4)]);
    assert_eq!(shopping["pages"][0]["open"], json!({"list": "courses"}));
    assert_eq!(shopping["count"], json!(1));
    // the 1.1 apps' latest items stay
    assert!(!shopping["recent"].as_array().unwrap().is_empty());
    // Dates: the lists and pages with a day to come, the soonest first
    let dates = screen("dates");
    let next: Vec<String> = dates["pages"].as_array().unwrap().iter().map(|p| s(&p["next"]["date"])).collect();
    assert!(!next.is_empty() && next.windows(2).all(|w| w[0] <= w[1]), "{next:?}");
    assert!(next.iter().all(|d| d.as_str() >= today.to_string().as_str()));
    // the sommaire counts these rows
    let summary = summary_at(&config, &fr, today, &[]);
    for line in summary["categories"].as_array().unwrap() {
        let id = s(&line["id"]);
        assert_eq!(line["count"], json!(screen(&id)["pages"].as_array().unwrap().len()), "{id}");
    }
    // nothing listed but pages and lists
    for id in c::FAMILY {
        if let Ok(v) = category_at(&config, id, &fr, today) {
            assert!(v["pages"].as_array().unwrap().iter().all(|p| p["kind"] == "page" || p["kind"] == "list"), "{id}");
        }
    }
}
