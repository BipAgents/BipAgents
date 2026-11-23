//! « N points semblent mal rangés » (Alex, 10/11: « améliorer l'assignation »): each night, without a model, the
//! Carnet looks for points that are clearly in the wrong place and proposes moving them, in one card on the Carnet's
//! home (never a notification, never a move without the person's « Déplacer »):
//!
//! - never a point of the private health part (Santé), whatever its words: the card is shown outside the app's lock
//!   (review 1 of carnet-s4, B1, 22/11: « Prednisolone le matin » has no word of the strict health filter and showed
//!   on the home with its page « Santé »). A card an older bridge kept with such points is read without them, and
//!   answering it by their ids moves nothing of Santé;
//! - on the family's pages (« Famille », « Famille élargie », « Toi »'s family), what a work agent noted (assign.rs)
//!   → the project's page its label names (« BipAgents : … » → « BipAgents »), else the « Travail » area;
//! - what goes against a correction of the person (moves.rs): a point like those they moved, by the same agent, back
//!   on the page they were moved from → where they put them.
//!
//! The card: « Tout déplacer » (moves.rs, kept as corrections like the person's own moves), « Un par un » (each point
//! with « Déplacer » and « Laisser ») and « Laisser » (those points are not proposed again).
//!
//!     GET  /v1/notebook                  `misplaced`: the card, or null
//!     POST /v1/notebook/misplaced        {action: "move"|"leave", ids?: [...]} (no ids: all) → {moved, toast, card}
//!
//! state.json `misplaced`: `{items: [item], left: [id]}`.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{assign, fold, lexicon, moves, split_attribution, template, Page, Part, Who, KINDS};
use crate::config::Config;
use crate::tools_knowledge as tk;

/// Points proposed at most.
pub const ITEMS_MAX: usize = 30;
/// Points left kept (not proposed again).
pub const LEFT_KEPT: usize = 500;

struct Texts {
    one: &'static str,
    many: &'static str,
    all: &'static str,
    by_one: &'static str,
    leave: &'static str,
    move_one: &'static str,
    moved: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { one: "Un point semble mal rangé", many: "{n} points semblent mal rangés", all: "Tout déplacer",
            by_one: "Un par un", leave: "Laisser", move_one: "Déplacer", moved: "Déplacé." },
        "de" => Texts { one: "Ein Punkt scheint falsch abgelegt", many: "{n} Punkte scheinen falsch abgelegt",
            all: "Alle verschieben", by_one: "Einzeln", leave: "Lassen", move_one: "Verschieben", moved: "Verschoben." },
        "es" => Texts { one: "Un punto parece mal archivado", many: "{n} puntos parecen mal archivados", all: "Mover todos",
            by_one: "Uno a uno", leave: "Dejar", move_one: "Mover", moved: "Movido." },
        _ => Texts { one: "One point looks misfiled", many: "{n} points look misfiled", all: "Move all",
            by_one: "One by one", leave: "Leave", move_one: "Move", moved: "Moved." },
    }
}

fn id_of(part: Part, path: &str, text: &str) -> String {
    let digest = Sha256::digest(format!("{}\n{path}\n{}", part.as_str(), fold(text)).as_bytes());
    format!("mp{}", &hex::encode(digest)[..12])
}

/// The points of a page: (section, text, who noted it).
fn points(config: &Config, page: &Page) -> Vec<(String, String, String)> {
    let body = super::in_outline(config, page);
    let mut out = Vec::new();
    let mut section = String::new();
    for line in body.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            section = name.trim().to_string();
            continue;
        }
        let t = line.trim();
        let Some(item) = t.strip_prefix("- ") else { continue };
        let (text, by) = split_attribution(item);
        out.push((section.clone(), text, by.map(|(w, _)| w).unwrap_or_default()));
    }
    out
}

fn item(part: Part, path: &str, page: &Page, text: &str, who: &str, to: Value, reason: &str) -> Value {
    json!({"id": id_of(part, path, text), "part": part.as_str(), "path": path, "title": page.title, "text": text,
           "by": who, "to": to, "reason": reason})
}

fn to_page(part: Part, path: &str, title: &str, kind: &str, section: &str) -> Value {
    json!({"part": part.as_str(), "path": path, "title": title, "kind": kind, "section": section})
}

/// Where a work agent's point on a family page goes: the page its label names (« BipAgents : … » → « BipAgents »)
/// when there is one, or when several of those points share that label (a block); else « Travail ».
fn work_target(config: &Config, text: &str, from: &Page, labels: &[String]) -> Value {
    let lang = super::language(config);
    if let Some((label, _)) = lexicon::split_label(text) {
        let generic = lexicon::label_of(&label).is_some() || label.chars().count() < 3;
        if !generic && fold(&label) != fold(&from.title) && !assign::is_family_title(&label) {
            let found = super::candidates(config, &[Part::Main], &label, "");
            if let [one] = found.as_slice() {
                return to_page(Part::Main, &one.path, &one.page.title, &one.page.kind(), "");
            }
            if labels.iter().filter(|l| fold(l) == fold(&label)).count() >= 2 {
                return to_page(Part::Main, "", &label, "subject", "");
            }
        }
    }
    let area = assign::work_area(&lang);
    match super::candidates(config, &[Part::Main], area, "area").into_iter().next() {
        Some(f) => to_page(Part::Main, &f.path, &f.page.title, &f.page.kind(), ""),
        None => to_page(Part::Main, "", area, "area", ""),
    }
}

/// The points to propose now (the left ones aside), the newest rules first: the shared part's only, never Santé's.
pub fn find(config: &Config) -> Vec<Value> {
    let state = super::read_state(config);
    let left = super::strings(state.get("misplaced").and_then(|m| m.get("left")));
    let mut out: Vec<Value> = Vec::new();
    let push = |v: Value, out: &mut Vec<Value>| {
        let id = tk::str_or_empty(v.get("id"));
        if out.len() < ITEMS_MAX && !left.contains(&id) && !out.iter().any(|o| o["id"] == json!(id)) {
            out.push(v);
        }
    };
    // 2. a work agent's points on the family's pages
    let corrections: Vec<Value> = state.get("corrections").and_then(Value::as_array).cloned().unwrap_or_default();
    for (path, page) in super::all_pages(config, Part::Main) {
        if !KINDS.contains(&page.kind().as_str()) {
            continue;
        }
        let family = assign::is_family_page(&page);
        let is_self = page.kind() == "self";
        let all = points(config, &page);
        let misfiled = |section: &str, who: &str| -> bool {
            (family || (is_self && template::resolve("self", section) == Some("family")))
                && assign::agent_named(config, who).is_some_and(|a| assign::is_work_agent(config, &a))
        };
        let labels: Vec<String> = all
            .iter()
            .filter(|(section, _, who)| misfiled(section, who))
            .filter_map(|(_, text, _)| lexicon::split_label(text).map(|(l, _)| l))
            .collect();
        for (section, text, who) in all {
            let Some(agent) = assign::agent_named(config, &who) else { continue };
            if misfiled(&section, &who) {
                let to = work_target(config, &text, &page, &labels);
                push(item(Part::Main, &path, &page, &text, &who, to, "work"), &mut out);
                continue;
            }
            // 3. against a correction: back where the person took such points from
            let id = page.id();
            let undone = corrections.iter().rev().find(|c| {
                tk::str_or_empty(c.get("agent")) == agent
                    && Part::parse(&tk::str_or_empty(c.get("from_part"))) == Part::Main
                    && (tk::str_or_empty(c.get("from_path")) == path
                        || (!id.is_empty() && tk::str_or_empty(c.get("from_id")) == id))
                    && moves::matches(c, &page.kind(), &section, &text)
            });
            if let Some(c) = undone {
                let to_part = Part::parse(&tk::str_or_empty(c.get("to_part")));
                let to_path = tk::str_or_empty(c.get("to_path"));
                let there = super::read_page_text(config, to_part, &to_path).is_some();
                let to = to_page(
                    to_part,
                    if there { &to_path } else { "" },
                    &tk::str_or_empty(c.get("to_title")),
                    &tk::str_or_empty(c.get("to_kind")),
                    &tk::str_or_empty(c.get("to_section")),
                );
                push(item(Part::Main, &path, &page, &text, &who, to, "moved"), &mut out);
            }
        }
    }
    out
}

/// The nightly look (free): the card's points, kept until answered. Returns how many there are.
pub fn detect(config: &Config) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let items = find(config);
    let n = items.len();
    super::update_state(config, |s| {
        let mut m = s.get("misplaced").and_then(Value::as_object).cloned().unwrap_or_default();
        m.insert("items".into(), Value::Array(items));
        s.insert("misplaced".into(), Value::Object(m));
    })?;
    Ok(n)
}

/// The card's points as kept, never one of the private health part (an older bridge's card may hold some: they are
/// neither shown nor moved, and leave the state at the next answer or night).
fn kept_items(config: &Config) -> Vec<Value> {
    super::read_state(config)
        .get("misplaced")
        .and_then(|m| m.get("items"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|i| Part::parse(&tk::str_or_empty(i.get("part"))) != Part::Health).cloned().collect())
        .unwrap_or_default()
}

fn shown_text(text: &str) -> String {
    match lexicon::split_label(text) {
        Some((label, _)) => label,
        None => tk::head(text, 40).to_string(),
    }
}

/// The card, ready to show (`shown`: the app's language), or None when nothing waits.
pub fn card(config: &Config, shown: &str) -> Option<Value> {
    let items = kept_items(config);
    if items.is_empty() {
        return None;
    }
    let t = texts(shown);
    let title = if items.len() == 1 { t.one.to_string() } else { t.many.replace("{n}", &items.len().to_string()) };
    // one line per page they leave and page they go to
    let mut groups: Vec<(String, String, Vec<&Value>)> = Vec::new();
    for item in &items {
        let from = tk::str_or_empty(item.get("title"));
        let to = tk::str_or_empty(item["to"].get("title"));
        match groups.iter_mut().find(|(f, t, _)| *f == from && *t == to) {
            Some((_, _, list)) => list.push(item),
            None => groups.push((from, to, vec![item])),
        }
    }
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(from, to, list)| {
            let names: Vec<String> = list.iter().take(2).map(|i| shown_text(&tk::str_or_empty(i.get("text")))).collect();
            let mut text = names.join(", ");
            if list.len() > 2 {
                text.push('…');
            }
            if list.len() > 1 {
                text.push_str(&format!(" ({})", list.len()));
            }
            let by: Vec<String> = list.iter().map(|i| tk::str_or_empty(i.get("by"))).collect();
            let by = if by.iter().all(|b| *b == by[0]) && !by[0].is_empty() { json!(by[0]) } else { Value::Null };
            json!({"ids": list.iter().map(|i| i["id"].clone()).collect::<Vec<_>>(), "text": text,
                   "from": from, "to": to, "by": by})
        })
        .collect();
    let rows: Vec<Value> = items
        .iter()
        .map(|i| json!({"id": i["id"], "text": i["text"], "from": i["title"], "to": i["to"]["title"], "by": i["by"]}))
        .collect();
    Some(json!({"title": title, "groups": groups, "items": rows, "all": t.all, "one": t.by_one, "leave": t.leave,
                "move": t.move_one}))
}

/// The person's answer: `move` the points `ids` (all when empty) or `leave` them. Returns (moved, toast, card).
pub fn answer(config: &Config, action: &str, ids: &[String], shown: &str) -> anyhow::Result<(usize, String, Option<Value>)> {
    let items = kept_items(config);
    let chosen: Vec<Value> = items
        .iter()
        .filter(|i| ids.is_empty() || ids.contains(&tk::str_or_empty(i.get("id"))))
        .cloned()
        .collect();
    let chosen_ids: Vec<String> = chosen.iter().map(|i| tk::str_or_empty(i.get("id"))).collect();
    let mut moved = 0;
    if action == "move" {
        let who = Who::you(config);
        let _guard = super::lock();
        // one move per page they leave and page they go to
        let mut batches: Vec<(Part, String, Value, Vec<String>)> = Vec::new();
        for i in &chosen {
            let part = Part::parse(&tk::str_or_empty(i.get("part")));
            let path = tk::str_or_empty(i.get("path"));
            let to = i["to"].clone();
            let text = tk::str_or_empty(i.get("text"));
            match batches.iter_mut().find(|(p, f, t, _)| *p == part && *f == path && *t == to) {
                Some((_, _, _, list)) => list.push(text),
                None => batches.push((part, path, to, vec![text])),
            }
        }
        for (part, path, to, list) in batches {
            let target = moves::Target {
                part: Some(Part::parse(&tk::str_or_empty(to.get("part")))),
                path: tk::str_or_empty(to.get("path")),
                title: tk::str_or_empty(to.get("title")),
                kind: tk::str_or_empty(to.get("kind")),
            };
            let section = tk::str_or_empty(to.get("section"));
            match moves::move_locked(config, part, &path, None, &list, None, &target, &section, &who, true) {
                Ok(m) => moved += m.moved,
                Err(err) if err.downcast_ref::<moves::Refused>().is_some() => {}
                Err(err) => return Err(err),
            }
        }
    }
    super::update_state(config, |s| {
        let mut m = s.get("misplaced").and_then(Value::as_object).cloned().unwrap_or_default();
        let rest: Vec<Value> = items.iter().filter(|i| !chosen_ids.contains(&tk::str_or_empty(i.get("id")))).cloned().collect();
        m.insert("items".into(), Value::Array(rest));
        if action == "leave" {
            let mut left = super::strings(m.get("left"));
            left.extend(chosen_ids.iter().cloned());
            let over = left.len().saturating_sub(LEFT_KEPT);
            left.drain(..over);
            m.insert("left".into(), json!(left));
        }
        s.insert("misplaced".into(), Value::Object(m));
    })?;
    let toast = if moved > 0 { texts(shown).moved.to_string() } else { String::new() };
    Ok((moved, toast, card(config, shown)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        for (id, name, category) in [("kerfy", "Kerfy Studio", Some("work")), ("quotidien", "Quotidien", None)] {
            let agent = crate::config::AgentConfig {
                name: id.into(),
                display_name: name.into(),
                category: category.map(str::to_string),
                ..Default::default()
            };
            config.agents.insert(id.into(), agent);
        }
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    #[test]
    fn misfiled_points_are_proposed_then_moved_or_left() {
        let (_dir, config) = setup();
        let root = super::super::part_root(&config, Part::Main);
        std::fs::create_dir_all(root.join("Domaines")).unwrap();
        std::fs::write(
            root.join("Domaines/Famille.md"),
            "---\ntype: area\n---\n# Famille\n\n## Ce qui compte\n- Judo de Zoé : mercredi 14 h — *Quotidien, 2026-11-03*\n\
             - BipAgents : panorama concurrentiel livré le 25/10 — *Kerfy Studio, 2026-10-25*\n\
             - BipAgents : aucun concurrent ne cumule agents, famille et serveur UE — *Kerfy Studio, 2026-10-25*\n\
             - Promotion : Buffer, plan gratuit — *Kerfy Studio, 2026-10-25*\n",
        )
        .unwrap();
        let health = super::super::part_root(&config, Part::Health);
        std::fs::create_dir_all(health.join("Domaines")).unwrap();
        let sante = "---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n- Allergie de Zoé : pénicilline — *Coach, 2026-10-14*\n\
             - Enceinte Bluetooth : à acheter pour le salon — *Quotidien, 2026-11-08*\n- Prednisolone le matin — *Coach, 2026-11-01*\n";
        std::fs::write(health.join("Domaines/Santé.md"), sante).unwrap();
        // the private health part is never proposed, whatever its words (review 1 of carnet-s4, B1): only Famille's
        assert_eq!(detect(&config).unwrap(), 3);
        let card = card(&config, "fr").unwrap();
        assert_eq!(card["title"], "3 points semblent mal rangés");
        let text = card.to_string();
        assert!(!text.contains("Santé") && !text.contains("Enceinte") && !text.contains("Prednisolone"), "{text}");
        let groups = card["groups"].as_array().unwrap();
        assert!(groups.iter().any(|g| g["from"] == "Famille" && g["to"] == "BipAgents" && g["by"] == "Kerfy Studio"));
        assert!(groups.iter().any(|g| g["from"] == "Famille" && g["to"] == "Travail"));
        // « Laisser » on the promotion: not proposed again
        let promotion = card["items"].as_array().unwrap().iter().find(|i| i["to"] == "Travail").unwrap()["id"].as_str().unwrap().to_string();
        let (moved, _, card) = answer(&config, "leave", &[promotion], "fr").unwrap();
        assert_eq!(moved, 0);
        assert_eq!(card.unwrap()["title"], "2 points semblent mal rangés");
        assert_eq!(detect(&config).unwrap(), 2);
        // « Tout déplacer »
        let (moved, toast, card) = answer(&config, "move", &[], "fr").unwrap();
        assert_eq!((moved, toast.as_str(), card.is_none()), (2, "Déplacé.", true));
        let famille = std::fs::read_to_string(root.join("Domaines/Famille.md")).unwrap();
        assert!(famille.contains("Judo") && famille.contains("Promotion") && !famille.contains("panorama"), "{famille}");
        let bip = std::fs::read_to_string(root.join("Sujets/BipAgents.md")).unwrap();
        assert!(bip.contains("- panorama concurrentiel livré le 25/10 — *Kerfy Studio, 2026-10-25*"), "{bip}");
        assert_eq!(std::fs::read_to_string(health.join("Domaines/Santé.md")).unwrap(), sante);
        assert_eq!(detect(&config).unwrap(), 0);
    }

    #[test]
    fn a_point_of_the_health_part_is_never_shown_nor_moved() {
        let (_dir, config) = setup();
        let health = super::super::part_root(&config, Part::Health);
        std::fs::create_dir_all(health.join("Domaines")).unwrap();
        let sante = "---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n- Prednisolone le matin — *Coach, 2026-11-01*\n";
        std::fs::write(health.join("Domaines/Santé.md"), sante).unwrap();
        // a card an older bridge kept in state.json, with a point of Santé in it
        let held = item(Part::Health, "Domaines/Santé.md", &Page::parse(sante, "Santé"), "Prednisolone le matin", "Coach",
                        to_page(Part::Main, "", "À ranger", "inbox", ""), "health");
        let id = held["id"].as_str().unwrap().to_string();
        super::super::update_state(&config, |s| {
            s.insert("misplaced".into(), json!({"items": [held]}));
        })
        .unwrap();
        assert_eq!(card(&config, "fr"), None);
        for ids in [vec![id.clone()], vec![]] {
            let (moved, _, card) = answer(&config, "move", &ids, "fr").unwrap();
            assert_eq!((moved, card), (0, None));
        }
        assert_eq!(std::fs::read_to_string(health.join("Domaines/Santé.md")).unwrap(), sante);
        assert!(!Value::Object(super::super::read_state(&config)).to_string().contains("Prednisolone"));
        assert_eq!(detect(&config).unwrap(), 0);
    }

    #[test]
    fn a_page_of_sante_is_never_the_cards_destination() {
        // review of fix-misplaced-health, B1: a correction towards Santé (the person moved a point of Famille to a page
        // of Santé) never makes a proposal, nor shows that page's title, on the card outside the lock
        let (_dir, config) = setup();
        let root = super::super::part_root(&config, Part::Main);
        std::fs::create_dir_all(root.join("Domaines")).unwrap();
        std::fs::write(
            root.join("Domaines/Famille.md"),
            "---\ntype: area\n---\n# Famille\n\n## Ce qui compte\n- Psy de Zoé : mardi 17 h — *Quotidien, 2026-11-03*\n\
             - Psy de Zoé : jeudi 18 h, nouveau créneau — *Quotidien, 2026-11-05*\n",
        )
        .unwrap();
        let to = moves::Target { part: Some(Part::Health), path: String::new(), title: "Psy de Zoé".into(), kind: "subject".into() };
        let moved = moves::move_points(&config, Part::Main, "Domaines/Famille.md", None, &["Psy de Zoé : mardi 17 h".into()],
            None, &to, "", &Who::you(&config), true)
        .unwrap();
        assert_eq!(moved.moved, 1);
        let state = super::super::read_state(&config);
        let corrections = state.get("corrections").and_then(Value::as_array).cloned().unwrap_or_default();
        assert!(corrections.iter().any(|c| c["to_part"] == "health"), "{corrections:?}");
        // the other one, like it, stays on Famille: not proposed towards Santé
        assert_eq!(detect(&config).unwrap(), 0);
        assert_eq!(card(&config, "fr"), None);
        // an older bridge's card with such a destination: neither shown nor moved
        let page = Page::parse("---\ntype: area\n---\n# Famille\n", "Famille");
        let held = item(Part::Main, "Domaines/Famille.md", &page, "Psy de Zoé : jeudi 18 h, nouveau créneau", "Quotidien",
                        to_page(Part::Health, "", "Psy de Zoé", "subject", ""), "moved");
        let id = held["id"].as_str().unwrap().to_string();
        super::super::update_state(&config, |s| {
            s.insert("misplaced".into(), json!({"items": [held]}));
        })
        .unwrap();
        assert_eq!(card(&config, "fr"), None);
        for ids in [vec![id.clone()], vec![]] {
            let (moved, _, card) = answer(&config, "move", &ids, "fr").unwrap();
            assert_eq!((moved, card), (0, None));
        }
        assert!(std::fs::read_to_string(root.join("Domaines/Famille.md")).unwrap().contains("jeudi 18 h"));
        let kept = super::super::read_state(&config).get("misplaced").cloned().unwrap_or(Value::Null);
        assert!(!kept.to_string().contains("Psy de Zoé"), "{kept}");
    }

    #[test]
    fn an_older_card_mixed_shows_and_moves_only_the_shared_points() {
        // review of fix-misplaced-health, R2: one point of Santé and two of Famille kept by an older bridge
        let (_dir, config) = setup();
        let root = super::super::part_root(&config, Part::Main);
        std::fs::create_dir_all(root.join("Domaines")).unwrap();
        let famille = "---\ntype: area\n---\n# Famille\n\n## Ce qui compte\n- Judo de Zoé : mercredi 14 h — *Quotidien, 2026-11-03*\n\
             - BipAgents : panorama concurrentiel livré le 25/10 — *Kerfy Studio, 2026-10-25*\n\
             - BipAgents : aucun concurrent ne cumule agents, famille et serveur UE — *Kerfy Studio, 2026-10-25*\n";
        std::fs::write(root.join("Domaines/Famille.md"), famille).unwrap();
        let health = super::super::part_root(&config, Part::Health);
        std::fs::create_dir_all(health.join("Domaines")).unwrap();
        let sante = "---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n- Prednisolone le matin — *Coach, 2026-11-01*\n";
        std::fs::write(health.join("Domaines/Santé.md"), sante).unwrap();
        let fam = Page::parse(famille, "Famille");
        let to_bip = to_page(Part::Main, "", "BipAgents", "subject", "");
        let items = vec![
            item(Part::Health, "Domaines/Santé.md", &Page::parse(sante, "Santé"), "Prednisolone le matin", "Coach",
                 to_page(Part::Main, "", "À ranger", "inbox", ""), "health"),
            item(Part::Main, "Domaines/Famille.md", &fam, "BipAgents : panorama concurrentiel livré le 25/10", "Kerfy Studio", to_bip.clone(), "work"),
            item(Part::Main, "Domaines/Famille.md", &fam, "BipAgents : aucun concurrent ne cumule agents, famille et serveur UE", "Kerfy Studio", to_bip, "work"),
        ];
        super::super::update_state(&config, |s| {
            s.insert("misplaced".into(), json!({"items": items}));
        })
        .unwrap();
        let shown = card(&config, "fr").unwrap();
        assert_eq!(shown["title"], "2 points semblent mal rangés");
        assert!(!shown.to_string().contains("Santé") && !shown.to_string().contains("Prednisolone"), "{shown}");
        let (moved, toast, card) = answer(&config, "move", &[], "fr").unwrap();
        assert_eq!((moved, toast.as_str(), card), (2, "Déplacé.", None));
        assert_eq!(std::fs::read_to_string(health.join("Domaines/Santé.md")).unwrap(), sante);
        let after = std::fs::read_to_string(root.join("Domaines/Famille.md")).unwrap();
        assert!(after.contains("Judo") && !after.contains("BipAgents"), "{after}");
    }
}
