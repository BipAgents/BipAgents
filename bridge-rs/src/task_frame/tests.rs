//! task_frame.rs' tests (contracts/task-frame/cases.json; design-agents-proactifs.md §4.6), no model anywhere.

use super::*;
use chrono::{Datelike, Timelike};
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

/// The day `days` from today (reviews C13: never a fixed date, never one that depends on the day the tests run): its
/// last second on the Paris clock.
fn day_from_today(days: i64) -> chrono::DateTime<Tz> {
    let day = chrono::Utc::now().with_timezone(&PARIS).date_naive() + chrono::Duration::days(days);
    PARIS.from_local_datetime(&day.and_hms_opt(23, 59, 59).unwrap()).unwrap()
}

/// `d` as the bridge writes a day: « 30/11 » (fr, es), « Nov 30 » (en), « 30.11. » (de), and the year when it is not
/// this year (review C9).
fn day_as_written(lang: &str, d: chrono::DateTime<Tz>) -> String {
    let this_year = d.year() == chrono::Utc::now().with_timezone(&PARIS).year();
    match (lang, this_year) {
        ("en", true) => d.format("%b %-d").to_string(),
        ("en", false) => d.format("%b %-d, %Y").to_string(),
        ("de", true) => format!("{}.{}.", d.day(), d.month()),
        ("de", false) => format!("{}.{}.{}", d.day(), d.month(), d.year()),
        (_, true) => format!("{}/{}", d.day(), d.month()),
        (_, false) => format!("{}/{}/{}", d.day(), d.month(), d.year()),
    }
}

fn epoch(d: chrono::DateTime<Tz>) -> f64 {
    d.timestamp() as f64
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
            assert_eq!(repair_failed_line(lang, task, "{name}"), s(&c["repair_failed"][k]), "{lang} {k}");
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
    let d = day_from_today(7);
    let last = epoch(d);
    let (dm, en) = (day_as_written("fr", d), day_as_written("en", d));
    assert_eq!(end_words("fr", Task::Routine, &End::Date(last), PARIS), format!("jusqu'au {dm}"));
    assert_eq!(end_words("en", Task::Routine, &End::Date(last), PARIS), format!("until {en}"));
    assert_eq!(end_words("de", Task::Routine, &End::Date(last), PARIS), format!("bis {}", day_as_written("de", d)));
    assert_eq!(end_words("es", Task::Routine, &End::Date(last), PARIS), format!("hasta el {dm}"));
    assert_eq!(end_words("it", Task::Routine, &End::Date(last), PARIS), format!("until {en}"));
    assert_eq!((clock("fr", 8, 0), clock("en", 8, 0), clock("de", 8, 0), clock("es", 8, 0)),
               ("vers 8 h".into(), "around 8 AM".into(), "gegen 8 Uhr".into(), "hacia las 8".into()));
    assert_eq!((clock("fr", 7, 30), clock("en", 15, 5), clock("de", 7, 30), clock("es", 19, 45)),
               ("vers 7 h 30".into(), "around 3:05 PM".into(), "gegen 7:30 Uhr".into(), "hacia las 19:45".into()));
    assert_eq!((clock("en", 0, 0), clock("en", 12, 0)), ("around 12 AM".into(), "around 12 PM".into()));
    let s = day_from_today(-2);
    let since = epoch(s) - 3600.0 * 16.0;
    assert_eq!(since_words("fr", since, PARIS), format!("le {}", day_as_written("fr", s)));
    assert_eq!(since_words("de", since, PARIS), format!("dem {}", day_as_written("de", s)));
    assert_eq!(fix_since_line("en", &since_words("en", since, PARIS), reason_words("en", "looping")),
               format!("Since {}: it was going round in circles.", day_as_written("en", s)));
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
    let d = day_from_today(7);
    let dm = day_as_written("fr", d);
    let watch = connectors::Watch { every: "1h".into(), notify: "on_change".into(), until: Some(epoch(d)),
                                    ..Default::default() };
    let shown = crate::watchers::watch_view_in(&watch, "fr", PARIS);
    assert_eq!(shown["ends"], json!({"kind": "date", "at": epoch(d), "text": format!("jusqu'au {dm}")}));
    assert_eq!(crate::watchers::card_text_in(&watch, PARIS),
               format!("Vérifie toutes les heures, ne réveille l'agent que si ça change, jusqu'au {dm}"));
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
    let last = PARIS.from_local_datetime(&until.and_hms_opt(23, 59, 59).unwrap()).unwrap();
    let day = day_words("fr", epoch(last), PARIS);
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
    // on a clock without summer time (UTC): its run « vers 8 h » then its next one, the next day. A fixed hour far
    // from 8 h (2026-11-18 03:00 UTC): from the real clock, between 8 h and 8 h + its shift, the exact 8 h is today's
    // and its run « vers 8 h » tomorrow's (int-1.1d, 24/11 at 09:14 Paris: left 86340, right -60)
    let (utc, after) = (Tz::UTC, 1_794_970_800.0);
    let exact = routines::next_after(&r.schedule, after - 900.0, utc).unwrap();
    let next = routines::next_run_of(&r, after - 900.0, utc).unwrap();
    assert_eq!(next - exact, approx_seconds(&r.id));
    // never twice the same day
    let then = routines::next_run_of(&r, next + 1.0, utc).unwrap();
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
    let now = py::time_time();
    let next = routines::next_run_of(&r, now, Tz::UTC).unwrap();
    assert_eq!(Some(next), routines::next_after(&r.schedule, now, Tz::UTC));
    let at = Tz::UTC.timestamp_opt(next as i64, 0).unwrap();
    assert_eq!((at.hour(), at.minute(), at.second()), (8, 0, 0));
    assert!(next > now && next - now <= DAY);
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
    // (its first run at +2 h, before its end: C10 of the review refuses an end before the first run)
    let until = now + 3.0 * 3600.0;
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
    let d = day_from_today(7);
    map.insert("until".into(), json!(d.format("%Y-%m-%d").to_string()));
    map.insert("once".into(), json!(false));
    let w = crate::watchers::parse_watch_in(&map, PARIS).unwrap();
    assert_eq!(w.until, Some(epoch(d)));
    let past = chrono::Utc::now().with_timezone(&PARIS).date_naive() - chrono::Duration::days(2);
    map.insert("until".into(), json!(past.format("%Y-%m-%d").to_string()));
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

// -- the review's corrections (23/11, briefs/review-task-frame.md), red first ----------------------------------------

/// A watcher of its agent's own, with its default end.
fn watcher_of_its_own(config: &Config, id: &str, until: f64, enabled: bool) {
    connectors::change(config, |store| {
        store.agents.entry("vie".into()).or_default().push(connectors::Connector {
            id: id.into(),
            name: "Prix du vélo".into(),
            slug: "prix_du_velo".into(),
            status: connectors::APPROVED.into(),
            enabled,
            approved_at: Some(crate::vault::now()),
            watch: Some(connectors::Watch { every: "1h".into(), notify: "on_change".into(), until: Some(until),
                                            default_end: true, ..Default::default() }),
            ..Default::default()
        });
    })
    .unwrap();
}

/// B1 (watchers): a watcher switched off (« Arrêter », or its switch) is never removed by its default end.
#[test]
fn b1_a_paused_watcher_never_goes_at_its_default_end_and_garder_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    watcher_of_its_own(&config, "c_veille", now + 2.0 * DAY, false);
    let found = due(&config, now);
    assert!(matches!(found.as_slice(), [Due::Raise { .. }]), "« Garder ? » asked though it is paused: {found:?}");
    let gone = due(&config, now + 3.0 * DAY);
    assert!(!gone.iter().any(|d| matches!(d, Due::Remove { .. })), "a paused watcher removed at its default end: {gone:?}");
}

/// C3 (b): a retry missed while the bridge was down is not run late (as a missed run).
#[test]
fn c3b_a_retry_missed_while_the_bridge_was_down_is_not_run_late() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Matin", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    routines::failed(&config, &r.id, routines::Ended::Failed, now, true);
    let r = routines::get(&config, &r.id).unwrap();
    let at = r.frame.retry_at.unwrap();
    assert!(!routines::retry_missed(&r, at + 300.0));
    assert!(routines::retry_missed(&r, at + 3.0 * 3600.0));
}

/// C3 (d): a pause drops a retry waiting (else it would run at the resume, besides its schedule).
#[test]
fn c3d_a_pause_drops_a_waiting_retry() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Matin", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    routines::failed(&config, &r.id, routines::Ended::Failed, py::time_time(), true);
    assert!(routines::get(&config, &r.id).unwrap().frame.retry_at.is_some());
    let args = json!({"action": "pause", "name": "Matin"}).as_object().cloned().unwrap();
    routines::call_asked(&config, "vie", &args, None, true);
    assert_eq!(routines::get(&config, &r.id).unwrap().frame.retry_at, None);
}

/// C4: a run that works while the routine is paused with its card (« Lancer maintenant », its agent's `run`) keeps the
/// card: only the person's answer takes it; « Réparer » answered stays so.
#[test]
fn c4_a_success_while_paused_keeps_its_card() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Courriels", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, now + f64::from(n), false);
    }
    assert!(routines::succeeded(&config, &r.id).is_none(), "its card withdrawn by a run that worked");
    let kept = routines::get(&config, &r.id).unwrap();
    assert!(!kept.enabled && kept.frame.fix.is_some() && kept.frame.failures == 0, "{kept:?}");
    let fix = kept.frame.fix.unwrap();
    settle(&config, "vie", &fix.id, true).unwrap();
    routines::succeeded(&config, &r.id);
    assert!(routines::get(&config, &r.id).unwrap().frame.repairing, "« Réparer » undone by a run that worked");
}

/// C5: « Réparer » that could not run (no conversation, a failed turn, the credit, nothing said) says so in one line
/// and asks again.
#[test]
fn c5_a_repair_that_fails_says_so_and_asks_again() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Courriels", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let now = py::time_time();
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, now + f64::from(n), false);
    }
    let fix = routines::get(&config, &r.id).unwrap().frame.fix.unwrap();
    settle(&config, "vie", &fix.id, true).unwrap();
    let text = routines::repair_failed(&config, &r.id, now + 60.0).unwrap();
    assert_eq!(text, "La routine « Courriels » n'a pas pu être réparée pour l'instant : elle reste en pause.");
    let again = routines::get(&config, &r.id).unwrap();
    assert!(!again.frame.repairing && !again.enabled);
    assert_eq!(again.frame.fix.map(|f| f.variant), Some(REPAIR.to_string()));
    // a watcher's
    watcher_of_its_own(&config, "c_veille", now + 30.0 * DAY, true);
    connectors::change(&config, |s| {
        let c = s.agents.get_mut("vie").unwrap().iter_mut().find(|c| c.id == "c_veille").unwrap();
        c.frame.fix = Some(Fix {
            id: "w1".into(),
            at: now,
            variant: REPAIR.into(),
            failures: 4,
            reason: "check".into(),
            since: now,
            ..Fix::default()
        });
    })
    .unwrap();
    settle(&config, "vie", "w1", true).unwrap();
    let said = crate::watchers::repair_failed(&config, "vie", "c_veille", now + 60.0).unwrap();
    assert_eq!(said, "Le veilleur « Prix du vélo » n'a pas pu être réparé pour l'instant.");
    assert!(connectors::get(&config, "vie", "c_veille").unwrap().frame.fix.is_some());
}

/// C7: the card of a routine a family member asked for is hers (her bridge shows it and answers it), never the creator's.
#[test]
fn c7_a_members_routine_card_is_hers() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = routines::create(&config, "vie", "Menus", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    routines::update(&config, &r.id, |x| {
        x.by = Some("lea".into());
        Ok(())
    })
    .unwrap();
    let now = py::time_time();
    for n in 0..3 {
        routines::failed(&config, &r.id, routines::Ended::Failed, now + f64::from(n), false);
    }
    let fix = routines::get(&config, &r.id).unwrap().frame.fix.unwrap();
    assert!(pending_items(&config, Some("vie")).is_empty(), "a member's routine card in the creator's list");
    let hers = routines::member_items(&config, "vie", "lea");
    assert!(hers.iter().any(|c| c["run_id"] == json!(format!("{RUN_PREFIX}{}", fix.id))), "{hers:?}");
    assert_eq!(fix_by(&config, "vie", &fix.id).as_deref(), Some("lea"));
}

/// C9: an end its agent gives on its own never passes its default 90 days (a date, or a number of runs); one the person
/// asked for stays; a day not of this year says its year.
#[test]
fn c9_an_end_its_agent_sets_on_its_own_never_passes_90_days() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let far = now + 400.0 * DAY;
    let own = agents(&config, "Loin", "every day at 8:00", &Ask { until: Some(far), asked: false, ..Ask::default() });
    let until = own.frame.until.unwrap();
    assert!(until <= now + DEFAULT_END_DAYS * DAY + 5.0 && own.frame.default_end, "{:?}", own.frame);
    let many = agents(&config, "Souvent", "every 2h", &Ask { repeat: Some(100_000), asked: false, ..Ask::default() });
    assert!(many.frame.until.is_some_and(|u| u <= now + DEFAULT_END_DAYS * DAY + 5.0), "{:?}", many.frame);
    let few = agents(&config, "Dix lundis", "every monday 18h", &Ask { repeat: Some(10), asked: false, ..Ask::default() });
    assert_eq!(routines::end_of(&few), End::Count(10));
    let asked_far = agents(&config, "Demandé", "every day at 9:00", &Ask { until: Some(far), ..asked() });
    assert_eq!(asked_far.frame.until, Some(far));
    let year = PARIS.timestamp_opt(far as i64, 0).unwrap().year();
    assert!(day_words("fr", far, PARIS).ends_with(&format!("/{year}")), "{}", day_words("fr", far, PARIS));
    assert!(day_words("en", far, PARIS).ends_with(&format!(", {year}")), "{}", day_words("en", far, PARIS));
    assert!(day_words("de", far, PARIS).ends_with(&format!(".{year}")), "{}", day_words("de", far, PARIS));
    // a watcher of its own: the same, « once » too (R2)
    let mut w = connectors::Watch { every: "1h".into(), notify: "on_change".into(), once: true, ..Default::default() };
    connectors::watch_frame(&mut w, false, now);
    assert!(w.default_end && w.until.is_some_and(|u| u <= now + DEFAULT_END_DAYS * DAY + 1.0), "{w:?}");
    let mut asked_w = connectors::Watch { every: "1h".into(), notify: "on_change".into(), until: Some(far), ..Default::default() };
    connectors::watch_frame(&mut asked_w, true, now);
    assert_eq!((asked_w.until, asked_w.default_end), (Some(far), false));
}

/// C12: a flag written as text says what it says (« false » is false).
#[test]
fn c12_a_flag_written_as_text_is_read_as_said() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let map = json!({"every": "1h", "notify": "on_change", "once": "false"}).as_object().cloned().unwrap();
    assert!(!crate::watchers::parse_watch_in(&map, PARIS).unwrap().once);
    let map = json!({"every": "1h", "notify": "on_change", "once": "true"}).as_object().cloned().unwrap();
    assert!(crate::watchers::parse_watch_in(&map, PARIS).unwrap().once);
    let args = json!({"action": "create", "name": "Pile", "schedule": "every day at 8:00", "prompt": "x", "approx": "false"})
        .as_object()
        .cloned()
        .unwrap();
    routines::call_asked(&config, "vie", &args, None, true);
    let made = routines::all(&config, Some("vie")).into_iter().find(|r| r.name == "Pile").unwrap();
    assert!(!made.approx);
}

/// R8: « vers » never moves a run to the day before (0:00 shifted back) nor past midnight.
#[test]
fn r8_vers_never_crosses_midnight() {
    let utc = Tz::UTC;
    let back = (0..200).map(|i| format!("r{i}")).find(|id| approx_seconds(id) < 0.0).unwrap();
    let r = Routine {
        id: back,
        approx: true,
        ..routines::create(&config_in(tempfile::tempdir().unwrap().path()), "vie", "Minuit", "every day at 0:00", "x",
                           None, Maker::Person { muted: false }).unwrap()
    };
    let next = routines::next_run_of(&r, py::time_time(), utc).unwrap();
    let at = utc.timestamp_opt(next as i64, 0).unwrap();
    assert_eq!(at.hour(), 0, "« vers 0 h » run at {at}");
}

/// R12: a watcher « once » that reaches its date says the date.
#[test]
fn r12_a_once_watcher_at_its_date_says_the_date() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    connectors::change(&config, |store| {
        store.agents.entry("vie".into()).or_default().push(connectors::Connector {
            id: "c_billets".into(),
            name: "Billets".into(),
            status: connectors::APPROVED.into(),
            enabled: true,
            approved_at: Some(crate::vault::now()),
            watch: Some(connectors::Watch { every: "1h".into(), notify: "on_change".into(), once: true,
                                            until: Some(now + 60.0), ..Default::default() }),
            ..Default::default()
        });
    })
    .unwrap();
    crate::watchers::change(&config, "vie", "c_billets", |rt| rt.checks = 3);
    let found = due(&config, now + 9.0 * DAY);
    let line = found.iter().find_map(|d| match d {
        Due::Line { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert!(line.as_deref().is_some_and(|t| t.contains("jusqu'au")), "{found:?}");
}

// -- review 2 of task-frame (23/11): red tests of the review (briefs/review-task-frame.md), not the branch's --------

/// B1 (« Remise en marche après sa date : 3 jours de plus », never gone at once): a watcher of its agent's own,
/// switched off past its default end, then turned back on by a « Oui » to its agent's card (tool_asks.rs
/// `Switch::Connector`, connectors.rs `set_enabled`), not by the app's switch (`switch_route`, which gives it 3 days
/// more): deleted, with its program, at the clock's next look (20 s).
#[test]
fn review2_a_watcher_back_on_by_a_card_after_its_default_end_is_not_deleted_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    // off 2 days before its default end: « Garder ? » asked then (B1), not answered
    watcher_of_its_own(&config, "c_veille", now + 2.0 * DAY, false);
    assert!(matches!(due(&config, now).as_slice(), [Due::Raise { .. }]));
    // a week later, still off: still there (B1)
    let later = now + 9.0 * DAY;
    assert!(!due(&config, later).iter().any(|d| matches!(d, Due::Remove { .. })));
    // « Oui » to its agent's card « Connecteur « Prix du vélo » »: on again
    connectors::set_enabled(&config, "vie", "c_veille", true).unwrap();
    let found = due(&config, later + 20.0);
    assert!(
        !found.iter().any(|d| matches!(d, Due::Remove { .. })),
        "turned back on by a « Oui », deleted at the clock's next look: {found:?}"
    );
}

/// C9 (an end its agent sets on its own: within 90 days): a one-time routine its agent makes repeat on its own, with
/// a number of runs that goes far past 90 days (`update`, `repeat`), gets neither an end within 90 days nor
/// « Garder ? » (`create` caps the same count: c9_an_end_its_agent_sets_on_its_own_never_passes_90_days).
#[test]
fn review2_a_count_its_agent_proposes_on_its_own_never_passes_90_days_either() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let tomorrow = chrono::Utc::now() + chrono::Duration::days(1);
    let tomorrow = tomorrow.with_timezone(&PARIS).format("%Y-%m-%dT09:00").to_string();
    agents(&config, "Dentiste", &tomorrow, &asked());
    let args = json!({"action": "update", "name": "Dentiste", "schedule": "every 2h", "repeat": 100_000})
        .as_object()
        .cloned()
        .unwrap();
    // its own idea (a routine's turn, not the person's): `asked` false
    let (_, card) = routines::call_asked(&config, "vie", &args, None, false).card.unwrap();
    let now = py::time_time();
    let after = routines::answer(&config, "vie", &card, "once").unwrap();
    assert!(
        after.frame.until.is_some_and(|u| u <= now + DEFAULT_END_DAYS * DAY + 5.0) && after.frame.default_end,
        "100 000 runs every 2 h (about 23 years), no end within 90 days, no « Garder ? »: {:?} repeat {:?}",
        after.frame,
        after.repeat
    );
}

// -- review 2 (23/11, 13:52): red first ------------------------------------------------------------------------------

/// B1 bis: every way a watcher comes back on (its switch, a « Oui » to its agent's card, the approval of a change) gives
/// a default end passed meanwhile its 3 days more (`connectors::switched_on`).
#[test]
fn r2_every_way_back_on_gives_a_passed_default_end_three_days() {
    let now = py::time_time();
    let mut c = connectors::Connector {
        id: "c_veille".into(),
        enabled: false,
        watch: Some(connectors::Watch { every: "1h".into(), notify: "on_change".into(), until: Some(now - DAY),
                                        default_end: true, ..Default::default() }),
        ..Default::default()
    };
    connectors::switched_on(&mut c, now);
    assert!(c.enabled);
    assert_eq!(c.watch.as_ref().and_then(|w| w.until), Some(now + KEEP_ASK_DAYS * DAY));
    // an end the person gave stays
    let mut asked = connectors::Connector {
        watch: Some(connectors::Watch { every: "1h".into(), notify: "on_change".into(), until: Some(now - DAY),
                                        ..Default::default() }),
        ..Default::default()
    };
    connectors::switched_on(&mut asked, now);
    assert_eq!(asked.watch.as_ref().and_then(|w| w.until), Some(now - DAY));
}

/// N1 (review 2): a « once » watcher of its agent's own that told its person is done: paused, no default end left, no
/// « Garder 3 mois de plus ? », a « Garder ? » waiting taken.
#[test]
fn r2_a_once_watcher_done_is_never_asked_garder() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    watcher_of_its_own(&config, "c_billets", now + 10.0 * DAY, true);
    connectors::change(&config, |s| {
        let c = s.agents.get_mut("vie").unwrap().iter_mut().find(|c| c.id == "c_billets").unwrap();
        if let Some(w) = c.watch.as_mut() {
            w.once = true;
        }
    })
    .unwrap();
    crate::watchers::once_told(&config, "vie", "c_billets");
    let done = connectors::get(&config, "vie", "c_billets").unwrap();
    let w = done.watch.clone().unwrap();
    assert!(!done.enabled && !w.default_end && w.until.is_none(), "{done:?}");
    assert!(due(&config, now + 8.0 * DAY).is_empty(), "« Garder ? » for a once watcher done");
}

// -- Alex's choices of 24/11 (passation-pro textes-1-1/decisions.md), red first -----------------------------------------

/// A watcher of its agent's own « en panne », its card « Réparer / Arrêter » made at `at`.
fn broken_watcher(config: &Config, id: &str, at: f64) {
    watcher_of_its_own(config, id, at + 60.0 * DAY, true);
    connectors::change(config, |s| {
        let c = s.agents.get_mut("vie").unwrap().iter_mut().find(|c| c.id == id).unwrap();
        c.frame.fix = Some(Fix {
            id: "f_panne".into(),
            at,
            variant: REPAIR.into(),
            since: at - DAY,
            failures: 3,
            reason: "check".into(),
            ..Fix::default()
        });
    })
    .unwrap();
}

/// Point 1: tf-ligne-panne-veilleur-remplacee (Alex: « prévenir avant de l'arrêter, et envoyer une carte pour continuer ou
/// non »): a watcher's card unanswered is asked again, pushed, saying the day it stops; only then, and never before
/// that day, is it switched off.
#[test]
fn a1_a_watcher_is_told_before_it_stops_with_a_card_to_go_on_or_not() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    broken_watcher(&config, "c_panne", now);
    assert!(due(&config, now + 3.9 * DAY).is_empty());
    let warned = due(&config, now + 4.0 * DAY + 10.0);
    let [Due::Raise { run_id, line, push: true, .. }] = warned.as_slice() else {
        panic!("no warning card: {warned:?}")
    };
    assert_ne!(run_id, "bip-fix-f_panne", "the same card, not placed again");
    let stops = day_words("fr", now + NO_ANSWER_DAYS * DAY, PARIS);
    assert!(line.ends_with(&format!("Sans réponse, il s'arrête le {stops}.")), "{line}");
    let c = connectors::get(&config, "vie", "c_panne").unwrap();
    assert!(c.enabled, "still on: the person decides");
    let [item] = pending_items(&config, Some("vie")).try_into().unwrap();
    assert_eq!(item["bip_choices"], json!(["repair", "stop"]));
    assert!(
        item["bip_fix"]["text"].as_str().unwrap().ends_with(&format!(" Sans réponse, il s'arrête le {stops}.")),
        "{item:?}"
    );
    assert!(item["bip_summary"]
        .as_str()
        .unwrap()
        .ends_with(&format!("(Non : il s'arrête.) Sans réponse, il s'arrête le {stops}.")));
    assert!(item["bip_fix"]["stops_at"].as_f64().is_some());
    assert!(due(&config, now + 6.9 * DAY).is_empty(), "warned once");
    let found = due(&config, now + 7.0 * DAY + 10.0);
    assert!(matches!(found.as_slice(), [Due::Line { task: Task::Watcher, .. }]), "{found:?}");
    assert!(!connectors::get(&config, "vie", "c_panne").unwrap().enabled);
    // a bridge down meanwhile: warned late, the 3 days the warning says are kept
    broken_watcher(&config, "c_tard", now);
    let late = due(&config, now + 9.0 * DAY);
    assert!(matches!(late.as_slice(), [Due::Raise { push: true, .. }]), "stopped without its warning: {late:?}");
    assert!(connectors::get(&config, "vie", "c_tard").unwrap().enabled);
    assert!(due(&config, now + 11.9 * DAY).is_empty());
    assert!(matches!(due(&config, now + 12.0 * DAY + 10.0).as_slice(), [Due::Line { .. }]));
}

/// Point 1: Its agent never removes a watcher on its own (outside a turn of its person's): only its person stops it.
#[test]
fn a1_the_agent_alone_never_removes_a_watcher() {
    let src =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/connectors.rs")).unwrap();
    let remove = &src[src.find("        REMOVE => {").unwrap()..];
    let remove = &remove[..remove.find("        LIST =>").unwrap()];
    assert!(
        remove.contains("asked_by_person(state, &agent.name, REMOVE)")
            && remove.contains("only your person stops a watcher"),
        "connector_remove removes a watcher its person never asked to stop: {remove}"
    );
}

/// Point 2: tf-carte-commune-ia-eteinte B: the AI of an agent doesn't answer (its model's server off): ONE card for the
/// agent, « son IA ne répond pas », its routines named; « Réessayer » turns them all back on, « Arrêter » leaves them
/// paused; a family member's routine keeps its own card.
#[test]
fn a2_one_card_per_agent_when_its_ai_does_not_answer() {
    assert!(ai_silent("APIConnectionError: Connection error."));
    assert!(ai_silent("Error code: 503 - Service Unavailable"));
    assert!(ai_silent("httpx.ConnectError: [Errno 111] Connection refused"));
    assert!(ai_silent("Request timed out."));
    assert!(ai_silent("model unreachable"));
    assert!(!ai_silent("a tool crashed"));
    assert!(!ai_silent("Error code: 400 - context length 5030 tokens"));
    assert!(!ai_silent(""));
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let one =
        routines::create(&config, "vie", "Courriels", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let two = routines::create(&config, "vie", "Météo", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let other =
        routines::create(&config, "coach", "Sport", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    let fail = |id: &str, n: f64| routines::failed_as(&config, id, routines::Ended::Failed, true, now + n, false);
    fail(&one.id, 0.0);
    fail(&one.id, 1.0);
    assert_eq!(fail(&one.id, 2.0), Some(After::Repair));
    fail(&two.id, 3.0);
    fail(&two.id, 4.0);
    assert_eq!(fail(&two.id, 5.0), Some(After::Joined), "a second card for the same agent");
    for _ in 0..3 {
        fail(&other.id, 6.0);
    }
    let items = pending_items(&config, Some("vie"));
    let [item] = items.as_slice() else { panic!("one card per routine: {items:?}") };
    assert_eq!(item["bip_fix"]["variant"], AI);
    assert_eq!(item["bip_fix"]["title"], "Vie · son IA ne répond pas");
    assert_eq!(item["bip_fix"]["buttons"], json!(["Réessayer", "Arrêter"]));
    assert_eq!(item["bip_fix"]["routines"], json!(["Courriels", "Météo"]));
    assert!(
        item["bip_fix"]["text"]
            .as_str()
            .unwrap()
            .ends_with(" : ses routines n'ont pas pu tourner (« Courriels », « Météo »)."),
        "{item:?}"
    );
    assert_eq!(item["bip_summary"], "L'IA de Vie ne répond pas : ses routines n'ont pas pu tourner (« Courriels », « Météo »). Réessayer ? (Non : elles s'arrêtent.)");
    assert_eq!(item["choices"], json!(["once", "deny"]));
    assert_eq!(crate::approval_level::classify_card(item), KIND);
    assert_eq!(pending_items(&config, Some("coach")).len(), 1, "another agent: its own card");
    for r in [&one, &two] {
        let r = routines::get(&config, &r.id).unwrap();
        assert!(!r.enabled && r.frame.fix.as_ref().unwrap().id == item["request_id"].as_str().unwrap());
    }
    // a week without an answer: it waits (its routines are paused already), nothing said
    assert!(due(&config, now + 8.0 * DAY).iter().all(|d| !matches!(d, Due::Line { agent, .. } if agent == "vie")));
    // « Réessayer »: all on again, the count from zero, no turn
    let id = item["request_id"].as_str().unwrap().to_string();
    let settled = settle(&config, "vie", &id, true).unwrap();
    assert_eq!((settled.variant.as_str(), settled.yes), (AI, true));
    for r in [&one, &two] {
        let r = routines::get(&config, &r.id).unwrap();
        assert!(r.enabled && r.frame.fix.is_none() && r.frame.failures == 0 && r.next_run.is_some(), "{r:?}");
    }
    assert!(pending_items(&config, Some("vie")).is_empty());
    // again, then « Arrêter »: paused, on their page
    for n in 0..3 {
        fail(&one.id, 100.0 + f64::from(n));
    }
    let id = pending_items(&config, Some("vie"))[0]["request_id"].as_str().unwrap().to_string();
    assert!(settle(&config, "vie", &id, false).is_some());
    let r = routines::get(&config, &one.id).unwrap();
    assert!(!r.enabled && r.frame.fix.is_none());
    // another failure stays a routine's own card
    let own =
        routines::create(&config, "vie", "Rangement", "every 2h", "x", None, Maker::Person { muted: false }).unwrap();
    for n in 0..3 {
        routines::failed(&config, &own.id, routines::Ended::Failed, now + f64::from(n), false);
    }
    assert_eq!(pending_items(&config, Some("vie"))[0]["bip_fix"]["variant"], REPAIR);
}

/// Point 2: The common card's texts in the four languages (to be validated by Alex), and the warning of 1: the contract's.
#[test]
fn a2_the_new_cards_texts_are_the_contracts() {
    let texts = &contract()["texts"];
    for lang in ["fr", "en", "de", "es"] {
        let c = &texts[lang];
        let s = |k: &str| c[k].as_str().unwrap().to_string();
        assert_eq!(warn_sentence(lang, "{date}"), s("warn"), "{lang}");
        assert_eq!(ai_title(lang, "{agent}"), s("ai_top"), "{lang}");
        assert_eq!(ai_since_line(lang, "{since}", "{names}"), s("ai_since"), "{lang}");
        assert_eq!(json!(ai_buttons(lang)), c["ai_buttons"], "{lang}");
        assert_eq!(ai_under_i(lang), s("ai_under_i"), "{lang}");
        assert_eq!(ai_summary(lang, "{agent}", "{names}"), s("ai_summary"), "{lang}");
    }
}

#[test]
fn a2_the_common_card_speaks_four_languages() {
    for (lang, title, buttons) in [
        ("fr", "Vie · son IA ne répond pas", ["Réessayer", "Arrêter"]),
        ("en", "Vie · its AI isn't answering", ["Try again", "Stop"]),
        ("de", "Vie · seine KI antwortet nicht", ["Erneut versuchen", "Stoppen"]),
        ("es", "Vie · su IA no responde", ["Reintentar", "Detener"]),
    ] {
        assert_eq!(ai_title(lang, "Vie"), title);
        assert_eq!(ai_buttons(lang), buttons);
        assert!(!ai_under_i(lang).is_empty() && !ai_summary(lang, "Vie", "x").is_empty());
    }
}

/// Point 3: tf-fin-par-defaut-de-lui-meme B2: what its agent set up on its own never stops at its default end without its
/// person: unanswered, it goes on and « Garder ? » is asked once more, pushed, 14 days later; unanswered again, it
/// goes on « until you stop it ». « La laisser s'arrêter » makes the end hers.
#[test]
fn a3_a_default_end_never_stops_it_without_its_person() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let r = agents(&config, "Météo", "every day at 7:30", &Ask { asked: false, ..Ask::default() });
    let until = r.frame.until.unwrap();
    assert!(matches!(due(&config, until - 2.0 * DAY).as_slice(), [Due::Raise { push: false, .. }]));
    // its date passed, no answer: still here, asked again, pushed
    let found = due(&config, until + 60.0);
    let [Due::Raise { push: true, line, .. }] = found.as_slice() else { panic!("{found:?}") };
    let kept = routines::get(&config, &r.id).expect("gone at its default end without its person");
    let next = kept.frame.until.unwrap();
    assert!((next - (until + 60.0 + REMIND_DAYS * DAY)).abs() < 5.0, "{kept:?}");
    assert!(line.starts_with("La routine « Météo » s'arrête le "), "{line}");
    let items = pending_items(&config, Some("vie"));
    let [item] = items.as_slice() else { panic!("{items:?}") };
    assert_eq!(item["bip_fix"]["variant"], KEEP);
    assert!(kept.enabled && kept.frame.reminded);
    assert!(due(&config, until + 120.0).is_empty(), "asked again once");
    // unanswered again: on until its person stops it, the card gone, nothing pushed
    let found = due(&config, next + 60.0);
    assert!(found.iter().all(|d| !matches!(d, Due::Raise { .. } | Due::Line { .. })), "{found:?}");
    let kept = routines::get(&config, &r.id).expect("gone at its second date");
    assert_eq!(routines::end_of(&kept), End::Never);
    assert!(kept.enabled && kept.frame.fix.is_none() && pending_items(&config, Some("vie")).is_empty());
    assert!(due(&config, next + 30.0 * DAY).is_empty());
    // « La laisser s'arrêter »: it stops at its date, as said
    let other = agents(&config, "Agenda", "every day at 8:00", &Ask { asked: false, ..Ask::default() });
    let until = other.frame.until.unwrap();
    due(&config, until - 2.0 * DAY);
    let id = pending_items(&config, Some("vie"))[0]["request_id"].as_str().unwrap().to_string();
    assert!(settle(&config, "vie", &id, false).is_some());
    assert!(routines::get(&config, &other.id).is_some());
    due(&config, until + 60.0);
    assert!(routines::get(&config, &other.id).is_none(), "« La laisser s'arrêter » kept it");
}

/// Point 3: The same for a watcher of its agent's own.
#[test]
fn a3_a_watchers_default_end_never_removes_it_without_its_person() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    watcher_of_its_own(&config, "c_veille", now + 2.0 * DAY, true);
    assert!(matches!(due(&config, now).as_slice(), [Due::Raise { push: false, .. }]));
    let found = due(&config, now + 2.0 * DAY + 60.0);
    assert!(!found.iter().any(|d| matches!(d, Due::Remove { .. })), "removed at its default end: {found:?}");
    assert!(matches!(found.as_slice(), [Due::Raise { push: true, .. }]), "{found:?}");
    let c = connectors::get(&config, "vie", "c_veille").unwrap();
    let until = c.watch.as_ref().and_then(|w| w.until).unwrap();
    assert!(c.enabled && until > now + 15.0 * DAY);
    let found = due(&config, until + 60.0);
    assert!(!found.iter().any(|d| matches!(d, Due::Remove { .. } | Due::Raise { .. })), "{found:?}");
    let c = connectors::get(&config, "vie", "c_veille").unwrap();
    assert_eq!(crate::watchers::end_of(c.watch.as_ref()), End::Never);
    assert!(c.enabled && c.frame.fix.is_none());
}
