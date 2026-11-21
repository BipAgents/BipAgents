"""Two households (two bridge processes) linked by an invitation (`/v1/family…`, `/family/…`), their agents writing to
each other with the bridge's own `message_agent` where the rules of « Entre tes agents » of both people allow it
(rules.rs; set here through an older app's `/v1/agent-links`, which become rules), each bridge telling the other its
side (`/family/rules`) and the ask going through both bridges (`/family/ask`, household_asks.rs). Hermes' Bot Mode is
off everywhere (Alex 21/11): no token, no peer, no mark, its doors gone (tests/test_households.py and test_peers.py,
over HTTP; the agents' Hermes is the fake, echoing what it gets)."""
import hashlib
import json
import os
import time

import yaml

from starlette.responses import JSONResponse

from bbkit.common import AUTH
from bbkit.servers import wait_until
from bbkit.fakes import split_now
from conftest import managed_install, marked, profile

RECEIVED = []


def echo(seen):
    """A Hermes whose Bot Chat is « bot-1 », answering any other call with what it received. The conversation the
    bridge makes for two agents (agent_pairs.rs) is « pair-1 », where Cuisine answers « Des lasagnes. »."""
    if seen.method == "GET" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"data": [{"id": "bot-1", "title": "Bot Chat"}, {"id": "s9", "title": "Courses"}]})
    if seen.method == "POST" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"session": {"id": "pair-1"}})
    if seen.method == "PATCH":
        return JSONResponse({})
    if seen.method == "GET" and seen.path.endswith("/api/sessions/pair-1/messages"):
        return JSONResponse({"messages": [{"role": "assistant", "content": "Des lasagnes."}]})
    if seen.method == "GET" and seen.path.endswith("/api/jobs"):
        return JSONResponse({"jobs": []})   # the routines' one look (routines.rs `migrate`): none
    if seen.method == "GET" and "/v1/runs/" in seen.path:
        return JSONResponse({"status": "completed", "output": "Des lasagnes."})
    RECEIVED.append((seen.path, seen.query, seen.header("authorization"), seen.json()))
    if seen.method == "POST" and seen.path.endswith("/v1/runs"):
        return JSONResponse({"run_id": f"run-{len(RECEIVED)}", "status": "started"}, status_code=202)
    return JSONResponse({"ok": True, "path": seen.path})


def agent_token(bridge, agent):
    """An agent's own token for the bridge's tools (agent-tools.json, as `bipbridge agent-tools` gives one)."""
    token = f"{agent}-tools-token-" + "t" * 20
    path = bridge.state_dir / "agent-tools.json"
    tokens = json.loads(path.read_text())["tokens"] if path.exists() else {}
    tokens[agent] = hashlib.sha256(token.encode()).hexdigest()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"tokens": tokens}))
    return token


def ask_agent(bridge, token, target, message):
    """The agent of `token` asks `target` with the bridge's message_agent: (what it is told, failed)."""
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "message_agent", "arguments": {"target": target, "message": message}}}
    answer = bridge.post("/v1/agent-tools/mcp", json=body, headers={"Authorization": f"Bearer {token}"})
    assert answer.status_code == 200, answer.text
    result = answer.json()["result"]
    return result["content"][0]["text"], result["isError"]


def household(bridges, fakes, name, agents):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
    bridge = bridges.new(name, change=change)
    bridge.config["public_url"] = bridge.url
    return bridge.start()


def test_two_households_write_to_each_other_only_where_both_allowed(bridges, fakes):
    RECEIVED.clear()
    fakes.hermes.override = echo
    a = household(bridges, fakes, "alex", ["vie", "budget"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    refused = a.post("/v1/family/invite", headers=AUTH)
    assert refused.status_code == 409 and refused.json() == {"detail": "set this household's name first"}
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).json() == {"name": "Alex"}
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"}).status_code == 200
    assert a.put("/v1/family/name", headers=AUTH, json={"name": " "}).status_code == 400
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert invitation["kind"] == "family" and invitation["url"] == a.url and invitation["v"] == 1
    assert invitation["name"] == "Alex" and len(invitation["code"]) >= 32
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200 and joined.json() == {"id": "alex", "name": "Alex"}
    again = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert again.status_code == 403 and again.json() == {"detail": "the invitation was refused (expired?)"}
    assert a.get("/v1/family", headers=AUTH).json() == {"name": "Alex", "url": a.url,
                                                        "households": [{"id": "lea", "name": "Léa", "url": b.url, "relink": False}]}
    stored = json.loads((a.state_dir / "households.json").read_text())
    assert oct(os.stat(a.state_dir / "households.json").st_mode & 0o777) == "0o600"
    assert stored["households"]["lea"]["key_in"] == hashlib.sha256(
        json.loads((b.state_dir / "households.json").read_text())["households"]["alex"]["key_out"].encode()).hexdigest()
    # Each side lists the other's agents among its own.
    names = {x["id"]: x for x in a.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert names["cuisine__lea"] == {"id": "cuisine__lea", "name": "Cuisine (Léa)", "managed": False, "household": "lea"}
    assert names["vie"] == {"id": "vie", "name": "Vie", "managed": False}

    # Alex lets his agents write to Cuisine (the rule is per household: Vie and Budget); Léa has not allowed it yet:
    # nothing works, nothing is sent.
    set_links = a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "cuisine__lea"}]})
    assert set_links.json() == {"links": [{"from": "budget", "to": "cuisine__lea"}, {"from": "vie", "to": "cuisine__lea"}],
                                "needs_setup": []}
    assert a.get("/v1/agent-rules", headers=AUTH).json()["households"][0]["out"]["state"] == "waiting"
    vie = agent_token(a, "vie")
    text, failed = ask_agent(a, vie, "bip/cuisine__lea", "qu'est-ce qu'on mange ce soir ?")
    assert failed and "hasn't let you write to Cuisine (Léa)" in text and RECEIVED == [], text
    # Léa allows « Vie (Alex) → Cuisine »: asked in their own conversation at Léa's (a relative's turn), signed with its
    # household and its answer address, the answer back in the same call
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie__alex", "to": "cuisine"}]}).status_code == 200
    text, failed = ask_agent(a, vie, "bip/cuisine__lea", "qu'est-ce qu'on mange ce soir ?")
    assert not failed and "Cuisine (Léa) (bip/cuisine__lea) answered:\nDes lasagnes." in text, text
    path, _, auth, body = [r for r in RECEIVED if r[0].endswith("/v1/runs")][-1]
    assert path == "/p/cuisine/v1/runs" and auth == "Bearer cuisine-key-" + "x" * 24
    now, words = split_now(body["input"])
    note, words = words.split("\n-->\n", 1)
    assert note.startswith("<!--bip:pair\nThis is your own conversation with Vie (Alex) (@vie__alex)")
    assert now and words == "Message from Vie (Alex) (@vie__alex): qu'est-ce qu'on mange ce soir ?"   # the time on top
    # in Cuisine's own conversation with Vie (Alex), never its Bot Chat: what Léa said to Cuisine stays out of it
    assert body["session_id"] == "pair-1"
    # Another household's agent writes: the turn is closed like a relative's (patch 0018's mark)
    assert body["guest_turn"]["tools"][-1] == "web_search" and "mcp__bipagents__weather" in body["guest_turn"]["tools"]
    assert not {"terminal", "session_search", "mcp__bipagents__mail_search"} & set(body["guest_turn"]["tools"])
    # Both apps read it as a pair: « Vie (Alex) ⇄ Cuisine » at Léa's, « Vie ⇄ Cuisine (Léa) » at Alex's
    pair = b.get("/v1/agent-pairs", headers=AUTH, params={"agent": "cuisine"}).json()["pairs"][0]
    assert {"id": "vie__alex", "name": "Vie (Alex)", "category": None, "color": None, "gone": False,
            "household": True} in pair["agents"]
    thread = b.get(f"/v1/agent-pairs/{pair['id']}", headers=AUTH).json()["thread"]
    assert [(m["from"], m["kind"], m["text"]) for m in thread] == [
        ("vie__alex", "ask", "qu'est-ce qu'on mange ce soir ?"), ("cuisine", "answer", "Des lasagnes.")]
    mine = a.get("/v1/agent-pairs", headers=AUTH, params={"agent": "vie"}).json()["pairs"][0]
    thread = a.get(f"/v1/agent-pairs/{mine['id']}", headers=AUTH).json()["thread"]
    assert [(m["from"], m["kind"]) for m in thread] == [("vie", "ask"), ("cuisine__lea", "answer")]
    # Budget writes to Cuisine too: the rule is per household. « À l'écart », it can't, and nothing is sent.
    budget = agent_token(a, "budget")
    assert not ask_agent(a, budget, "bip/cuisine__lea", "coucou")[1]
    assert a.put("/v1/agent-rules/aside", headers=AUTH, json={"agent": "budget", "on": True}).json()["aside"] == ["budget"]
    sent = len(RECEIVED)
    text, failed = ask_agent(a, budget, "bip/cuisine__lea", "coucou")
    assert failed and "hasn't let you write" in text and len(RECEIVED) == sent
    # Hermes' Bot Mode is off: its doors are gone, no token anywhere; nothing without the household's key
    assert a.post("/peer/p/cuisine__lea/v1/runs", json={}).status_code == 404
    assert b.post("/family/p/cuisine/v1/runs", json={}).status_code == 404
    for bridge in (a, b):
        peers = bridge.state_dir / "peers.json"
        assert not peers.exists() or json.loads(peers.read_text()).get("tokens", {}) == {}
    nope = b.post("/family/ask", headers={"Authorization": "Bearer nope"}, json={})
    assert nope.status_code == 401 and nope.json() == {"detail": "unknown household"}
    assert b.get("/family/agents", headers={"Authorization": "Bearer nope"}).status_code == 401

    # Unlinking the household drops its rules.
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 204
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 404
    assert b.get("/v1/agent-links", headers=AUTH).json()["links"] == []
    assert ask_agent(a, vie, "bip/cuisine__lea", "encore ?")[1]


def test_invitations_are_one_time_and_expire(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie"])
    a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    code = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    key = "k" * 40
    answer = a.post("/family/accept", json={"code": code, "name": "Léa", "url": "https://b.ts.net", "key": key})
    assert answer.status_code == 200 and answer.json()["name"] == "Alex" and len(answer.json()["key"]) >= 32
    assert key not in (a.state_dir / "households.json").read_text()        # only a digest of their key
    used = a.post("/family/accept", json={"code": code, "name": "Paul", "url": "https://c.ts.net", "key": key})
    assert used.status_code == 403 and used.json() == {"detail": "this invitation is unknown or expired"}
    agents = a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).json()
    # `told_by`: that household's agents whose family news it reads (its person's links: none yet, option B 19/11)
    assert agents == {"name": "Alex", "agents": [{"id": "vie", "name": "Vie", "told_by": []}]}
    late = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    path = a.state_dir / "households.json"
    data = json.loads(path.read_text())
    data["invites"] = {k: time.time() - 1 for k in data["invites"]}
    path.write_text(json.dumps(data))
    assert a.post("/family/accept", json={"code": late, "name": "Paul", "url": "https://c.ts.net",
                                          "key": "p" * 40}).status_code == 403
    no_url = bridges.new("nourl").start()
    refused = no_url.post("/v1/family/invite", headers=AUTH)
    assert refused.status_code == 409 and refused.json() == {"detail": "this bridge has no address on the tailnet (public_url)"}
    cannot = no_url.post("/v1/family/join", headers=AUTH, json={"url": a.url, "code": "x"})
    assert cannot.status_code == 409


def test_a_new_link_cannot_take_a_linked_households_name(bridges, fakes):
    """Audit 2026-10-27 #9: a second household introducing itself under a linked household's name is refused (409),
    the first link stays as it was and the invitation is still good; the same household, with its current key, may
    link again."""
    a = household(bridges, fakes, "alex", ["vie"])
    a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    key = "k" * 40
    code = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    assert a.post("/family/accept", json={"code": code, "name": "Léa", "url": "https://b.ts.net",
                                          "key": key}).status_code == 200
    before = json.loads((a.state_dir / "households.json").read_text())["households"]
    code = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    intruder = {"code": code, "name": "LÉA", "url": "https://evil.ts.net", "key": "e" * 40}
    taken = a.post("/family/accept", json=intruder)
    assert taken.status_code == 409
    wrong_key = a.post("/family/accept", json=intruder, headers={"Authorization": "Bearer " + "x" * 40})
    assert wrong_key.status_code == 409
    assert json.loads((a.state_dir / "households.json").read_text())["households"] == before
    assert a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).status_code == 200
    assert a.get("/family/agents", headers={"Authorization": "Bearer " + "e" * 40}).status_code == 401
    # Léa's bridge, with the key it calls us with today, links again (same code: the refusals did not use it)
    again = a.post("/family/accept", headers={"Authorization": f"Bearer {key}"},
                   json={"code": code, "name": "Léa", "url": "https://b2.ts.net", "key": "n" * 40})
    assert again.status_code == 200
    assert a.get("/family/agents", headers={"Authorization": "Bearer " + "n" * 40}).status_code == 200
    assert a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).status_code == 401
    assert [h["url"] for h in a.get("/v1/family", headers=AUTH).json()["households"]] == ["https://b2.ts.net"]


def test_two_linked_households_can_link_again(bridges, fakes):
    """Scanning a new invitation of a household already linked (e.g. a link to be redone) still works on both sides."""
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"})
    for _ in range(2):
        invitation = a.post("/v1/family/invite", headers=AUTH).json()
        joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
        assert joined.status_code == 200 and joined.json() == {"id": "alex", "name": "Alex"}
    key = json.loads((b.state_dir / "households.json").read_text())["households"]["alex"]["key_out"]
    assert a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).status_code == 200


def test_the_bot_modes_doors_and_command_are_gone(bridge, fakes):
    """Hermes' Bot Mode is off everywhere (Alex 21/11): no `bipbridge peer-token`, no `/peer/p/…` for the agents'
    gateways, no `/family/p/…` for another household's bridge (household_asks.rs `/family/ask` instead)."""
    assert bridge.cli("peer-token", "--agent", "wellness").returncode == 2
    for path in ("/peer/p/wellness/api/sessions", "/family/p/wellness/v1/runs"):
        assert bridge.get(path, headers={"Authorization": "Bearer x"}).status_code == 404, path
    assert bridge.get("/v1/agent-links", headers=AUTH).json() == {
        "agents": [{"id": "wellness", "name": "Wellness", "managed": False}], "links": []}


def bot_peer(gateway):
    """The `bot_peers.bip` of a gateway's config.yaml (None: Hermes' Bot Mode has no peer there)."""
    path = gateway / "config.yaml"
    data = yaml.safe_load(path.read_text()) if path.exists() else None
    return ((data or {}).get("bot_peers") or {}).get("bip")


def peer_key(gateway):
    """The gateway's own token for the bridge as its peer (None: none)."""
    env = gateway / ".env"
    text = env.read_text() if env.exists() else ""
    return text.split("HERMES_PEER_BIP_KEY=")[1].split("\n")[0].strip() if "HERMES_PEER_BIP_KEY=" in text else None


def test_no_bot_mode_even_towards_another_household_and_each_sender_learns_whom_it_may_write_to(bridges, fakes):
    """`PUT /v1/agent-links` (an older app) through the rules: Hermes' Bot Mode never comes back, towards another
    household neither (the bridge's message_agent goes there, through both bridges); a skill lists each sender's
    targets."""
    RECEIVED.clear()
    bridge = bridges.new("alex")
    hub = managed_install(bridge, fakes, agents=("budget", "maison"))
    for name in ("budget", "maison"):
        profile(hub, name)
    space = bridge.root / "spaces" / "jardin"
    profile(space, "jardin")
    bridge.config["agents"]["jardin"] = {"hermes_url": f"{fakes.hermes.url}/p/jardin", "hermes_key": "j" * 32,
                                         "isolated": True, "sandboxed": True, "space": str(space)}
    bridge.config["public_url"] = bridge.url
    bridge.start()
    lea = household(bridges, fakes, "lea", ["cuisine"])
    assert bridge.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert lea.put("/v1/family/name", headers=AUTH, json={"name": "Léa"}).status_code == 200
    invitation = bridge.post("/v1/family/invite", headers=AUTH).json()
    joined = lea.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200
    unknown = bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "budget", "to": "ghost"}]})
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown link budget → ghost"}
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "budget", "to": "budget"}]}).status_code == 400

    # Within the household: the contacts skill lists the targets
    local = [{"from": "Budget", "to": "jardin"}, {"from": "jardin", "to": "budget"}, {"from": "budget", "to": "maison"}]
    result = bridge.put("/v1/agent-links", headers=AUTH, json={"links": local}).json()
    assert result == {"links": [{"from": "budget", "to": "jardin"}, {"from": "budget", "to": "maison"},
                                {"from": "jardin", "to": "budget"}], "needs_setup": []}
    contacts = (hub / "profiles" / "budget" / "skills" / "agent-contacts" / "SKILL.md").read_text()
    assert "`bip/jardin`: Jardin" in contacts and "`bip/maison`: Maison" in contacts
    assert not (hub / "profiles" / "maison" / "skills" / "agent-contacts").exists()   # writes to nobody
    managed = {a["id"]: a["managed"] for a in bridge.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert {k: managed[k] for k in ("wellness", "budget", "maison", "jardin")} == {
        "wellness": False, "budget": True, "maison": True, "jardin": True}

    # Alex's agents may write to Léa's Cuisine and she agrees: still no Bot Mode anywhere
    both = local + [{"from": "jardin", "to": "cuisine__lea"}]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": both}).json()["needs_setup"] == []
    agreed = lea.put("/v1/agent-rules/households/alex", headers=AUTH, json={"way": "in", "mode": "only", "agents": ["cuisine"]})
    assert agreed.status_code == 200 and agreed.json()["households"][0]["in"]["state"] == "agreed"
    maison_contacts = hub / "profiles" / "maison" / "skills" / "agent-contacts" / "SKILL.md"
    assert wait_until(lambda: maison_contacts.exists() and "`bip/cuisine__lea`" in maison_contacts.read_text(), timeout=10)
    for gateway in (hub, space):
        assert bot_peer(gateway) is None and peer_key(gateway) is None
    assert not any(marked(f) for f in (hub / "profiles" / "budget", hub / "profiles" / "maison", space / "profiles" / "jardin"))
    peers = bridge.state_dir / "peers.json"
    assert not peers.exists() or json.loads(peers.read_text()).get("tokens", {}) == {}
    # Wellness (set up by hand) writes only to this household: nothing to ask its administrator for
    again = bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "wellness", "to": "budget"}]}).json()
    assert again["needs_setup"] == []
    assert not (hub / "profiles" / "budget" / "skills" / "agent-contacts" / "SKILL.md").exists()


def test_a_place_set_up_with_bot_mode_between_its_own_agents_loses_it_at_start(bridges, fakes):
    """A place whose links were set while every sender got Bot Mode: at start the hub's peer and token go (Bot Mode is
    off everywhere), the links of before, the gateway's other settings and the contacts stay."""
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("budget", "maison"))
    for name in ("budget", "maison"):
        (profile(hub, name) / "profile.yaml").write_text("description: x\nui_meta:\n  hermes-bots: {}\n")
    (hub / "config.yaml").write_text(yaml.safe_dump({"model": {"default": "glm"}, "bot_peers": {
        "bip": {"url": "http://127.0.0.1:1/peer", "note": "old"}, "other": {"url": "http://example.test"}}}))
    (hub / ".env").write_text("HERMES_PEER_BIP_KEY=old-token\nOTHER=1\n")
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "peers.json").write_text(json.dumps(
        {"links": [["budget", "maison"]], "tokens": {str(hub): hashlib.sha256(b"old-token").hexdigest()}}))
    bridge.start()
    assert bot_peer(hub) is None and peer_key(hub) is None
    assert not marked(hub / "profiles" / "budget") and not marked(hub / "profiles" / "maison")
    kept = yaml.safe_load((hub / "config.yaml").read_text())
    assert kept["model"]["default"] == "glm" and kept["bot_peers"] == {"other": {"url": "http://example.test"}}
    assert "OTHER=1" in (hub / ".env").read_text()
    stored = json.loads((bridge.state_dir / "peers.json").read_text())
    assert stored["tokens"] == {} and stored["links"] == [["budget", "maison"]]
    assert bridge.post("/peer/p/maison/v1/runs", headers={"Authorization": "Bearer old-token"},
                       json={"message": "Message from Budget (@budget): coucou"}).status_code == 404   # no such door
    assert "`bip/maison`" in (hub / "profiles" / "budget" / "skills" / "agent-contacts" / "SKILL.md").read_text()
