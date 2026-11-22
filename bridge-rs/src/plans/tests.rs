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
        ("event", Box::new(|a: &mut Value| a["event"] = json!({"source": "google", "id": "x"}))),
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
    assert!(matches!(done, Some(Answer::Set { kept: 1, of: 3, passed: 1 })));
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
    let event = EventRef { source: "phone".into(), id: "e1".into(), start };
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
