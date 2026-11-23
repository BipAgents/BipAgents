//! task_frame.rs' tests (contracts/task-frame/cases.json; design-agents-proactifs.md §4.6), no model anywhere.

use super::*;
use crate::routines::{self, Ask, Maker, Routine, Schedule};

fn config_in(dir: &std::path::Path) -> Config {
    let mut config = Config::with_key("k");
    config.outbox.db_path = dir.join("bridge.db").to_string_lossy().into_owned();
    // the agents' language: French (none set: English)
    config.browser.language = "fr".into();
    for name in ["vie", "coach"] {
        config.agents.insert(
            name.into(),
            AgentConfig { name: name.into(), display_name: "Vie".into(), ..Default::default() },
        );
    }
    std::fs::create_dir_all(dir).unwrap();
    crate::dates::change(&config, |p| {
        p.time_zone = "Europe/Paris".into();
        p.routines = "Europe/Paris".into();
    })
    .unwrap();
    config
}

fn contract() -> Value {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/task-frame/cases.json");
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

const PARIS: Tz = chrono_tz::Europe::Paris;

fn at(text: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(text).unwrap().timestamp() as f64
}

/// A routine its agent made in the person's own conversation, on.
fn agents(config: &Config, name: &str, schedule: &str, ask: &Ask) -> Routine {
    let r = routines::create_framed(config, "vie", name, schedule, "Fais-le.", None, Maker::Agent, ask).unwrap();
    let card = r.card.clone().unwrap().id;
    routines::answer(config, "vie", &card, "once").unwrap()
}

fn asked() -> Ask {
    Ask { asked: true, ..Ask::default() }
}

#[test]
fn the_contract_s_texts_are_the_bridge_s() {
    let texts = &contract()["texts"];
    for lang in ["fr", "en", "de", "es"] {
        let c = &texts[lang];
        let s = |v: &Value| v.as_str().unwrap().to_string();
        for task in [Task::Routine, Task::Watcher] {
            let k = task.key();
            assert_eq!(fix_title(lang, task, "{agent}", "{name}"), s(&c["fix_top"][k]), "{lang} {k}");
            assert_eq!(fix_under_i(lang, task, "{agent}"), s(&c["fix_under_i"][k]), "{lang} {k}");
            assert_eq!(fix_summary(lang, task, "{agent}", "{name}"), s(&c["fix_summary"][k]), "{lang} {k}");
            assert_eq!(stopped_line(lang, task, "{name}"), s(&c["stopped"][k]), "{lang} {k}");
            assert_eq!(ended_line(lang, task, "{name}", "{end}"), s(&c["ended"][k]), "{lang} {k}");
            assert_eq!(keep_sentence(lang, task, "{name}", "{date}"), s(&c["keep"][k]), "{lang} {k}");
            let buttons: Vec<String> = c["keep_buttons"][k].as_array().unwrap().iter().map(s).collect();
            assert_eq!(keep_buttons(lang, task).to_vec(), buttons, "{lang} {k}");
            assert_eq!(end_words(lang, task, &End::Once, PARIS), s(&c["ends"]["once"][k]), "{lang} {k}");
            let never = &c["ends"]["never"];
            assert_eq!(end_words(lang, task, &End::Never, PARIS), s(&never[k]), "{lang} {k}");
        }
        let buttons: Vec<String> = c["fix_buttons"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(fix_buttons(lang).to_vec(), buttons, "{lang}");
        assert_eq!(end_words(lang, Task::Routine, &End::Plan, PARIS), s(&c["ends"]["plan"]), "{lang}");
        assert_eq!(end_words(lang, Task::Routine, &End::Count(10), PARIS), s(&c["ends"]["count"]).replace("{n}", "10"));
        for (reason, words) in c["reasons"].as_object().unwrap() {
            assert_eq!(reason_words(lang, reason), s(words), "{lang} {reason}");
        }
        assert_eq!(fix_since_line(lang, "{since}", "{reason}"), s(&c["fix_since"]), "{lang}");
        for kind in ["conversation", "boite", "silent"] {
            assert_eq!(deliver_words(lang, kind), s(&c["deliver"][kind]), "{lang} {kind}");
        }
        let start = routines::start_sentence_with(lang, "{agent}", "{name}", "{when}", Some("{end}"));
        assert_eq!(start, s(&c["start"]), "{lang}");
        assert_eq!(routines::repeat_sentence(lang, "{agent}", "{name}", "{when}", Some("{end}")), s(&c["repeat"]), "{lang}");
    }
}

#[test]
fn dates_and_clocks_in_the_four_languages() {
    let last = at("2026-11-30T23:59:59+01:00");
    assert_eq!(end_words("fr", Task::Routine, &End::Date(last), PARIS), "jusqu'au 30/11");
    assert_eq!(end_words("en", Task::Routine, &End::Date(last), PARIS), "until Nov 30");
    assert_eq!(end_words("de", Task::Routine, &End::Date(last), PARIS), "bis 30.11.");
    assert_eq!(end_words("es", Task::Routine, &End::Date(last), PARIS), "hasta el 30/11");
    assert_eq!(end_words("it", Task::Routine, &End::Date(last), PARIS), "until Nov 30");
    assert_eq!((clock("fr", 8, 0), clock("en", 8, 0), clock("de", 8, 0), clock("es", 8, 0)),
               ("vers 8 h".into(), "around 8 AM".into(), "gegen 8 Uhr".into(), "hacia las 8".into()));
    assert_eq!((clock("fr", 7, 30), clock("en", 15, 5), clock("de", 7, 30), clock("es", 19, 45)),
               ("vers 7 h 30".into(), "around 3:05 PM".into(), "gegen 7:30 Uhr".into(), "hacia las 19:45".into()));
    assert_eq!((clock("en", 0, 0), clock("en", 12, 0)), ("around 12 AM".into(), "around 12 PM".into()));
    let since = at("2026-11-21T07:00:00+01:00");
    assert_eq!(since_words("fr", since, PARIS), "le 21/11");
    assert_eq!(since_words("de", since, PARIS), "dem 21.11.");
    assert_eq!(fix_since_line("en", &since_words("en", since, PARIS), reason_words("en", "looping")),
               "Since Nov 21: it was going round in circles.");
}

#[test]
fn every_agent_made_routine_watcher_and_trigger_has_an_owner_an_end_and_a_place() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = agents(&config, "Météo", "every day at 7:30", &asked());
    let v = routines::view_of(&config, &r);
    assert_eq!(v["owner"], json!({"agent": "vie", "person": "you"}));
    assert_eq!(v["ends"]["kind"], "never");
    assert_eq!(v["deliver_to"]["kind"], "boite");
    assert_eq!((v["approx"].as_bool(), v["failures"].as_u64(), v["fix"].is_null()), (Some(true), Some(0), true));
    // Hermes' own `deliver` stays as an app 1.0 reads it
    assert_eq!(v["deliver"], "local");
    // made in a conversation: it answers there; silent: on its page
    let there = routines::create_framed(&config, "vie", "Agenda", "every day at 8:00", "x", Some("s9"), Maker::Agent, &asked())
        .unwrap();
    assert_eq!(routines::view_of(&config, &there)["deliver_to"], json!({"kind": "conversation", "session_id": "s9", "title": null}));
    let quiet = routines::create(&config, "vie", "Archive", "every 2h", "x", None, Maker::Person { muted: true }).unwrap();
    assert_eq!(routines::view_of(&config, &quiet)["deliver_to"]["kind"], "silent");
    // a watcher, its end on its card and its views
    let watch = connectors::Watch { every: "1h".into(), notify: "on_change".into(), until: Some(at("2026-11-30T23:59:59+01:00")),
                                    ..Default::default() };
    let shown = crate::watchers::watch_view(&watch, "fr");
    assert_eq!(shown["ends"], json!({"kind": "date", "at": at("2026-11-30T23:59:59+01:00"), "text": "jusqu'au 30/11"}));
    assert_eq!(crate::watchers::card_text(&watch), "Vérifie toutes les heures, ne réveille l'agent que si ça change, jusqu'au 30/11");
    let once = connectors::Watch { every: "1h".into(), notify: "on_change".into(), once: true, ..Default::default() };
    assert_eq!(crate::watchers::card_text(&once),
               "Vérifie toutes les heures, ne réveille l'agent que si ça change, une seule fois, puis il s'arrête");
    let plain = connectors::Watch { every: "1h".into(), notify: "on_change".into(), ..Default::default() };
    assert_eq!(crate::watchers::card_text(&plain), "Vérifie toutes les heures, ne réveille l'agent que si ça change");
    // a trigger
    let f = crate::triggers::Fields { name: Some("Raccourci".into()), prompt: Some("Note".into()),
                                      kind: Some(crate::triggers::LINK.into()), ..Default::default() };
    let t = crate::triggers::create(&config, "vie", &f, "person").unwrap();
    let tv = crate::triggers::view(&config, &t);
    assert_eq!((tv["owner"]["person"].as_str(), tv["ends"]["kind"].as_str(), tv["deliver"]["kind"].as_str()),
               (Some("you"), Some("never"), Some("boite")));
}

#[test]
fn a_once_yes_never_becomes_a_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let tomorrow = chrono::Utc::now() + chrono::Duration::days(1);
    let when = tomorrow.with_timezone(&PARIS).format("%Y-%m-%dT09:00").to_string();
    let made = routines::create_framed(&config, "vie", "Dentiste", &when, "Rappelle-moi le dentiste.", None, Maker::Agent,
                                       &asked()).unwrap();
    let card = made.card.clone().unwrap().id;
    // « Toujours » on its card is one « Oui », nothing kept for later cards
    let r = routines::answer(&config, "vie", &card, "always").unwrap();
    assert!(matches!(r.schedule, Schedule::Once { .. }) && r.repeat == Some(1) && !r.approx);
    assert_eq!(routines::end_of(&r), End::Once);
    assert!(!crate::approval_level::offers_always("routine_change") && !crate::approval_level::offers_always(KIND));
    // its agent making it repeat: a proposal, the routine as it was
    let args = json!({"action": "update", "name": "Dentiste", "schedule": "every day at 8:00"}).as_object().cloned().unwrap();
    let called = routines::call_asked(&config, "vie", &args, None, true);
    assert!(called.card.is_some());
    let r = routines::get(&config, &r.id).unwrap();
    assert!(matches!(r.schedule, Schedule::Once { .. }) && r.change.is_some());
}

#[test]
fn a_change_from_once_to_a_repeat_says_its_rhythm_and_end_in_the_summary() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let tomorrow = chrono::Utc::now() + chrono::Duration::days(1);
    let when = tomorrow.with_timezone(&PARIS).format("%Y-%m-%dT09:00").to_string();
    let r = agents(&config, "Météo", &when, &asked());
    let until = (chrono::Utc::now() + chrono::Duration::days(20)).with_timezone(&PARIS).date_naive();
    let args = json!({"action": "update", "name": "Météo", "schedule": "every day at 8:00", "until": until.to_string()})
        .as_object()
        .cloned()
        .unwrap();
    let (line, _) = routines::call_asked(&config, "vie", &args, None, true).card.unwrap();
    let day = until.format("%-d/%-m").to_string();
    assert_eq!(line, format!("Vie propose que la routine « Météo » se répète : chaque jour vers 8 h, jusqu'au {day}. D'accord ?"));
    let r = routines::get(&config, &r.id).unwrap();
    let item = routines::change_item(&config, &r, r.change.as_ref().unwrap());
    assert_eq!(item["bip_summary"], json!(line));
    assert!(item["bip_detail"].as_str().unwrap().contains(&format!("→ Chaque jour vers 8 h, jusqu'au {day}")), "{item:?}");
    // a repeat to another repeat: today's sentence
    let r2 = agents(&config, "Agenda", "every day at 7:00", &asked());
    let args = json!({"action": "update", "name": "Agenda", "schedule": "every day at 9:00"}).as_object().cloned().unwrap();
    let (line, _) = routines::call_asked(&config, "vie", &args, None, true).card.unwrap();
    assert_eq!(line, "Vie propose de modifier la routine « Agenda ». Garder ce changement ?");
    assert!(routines::get(&config, &r2.id).unwrap().change.is_some());
}

#[test]
fn approx_routines_fire_within_ten_minutes_stable_per_routine() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let shifts: Vec<f64> = (0..40).map(|i| approx_seconds(&format!("r{i}"))).collect();
    assert!(shifts.iter().all(|s| (-600.0..=600.0).contains(s) && s % 60.0 == 0.0), "{shifts:?}");
    assert!(shifts.iter().any(|s| *s != shifts[0]), "always the same shift");
    assert_eq!(approx_seconds("r7"), approx_seconds("r7"));
    let r = agents(&config, "Météo", "every day at 8:00", &asked());
    assert!(r.approx);
    let after = at("2026-11-23T12:00:00+01:00");
    let exact = routines::next_after(&r.schedule, after, PARIS).unwrap();
    let next = routines::next_run_of(&r, after, PARIS).unwrap();
    assert_eq!(next - exact, approx_seconds(&r.id));
    // its run « vers 8 h » then its next one: the next day, never twice the same day
    let then = routines::next_run_of(&r, next + 1.0, PARIS).unwrap();
    assert_eq!(then - next, 86_400.0);
    // its card says « vers »
    let card = routines::card_sentence(&config, &Routine { awaits_start: true, ..r.clone() });
    assert_eq!(card, "Vie a créé la routine « Météo » : chaque jour vers 8 h. La démarrer ?");
    // false when the person asked for an exact time; an interval never
    let exact_one = agents(&config, "Pilule", "every day at 8:00", &Ask { approx: Some(false), ..asked() });
    assert!(!exact_one.approx);
    assert!(!agents(&config, "Courriels", "every 2h", &asked()).approx);
}

#[test]
fn a_persons_routine_stays_exact() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Point", "every day at 8:00", "x", None, Maker::Person { muted: false }).unwrap();
    assert!(!r.approx);
    let after = at("2026-11-23T12:00:00+01:00");
    assert_eq!(routines::next_run_of(&r, after, PARIS), Some(at("2026-11-24T08:00:00+01:00")));
    assert_eq!(routines::view_of(&config, &r)["ends"], json!({"kind": "never", "text": "jusqu'à ce que tu l'arrêtes"}));
}

#[test]
fn a_first_failure_is_retried_once_in_silence() {
    let now = 1_000_000.0;
    let mut frame = Frame::default();
    assert_eq!(after_failure(&mut frame, now, true, Some(now + 7200.0)), After::Retry(now + RETRY_SECONDS));
    assert_eq!((frame.failures, frame.failing_since, frame.retry_at), (1, now, Some(now + RETRY_SECONDS)));
    // the retry fails too: today's notice
    assert_eq!(after_failure(&mut frame, now + 900.0, false, Some(now + 7200.0)), After::Tell);
    assert_eq!((frame.failures, frame.failing_since, frame.retry_at), (2, now, None));
    // its next run comes before the retry would: told at once
    let mut soon = Frame::default();
    assert_eq!(after_failure(&mut soon, now, true, Some(now + 600.0)), After::Tell);
    // the person's « Lancer maintenant »: told at once
    let mut asked = Frame::default();
    assert_eq!(after_failure(&mut asked, now, false, Some(now + 7200.0)), After::Tell);
    // a success starts the count again
    assert!(!after_success(&mut frame));
    assert_eq!(frame, Frame::default());
}

#[test]
fn three_failures_in_a_row_pause_the_routine_and_ask_repair_or_stop() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Courriels", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    let failed = |n: f64, scheduled| routines::failed(&config, &r.id, routines::Ended::Looping, now + n, scheduled);
    assert!(matches!(failed(0.0, true), Some(After::Retry(_))));
    assert_eq!(failed(900.0, false), Some(After::Tell));
    assert_eq!(failed(7200.0, true), Some(After::Repair));
    let r = routines::get(&config, &r.id).unwrap();
    assert!(!r.enabled && r.next_run.is_none());
    let fix = r.frame.fix.clone().unwrap();
    assert_eq!((fix.variant.as_str(), fix.failures, fix.since, fix.reason.as_str()), (REPAIR, 3, now, "looping"));
    let items = pending_items(&config, Some("vie"));
    let [item] = items.as_slice() else { panic!("{items:?}") };
    assert_eq!(item["run_id"], json!(format!("{RUN_PREFIX}{}", fix.id)));
    assert_eq!(item["choices"], json!(["once", "deny"]));
    assert_eq!(item["bip_choices"], json!(["repair", "stop"]));
    assert_eq!(item["bip_summary"], "La routine « Courriels » de Vie a échoué 3 fois de suite. La faire réparer ? (Non : elle s'arrête.)");
    assert_eq!(item["bip_fix"]["title"], "Vie · la routine « Courriels » a échoué 3 fois de suite");
    assert!(item["bip_fix"]["text"].as_str().unwrap().ends_with(" : elle tournait en rond."), "{item:?}");
    assert_eq!(crate::approval_level::classify_card(item), KIND);
    assert!(crate::approval_level::rule_of(KIND).critical);
    // a fourth failure (« Lancer maintenant » while it waits): told, no second card
    assert_eq!(failed(7300.0, false), Some(After::Tell));
    assert_eq!(pending_items(&config, Some("vie")).len(), 1);
}

#[test]
fn stop_pauses_never_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Courriels", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, now + f64::from(n), false);
    }
    let fix = routines::get(&config, &r.id).unwrap().frame.fix.unwrap();
    assert_eq!(settle(&config, "vie", &fix.id, false),
               Some(Settled { task: Task::Routine, id: r.id.clone(), variant: REPAIR.into(), yes: false }));
    let r = routines::get(&config, &r.id).unwrap();
    assert!(!r.enabled && r.frame.fix.is_none() && !r.frame.repairing);
    assert!(pending_items(&config, Some("vie")).is_empty());
    assert_eq!(settle(&config, "vie", &fix.id, true), None);
}

#[test]
fn repair_is_one_turn_and_only_through_existing_cards() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Courriels", "every 2h", "Regarde.", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::TooLong, now + f64::from(n), false);
    }
    let fix = routines::get(&config, &r.id).unwrap().frame.fix.unwrap();
    let settled = settle(&config, "vie", &fix.id, true).unwrap();
    assert_eq!((settled.task, settled.yes), (Task::Routine, true));
    let r = routines::get(&config, &r.id).unwrap();
    // still paused: nothing changes but through its card
    assert!(!r.enabled && r.frame.repairing && r.frame.fix.is_none());
    let note = routines::repair_note(&r);
    assert!(note.starts_with("<!--bip:repair\n") && note.contains("« Courriels »") && note.contains("failed 3 times in a row"));
    assert!(note.contains("routine_schedule") && note.contains("Change nothing any other way"), "{note}");
    // its agent's fix: a proposal, back on only with the person's « Oui »
    let args = json!({"action": "update", "name": "Courriels", "schedule": "every 3h"}).as_object().cloned().unwrap();
    let called = routines::call_asked(&config, "vie", &args, None, true);
    let (_, card) = called.card.unwrap();
    let r = routines::get(&config, &r.id).unwrap();
    assert!(!r.enabled && r.change.as_ref().is_some_and(|c| c.resumed));
    let r = routines::answer(&config, "vie", &card, "once").unwrap();
    assert!(r.enabled && !r.frame.repairing && r.frame.failures == 0 && r.next_run.is_some());
    assert_eq!(r.schedule, Schedule::Interval { minutes: 180 });
}

#[test]
fn no_answer_in_7_days_pauses_with_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Courriels", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, now + f64::from(n), false);
    }
    assert!(due(&config, now + 6.0 * DAY).is_empty());
    let found = due(&config, now + 7.0 * DAY + 10.0);
    assert_eq!(found, vec![Due::Line { agent: "vie".into(), task: Task::Routine, id: r.id.clone(),
                                       text: "« Courriels » est arrêtée : elle ne marchait plus depuis 7 jours. Elle reste sur sa page.".into() }]);
    let r = routines::get(&config, &r.id).unwrap();
    assert!(!r.enabled && r.frame.fix.is_none());
    assert!(due(&config, now + 8.0 * DAY).is_empty(), "said once");
}

#[test]
fn an_end_reached_goes_quietly_and_says_it_once_after_7_days() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let until = now + 3600.0;
    let long = routines::create_framed(&config, "vie", "Ancienne", "every 2h", "x", None, Maker::Person { muted: false },
                                       &Ask { until: Some(until), ..asked() }).unwrap();
    let short = routines::create_framed(&config, "vie", "Récente", "every 2h", "x", None, Maker::Person { muted: false },
                                        &Ask { until: Some(until), ..asked() }).unwrap();
    routines::update(&config, &long.id, |r| {
        r.created = now - 8.0 * DAY;
        r.last_run = Some(now - 600.0);
        Ok(())
    })
    .unwrap();
    routines::update(&config, &short.id, |r| {
        r.last_run = Some(now - 600.0);
        Ok(())
    })
    .unwrap();
    assert!(due(&config, now).is_empty());
    let found = due(&config, until + 1.0);
    let day = crate::task_frame::day_words("fr", until, PARIS);
    assert_eq!(found, vec![Due::Line { agent: "vie".into(), task: Task::Routine, id: long.id.clone(),
                                       text: format!("La routine « Ancienne » s'est arrêtée comme prévu (jusqu'au {day}).") }]);
    assert!(routines::all(&config, Some("vie")).is_empty());
    // a count end too; a one-time routine stays « terminée », as before
    let counted = routines::create_framed(&config, "vie", "Dix", "every 2h", "x", None, Maker::Person { muted: false },
                                          &Ask { repeat: Some(10), ..asked() }).unwrap();
    assert_eq!(routines::end_of(&counted), End::Count(10));
    routines::update(&config, &counted.id, |r| {
        r.repeat = Some(0);
        r.next_run = None;
        Ok(())
    })
    .unwrap();
    assert!(due(&config, until + 2.0).is_empty() && routines::get(&config, &counted.id).is_none());
}

#[test]
fn what_its_agent_sets_up_on_its_own_ends_after_90_days_and_asks_3_days_before() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let r = agents(&config, "Météo", "every day at 7:30", &Ask { asked: false, ..Ask::default() });
    let until = r.frame.until.unwrap();
    assert!(r.frame.default_end && (until - (now + DEFAULT_END_DAYS * DAY)).abs() < 5.0);
    assert_eq!(routines::end_of(&r), End::Date(until));
    // asked, no end: until the person stops it; a count or a date given: no default
    assert_eq!(routines::end_of(&agents(&config, "Agenda", "every day at 8:00", &asked())), End::Never);
    let given = agents(&config, "Dix", "every day at 9:00", &Ask { repeat: Some(10), asked: false, ..Ask::default() });
    assert!(!given.frame.default_end && given.frame.until.is_none());
    // 3 days before: the card « Garder 3 mois de plus ? », once
    assert!(due(&config, until - 4.0 * DAY).is_empty());
    let found = due(&config, until - 2.0 * DAY);
    assert!(matches!(found.as_slice(), [Due::Raise { push: false, .. }]), "{found:?}");
    assert!(due(&config, until - 2.0 * DAY + 60.0).is_empty());
    let items = pending_items(&config, Some("vie"));
    let [item] = items.as_slice() else { panic!("{items:?}") };
    assert_eq!(item["bip_choices"], json!(["keep", "stop"]));
    assert_eq!(item["bip_fix"]["variant"], KEEP);
    let date = day_words("fr", until, PARIS);
    assert_eq!(item["bip_summary"], json!(format!("La routine « Météo » s'arrête le {date}. La garder 3 mois de plus ?")));
    let id = item["request_id"].as_str().unwrap().to_string();
    // « Garder »: 90 days more
    assert!(settle(&config, "vie", &id, true).is_some());
    let kept = routines::get(&config, &r.id).unwrap();
    assert_eq!(kept.frame.until, Some(until + KEEP_DAYS_MORE * DAY));
    assert!(kept.frame.fix.is_none() && !kept.frame.keep_asked);
}

#[test]
fn a_once_watcher_and_a_watchers_end_are_read_from_its_watch() {
    let mut map = json!({"every": "1h", "notify": "on_change", "once": true}).as_object().cloned().unwrap();
    let w = crate::watchers::parse_watch_in(&map, PARIS).unwrap();
    assert!(w.once && w.until.is_none());
    map.insert("until".into(), json!("2026-11-30"));
    map.insert("once".into(), json!(false));
    let w = crate::watchers::parse_watch_in(&map, PARIS).unwrap();
    assert_eq!(w.until, Some(at("2026-11-30T23:59:59+01:00")));
    map.insert("until".into(), json!("2020-01-01"));
    assert!(crate::watchers::parse_watch_in(&map, PARIS).is_err());
}

// -- review-task-frame (23/11): red tests of the review (briefs/review-task-frame.md), not the branch's ------------------

/// « Arrêter » pauses, never deletes; « Garder 3 mois de plus ? » 3 days before a default end. A routine its agent made
/// on its own, stopped after 3 failures: no « Garder ? » is asked (it is paused), and its default end deletes it.
#[test]
fn review_stop_never_deletes_even_at_its_default_end() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = agents(&config, "Veille", "every day at 7:30", &Ask { asked: false, ..Ask::default() });
    let until = r.frame.until.unwrap();
    assert!(r.frame.default_end);
    let then = until - 10.0 * DAY;
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, then + f64::from(n), false);
    }
    let fix = routines::get(&config, &r.id).unwrap().frame.fix.unwrap();
    assert!(settle(&config, "vie", &fix.id, false).is_some(), "« Arrêter »");
    let before = due(&config, until - 2.0 * DAY);
    due(&config, until + 60.0);
    assert!(
        routines::get(&config, &r.id).is_some(),
        "« Arrêter : elle ne tourne plus et reste sur sa page », then deleted at its default end, no « Garder ? » asked \
         ({before:?})"
    );
}

/// A count end reached, for what ran more than 7 days: « …s'est arrêtée comme prévu (10 fois) », never « (0 fois) ».
#[test]
fn review_a_count_end_reached_says_its_count_not_zero() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let r = routines::create_framed(&config, "vie", "Dix", "every 2h", "x", None, Maker::Person { muted: false },
                                    &Ask { repeat: Some(10), ..asked() }).unwrap();
    // its 10 runs made (fire_via: the last one leaves repeat 0, no next run)
    routines::update(&config, &r.id, |x| {
        (x.created, x.last_run, x.repeat, x.next_run) = (now - 20.0 * DAY, Some(now - 600.0), Some(0), None);
        Ok(())
    })
    .unwrap();
    let found = due(&config, now);
    let [Due::Line { text, .. }] = found.as_slice() else { panic!("{found:?}") };
    assert!(!text.contains("(0 fois)"), "{text}");
}

/// contracts/task-frame tools.routine_schedule: « An `until` in the past, or before its first run, is refused ».
#[test]
fn review_an_until_before_its_first_run_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let schedule = "0 8 1 1 *";
    let first = routines::next_after(&routines::parse(schedule, PARIS).unwrap(), now, PARIS).unwrap();
    if first - now < 7200.0 {
        return;
    }
    let until = now + (first - now) / 2.0;
    let made = routines::create_framed(&config, "vie", "Bonne année", schedule, "x", None, Maker::Agent,
                                       &Ask { until: Some(until), ..asked() });
    assert!(made.is_err(), "an end before its first run, accepted: it never runs, then goes with its card");
}

/// The last run of a routine with a number of runs fails: its silent retry (15 min later) must not be dropped by its
/// end at the clock's next look (20 s later), nor the failure go untold.
#[test]
fn review_the_retry_of_a_last_counted_run_is_not_dropped_by_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create_framed(&config, "vie", "Deux", "every 2h", "x", None, Maker::Person { muted: false },
                                    &Ask { repeat: Some(2), ..asked() }).unwrap();
    let now = py::time_time();
    routines::update(&config, &r.id, |x| {
        (x.repeat, x.next_run, x.last_run) = (Some(0), None, Some(now));
        Ok(())
    })
    .unwrap();
    assert!(matches!(routines::failed(&config, &r.id, routines::Ended::Failed, now, true), Some(After::Retry(_))));
    due(&config, now + 20.0);
    let kept = routines::get(&config, &r.id);
    assert!(kept.as_ref().is_some_and(|k| k.frame.retry_at.is_some()), "its retry dropped with the routine: {kept:?}");
}

/// « Garder 3 mois de plus ? » replaced by « Réparer / Arrêter » (a failure 2 days before its end): once repaired, it
/// is asked again before the end, not lost.
#[test]
fn review_a_keep_card_replaced_by_a_repair_is_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = agents(&config, "Météo", "every day at 7:30", &Ask { asked: false, ..Ask::default() });
    let until = r.frame.until.unwrap();
    assert!(matches!(due(&config, until - 2.5 * DAY).as_slice(), [Due::Raise { .. }]));
    let then = until - 2.4 * DAY;
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, then + f64::from(n), false);
    }
    assert_eq!(routines::get(&config, &r.id).unwrap().frame.fix.unwrap().variant, REPAIR);
    // the person turns it back on (its page): the count starts again
    routines::update(&config, &r.id, |x| {
        x.enabled = true;
        resumed(&mut x.frame);
        Ok(())
    })
    .unwrap();
    let found = due(&config, until - 2.0 * DAY);
    assert!(matches!(found.as_slice(), [Due::Raise { .. }]), "no « Garder ? » any more before its end: {found:?}");
}
