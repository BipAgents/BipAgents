"""A new agent « launched » by the one that made it (launch.rs, Alex 26/11): made from a card of Bip's, it gets Bip's
brief in its own conversation (its role, tone, what Bip knows, what to do first, the usual rule), signed as an agent's
message (the apps draw it as Bip's), and starts on it at once in a turn of its own. A team card's agents the same, with
their mission and first actions. Made by hand (the form): not launched, its welcome opens its conversation."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_agent_create import GUIDE_TOKEN, next_run
from test_guide import place


def bip(bridge):
    made = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"}).json()["agent"]["id"]
    give_token(bridge, made, GUIDE_TOKEN)
    return made


def runs_of(fakes, agent):
    return [r for r in fakes.hermes.started.values() if r["agent"] == agent]


def test_an_agent_bip_made_starts_on_its_first_step_after_bips_brief(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    guide = bip(bridge)
    made = call(bridge, "propose_agent", {"kind": "wellness", "name": "Forme", "role": "Me préparer à mon semi-marathon",
                                          "tone": "motivant", "for": "me", "handover": "Elle court le matin.",
                                          "first": "Préparer le plan de la première semaine"}, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    card = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert card["first"] == "Préparer le plan de la première semaine"
    next_run(fakes)
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    assert accepted.json()["launched"] is True
    forme = accepted.json()["agent"]["id"]
    assert wait_until(lambda: runs_of(fakes, forme), timeout=30), bridge.log_tail()
    run = runs_of(fakes, forme)[0]
    # Bip's brief, signed as an agent's message, then the hidden rules of its first turn
    assert "<!--bip:launch" in run["input"] and "Don't answer Bip" in run["input"]
    brief = run["input"][run["input"].index("Message from Bip"):]
    assert brief.startswith(f"Message from Bip (@{guide}): Bienvenue Forme ! Je viens de te créer.\n"), brief
    for line in ("Ton rôle : Me préparer à mon semi-marathon.", "Ton ton : motivant.",
                 "Ce que je sais déjà : Elle court le matin.", "Pour commencer : Préparer le plan de la première semaine.",
                 "Comme toujours : tu demandes l'accord avant toute action sensible, et rien ne part sans accord."):
        assert line in brief, brief
    # in its own main conversation, drawn as Bip's message
    def shown():
        page = bridge.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": forme}).json()
        return [i for i in page["items"] if i.get("role") == "peer"]
    assert wait_until(shown, timeout=15)
    peer = shown()[0]
    assert (peer["from"], peer["from_name"]) == (guide, "Bip") and peer["text"].startswith("Bienvenue Forme !")
    # no template welcome on top: its own first message answers Bip's brief
    assert forme not in bridge.get("/v1/guide", headers=AUTH).json()["welcome"]


def test_the_form_does_not_launch_and_the_switch_turns_it_off(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    made = bridge.post("/v1/agents", headers=AUTH, json={"name": "Cuisine", "kind": "daily", "lang": "fr"})
    assert made.status_code == 201, made.text
    assert "cuisine" in bridge.get("/v1/guide", headers=AUTH).json()["welcome"]
    assert not runs_of(fakes, "cuisine")
    # `[guide] first_turns = false`: a card's agent opens with its welcome, as before
    bridge.config["guide"] = {"first_turns": False}
    bridge.restart()
    bip(bridge)
    call(bridge, "propose_agent", {"kind": "daily", "name": "Budget", "role": "Suivre mes dépenses", "for": "me"},
         token=GUIDE_TOKEN)
    card = next(c for c in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if c.get("name") == "Budget")
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201 and accepted.json()["launched"] is False, accepted.text
    assert "budget" in bridge.get("/v1/guide", headers=AUTH).json()["welcome"]
    assert not runs_of(fakes, "budget")


VELO_TOKEN = "velo-token-0123456789abcdefghijklmno"


def rpc_as(bridge, token, path, method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    resp = bridge.post(path, json=body, headers={"Authorization": f"Bearer {token}"}, timeout=120)
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def test_a_bip_with_a_watcher_sets_up_its_own_watcher_once_launched(bridges, fakes):
    """26/11, Alex (his place of the offer): « crée un Bip avec un veilleur »: the agent first made the watcher for
    itself, then said the new agent could not make one, and the new agent found its watchers' tool only after he
    insisted. Now: the creator is told the new agent makes it itself (never « it can't »), Bip's brief gives it as its
    first step with the tool to use, and the new agent has the watchers' tool and makes its watcher (the person's card,
    as for any watcher)."""
    from concurrent.futures import ThreadPoolExecutor
    from conftest import managed_install, profile, wait_registered
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    bridge.config["sandbox"] = {"isolate": "on"}
    hub = managed_install(bridge, fakes, agents=("coach",), isolated=True)
    bridge.config["hermes"]["python"] = ["/usr/bin/false"]   # the hub's Python keeps its agents' vaults
    # what every agent is cloned from: its tools in Hermes' docker sandbox (the watchers offered, connectors.rs)
    settings = "model:\n  provider: lmstudio\nterminal:\n  backend: docker\n"
    (hub / "config.yaml").write_text(settings)
    coach = profile(hub, "coach")
    (coach / "config.yaml").write_text(settings)
    (coach / "SOUL.md").write_text("Tu es Coach.\n")
    bridge.commands.rule("podman", ["network", "exists"], exit=1)
    for name in ("coach", "bip", "velo"):   # the sandboxes' volumes, the new agents' too
        for kind in ("files", "envoi", "share"):
            volume = bridge.root / "volumes" / f"{kind}-{name}"
            volume.mkdir(parents=True)
            bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}", f"bipagents-{kind}-{name}"],
                                 stdout=f"{volume}\n")
    bridge.start()
    wait_registered(bridge, ["coach"])
    guide = bip(bridge)
    made = call(bridge, "propose_agent", {"kind": "daily", "name": "Vélo", "for": "me",
                                          "role": "Me prévenir quand le prix du vélo baisse sous 900 €",
                                          "first": "Mettre en place le veilleur du prix du vélo"}, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    # the creator: the watcher is the new agent's to make, and it can
    said = text_of(made)
    assert "starts on its first step itself" in said and "watchers" in said and "never say it can't" in said, said
    card = next(c for c in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if c.get("name") == "Vélo")
    next_run(fakes)
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201 and accepted.json()["launched"] is True, accepted.text
    velo = accepted.json()["agent"]["id"]
    assert wait_until(lambda: runs_of(fakes, velo), timeout=30), bridge.log_tail()
    brief = runs_of(fakes, velo)[0]["input"]
    assert "Pour commencer : Mettre en place le veilleur du prix du vélo." in brief, brief
    assert f"(@{guide})" in brief and "YOUR watcher: connector_register with watch {every, notify}" in brief, brief
    # its own tools: the watchers' one is there from the start
    give_token(bridge, velo, VELO_TOKEN)
    listed = rpc_as(bridge, VELO_TOKEN, "/v1/agent-tools/connectors", "tools/list")["tools"]
    assert "connector_register" in {t["name"] for t in listed}, [t["name"] for t in listed]
    # it makes its watcher: the person's card, as for any watcher
    work = profile(hub, velo) / "sandboxes" / "docker" / "default" / "workspace" / "connecteurs" / "velo"
    work.mkdir(parents=True)
    (work / "check.py").write_text("import json\nprint(json.dumps({'state': {'price': 950}, 'summary': '950 €'}))\n")
    register = {"name": "Prix du vélo", "sites": ["www.velo.example"], "description": "Le prix du vélo",
                "icon": "bicycle", "folder": "/workspace/connecteurs/velo", "entry": "python3 check.py",
                "watch": {"every": "1h", "notify": "on_condition"}}
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(rpc_as, bridge, VELO_TOKEN, "/v1/agent-tools/connectors", "tools/call",
                            {"name": "connector_register", "arguments": register})

        def card_of_velo():
            items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": velo}).json()["items"]
            return [i for i in items if i["run_id"].startswith("bip-conn-")]
        assert wait_until(lambda: card_of_velo() or asked.done(), timeout=30), bridge.log_tail()
        assert card_of_velo(), asked.result()
        request = card_of_velo()[0]["request_id"]
        assert bridge.post(f"/v1/connectors/requests/{request}", headers=AUTH, json={"allow": False}).status_code == 200
        asked.result(timeout=60)
