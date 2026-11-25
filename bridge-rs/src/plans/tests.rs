//! plans.rs' tests (contracts/plans/cases.json; design-agents-proactifs.md §3.9), no model anywhere.

use super::*;

fn config_in(dir: &std::path::Path) -> Config {
    let mut config = Config::with_key("k");
    config.outbox.db_path = dir.join("bridge.db").to_string_lossy().into_owned();
    for name in ["vie", "budget", "coach"] {
        config.agents.insert(name.into(), AgentConfig { name: name.into(), ..Default::default() });
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
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/plans/cases.json");
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn example(name: &str) -> Value {
    contract()["examples"].as_array().unwrap().iter().find(|e| e["name"] == name).unwrap().clone()
}

fn args(value: &Value) -> Dict {
    value.as_object().unwrap().clone()
}

fn at(text: &str) -> f64 {
    DateTime::parse_from_rfc3339(text).unwrap().timestamp() as f64
}

const PARIS: Tz = chrono_tz::Europe::Paris;
const LISBON: Tz = chrono_tz::Europe::Lisbon;

/// The lisbon example read at its « now ».
fn lisbon() -> Draft {
    let case = example("lisbon");
    draft(&args(&case["arguments"]), "vie", PARIS, PARIS, at(case["now"].as_str().unwrap())).unwrap()
}

#[test]
fn the_contract_s_texts_are_the_bridge_s() {
    let texts = &contract()["texts"];
    for t in TEXTS.iter() {
        let c = &texts[t.lang];
        let pairs = [
            ("other_zone", t.other_zone),
            ("route", t.route),
            ("watch", t.watch),
            ("under_i", t.under_i),
            ("first_card", t.first_card),
            ("moved", t.moved),
            ("cancelled", t.cancelled),
            ("route_pending", t.route_pending),
            ("before", t.before),
            ("route_failed", t.route_failed),
            ("watch_failed", t.watch_failed),
            ("notification", t.notification),
            ("summary", t.summary),
        ];
        for (key, ours) in pairs {
            assert_eq!(c[key].as_str().unwrap(), ours, "{} {key}", t.lang);
        }
        for (i, mode) in MODES.iter().enumerate() {
            assert_eq!(c["modes"][mode].as_str().unwrap(), t.modes[i], "{} {mode}", t.lang);
        }
        // the rhythm of the water example, in each language, as validated
        let water = Repeat { days: "1-5".into(), said: "weekdays".into(), times: vec!["10:00".into(), "15:00".into()],
                             weeks: 2, first: "2026-11-23".into() };
        assert_eq!(rhythm_text(t.lang, &water), c["rhythm_example"].as_str().unwrap(), "{}", t.lang);
    }
}

#[test]
fn a_trip_is_one_card_with_its_moments_in_their_place_s_time() {
    let case = example("lisbon");
    let plan = lisbon().plan;
    assert_eq!((plan.kind.as_str(), plan.title.as_str(), plan.moments.len()), ("trip", "Lisbonne", 3));
    let wanted = &case["card"];
    let whens: Vec<String> =
        plan.moments.iter().map(|m| when_text("fr", m.at, zone_of(&m.zone), PARIS, &plan.city)).collect();
    assert_eq!(json!(whens), wanted["when_text"]);
    let says: Vec<&str> = plan.moments.iter().map(|m| m.say.as_str()).collect();
    assert_eq!(json!(says), wanted["says"]);
    assert_eq!(summary("fr", "Vie", &plan.title, count(&plan)), wanted["bip_summary"].as_str().unwrap());
    assert_eq!(watch_text("fr", &plan.watch).unwrap(), wanted["watch_text"].as_str().unwrap());
    assert_eq!(notification("fr", &plan.title), wanted["notification"].as_str().unwrap());
    // the arrival at 10:50 in Lisbon is 11:50 in Paris: the moment keeps its own clock
    assert_eq!(plan.moments[1].at, at("2026-11-25T10:50:00+00:00"));
    assert_eq!(plan.state, WAITING);
}

#[test]
fn moments_keep_their_zone_when_the_person_travels() {
    let plan = lisbon().plan;
    let arrival = &plan.moments[1];
    // at home in Paris: « heure de Lisbonne »; in Lisbon now: its own clock, nothing to add; the same instant
    assert!(when_text("fr", arrival.at, LISBON, PARIS, "Lisbonne").ends_with("10:50 · heure de Lisbonne"));
    assert_eq!(when_text("fr", arrival.at, LISBON, LISBON, "Lisbonne"), "mer. 25 · 10:50");
    // a one-time routine is an instant: the routines' zone moving (dates.rs) leaves it where it is
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let r = crate::routines::create_said(&config, "vie", "x", crate::routines::Schedule::Once { at: now + 3600.0 },
                                         "Hop.", false, "p1", None, Some(1), now);
    crate::dates::change(&config, |p| p.routines = "America/New_York".into()).unwrap();
    assert_eq!(crate::routines::get(&config, &r.id).unwrap().next_run, Some(now + 3600.0));
}

#[test]
fn what_the_agent_writes_is_checked_before_any_card() {
    let now = at("2026-11-22T18:02:00+01:00");
    let base = example("lisbon")["arguments"].clone();
    let with = |change: &dyn Fn(&mut Value)| {
        let mut a = base.clone();
        change(&mut a);
        draft(&args(&a), "vie", PARIS, PARIS, now)
    };
    type Change = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Change)> = vec![
        ("past", Box::new(|a: &mut Value| a["moments"][0]["at"] = json!("2026-11-20T09:00"))),
        ("zone", Box::new(|a: &mut Value| a["zone"] = json!("Lisbon"))),
        ("http", Box::new(|a: &mut Value| a["moments"][1]["say"] = json!("voir http://tap.pt"))),
        ("link", Box::new(|a: &mut Value| a["moments"][1]["link"] = json!("http://tap.pt"))),
        ("markup", Box::new(|a: &mut Value| a["moments"][1]["say"] = json!("**Bienvenue**"))),
        ("break", Box::new(|a: &mut Value| a["moments"][1]["say"] = json!("Bienvenue\nà Lisbonne"))),
        ("domain", Box::new(|a: &mut Value| a["watch"]["mail_from"][0]["domain"] = json!("https://flytap.com/x"))),
        ("kind", Box::new(|a: &mut Value| a["kind"] = json!("party"))),
        ("too_many", Box::new(|a: &mut Value| {
            let one = a["moments"][1].clone();
            a["moments"] = json!(vec![one; 7]);
        })),
        ("repeat_on_a_trip", Box::new(|a: &mut Value| a["repeat"] = json!({"days": "daily", "times": ["10:00"], "weeks": 1}))),
        ("long_say", Box::new(|a: &mut Value| a["moments"][1]["say"] = json!("a".repeat(SAY_MAX + 1)))),
        // (Google and CalDAV are sources since plans-sources: another one is still refused)
        ("event", Box::new(|a: &mut Value| a["event"] = json!({"source": "outlook", "id": "x"}))),
    ];
    for (name, change) in cases {
        assert!(with(&*change).is_err(), "{name}");
    }
    // a habit needs its rhythm, and only one moment
    let habit = json!({"kind": "habit", "title": "Eau", "moments": [{"what": "Eau", "say": "Un verre ?"}]});
    assert!(draft(&args(&habit), "vie", PARIS, PARIS, now).unwrap_err().contains("repeat"));
    assert!(with(&|_: &mut Value| {}).is_ok());
}

#[test]
fn a_habit_runs_vers_its_times_and_stops_after_its_weeks() {
    let case = example("water");
    let now = at(case["now"].as_str().unwrap());
    let plan = draft(&args(&case["arguments"]), "vie", PARIS, PARIS, now).unwrap().plan;
    let repeat = plan.repeat.clone().unwrap();
    assert_eq!(repeat.first, "2026-11-23"); // from tomorrow
    assert_eq!(rhythm_text("fr", &repeat), case["card"]["rhythm_text"].as_str().unwrap());
    assert_eq!(count(&plan), case["card"]["runs"].as_u64().unwrap() as usize);
    assert_eq!(summary("fr", "Vie", &plan.title, count(&plan)), case["card"]["bip_summary"].as_str().unwrap());
    // « vers »: a stable shift of −10 to +10 min, the same for every time of the plan
    for id in ["a", "b", "c", "0123456789ab"] {
        let shift = approx_shift(id);
        assert!((-10..=10).contains(&shift) && shift == approx_shift(id), "{id}");
    }
    let schedules = habit_schedules(&repeat, -7);
    assert_eq!(schedules, vec![("53 9 * * 1-5".to_string(), 10), ("53 14 * * 1-5".to_string(), 10)]);
    // its moment is its first run, Monday 23 around 10:00
    let first = PARIS.timestamp_opt(plan.moments[0].at as i64, 0).unwrap();
    assert_eq!(first.date_naive().to_string(), "2026-11-23");
    // a list of days, in words
    let gym = Repeat { days: "1,3,5".into(), said: "mon,wed,fri".into(), times: vec!["07:30".into()], weeks: 1,
                       first: "2026-11-23".into() };
    assert_eq!(rhythm_text("fr", &gym), "Lun., mer. et ven. vers 7 h 30 · 1 semaine");
    assert_eq!(rhythm_text("en", &gym), "Mon, Wed and Fri around 7:30 AM · 1 week");
    assert_eq!(habit_schedules(&gym, 0), vec![("30 7 * * 1,3,5".to_string(), 3)]);
}

fn made_store(entries: &[(&str, bool, f64)]) -> Store {
    Store {
        made: entries.iter().map(|(a, asked, at)| Made { agent: a.to_string(), at: *at, asked: *asked, run: None }).collect(),
        ..Store::default()
    }
}

#[test]
fn caps_two_unasked_cards_per_agent_four_per_place_one_waiting() {
    let now = at("2026-11-22T18:02:00+01:00");
    let mut plan = lisbon().plan;
    // unasked: 2 a day for the agent
    let two = made_store(&[("vie", false, now - 60.0), ("vie", false, now - 30.0)]);
    assert!(refused(&two, &plan, None, PARIS, now).unwrap().contains("already made 2"));
    // yesterday's don't count (the person's day), nor what was asked
    let old = made_store(&[("vie", false, now - 86400.0), ("vie", true, now - 30.0), ("vie", false, now - 30.0)]);
    assert_eq!(refused(&old, &plan, None, PARIS, now), None);
    // 4 a day for the place
    let place = made_store(&[("budget", false, now - 9.0), ("budget", false, now - 8.0), ("coach", false, now - 7.0),
                             ("coach", false, now - 6.0)]);
    assert!(refused(&place, &plan, None, PARIS, now).unwrap().contains("4 cards"));
    // one waiting per agent
    let mut other = plan.clone();
    other.title = "Porto".into();
    let waiting = Store { plans: vec![other], ..Store::default() };
    assert!(refused(&waiting, &plan, None, PARIS, now).unwrap().contains("already waits"));
    // asked, in the person's turn: one plan per reply, whatever the day's count
    plan.asked = true;
    let mut replied = place.clone();
    assert_eq!(refused(&replied, &plan, Some("run_1"), PARIS, now), None);
    replied.made.push(Made { agent: "vie".into(), at: now, asked: true, run: Some("run_1".into()) });
    assert!(refused(&replied, &plan, Some("run_1"), PARIS, now).unwrap().contains("one plan per reply"));
    assert_eq!(refused(&replied, &plan, Some("run_2"), PARIS, now), None);
}

#[test]
fn two_proposals_of_the_same_plan_and_a_no_thanks_kept_14_days() {
    let now = at("2026-11-22T18:02:00+01:00");
    let plan = lisbon().plan;
    let mut same = plan.clone();
    same.state = SET.into();
    let set = Store { plans: vec![same], ..Store::default() };
    assert!(refused(&set, &plan, None, PARIS, now).unwrap().contains("already set"));
    let print = print_of("vie", "  LISBONNE ");
    assert_eq!(print, print_of("vie", "Lisbonne"));
    assert_ne!(print, print_of("budget", "Lisbonne"));
    let declined = Store { declined: vec![Print { agent: "vie".into(), print, at: now - 13.0 * 86400.0 }], ..Store::default() };
    assert!(refused(&declined, &plan, None, PARIS, now).unwrap().contains("said no"));
    assert_eq!(refused(&declined, &plan, None, PARIS, now + 2.0 * 86400.0), None);
}

#[test]
fn a_plan_is_one_card_and_nothing_runs_before_yes() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let mut first = lisbon().plan;
    first.first = true;
    let mut second = first.clone();
    (second.id, second.title, second.first, second.made) = ("p2".into(), "Porto".into(), false, first.made + 1.0);
    change(&config, |s| s.plans.extend([first.clone(), second.clone()]));
    let items = pending_items(&config, Some("vie"));
    assert_eq!(items.len(), 2);
    let card = &items[0];
    let lang = lang_of(&config, "vie");
    let t = texts(lang);
    assert_eq!(card["run_id"], format!("{RUN_PREFIX}{}", first.id));
    assert_eq!(card["choices"], json!(["once", "deny"]));
    assert_eq!(card["bip_summary"], json!(summary(lang, "vie", "Lisbonne", 3)));
    assert_eq!(card["bip_plan"]["offers"], json!([]));
    assert_eq!(card["bip_plan"]["moments"].as_array().unwrap().len(), 3);
    let detail = card["bip_detail"].as_str().unwrap();
    assert!(detail.contains(t.under_i) && detail.contains(t.first_card), "{detail}");
    assert!(!items[1]["bip_detail"].as_str().unwrap().contains(t.first_card));
    // nothing scheduled
    assert!(crate::routines::all(&config, None).is_empty());
    assert!(pending_items(&config, Some("budget")).is_empty());
}

#[test]
fn yes_makes_once_routines_and_none_calls_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let mut plan = lisbon().plan;
    for (i, m) in plan.moments.iter_mut().enumerate() {
        m.at = now + 3600.0 * (i as f64 + 1.0);
    }
    plan.moments[0].at = now - 60.0; // passed while the card waited
    change(&config, |s| s.plans.push(plan.clone()));
    let skip = vec!["m3".to_string()];
    let done = answer(&config, "vie", &plan.id, true, &skip, "api_s", now);
    assert!(matches!(done, Some(Answer::Set { kept: 1, of: 3, passed: 1, watch: Some(false) })));
    let routines = crate::routines::all(&config, Some("vie"));
    assert_eq!(routines.len(), 1);
    let r = &routines[0];
    assert_eq!((r.say.as_deref(), r.plan.as_deref(), r.origin.as_str()), (Some(plan.moments[1].say.as_str()), Some(plan.id.as_str()), "api_s"));
    assert!(r.enabled && !r.awaits_start && r.card.is_none() && r.repeat == Some(1));
    assert!(matches!(r.schedule, crate::routines::Schedule::Once { at } if at == plan.moments[1].at));
    let kept = get(&config, &plan.id).unwrap();
    assert_eq!((kept.state.as_str(), kept.routines.clone()), (SET, vec![r.id.clone()]));
    assert_eq!(kept.ends, plan.moments[2].at + END_AFTER_SECONDS);
    // answered: no card any more, a second answer finds none
    assert!(pending_items(&config, None).is_empty());
    assert!(answer(&config, "vie", &plan.id, true, &[], "", now).is_none());
    // its view says its plan and its words
    let view = crate::routines::view(r, PARIS);
    assert_eq!((view["plan"].clone(), view["say"].clone(), view["urgent"].clone()),
               (json!(plan.id), json!(plan.moments[1].say), json!(false)));
}

#[test]
fn no_thanks_forgets_it_and_keeps_its_print_only() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let plan = lisbon().plan;
    change(&config, |s| s.plans.push(plan.clone()));
    assert!(matches!(answer(&config, "vie", &plan.id, false, &[], "", py::time_time()), Some(Answer::Declined)));
    let store = load(&config);
    assert!(store.plans.is_empty() && crate::routines::all(&config, None).is_empty());
    assert_eq!(store.declined.len(), 1);
    let file = std::fs::read_to_string(path(&config)).unwrap();
    assert!(!file.contains("Lisbonne"), "only its print");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(path(&config)).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn a_moved_event_shifts_the_moments_and_a_cancelled_event_removes_the_plan() {
    let start = at("2026-11-26T15:00:00+01:00");
    let event = EventRef { source: "phone".into(), id: "e1".into(), start, calendar: String::new() };
    let copy = |events: Value| args(&json!({"from": "2026-11-15", "to": "2027-01-20", "events": events}));
    let same = copy(json!([{"id": "e1", "start": "2026-11-26T15:00:00+01:00"}]));
    assert_eq!(event_news(&event, &same, PARIS), EventNews::Same);
    let moved = copy(json!([{"id": "e1", "start": "2026-11-27T16:00:00+01:00"}]));
    assert_eq!(event_news(&event, &moved, PARIS), EventNews::Moved { start: start + 25.0 * 3600.0, delta: 25.0 * 3600.0 });
    assert_eq!(event_news(&event, &copy(json!([])), PARIS), EventNews::Cancelled);
    // a copy that doesn't cover its day says nothing of it
    let short = args(&json!({"from": "2026-11-15", "to": "2026-11-20", "events": []}));
    assert_eq!(event_news(&event, &short, PARIS), EventNews::Same);
    // the lines, as validated
    let dentist = &example("dentist")["texts"];
    assert_eq!(moved_line("fr", "Dentiste", start + 25.0 * 3600.0, PARIS), dentist["moved"].as_str().unwrap());
    assert_eq!(cancelled_line("fr", "Dentiste"), dentist["cancelled"].as_str().unwrap());
    assert_eq!(route_words("fr", "Cabinet du Dr Martin", 25, "walking"), dentist["route"].as_str().unwrap());
    // the routines move with it, and nothing else
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let now = py::time_time();
    let r = crate::routines::create_said(&config, "vie", "x", crate::routines::Schedule::Once { at: now + 600.0 },
                                         "Hop.", true, "p1", None, Some(1), now);
    crate::routines::shift_once(&config, &r.id, 3600.0);
    let moved = crate::routines::get(&config, &r.id).unwrap();
    assert_eq!(moved.next_run, Some(now + 4200.0));
    assert!(matches!(moved.schedule, crate::routines::Schedule::Once { at } if at == now + 4200.0));
}

#[test]
fn a_set_plan_goes_a_day_after_its_last_moment_and_a_lapsed_card_goes() {
    let mut plan = lisbon().plan;
    let last = plan.moments.iter().map(|m| m.at).fold(0.0, f64::max);
    assert!(lapsed(&plan, last + 1.0) && !lapsed(&plan, last - 1.0));
    plan.state = SET.into();
    plan.ends = last + END_AFTER_SECONDS;
    assert!(!over(&plan, last + 3600.0) && over(&plan, last + END_AFTER_SECONDS + 1.0));
    assert!(!lapsed(&plan, last + 1.0));
}

#[test]
fn no_plan_for_an_agent_others_use_or_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let proposes = crate::agent_proactive::proposes;
    assert!(proposes(&config, "vie") && offered(&config, "vie"));
    use crate::agent_proactive::{set_level, Level};
    set_level(&config, "vie", Level::Off).unwrap();
    set_level(&config, "budget", Level::Smart).unwrap();
    assert!(!proposes(&config, "vie") && proposes(&config, "budget") && proposes(&config, "coach"));
    // its routines off: no plan either (its moments are routines)
    crate::routines::set_enabled(&config, "coach", false);
    assert!(proposes(&config, "coach") && !offered(&config, "coach"));
    // an agent other people talk to (a project agent): never
    let projects = PathBuf::from(config.data_dir()).join("project-agents.json");
    std::fs::write(&projects, json!({"projects": {"budget": {"name": "Lisbonne", "ends": 1.0e10, "archived": null}}}).to_string())
        .unwrap();
    assert!(crate::project_agent::is_project(&config, "budget"), "the test's project file is not the bridge's");
    assert!(!proposes(&config, "budget"));
}

#[test]
fn plan_and_its_card_are_critical_never_auto_approved() {
    assert!(crate::approval_level::rule_of("plan").critical);
    let card = json!({"run_id": "bip-plan-x"});
    assert_eq!(crate::approval_level::classify_card(card.as_object().unwrap()), "plan");
}

/// An event of the phone's copy as `plan_propose` keeps it (read like plans.json, so a field added later is welcome).
fn event_ref(value: Value) -> EventRef {
    serde_json::from_value(value).unwrap()
}

/// Review 1 (B2): every occurrence of a repeating event has the same id (the iPhone's `eventIdentifier`, Android's
/// `EVENT_ID`), the one a week before first in the copy. The plan's occurrence is never taken for another one: no
/// « a bougé » of a week, whether the week before is still in the copy or gone from it, nor when its own occurrence
/// went and the others stay (a moved occurrence on Android gets an id of its own).
#[test]
fn a_repeating_event_never_moves_the_plan_by_a_week() {
    let start = at("2026-11-26T15:00:00+01:00");
    let event = event_ref(json!({"source": "phone", "id": "e1", "start": start}));
    let copy = |events: Value| args(&json!({"from": "2026-11-15", "to": "2027-01-20", "events": events}));
    let weekly = |days: &[&str]| {
        json!(days.iter().map(|d| json!({"id": "e1", "start": format!("2026-{d}T15:00:00+01:00"), "recurring": true}))
            .collect::<Vec<_>>())
    };
    assert_eq!(event_news(&event, &copy(weekly(&["11-19", "11-26", "12-03"])), PARIS), EventNews::Same);
    assert_eq!(event_news(&event, &copy(weekly(&["11-26", "12-03"])), PARIS), EventNews::Same);
    assert_eq!(event_news(&event, &copy(weekly(&["11-19", "12-03"])), PARIS), EventNews::Same, "its own occurrence gone");
    // its own occurrence moved within its day: moved
    let later = copy(json!([{"id": "e1", "start": "2026-11-19T15:00:00+01:00", "recurring": true},
                            {"id": "e1", "start": "2026-11-26T17:00:00+01:00", "recurring": true}]));
    assert_eq!(event_news(&event, &later, PARIS), EventNews::Moved { start: start + 7200.0, delta: 7200.0 });
}

/// Review 1 (R5): the person took the event's calendar out of the phone's copy (the app's choice of calendars): the
/// event is not in the copy any more, but it is not cancelled: the plan stays, no line.
#[test]
fn a_calendar_taken_out_of_the_copy_cancels_nothing() {
    let start = at("2026-11-26T15:00:00+01:00");
    let event = event_ref(json!({"source": "phone", "id": "e1", "start": start, "calendar": "C1"}));
    let with = |calendars: Value| {
        args(&json!({"from": "2026-11-15", "to": "2027-01-20", "calendars": calendars, "events": []}))
    };
    assert_eq!(event_news(&event, &with(json!([{"id": "C2", "title": "Pro"}])), PARIS), EventNews::Same);
    assert_eq!(event_news(&event, &with(json!([{"id": "C1", "title": "Perso"}, {"id": "C2"}])), PARIS),
               EventNews::Cancelled);
}

/// Review 1 (R3): `urgent` (the system's urgent level, through « Ne pas déranger ») is described to the agent and kept
/// for what must be on time: never a habit's (every day), two moments of a plan at most (leaving, a check-in).
#[test]
fn urgent_is_described_and_kept_for_what_must_be_on_time() {
    let schema = tool();
    let urgent = &schema["inputSchema"]["properties"]["moments"]["items"]["properties"]["urgent"];
    let words = urgent["description"].as_str().unwrap_or("");
    assert!(words.contains("leave") && words.contains("check-in") && words.contains("Do Not Disturb"), "{words}");
    let now = at("2026-11-22T18:02:00+01:00");
    let mut habit = example("water")["arguments"].clone();
    habit["moments"][0]["urgent"] = json!(true);
    assert!(draft(&args(&habit), "vie", PARIS, PARIS, now).unwrap_err().contains("urgent"));
    let mut trip = example("lisbon")["arguments"].clone();
    for m in trip["moments"].as_array_mut().unwrap() {
        m["urgent"] = json!(true);
    }
    assert!(draft(&args(&trip), "vie", PARIS, PARIS, now).unwrap_err().contains("urgent"));
    trip["moments"][2]["urgent"] = json!(false);
    assert!(draft(&args(&trip), "vie", PARIS, PARIS, now).is_ok());
}

/// Review 1 (B2): the event a plan would follow, as the phone's copy has it: one event, its start and calendar; a
/// repeating one (marked, or several occurrences with that id) never.
#[test]
fn a_plan_follows_one_event_never_a_repeating_one() {
    let copy = args(&json!({"from": "2026-11-15", "to": "2027-01-20", "events": [
        {"id": "e1", "calendar": "C1", "start": "2026-11-26T15:00:00+01:00"},
        {"id": "w1", "calendar": "C1", "start": "2026-11-19T09:00:00+01:00", "recurring": true},
        {"id": "w1", "calendar": "C1", "start": "2026-11-26T09:00:00+01:00", "recurring": true},
        {"id": "r1", "calendar": "C2", "start": "2026-11-26T09:00:00+01:00", "recurring": true}]}));
    assert_eq!(phone_event(&copy, "e1", PARIS),
               PhoneEvent::Found { start: at("2026-11-26T15:00:00+01:00"), calendar: "C1".into() });
    assert_eq!(phone_event(&copy, "w1", PARIS), PhoneEvent::Repeating);
    assert_eq!(phone_event(&copy, "r1", PARIS), PhoneEvent::Repeating, "one occurrence in the copy, marked");
    assert_eq!(phone_event(&copy, "nope", PARIS), PhoneEvent::Missing);
}

/// Review 1 (R1): the person's home is the « Coordonnées » entry labelled as home (or whose « Indications » say so),
/// never just the first one; one whose label says work never is.
#[test]
fn the_home_is_the_entry_labelled_home_never_the_first_one() {
    let entry = |label: &str, hint: Option<&str>, line: &str| crate::vault_contacts::Entry {
        id: label.into(),
        label: label.into(),
        hint: hint.map(str::to_string),
        secret: args(&json!({"address_line1": line, "postal_code": "69001", "city": "Lyon", "country": "FR"})),
    };
    let pro = || entry("Kerfy Studio (pro)", Some("pour la société"), "1 rue de l'Essai");
    assert_eq!(home_of(&[pro()]), None);
    let home = entry("Domicile", None, "3 rue du Test");
    assert_eq!(home_of(&[pro(), home]).as_deref(), Some("3 rue du Test, 69001 Lyon, FR"));
    for label in ["Maison", "Chez moi", "Home", "Zuhause", "Casa", "Mon domicile"] {
        assert!(home_of(&[pro(), entry(label, None, "x")]).is_some(), "{label}");
    }
    assert!(home_of(&[entry("Moi", Some("à la maison"), "x")]).is_some(), "its indications");
    assert_eq!(home_of(&[entry("Home office (pro)", None, "x"), entry("Camille", None, "y")]), None);
    assert_eq!(home_of(&[entry("Domicile", None, "")]), None, "no address in it");
}

/// Review 2 (R13): « Perso », « Appartement », « Mon adresse », « Résidence » say home too, and theirs in the other
/// languages a label may be written in; someone else's home or a holiday one never is the person's.
#[test]
fn the_home_is_said_in_every_language_and_never_someone_elses_nor_a_holiday_one() {
    let entry = |label: &str| crate::vault_contacts::Entry {
        id: label.into(),
        label: label.into(),
        hint: None,
        secret: args(&json!({"address_line1": "3 rue du Test", "postal_code": "69001", "city": "Lyon", "country": "FR"})),
    };
    for label in ["Perso", "Appartement", "Mon adresse", "Résidence", "Mon appart", "Personal", "Apartment", "Flat",
                  "My address", "Privat", "Meine Wohnung", "Meine Adresse", "Piso", "Mi dirección", "Residencia",
                  "Apartamento", "Abitazione", "Appartamento", "Il mio indirizzo"] {
        assert!(home_of(&[entry(label)]).is_some(), "{label}");
    }
    for label in ["Maison de ma mère", "Maison de vacances", "Chez mes parents", "Holiday home", "Mum's house",
                  "Ferienwohnung", "Wohnung meiner Eltern", "Casa de mis padres", "Casa de vacaciones",
                  "Casa al mare per le vacanze", "Appartement pro"] {
        assert!(home_of(&[entry(label)]).is_none(), "{label}");
    }
}

/// Finitions 1.1, point 1: the card's top line comes from the bridge (`bip_plan.top`), in the agent's language like the
/// rest of the card (an app 1.1 drew it in the phone's; an app 1.0 ignores the key). A habit says « une habitude ».
#[test]
fn f1_a_plan_card_carries_its_top_line_in_the_agent_s_language() {
    let texts = &contract()["texts"];
    for t in TEXTS.iter() {
        let c = &texts[t.lang];
        let fill = |key: &str| c[key].as_str().unwrap().replace("{agent}", "Vie").replace("{title}", "Lisbonne");
        assert_eq!(top(t.lang, "Vie", "Lisbonne", "trip"), fill("top"), "{}", t.lang);
        assert_eq!(top(t.lang, "Vie", "Lisbonne", "appointment"), fill("top"), "{}", t.lang);
        assert_eq!(top(t.lang, "Vie", "Lisbonne", "habit"), fill("top_habit"), "{}", t.lang);
    }
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let plan = lisbon().plan;
    change(&config, |s| s.plans.push(plan.clone()));
    let card = &pending_items(&config, Some("vie"))[0];
    let lang = lang_of(&config, "vie");
    assert_eq!(card["bip_plan"]["top"], json!(top(lang, "vie", "Lisbonne", "trip")), "{card:?}");
}

/// Bench 1.1 (24/11, `briefs/bench-1-1.md` problem 5): 6 of GLM-5.3 Flash's 8 habits refused for their shape, and the
/// Spark's too: `repeat` inside the moment, `weeks` forgotten, days as numbers, `what` or `say` left out. Each shape
/// the models wrote is taken now (weeks: `DEFAULT_WEEKS`); a refusal left says the right shape at once.
#[test]
fn bench_a_habit_as_the_models_write_it_is_taken() {
    let now = at("2026-11-24T14:00:00+01:00");
    let shapes = [
        // GLM « boire plus d'eau »: no weeks, the rhythm in the moment and beside it
        json!({"kind": "habit", "title": "Boire plus d'eau", "moments": [{"repeat": {"days": "daily", "times": ["10:00", "15:00"]},
               "say": "Pense à ton verre d'eau 💧", "what": "Un verre d'eau ?"}],
               "repeat": {"days": "daily", "times": ["10:00", "15:00"]}}),
        // GLM « courir »: days as numbers, no `what`
        json!({"kind": "habit", "title": "Courir 3 fois par semaine", "moments": [{"say": "C'est l'heure de ta sortie de course !"}],
               "repeat": {"days": [2, 4, 6], "times": ["09:00"]}}),
        // the Spark « Joggen »: only in the moment
        json!({"kind": "habit", "title": "Joggen – 3× pro Woche", "moments": [{"repeat": {"days": ["mon", "wed", "fri"],
               "times": ["08:00"], "weeks": 12}, "what": "Joggen – Mo, Mi, Fr, 8 Uhr"}]}),
        // a day list in one text, one time as text, weeks as text
        json!({"kind": "habit", "title": "Spanisch", "moments": [{"what": "Spanisch", "say": "20 Minuten?"}],
               "repeat": {"days": "mon, tue, wed", "times": "20:00", "weeks": "3"}}),
    ];
    for shape in &shapes {
        let plan = draft(&args(shape), "vie", PARIS, PARIS, now).unwrap_or_else(|e| panic!("{shape}: {e}")).plan;
        let r = plan.repeat.clone().unwrap();
        assert!(!plan.moments[0].what.is_empty() && !plan.moments[0].say.is_empty(), "{shape}");
        assert!((1..=WEEKS_MAX).contains(&r.weeks), "{shape}");
    }
    let water = draft(&args(&shapes[0]), "vie", PARIS, PARIS, now).unwrap().plan.repeat.unwrap();
    assert_eq!(water.weeks, DEFAULT_WEEKS);
    let run = draft(&args(&shapes[1]), "vie", PARIS, PARIS, now).unwrap().plan.repeat.unwrap();
    assert_eq!(run.days, "2,4,6", "Tuesday, Thursday, Saturday");
    let sunday = json!({"kind": "habit", "title": "Yoga", "moments": [{"what": "Yoga"}], "repeat": {"days": [7], "times": ["09:00"]}});
    assert_eq!(draft(&args(&sunday), "vie", PARIS, PARIS, now).unwrap().plan.repeat.unwrap().days, "0");
    // a line too long for the card: cut at a word (the bench's « Joggen – Mo, Mi, Fr, 8 Uhr, 30 Minuten locker… »)
    let long = json!({"kind": "habit", "title": "Joggen", "repeat": {"days": "daily", "times": ["08:00"]},
                      "moments": [{"what": "Joggen am Morgen, dreißig Minuten locker im Park und danach ausgiebig dehnen nicht vergessen", "say": "Los!"}]});
    let what = draft(&args(&long), "vie", PARIS, PARIS, now).unwrap().plan.moments[0].what.clone();
    assert!(what.chars().count() <= WHAT_MAX && what.ends_with('…'), "{what}");
    // what is still refused says the shape the tool takes, so the next call is right
    let none = json!({"kind": "habit", "title": "Eau", "moments": [{"what": "Eau", "say": "Un verre ?"}]});
    let said = draft(&args(&none), "vie", PARIS, PARIS, now).unwrap_err();
    assert!(said.contains("\"repeat\": {\"days\"") && said.contains("weeks"), "{said}");
    // one moment per time of day (GLM « boire plus d'eau », proactive-tune's bench): its times from them, its first words
    let water = json!({"kind": "habit", "title": "Boire plus d'eau", "moments": [
        {"at": "2026-11-25T09:00", "what": "Verre d'eau du matin", "say": "Un grand verre d'eau 💧",
         "repeat": {"days": "daily"}},
        {"at": "2026-11-25T13:00", "what": "Verre d'eau du midi", "say": "Ta gourde 💧"}]});
    // its moments said differently (« Ta gourde » at 13:00): refused with the shape, never one moment's words for all
    // (the review of 24/11)
    let said = draft(&args(&water), "vie", PARIS, PARIS, now).unwrap_err();
    assert!(said.contains("said the same way") && said.contains("\"repeat\": {\"days\""), "{said}");
    let water = json!({"kind": "habit", "title": "Boire plus d'eau", "moments": [
        {"at": "2026-11-25T09:00", "what": "Verre d'eau", "say": "Un grand verre d'eau 💧", "repeat": {"days": "daily"}},
        {"at": "2026-11-25T13:00", "what": "Verre d'eau", "say": "Un grand verre d'eau 💧"}]});
    let plan = draft(&args(&water), "vie", PARIS, PARIS, now).unwrap().plan;
    assert_eq!(plan.moments.len(), 1);
    assert_eq!(plan.moments[0].what, "Verre d'eau");
    assert_eq!(plan.repeat.unwrap().times, vec!["09:00".to_string(), "13:00".to_string()]);
    // one moment per day (GLM « courir »): its rhythm given stays; none given, the days from the moments
    let run = json!({"kind": "habit", "title": "Courir", "moments": [
        {"at": "2026-11-30T08:00", "what": "Course (lundi)", "say": "Ta course !"},
        {"at": "2026-12-02T08:00", "what": "Course (mercredi)", "say": "Ta course !"},
        {"at": "2026-12-05T08:00", "what": "Course (samedi)", "say": "Ta course !"}]});
    let plan = draft(&args(&run), "vie", PARIS, PARIS, now).unwrap().plan;
    let r = plan.repeat.unwrap();
    assert_eq!((r.days.as_str(), r.times.clone()), ("1,3,6", vec!["08:00".to_string()]));
    assert_eq!(plan.moments[0].what, "Course", "what told one day from another left out");
}

/// Fixes of the review (24/11): `plan_propose` reads a habit's rhythm as the tool writes it, refuses with the shape what
/// it would have to guess, and never merges moments that say different things.
#[test]
fn fixes_a_habits_rhythm_is_read_never_guessed() {
    let now = at("2026-11-24T14:00:00+01:00");
    let habit = |repeat: Value| json!({"kind": "habit", "title": "Yoga", "moments": [{"what": "Yoga", "say": "Yoga ?"}],
                                       "repeat": repeat});
    // « monthly » is not Monday any more; nor any word that only starts like a day
    for days in [json!("monthly"), json!("Monthly"), json!("every other week"), json!(["monthly"]), json!("sunny")] {
        let said = draft(&args(&habit(json!({"days": days, "times": ["09:00"]}))), "vie", PARIS, PARIS, now).unwrap_err();
        assert!(said.contains("repeat.days") && said.contains("\"repeat\": {\"days\""), "{days}: {said}");
    }
    // the days as words, whole or short, one or a list
    for (days, cron) in [(json!("mon"), "1"), (json!("Monday"), "1"), (json!(["Mon.", "thurs", "Sunday"]), "1,4,0"),
                         (json!("tue, fri"), "2,5"), (json!("weekdays"), "1-5")] {
        let r = draft(&args(&habit(json!({"days": days, "times": ["09:00"]}))), "vie", PARIS, PARIS, now)
            .unwrap_or_else(|e| panic!("{days}: {e}")).plan.repeat.unwrap();
        assert_eq!(r.days, cron, "{days}");
    }
    // numbers: ISO's (1 Monday … 7 Sunday); 0, 8 or a fraction refused with the shape (Python's 0 is Monday)
    let r = draft(&args(&habit(json!({"days": [1, 3, 7], "times": ["09:00"]}))), "vie", PARIS, PARIS, now).unwrap().plan.repeat.unwrap();
    assert_eq!(r.days, "1,3,0");
    for days in [json!([0, 2, 4]), json!([8]), json!([1.5]), json!(0)] {
        let said = draft(&args(&habit(json!({"days": days, "times": ["09:00"]}))), "vie", PARIS, PARIS, now).unwrap_err();
        assert!(said.contains("repeat.days") && said.contains("\"repeat\": {\"days\""), "{days}: {said}");
    }
    // two activities at two moments: refused, never « course » both days at both times
    let two = json!({"kind": "habit", "title": "Sport", "moments": [
        {"at": "2026-11-30T18:30", "what": "Course", "say": "Ta course !"},
        {"at": "2026-12-02T07:00", "what": "Piscine", "say": "Ta piscine !"}]});
    let said = draft(&args(&two), "vie", PARIS, PARIS, now).unwrap_err();
    assert!(said.contains("one plan_propose each"), "{said}");
    // one thing, but other times on other days: refused too (not every day at every time)
    let mixed = json!({"kind": "habit", "title": "Course", "moments": [
        {"at": "2026-11-30T18:30", "what": "Course", "say": "Ta course !"},
        {"at": "2026-12-02T07:00", "what": "Course", "say": "Ta course !"}]});
    let said = draft(&args(&mixed), "vie", PARIS, PARIS, now).unwrap_err();
    assert!(said.contains("each day at the same times"), "{said}");
    // two moments on one day and no days given: every day, or that day each week? asked, not « daily »
    let one_day = json!({"kind": "habit", "title": "Eau", "moments": [
        {"at": "2026-11-25T09:00", "what": "Eau", "say": "Un verre ?"},
        {"at": "2026-11-25T13:00", "what": "Eau", "say": "Un verre ?"}]});
    let said = draft(&args(&one_day), "vie", PARIS, PARIS, now).unwrap_err();
    assert!(said.contains("repeat.days: which days"), "{said}");
}

/// Verification of the fixes (25/11): a day's whole name in the plural (« Mondays », as models write « every
/// Mondays ») is that day, as before the fix (« monthly » only is not a day).
#[test]
fn verify_a_days_plural_is_that_day() {
    let now = at("2026-11-24T14:00:00+01:00");
    let habit = |repeat: Value| json!({"kind": "habit", "title": "Yoga", "moments": [{"what": "Yoga", "say": "Yoga ?"}],
                                       "repeat": repeat});
    for (days, cron) in [(json!(["Mondays", "Thursdays"]), "1,4"), (json!("Sundays"), "0"), (json!("tuesdays, fridays"), "2,5")] {
        let r = draft(&args(&habit(json!({"days": days, "times": ["09:00"]}))), "vie", PARIS, PARIS, now)
            .unwrap_or_else(|e| panic!("{days}: {e}")).plan.repeat.unwrap();
        assert_eq!(r.days, cron, "{days}");
    }
    let said = draft(&args(&habit(json!({"days": "mons", "times": ["09:00"]}))), "vie", PARIS, PARIS, now).unwrap_err();
    assert!(said.contains("repeat.days"), "{said}");
}
