//! plans/offers.rs' tests (contracts/offers/cases.json; design-agents-proactifs.md §3.3, §3.9, §3b.4 in basic), no
//! model anywhere.

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
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/offers/cases.json");
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn example(name: &str) -> Value {
    contract()["examples"].as_array().unwrap().iter().find(|e| e["name"] == name).unwrap().clone()
}

fn args(value: &Value) -> Dict {
    value.as_object().unwrap().clone()
}

const PARIS: Tz = chrono_tz::Europe::Paris;
const DAY: f64 = 86400.0;

/// Now, to the second: nothing here depends on the day the tests run (task-frame's review, C13).
fn now() -> f64 {
    py::time_time().floor()
}

/// « YYYY-MM-DDTHH:MM » on the person's clock, `days` from today at `hour:minute` (always to come for `days` ≥ 1).
fn day_at(days: i64, hour: u32, minute: u32) -> String {
    let today = PARIS.timestamp_opt(now() as i64, 0).single().unwrap().date_naive();
    format!("{}T{hour:02}:{minute:02}", (today + chrono::Duration::days(days)).format("%Y-%m-%d"))
}

/// « YYYY-MM-DDTHH:MM » of `at` on the person's clock.
fn local(at: f64) -> String {
    PARIS.timestamp_opt(at as i64, 0).single().unwrap().format("%Y-%m-%dT%H:%M").to_string()
}

fn epoch(text: &str) -> f64 {
    crate::plans::parse_at(text, PARIS).unwrap()
}

/// Everything this agent could do (a browser, payments, a calendar it writes to, the streets).
fn all() -> Vec<&'static str> {
    KINDS.to_vec()
}

fn offer(kind: &str, about: &str) -> Dict {
    let mut a = json!({"kind": kind, "label": "Je regarde ?", "about": about});
    match kind {
        "route" => a["params"] = json!({"to": "Cabinet du Dr Martin, Lyon", "from": "Gare Part-Dieu, Lyon"}),
        "reminder" => a["params"] = json!({"at": day_at(2, 19, 0), "say": "Dentiste demain à 15:00."}),
        "calendar" => a["params"] = json!({"title": "Dentiste", "start": day_at(4, 15, 0), "duration_minutes": 45}),
        "cart" => {
            a["prompt"] = json!("Mets le vélo au panier, jusqu’à la page de paiement.");
            a["site"] = json!("www.decathlon.fr");
        }
        _ => a["prompt"] = json!("Cherche 3 hôtels près de Baixa."),
    }
    args(&a)
}

fn read(kind: &str, about: &str) -> Offer {
    draft(&offer(kind, about), "vie", &all(), PARIS, now()).unwrap()
}

#[test]
fn the_contract_s_kinds_and_texts_are_the_bridge_s() {
    let c = contract();
    let kinds: Vec<&str> = c["tool"]["kinds"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, KINDS.to_vec());
    assert_eq!(BY_BRIDGE.to_vec(), vec!["route", "reminder", "calendar"]);
    assert_eq!(c["caps"]["offers_per_reply"], json!(PER_REPLY));
    assert_eq!(c["caps"]["same_offer_days"], json!(SAME_DAYS as u64));
    assert_eq!(c["caps"]["expires_hours"], json!((EXPIRES_SECONDS / 3600.0) as u64));
    for t in TEXTS.iter() {
        let c = &c["texts"][t.lang];
        let pairs = [
            ("top", t.top),
            ("after_once", t.after_once),
            ("after_deny", t.after_deny),
            ("expired", t.expired),
            ("notification", t.notification),
            ("summary", t.summary),
            ("first_card", t.first_card),
            ("route_done", t.route_done),
            ("route_failed", t.route_failed),
            ("calendar_failed", t.calendar_failed),
            ("not_started", t.not_started),
            ("detail_route", t.detail_route),
            ("detail_reminder", t.detail_reminder),
            ("detail_calendar", t.detail_calendar),
            ("home", t.home),
            ("phone_calendar", t.phone_calendar),
        ];
        for (key, ours) in pairs {
            assert_eq!(c[key].as_str().unwrap(), ours, "{} {key}", t.lang);
        }
        assert_eq!(c["buttons"], json!(t.buttons), "{}", t.lang);
        // the first card's line and the modes are the plans' (one wording)
        assert_eq!(t.first_card, crate::plans::texts(t.lang).first_card);
        for (i, mode) in crate::plans::MODES.iter().enumerate() {
            assert_eq!(c["modes"][mode].as_str().unwrap(), crate::plans::texts(t.lang).modes[i], "{} {mode}", t.lang);
        }
    }
    // the hotels example, as validated
    let hotels = example("hotels");
    let card = &hotels["card"];
    let label = hotels["arguments"]["label"].as_str().unwrap();
    assert_eq!(summary("fr", "Vie", label), card["bip_summary"].as_str().unwrap());
    assert_eq!(notification("fr", label), card["notification"].as_str().unwrap());
    assert_eq!(top("fr", "Vie", "Lisbonne"), card["top"].as_str().unwrap());
}

#[test]
fn an_offer_kind_the_agent_cannot_do_is_refused_and_not_in_its_tool_description() {
    // what it can do here, now: nothing but what its tools and rights allow
    let bare = Can { streets: false, routines: false, calendar: None, web: false, browser: false, cart: false };
    assert_eq!(kinds_of(&bare), vec!["draft", "page"]);
    let some = Can { streets: true, routines: true, calendar: Some(Calendar::Phone), web: true, browser: false, cart: false };
    assert_eq!(kinds_of(&some), vec!["route", "reminder", "calendar", "search", "draft", "page"]);
    // a browser alone searches too; a cart needs its browser as well as payments
    let browser = Can { web: false, browser: true, cart: true, ..bare.clone() };
    assert_eq!(kinds_of(&browser), vec!["search", "draft", "page", "cart"]);
    let no_browser = Can { cart: true, ..bare.clone() };
    assert!(!kinds_of(&no_browser).contains(&"cart"));
    // its tool says only those, and lists only those
    let kinds = kinds_of(&some);
    let t = tool(&kinds, None);
    assert_eq!(t["name"], TOOL_NAME);
    assert_eq!(t["inputSchema"]["properties"]["kind"]["enum"], json!(kinds));
    let description = t["description"].as_str().unwrap();
    for k in KINDS {
        let line = format!("- {k}:");
        assert_eq!(description.contains(&line), kinds.contains(&k), "{k} in « {description} »");
    }
    assert!(description.contains("never book, pay or send"), "{description}");
    // refused: what it can't do here, and what is no kind at all
    let refused = draft(&offer("cart", "Vélo"), "vie", &kinds, PARIS, now()).unwrap_err();
    assert!(refused.starts_with("Not done: you can't") && refused.contains("don't offer it"), "{refused}");
    let mut odd = offer("search", "Lisbonne");
    odd.insert("kind".into(), json!("book"));
    let refused = draft(&odd, "vie", &kinds, PARIS, now()).unwrap_err();
    assert!(refused.starts_with("Not done: kind:"), "{refused}");
}

#[test]
fn route_reminder_and_calendar_take_params_the_others_a_prompt() {
    for kind in KINDS {
        let o = read(kind, "Lisbonne");
        assert_eq!(o.kind, kind);
        assert_eq!(by_bridge(&o), BY_BRIDGE.contains(&kind), "{kind}");
        assert_eq!(o.prompt.is_empty(), BY_BRIDGE.contains(&kind), "{kind}");
        assert_eq!(o.params.is_some(), BY_BRIDGE.contains(&kind), "{kind}");
    }
    // a prompt where the bridge does it, params where the agent does: refused
    let mut wrong = offer("reminder", "Dentiste");
    wrong.insert("prompt".into(), json!("Rappelle-lui."));
    assert!(draft(&wrong, "vie", &all(), PARIS, now()).unwrap_err().starts_with("Not done:"));
    let mut wrong = offer("search", "Lisbonne");
    wrong.remove("prompt");
    assert!(draft(&wrong, "vie", &all(), PARIS, now()).unwrap_err().starts_with("Not done:"));
    // plain text only: no markup, no break, no http
    for bad in ["Cherche <b>3</b> hôtels", "Cherche\n3 hôtels", "Voir http://example.com", "**3** hôtels"] {
        let mut a = offer("search", "Lisbonne");
        a.insert("prompt".into(), json!(bad));
        assert!(draft(&a, "vie", &all(), PARIS, now()).unwrap_err().starts_with("Not done:"), "{bad}");
    }
    let mut long = offer("search", "Lisbonne");
    long.insert("prompt".into(), json!("a".repeat(PROMPT_MAX + 1)));
    assert!(draft(&long, "vie", &all(), PARIS, now()).is_err());
    let mut long = offer("search", "Lisbonne");
    long.insert("label".into(), json!("a".repeat(LABEL_MAX + 1)));
    assert!(draft(&long, "vie", &all(), PARIS, now()).is_err());
    // a reminder or an event not to come
    let mut past = offer("reminder", "Dentiste");
    past.insert("params".into(), json!({"at": day_at(-2, 19, 0), "say": "Trop tard."}));
    assert!(draft(&past, "vie", &all(), PARIS, now()).unwrap_err().starts_with("Not done: params"));
    let mut past = offer("calendar", "Dentiste");
    past.insert("params".into(), json!({"title": "Dentiste", "start": day_at(-2, 15, 0)}));
    assert!(draft(&past, "vie", &all(), PARIS, now()).unwrap_err().starts_with("Not done: params"));
    // a cart names its shop
    let mut shop = offer("cart", "Vélo");
    shop.remove("site");
    assert!(draft(&shop, "vie", &all(), PARIS, now()).unwrap_err().starts_with("Not done: site"));
    // the reminder's moment on the person's clock
    let r = read("reminder", "Dentiste");
    let at = r.params.as_ref().unwrap()["at"].as_f64().unwrap();
    assert_eq!(at, epoch(&day_at(2, 19, 0)));
}

#[test]
fn an_offer_expires_after_24h_or_when_its_moment_has_passed() {
    let n = now();
    let search = draft(&offer("search", "Lisbonne"), "vie", &all(), PARIS, n).unwrap();
    assert_eq!(search.expires, n + EXPIRES_SECONDS);
    // a reminder within the day: its moment first
    let mut soon = offer("reminder", "Four");
    soon.insert("params".into(), json!({"at": local(n + 7200.0), "say": "Sors le gâteau du four."}));
    let soon = draft(&soon, "vie", &all(), PARIS, n).unwrap();
    assert_eq!(soon.expires, epoch(&local(n + 7200.0)));
    // taken out once expired, counted as such, never told
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    change(&config, |s| s.offers.extend([search.clone(), soon.clone()]));
    assert!(expire(&config, n + 3600.0).is_empty());
    let gone = expire(&config, soon.expires + 1.0);
    assert_eq!(gone.iter().map(|o| o.id.clone()).collect::<Vec<_>>(), vec![soon.id.clone()]);
    let store = load(&config);
    assert_eq!(store.offers.len(), 1);
    assert_eq!(store.answers.iter().filter(|a| a.outcome == EXPIRED).count(), 1);
    // an expired one can't be answered
    assert!(answer(&config, "vie", &soon.id, true, soon.expires + 2.0).is_none());
    assert!(pending_items(&config, Some("vie")).iter().all(|i| i["request_id"] != json!(soon.id)));
}

#[test]
fn two_offers_per_reply_and_the_unasked_caps_shared_with_plans() {
    let n = now();
    let plans = crate::plans::Store::default();
    let mut store = Store::default();
    let mut asked = read("search", "Lisbonne");
    asked.asked = true;
    // asked, in its person's turn: two per reply
    for about in ["Lisbonne", "Porto"] {
        let mut o = read("search", about);
        o.asked = true;
        assert!(refused(&store, &plans, &o, Some("run_1"), PARIS, now()).is_none(), "{about}");
        store.made.push(Made { agent: "vie".into(), at: n, asked: true, run: Some("run_1".into()), pill: false });
        store.offers.push(o);
    }
    let third = refused(&store, &plans, &asked, Some("run_1"), PARIS, now()).unwrap();
    assert!(third.starts_with("Not done: two offers per reply"), "{third}");
    // the next reply may again
    let mut next = read("page", "Bagages");
    next.asked = true;
    assert!(refused(&store, &plans, &next, Some("run_2"), PARIS, now()).is_none());
    // a plan's offers on its card count in the reply's two
    let mut with_plan = Store::default();
    with_plan.made.push(Made { agent: "vie".into(), at: n, asked: true, run: Some("run_3".into()), pill: true });
    with_plan.made.push(Made { agent: "vie".into(), at: n, asked: true, run: Some("run_3".into()), pill: true });
    assert!(refused(&with_plan, &plans, &next, Some("run_3"), PARIS, now()).is_some());
    // unasked: one waiting (an offer or a plan), 2 a day for the agent, 4 for the place, plans and offers together
    let unasked = read("page", "Courses");
    let mut store = Store::default();
    store.offers.push(read("search", "Lisbonne"));
    let waiting = refused(&store, &plans, &unasked, None, PARIS, now()).unwrap();
    assert!(waiting.starts_with("Not done: one of your cards already waits"), "{waiting}");
    let mut plans_waiting = crate::plans::Store::default();
    plans_waiting.plans.push(crate::plans::Plan {
        id: "p1".into(), agent: "vie".into(), kind: "trip".into(), title: "Lisbonne".into(), city: "Lisbonne".into(),
        zone: "Europe/Lisbon".into(), moments: Vec::new(), repeat: None, watch: Vec::new(), event: None,
        state: crate::plans::WAITING.into(), made: n, asked: false, first: false, origin: String::new(),
        routines: Vec::new(), trigger: None, ends: 0.0,
    });
    assert!(refused(&Store::default(), &plans_waiting, &unasked, None, PARIS, now()).is_some());
    let mut day = crate::plans::Store::default();
    day.made.push(crate::plans::Made { agent: "vie".into(), at: n, asked: false, run: None });
    let mut store = Store::default();
    store.made.push(Made { agent: "vie".into(), at: n, asked: false, run: None, pill: false });
    let two = refused(&store, &day, &unasked, None, PARIS, now()).unwrap();
    assert!(two.starts_with("Not done: you already made 2"), "{two}");
    // asked cards count in no daily cap
    let mut asked_day = Store::default();
    for _ in 0..3 {
        asked_day.made.push(Made { agent: "vie".into(), at: n, asked: true, run: Some("r".into()), pill: false });
    }
    assert!(refused(&asked_day, &crate::plans::Store::default(), &unasked, None, PARIS, now()).is_none());
    let mut place = crate::plans::Store::default();
    for agent in ["budget", "coach"] {
        for _ in 0..2 {
            place.made.push(crate::plans::Made { agent: agent.into(), at: n, asked: false, run: None });
        }
    }
    let four = refused(&Store::default(), &place, &unasked, None, PARIS, now()).unwrap();
    assert!(four.starts_with("Not done: your person's agents made 4"), "{four}");
    // and the other way: an offer made unasked today counts for the plans
    let mut offers_day = Store::default();
    for _ in 0..2 {
        offers_day.made.push(Made { agent: "vie".into(), at: n, asked: false, run: None, pill: false });
    }
    assert_eq!(unasked_today(&offers_day, Some("vie"), PARIS, now()), 2);
    assert_eq!(unasked_today(&offers_day, None, PARIS, now()), 2);
    assert_eq!(unasked_today(&offers_day, Some("budget"), PARIS, now()), 0);
}

#[test]
fn the_same_offer_is_not_made_twice_in_14_days() {
    let n = now();
    let plans = crate::plans::Store::default();
    let mut store = Store::default();
    let mut o = read("search", "Lisbonne");
    o.asked = true;
    remember(&mut store, &o, n);
    // the same kind and subject (case and spaces aside), whatever was answered
    let mut again = read("search", "  lisbonne ");
    again.asked = true;
    let said = refused(&store, &plans, &again, Some("run_9"), PARIS, n + 13.0 * DAY).unwrap();
    assert!(said.starts_with("Not done: you offered"), "{said}");
    // another kind on the same subject, or 14 days later: offered
    let mut other = read("page", "Lisbonne");
    other.asked = true;
    assert!(refused(&store, &plans, &other, Some("run_9"), PARIS, n + DAY).is_none());
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    change(&config, |s| remember(s, &o, n - 15.0 * DAY));
    assert!(load(&config).same.is_empty(), "kept no longer than 14 days");
}

#[test]
fn two_no_thanks_in_a_row_quiet_that_kind_for_30_days_not_off() {
    let n = now();
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let mut store = Store::default();
    record(&mut store, "vie", "page", "Bagages", NO, n);
    assert!(!quiet(&store, "vie", "page", n));
    // an expiry in between breaks nothing
    record(&mut store, "vie", "page", "Courses", EXPIRED, n + 10.0);
    record(&mut store, "vie", "page", "Menus", NO, n + 20.0);
    assert!(quiet(&store, "vie", "page", n + 30.0));
    assert!(quiet(&store, "vie", "page", n + 29.0 * DAY) && !quiet(&store, "vie", "page", n + 31.0 * DAY));
    // for that agent and kind only
    assert!(!quiet(&store, "budget", "page", n + 30.0) && !quiet(&store, "vie", "search", n + 30.0));
    assert_eq!(available(&store, "vie", &all(), n + 30.0), vec!["route", "reminder", "calendar", "search", "draft", "cart"]);
    let refused_kind = refused(&store, &crate::plans::Store::default(), &read("page", "Liste"), None, PARIS, n + 30.0)
        .unwrap();
    assert!(refused_kind.contains("said no thanks"), "{refused_kind}");
    // a « Oui » starts the series again
    let mut store = Store::default();
    record(&mut store, "vie", "search", "A", NO, n);
    record(&mut store, "vie", "search", "B", YES, n + 1.0);
    record(&mut store, "vie", "search", "C", NO, n + 2.0);
    assert!(!quiet(&store, "vie", "search", n + 3.0));
    // never « Éteinte »
    assert_eq!(crate::agent_proactive::level(&config, "vie"), crate::agent_proactive::Level::Basic);
}

#[test]
fn a_low_yes_rate_quiets_a_kind() {
    let n = now();
    let mut store = Store::default();
    // 5 offers: 1 yes, 4 expired (each half a no): 1 / (1 + 2) = 33 %: not quiet
    record(&mut store, "vie", "search", "A", YES, n);
    for (i, t) in ["B", "C", "D", "E"].iter().enumerate() {
        record(&mut store, "vie", "search", t, EXPIRED, n + 1.0 + i as f64);
    }
    assert!(!quiet(&store, "vie", "search", n + 10.0));
    // 10 offers, 1 yes: 1 / (1 + 9 × 0.5 + …) under 20 %: quiet
    for (i, t) in ["F", "G", "H", "I", "J"].iter().enumerate() {
        record(&mut store, "vie", "search", t, EXPIRED, n + 10.0 + i as f64);
    }
    assert!(quiet(&store, "vie", "search", n + 20.0));
    // older than 30 days: forgotten
    let mut old = Store::default();
    for i in 0..6 {
        record(&mut old, "vie", "draft", &format!("T{i}"), EXPIRED, n - 40.0 * DAY);
    }
    assert!(!quiet(&old, "vie", "draft", n));
}

#[test]
fn a_declined_subject_is_not_offered_again_for_30_days() {
    let n = now();
    let plans = crate::plans::Store::default();
    let mut store = Store::default();
    record(&mut store, "vie", "search", "Mariage samedi", NO, n);
    let mut o = read("page", "mariage  Samedi");
    o.asked = true;
    let said = refused(&store, &plans, &o, Some("r"), PARIS, n + 20.0 * DAY).unwrap();
    assert!(said.starts_with("Not done: your person said no"), "{said}");
    assert!(refused(&store, &plans, &o, Some("r"), PARIS, n + 31.0 * DAY).is_none());
}

#[test]
fn the_learning_line_says_what_its_person_declines_and_accepts() {
    let n = now();
    let mut store = Store::default();
    assert_eq!(learning_line(&store, "vie", &all(), n), None);
    // three answers at least: under a third yes, declines; two thirds or more, accepts
    for (t, o) in [("A", YES), ("B", YES), ("C", YES)] {
        record(&mut store, "vie", "search", t, o, n);
    }
    for (t, o) in [("D", NO), ("E", YES), ("F", NO)] {
        record(&mut store, "vie", "cart", t, o, n);
    }
    record(&mut store, "vie", "route", "G", YES, n);
    let line = learning_line(&store, "vie", &all(), n).unwrap();
    assert_eq!(line, "Your person often accepts: searches.");
    record(&mut store, "vie", "cart", "H", EXPIRED, n);
    record(&mut store, "vie", "cart", "I", NO, n);
    let line = learning_line(&store, "vie", &all(), n).unwrap();
    assert_eq!(line, "Your person often accepts: searches.", "cart is quiet now: out of its list, out of the line");
    record(&mut store, "vie", "page", "J", NO, n);
    record(&mut store, "vie", "page", "K", YES, n);
    record(&mut store, "vie", "page", "L", NO, n);
    record(&mut store, "vie", "page", "M", EXPIRED, n);
    let kinds = available(&store, "vie", &all(), n);
    assert_eq!(learning_line(&store, "vie", &kinds, n).unwrap(), "Your person often declines: pages. Often accepts: searches.");
    // the tool says it
    let t = tool(&kinds, learning_line(&store, "vie", &kinds, n).as_deref());
    assert!(t["description"].as_str().unwrap().contains("often declines: pages"));
}

#[test]
fn an_offer_is_critical_and_a_calendar_offer_is_the_calendar_write() {
    use crate::approval_level::{classify_card, offers_always, rule_of};
    let card = |kind: &str| json!({"run_id": "bip-offer-abc", "bip_offer": {"kind": kind}}).as_object().unwrap().clone();
    assert_eq!(classify_card(&card("search")), KIND);
    assert_eq!(classify_card(&card("calendar")), "calendar_write");
    assert!(rule_of(KIND).critical && rule_of("calendar_write").critical);
    assert!(!offers_always(KIND));
}

#[test]
fn the_card_is_a_generic_yes_no_card_with_its_prompt_under_i() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config_in(dir.path());
    config.agents.get_mut("vie").unwrap().display_name = "Vie".into();
    let hotels = example("hotels");
    let mut o = draft(&args(&hotels["arguments"]), "vie", &all(), PARIS, now()).unwrap();
    o.first = true;
    let item = card_item(&config, &o);
    assert_eq!(item["run_id"], json!(format!("{RUN_PREFIX}{}", o.id)));
    assert_eq!(item["request_id"], json!(o.id));
    assert_eq!(item["choices"], json!(["once", "deny"]));
    let lang = crate::plans::texts(lang_of(&config, "vie")).lang;
    let t = texts(lang);
    let label = hotels["arguments"]["label"].as_str().unwrap();
    assert_eq!(item["bip_summary"], json!(summary(lang, "Vie", label)));
    assert_eq!(item["description"], item["bip_summary"]);
    assert_eq!(item["command"], json!(format!("{TOOL_NAME} · {label}")));
    // under « i »: exactly what the agent will be asked, then where its suggestions are set (its first card)
    let prompt = hotels["arguments"]["prompt"].as_str().unwrap();
    assert_eq!(item["bip_detail"], json!(format!("{prompt}\n{}", t.first_card)));
    let shown = &item["bip_offer"];
    assert_eq!(shown["kind"], json!("search"));
    assert_eq!(shown["about"], json!("Lisbonne"));
    assert_eq!(shown["label"], json!(label));
    assert_eq!(shown["agent"], json!("Vie"));
    assert_eq!(shown["mode"], json!("basic"));
    assert_eq!(shown["plan"], Value::Null);
    assert_eq!(shown["pill"], json!(false));
    assert_eq!(shown["expires_at"], json!(o.expires));
    // what the bridge does, said exactly
    let reminder = example("reminder");
    let mut r = read("reminder", "Dentiste");
    let given = &reminder["arguments"]["params"];
    r.params = Some(json!({"at": epoch(given["at"].as_str().unwrap()), "say": given["say"]}));
    assert_eq!(detail("fr", &r, PARIS), reminder["card"]["bip_detail"].as_str().unwrap());
    let route = read("route", "Dentiste");
    assert_eq!(detail("fr", &route, PARIS), "Le trajet à pied de Gare Part-Dieu, Lyon à Cabinet du Dr Martin, Lyon");
    let mut home = offer("route", "Dentiste");
    home.insert("params".into(), json!({"to": "Cabinet du Dr Martin", "mode": "driving"}));
    let home = draft(&home, "vie", &all(), PARIS, now()).unwrap();
    assert_eq!(detail("fr", &home, PARIS), "Le trajet en voiture de chez toi à Cabinet du Dr Martin");
    let event = read("calendar", "Dentiste");
    assert!(detail("fr", &event, PARIS).starts_with("Dans "), "{}", detail("fr", &event, PARIS));
    let when = crate::plans::when_text("fr", epoch(&day_at(4, 15, 0)), PARIS, PARIS, "");
    assert!(detail("fr", &event, PARIS).contains(&format!("« Dentiste », {when}–15:45")), "{}", detail("fr", &event, PARIS));
}

#[test]
fn the_yes_note_says_exactly_the_prompt_and_drafts_only() {
    let o = read("search", "Lisbonne");
    let note = yes_note(&o);
    assert!(note.contains(&format!("Your person said yes to your offer: « {} ». Do exactly that now.", o.prompt)), "{note}");
    assert!(note.contains("Drafts only: never book, pay or send; anything like that goes through its own card."));
    assert!(note.starts_with("<!--bip:") && note.trim_end().ends_with("-->"));
}

#[test]
fn the_answer_counts_for_its_kind_and_a_second_answer_finds_nothing() {
    let n = now();
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let o = read("page", "Bagages");
    change(&config, |s| s.offers.push(o.clone()));
    assert!(answer(&config, "budget", &o.id, true, n).is_none(), "another agent's card");
    let taken = answer(&config, "vie", &o.id, false, n + 1.0).unwrap();
    assert_eq!(taken.id, o.id);
    assert!(answer(&config, "vie", &o.id, true, n + 2.0).is_none());
    let store = load(&config);
    assert!(store.offers.is_empty());
    assert_eq!(store.answers.iter().map(|a| (a.kind.as_str(), a.outcome.as_str())).collect::<Vec<_>>(), vec![("page", NO)]);
    // a plan's « Non merci »: its offers on its card go, counted for nothing
    let mut pill = read("search", "Lisbonne");
    pill.plan = Some("p1".into());
    pill.pill = true;
    change(&config, |s| s.offers.push(pill.clone()));
    withdraw_plan(&config, "p1");
    let store = load(&config);
    assert!(store.offers.is_empty() && store.answers.len() == 1);
}

/// Review of offers (23/11, R3): a calendar offer's card « vaut celle de calendar_write : elle montre exactement
/// l'événement » (design §3.3). A field the card does not show (calendar.rs `new_event`'s `notes`, 2000 characters,
/// never checked: links in http, line breaks) is never written: refused, or shown.
#[test]
fn review_a_calendar_offer_writes_nothing_its_card_does_not_show() {
    let mut a = offer("calendar", "Dentiste");
    let notes = "Paie l’acompte sur http://example.com/pay avant jeudi.";
    a.insert(
        "params".into(),
        json!({"title": "Dentiste", "start": day_at(4, 15, 0), "duration_minutes": 45, "notes": notes}),
    );
    match draft(&a, "vie", &all(), PARIS, now()) {
        Err(why) => assert!(why.starts_with("Not done: params"), "{why}"),
        Ok(o) => {
            let written = o.params.as_ref().unwrap()["event"]["notes"].as_str().unwrap_or("").to_string();
            let shown = detail("fr", &o, PARIS);
            assert!(written.is_empty() || shown.contains(&written), "written « {written} », shown « {shown} »");
        }
    }
}

/// Review of offers (23/11, R7): an event given with its offset (« …T15:00:00+00:00 ») is shown on the person's clock,
/// its end too (not its start on the person's clock and its end as written).
#[test]
fn review_a_calendar_offer_shows_its_start_and_end_on_the_same_clock() {
    let day = &day_at(4, 0, 0)[..10];
    let mut a = offer("calendar", "Dentiste");
    a.insert(
        "params".into(),
        json!({"title": "Dentiste", "start": format!("{day}T15:00:00+00:00"), "end": format!("{day}T15:45:00+00:00")}),
    );
    let o = draft(&a, "vie", &all(), PARIS, now()).unwrap();
    let start = chrono::DateTime::parse_from_rfc3339(&format!("{day}T15:00:00+00:00")).unwrap().timestamp() as f64;
    let shown = detail("fr", &o, PARIS);
    let start_text = crate::plans::when_text("fr", start, PARIS, PARIS, "");
    let end_clock = PARIS.timestamp_opt(start as i64 + 45 * 60, 0).single().unwrap().format("%H:%M").to_string();
    assert!(shown.contains(&format!("{start_text}–{end_clock}")), "« {shown} »: {start_text}–{end_clock}");
}

/// Review of offers (23/11, R2): an offer's card never offers « Toujours » (critical, design §3.4), a calendar one
/// neither, whose kind is `calendar_write`: branch calendar-always (7f9093677) gives that kind a « Toujours » of its
/// own on every bridge card (approval_level.rs `with_always`), and app.rs `approve` would then keep, on a « Oui » to
/// one offer, a standing right to write in the person's Google Calendar. Green on `offers` alone; red once merged with
/// calendar-always.
#[test]
fn review_an_offer_card_never_offers_toujours() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    for kind in KINDS {
        let mut o = read(kind, "Dentiste");
        if kind == "calendar" {
            o.params.as_mut().unwrap()["calendar"] = json!("google");
        }
        let item = crate::approval_level::with_always(&config, card_item(&config, &o));
        assert!(item.get("bip_always").is_none() && item.get("bip_always_kind").is_none(), "{kind}: {item:?}");
    }
}

/// Review of offers (23/11, R4): the plan « Lisbonne » shows a search on its card (a pill); its moment two days later
/// offers another search (accepted with « Prévoir »: its line was on the plan's « i »). That moment's offer is not
/// « the same offer » as the pill, nor « a subject declined » when the pill got a « Non merci »: placed at its time.
#[test]
fn review_a_plan_s_moment_offer_is_not_held_by_another_offer_of_its_plan() {
    let n = now();
    let plans = crate::plans::Store::default();
    let pill_spec = json!({"kind": "search", "label": "Je regarde les hôtels ?", "prompt": "Cherche 3 hôtels près de Baixa."});
    let mut pill = spec_of(&pill_spec, "vie", "Lisbonne", &all(), PARIS, n, PILL_MAX).unwrap();
    (pill.plan, pill.pill, pill.asked) = (Some("p1".into()), true, true);
    let moment_spec = json!({"kind": "search", "label": "Je cherche un resto près du Chiado pour ce soir ?",
                             "prompt": "Cherche 3 restaurants près du Chiado, ouverts ce soir."});
    let later = n + 2.0 * DAY;
    let mut moment = spec_of(&moment_spec, "vie", "Lisbonne", &all(), PARIS, later, LABEL_MAX).unwrap();
    moment.plan = Some("p1".into());
    // the pill made with the plan
    let mut store = Store::default();
    remember(&mut store, &pill, n);
    let held = refused(&store, &plans, &moment, None, PARIS, later);
    assert!(held.is_none(), "held by its own plan's pill: {held:?}");
    // the pill declined
    let mut store = Store::default();
    record(&mut store, "vie", "search", &pill.about, NO, n + 60.0);
    let held = refused(&store, &plans, &moment, None, PARIS, later);
    assert!(held.is_none(), "held by its own plan's declined pill: {held:?}");
}

#[test]
fn a_plan_s_offers_and_a_moment_s_offer_are_read_with_the_same_checks() {
    let n = now();
    let kinds = vec!["search", "page", "reminder"];
    // a plan's offer: about is the plan's title, label 60 at most, a pill
    let spec = json!({"kind": "search", "label": "Je regarde les hôtels ?", "prompt": "Cherche 3 hôtels près de Baixa."});
    let pill = spec_of(&spec, "vie", "Lisbonne", &kinds, PARIS, n, PILL_MAX).unwrap();
    assert_eq!((pill.about.as_str(), pill.kind.as_str()), ("Lisbonne", "search"));
    let long = json!({"kind": "search", "label": "a".repeat(PILL_MAX + 1), "prompt": "x"});
    assert!(spec_of(&long, "vie", "Lisbonne", &kinds, PARIS, n, PILL_MAX).is_err());
    let cannot = json!({"kind": "cart", "label": "Panier ?", "prompt": "x", "site": "a.fr"});
    assert!(spec_of(&cannot, "vie", "Lisbonne", &kinds, PARIS, n, PILL_MAX).unwrap_err().starts_with("Not done: you can't"));
    // a moment that offers says nothing: a line to tell, a card to decide
    let moment = |extra: Value| {
        let mut m = json!({"at": day_at(1, 18, 0), "what": "Liste de bagages",
                           "offer": {"kind": "page", "label": "Il pleuvra à Lisbonne. Je te fais la liste de bagages ?",
                                     "prompt": "Fais une page avec la liste de bagages pour 3 jours de pluie."}});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        json!({"kind": "trip", "title": "Lisbonne", "zone": "Europe/Lisbon", "moments": [m]})
    };
    let d = crate::plans::draft(&args(&moment(json!({}))), "vie", PARIS, PARIS, now()).unwrap();
    let m = &d.plan.moments[0];
    assert!(m.offer.is_some() && m.say.is_empty());
    let both = crate::plans::draft(&args(&moment(json!({"say": "Il pleuvra."}))), "vie", PARIS, PARIS, now()).unwrap_err();
    assert!(both.contains("offer"), "{both}");
}

// -- review of offers (23/11): the author's red tests -------------------------------------------------------------------

/// R3: what the bridge does at « Oui » takes a closed list of fields, kind by kind; anything else is refused, never kept
/// out of the card's sight.
#[test]
fn r3_params_are_a_closed_list_for_every_kind() {
    for (kind, extra) in
        [("route", "via"), ("reminder", "urgent"), ("calendar", "notes"), ("calendar", "attendees"), ("calendar", "calendar")]
    {
        let mut a = offer(kind, "Dentiste");
        let mut p = a.get("params").cloned().unwrap();
        p[extra] = json!("x");
        a.insert("params".into(), p);
        let why = draft(&a, "vie", &all(), PARIS, now()).unwrap_err();
        assert!(why.starts_with(&format!("Not done: params.{extra}")), "{kind}: {why}");
    }
    // what the card shows, and only that, is written
    let o = read("calendar", "Dentiste");
    let event = o.params.as_ref().unwrap()["event"].as_object().unwrap().clone();
    let written: Vec<&str> = event.iter().filter(|(_, v)| v.as_str().is_some_and(|s| !s.is_empty())).map(|(k, _)| k.as_str()).collect();
    assert!(written.iter().all(|k| ["title", "start", "end"].contains(k)), "{written:?}");
}

fn plan_of(id: &str, state: &str, n: f64) -> crate::plans::Plan {
    crate::plans::Plan {
        id: id.into(), agent: "vie".into(), kind: "trip".into(), title: "Lisbonne".into(), city: "Lisbonne".into(),
        zone: "Europe/Lisbon".into(), moments: Vec::new(), repeat: None, watch: Vec::new(), event: None,
        state: state.into(), made: n, asked: true, first: false, origin: String::new(), routines: Vec::new(),
        trigger: None, ends: n + 3.0 * DAY,
    }
}

fn pill_of(plan: &str, kind: &str, label: &str, n: f64) -> Offer {
    let mut spec = json!({"kind": kind, "label": label, "prompt": format!("{label} : fais-le.")});
    if kind == "reminder" {
        spec = json!({"kind": kind, "label": label, "params": {"at": day_at(2, 9, 0), "say": "Rappel."}});
    }
    let mut p = spec_of(&spec, "vie", "Lisbonne", &all(), PARIS, n, PILL_MAX).unwrap();
    (p.plan, p.pill, p.asked) = (Some(plan.into()), true, true);
    p
}

/// R5: a plan that goes without « Non merci » (its event cancelled, its end, « Initiative » off) takes its pills with it,
/// counted for nothing.
#[test]
fn r5_a_plan_gone_takes_its_pills_with_it() {
    let n = now();
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let plan = plan_of("p1", crate::plans::SET, n);
    crate::plans::change(&config, |s| s.plans.push(plan.clone()));
    change(&config, |s| {
        s.offers.push(pill_of("p1", "search", "Je regarde les hôtels ?", n));
        s.offers.push(pill_of("p2", "page", "Je te fais la liste ?", n));
    });
    crate::plans::remove(&config, &plan);
    let left = load(&config);
    assert_eq!(left.offers.iter().map(|o| o.plan.clone().unwrap()).collect::<Vec<_>>(), vec!["p2".to_string()]);
    assert!(left.answers.is_empty());
}

/// R7: at « Oui », the calendar of the card is checked again, as on the card: Google Calendar still given to the
/// agent, the phone's still shared with it in writing.
#[test]
fn r7_a_calendar_yes_checks_its_right_again() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let mut o = read("calendar", "Dentiste");
    o.params.as_mut().unwrap()["calendar"] = json!("google");
    assert!(!still_writes(&config, "vie", &o), "Google Calendar not given to Vie");
    o.params.as_mut().unwrap()["calendar"] = json!("phone");
    assert!(!still_writes(&config, "vie", &o), "the phone's calendar not shared with Vie");
    crate::calendar::allow(&config, &["vie".to_string()]).unwrap();
    assert!(still_writes(&config, "vie", &o));
}

/// Choice 2 (the coordinator, design §3.2 validated by Alex): a moment's offer held by the caps waits for the next day,
/// at the same time, while its plan lasts; never lost in silence.
#[test]
fn choice2_a_held_moment_offer_waits_for_the_next_day_while_its_plan_lasts() {
    let n = now();
    assert_eq!(next_try(n, n + 3.0 * DAY), Some(n + DAY));
    assert_eq!(next_try(n, n + 0.5 * DAY), None, "its plan over by then");
    // held by the caps only: a rule (the same offer, a subject declined, a kind quiet) drops it
    let mut store = Store::default();
    let mut moment = read("page", "Lisbonne · Bagages");
    moment.plan = Some("p1".into());
    assert!(rule_refusal(&store, &moment, n).is_none());
    remember(&mut store, &moment, n);
    assert!(rule_refusal(&store, &moment, n).is_some());
}

/// Choice 4 (the coordinator): on an app 1.0 each pill is a card of its own, so a plan's pills count in the day's cap
/// of cards nobody asked for: an unasked plan and its pills must fit in it.
#[test]
fn choice4_a_plan_s_pills_count_in_the_unasked_day() {
    let n = now();
    let mut store = Store::default();
    store.made.push(Made { agent: "vie".into(), at: n, asked: false, run: None, pill: true });
    assert_eq!(unasked_today(&store, Some("vie"), PARIS, n), 1);
    let plans = crate::plans::Store::default();
    let empty = Store::default();
    assert!(pills_room(&empty, &plans, "vie", 1, PARIS, n).is_none(), "the plan and one pill: 2");
    let over = pills_room(&empty, &plans, "vie", 2, PARIS, n).unwrap();
    assert!(over.starts_with("Not done: offers:"), "{over}");
    assert!(pills_room(&store, &plans, "vie", 1, PARIS, n).is_some(), "one card made already");
}

/// R8: the day of the caps is the person's, wherever the server is.
#[test]
fn r8_the_day_of_the_caps_is_the_person_s() {
    let n = now();
    let far = chrono_tz::Pacific::Auckland;
    let midnight = far
        .from_local_datetime(&far.timestamp_opt(n as i64, 0).single().unwrap().date_naive().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp() as f64;
    let mut store = Store::default();
    store.made.push(Made { agent: "vie".into(), at: midnight - 1800.0, asked: false, run: None, pill: false });
    store.made.push(Made { agent: "vie".into(), at: midnight + 60.0, asked: false, run: None, pill: false });
    assert_eq!(unasked_today(&store, Some("vie"), far, midnight + 120.0), 1);
}

/// Remark: a subject said again in other words of case, accents or punctuation is the same subject.
#[test]
fn remark_a_subject_reworded_by_case_accents_or_punctuation_is_the_same() {
    assert_eq!(same_print("vie", "search", "Séville"), same_print("vie", "search", "  seville ! "));
    assert_eq!(topic_print("vie", "Mariage samedi"), topic_print("vie", "mariage, Samedi…"));
    assert_ne!(topic_print("vie", "Lisbonne"), topic_print("vie", "Porto"));
}

/// Remark: the turn of « Oui » reads what its kind allows, so another kind can't do a quiet kind's work.
#[test]
fn remark_the_yes_note_says_what_its_kind_allows() {
    let note = yes_note(&read("page", "Bagages"));
    assert!(note.contains("It was offered as a page: "), "{note}");
    assert!(note.contains("nothing else"), "{note}");
}

/// Remark: an offer made in a turn that read a message from outside (an e-mail's, a link's trigger) is said so on its
/// card and in the note of its « Oui ».
#[test]
fn remark_an_offer_made_after_outside_content_is_said_on_its_card_and_in_its_note() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let mut o = read("search", "Lisbonne");
    o.outside = true;
    let lang = crate::plans::texts(lang_of(&config, "vie")).lang;
    let item = card_item(&config, &o);
    assert!(item["bip_detail"].as_str().unwrap().starts_with(texts(lang).outside), "{:?}", item["bip_detail"]);
    assert_eq!(item["bip_offer"]["outside"], json!(true));
    assert!(yes_note(&o).contains("a message from outside"), "{}", yes_note(&o));
    assert!(!yes_note(&read("search", "Lisbonne")).contains("a message from outside"));
}

/// Remark: a « Oui » whose work had not ended when the bridge stopped is done after its restart, once.
#[test]
fn remark_a_yes_not_done_yet_is_done_after_a_restart() {
    let n = now();
    let mut store = Store::default();
    let o = read("page", "Bagages");
    store.doing.push(Doing { offer: o.clone(), session: None, report: None, at: n - 120.0, tries: 1 });
    let running = std::collections::HashSet::new();
    assert_eq!(stale(&store, &running, n).iter().map(|d| d.offer.id.clone()).collect::<Vec<_>>(), vec![o.id.clone()]);
    let mut busy = std::collections::HashSet::new();
    busy.insert(o.id.clone());
    assert!(stale(&store, &busy, n).is_empty(), "running in this bridge");
    assert!(stale(&store, &running, n - 100.0).is_empty(), "just started");
    store.doing[0].tries = MAX_TRIES;
    assert!(stale(&store, &running, n).is_empty(), "tried enough");
}

/// Remark (design §3.1): a plan's « i » says the whole text of each of its offers, not only their question.
#[test]
fn remark_the_plan_s_i_says_its_offers_whole() {
    let n = now();
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(dir.path());
    let plan = plan_of("p1", crate::plans::WAITING, n);
    crate::plans::change(&config, |s| s.plans.push(plan.clone()));
    let pill = pill_of("p1", "search", "Je regarde les hôtels ?", n);
    change(&config, |s| s.offers.push(pill.clone()));
    let item = crate::plans::card_item(&config, &plan);
    let detail = item["bip_detail"].as_str().unwrap();
    assert!(detail.contains(&pill.prompt) && detail.contains(&pill.label), "{detail}");
}
