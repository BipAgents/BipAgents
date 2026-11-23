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
        .map(|p| json!({"id": p["id"], "title": p["title"], "text": p["text"], "by": p["by"],
                        "updated": now - 3600.0 * p["hours_ago"].as_f64().unwrap_or(0.0)}))
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
            let want: Vec<String> = reference[&e.id]["categories"]
                .as_array()
                .unwrap()
                .iter()
                .map(s)
                .filter(|c| c != "dates")
                .collect();
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
                "text": "Wifi : voir la box.\nPlombier : M. Garnier, 06 12 34 56 78.\nÉcole de Tom : 01 00 00 00 00."}]),
    );
    file_all(&mut data);
    let copy = entries(&data);
    let by_id = |id: &str| copy.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no {id}: {copy:#?}"));
    let lait = by_id("fam:list:courses/c1");
    assert_eq!((lait.kind.as_str(), lait.title.as_str(), lait.line.as_str()), ("list_item", "Courses", "Lait"));
    assert_eq!((lait.categories.clone(), lait.who.clone(), lait.fold.as_str()), (vec!["shopping".to_string()], Some("Marie".into()), "lait"));
    assert_eq!(lait.open, json!({"list": "courses", "item": "c1"}));
    assert!(!lait.done && by_id("fam:list:courses/c5").done);
    let today = Local::now().date_naive();
    // a task's day; a birthday every year (its day without the year), never a day of its own
    assert_eq!(by_id("fam:list:taches/t1").date, Some((today + Duration::days(3)).to_string()));
    let mamie = by_id("fam:list:dates/d1");
    assert_eq!((mamie.date.clone(), mamie.yearly.clone()), (None, Some((today + Duration::days(6)).format("%m-%d").to_string())));
    let page = by_id("fam:page:p2");
    assert_eq!((page.kind.as_str(), page.title.as_str()), ("page", "Codes et numéros utiles"));
    assert_eq!(page.open, json!({"path": "p2"}));
    // never a number: the lines holding one are left out of the copy
    assert!(page.line.starts_with("Wifi : voir la box.") && !page.line.contains("06") && !page.fold.contains("06"), "{page:?}");
    assert!(page.fold.contains("wifi") && page.fold.contains("codes"));
    assert!(copy.iter().all(|e| e.noted.as_deref().is_some_and(|n| n.len() == 10)));
}

#[test]
fn filing_at_a_write_keeps_the_persons_choice() {
    let mut data = Dict::new();
    let body = |v: Value| v.as_object().cloned().unwrap();
    apply(&mut data, &body(json!({"op": "list_save", "title": "Courses", "kind": "list"})), "Marie", 1.0).unwrap();
    apply(&mut data, &body(json!({"op": "list_save", "title": "Tâches", "kind": "tasks"})), "Marie", 1.0).unwrap();
    apply(&mut data, &body(json!({"op": "page_save", "title": "Menus de la semaine", "text": "Lundi : pâtes."})), "Léa", 2.0).unwrap();
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
    apply(&mut data, &body(json!({"op": "page_save", "id": pid, "title": "Recettes", "text": "Gratin."})), "Léa", 3.0).unwrap();
    assert_eq!((data["pages"][0]["categories"].clone(), data["pages"][0]["categories_by"].clone()), (json!(["home"]), json!("person")));
    // the bridge's own: filed again when the title changes
    let lid = s(&list["id"]);
    apply(&mut data, &body(json!({"op": "list_save", "id": lid, "title": "Menus"})), "Marie", 4.0).unwrap();
    assert_eq!(data["lists"][0]["categories"], json!(["menus"]));
    // today's view of the Carnet (the 1.0 apps, the agent's read) shows none of it
    let view = crate::family_agent::view_for_tests(&data);
    assert!(!view.to_string().contains("categor"), "{view}");
}
