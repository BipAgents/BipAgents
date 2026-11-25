"""Plans an agent proposes on its own (proactive agents, part 3: bridge-rs/src/plans.rs; passation-pro
briefs/design-agents-proactifs.md §3; contracts/plans/cases.json): a trip, an appointment or a habit is ONE card
(`bip-plan-<id>`) with its moments; nothing runs before « Prévoir »; then each moment is a one-time routine whose words
the bridge delivers at its time WITHOUT the model; an appointment moved or cancelled in the phone's calendar moves or
removes its reminders, again without the model; « Non merci » forgets it for 14 days; the caps (D2 A) keep it from
bothering; « Initiative » off (contracts/proactive) proposes nothing; an app 1.0 sees its generic Yes / No card."""
import datetime
import json
import threading
import time
from pathlib import Path
from zoneinfo import ZoneInfo

from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token, not_done, rpc, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered
from test_agents_admin import managed
from test_mailbox import add_mailbox
from test_triggers import mail_setup

CASES = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "plans" / "cases.json").read_text())
TEXTS = CASES["texts"]
LANGS = ("fr", "en", "de", "es")
TOKEN = "coach-token-" + "p" * 30
DEVICE = "d" * 64
NOW = "2026-11-22T08:00:00.000Z"
PARIS = ZoneInfo("Europe/Paris")


def variants(key, **fields):
    """A text of the contract in each of its languages (the agent's language is the place's)."""
    return [TEXTS[lang][key].format(**fields) for lang in LANGS]


def lang_of(card):
    """The card's language: the words between the agent's name and the title in its summary."""
    for lang in LANGS:
        if TEXTS[lang]["summary"].split("{agent}")[1].split("{title}")[0] in card["bip_summary"]:
            return lang
    raise AssertionError(card["bip_summary"])


def clock(lang, hhmm):
    """« 15:00 » as the bridge says it in `lang`: French « 15 h », « 9 h 15 » (non-breaking spaces; Alex 25/11)."""
    if lang != "fr":
        return hhmm
    h, m = (int(x) for x in hhmm.split(":"))
    return f"{h}\u00a0h" if m == 0 else f"{h}\u00a0h\u00a0{m:02d}"


def place(bridges, fakes, agents=("coach",), change=None):
    bridge = bridges.new()
    # a phone to push to
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (DEVICE, "sandbox", "[]", NOW, NOW))])
    home = managed_install(bridge, fakes, agents=agents)
    for name in agents:
        profile(home, name)
    if change:
        change(bridge.config)
    bridge.start()
    wait_registered(bridge, list(agents))
    give_token(bridge, agents[0], TOKEN)
    # the person lives in Paris (dates/mod.rs): the moments elsewhere say their place's time
    assert bridge.patch("/v1/time", headers=AUTH, json={"auto": False, "time_zone": "Europe/Paris"}).status_code == 200
    return bridge


def local(days, hour, minute=0, zone=PARIS):
    """« YYYY-MM-DDTHH:MM » `days` from today in `zone`."""
    day = datetime.datetime.now(zone).date() + datetime.timedelta(days=days)
    return f"{day.isoformat()}T{hour:02d}:{minute:02d}"


def epoch(text, zone=PARIS):
    return datetime.datetime.fromisoformat(text).replace(tzinfo=zone).timestamp()


def trip(title="Lisbonne", days=3, **extra):
    return {"kind": "trip", "title": title, "zone": "Europe/Lisbon", "city": "Lisbonne",
            "moments": [
                {"at": local(days - 1, 9, 15), "zone": "Europe/Paris", "what": "Enregistrement en ligne",
                 "say": "L’enregistrement en ligne du TP 433 est ouvert", "link": "https://www.flytap.com/checkin"},
                {"at": local(days, 10, 50, ZoneInfo("Europe/Lisbon")), "what": "Arrivée : métro rouge vers le centre",
                 "say": "Bienvenue à Lisbonne : le métro rouge va au centre en 20 min."},
                {"at": local(days, 15, 0, ZoneInfo("Europe/Lisbon")), "what": "Ta chambre est prête",
                 "say": "Ta chambre est prête à partir de 15:00."}], **extra}


def plan_cards(bridge, agent="coach"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [c for c in items if c["run_id"].startswith("bip-plan-")]


def answer(bridge, card, choice, agent="coach", **extra):
    return bridge.post("/v1/approve", headers=AUTH, json={"agent": agent, "run_id": card["run_id"], "choice": choice,
                                                          **extra})


def plan_jobs(bridge, plan, agent="coach"):
    jobs = bridge.get(f"/v1/agents/{agent}/routines", headers=AUTH).json()["jobs"]
    return [j for j in jobs if j.get("plan") == plan]


def done(result):
    """Made: neither an error nor a refusal (a refusal is a plain answer too, `isError` false: bbkit.mcp `not_done`)."""
    return result["isError"] is False and not not_done(result)


def propose(bridge, arguments, token=TOKEN, **kw):
    made = call(bridge, "plan_propose", arguments, token=token, **kw)
    assert done(made), text_of(made)
    return made


def test_a_trip_is_one_card_with_its_moments_and_nothing_runs_before_yes(bridges, fakes):
    bridge = place(bridges, fakes)
    tools = rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]
    [tool] = [t for t in tools if t["name"] == "plan_propose"]
    assert set(tool["inputSchema"]["properties"]) >= {"kind", "title", "zone", "moments", "watch", "event", "repeat"}
    assert "offers" in tool["inputSchema"]["properties"]   # the kinds offer_act lists for Coach (plans/offers.rs)
    made = propose(bridge, trip(watch={"mail_from": [{"name": "TAP", "domain": "flytap.com"}]}))
    # ONE card, nothing scheduled, no model called by the bridge
    assert "Prévoir" in text_of(made) and "nothing runs" in text_of(made)
    assert "no mailbox" in text_of(made)   # Coach reads none: made without its watch, and told
    [card] = plan_cards(bridge)
    plan = card["bip_plan"]
    lang = lang_of(card)
    texts = TEXTS[lang]
    assert card["choices"] == ["once", "deny"] and card["request_id"] == plan["id"]
    assert card["bip_summary"] == texts["summary"].format(agent="Coach", title="Lisbonne", n=3)
    assert (plan["kind"], plan["title"], plan["offers"], plan["rhythm_text"], plan["watch_text"]) == (
        "trip", "Lisbonne", [], None, None)
    first, arrival, room = plan["moments"]
    assert first["when_text"].endswith(clock(lang, "09:15")) and first["when_text"].count("·") == 1, first
    other = texts["other_zone"].format(city="Lisbonne")
    assert arrival["when_text"].endswith(clock(lang, "10:50") + " " + other)
    assert room["when_text"].endswith(clock(lang, "15:00") + " " + other)
    assert first["say"] == "L’enregistrement en ligne du TP 433 est ouvert https://www.flytap.com/checkin"
    assert abs(arrival["at"] - epoch(trip()["moments"][1]["at"], ZoneInfo("Europe/Lisbon"))) < 1
    assert not any(m["urgent"] for m in plan["moments"])
    # everything that would be said, under « i », with what is never done, and where to set it (first card)
    detail = card["bip_detail"]
    for m in plan["moments"]:
        assert m["say"] in detail and m["when_text"] in detail
    assert texts["under_i"] in detail and texts["first_card"] in detail
    # nothing runs before « Prévoir »
    assert bridge.get("/v1/agents/coach/routines", headers=AUTH).json()["jobs"] == []
    time.sleep(0.5)
    assert fakes.hermes.started == {}
    # pushed as a card, critical at every level
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if card["request_id"] in json.dumps(p)], timeout=20)
    kinds = {k["kind"]: k for k in bridge.get("/v1/approval-level", headers=AUTH).json()["kinds"]}
    assert kinds["plan"]["critical"] is True
    # a second plan card's « i » no longer repeats where to set it (unasked: one waiting at a time, so « Non merci »
    # to the first)
    assert answer(bridge, card, "deny").status_code == 200
    propose(bridge, trip(title="Porto", days=5))
    porto = next(c for c in plan_cards(bridge) if c["bip_plan"]["title"] == "Porto")
    assert texts["first_card"] not in porto["bip_detail"] and texts["under_i"] in porto["bip_detail"]


def test_yes_makes_one_time_routines_that_say_their_words_without_the_model(bridges, fakes):
    bridge = place(bridges, fakes)
    propose(bridge, trip())
    [card] = plan_cards(bridge)
    plan = card["bip_plan"]
    skipped = plan["moments"][2]["id"]
    # neither « Prévoir » nor « Non merci »: nothing
    assert answer(bridge, card, "maybe").status_code == 422
    done = answer(bridge, card, "once", skip=[skipped])
    assert done.status_code == 200, done.text
    assert done.json() == {"ok": True, "choice": "once", "moments": 2, "of": 3, "passed": 0}
    assert plan_cards(bridge) == [] and answer(bridge, card, "once").status_code == 404
    jobs = plan_jobs(bridge, plan["id"])
    assert sorted(j["say"] for j in jobs) == sorted(m["say"] for m in plan["moments"][:2])
    for job in jobs:
        assert job["schedule"]["kind"] == "once" and job["state"] == "scheduled" and job["urgent"] is False, job
        assert job["waits_for_answer"] is False
    # « Ce que tes agents font tout seuls », filtered on the plan
    listed = bridge.get("/v1/autonomous", headers=AUTH, params={"plan": plan["id"]}).json()["items"]
    assert sorted(i["ref"] for i in listed) == sorted(j["id"] for j in jobs)
    everything = bridge.get("/v1/autonomous", headers=AUTH).json()["items"]
    assert {i["ref"] for i in listed} <= {i["ref"] for i in everything}
    # its time come (« Lancer maintenant » here): its words as written, one Boîte line, no turn of the model
    checkin = next(j for j in jobs if "TP 433" in j["say"])
    assert bridge.post(f"/v1/routines/{checkin['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: [m for m in boite_items(bridge) if m.get("text") == checkin["say"]], timeout=20)
    time.sleep(0.5)
    assert fakes.hermes.started == {}


def test_an_older_apps_yes_is_the_whole_plan_on_its_generic_card(bridges, fakes):
    """An app 1.0 draws any unknown `bip-` card as its generic card: its sentence, « i », « Oui » / « Non » sending
    once / deny with no `skip`."""
    bridge = place(bridges, fakes)
    propose(bridge, trip())
    [card] = plan_cards(bridge)
    for key in ("agent", "run_id", "request_id", "command", "description", "choices", "created_at", "bip_summary",
                "bip_detail"):
        assert key in card, key
    assert card["description"] == card["bip_summary"] and card["command"] == "plan_propose · Lisbonne"
    assert answer(bridge, card, "once").json() == {"ok": True, "choice": "once", "moments": 3, "of": 3, "passed": 0}
    assert len(plan_jobs(bridge, card["bip_plan"]["id"])) == 3


def test_a_route_moment_is_timed_by_the_streets_and_urgent(bridges, fakes):
    def streets(c):
        c["maps"].update(streets="osrm", streets_url=fakes.maps.url)
    bridge = place(bridges, fakes, change=streets)
    arrive = local(4, 15, 0)
    appointment = {"kind": "appointment", "title": "Dentiste",
                   "moments": [{"at": local(3, 19, 0), "what": "Dentiste demain 15:00", "say": "Dentiste demain à 15:00."},
                               {"route": {"to": "Cabinet du Dr Martin", "from": "12 rue Haute, Lyon",
                                          "arrive_by": arrive, "mode": "walking"}}]}
    propose(bridge, appointment)
    [card] = plan_cards(bridge)
    texts = TEXTS[lang_of(card)]
    route = card["bip_plan"]["moments"][1]
    walking = texts["modes"]["walking"]
    # review 2 (R9): while the card waits, nothing goes to a geocoder or a router, not even the agent's `from` nor
    # the place to go: the moment says « avant 15:00 »
    assert fakes.maps.seen("/search") == [] and fakes.maps.seen("/routed-") == []
    assert route["what"] == texts["route_pending"].format(place="Cabinet du Dr Martin", mode=walking), route
    assert (route["urgent"], route["route"], route["pending"]) == (True, True, True), route
    assert route["when_text"].endswith(texts["before"].format(time=clock(lang_of(card), "15:00"))), route
    # « Prévoir »: found and timed then, 25 min on foot and 5 min to spare: 14:30
    assert answer(bridge, card, "once").status_code == 200
    words = texts["route"].format(place="Cabinet du Dr Martin", n=25, mode=walking)
    [leave] = [j for j in plan_jobs(bridge, card["bip_plan"]["id"]) if j["urgent"]]
    assert leave["say"] == words, leave
    assert datetime.datetime.fromisoformat(leave["next_run_at"]).timestamp() == epoch(arrive) - 30 * 60, leave
    # the router got coordinates only (the start's rounded to 3 decimals, about 100 m), never a place's name
    asked = [s.path for s in fakes.maps.seen("/routed-foot/")]
    assert asked and not any("Martin" in p or "Haute" in p for p in asked), asked
    assert "4.83200,45.75800" in asked[0] and "45.7578," not in asked[0], asked
    # nothing of the way kept on disk (map_route.rs keeps a drawn map's way 30 days: not a plan's)
    routes = bridge.state_dir / "map-routes"
    assert not routes.exists() or not list(routes.iterdir()), list(routes.iterdir())
    # its time come (« Lancer maintenant » here): said at the system's urgent level, no model
    pushed = len(fakes.apns.payloads())
    assert bridge.post(f"/v1/routines/{leave['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: [m for m in boite_items(bridge) if m.get("text") == words], timeout=20)
    assert wait_until(lambda: [p for p in fakes.apns.payloads()[pushed:]
                               if p.get("aps", {}).get("interruption-level") == "time-sensitive"], timeout=20)
    assert fakes.hermes.started == {}
    # no start address (none given, no « Coordonnées » for Coach): refused, nothing made
    nowhere = dict(appointment, title="Kiné")
    nowhere["moments"] = [{"route": {"to": "Cabinet du Dr Martin", "arrive_by": arrive, "mode": "walking"}}]
    refused = call(bridge, "plan_propose", nowhere, token=TOKEN)
    assert not_done(refused) and "route" in text_of(refused), text_of(refused)
    # review 2 (R13): what it invites waits for « Prévoir » too
    assert "Prévoir" in text_of(refused), text_of(refused)
    assert plan_cards(bridge) == []


def test_no_thanks_forgets_it_and_the_same_plan_is_not_proposed_again(bridges, fakes):
    bridge = place(bridges, fakes)
    propose(bridge, trip())
    [card] = plan_cards(bridge)
    assert answer(bridge, card, "deny").json() == {"ok": True, "choice": "deny"}
    assert plan_cards(bridge) == [] and bridge.get("/v1/agents/coach/routines", headers=AUTH).json()["jobs"] == []
    kept = json.loads((bridge.state_dir / "plans.json").read_text())
    assert kept["plans"] == [] and "Lisbonne" not in json.dumps(kept, ensure_ascii=False)   # its print only
    assert (bridge.state_dir / "plans.json").stat().st_mode & 0o777 == 0o600
    again = call(bridge, "plan_propose", trip(title="  lisbonne "), token=TOKEN)
    assert not_done(again) and "said no" in text_of(again) and plan_cards(bridge) == []
    propose(bridge, trip(title="Porto"))
    assert [c["bip_plan"]["title"] for c in plan_cards(bridge)] == ["Porto"]


def test_a_moved_appointment_moves_its_reminders_and_a_cancelled_one_removes_them_without_the_model(bridges, fakes):
    bridge = place(bridges, fakes)
    assert bridge.put("/v1/calendar/agents", json={"agents": ["Coach"]}, headers=AUTH).status_code == 200
    give_token(bridge, "coach", TOKEN)
    start = local(4, 15, 0)
    today = datetime.datetime.now(PARIS).date()
    copy = {"from": (today - datetime.timedelta(days=7)).isoformat(),
            "to": (today + datetime.timedelta(days=60)).isoformat(),
            "calendars": [{"id": "C1", "title": "Perso"}],
            "events": [{"id": "e1", "calendar": "C1", "title": "Dentiste", "all_day": False,
                        "start": datetime.datetime.fromisoformat(start).replace(tzinfo=PARIS).isoformat(),
                        "end": datetime.datetime.fromisoformat(local(4, 15, 30)).replace(tzinfo=PARIS).isoformat()}]}
    assert bridge.put("/v1/calendar", json=copy, headers=AUTH).status_code == 200
    unknown = call(bridge, "plan_propose", {"kind": "appointment", "title": "Dentiste", "event": {"source": "phone",
                   "id": "nope"}, "moments": [{"at": local(3, 19), "what": "Dentiste demain", "say": "Dentiste demain."}]},
                   token=TOKEN)
    assert not_done(unknown) and "event" in text_of(unknown)
    propose(bridge, {"kind": "appointment", "title": "Dentiste", "event": {"source": "phone", "id": "e1"},
                     "moments": [{"at": local(3, 19), "what": "Dentiste demain 15:00", "say": "Dentiste demain à 15:00."},
                                 {"at": local(4, 14, 20), "what": "Pars", "say": "Pars pour le dentiste."}]})
    [card] = plan_cards(bridge)
    texts = TEXTS[lang_of(card)]
    plan = card["bip_plan"]["id"]
    assert answer(bridge, card, "once").status_code == 200
    before = {j["say"]: j["next_run_at"] for j in plan_jobs(bridge, plan)}
    assert len(before) == 2
    # moved by 25 hours: every reminder by as much, one line, no model
    moved = dict(copy, events=[dict(copy["events"][0],
                                    start=datetime.datetime.fromisoformat(local(5, 16, 0)).replace(tzinfo=PARIS).isoformat())])
    assert bridge.put("/v1/calendar", json=moved, headers=AUTH).status_code == 200

    def shifted():
        after = {j["say"]: j["next_run_at"] for j in plan_jobs(bridge, plan)}
        return after and all(datetime.datetime.fromisoformat(after[s]) - datetime.datetime.fromisoformat(before[s])
                             == datetime.timedelta(hours=25) for s in before)
    assert wait_until(shifted, timeout=30)
    assert wait_until(lambda: [m for m in boite_items(bridge) if "Dentiste" in m.get("text", "")
                               and texts["moved"].split("{day}")[1].split("{time}")[1] in m["text"]], timeout=20)
    # cancelled (gone from a copy that covers its day): the plan and its reminders go, one line, no model
    assert bridge.put("/v1/calendar", json=dict(copy, events=[]), headers=AUTH).status_code == 200
    assert wait_until(lambda: plan_jobs(bridge, plan) == [], timeout=30)
    cancelled = texts["cancelled"].format(title="Dentiste")
    assert wait_until(lambda: [m for m in boite_items(bridge) if m.get("text") == cancelled], timeout=20)
    time.sleep(0.5)
    assert fakes.hermes.started == {}


def test_caps_one_plan_per_reply_two_unasked_per_agent_four_per_place_one_waiting(bridges, fakes):
    bridge = place(bridges, fakes, agents=("coach", "vie", "budget"))
    tokens = {"coach": TOKEN, "vie": "vie-token-" + "v" * 30, "budget": "budget-token-" + "b" * 30}
    for agent, token in tokens.items():
        give_token(bridge, agent, token)
    n = iter(range(100))

    def plan(agent, session="api_unasked"):
        # unasked: a call from a conversation no turn of the person's runs in (a routine's, a watcher's…)
        return call(bridge, "plan_propose", trip(title=f"Voyage {next(n)}"), token=tokens[agent], session=session)

    def deny_all(agent):
        for card in plan_cards(bridge, agent):
            assert answer(bridge, card, "deny", agent=agent).status_code == 200

    # asked, in the person's own conversation: one plan per reply, and it counts in no daily cap
    gate = threading.Event()
    fakes.hermes.runs["run_p"] = [sse("tool.started", run_id="run_p", tool="mcp_bipagents_plan_propose", preview="x"),
                                  gate, sse("run.completed", run_id="run_p", output="Bon voyage !")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "coach", "run_id": "run_p", "session_id": "api_p"}).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/p/coach/v1/runs/run_p/events"))
    time.sleep(0.3)
    try:
        assert done(plan("coach", session="api_p"))
        second = plan("coach", session="api_p")
        assert not_done(second) and "one plan per reply" in text_of(second)
    finally:
        gate.set()
    deny_all("coach")
    # unasked (no turn of the person's): one waiting at a time, 2 a day per agent
    assert done(plan("coach"))
    waiting = plan("coach")
    assert not_done(waiting) and "already waits" in text_of(waiting)
    deny_all("coach")
    assert done(plan("coach"))
    deny_all("coach")
    third = plan("coach")
    assert not_done(third) and "already made 2" in text_of(third)
    # 4 a day for the whole place
    for _ in range(2):
        assert done(plan("vie"))
        deny_all("vie")
    fifth = plan("budget")
    assert not_done(fifth) and "4 cards" in text_of(fifth)
    assert plan_cards(bridge, "budget") == []


def test_off_proposes_nothing_and_an_agent_others_talk_to_never_does(bridges, fakes):
    """« Initiative » (contracts/proactive, set on the agent's page: PUT /v1/agents/{id}/proactive): off proposes
    nothing; an agent other people talk to (a project agent) has no « Initiative » at all."""
    bridge, _ = managed(bridges, fakes, isolated=True)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Budget", "kind": "finance"}).status_code == 201
    own = "budget-token-" + "o" * 30
    give_token(bridge, "budget", own)

    def listed(token=own):
        return "plan_propose" in [t["name"] for t in rpc(bridge, "tools/list", token=token).json()["result"]["tools"]]

    def set_level(level):
        assert bridge.put("/v1/agents/budget/proactive", headers=AUTH, json={"level": level}).status_code == 200
        # its tools' list changed: the bridge registered them again, with a token of its own since this test's isn't
        # the one in its Hermes profile (agent_tools.rs `register_at`): this test's back
        give_token(bridge, "budget", own)

    assert listed()
    set_level("off")
    assert not listed()
    refused = call(bridge, "plan_propose", trip(), token=own)
    assert not_done(refused) and "turned your suggestions off" in text_of(refused)
    assert plan_cards(bridge, "budget") == []
    set_level("smart")
    assert listed() and done(call(bridge, "plan_propose", trip(), token=own))
    # turned off again: the card still waiting goes without a word (§2.1)
    assert len(plan_cards(bridge, "budget")) == 1
    set_level("off")
    assert wait_until(lambda: plan_cards(bridge, "budget") == [], timeout=20)
    # an agent other people talk to (a project agent): never listed, never made
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Voyage à Lisbonne", "lang": "fr",
                                                                  "ends": time.time() + 86400 * 10, "category": "creative"})
    assert made.status_code == 201, made.text
    project = made.json()["agent"]
    other = "project-token-" + "x" * 30
    give_token(bridge, project, other)
    assert not listed(other)
    assert not_done(call(bridge, "plan_propose", trip(), token=other)) and plan_cards(bridge, project) == []


def test_a_habit_is_one_card_with_its_rhythm_and_stops_after_its_weeks(bridges, fakes):
    bridge = place(bridges, fakes)
    propose(bridge, {"kind": "habit", "title": "Boire plus d’eau",
                     "moments": [{"what": "Un verre d’eau", "say": "Un verre d’eau ?"}],
                     "repeat": {"days": "weekdays", "times": ["10:00", "15:00"], "weeks": 2}})
    [card] = plan_cards(bridge)
    texts = TEXTS[lang_of(card)]
    plan = card["bip_plan"]
    assert plan["kind"] == "habit" and plan["rhythm_text"] == texts["rhythm_example"]
    assert card["bip_summary"] == texts["summary"].format(agent="Coach", title="Boire plus d’eau", n=20)
    assert answer(bridge, card, "once").status_code == 200
    jobs = plan_jobs(bridge, plan["id"])
    assert len(jobs) == 2 and all(j["schedule"]["kind"] == "cron" and j["repeat"] == 10 for j in jobs), jobs
    assert all(j["say"] == "Un verre d’eau ?" for j in jobs)
    # « vers 10 h »: within 10 minutes of it, the same shift for both
    hours = sorted(j["schedule"]["expr"].split()[1] for j in jobs)
    minutes = {j["schedule"]["expr"].split()[0] for j in jobs}
    assert len(minutes) == 1 and hours in (["10", "15"], ["14", "9"]), jobs


def test_a_watched_mailbox_starts_with_the_plan(bridges, fakes):
    bridge, _ = mail_setup(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    from test_mailbox import TOKEN as COACH
    made = call(bridge, "plan_propose", trip(watch={"mail_from": [{"name": "TAP", "domain": "flytap.com"}]}),
                token=COACH)
    assert done(made), text_of(made)
    [card] = plan_cards(bridge)
    texts = TEXTS[lang_of(card)]
    assert card["bip_plan"]["watch_text"] == texts["watch"].format(name="TAP")
    assert card["bip_plan"]["watch_text"] in card["bip_detail"]
    bad = call(bridge, "plan_propose", trip(title="Rome", watch={"mail_from": [{"name": "X", "domain": "http://x"}]}),
               token=COACH)
    assert not_done(bad) and "watch" in text_of(bad)
    # nothing watched before « Prévoir »
    assert bridge.get("/v1/triggers", headers=AUTH).json()["items"] == []
    assert answer(bridge, card, "once").json() == {"ok": True, "choice": "once", "moments": 3, "of": 3, "passed": 0,
                                                    "watch": True}
    [trigger] = bridge.get("/v1/triggers", headers=AUTH).json()["items"]
    assert trigger["state"] == "active" and "flytap.com" in json.dumps(trigger), trigger
    # a few k tokens for a wake-up (§3.8): its budget the smallest
    assert trigger["budget_tokens"] == 20_000, trigger
    # « Ce que tes agents font tout seuls », filtered on the plan: its moments and its watch (review 1, R6)
    listed = bridge.get("/v1/autonomous", headers=AUTH, params={"plan": card["bip_plan"]["id"]}).json()["items"]
    assert sorted(i["kind"] for i in listed) == ["routine", "routine", "routine", "trigger"], listed
    assert trigger["id"] in {i["ref"] for i in listed}
    assert wait_until(lambda: fakes.imap.idling >= 1, timeout=20)
    # headers only (review 1, R2; design §3.4, §3.7): the sender's address, never its display name; the turn reads
    # From, Subject, Date, never the text, which the bridge does not even fetch
    from test_triggers import mail, turns
    fetched = len(fakes.imap.commands)
    # its turn held open, to try the mailbox's tools from it (review 2, R10)
    fakes.hermes.rounds = 10_000
    run_id = f"r{len(fakes.hermes.started) + 1}"
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("tool.started", run_id=run_id, tool="mcp_bipagents_mail_search", preview="x"),
                                 gate, sse("run.completed", run_id=run_id, output="[SILENT]")]
    fakes.imap.add("INBOX", 201, mail(201, "flytap.com <promo@ailleurs.io>", "Votre vol TP 433", "Promo", "<a1@x>"))
    fakes.imap.add("INBOX", 202, mail(202, "TAP Air Portugal <info@flytap.com>", "Changement d’horaire : TP 433",
                                      "SECRET-BODY porte 12", "<a2@flytap.com>"))
    try:
        assert wait_until(lambda: run_id in fakes.hermes.started, timeout=30)
        time.sleep(0.5)
        session = fakes.hermes.started[run_id]["session_id"]
        for tool, arguments in (("mail_search", {"query": "TP 433"}), ("mail_read", {"id": "x|INBOX|202"})):
            refused = call(bridge, tool, arguments, token=COACH, session=session)
            assert not_done(refused) and "headers" in text_of(refused), (tool, text_of(refused))
    finally:
        gate.set()
        fakes.hermes.rounds = 1
    assert wait_until(lambda: turns(fakes), timeout=30)
    time.sleep(1)
    [turn] = turns(fakes)
    note = turn["input"]
    assert "From: info@flytap.com" in note and "Subject: Changement d’horaire : TP 433" in note and "Date: " in note
    assert "SECRET-BODY" not in note and "mail_read" not in note and "ailleurs" not in note and "Air Portugal" not in note
    whole = [c for c in fakes.imap.commands[fetched:] if "FETCH" in c and "BODY.PEEK[]" in c]
    assert whole == [], whole
    # the watch can't be made at « Prévoir » (20 routines of that kind already): said in one line, and in the answer
    porto = trip(title="Porto", days=5, watch={"mail_from": [{"name": "TAP", "domain": "flytap.com"}]})
    assert done(call(bridge, "plan_propose", porto, token=COACH))
    [card] = plan_cards(bridge)
    account = bridge.get("/v1/mail/accounts", headers=AUTH).json()["accounts"][0]["id"]
    for i in range(19):
        made = bridge.post("/v1/agents/coach/triggers", headers=AUTH, json={
            "name": f"R{i}", "prompt": "Dis-moi.", "kind": "mail", "mail": {"account": account, "from": f"s{i}.example"}})
        assert made.status_code == 200, made.text
    assert answer(bridge, card, "once").json()["watch"] is False
    failed = texts["watch_failed"].format(name="TAP")
    assert wait_until(lambda: [m for m in boite_items(bridge) if m.get("text") == failed], timeout=20)


def test_a_plan_in_a_turn_another_agent_asked_for_is_unasked_and_lands_in_the_main_conversation(bridges, fakes):
    """Review 1 (B1, and the caps after « Initiative »'s B1): Quotidien asks Coach (« message_agent », their pair's
    hidden conversation) and Coach proposes a plan in that turn. Its person asked nothing: unasked (one card waiting at a
    time, two a day); its card shows in Coach's main conversation, and its moments are said there, never in the pair's
    hidden thread (nor would « a bougé / annulé » be)."""
    from concurrent.futures import ThreadPoolExecutor
    from test_agent_messages import TOKENS as PAIR_TOKENS, ask
    from test_agent_pairs import linked

    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    linked(bridge, fakes, [{"from": "quotidien", "to": "coach"}])
    assert bridge.patch("/v1/time", headers=AUTH, json={"auto": False, "time_zone": "Europe/Paris"}).status_code == 200
    fakes.hermes.add_session("botchat-coach", [], title="Bot Chat", agent="coach", last_active=1.0)
    fakes.hermes.rounds = 10_000   # Coach's run goes on until the test lets it end
    run_id = f"r{len(fakes.hermes.started) + 1}"
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("tool.started", run_id=run_id, tool="mcp_bipagents_plan_propose", preview="x"), gate,
                                 sse("run.completed", run_id=run_id, output="C'est prévu.")]
    coach = PAIR_TOKENS["coach"]
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(ask, bridge, "quotidien", target="bip/coach", message="Prévois son voyage à Lisbonne.")
        assert wait_until(lambda: run_id in fakes.hermes.started and fakes.hermes.seen(f"/p/coach/v1/runs/{run_id}/events"))
        time.sleep(0.5)
        pair = fakes.hermes.started[run_id]["session_id"]
        try:
            propose(bridge, trip(), token=coach, session=pair)
            # unasked: one card waiting at a time (asked, it would say « one plan per reply »)
            second = call(bridge, "plan_propose", trip(title="Porto"), token=coach, session=pair)
            assert not_done(second) and "already waits" in text_of(second), text_of(second)
            # review 2 (R12): any card made in the pair's turn (here a routine Coach proposes) is shown to the person
            # in its main conversation, never in the pair's hidden one (card_place.rs `report_of`)
            routine = call(bridge, "routine_schedule", {"action": "create", "name": "Point du soir",
                                                        "schedule": "every day at 21:00", "prompt": "Le point."},
                           token=coach, session=pair)
            assert routine["isError"] is False and not not_done(routine), text_of(routine)
        finally:
            gate.set()
            fakes.hermes.rounds = 1
        asked.result(timeout=60)
    kept = json.loads((bridge.state_dir / "plans.json").read_text())
    assert [m["asked"] for m in kept["made"]] == [False]
    [card] = plan_cards(bridge)
    places = json.loads((bridge.state_dir / "card-places.json").read_text())
    assert places[card["run_id"]]["report"] is None, places[card["run_id"]]   # the main conversation shows it
    [routine_card] = [r for r in places if r.startswith("bip-rout-")]
    assert places[routine_card]["report"] is None, places[routine_card]
    assert answer(bridge, card, "once").status_code == 200
    jobs = plan_jobs(bridge, card["bip_plan"]["id"])
    assert jobs and all(j["origin_session_id"] is None for j in jobs), jobs
    # its time come: said in Coach's main conversation, never the pair's
    job = jobs[0]
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: [m for m in boite_items(bridge, "coach") if m.get("text") == job["say"]], timeout=20)
    [said] = [m for m in boite_items(bridge, "coach") if m.get("text") == job["say"]]
    assert said["session_id"] != pair, said


def home_place(bridges, fakes):
    """Coach in its own container (« Coordonnées » need the vault's isolation), the streets routed by the fake OSRM."""
    from test_vault_contacts import ANSWER
    bridge = bridges.new()
    bridge.config["agents"]["Coach"] = {
        "hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
        "python": f"{bridge.fakebin}/podman exec -i hermes-coach python", "hermes_home": "/home/hermes/.hermes"}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-coach"], stdout=ANSWER)
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    bridge.config["maps"].update(streets="osrm", streets_url=fakes.maps.url)
    bridge.start()
    give_token(bridge, "coach", TOKEN)
    assert bridge.patch("/v1/time", headers=AUTH, json={"auto": False, "time_zone": "Europe/Paris"}).status_code == 200
    return bridge


def test_a_route_from_home_is_timed_only_after_yes_from_the_entry_labelled_home(bridges, fakes):
    """Review 1 (R1, privacy): the person's address (« Coordonnées ») goes to the geocoder only after « Prévoir », never
    while the card waits nor after « Non merci »; it is the entry labelled as home, never just the first one (here a
    pro address); without such an entry the route needs its `from`. The place to go is found at once (the agent hears
    of a place not found); the way is timed with the home's coordinates then."""
    from test_vault_contacts import PRO
    bridge = home_place(bridges, fakes)
    arrive = local(4, 15, 0)

    def dentist(title):
        return {"kind": "appointment", "title": title,
                "moments": [{"route": {"to": "Cabinet du Dr Martin", "arrive_by": arrive, "mode": "walking"}}]}

    def searched():
        return [s.params.get("q", "") for s in fakes.maps.seen("/search")]

    assert bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["coach"]}).status_code == 201
    pro_only = call(bridge, "plan_propose", dentist("Ostéo"), token=TOKEN)
    assert not_done(pro_only) and "route" in text_of(pro_only) and "home" in text_of(pro_only), text_of(pro_only)
    # review 2 (R13): « Perso » says home too
    home = {"kind": "address", "label": "Perso", "full_name": "Camille Exemple", "address_line1": "3 rue du Test",
            "city": "Lyon", "postal_code": "69001", "country": "fr", "agents": ["coach"]}
    assert bridge.post("/v1/vault", headers=AUTH, json=home).status_code == 201
    propose(bridge, dentist("Dentiste"))
    [card] = plan_cards(bridge)
    texts = TEXTS[lang_of(card)]
    [route] = card["bip_plan"]["moments"]
    # while the card waits: nothing to a geocoder nor a router, not even the place to go (review 2, R9)
    assert searched() == [] and not fakes.maps.seen("/routed-")
    walking = texts["modes"]["walking"]
    assert (route["route"], route["urgent"], route["pending"]) == (True, True, True), route
    assert route["what"] == texts["route_pending"].format(place="Cabinet du Dr Martin", mode=walking), route
    assert route["when_text"].endswith(texts["before"].format(time=clock(lang_of(card), "15:00"))), route
    # « Non merci »: still nothing
    assert answer(bridge, card, "deny").status_code == 200
    assert searched() == [] and not fakes.maps.seen("/routed-")
    # « Prévoir »: the entry labelled as home, timed by the streets then (25 min on foot, 5 to spare)
    propose(bridge, dentist("Kiné"))
    [card] = plan_cards(bridge)
    done = answer(bridge, card, "once")
    assert done.status_code == 200 and done.json()["moments"] == 1, done.text
    asked = [q for q in searched() if "Test" in q or "Essai" in q]
    assert len(asked) == 1 and "3 rue du Test" in asked[0] and "Lyon" in asked[0], asked
    assert "Cabinet du Dr Martin" in searched(), searched()
    [job] = plan_jobs(bridge, card["bip_plan"]["id"])
    assert job["say"] == texts["route"].format(place="Cabinet du Dr Martin", n=25, mode=walking) and job["urgent"], job
    assert datetime.datetime.fromisoformat(job["next_run_at"]).timestamp() == epoch(arrive) - 30 * 60, job
    # the router got coordinates only, the home's rounded to 3 decimals (about 100 m)
    routed = [s.path for s in fakes.maps.seen("/routed-foot/")]
    assert routed and not any("Test" in p for p in routed) and "4.83200,45.75800" in routed[0], routed


def test_a_repeating_event_is_not_followed_and_a_calendar_taken_out_cancels_nothing(bridges, fakes):
    """Review 1 (B2, R5): every occurrence of a repeating event has the same id on both phones, so a plan never follows
    one (made without it, the agent told why: no « a bougé » of a week later); and the person taking a calendar out of
    the phone's copy is no cancellation: the plan of an event of that calendar stays, no line."""
    bridge = place(bridges, fakes)
    assert bridge.put("/v1/calendar/agents", json={"agents": ["Coach"]}, headers=AUTH).status_code == 200
    give_token(bridge, "coach", TOKEN)
    today = datetime.datetime.now(PARIS).date()

    def at(days, hour):
        return datetime.datetime.fromisoformat(local(days, hour)).replace(tzinfo=PARIS).isoformat()

    weekly = [{"id": "w1", "calendar": "C1", "title": "Kiné", "all_day": False, "start": at(d, 9), "end": at(d, 10),
               "recurring": True} for d in (-3, 4)]
    dentist = {"id": "e2", "calendar": "C2", "title": "Dentiste", "all_day": False, "start": at(4, 15), "end": at(4, 16)}
    copy = {"from": (today - datetime.timedelta(days=7)).isoformat(),
            "to": (today + datetime.timedelta(days=60)).isoformat(),
            "calendars": [{"id": "C1", "title": "Perso"}, {"id": "C2", "title": "Santé"}], "events": [*weekly, dentist]}
    assert bridge.put("/v1/calendar", json=copy, headers=AUTH).status_code == 200
    made = propose(bridge, {"kind": "appointment", "title": "Kiné", "event": {"source": "phone", "id": "w1"},
                            "moments": [{"at": local(3, 19), "what": "Kiné demain 9:00", "say": "Kiné demain à 9:00."}]})
    assert "repeats" in text_of(made), text_of(made)
    [card] = plan_cards(bridge)
    kept = json.loads((bridge.state_dir / "plans.json").read_text())
    assert [p.get("event") for p in kept["plans"]] == [None], kept["plans"]
    assert answer(bridge, card, "once").status_code == 200
    propose(bridge, {"kind": "appointment", "title": "Dentiste", "event": {"source": "phone", "id": "e2"},
                     "moments": [{"at": local(3, 19), "what": "Dentiste demain", "say": "Dentiste demain à 15:00."}]})
    [card] = plan_cards(bridge)
    plan = card["bip_plan"]["id"]
    assert answer(bridge, card, "once").status_code == 200
    # « Santé » taken out of the copy: its events with it, but nothing was cancelled
    taken_out = dict(copy, calendars=[{"id": "C1", "title": "Perso"}], events=weekly)
    assert bridge.put("/v1/calendar", json=taken_out, headers=AUTH).status_code == 200
    time.sleep(8)   # the plans' look every 5 s
    assert len(plan_jobs(bridge, plan)) == 1
    cancelled = [TEXTS[lang]["cancelled"].format(title="Dentiste") for lang in LANGS]
    assert not [m for m in boite_items(bridge) if m.get("text") in cancelled]
