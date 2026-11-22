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
