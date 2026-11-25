"""The frame of what the agents do on their own (contracts/task-frame/cases.json; design-agents-proactifs.md §4,
Alex 22/11 Q6 A, Q7 A, Q8 A): every routine, watcher and trigger has an owner, an end and a place where it answers;
a « Oui » to a one-time routine is one time, a repeat asks its own « Oui » that says its rhythm and its end; the
agent's routines run « vers » their time, the person's at theirs; what the agent sets up on its own ends after 90
days (a card 3 days before: « Garder ? »); what fails is repaired in its frame or stopped with a word
(the card « Réparer / Arrêter », `bip-fix-`, no model call before « Réparer »); an end reached goes quietly, with
one line for what ran more than 7 days. An app 1.0 sees today's answers (the added keys left out)."""
import json
import os
import re
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import test_watchers as tw
from test_watchers import persons_turn
from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_approval_level import auto_lines, level, with_device
from test_connectors import TOKEN as VIE

DAY = 86400.0
#: The base's answers to an app 1.0 (recorded on int-1.1c 938d65f23 with TASK_FRAME_RECORD=1).
RECORDED = Path(__file__).parent / "fixtures" / "task_frame_app_1_0.json"
#: What this branch adds to them (contracts/task-frame `app_1_0.ignores`).
ADDED = {"owner", "ends", "deliver_to", "approx", "bip_fix", "bip_choices", "until", "once", "default_end"}
#: Added to what carries a frame (a job, a row, a trigger: with its `owner`); a watcher's detail had `failures` already.
FRAMED = {"deliver", "failures", "fix"}


def routine_runs(fakes):
    return [rid for rid, r in fakes.hermes.started.items() if "<!--bip:routine" in (r.get("input") or "")]


def repair_runs(fakes):
    return [rid for rid, r in fakes.hermes.started.items() if "<!--bip:repair" in (r.get("input") or "")]


def jobs(bridge, agent="wellness"):
    return {j["name"]: j for j in bridge.get(f"/v1/agents/{agent}/routines", headers=AUTH).json()["jobs"]}


def cards(bridge, prefix, agent="wellness"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [c for c in items if c["run_id"].startswith(prefix)]


def autonomous(bridge, agent):
    return bridge.get("/v1/autonomous", headers=AUTH, params={"agent": agent}).json()["items"]


def approve(bridge, card, choice, agent="wellness"):
    return bridge.post("/v1/approve", headers=AUTH, json={"agent": agent, "run_id": card["run_id"], "choice": choice})


def routine(bridge, rid):
    book = json.loads((bridge.state_dir / "routines.json").read_text())
    return next(r for r in book["routines"] if r["id"] == rid)


def edit_routine(bridge, rid, **changes):
    """routines.json changed as the bridge's clock would find it (the bridge idle: nothing of it runs now)."""
    path = bridge.state_dir / "routines.json"
    book = json.loads(path.read_text())
    for r in book["routines"]:
        if r["id"] == rid:
            for key, value in changes.items():
                if key == "frame":
                    r.setdefault("frame", {}).update(value)
                else:
                    r[key] = value
    path.write_text(json.dumps(book))


def day_en(at):
    """« Nov 30 » (texts.en, the routines' clock: UTC here), « Jan 2, 2027 » when not this year (review C9)."""
    d = datetime.fromtimestamp(at, timezone.utc)
    year = "" if d.year == datetime.now(timezone.utc).year else f", {d.year}"
    return f"{d.strftime('%b')} {d.day}{year}"


def made_by_agent(bridge, name, schedule, token=None, agent="wellness", **extra):
    kwargs = {"token": token} if token else {}
    made = call(bridge, "routine_schedule", {"action": "create", "name": name, "schedule": schedule,
                                             "prompt": f"{name} : fais-le.", **extra}, **kwargs)
    assert not made["isError"], text_of(made)
    [card] = [c for c in cards(bridge, "bip-rout-", agent) if c["command"] == f"routine_schedule · {name}"]
    return made, card


def started(bridge):
    bridge.start()
    give_token(bridge)
    return bridge


def test_every_routine_watcher_and_trigger_has_an_owner_an_end_and_a_place(bridges, fakes):
    """§4.1 rule 1 and 2: who does it and for whom, until when, where it answers — on each row of « Ce que tes agents
    font tout seuls » and on each one's own view."""
    bridge, value, conn, card = tw.setup(bridges, fakes)
    # the watcher's card already said how it ends: nothing given, asked (no default end): the line of today
    assert card["bip_connector_schedule"] == "Vérifie toutes les 15 min, ne réveille l'agent que si ça change"
    assert card["bip_connector_watch"]["once"] is False and card["bip_connector_watch"]["until"] is None
    assert card["bip_connector_watch"]["ends"] == {"kind": "never", "text": "until you stop it"}
    mine = bridge.post("/v1/agents/vie/routines", headers=AUTH,
                       json={"name": "Point du soir", "schedule": "every day at 19:00", "prompt": "Fais le point."})
    assert mine.status_code == 200, mine.text
    until = (datetime.now(timezone.utc) + timedelta(days=40)).date()
    _, weather = made_by_agent(bridge, "Météo", "every day at 7:30", token=VIE, agent="vie", until=until.isoformat())
    last = datetime(until.year, until.month, until.day, 23, 59, 59, tzinfo=timezone.utc).timestamp()
    assert weather["bip_summary"] == \
        f"Vie created the routine “Météo”: every day around 7:30 AM, until {day_en(last)}. Start it?", weather
    assert approve(bridge, weather, "once", agent="vie").status_code == 200
    _, reminders = made_by_agent(bridge, "Rappels", "every monday 18h", token=VIE, agent="vie", repeat=10)
    assert reminders["bip_summary"] == "Vie created the routine “Rappels”: every Monday around 6 PM, 10 times. Start it?"
    trigger = bridge.post("/v1/agents/vie/triggers", headers=AUTH,
                          json={"name": "Raccourci", "prompt": "Note ce que je dicte", "kind": "link"})
    assert trigger.status_code in (200, 201), trigger.text

    items = autonomous(bridge, "vie")
    for item in items:
        if item["kind"] in ("routine", "watcher", "trigger"):
            assert item["owner"] == {"agent": "vie", "person": "you"}, item
            assert set(item["ends"]) >= {"kind", "text"} and item["deliver"]["kind"] in ("conversation", "boite",
                                                                                          "silent"), item
            assert item["failures"] == 0 and item["fix"] is None, item
    rows = {i["title"]: i for i in items}
    point = rows["Point du soir"]
    assert point["ends"] == {"kind": "never", "text": "until you stop it"} and point["deliver"]["kind"] == "boite"
    assert point["approx"] is False and point["when"]["text"] == "Chaque jour à 19:00"
    meteo = rows["Météo"]
    assert meteo["ends"] == {"kind": "date", "at": last, "text": f"until {day_en(last)}"}, meteo["ends"]
    assert meteo["approx"] is True and meteo["when"]["approx"] is True
    assert meteo["when"]["text"] == "Chaque jour vers 7 h 30", meteo["when"]
    assert rows["Rappels"]["ends"] == {"kind": "count", "count": 10, "text": "10 times"}, rows["Rappels"]["ends"]
    watcher = rows["Créneau médecin"]
    assert watcher["ends"] == {"kind": "never", "text": "until you stop it"} and watcher["deliver"]["kind"] == "boite"
    assert rows["Raccourci"]["ends"]["kind"] == "never" and rows["Raccourci"]["deliver"]["kind"] == "boite"
    # the job view says the same (a routine's page)
    job = jobs(bridge, "vie")["Météo"]
    assert (job["owner"], job["ends"], job["approx"], job["failures"], job["plan"], job["fix"]) == \
        ({"agent": "vie", "person": "you"}, meteo["ends"], True, 0, None, None), job
    # (its `deliver` stays Hermes' « local »: the frame's place is `deliver_to`)
    assert job["deliver"] == "local" and job["deliver_to"]["kind"] in ("conversation", "boite"), job
    # « vers 7 h 30 »: within ten minutes of it, the schedule itself exact (an app 1.0 shows « 7:30 »)
    assert job["schedule"]["expr"] == "30 7 * * *"
    at = datetime.fromisoformat(job["next_run_at"])
    shift = (at.hour * 60 + at.minute) - (7 * 60 + 30)
    assert -10 <= shift <= 10 and at.second == 0, job["next_run_at"]
    assert jobs(bridge, "vie")["Point du soir"]["next_run_at"].endswith("19:00:00+00:00")
    detail = bridge.get(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH).json()["watch"]
    assert detail["ends"]["kind"] == "never" and detail["once"] is False
    listed = bridge.get("/v1/triggers", headers=AUTH, params={"agent": "vie"}).json()["items"]
    assert listed[0]["owner"] == {"agent": "vie", "person": "you"} and listed[0]["ends"]["kind"] == "never"


def test_a_once_yes_never_becomes_a_repeat_and_a_repeat_asks_its_own_yes(bridges, fakes):
    """§4.1 rules 3 and 4: « Oui » (even « Toujours ») to a one-time routine is one run; its agent making it repeat is a
    proposal of its own, critical, whose sentence says the rhythm and the end (an app 1.0 reads it too)."""
    bridge = started(with_device(bridges))
    level(bridge, "critical")
    tomorrow = (datetime.now(timezone.utc) + timedelta(days=1)).strftime("%Y-%m-%dT09:00")
    _, card = made_by_agent(bridge, "Dentiste", tomorrow)
    assert approve(bridge, card, "always").status_code == 200
    job = jobs(bridge)["Dentiste"]
    assert (job["schedule"]["kind"], job["repeat"], job["state"]) == ("once", 1, "scheduled"), job
    assert job["ends"] == {"kind": "once", "text": "once, then it stops"} and job["approx"] is False
    until = (datetime.now(timezone.utc) + timedelta(days=20)).date()
    last = datetime(until.year, until.month, until.day, 23, 59, 59, tzinfo=timezone.utc).timestamp()
    asked = call(bridge, "routine_schedule", {"action": "update", "name": "Dentiste", "schedule": "every day at 8:00",
                                              "until": until.isoformat()})
    assert not asked["isError"], text_of(asked)
    [change] = cards(bridge, "bip-rout-")
    assert change["bip_summary"] == \
        f"Wellness suggests repeating the routine “Dentiste”: every day around 8 AM, until {day_en(last)}. OK?", change
    assert "→ every day around 8 AM, until " in change["bip_detail"], change["bip_detail"]
    # not applied, not approved for it: « Toujours » on its start covered nothing after it
    job = jobs(bridge)["Dentiste"]
    assert job["schedule"]["kind"] == "once" and auto_lines(bridge) == []
    kinds = {k["kind"]: k for k in bridge.get("/v1/approval-level", headers=AUTH).json()["kinds"]}
    assert kinds["routine_change"]["critical"] is True and kinds["task_fix"]["critical"] is True
    assert approve(bridge, change, "once").status_code == 200
    job = jobs(bridge)["Dentiste"]
    assert (job["schedule"]["kind"], job["schedule"]["expr"], job["approx"]) == ("cron", "0 8 * * *", True), job
    assert job["ends"]["kind"] == "date" and job["ends"]["at"] == last


def test_a_failing_routine_is_retried_in_silence_then_told_then_paused_with_repair_or_stop(bridges, fakes):
    """§4.1 rule 6: the first failure is tried again 15 minutes later in silence; failed again, today's notice; three
    runs failed in a row: paused, and the card « Réparer / Arrêter ». « Réparer » is ONE turn of its agent, which
    changes something only through the existing cards: the routine stays paused until the person's « Oui »."""
    bridge = started(with_device(bridges))
    hermes = fakes.hermes
    hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    for n in (1, 2, 3):
        hermes.run_info[f"r{n}"] = {"status": "failed", "error": "a tool crashed"}
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": "Courriels", "schedule": "every 2h", "prompt": "Regarde mes e-mails."}).json()["job"]
    rid = made["id"]
    told = lambda: [i for i in boite_items(bridge) if i["title"] == "Courriels"]  # noqa: E731

    def ran(n):
        return wait_until(lambda: len(routine_runs(fakes)) == n and routine(bridge, rid).get("last_status")
                          and jobs(bridge)["Courriels"]["state"] != "running", timeout=60)

    # 1. its scheduled run fails, after a while: tried again 15 minutes after its end (review C3 a), nothing told
    edit_routine(bridge, rid, next_run=time.time() - 5)
    assert ran(1)
    time.sleep(1.0)
    assert told() == [] and jobs(bridge)["Courriels"]["failures"] == 1
    kept = routine(bridge, rid)
    # (its start is `last_run`: the retry counts from its end, after it)
    assert kept["frame"]["retry_at"] > kept["last_run"] + 900, kept
    assert abs(kept["frame"]["retry_at"] - (time.time() + 900)) < 120, kept
    next_run = kept["next_run"]
    # 2. its retry fails too: today's notice, the run of the day not moved
    edit_routine(bridge, rid, frame={"retry_at": time.time() - 5})
    assert ran(2)
    assert wait_until(lambda: told(), timeout=20)
    [notice] = told()
    assert notice["text"].startswith("The routine “Courriels” couldn't finish: something stopped it."), notice
    kept = routine(bridge, rid)
    assert kept["next_run"] == next_run and kept["frame"].get("retry_at") is None and kept["frame"]["failures"] == 2
    # 3. the next run fails: paused, and the card (no notice more)
    edit_routine(bridge, rid, next_run=time.time() - 5)
    assert ran(3)
    assert wait_until(lambda: cards(bridge, "bip-fix-"), timeout=20)
    [card] = cards(bridge, "bip-fix-")
    job = jobs(bridge)["Courriels"]
    assert (job["state"], job["enabled"], job["failures"], job["fix"]) == ("paused", False, 3, card["run_id"]), job
    assert card["choices"] == ["once", "deny"] and card["bip_choices"] == ["repair", "stop"]
    assert card["bip_summary"] == "Wellness's routine “Courriels” failed 3 times in a row. Have it repaired? (No: it stops.)"
    assert card["bip_detail"] == ("Repair: Wellness fixes it once; you'll approve the change. Stop: it no longer runs "
                                  "and stays on its page.")
    fix = card["bip_fix"]
    assert (fix["kind"], fix["ref"], fix["name"], fix["variant"], fix["failures"], fix["reason"]) == \
        ("routine", rid, "Courriels", "repair", 3, "failed"), fix
    assert fix["title"] == "Wellness · the routine “Courriels” failed 3 times in a row"
    assert re.fullmatch(r"Since \w{3} \d{1,2}: something stopped it\.", fix["text"]), fix["text"]
    assert fix["buttons"] == ["Repair", "Stop"]
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if card["run_id"] in json.dumps(p)], timeout=20)
    assert len(told()) == 1 and len(routine_runs(fakes)) == 3 and repair_runs(fakes) == []

    # « Réparer »: one turn, with what failed; nothing changed but through its card
    hermes.run_output = "Je te propose de la lancer toutes les 3 heures."
    assert approve(bridge, card, "once").status_code == 200
    assert wait_until(lambda: len(repair_runs(fakes)) == 1, timeout=30)
    turn = hermes.started[repair_runs(fakes)[0]]["input"]
    assert "« Courriels »" in turn and "routine_schedule" in turn and "failed 3 times in a row" in turn, turn
    assert wait_until(lambda: [i for i in told() if i["text"] == "Je te propose de la lancer toutes les 3 heures."],
                      timeout=20)
    assert cards(bridge, "bip-fix-") == [] and jobs(bridge)["Courriels"]["enabled"] is False
    assert approve(bridge, card, "once").status_code == 404
    time.sleep(1.0)
    assert len(repair_runs(fakes)) == 1 and len(routine_runs(fakes)) == 3
    # its agent's fix is a proposal; the person's « Oui » applies it and turns the routine back on
    proposed = call(bridge, "routine_schedule", {"action": "update", "name": "Courriels", "schedule": "every 3h"})
    assert not proposed["isError"], text_of(proposed)
    [change] = cards(bridge, "bip-rout-")
    job = jobs(bridge)["Courriels"]
    assert (job["enabled"], job["schedule"]["minutes"]) == (False, 120), job
    assert approve(bridge, change, "once").status_code == 200
    job = jobs(bridge)["Courriels"]
    assert (job["enabled"], job["state"], job["schedule"]["minutes"], job["failures"]) == (True, "scheduled", 180, 0), job


def test_the_persons_run_now_is_told_at_once_never_retried_in_silence(bridges, fakes):
    """« Lancer maintenant »: the person waits for it — a failure is told at once, as before; it still counts."""
    bridge = started(with_device(bridges))
    hermes = fakes.hermes
    hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    hermes.run_info["r1"] = {"status": "failed", "error": "model unreachable"}
    job = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                      json={"name": "Archive", "schedule": "every 2h", "prompt": "Range."}).json()["job"]
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: jobs(bridge)["Archive"]["last_status"] == "ended failed, told", timeout=30)
    assert routine(bridge, job["id"])["frame"].get("retry_at") is None and jobs(bridge)["Archive"]["failures"] == 1


def test_stop_pauses_never_deletes_and_no_answer_in_7_days_pauses_with_one_line(bridges, fakes):
    """« Arrêter »: paused, on its page. A card nobody answers for 7 days: gone, the routine paused, one line (not
    pushed)."""
    bridge = started(with_device(bridges))
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    now = time.time()
    made = {}
    for name in ("Ancienne", "Récente"):
        made[name] = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                                 json={"name": name, "schedule": "every 2h", "prompt": "Fais-le."}).json()["job"]["id"]
    for name, at in (("Ancienne", now - 8 * DAY), ("Récente", now)):
        edit_routine(bridge, made[name], enabled=False, next_run=None, frame={
            "failures": 3, "failing_since": at - DAY,
            "fix": {"id": f"f{name[0].lower()}", "at": at, "variant": "repair", "since": at - DAY, "failures": 3,
                    "reason": "looping"}})
    assert wait_until(lambda: len(cards(bridge, "bip-fix-")) == 1, timeout=60)
    [card] = cards(bridge, "bip-fix-")
    assert card["bip_fix"]["name"] == "Récente"
    line = "“Ancienne” is stopped: it hadn't worked for 7 days. It stays on its page."
    assert wait_until(lambda: [i for i in boite_items(bridge) if i["text"] == line], timeout=20)
    [item] = [i for i in boite_items(bridge) if i["text"] == line]
    time.sleep(1.0)
    assert not [p for p in fakes.apns.payloads() if p.get("m") == f"o_{item['id']}"]
    assert jobs(bridge)["Ancienne"]["state"] == "paused" and jobs(bridge)["Ancienne"]["fix"] is None
    # « Arrêter »: paused, not deleted, no turn
    assert approve(bridge, card, "deny").status_code == 200
    job = jobs(bridge)["Récente"]
    assert (job["state"], job["fix"]) == ("paused", None) and len(jobs(bridge)) == 2
    assert cards(bridge, "bip-fix-") == [] and repair_runs(fakes) == []
    assert approve(bridge, card, "maybe").status_code in (404, 422)


def test_what_the_agent_sets_up_on_its_own_ends_after_90_days_and_asks_3_days_before(bridges, fakes):
    """Alex Q6 A: a routine its agent makes on its own (here in a routine's run: nobody asked) with no end given stops
    by itself 90 days after; 3 days before, « Garder ? »."""
    bridge = started(with_device(bridges))
    hermes = fakes.hermes
    hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    # asked in the person's own conversation, no end given: until the person stops it
    with persons_turn(bridge, fakes):
        _, asked = made_by_agent(bridge, "Agenda", "every day at 8:00")
    assert asked["bip_summary"] == "Wellness created the routine “Agenda”: every day around 8 AM. Start it?"
    # outside any turn the bridge follows: its own idea too (review C8, as plans.rs)
    _, outside = made_by_agent(bridge, "Hors tour", "every day at 9:00")
    end = jobs(bridge)["Hors tour"]["ends"]
    assert end["kind"] == "date" and abs(end["at"] - (time.time() + 90 * DAY)) < 2 * DAY, end
    host = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": "Veille", "schedule": "every 2h", "prompt": "Regarde."}).json()["job"]
    gate = threading.Event()
    hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_routine_schedule", preview=""), gate]
    assert bridge.post(f"/v1/routines/{host['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: hermes.seen("/v1/runs/r1/events"), timeout=20)
    time.sleep(0.5)
    before = time.time()
    _, card = made_by_agent(bridge, "Météo", "every day at 7:30")
    gate.set()
    job = jobs(bridge)["Météo"]
    assert job["ends"]["kind"] == "date" and abs(job["ends"]["at"] - (before + 90 * DAY)) < 2 * DAY, job["ends"]
    assert card["bip_summary"] == \
        f"Wellness created the routine “Météo”: every day around 7:30 AM, until {day_en(job['ends']['at'])}. Start it?"
    assert approve(bridge, card, "once").status_code == 200
    # 3 days before its end: the card, once, not pushed
    soon = time.time() + 2 * DAY
    edit_routine(bridge, job["id"], frame={"until": soon})
    assert wait_until(lambda: cards(bridge, "bip-fix-"), timeout=60)
    [keep] = cards(bridge, "bip-fix-")
    assert keep["bip_summary"] == ("The routine “Météo” has been running for 3 months. Keep it? With no answer, it goes "
                                   "on."), keep
    assert keep["bip_choices"] == ["keep", "stop"] and keep["bip_fix"]["variant"] == "keep"
    assert keep["bip_fix"]["buttons"] == ["Keep", f"Stop it on {day_en(soon)}"], keep["bip_fix"]
    time.sleep(1.0)
    assert not [p for p in fakes.apns.payloads() if keep["run_id"] in json.dumps(p)]
    # « Garder »: 90 days more
    assert approve(bridge, keep, "once").status_code == 200
    assert jobs(bridge)["Météo"]["ends"]["at"] == soon + 90 * DAY and cards(bridge, "bip-fix-") == []
    assert jobs(bridge)["Agenda"]["ends"] == {"kind": "never", "text": "until you stop it"}


def test_an_end_reached_goes_quietly_and_says_it_once_after_7_days(bridges, fakes):
    bridge = started(with_device(bridges))
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    now = time.time()
    ids = {}
    for name, created in (("Ancienne", now - 8 * DAY), ("Récente", now - DAY)):
        ids[name] = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                                json={"name": name, "schedule": "every 2h", "prompt": "Fais-le."}).json()["job"]["id"]
        edit_routine(bridge, ids[name], created=created, last_run=now - 3600, frame={"until": now - 60})
    assert wait_until(lambda: jobs(bridge) == {}, timeout=60)
    line = f"The routine “Ancienne” stopped as planned (until {day_en(now - 60)})."
    assert wait_until(lambda: [i for i in boite_items(bridge) if i["text"] == line], timeout=20)
    time.sleep(1.0)
    shown = [i["text"] for i in boite_items(bridge)]
    assert shown == [line], shown
    [item] = boite_items(bridge)
    assert not [p for p in fakes.apns.payloads() if p.get("m") == f"o_{item['id']}"]


def test_a_broken_watcher_asks_repair_or_stop_without_a_model_call(bridges, fakes):
    """§4.1 rule 6, §5: « en panne », the card instead of the bridge's line, no model call; « Arrêter »: switched
    off, never deleted."""
    bridge, value, conn, _ = tw.setup(bridges, fakes)
    assert tw.check(bridge, conn)["step"] == "baseline"
    value.write_text("fail")
    outcomes = [tw.check(bridge, conn) for _ in range(3)]
    assert [o["step"] for o in outcomes] == ["failed", "failed", "broken"] and outcomes[-1]["sent"] is True
    assert list(fakes.hermes.started) == [] and not fakes.model.requests
    assert wait_until(lambda: cards(bridge, "bip-fix-", "vie"), timeout=20)
    [card] = cards(bridge, "bip-fix-", "vie")
    assert card["bip_summary"] == "Vie's watcher “Créneau médecin” stopped working. Have it repaired? (No: it stops.)"
    fix = card["bip_fix"]
    assert (fix["kind"], fix["ref"], fix["variant"], fix["failures"], fix["reason"]) == \
        ("watcher", conn["id"], "repair", 3, "check"), fix
    assert fix["title"] == "Vie · the watcher “Créneau médecin” stopped working"
    assert re.fullmatch(r"Since \w{3} \d{1,2}: its check doesn't work any more\.", fix["text"]), fix["text"]
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if card["run_id"] in json.dumps(p)], timeout=20)
    # the card says it: no line of the bridge besides
    time.sleep(1.0)
    assert tw.boite(bridge) == []
    assert tw.check(bridge, conn)["step"] == "failed" and len(cards(bridge, "bip-fix-", "vie")) == 1
    row = tw.row(bridge, conn)
    assert row["fix"] == card["run_id"] and row["failures"] == 4 and row["detail"]["broken"] is True
    # « Arrêter »
    assert approve(bridge, card, "deny", agent="vie").status_code == 200
    listed = bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"]
    assert [(c["id"], c["enabled"]) for c in listed] == [(conn["id"], False)]
    assert tw.row(bridge, conn)["state"] == "paused" and cards(bridge, "bip-fix-", "vie") == []
    assert list(fakes.hermes.started) == []


def test_repairing_a_watcher_is_one_turn_in_its_conversation(bridges, fakes):
    bridge, value, conn, _ = tw.setup(bridges, fakes)
    assert tw.check(bridge, conn)["step"] == "baseline"
    value.write_text("fail")
    assert [tw.check(bridge, conn)["step"] for _ in range(3)] == ["failed", "failed", "broken"]
    assert wait_until(lambda: cards(bridge, "bip-fix-", "vie"), timeout=20)
    [card] = cards(bridge, "bip-fix-", "vie")
    fakes.hermes.run_output = "Le site du Dr Martin a changé : je te propose une nouvelle version du veilleur."
    assert approve(bridge, card, "once", agent="vie").status_code == 200
    assert wait_until(lambda: tw.turns(fakes), timeout=30)
    [turn] = tw.turns(fakes)
    assert "asked you to repair" in turn["input"] and "connector_update" in turn["input"], turn["input"]
    created = [body for agent, body in fakes.hermes.created if agent == "vie"]
    assert created and created[-1]["title"] == "Watcher: Créneau médecin"
    assert wait_until(lambda: tw.boite(bridge), timeout=20)
    [said] = tw.boite(bridge)
    assert said["text"].startswith("Le site du Dr Martin a changé") and said["session_id"]
    assert cards(bridge, "bip-fix-", "vie") == [] and len(fakes.hermes.started) == 1
    listed = bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"]
    assert listed[0]["enabled"] is True


def test_a_once_watcher_stops_once_it_told_its_person(bridges, fakes):
    bridge, value, conn, card = tw.setup(bridges, fakes, watch={"every": "15m", "notify": "on_change", "once": True})
    assert card["bip_connector_watch"]["once"] is True
    assert card["bip_connector_watch"]["ends"] == {"kind": "once", "text": "once, then it stops"}
    assert card["bip_connector_schedule"] == \
        "Vérifie toutes les 15 min, ne réveille l'agent que si ça change, une seule fois, puis il s'arrête"
    assert tw.check(bridge, conn)["step"] == "baseline"
    listed = lambda: bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"]  # noqa: E731
    # not yet what the person waits for: its agent keeps it to itself, the watcher goes on (review C2); its note says
    # it is « once »
    fakes.hermes.run_output = "NOTHING"
    value.write_text("mardi 18:00")
    assert tw.check(bridge, conn)["step"] == "woke"
    assert "only once" in tw.turns(fakes)[-1]["input"], tw.turns(fakes)[-1]["input"]
    time.sleep(1.0)
    assert [c["enabled"] for c in listed()] == [True]
    # its agent's turn fails: the bridge's own line is no « once » told, it goes on
    fakes.hermes.run_info[f"r{len(fakes.hermes.started) + 1}"] = {"status": "failed", "error": "model unreachable"}
    value.write_text("mardi 19:00")
    assert tw.check(bridge, conn)["step"] == "woke"
    time.sleep(1.0)
    assert [c["enabled"] for c in listed()] == [True]
    # what it waited for: told, then it stops (paused on its page, never deleted)
    fakes.hermes.run_output = "Un créneau s'est libéré mercredi à 9 h."
    value.write_text("mercredi 09:00")
    woke = tw.check(bridge, conn)
    assert woke["step"] == "woke" and woke["sent"] is True, woke
    assert wait_until(lambda: [c["enabled"] for c in listed()] == [False], timeout=20)
    assert [i["state"] for i in autonomous(bridge, "vie") if i["kind"] == "watcher"] == ["paused"]
    assert wait_until(lambda: [i for i in tw.boite(bridge) if i["text"].startswith("Un créneau")], timeout=20)


# -- an app 1.0 ------------------------------------------------------------------------------------------------------

VOLATILE = {"id", "ref", "run_id", "request_id", "created_at", "created", "updated", "updated_at", "approved_at",
            "next_run_at", "last_run_at", "next_run", "last_run", "last_check", "next_check", "last_change",
            "last_wake", "at", "token", "path", "url", "session_id", "origin_session_id", "last_session_id",
            "card_run_id", "bip_connector_id", "image", "job"}
HEX = re.compile(r"[0-9a-f]{12,}")
ISO = re.compile(r"\d{4}-\d\d-\d\dT[\d:.]+(Z|[+-]\d\d:\d\d)?")


def normal(value, key=None):
    """An answer with what changes from one run to the next (ids, times, tokens) blanked."""
    if key in VOLATILE and value is not None:
        return "<v>"
    if isinstance(value, dict):
        return {k: normal(v, k) for k, v in value.items()}
    if isinstance(value, list):
        return [normal(v) for v in value]
    if isinstance(value, str):
        return ISO.sub("<iso>", HEX.sub("<hex>", value))
    if isinstance(value, (int, float)) and not isinstance(value, bool) and value > 1_600_000_000:
        return "<t>"
    return value


def without_added(value):
    if isinstance(value, dict):
        # (a job's own `deliver`, « local » in Hermes' shape, stays: the frame's is an object)
        framed = "owner" in value
        out = {k: without_added(v) for k, v in value.items()
               if k not in ADDED and not (framed and k in FRAMED and (k != "deliver" or isinstance(v, dict)))}
        if "group" in value:
            # a row of « Ce que tes agents font tout seuls » gains its plan (a job had it already)
            out.pop("plan", None)
        if isinstance(out.get("when"), dict):
            out["when"].pop("approx", None)
        return out
    if isinstance(value, list):
        return [without_added(v) for v in value]
    return value


def test_an_app_1_0_sees_today_s_answers(bridges, fakes):
    """« Tout s'ajoute, rien ne casse »: what an app 1.0 reads of the routines, « Ce que tes agents font tout seuls »,
    the cards, the connectors and the triggers is what the base gives (int-1.1c), the added keys left out. Nothing here
    has an end, a « vers » or a failure: those change, on purpose, only the sentences that say them."""
    bridge, value, conn, _ = tw.setup(bridges, fakes)
    assert bridge.post("/v1/agents/vie/routines", headers=AUTH,
                       json={"name": "Point du soir", "schedule": "every day at 19:00", "prompt": "Fais le point."}
                       ).status_code == 200
    with persons_turn(bridge, fakes, agent="vie"):
        made_by_agent(bridge, "Relevés", "every 2h", token=VIE, agent="vie")
    asked = call(bridge, "routine_trigger", {"action": "create", "kind": "link", "name": "Raccourci",
                                             "prompt": "Note ce que je dicte."}, token=VIE)
    assert not asked["isError"], text_of(asked)
    assert bridge.post("/v1/agents/vie/triggers", headers=AUTH,
                       json={"name": "Formulaire", "prompt": "Range la réponse.", "kind": "link"}).status_code in (200, 201)
    assert tw.check(bridge, conn)["step"] == "baseline"
    answers = {
        "routines": bridge.get("/v1/agents/vie/routines", headers=AUTH).json(),
        "autonomous": bridge.get("/v1/autonomous", headers=AUTH, params={"agent": "vie"}).json()["items"],
        "approvals": bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"],
        "connectors": bridge.get("/v1/agents/vie/connectors", headers=AUTH).json(),
        "connector": bridge.get(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH).json(),
        "triggers": bridge.get("/v1/triggers", headers=AUTH, params={"agent": "vie"}).json(),
    }
    seen = normal(answers)
    if os.environ.get("TASK_FRAME_RECORD"):
        RECORDED.write_text(json.dumps(seen, ensure_ascii=False, indent=1, sort_keys=True) + "\n")
        return
    base = json.loads(RECORDED.read_text())
    assert without_added(seen) == base


def test_switching_a_broken_watcher_off_or_removing_it_takes_its_card(bridges, fakes):
    """Review C11: its switch in the app, or its removal, answers its card « Réparer / Arrêter »: no « arrêté depuis 7
    jours » later for a watcher the person switched off."""
    bridge, value, conn, _ = tw.setup(bridges, fakes)
    assert tw.check(bridge, conn)["step"] == "baseline"
    value.write_text("fail")
    assert [tw.check(bridge, conn)["step"] for _ in range(3)] == ["failed", "failed", "broken"]
    assert wait_until(lambda: cards(bridge, "bip-fix-", "vie"), timeout=20)
    off = bridge.patch(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH, json={"enabled": False})
    assert off.status_code == 200, off.text
    assert cards(bridge, "bip-fix-", "vie") == []
    kept = json.loads((bridge.state_dir / "connectors" / "connectors.json").read_text())
    assert not kept["agents"]["vie"][0].get("frame", {}).get("fix"), kept["agents"]["vie"][0]


def test_a_proposal_on_a_members_routine_is_hers_never_pushed_to_the_creator(bridges, fakes):
    """Review 2 (C7 bis): a routine a family member asked for (`by`: her household) is hers, its agent's proposal too
    (after her « Réparer », in the repair's turn, nobody's member turn): told to her bridge, never pushed to the
    creator, who neither sees it nor may answer it."""
    bridge = started(with_device(bridges))
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    rid = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                      json={"name": "Menus", "schedule": "every 2h", "prompt": "Propose les menus."}).json()["job"]["id"]
    edit_routine(bridge, rid, by="lea")
    proposed = call(bridge, "routine_schedule", {"action": "update", "name": "Menus", "schedule": "every 3h"})
    assert not proposed["isError"], text_of(proposed)
    card = routine(bridge, rid)["change"]["card"]["id"]
    time.sleep(1.5)
    assert not [p for p in fakes.apns.payloads() if card in json.dumps(p)], "her routine's proposal pushed to the creator"
    assert cards(bridge, "bip-rout-") == []
