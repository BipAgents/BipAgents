//! Tests of spaces.rs (brick S5, « Perso / Pro »): the setting, a Pro agent's new pages, « Ranger dans… » and its
//! undo, what each space shows (sommaire, category, search, the phone's copy, kept searches and searches made), Santé
//! in none of them.

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::categories::Target;
use super::reads::Turn;
use super::summary as sm;
use crate::config::Config;
use crate::notebook::{self as nb, spaces, Part, Who};
use crate::Dict;

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn fr() -> Vec<String> {
    vec!["fr".to_string()]
}

fn now() -> f64 {
    crate::py::time_time()
}

fn demo() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let config = nb::tests_config(dir.path());
    nb::search_bench::write_demo_for(&config, "fr", today());
    (dir, config)
}

/// The agent « studio » in the person's « Pro » group of agents (agent_groups.rs), « daily » in no group.
fn pro_agent(config: &Config) {
    crate::agent_groups::set_group(config, "studio", "Pro").unwrap();
}

fn write(config: &Config, agent: &str, args: Value) -> String {
    let args: Dict = args.as_object().unwrap().clone();
    let out = nb::tools::call(config, agent, nb::tools::WRITE, &args, &Turn::default()).unwrap();
    assert_eq!(out["isError"], json!(false), "{out:?}");
    out["content"][0]["text"].as_str().unwrap().to_string()
}

fn tool(config: &Config, agent: &str, name: &str, args: Value) -> String {
    let args: Dict = args.as_object().unwrap().clone();
    let out = nb::tools::call(config, agent, name, &args, &Turn::default()).unwrap();
    out["content"][0]["text"].as_str().unwrap().to_string()
}

fn text(config: &Config, path: &str) -> String {
    nb::read_page_text(config, Part::Main, path).unwrap_or_default()
}

/// The copy's item of a page (by its path), and its space.
fn space_of_page(config: &Config, path: &str) -> String {
    let f = nb::index::finder(config, &fr());
    let e = f.items.iter().find(|e| e.kind == "page" && e.path.as_deref() == Some(path)).unwrap_or_else(|| panic!("no {path}"));
    e.space().to_string()
}

fn space_of_item(config: &Config, id: &str) -> String {
    let f = nb::index::finder(config, &fr());
    f.items.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no {id}")).space().to_string()
}

fn total(config: &Config, space: &str) -> u64 {
    sm::summary_at(config, space, &fr(), today(), now())["total"]["count"].as_u64().unwrap()
}

fn santé(config: &Config) {
    let private = nb::part_root(config, Part::Health).join("Domaines");
    std::fs::create_dir_all(&private).unwrap();
    // `space: pro` written by hand on a page of Santé: Santé stays outside the spaces
    std::fs::write(private.join("Santé.md"), "---\ntype: area\nspace: pro\n---\n# Santé\n\n## Ce qui compte\n- Prednisolone le matin — *Wellness, 2026-11-01*\n").unwrap();
}

#[test]
fn the_setting_is_off_at_first_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let config = nb::tests_config(dir.path());
    assert!(!spaces::on(&config));
    assert_eq!(nb::api::settings_view(&config)["separate_spaces"], json!(false));
    spaces::set_on(&config, true).unwrap();
    assert!(spaces::on(&config));
    assert_eq!(nb::api::settings_view(&config)["separate_spaces"], json!(true));
    assert_eq!(nb::read_state(&config)["separate_spaces"], json!(true));
    spaces::set_on(&config, false).unwrap();
    assert!(!spaces::on(&config));
    // the home's capabilities: `spaces` (the app shows the setting)
    assert!(nb::categories::CAPABILITIES.contains(&"spaces"));
}

#[test]
fn a_pro_agents_new_page_goes_to_pro_only_when_on() {
    let (_dir, config) = demo();
    pro_agent(&config);
    // off: nothing changes, a Pro agent's new page is like any other
    write(&config, "studio", json!({"page": "Client Dupont", "kind": "subject", "section": "status", "facts": ["Devis envoyé"]}));
    let off = text(&config, "Sujets/Client Dupont.md");
    assert!(!off.contains("space:"), "{off}");
    assert_eq!(space_of_page(&config, "Sujets/Client Dupont.md"), "perso");
    // on: its new pages are Pro (`space: pro` in the header, readable in Obsidian, after its categories)
    spaces::set_on(&config, true).unwrap();
    write(&config, "studio", json!({"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Contrat signé"]}));
    let pro = text(&config, "Sujets/Client Martin.md");
    assert!(pro.contains("categories_by: auto\nspace: pro\nupdated: "), "{pro}");
    assert_eq!(nb::Page::parse(&pro, "").front["space"], json!("pro"));
    assert_eq!(space_of_page(&config, "Sujets/Client Martin.md"), "pro");
    // its lines follow their page
    let f = nb::index::finder(&config, &fr());
    let line = f.items.iter().find(|e| e.kind == "line" && e.line == "Contrat signé").unwrap();
    assert_eq!(line.space(), "pro");
    // a person it makes, too
    write(&config, "studio", json!({"page": "Sophie Leroy", "kind": "person", "section": "work", "facts": ["Acheteuse chez Dupont"]}));
    assert_eq!(space_of_page(&config, "Personnes/Sophie Leroy.md"), "pro");
    // a page that exists keeps its space: nothing moves by itself (8 B)
    write(&config, "studio", json!({"page": "Client Dupont", "section": "status", "facts": ["Relance faite"]}));
    assert!(!text(&config, "Sujets/Client Dupont.md").contains("space:"));
    // an agent of no group, or of « Perso »: Perso
    crate::agent_groups::set_group(&config, "maison", "Perso").unwrap();
    write(&config, "daily", json!({"page": "Potager", "kind": "subject", "section": "status", "facts": ["Tomates plantées"]}));
    write(&config, "maison", json!({"page": "Chaudière", "kind": "subject", "section": "status", "facts": ["Entretien fait"]}));
    assert!(!text(&config, "Sujets/Potager.md").contains("space:"));
    assert!(!text(&config, "Sujets/Chaudière.md").contains("space:"));
    // « Toi » stays in Perso, whoever makes it
    std::fs::remove_file(nb::part_root(&config, Part::Main).join("Toi.md")).ok();
    write(&config, "studio", json!({"page": "Toi", "kind": "self", "section": "work", "facts": ["Fondateur de Kerfy"]}));
    assert!(!text(&config, "Toi.md").contains("space:"), "{}", text(&config, "Toi.md"));
    // the person's new page in the app's open tab (« Écrire une page » in Pro)
    let made = nb::new_page_in(&config, Part::Main, "subject", "Salon pro", Some("pro"), &Who::you(&config)).unwrap().unwrap();
    assert_eq!(space_of_page(&config, &made), "pro");
    let made = nb::new_page_in(&config, Part::Main, "subject", "Salon perso", Some("perso"), &Who::you(&config)).unwrap().unwrap();
    assert!(!text(&config, &made).contains("space:"));
    // switched off again: the next new page of a Pro agent is Perso, the Pro ones stay Pro
    spaces::set_on(&config, false).unwrap();
    write(&config, "studio", json!({"page": "Client Bernard", "kind": "subject", "section": "status", "facts": ["Premier appel"]}));
    assert!(!text(&config, "Sujets/Client Bernard.md").contains("space:"));
    assert_eq!(space_of_page(&config, "Sujets/Client Martin.md"), "pro");
}

#[test]
fn switching_off_loses_nothing_and_shows_everything_together() {
    let (_dir, config) = demo();
    pro_agent(&config);
    let all_before = total(&config, "all");
    spaces::set_on(&config, true).unwrap();
    write(&config, "studio", json!({"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Contrat signé", "Facture envoyée"]}));
    let page = text(&config, "Sujets/Client Martin.md");
    // on: Perso and Pro share the Carnet between them, each note in one
    let (all, perso, pro) = (total(&config, "all"), total(&config, "perso"), total(&config, "pro"));
    assert_eq!(all, all_before + 3);
    assert_eq!((pro, perso + pro), (3, all));
    let s = sm::summary_at(&config, "perso", &fr(), today(), now());
    assert_eq!(s["spaces"], json!([{"id": "perso", "title": "Perso", "selected": true}, {"id": "pro", "title": "Pro", "selected": false}]));
    // the kept searches of before (`all`) are in Perso, the Pro ones apart
    sm::keep(&config, "anniversaire", None, "all", None).unwrap();
    sm::keep(&config, "facture", None, "pro", None).unwrap();
    sm::keep(&config, "voyage", None, "perso", None).unwrap();
    let kept = |space: &str| -> Vec<String> {
        sm::kept_at(&config, space, &fr(), today())["kept"].as_array().unwrap().iter().map(|k| k["q"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(kept("perso"), ["anniversaire", "voyage"]);
    assert_eq!(kept("pro"), ["facture"]);
    // off again: nothing lost, everything together; the Pro page keeps its header (on again, it is Pro again)
    spaces::set_on(&config, false).unwrap();
    assert_eq!(text(&config, "Sujets/Client Martin.md"), page);
    assert_eq!(total(&config, "all"), all);
    let s = sm::summary_at(&config, "all", &fr(), today(), now());
    assert_eq!(s["spaces"], json!([]));
    assert_eq!(kept("all"), ["anniversaire", "facture", "voyage"]);
    let counts: Vec<u64> = s["kept"].as_array().unwrap().iter().map(|k| k["count"].as_u64().unwrap()).collect();
    assert!(counts[1] >= 1, "{}", s["kept"]);
    spaces::set_on(&config, true).unwrap();
    assert_eq!(total(&config, "pro"), 3);
}

#[test]
fn ranger_dans_pro_and_undo() {
    let (_dir, config) = demo();
    let you = Who::you(&config);
    let path = "Sujets/Rénovation cuisine.md";
    let before = text(&config, path);
    assert!(!before.is_empty());
    // « Ranger dans… Pro » from the page: only its header's `space` line changes
    let done = spaces::categorize(&config, &Target::Page { path: path.into() }, &[], Some("pro"), "fr", &you).unwrap().unwrap();
    let after = text(&config, path);
    assert_eq!(after.replace("space: pro\n", ""), before, "{after}");
    assert_eq!((done["space"].clone(), done["before"]["space"].clone()), (json!("pro"), json!("perso")));
    assert_eq!(done["toast"], json!("Rangé dans Pro."));
    assert_eq!(done["categories"], done["before"]["categories"]);
    assert_eq!(space_of_page(&config, path), "pro");
    // its lines with it
    let f = nb::index::finder(&config, &fr());
    assert!(f.items.iter().filter(|e| e.path.as_deref() == Some(path)).all(|e| e.space() == "pro"));
    // the history: the person's filing
    assert_eq!(nb::history(&config, Some(Part::Main), Some(path), 1)[0]["action"], json!(nb::categories::ACTION));
    // today's re-filings list it, undoable
    let list = spaces::today(&config, "fr");
    assert!(list["items"].as_array().unwrap().iter().any(|i| i["undo"] == done["undo"] && i["space"] == "pro" && i["text"] == "Rangé dans Pro."), "{list}");
    // undone: the page as it was, byte for byte
    let undo = done["undo"].as_str().unwrap();
    let undone = spaces::undo(&config, undo, "fr", &you).unwrap().unwrap();
    assert_eq!(undone["space"], json!("perso"));
    assert_eq!(text(&config, path), before);
    assert!(spaces::undo(&config, undo, "fr", &you).unwrap().is_none());
    // « Ranger ailleurs » with Perso or Pro: the categories and the space in one gesture, one undo
    let both = spaces::categorize(&config, &Target::Page { path: path.into() }, &["money".to_string()], Some("pro"), "fr", &you).unwrap().unwrap();
    assert_eq!((both["categories"].clone(), both["space"].clone()), (json!(["money"]), json!("pro")));
    assert_eq!(both["toast"], json!("Rangé dans Argent et Pro."));
    let page = nb::Page::parse(&text(&config, path), "");
    assert_eq!((page.front["categories"].clone(), page.front["space"].clone()), (json!(["money"]), json!("pro")));
    spaces::undo(&config, both["undo"].as_str().unwrap(), "fr", &you).unwrap().unwrap();
    assert_eq!(text(&config, path), before);
    // without a space: exactly S1's « Ranger ailleurs » (no `space` key added to its answer)
    let only = spaces::categorize(&config, &Target::Page { path: path.into() }, &["money".to_string()], None, "fr", &you).unwrap().unwrap();
    assert!(only.get("space").is_none(), "{only}");
    spaces::undo(&config, only["undo"].as_str().unwrap(), "fr", &you).unwrap().unwrap();
    // a line alone, by its Item id: the line in Pro, its page in Perso
    let f = nb::index::finder(&config, &fr());
    let line = f.items.iter().find(|e| e.kind == "line" && e.path.as_deref() == Some(path)).unwrap().id.clone();
    let page_id = f.items.iter().find(|e| e.kind == "page" && e.path.as_deref() == Some(path)).unwrap().id.clone();
    let marked = spaces::categorize(&config, &Target::Item(line.clone()), &[], Some("pro"), "fr", &you).unwrap().unwrap();
    assert_eq!((space_of_item(&config, &line), space_of_item(&config, &page_id)), ("pro".to_string(), "perso".to_string()));
    assert_eq!(text(&config, path), before, "a line's space is kept in the state, never in the page");
    // the state keeps keys only (« Oublie ça »)
    let state = nb::read_state(&config);
    assert!(state["space_marks"].as_object().unwrap().keys().all(|k| k.starts_with("line:")), "{}", state["space_marks"]);
    spaces::undo(&config, marked["undo"].as_str().unwrap(), "fr", &you).unwrap().unwrap();
    assert_eq!(space_of_item(&config, &line), "perso");
    // a record, a task: their own marks; a task follows its page otherwise
    let record = f.items.iter().find(|e| e.kind == "record").unwrap().id.clone();
    spaces::categorize(&config, &Target::Item(record.clone()), &[], Some("pro"), "fr", &you).unwrap().unwrap();
    assert_eq!(space_of_item(&config, &record), "pro");
    nb::add_task(&config, Part::Main, "Rappeler l'artisan", "", "Rénovation cuisine", "asked", "", &you).unwrap();
    spaces::categorize(&config, &Target::Page { path: path.into() }, &[], Some("pro"), "fr", &you).unwrap().unwrap();
    let f = nb::index::finder(&config, &fr());
    let task = f.items.iter().find(|e| e.kind == "task" && e.title == "Rappeler l'artisan").unwrap();
    assert_eq!(task.space(), "pro");
    // refused: another space, nothing to file, a page that is not there
    let refused = |space: &str| spaces::categorize(&config, &Target::Page { path: path.into() }, &[], Some(space), "fr", &you).unwrap().unwrap_err();
    assert_eq!(refused("family"), spaces::Refused::SpaceUnknown);
    assert_eq!(refused("all"), spaces::Refused::SpaceUnknown);
    let none = spaces::categorize(&config, &Target::Page { path: "Sujets/Rien.md".into() }, &[], Some("pro"), "fr", &you).unwrap();
    assert_eq!(none.unwrap_err().code(), "not_found");
    let nothing = spaces::categorize(&config, &Target::Page { path: path.into() }, &[], None, "fr", &you).unwrap();
    assert_eq!(nothing.unwrap_err().code(), "category_count");
}

#[test]
fn a_pro_agent_reads_everything() {
    let (_dir, config) = demo();
    pro_agent(&config);
    spaces::set_on(&config, true).unwrap();
    write(&config, "studio", json!({"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Contrat signé"]}));
    // Perso's pages as before (7 B: the separation files, it hides nothing from the person's agents)
    let found = tool(&config, "studio", nb::tools::SEARCH, json!({"words": "Marc"}));
    assert!(found.contains("Marc Durand"), "{found}");
    let read = tool(&config, "studio", nb::tools::READ, json!({"page": "Marc Durand"}));
    assert!(read.contains("Marc Durand") && !read.starts_with("Not"), "{read}");
    // and a Perso agent reads Pro's
    let found = tool(&config, "daily", nb::tools::SEARCH, json!({"words": "Client Martin"}));
    assert!(found.contains("Client Martin"), "{found}");
}

#[test]
fn kept_searches_and_searches_made_by_space() {
    let (_dir, config) = demo();
    pro_agent(&config);
    spaces::set_on(&config, true).unwrap();
    write(&config, "studio", json!({"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Facture envoyée le 3 novembre"]}));
    // a kept search counts in its space
    sm::keep(&config, "facture", None, "pro", None).unwrap();
    sm::keep(&config, "facture", None, "perso", None).unwrap();
    let count = |space: &str| sm::kept_at(&config, space, &fr(), today())["kept"][0]["count"].as_u64().unwrap();
    let f = nb::index::finder(&config, &fr());
    assert_eq!(count("pro"), f.search("facture", today(), None, Some("pro")).counts["all"] as u64);
    assert_eq!(count("perso"), f.search("facture", today(), None, Some("perso")).counts["all"] as u64);
    assert!(count("pro") >= 1);
    // six a view: Perso's six count those kept before the setting (`all`)
    for q in ["a1", "b2", "c3", "d4"] {
        sm::keep(&config, &format!("mot {q}"), None, "all", None).unwrap();
    }
    sm::keep(&config, "mot e5", None, "perso", None).unwrap();
    assert_eq!(sm::keep(&config, "mot f6", None, "perso", None), Err(sm::Refused::KeptFull));
    sm::keep(&config, "mot f6", None, "pro", None).unwrap();
    // a search made: under the best result of its space; « Cherché ici » of that space only
    assert!(sm::searched_at(&config, "facture envoyée", None, "pro", &fr(), today()).unwrap());
    let here = |space: &str| -> Vec<String> {
        let page = sm::category_at(&config, "work", space, None, &fr(), false, today()).unwrap();
        page["searched"].as_array().unwrap().iter().map(|s| s["q"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(here("pro"), ["facture envoyée"]);
    assert_eq!(here("perso"), Vec::<String>::new());
    // the phone's copy: every kept search with its space (the phone shows each view by the same rule)
    let (copy, _) = nb::index::answer(&config, &fr(), None, None, "");
    let spaces_kept: Vec<Value> = copy["kept"].as_array().unwrap().iter().map(|k| k.get("space").cloned().unwrap_or(Value::Null)).collect();
    assert!(spaces_kept.contains(&json!("pro")) && spaces_kept.contains(&json!("perso")) && spaces_kept.contains(&Value::Null));
}

#[test]
fn the_sommaire_a_category_the_search_and_the_copy_by_space() {
    let (_dir, config) = demo();
    pro_agent(&config);
    spaces::set_on(&config, true).unwrap();
    write(&config, "studio", json!({"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Contrat signé"]}));
    write(&config, "studio", json!({"page": "Sophie Leroy", "kind": "person", "section": "work", "facts": ["Acheteuse chez Martin"]}));
    let f = nb::index::finder(&config, &fr());
    let pro_ids: Vec<String> = f.items.iter().filter(|e| e.space() == "pro").map(|e| e.id.clone()).collect();
    assert!(pro_ids.len() >= 4, "{pro_ids:?}");
    // the sommaire: each category counts its space's notes; Pro has no Santé line and no card
    let pro = sm::summary_at(&config, "pro", &fr(), today(), now());
    assert_eq!(pro["total"]["count"], json!(pro_ids.len()));
    assert!(pro["categories"].as_array().unwrap().iter().all(|l| l["id"] != "health"));
    assert_eq!(pro["card"], Value::Null);
    assert_eq!(pro["spaces"], json!([{"id": "perso", "title": "Perso", "selected": false}, {"id": "pro", "title": "Pro", "selected": true}]));
    let people = pro["categories"].as_array().unwrap().iter().find(|l| l["id"] == "people").unwrap();
    assert_eq!(people["first"]["space"], json!("pro"));
    // in the app's language
    let en = sm::summary_at(&config, "pro", &["fr".to_string(), "en".to_string()], today(), now());
    assert_eq!(en["spaces"][1]["title"], json!("Work"));
    // asked `all` while on: none selected
    let all = sm::summary_at(&config, "all", &fr(), today(), now());
    assert!(all["spaces"].as_array().unwrap().iter().all(|s| s["selected"] == json!(false)), "{}", all["spaces"]);
    // a category's page
    let work = sm::category_at(&config, "work", "pro", None, &fr(), false, today()).unwrap();
    assert!(work["recent"].as_array().unwrap().iter().all(|i| i["space"] == "pro"));
    assert!(work["count"].as_u64().unwrap() >= 1);
    let perso_people = sm::category_at(&config, "people", "perso", None, &fr(), false, today()).unwrap();
    assert!(perso_people["recent"].as_array().unwrap().iter().all(|i| i["space"] == "perso"));
    // the search
    let found = f.search("Martin", today(), None, Some("pro"));
    assert!(found.counts["all"] >= 1 && found.order.iter().all(|at| f.items[*at].space() == "pro"));
    // the phone's copy of one space: its notes only, its own ETag; a note that leaves the space is removed
    let (pro_copy, pro_etag) = nb::index::answer_in(&config, &fr(), None, None, "", "pro");
    let ids: Vec<String> = pro_copy["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), pro_ids.len());
    assert!(pro_copy["items"].as_array().unwrap().iter().all(|i| i["space"] == "pro"));
    assert_ne!(pro_etag, nb::index::etag(&config, &fr()));
    assert_eq!(pro_etag, nb::index::etag_in(&config, &fr(), "pro"));
    let (whole, _) = nb::index::answer_in(&config, &fr(), None, None, "", "all");
    assert_eq!(whole["items"], nb::index::answer(&config, &fr(), None, None, "").0["items"]);
    let version = pro_copy["version"].as_u64().unwrap();
    let epoch = pro_copy["epoch"].as_str().unwrap().to_string();
    let done = spaces::categorize(&config, &Target::Page { path: "Sujets/Client Martin.md".into() }, &[], Some("perso"), "fr", &Who::you(&config)).unwrap().unwrap();
    assert_eq!(done["space"], json!("perso"));
    let (diff, _) = nb::index::answer_in(&config, &fr(), Some(version), Some(&epoch), "", "pro");
    assert_eq!(diff["full"], json!(false));
    let removed: Vec<&str> = diff["removed"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
    assert!(removed.iter().any(|r| r.starts_with("page:")), "{removed:?}");
    assert!(diff["items"].as_array().unwrap().iter().all(|i| i["space"] == "pro"));
    // the setting off: no selector (the app keeps its « Moi | Famille »)
    spaces::set_on(&config, false).unwrap();
    assert_eq!(sm::summary_at(&config, "pro", &fr(), today(), now())["spaces"], json!([]));
}

#[test]
fn pro_shows_in_the_selector_only_with_a_pro_agent_or_a_pro_note() {
    let (_dir, config) = demo();
    spaces::set_on(&config, true).unwrap();
    // on, no Pro agent, no Pro note: Perso alone (the app adds its Famille)
    assert_eq!(sm::summary_at(&config, "perso", &fr(), today(), now())["spaces"], json!([{"id": "perso", "title": "Perso", "selected": true}]));
    pro_agent(&config);
    assert_eq!(sm::summary_at(&config, "perso", &fr(), today(), now())["spaces"].as_array().unwrap().len(), 2);
    crate::agent_groups::set_group(&config, "studio", "").unwrap();
    spaces::categorize(&config, &Target::Page { path: "Sujets/Rénovation cuisine.md".into() }, &[], Some("pro"), "fr", &Who::you(&config)).unwrap().unwrap();
    assert_eq!(sm::summary_at(&config, "perso", &fr(), today(), now())["spaces"].as_array().unwrap().len(), 2);
}

#[test]
fn sante_is_in_no_space() {
    let (_dir, config) = demo();
    santé(&config);
    pro_agent(&config);
    spaces::set_on(&config, true).unwrap();
    // never in the copy, whatever its header says; never counted in Perso or Pro
    let (copy, _) = nb::index::answer(&config, &fr(), None, None, "");
    assert!(!copy.to_string().contains("Prednisolone"));
    for space in ["pro", "perso"] {
        let (c, _) = nb::index::answer_in(&config, &fr(), None, None, "", space);
        assert!(!c.to_string().contains("Prednisolone"), "{space}");
        let s = sm::summary_at(&config, space, &fr(), today(), now());
        assert!(!s.to_string().contains("Prednisolone"), "{space}");
    }
    // its line: locked in Perso (and `all`), absent from Pro
    let line = |space: &str| sm::summary_at(&config, space, &fr(), today(), now())["categories"].as_array().unwrap().iter().any(|l| l["id"] == "health");
    assert!(line("perso") && line("all") && !line("pro"));
    // its page behind the lock: the whole of Santé, whatever the space asked (it has none)
    for space in ["all", "perso", "pro"] {
        let page = sm::category_at(&config, "health", space, None, &fr(), true, today()).unwrap();
        assert_eq!(page["count"], json!(2), "{space}: {page}");
    }
    // Santé's search too
    let h = nb::index::health_finder(&config, &fr());
    assert!(h.items.iter().all(|e| e.space() == "perso"));
    // a Pro agent never gives `space` to a page of the private part (the health agent's write there: none)
    std::fs::write(nb::part_root(&config, Part::Health).join("Domaines/Santé.md"), "---\ntype: area\n---\n# Santé\n").unwrap();
    let mut page = nb::Page::new("subject", "Kiné");
    assert!(!spaces::on_write(&config, Part::Health, "Sujets/Kiné.md", &mut page, &Who::agent(&config, "studio"), true));
    assert!(!page.front.contains_key("space"));
    // « Ranger dans… » never reaches it (its pages are not in the shared part)
    let r = spaces::categorize(&config, &Target::Page { path: "Domaines/Santé.md".into() }, &[], Some("pro"), "fr", &Who::you(&config)).unwrap();
    assert_eq!(r.unwrap_err().code(), "not_found");
}

#[test]
fn the_phones_copy_moves_with_a_space() {
    let (_dir, config) = demo();
    let (first, etag) = nb::index::answer(&config, &fr(), None, None, "");
    let path = "Sujets/Rénovation cuisine.md";
    spaces::categorize(&config, &Target::Page { path: path.into() }, &[], Some("pro"), "fr", &Who::you(&config)).unwrap().unwrap();
    let (diff, moved) = nb::index::answer(&config, &fr(), first["version"].as_u64(), first["epoch"].as_str(), "");
    assert_ne!(moved, etag);
    // the page, its lines and its tasks (« Rappeler Marc pour le devis » is linked to it), now Pro; nothing else
    let items = diff["items"].as_array().unwrap();
    assert!(items.iter().all(|i| i["space"] == "pro" && (i["path"] == path || i["kind"] == "task")), "{items:?}");
    assert!(items.iter().any(|i| i["kind"] == "page") && items.iter().any(|i| i["kind"] == "task"), "{items:?}");
    // a key at its default left out: a Perso note has no `space`
    let (whole, _) = nb::index::answer(&config, &fr(), None, None, "");
    let pro: Vec<&Value> = whole["items"].as_array().unwrap().iter().filter(|i| i.get("space").is_some()).collect();
    assert_eq!(pro.len(), items.len());
}
