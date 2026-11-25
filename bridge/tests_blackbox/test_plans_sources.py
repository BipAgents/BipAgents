"""The plans' sources the bridge reads itself (proactive agents, branch 7: bridge-rs/src/plans/sources.rs; passation-pro
briefs/design-agents-proactifs.md §3.2 n° 4, §3.4 « L'agenda »; contracts/plans-sources/cases.json): an appointment of
the calendars an agent reads, in 1 to 7 days with a place and no plan, and a trip of the Carnet within 7 days with no
plan. First ONE hidden line at the person's next message to that agent (no turn of its own); without any exchange by
the day before (an appointment) or two days before (a trip), ONE offer card placed by the bridge, no model. An
appointment's « Oui » sets its plan up without the model (a reminder the day before, when to leave), a trip's starts
one turn that may propose its plan. Nothing for an agent that doesn't propose."""
import datetime
import json
import time
from pathlib import Path
from zoneinfo import ZoneInfo

from bbkit.common import AUTH
from bbkit.fakes import taken_notes, waiting_notes
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_plans import PARIS, TOKEN, answer, done, local, place, plan_jobs, propose

CASES = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "plans-sources" / "cases.json").read_text())
TEXTS = CASES["texts"]
PLAN_TEXTS = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "plans" / "cases.json").read_text())["texts"]
LANGS = ("fr", "en", "de", "es")
#: The sources looked at every tick (never only « vers 8 h » / « vers 18 h »), e-mails due at once.
SOURCES = {"morning": "any", "evening": "any", "due_hours": 0, "tick_seconds": 0.3, "follow_seconds": 1}
CALENDAR_UNTRUSTED = "[The events between these markers come from calendars:"


def with_sources(c):
    c["plans_sources"] = dict(SOURCES)


def at(days, hour, minute=0):
    return datetime.datetime.fromisoformat(local(days, hour, minute)).replace(tzinfo=PARIS).isoformat()


def copy_of(*events):
    today = datetime.datetime.now(PARIS).date()
    return {"from": (today - datetime.timedelta(days=7)).isoformat(),
            "to": (today + datetime.timedelta(days=60)).isoformat(),
            "calendars": [{"id": "C1", "title": "Perso"}], "events": list(events)}


def event(eid, title, days, hour=15, location="Cabinet du Dr Martin, 12 rue Exemple, Lyon", **extra):
    return {"id": eid, "calendar": "C1", "title": title, "all_day": False, "start": at(days, hour),
            "end": at(days, hour, 45), "location": location, **extra}


def share_calendar(bridge, *events, agent="Coach", token=TOKEN):
    assert bridge.put("/v1/calendar/agents", json={"agents": [agent]}, headers=AUTH).status_code == 200
    give_token(bridge, agent.lower(), token)
    assert bridge.put("/v1/calendar", json=copy_of(*events), headers=AUTH).status_code == 200


def offer_cards(bridge, agent="coach", source=None):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [c for c in items if c["run_id"].startswith("bip-offer-")
            and (source is None or c["bip_offer"].get("source") == source)]


def lang_of(card):
    """The card's language: the words of its question (the bridge's own, from the contract)."""
    for lang in LANGS:
        for key in ("calendar_offer", "trip_offer"):
            tail = TEXTS[lang][key].split(" · ")[1]
            if card["bip_offer"]["label"].endswith(tail):
                return lang
    raise AssertionError(card["bip_offer"]["label"])


def sources_home_place(bridges, fakes):
    """test_plans.home_place with the sources looked at every tick: Coach in its own container, « Perso » its home
    (« Coordonnées »), the streets by the fake OSRM (25 min on foot)."""
    from test_vault_contacts import ANSWER
    bridge = bridges.new()
    bridge.config["agents"]["Coach"] = {
        "hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
        "python": f"{bridge.fakebin}/podman exec -i hermes-coach python", "hermes_home": "/home/hermes/.hermes"}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-coach"], stdout=ANSWER)
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    # the install's model the person's own (as conftest.managed_install): a model the bridge cannot tell is outside,
    # and an agent on a model outside reads no calendar to propose (plans/sources.rs `outside`)
    hub = bridge.home / ".hermes"
    hub.mkdir(parents=True, exist_ok=True)
    (hub / "config.yaml").write_text("model:\n  provider: lmstudio\n")
    bridge.config["maps"].update(streets="osrm", streets_url=fakes.maps.url)
    with_sources(bridge.config)
    bridge.start()
    give_token(bridge, "coach", TOKEN)
    assert bridge.patch("/v1/time", headers=AUTH, json={"auto": False, "time_zone": "Europe/Paris"}).status_code == 200
    home = {"kind": "address", "label": "Perso", "full_name": "Camille Exemple", "address_line1": "3 rue du Test",
            "city": "Lyon", "postal_code": "69001", "country": "fr", "agents": ["coach"]}
    assert bridge.post("/v1/vault", headers=AUTH, json=home).status_code == 201
    return bridge


def plans_kept(bridge):
    path = bridge.state_dir / "plans.json"
    return json.loads(path.read_text())["plans"] if path.exists() else []


def sources_kept(bridge):
    path = bridge.state_dir / "plans-sources.json"
    return json.loads(path.read_text()) if path.exists() else {}


def test_an_appointment_to_come_is_told_once_at_the_next_exchange_and_runs_nothing(bridges, fakes):
    bridge = place(bridges, fakes, change=with_sources)
    share_calendar(bridge,
                   event("e1", "Dentiste", 4),
                   event("e2", "Kiné", 3, recurring=True),                  # repeats: never followed
                   dict(event("e3", "Anniversaire de Léa", 2), all_day=True,
                        start=local(2, 0)[:10], end=local(2, 0)[:10]),     # all day
                   event("e4", "Appel banque", 2, location=""),             # no place
                   event("e5", "Salon", 9),                                 # beyond 7 days
                   event("e6", "Réunion", 0, hour=23, location="Bureau"))   # today: not 1 to 7 days ahead
    # looked at (a tick or two): no turn, no card, no push of its own
    assert wait_until(lambda: sources_kept(bridge).get("hints"), timeout=20), sources_kept(bridge)
    time.sleep(1)
    assert fakes.hermes.started == {} and offer_cards(bridge) == []
    # the next exchange: ONE hidden line, with the event between the calendars' markers and how to follow it
    note = waiting_notes(taken_notes(bridge, agent="coach", session="s1", text="Salut"))
    assert note is not None and "<!--bip:plans\n" in note, note
    plans_note = note[note.index("<!--bip:plans\n"):]
    assert CALENDAR_UNTRUSTED in plans_note and "Dentiste" in plans_note and "Cabinet du Dr Martin" in plans_note
    assert '"source": "phone"' in plans_note and '"id": "e1"' in plans_note, plans_note
    assert "plan_propose" in plans_note and "after answering" in plans_note.lower(), plans_note
    for other in ("Kiné", "Anniversaire", "Appel banque", "Salon", "Réunion"):
        assert other not in plans_note, other
    # told once: the next message has no such line
    again = waiting_notes(taken_notes(bridge, agent="coach", session="s1", text="Et demain ?"))
    assert again is None or "<!--bip:plans" not in again, again
    # only the two messages' own turns ran
    assert len(fakes.hermes.started) == 2
    assert (bridge.state_dir / "plans-sources.json").stat().st_mode & 0o777 == 0o600


def test_an_appointment_with_its_plan_is_never_told(bridges, fakes):
    bridge = place(bridges, fakes, change=with_sources)
    share_calendar(bridge, event("e1", "Dentiste", 4))
    propose(bridge, {"kind": "appointment", "title": "Dentiste", "event": {"source": "phone", "id": "e1"},
                     "moments": [{"at": local(3, 19), "what": "Dentiste demain", "say": "Dentiste demain à 15:00."}]})
    time.sleep(2)
    assert not sources_kept(bridge).get("hints"), sources_kept(bridge)
    note = waiting_notes(taken_notes(bridge, agent="coach", session="s1"))
    assert note is None or "<!--bip:plans" not in note, note


def state_of(bridge, fakes):
    """What a failure of the day-before test needs to be understood (flaky twice on 24/11 at 16:49, never since): the
    test's clock and the bridge's (its answer's Date), the plans kept and their routines, the cards, the sources."""
    try:
        bridge_date = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "coach"}).headers.get("date")
    except Exception as err:   # noqa: BLE001 — a diagnosis, never a failure of its own
        bridge_date = f"unknown ({err})"
    try:
        jobs = {p.get("id"): plan_jobs(bridge, p.get("id")) for p in plans_kept(bridge)}
    except Exception as err:   # noqa: BLE001
        jobs = f"unknown ({err})"
    try:
        cards = [(c.get("run_id"), c.get("bip_offer")) for c in
                 bridge.get("/v1/approvals", headers=AUTH, params={"agent": "coach"}).json()["items"]]
    except Exception as err:   # noqa: BLE001
        cards = f"unknown ({err})"
    return json.dumps({"test_now": datetime.datetime.now(PARIS).isoformat(), "test_time": time.time(),
                       "bridge_date": bridge_date, "plans_kept": plans_kept(bridge), "plan_jobs": jobs, "cards": cards,
                       "sources_kept": sources_kept(bridge), "maps_seen": [p for p in ("/search", "/routed-")
                                                                            if fakes.maps.seen(p)],
                       "hermes_started": fakes.hermes.started}, default=str, ensure_ascii=False, indent=1)


def test_without_an_exchange_the_day_before_one_card_and_yes_sets_the_plan_without_the_model(bridges, fakes):
    bridge = sources_home_place(bridges, fakes)
    share_calendar(bridge, event("e1", "Dentiste", 1))
    why = lambda: state_of(bridge, fakes)  # noqa: E731
    # the day before (tomorrow's event) and no exchange: ONE card placed by the bridge
    assert wait_until(lambda: offer_cards(bridge, source="calendar"), timeout=20), why()
    [card] = offer_cards(bridge)
    offer = card["bip_offer"]
    texts = TEXTS[lang_of(card)]
    assert (offer["kind"], offer["source"], offer["about"]) == ("appointment", "calendar", "Dentiste"), why()
    assert card["choices"] == ["once", "deny"] and card["bip_summary"].endswith(offer["label"]), (card, why())
    assert offer["label"].startswith("Dentiste ") and texts["calendar_offer"].split(" · ")[1] in offer["label"], why()
    assert "15:00" in offer["label"], why()
    detail = card["bip_detail"]
    assert texts["calendar_detail_route"].format(place="Cabinet du Dr Martin, 12 rue Exemple, Lyon") in detail, (detail, why())
    assert texts["calendar_detail_follow"] in detail, (detail, why())
    # critical, never « Toujours »: the person answers it each time
    assert "bip_always" not in card, why()
    assert answer(bridge, card, "always").status_code == 422, why()
    # nothing found or timed before « Oui », nothing run
    assert not fakes.maps.seen("/search") and not fakes.maps.seen("/routed-"), why()
    assert fakes.hermes.started == {}, why()
    yes = answer(bridge, card, "once")
    assert yes.json() == {"ok": True, "choice": "once", "kind": "appointment", "by": "bridge"}, (yes.text, why())

    def plan():
        return next((p for p in plans_kept(bridge) if p.get("event", {}).get("id") == "e1"), None)
    assert wait_until(lambda: plan() and plan()["state"] == "set" and plan()["routines"], timeout=30), why()
    made = plan()
    assert made["kind"] == "appointment" and made["title"] == "Dentiste" and made["event"]["source"] == "phone", why()
    jobs = plan_jobs(bridge, made["id"])
    walking = PLAN_TEXTS[lang_of(card)]["modes"]["walking"]
    route = next((j for j in jobs if j["urgent"]), None)
    assert route is not None, why()
    assert route["say"] == PLAN_TEXTS[lang_of(card)]["route"].format(
        place="Cabinet du Dr Martin, 12 rue Exemple, Lyon", n=25, mode=walking), why()
    start = datetime.datetime.fromisoformat(at(1, 15)).timestamp()
    assert datetime.datetime.fromisoformat(route["next_run_at"]).timestamp() == start - 30 * 60, why()
    eve = datetime.datetime.fromisoformat(at(0, 19)).timestamp()
    reminders = [j for j in jobs if not j["urgent"]]
    if eve > time.time() + 60:
        assert len(reminders) == 1, why()
        assert reminders[0]["say"] == texts["calendar_reminder"].format(title="Dentiste", time="15:00"), why()
    else:
        assert reminders == [], why()   # the day before at 19:00 had passed: only when to leave
    # the home and the place went to the geocoder only now; no model at all
    assert fakes.maps.seen("/search"), why()
    assert fakes.hermes.started == {}, why()
    # never twice for that event
    time.sleep(1.5)
    assert offer_cards(bridge) == [], why()


def test_a_trip_in_the_carnet_is_offered_two_days_before_and_yes_is_one_turn_that_may_propose_its_plan(bridges, fakes):
    bridge = place(bridges, fakes, change=with_sources)
    folder = bridge.state_dir / "notebook" / "carnet" / "Voyages"
    folder.mkdir(parents=True, exist_ok=True)
    start, end = (datetime.datetime.now(PARIS).date() + datetime.timedelta(days=d) for d in (2, 5))
    (folder / "Lisbonne.md").write_text(f"---\ntype: trip\nid: t1\nstart: {start}\nend: {end}\ndestination: Lisbonne\n"
                                        "---\n# Lisbonne\n\n## Transports\n- Vol TP 433 Orly → Lisbonne à 9 h 15\n")
    assert wait_until(lambda: offer_cards(bridge, source="trip"), timeout=20)
    [card] = offer_cards(bridge)
    offer = card["bip_offer"]
    texts = TEXTS[lang_of(card)]
    assert (offer["kind"], offer["about"]) == ("trip", "Lisbonne"), offer
    assert offer["label"].startswith("Lisbonne, ") and offer["label"].endswith(texts["trip_offer"].split(" · ")[1])
    assert card["bip_detail"].startswith(texts["trip_prompt"]), card["bip_detail"]
    assert fakes.hermes.started == {}
    yes = answer(bridge, card, "once")
    assert yes.json() == {"ok": True, "choice": "once", "kind": "trip", "by": "agent"}, yes.text

    def turns():
        return [r for r in fakes.hermes.started.values() if "Your person said yes to your offer" in (r["input"] or "")]
    assert wait_until(turns, timeout=20)
    [turn] = turns()
    assert texts["trip_prompt"] in turn["input"] and "plan_propose" in turn["input"], turn["input"]
    assert "Lisbonne" in turn["input"]
    # the plan it proposes now is the person's ask (a yes to « Je prépare ton voyage ? »): made, counted as asked
    trip_plan = {"kind": "trip", "title": "Lisbonne", "zone": "Europe/Lisbon", "city": "Lisbonne",
                 "moments": [{"at": local(1, 9, 15), "what": "Enregistrement en ligne",
                              "say": "L’enregistrement en ligne du TP 433 est ouvert."}]}
    made = call(bridge, "plan_propose", trip_plan, token=TOKEN, session=turn["session_id"])
    assert done(made), text_of(made)
    [kept] = [p for p in plans_kept(bridge) if p["title"] == "Lisbonne"]
    assert kept["asked"] is True and kept["state"] == "waiting"
    time.sleep(1)
    assert len(turns()) == 1


def test_a_trip_further_off_is_told_at_the_next_exchange_only(bridges, fakes):
    bridge = place(bridges, fakes, change=with_sources)
    folder = bridge.state_dir / "notebook" / "carnet" / "Voyages"
    folder.mkdir(parents=True, exist_ok=True)
    start, end = (datetime.datetime.now(PARIS).date() + datetime.timedelta(days=d) for d in (5, 8))
    (folder / "Porto.md").write_text(f"---\ntype: trip\nid: t2\nstart: {start}\nend: {end}\ndestination: Porto\n---\n"
                                     "# Porto\n")
    assert wait_until(lambda: sources_kept(bridge).get("hints"), timeout=20), sources_kept(bridge)
    time.sleep(1)
    assert offer_cards(bridge) == [] and fakes.hermes.started == {}
    note = waiting_notes(taken_notes(bridge, agent="coach", session="s1"))
    assert note is not None and "<!--bip:plans" in note and "Porto" in note and start.isoformat() in note, note


def test_nothing_for_an_agent_turned_off(bridges, fakes):
    bridge = place(bridges, fakes, change=with_sources)
    assert bridge.put("/v1/agents/coach/proactive", headers=AUTH, json={"level": "off"}).status_code == 200
    share_calendar(bridge, event("e1", "Dentiste", 1), event("e2", "Ostéo", 4))
    folder = bridge.state_dir / "notebook" / "carnet" / "Voyages"
    folder.mkdir(parents=True, exist_ok=True)
    start = datetime.datetime.now(PARIS).date() + datetime.timedelta(days=2)
    (folder / "Rome.md").write_text(f"---\ntype: trip\nid: t3\nstart: {start}\nend: {start}\ndestination: Rome\n---\n"
                                    "# Rome\n")
    time.sleep(2.5)
    assert offer_cards(bridge) == [] and not sources_kept(bridge).get("hints")
    note = waiting_notes(taken_notes(bridge, agent="coach", session="s1"))
    assert note is None or "<!--bip:plans" not in note, note
    assert len(fakes.hermes.started) == 1
