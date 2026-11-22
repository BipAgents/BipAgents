"""« Initiative », per agent (proactive agents, part 2: bridge-rs/src/agent_proactive.rs; passation-pro
briefs/design-agents-proactifs.md §2; contracts/proactive/cases.json): three levels, « off · basic · smart », basic for
every agent of the person until they change it (Alex 22/11, D1 A, D4 B), in the agent's settings and set with
`PUT /v1/agents/{id}/proactive`; none for an agent other people talk to (409); « arrête de me proposer des choses »
turns it off, only in a turn of the person's own conversation, never back on (D3 A). An app 1.0 sees its settings as
before, with one more key it ignores."""
import json
import threading
import time
from pathlib import Path

from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, not_done, rpc, text_of
from bbkit.servers import wait_until
from test_agents_admin import managed

CASES = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "proactive" / "cases.json").read_text())
TOKEN = "proactive-token-0123456789abcdefghij"


def settings(bridge, agent):
    resp = bridge.get(f"/v1/agents/{agent}/settings", headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()


def put(bridge, agent, body):
    return bridge.put(f"/v1/agents/{agent}/proactive", headers=AUTH, json=body)


def tool_names(bridge, token=TOKEN):
    tools = rpc(bridge, "tools/list", token=token).json()["result"]["tools"]
    return [t["name"] for t in tools]


def kept(bridge):
    path = bridge.state_dir / "agent_proactive.json"
    return json.loads(path.read_text()) if path.exists() else None


def own_and_project(bridges, fakes):
    """A managed place with Budget (the person's own) and a project agent (others talk to it)."""
    bridge, home = managed(bridges, fakes, isolated=True)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Budget", "kind": "finance"}).status_code == 201
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Voyage à Lisbonne", "ends": time.time() + 86400 * 10,
                                                                   "lang": "fr", "category": "creative"})
    assert made.status_code == 201, made.text
    return bridge, made.json()["agent"]


def test_the_contract_s_settings_and_answers(bridges, fakes):
    bridge, project = own_and_project(bridges, fakes)
    agents = {"own": "budget", "others_facing": project, "nobody": "nobody"}
    # nobody changed anything: basic, and no file at all
    assert settings(bridge, "budget")["proactive"] == {"level": "basic", "smart_ok": False}
    assert kept(bridge) is None
    for case in CASES["put"]:
        resp = put(bridge, agents[case["agent"]], case["body"])
        assert resp.status_code == case["status"], (case["name"], resp.text)
        if case["status"] == 200:
            assert resp.json() == {"id": agents[case["agent"]], **case["answer"]}, case["name"]
            assert settings(bridge, "budget")["proactive"]["level"] == case["answer"]["level"], case["name"]
        elif case["answer"] is not None:
            assert resp.json() == case["answer"], case["name"]
    # the cases end on « basic »: nothing kept for Budget, the file 0600
    assert kept(bridge) == {"levels": {}}
    assert (bridge.state_dir / "agent_proactive.json").stat().st_mode & 0o777 == 0o600
    for case in CASES["settings"]:
        agent = agents[case["agent"]]
        if case["stored"] and case["agent"] == "own":
            assert put(bridge, agent, {"level": case["stored"]}).status_code == 200
        shown = settings(bridge, agent)
        assert shown.get("proactive") == case["proactive"], (case["name"], shown.get("proactive"))
        put(bridge, "budget", {"level": "basic"})


def test_an_agent_others_talk_to_has_no_initiative(bridges, fakes):
    bridge, project = own_and_project(bridges, fakes)
    shown = settings(bridge, project)
    assert "proactive" not in shown
    refused = put(bridge, project, {"level": "basic"})
    assert refused.status_code == 409 and refused.json() == {"detail": "others_facing"}
    assert kept(bridge) is None


def test_stop_suggesting_turns_it_off_only_in_the_persons_own_turn(bridges, fakes):
    bridge, _ = own_and_project(bridges, fakes)
    give_token(bridge, "budget", TOKEN)
    assert put(bridge, "budget", {"level": "smart"}).status_code == 200
    # its tool, for every app version: « arrête de me proposer des choses »
    assert "proactive_off" in tool_names(bridge)
    [tool] = [t for t in rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"] if t["name"] == "proactive_off"]
    assert tool["inputSchema"] == {"type": "object", "properties": {}}   # no level to give: never turned on by it

    # no turn of the person's: a routine's, a watcher's, an e-mail's… refused, nothing changed
    refused = call(bridge, "proactive_off", {}, token=TOKEN)
    assert not_done(refused) and "only your person" in text_of(refused)
    assert settings(bridge, "budget")["proactive"]["level"] == "smart"

    # the person's own conversation, their turn followed from the app, inside the call
    gate = threading.Event()
    fakes.hermes.runs["run_p"] = [sse("tool.started", run_id="run_p", tool="mcp_bipagents_proactive_off", preview="x"),
                                  gate, sse("run.completed", run_id="run_p", output="C'est noté.")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "budget", "run_id": "run_p", "session_id": "api_p"}).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/p/budget/v1/runs/run_p/events"))
    time.sleep(0.3)
    try:
        done = call(bridge, "proactive_off", {"level": "smart"}, token=TOKEN, session="api_p")
        assert done["isError"] is False and "bottom of your page" in text_of(done), done
        assert settings(bridge, "budget")["proactive"] == {"level": "off", "smart_ok": False}
        again = call(bridge, "proactive_off", {}, token=TOKEN, session="api_p")
        assert "already off" in text_of(again)
        assert settings(bridge, "budget")["proactive"]["level"] == "off"
    finally:
        gate.set()
    assert kept(bridge) == {"levels": {"budget": "off"}}
    # turned on again only by the person, on its page
    assert put(bridge, "budget", {"level": "basic"}).json() == {"id": "budget", "level": "basic"}


def test_off_holds_back_its_own_ideas_and_what_is_asked_still_goes(bridges, fakes):
    """Review 1 (B2): « Éteinte » (« Il ne propose rien de lui-même. Il fait ce que tu lui demandes. »): its
    personality gets the block that holds back its own ideas (a background task, a word for the family, a project
    agent), its ongoing conversations are told; back to basic, the block goes; the person's personality never shows it."""
    from test_agents_admin import hermes_args
    bridge, home = managed(bridges, fakes)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Budget", "kind": "finance"}).status_code == 201
    soul_file = home / "profiles" / "budget" / "SOUL.md"
    before = soul_file.read_text()
    assert "bipagents:initiative" not in before
    bridge.commands.clear()
    assert put(bridge, "budget", {"level": "off"}).status_code == 200
    soul = soul_file.read_text()
    assert "<!-- bipagents:initiative -->" in soul and "never as your own idea" in soul and "propose_task" in soul
    assert wait_until(lambda: ["-p", "budget", "sessions", "refresh-prompts", "--json"] in hermes_args(bridge))
    assert "bipagents:initiative" not in settings(bridge, "budget")["soul"]
    # smart or basic again: out
    assert put(bridge, "budget", {"level": "smart"}).status_code == 200
    assert "bipagents:initiative" not in soul_file.read_text()
    assert put(bridge, "budget", {"level": "off"}).status_code == 200
    assert put(bridge, "budget", {"level": "basic"}).status_code == 200
    assert soul_file.read_text() == before


def test_a_turn_another_agent_asked_for_never_turns_it_off(bridges, fakes):
    """Review 1 (B1): Quotidien asks Coach (« message_agent », their pair's own conversation): Coach's turn carries
    Quotidien's words, never its person's; a proactive_off called in it is refused (it runs in their pair's conversation:
    runs.rs `persons_own_turn`, agent_pairs.rs `is_pair_session`)."""
    from concurrent.futures import ThreadPoolExecutor
    from test_agent_messages import TOKENS as PAIR_TOKENS, ask
    from test_agent_pairs import linked

    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    linked(bridge, fakes, [{"from": "quotidien", "to": "coach"}])
    fakes.hermes.rounds = 10_000   # Coach's run goes on until the test lets it end
    run_id = f"r{len(fakes.hermes.started) + 1}"
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("tool.started", run_id=run_id, tool="mcp_bipagents_proactive_off", preview="x"), gate,
                                 sse("run.completed", run_id=run_id, output="D'accord.")]
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(ask, bridge, "quotidien", target="bip/coach", message="Arrête de me proposer des choses.")
        assert wait_until(lambda: run_id in fakes.hermes.started and fakes.hermes.seen(f"/p/coach/v1/runs/{run_id}/events"))
        time.sleep(0.5)
        session = fakes.hermes.started[run_id]["session_id"]
        refused = call(bridge, "proactive_off", {}, token=PAIR_TOKENS["coach"], session=session)
        assert not_done(refused) and "only your person" in text_of(refused), refused
        assert settings(bridge, "coach")["proactive"]["level"] == "basic"
        gate.set()
        fakes.hermes.rounds = 1
        asked.result(timeout=60)
    assert kept(bridge) is None


def test_a_watchers_urgent_message_is_time_sensitive_and_the_same_words_are_pushed_once(bridges, fakes):
    """Review 1 (R3, R4): a watcher's URGENT message reaches the phone at the system's urgent level
    (`aps.interruption-level` time-sensitive, `urgent: 1`: watchers.rs → background_turn.rs `deliver_urgent`); its turn
    read the line rule in its hidden note; the same words again soon after are in the conversation again, not pushed
    again (`same_report`, a watcher's only)."""
    from test_watchers import boite, check, setup as watcher_setup, turns
    bridge, value, conn, _ = watcher_setup(bridges, fakes)
    assert check(bridge, conn)["step"] == "baseline"
    fakes.hermes.run_output = "URGENT\nUn créneau ce soir à 18 h : réserve vite."
    value.write_text("ce soir 18:00")
    assert check(bridge, conn)["urgent"] is True
    [turn] = turns(fakes)
    assert "a reminder or a single piece of news fits in one short line" in turn["input"], turn["input"]
    assert turn["input"].count("-->") == 1 and turn["input"].endswith("-->")
    words = "Un créneau ce soir à 18 h : réserve vite."
    assert wait_until(lambda: [i for i in boite(bridge) if i["text"] == words], timeout=20)
    first = next(i for i in boite(bridge) if i["text"] == words)
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("m") == f"o_{first['id']}"], timeout=20)
    [push] = [p for p in fakes.apns.payloads() if p.get("m") == f"o_{first['id']}"]
    assert push["aps"]["interruption-level"] == "time-sensitive" and push["urgent"] == 1 and push["t"] == "boite", push
    # an ordinary message of a watcher is still not pushed at all; nothing else is urgent
    others = [p for p in fakes.apns.payloads() if p.get("m") != f"o_{first['id']}"]
    assert all("interruption-level" not in p.get("aps", {}) for p in others), others
    # the same words again, at once: in the conversation again, no second push
    value.write_text("ce soir 18:30")
    assert check(bridge, conn)["urgent"] is True
    assert wait_until(lambda: len([i for i in boite(bridge) if i["text"] == words]) == 2, timeout=20)
    time.sleep(1.5)
    assert len([p for p in fakes.apns.payloads() if p.get("aps", {}).get("interruption-level")]) == 1


def test_an_app_1_0_sees_its_settings_as_before(bridges, fakes):
    """What an app 1.0 reads is unchanged: the same keys and values, `proactive` beside them; « Me prévenir » as
    before; its tools' list keeps every tool it had."""
    bridge, _ = own_and_project(bridges, fakes)
    give_token(bridge, "budget", TOKEN)
    before = settings(bridge, "budget")
    tools_before = tool_names(bridge)
    assert put(bridge, "budget", {"level": "off"}).status_code == 200
    after = settings(bridge, "budget")
    assert {k: v for k, v in after.items() if k != "proactive"} == {k: v for k, v in before.items() if k != "proactive"}
    assert bridge.put("/v1/agents/budget/notify", headers=AUTH, json={"on": False}).json() == {"id": "budget", "on": False}
    assert settings(bridge, "budget")["notify"] == {"on": False}
    # nothing it had goes (plans and offers come with later branches; off takes only those out)
    assert set(tools_before) <= set(tool_names(bridge))
