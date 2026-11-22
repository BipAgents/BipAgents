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


def propose(bridge, arguments, token=TOKEN, **kw):
    made = call(bridge, "plan_propose", arguments, token=token, **kw)
    assert made["isError"] is False, text_of(made)
    return made


def test_a_trip_is_one_card_with_its_moments_and_nothing_runs_before_yes(bridges, fakes):
    bridge = place(bridges, fakes)
    tools = rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]
    [tool] = [t for t in tools if t["name"] == "plan_propose"]
    assert set(tool["inputSchema"]["properties"]) >= {"kind", "title", "zone", "moments", "watch", "event", "repeat"}
    assert "offers" not in tool["inputSchema"]["properties"]   # offer_act's branch
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
    assert first["when_text"].endswith("09:15") and first["when_text"].count("·") == 1, first
    other = texts["other_zone"].format(city="Lisbonne")
    assert arrival["when_text"].endswith("10:50 " + other) and room["when_text"].endswith("15:00 " + other)
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
    words = texts["route"].format(place="Cabinet du Dr Martin", n=25, mode=texts["modes"]["walking"])
    assert (route["what"], route["say"], route["urgent"], route["route"]) == (words, words, True, True), route
    # 25 min on foot and 5 min to spare: 14:30
    assert route["at"] == epoch(arrive) - 30 * 60, (route["at"], epoch(arrive))
    # the router got coordinates only, never the place's name nor the person's address
    asked = [s.path for s in fakes.maps.seen("/routed-foot/")]
    assert asked and not any("Martin" in p or "Haute" in p for p in asked)
    # no start address (none given, no « Coordonnées » for Coach): refused, nothing made
    nowhere = dict(appointment, title="Kiné")
    nowhere["moments"] = [{"route": {"to": "Cabinet du Dr Martin", "arrive_by": arrive, "mode": "walking"}}]
    refused = call(bridge, "plan_propose", nowhere, token=TOKEN)
    assert not_done(refused) and "route" in text_of(refused)
    assert len(plan_cards(bridge)) == 1


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
        assert plan("coach", session="api_p")["isError"] is False
        second = plan("coach", session="api_p")
        assert not_done(second) and "one plan per reply" in text_of(second)
    finally:
        gate.set()
    deny_all("coach")
    # unasked (no turn of the person's): one waiting at a time, 2 a day per agent
    assert plan("coach")["isError"] is False
    waiting = plan("coach")
    assert not_done(waiting) and "already waits" in text_of(waiting)
    deny_all("coach")
    assert plan("coach")["isError"] is False
    deny_all("coach")
    third = plan("coach")
    assert not_done(third) and "already made 2" in text_of(third)
    # 4 a day for the whole place
    for _ in range(2):
        assert plan("vie")["isError"] is False
        deny_all("vie")
    fifth = plan("budget")
    assert not_done(fifth) and "4 cards" in text_of(fifth)
    assert plan_cards(bridge, "budget") == []


def test_off_proposes_nothing_and_an_agent_others_talk_to_never_does(bridges, fakes):
    """As contracts/proactive: « off » is kept in <data>/agent_proactive.json (proactive-level's file), an agent other
    people talk to (a project agent) has no « Initiative » at all."""
    bridge, _ = managed(bridges, fakes, isolated=True)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Budget", "kind": "finance"}).status_code == 201
    own = "budget-token-" + "o" * 30
    give_token(bridge, "budget", own)

    def listed(token=own):
        return "plan_propose" in [t["name"] for t in rpc(bridge, "tools/list", token=token).json()["result"]["tools"]]

    assert listed()
    (bridge.state_dir / "agent_proactive.json").write_text(json.dumps({"levels": {"budget": "off"}}))
    assert not listed()
    refused = call(bridge, "plan_propose", trip(), token=own)
    assert not_done(refused) and "turned your suggestions off" in text_of(refused)
    assert plan_cards(bridge, "budget") == []
    (bridge.state_dir / "agent_proactive.json").write_text(json.dumps({"levels": {"budget": "smart"}}))
    assert listed() and call(bridge, "plan_propose", trip(), token=own)["isError"] is False
    # turned off again: the card still waiting goes without a word (§2.1)
    assert len(plan_cards(bridge, "budget")) == 1
    (bridge.state_dir / "agent_proactive.json").write_text(json.dumps({"levels": {"budget": "off"}}))
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
    assert made["isError"] is False, text_of(made)
    [card] = plan_cards(bridge)
    texts = TEXTS[lang_of(card)]
    assert card["bip_plan"]["watch_text"] == texts["watch"].format(name="TAP")
    assert card["bip_plan"]["watch_text"] in card["bip_detail"]
    bad = call(bridge, "plan_propose", trip(title="Rome", watch={"mail_from": [{"name": "X", "domain": "http://x"}]}),
               token=COACH)
    assert not_done(bad) and "watch" in text_of(bad)
    # nothing watched before « Prévoir »
    assert bridge.get("/v1/triggers", headers=AUTH).json()["items"] == []
    assert answer(bridge, card, "once").status_code == 200
    [trigger] = bridge.get("/v1/triggers", headers=AUTH).json()["items"]
    assert trigger["state"] == "active" and "flytap.com" in json.dumps(trigger), trigger
    assert wait_until(lambda: fakes.imap.idling >= 1, timeout=20)
