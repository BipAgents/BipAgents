"""Credentials and the shared hub (bridge-rs/src/own_space.rs, vault.rs `holds_in_hub`). The person's own agents keep
their secrets right where they are, in the hub, and never move for it (Alex, 24/11, for 1.1). Only a project agent
breaks the hub's privacy (its guests' turns are its own): it leaves the hub as soon as it is made, its share opening
once that move ended; while one is still on its way out, the hub's agents keep nothing new (an agent's ask waits for the
move under way, or gets one plain answer at once). Agents other people talk to otherwise (the family's agent, one
shared with a relative, one another household's agents write to) stay in the hub (21/11, guest turns)."""
import json
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor

import pytest
import yaml

from bbkit.common import AUTH
from bbkit.mcp import not_done
from bbkit.servers import free_port, wait_until
from conftest import managed_install, profile, wait_registered

TOKEN = ""
TOOLS = "/v1/agent-tools/connectors"
ASK = {"name": "garmin", "site": "connect.garmin.example", "kind": "password",
       "identifier_label": "E-mail de ton compte Garmin", "reason": "Pour tes séances"}


def hub_place(bridges, fakes, agents=("coach",), start=None, protected=True, project=None, hub_python=None,
              tidy_after=None):
    bridge = bridges.new()
    bridge.config["sandbox"] = {"isolate": "on"}   # its tools in a container (the default on Linux)
    agents = tuple(agents) + ((project,) if project else ())
    hub = managed_install(bridge, fakes, agents=agents, isolated=protected,
                          python=hub_python(bridge) if hub_python else ["/usr/bin/false"])
    if project:   # an agent other people talk to, in the hub
        bridge.state_dir.mkdir(parents=True, exist_ok=True)
        (bridge.state_dir / "project-agents.json").write_text(json.dumps({"projects": {project: {
            "created": time.time(), "ends": time.time() + 86400, "archived": None, "from": None, "lang": "fr"}}}))
    (hub / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\n")   # the person's own server
    (hub / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-hub\n")
    for name in agents:
        folder = profile(hub, name)
        (folder / "SOUL.md").write_text(f"Tu es {name.title()}.\n")
        (folder / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "docker"}}))
        (folder / "memories").mkdir()
        (folder / "memories" / "MEMORY.md").write_text("Alex court le dimanche.")
    bridge.config["spaces"] = {"root": str(bridge.root / "spaces"), "first_port": free_port(),
                               "start": start or f"{bridge.fakebin}/fake-gateway {{home}}"}
    if tidy_after is not None:
        bridge.config["spaces"]["tidy_after"] = tidy_after
    return bridge, hub


def use_token_of(hub):
    """The agent's token as its Hermes has it (its profile, copied as it is when it moves: the same token)."""
    global TOKEN
    TOKEN = entries(hub, "coach")["bipagents"]["headers"]["Authorization"].split()[1]


def tool(bridge, name, arguments, timeout=180):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
    resp = bridge.post(TOOLS, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=timeout)
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def text(result):
    return result["content"][0]["text"]


def secret_cards(bridge, agent="coach"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-secret-")]


def entries(hub, name):
    path = hub / "profiles" / name / "config.yaml"
    return yaml.safe_load(path.read_text()).get("mcp_servers", {}) if path.exists() else {}


def why(bridge):
    return {a["id"]: a.get("why") for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}


def test_an_agent_asking_a_secret_while_a_project_agent_is_in_the_hub_never_moves(bridges, fakes):
    """24/11: a project agent still in the hub (made before, its move out at start not due yet): the person's agent
    keeps the tools, its ask gets the plain answer at once (no move under way to wait for) and nothing moves; the app's
    give says « dans quelques minutes » (needs_own_space). Red before: Coach moved into a space of its own."""
    bridge, hub = hub_place(bridges, fakes, project="fete")
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])
    use_token_of(hub)
    servers = entries(hub, "coach")
    assert "bipagents_connectors" in servers and "bipagents_vault" in servers
    result = tool(bridge, "ask_secret", ASK, timeout=30)
    assert not_done(result) and "try again a bit later" in text(result) and "space" not in text(result)
    assert not secret_cards(bridge)
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    assert not tomllib.loads(bridge.toml_text())["agents"]["coach"].get("isolated")
    assert why(bridge)["coach"] == "needs_own_space"
    assert "moving into its own space" not in bridge.log_tail(20000)
    assert not any(e["argv"][:2] == ["profile", "delete"] for e in bridge.commands.entries("hermes"))


@pytest.mark.slow
def test_an_ask_while_the_project_agent_leaves_waits_and_keeps_the_secret_in_the_hub(bridges, fakes):
    """The project agent's move out of the hub under way (its space slow to start): the person's agent asking a secret
    waits for it, then gets the card and keeps the secret right in the hub. Red before: Coach moved too."""
    bridge, hub = hub_place(bridges, fakes, project="fete", hub_python=hub_podman, tidy_after=0)
    bridge.config["spaces"]["start"] = f"sleep 12; {bridge.fakebin}/fake-gateway {{home}}"
    bridge.commands.rule("podman", ["exec", "-i", "hermes-hub"], stdout=json.dumps({"id": "vault_1"}) + "\n")
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])
    use_token_of(hub)
    assert wait_until(lambda: "moving out of the shared hub" in bridge.log_tail(20000), timeout=30), bridge.log_tail(4000)
    fete_out = lambda: tomllib.loads(bridge.toml_text())["agents"]["fete"].get("isolated") is True
    assert not fete_out()
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "ask_secret", ASK)
        assert wait_until(lambda: secret_cards(bridge), timeout=60), bridge.log_tail(4000)
        assert fete_out()   # the card came once the project agent was out
        card = secret_cards(bridge)[0]
        sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
        assert sent.status_code == 200, sent.text
        result = call.result(timeout=60)
    assert not result["isError"] and "Saved as « garmin »" in text(result), text(result)
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    assert not tomllib.loads(bridge.toml_text())["agents"]["coach"].get("isolated")
    assert why(bridge)["coach"] is None


def test_no_move_when_its_space_would_hold_the_desktop_too(bridges, fakes):
    """Investigation of 24/11: a home install without the visible browser's containers (the person answered « n »),
    whose agents' sandbox holds Hermes' Bot Desktop (installer setup_desktop, `bot_desktop.auto_start`), isolates no
    sandbox. Its space would get the same settings and refuse the secret all the same: the agent moved (~250 MB), kept
    nothing, and stayed mute. Now nothing moves: the refusal that exists, at once."""
    bridge, hub = hub_place(bridges, fakes, project="fete")
    (hub / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\nbot_desktop:\n  auto_start: true\n")
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])
    use_token_of(hub)
    assert "bipagents_connectors" not in entries(hub, "coach")   # not offered either
    result = tool(bridge, "ask_secret", ASK)
    assert not_done(result) and "Connectors and secrets aren't available to you here" in text(result), text(result)
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    assert tomllib.loads(bridge.toml_text())["agents"]["coach"].get("isolated") is not True
    assert not secret_cards(bridge)
    assert "moving into its own space" not in bridge.log_tail(4000)


def test_family_project_and_unprotected_agents_are_never_offered_the_tools(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes, agents=("coach", "famille", "fete"))
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": "famille", "created": 1.0, "lang": "fr"}))
    (bridge.state_dir / "project-agents.json").write_text(json.dumps({"projects": {"fete": {
        "created": time.time(), "ends": time.time() + 86400, "archived": None, "from": None, "lang": "fr"}}}))
    bridge.start()
    wait_registered(bridge, ["coach", "famille", "fete"])
    assert "bipagents_connectors" in entries(hub, "coach")
    for name in ("famille", "fete"):
        assert "bipagents_connectors" not in entries(hub, name), name


def test_an_install_that_does_not_protect_the_vault_offers_none(bridges, fakes):
    other, other_hub = hub_place(bridges, fakes, protected=False)
    other.start()
    wait_registered(other, ["coach"])
    servers = entries(other_hub, "coach")
    assert "bipagents_connectors" not in servers and "bipagents_vault" not in servers


def hub_podman(bridge):
    return [f"{bridge.fakebin}/podman", "exec", "-i", "hermes-hub", "/opt/hermes/venv/bin/python"]


def hub_vault_calls(bridge):
    return [json.loads(e["stdin"]) for e in bridge.commands.entries("podman") if e["argv"][:3] == ["exec", "-i", "hermes-hub"]]


def test_a_hub_of_the_person_s_own_agents_keeps_secrets_without_moving(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes, agents=("coach", "cuisine"), hub_python=hub_podman)
    bridge.commands.rule("podman", ["exec", "-i", "hermes-hub"], stdout=json.dumps({"id": "vault_1"}) + "\n")
    bridge.start()
    wait_registered(bridge, ["coach", "cuisine"])
    use_token_of(hub)
    servers = entries(hub, "coach")
    assert "bipagents_connectors" in servers and "bipagents_vault" in servers
    vault = {a["id"]: a.get("why") for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert {k: vault[k] for k in ("coach", "cuisine")} == {"coach": None, "cuisine": None}
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "ask_secret", ASK)
        assert wait_until(lambda: secret_cards(bridge), timeout=20), bridge.log_tail(4000)
        card = secret_cards(bridge)[0]
        assert card["description"] == "Coach a besoin de ton accès Garmin (connect.garmin.example)"
        sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
        assert sent.status_code == 200, sent.text
        result = call.result(timeout=60)
    assert not result["isError"] and "Saved as « garmin »" in text(result), text(result)
    # nothing moved: no space, still served by the hub
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    section = tomllib.loads(bridge.toml_text())["agents"]["coach"]
    assert not section.get("isolated") and not section.get("space")
    assert not any(e["argv"][:2] == ["profile", "delete"] for e in bridge.commands.entries("hermes"))
    # a login from the app goes into the hub profile's own vault, through the hub's Python
    made = bridge.post("/v1/vault", headers=AUTH, json={"label": "SNCF", "site": "sncf-connect.com",
                                                        "identifier": "alex@example.org", "password": "s3cret-Pass!",
                                                        "agents": ["coach"]})
    assert made.status_code == 201, made.text
    [request] = hub_vault_calls(bridge)
    assert request["home"] == str(hub / "profiles" / "coach") and request["op"] == "add"


def test_while_a_project_agent_is_in_the_hub_nobody_there_gets_credentials(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes, agents=("coach", "famille"), project="fete", hub_python=hub_podman)
    (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": "famille", "created": 1.0, "lang": "fr"}))
    bridge.start()
    wait_registered(bridge, ["coach", "famille", "fete"])
    vault = {a["id"]: a.get("why") for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert {k: vault[k] for k in ("coach", "famille", "fete")} == dict.fromkeys(("coach", "famille", "fete"), "needs_own_space")
    for name in ("coach", "famille", "fete"):
        made = bridge.post("/v1/vault", headers=AUTH, json={"label": "SNCF", "site": "sncf-connect.com",
                                                            "identifier": "a@b.c", "password": "p", "agents": [name]})
        assert made.status_code >= 400, made.text
    assert not hub_vault_calls(bridge)
    assert "bipagents_connectors" in entries(hub, "coach")   # offered: kept in the hub once the project agent is out


def test_an_agent_another_household_writes_to_stays_in_the_hub_and_the_hub_stays_private(bridges, fakes):
    """Alex 21/11: what comes from someone else runs as a guest turn (household_asks.rs), and no agent leaves the hub
    because others talk to it: the person's agents keep their credentials in the hub."""
    bridge, hub = hub_place(bridges, fakes, agents=("coach", "cuisine"), hub_python=hub_podman)
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    # a relative's agents may write to Cuisine, both sides agreeing (« Entre tes agents », rules.rs)
    (bridge.state_dir / "households.json").write_text(json.dumps({"households": {"lea": {"name": "Léa"}}}))
    (bridge.state_dir / "agent-rules.json").write_text(json.dumps({"households": {"lea": {
        "in": {"mode": "only", "agents": ["cuisine"]}, "theirs": {"in": {"mode": "all"}}}}}))
    bridge.start()
    wait_registered(bridge, ["coach", "cuisine"])
    vault = {a["id"]: a.get("why") for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert {k: vault[k] for k in ("coach", "cuisine")} == {"coach": None, "cuisine": None}
    assert not (bridge.root / "spaces" / "cuisine").exists()


def test_a_new_project_agent_leaves_the_hub_at_once_and_its_share_opens_after(bridges, fakes):
    """A project agent never lives in the person's hub: made, it moves into its own space at once (no minute's wait,
    no turn awaited), and its friends' share, asked meanwhile, opens once that move ended. The hub keeps only the
    person's agents, whose credentials stay there. Red before: it moved a minute later, the share opened in the hub."""
    from test_project_agents import share_body
    from test_read_shares import FakeGuichet

    bridge, hub = hub_place(bridges, fakes, agents=("coach",), hub_python=hub_podman)
    bridge.config["spaces"]["start"] = f"sleep 3; {bridge.fakebin}/fake-gateway {{home}}"
    bridge.start()
    wait_registered(bridge, ["coach"])
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Fête", "ends": time.time() + 86400})
    assert made.status_code == 201, made.text
    agent = made.json()["agent"]

    def moved():
        section = tomllib.loads(bridge.toml_text())["agents"].get(agent, {})
        return section.get("isolated") is True and section.get("space") == str(bridge.root / "spaces" / agent)
    assert not moved()
    guichet = FakeGuichet().start()
    try:
        shared = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, agent), timeout=60)
        assert moved(), bridge.log_tail(4000)   # the share waited for the move
        assert shared.status_code == 200, shared.text
    finally:
        guichet.stop()
    assert wait_until(lambda: any(e["argv"] == ["profile", "delete", agent, "--yes"]
                                  for e in bridge.commands.entries("hermes")), timeout=20)
    assert why(bridge)["coach"] is None


@pytest.mark.slow
def test_an_others_facing_agent_left_in_the_hub_moves_out_at_start(bridges, fakes):
    """An install updated with a project agent still in its hub: at start, the project agent moves into its own space
    by itself once idle; the hub is then the person's alone, and their agent keeps a secret right there."""
    bridge, hub = hub_place(bridges, fakes, agents=("coach",), project="fete", hub_python=hub_podman, tidy_after=1)
    bridge.commands.rule("podman", ["exec", "-i", "hermes-hub"], stdout=json.dumps({"id": "vault_1"}) + "\n")
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])
    use_token_of(hub)

    def moved():
        section = tomllib.loads(bridge.toml_text())["agents"].get("fete", {})
        return section.get("isolated") is True and section.get("space") == str(bridge.root / "spaces" / "fete")
    assert wait_until(moved, timeout=120), bridge.log_tail(4000)
    assert (bridge.root / "spaces" / "fete" / "profiles" / "fete" / "SOUL.md").read_text().startswith("Tu es Fete.")
    vault = {a["id"]: a.get("why") for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert vault["coach"] is None
    # the person's own agent keeps its secret in the hub: the card at once, nothing moves
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "ask_secret", ASK)
        assert wait_until(lambda: secret_cards(bridge), timeout=20), bridge.log_tail(4000)
        card = secret_cards(bridge)[0]
        sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
        assert sent.status_code == 200, sent.text
        result = call.result(timeout=60)
    assert not result["isError"] and "Saved as « garmin »" in text(result), text(result)
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    assert not tomllib.loads(bridge.toml_text())["agents"]["coach"].get("isolated")


def test_an_unprotected_install_is_never_tidied(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes, agents=("coach",), project="fete", protected=False, tidy_after=0)
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])
    time.sleep(3)
    assert not (bridge.root / "spaces" / "fete").exists()
    assert not tomllib.loads(bridge.toml_text())["agents"]["fete"].get("isolated")


def test_an_agent_shared_with_a_relative_gets_a_secret_only_once_confirmed(bridges, fakes):
    """An agent other people talk to (here shared with Léa's household) may keep a secret, but only once the person
    confirmed it: the card says who else talks to it, and the bridge refuses an answer without `confirmed_others` (an
    app from before the confirmation) with a plain refusal. It keeps it in the hub (Alex 21/11: their turns are guest
    turns, no agent leaves the hub because others talk to it)."""
    bridge, hub = hub_place(bridges, fakes, agents=("coach",), hub_python=hub_podman)
    bridge.commands.rule("podman", ["exec", "-i", "hermes-hub"], stdout=json.dumps({"id": "vault_1"}) + "\n")
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "households.json").write_text(json.dumps({"households": {"lea": {"name": "Léa"}}}))
    (bridge.state_dir / "agent-shares.json").write_text(json.dumps({"shared": {"coach": ["lea"]}}))
    bridge.start()
    wait_registered(bridge, ["coach"])
    use_token_of(hub)
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "ask_secret", ASK)
        assert wait_until(lambda: secret_cards(bridge), timeout=60), bridge.log_tail(4000)
        assert not tomllib.loads(bridge.toml_text())["agents"]["coach"].get("isolated")   # still in the hub
        card = secret_cards(bridge)[0]
        assert card["bip_secret_others"] is True and card["bip_secret_others_names"] == ["Léa"]
        answer = {"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"}
        old_app = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH, json=answer)
        assert old_app.status_code == 409, old_app.text
        assert old_app.json()["detail"]["code"] == "confirm_others"
        assert "Mets à jour l’app" in old_app.json()["detail"]["message"]
        assert "Fake-Pa55word" not in old_app.text
        assert secret_cards(bridge), "the card still waits"
        sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={**answer, "confirmed_others": True})
        assert sent.status_code == 200, sent.text
        result = call.result(timeout=60)
    assert not result["isError"] and "Saved as « garmin »" in text(result), text(result)
