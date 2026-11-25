"""Alex 25/11: a finished background task's screens (« En bref », the « Terminées » row, « Lire le résultat complet »)
and its « Rapport » page showed the report's hidden spoken version as text (« …leroy-merlin-tickets.md). <!--bip:say
J'ai retrouvé la facture : … »). The bridge never passes a reply's `<!--bip:say …-->` (spoken.rs) on in a text the
apps show, here in the views test_spoken_version.py does not cover: a task's report (`/v1/jobs`) and its page, a report
kept with it before (cleaned once at start), a read-only share's items, a guest agent's words in a group discussion,
and the conversation of a reply whose spoken version follows an empty mark (GLM, 10/11)."""
import json
import threading
import time

import pytest

from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token
from bbkit.servers import wait_until
from test_households_and_peers import household
from test_long_work import jobs, with_device
from test_read_shares import FakeGuichet, Phone, open_item
from test_rooms import messages, send
from test_shared_agents import link

WORDS = ("Les deux tickets sont ci-dessous, et une fiche récapitulative est dans ta mémoire des e-mails.\n\n"
         "- Tuyau Geolia 25 m : 39,90 €\n- Raccords : 7,50 €")
SAY = "<!--bip:say J'ai retrouvé la facture : le tuyau Geolia de 25 mètres, acheté le 12 mai.-->"
REPORT = f"{WORDS}\n\n{SAY}"
SPOKEN = "retrouvé la facture"


def shown(value) -> str:
    return value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)


def words_only(value) -> bool:
    text = shown(value)
    return "bip:" not in text and SPOKEN not in text and "--&gt;" not in text


@pytest.mark.slow
def test_a_tasks_report_and_its_page_show_its_words_only(bridges, fakes):
    """A task started from a conversation that is gone meanwhile: its report in `/v1/jobs`, its « Rapport » page and
    the templated message in the Boîte."""
    bridge = with_device(bridges, {})
    give_token(bridge)
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.run_outputs = {"r1": REPORT}
    gate = threading.Event()
    hermes.runs["run_g"] = [sse("tool.started", run_id="run_g", tool="mcp_bipagents_start_task", preview="x"), gate,
                            sse("run.completed", run_id="run_g", output="ok")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_g", "session_id": "api_gone"})
    assert wait_until(lambda: hermes.seen("/v1/runs/run_g/events"))
    time.sleep(0.3)
    started = call(bridge, "start_task", {"title": "Tickets Leroy Merlin", "instructions": "Retrouve mes tickets."})
    assert not started["isError"]

    def refuse(seen):
        from starlette.responses import JSONResponse
        body = seen.json() if seen.method == "POST" and seen.path == "/v1/runs" else None
        if body and body.get("session_id") == "api_gone":
            return JSONResponse({"error": "Session not found"}, status_code=404)
        return None

    hermes.override = refuse
    gate.set()
    assert wait_until(lambda: boite_items(bridge), timeout=40)
    job = jobs(bridge)[0]
    assert job["state"] == "done" and job["report"] == WORDS, job["report"]
    page = bridge.get("/v1/media", headers=AUTH, params={"path": f"/bipagents/pages/wellness/rapport-{job['id']}.html"})
    assert page.status_code == 200 and "Tuyau Geolia 25 m" in page.text
    assert words_only(page.text), page.text
    assert words_only(boite_items(bridge))


def test_a_report_kept_with_its_spoken_version_is_cleaned_at_start(bridges):
    """A task that ended before 25/11: its report and its page cleaned once when the bridge starts. A long one (its
    report keeps the first 4 000 characters, its page has them all) has its spoken version on its page only."""
    bridge = bridges.new()
    folder = bridge.state_dir / "jobs"
    folder.mkdir(parents=True, exist_ok=True)
    pages = bridge.state_dir / "agent-pages" / "wellness"
    pages.mkdir(parents=True, exist_ok=True)
    escaped = SAY.replace("<", "&lt;").replace(">", "&gt;").replace("'", "&#x27;")
    long_words = "<li>Tuyau Geolia 25 m : 39,90 €</li>\n" * 150
    kept = {"t0123456789ab": (REPORT, "<li>Tuyau Geolia 25 m : 39,90 €</li>\n"),
            "t0123456789ac": (("- Tuyau Geolia 25 m : 39,90 €\n" * 130).rstrip(), long_words)}
    for job_id, (report, items) in kept.items():
        (folder / f"{job_id}.json").write_text(json.dumps({
            "id": job_id, "kind": "task", "agent": "wellness", "title": "Tickets", "prompt": "Retrouve mes tickets.",
            "state": "done", "report": report[:4000], "created": 1_000.0, "started": 1_000.0, "finished": 1_100.0,
            "seen": True}))
        html = (f"<!doctype html><html><head><title>Report — Tickets</title></head><body><main>\n"
                f"<h1>Report — Tickets</h1>\n<p>Les deux tickets sont ci-dessous.</p>\n<ul>\n{items}</ul>\n"
                f"<p>{escaped}</p>\n</main></body></html>")
        for name, text in [(f"rapport-{job_id}.source", html), (f"rapport-{job_id}.html", html),
                           (f"rapport-{job_id}.json", json.dumps({"title": "Report — Tickets", "version": 1,
                                                                  "created": "2026-11-20T08:00:00", "bytes": len(html)}))]:
            (pages / name).write_text(text)
    bridge.start()
    listed = {j["id"]: j for j in jobs(bridge)}
    assert listed["t0123456789ab"]["report"] == WORDS, listed["t0123456789ab"]["report"]
    assert listed["t0123456789ac"]["report"] == kept["t0123456789ac"][0][:4000]
    for job_id in kept:
        page = bridge.get("/v1/media", headers=AUTH, params={"path": f"/bipagents/pages/wellness/rapport-{job_id}.html"})
        assert page.status_code == 200 and "Tuyau Geolia 25 m" in page.text
        assert words_only(page.text), (job_id, page.text[-400:])


def test_a_read_only_share_carries_the_words_only(bridge, fakes):
    guichet = FakeGuichet().start()
    try:
        fakes.hermes.add_session("sess-tickets", [
            {"id": 1, "role": "user", "content": "Tu retrouves mes tickets Leroy Merlin ?"},
            {"id": 2, "role": "assistant", "content": REPORT},
            {"id": 3, "role": "assistant", "content": "| Article | Prix |\n|---|---|\n| Tuyau | 39,90 € |\n\n"
                                                      "<!--bip:say-->\nJ'ai retrouvé la facture du tuyau."},
        ], title="Tickets")
        share = "share0123456789abcdef"
        made = bridge.post("/v1/read-shares", headers=AUTH, json={
            "agent": "wellness", "session": "sess-tickets", "title": "Tickets", "from": "Alex", "share": share,
            "writer": "writer-token-0123456789", "guichet": guichet.url, "ends": time.time() + 86400})
        assert made.status_code == 200, made.text
        link = bridge.post(f"/v1/read-shares/{share}/invites", headers=AUTH, json={"name": "Léa"}).json()
        lea = Phone(link["link"])
        guichet.redeem(lea.invite, lea.public, lea.proof(), "1")
        assert bridge.post(f"/v1/read-shares/{share}/sync", headers=AUTH).status_code == 200
        key = lea.unwrap({v["viewer"]: v for v in guichet.viewers}["1"]["wrapped"], share, 1)
        items = {i: open_item(key, guichet.items[i]["data"]) for i in guichet.items}
        assert items["m-2"]["text"] == WORDS, items["m-2"]["text"]
        assert items["m-3"]["text"] == "| Article | Prix |\n|---|---|\n| Tuyau | 39,90 € |", items["m-3"]["text"]
        assert words_only(items)
    finally:
        guichet.stop()


def test_a_guest_agents_words_in_a_group_discussion(bridges, fakes):
    """A relative's agent answers on its own bridge (rooms_remote.rs): its words cross, never its spoken version."""
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "marie", ["cuisine"])
    link(a, b)
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "cuisine__marie"}]}).status_code == 200
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie__alex", "to": "cuisine"}]}).status_code == 200
    made = a.post("/v1/rooms", headers=AUTH, json={"participants": ["vie", "cuisine@marie"], "title": "Tickets"})
    assert made.status_code == 200, made.text
    rid = made.json()["id"]
    fakes.hermes.run_output = REPORT
    assert send(a, rid, "@cuisine tu as les tickets ?").status_code == 200
    end, page = time.time() + 40, {}
    while time.time() < end:
        page = messages(a, rid)
        if page["pending"]["room"]["answering"] == [] and len(page["items"]) >= 2:
            break
        time.sleep(0.5)
    last = page["items"][-1]
    assert (last["from"], last["role"]) == ("cuisine__marie", "agent") and last["text"] == WORDS, last
    assert words_only(page["items"])


def test_a_spoken_version_after_an_empty_mark_never_shows_in_the_conversation(bridges, fakes):
    """GLM wrote its spoken version after an empty mark (10/11, spoken.rs): the rest is spoken, not shown."""
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [
        {"role": "user", "content": "Mes tickets ?"},
        {"role": "assistant", "content": f"{WORDS}\n\n<!--bip:say-->\nJ'ai retrouvé la facture du tuyau."}],
        title="Bot Chat", last_active=10, started_at=1)
    bridge = bridges.new().start()
    main = bridge.get("/v1/threads/main", params={"agent": "wellness"}, headers=AUTH).json()
    items = bridge.get(f"/v1/threads/{main['id']}/timeline", headers=AUTH).json()["items"]
    reply = next(i for i in items if i["kind"] == "message" and i.get("role") == "agent")
    assert reply["text"] == WORDS and reply["spoken"] == "J'ai retrouvé la facture du tuyau.", reply
