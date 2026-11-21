"""An agent that left the shared hub for its own space only because other people talk to it comes back by itself
(bridge-rs/src/own_space_back.rs; Alex 21/11: what comes from someone else runs as a guest turn, and no agent leaves
the hub for that): at start, once idle, its conversations, memory and credentials with it, its space stopped and
removed. A way back a restart cut off is taken up, before or after the switch. Never a project agent.

Places made before 21/11 have such agents: here Fête moves out as a project agent did then (own_space.rs), gets a login
in its space, then is a project agent no more and is shared with Léa (an agent other people talk to)."""
import json
import shutil
import sqlite3
import time
import tomllib

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import wait_registered
import test_own_space as own

BINDING = "gateway.api.binding.v1."
LOGIN = {"label": "Traiteur", "site": "traiteur.example", "identifier": "alex@example.org", "password": "s3cret-Pass!"}


def section(bridge, agent="fete"):
    return tomllib.loads(bridge.toml_text())["agents"].get(agent, {})


def moved_out(bridges, fakes):
    """Fête in its own space with a login there, the bridge stopped; returns (bridge, hub, its space, the stop marker)."""
    bridge, hub = own.hub_place(bridges, fakes, agents=("coach",), project="fete", hub_python=own.hub_podman,
                                tidy_after=1)
    bridge.commands.rule("podman", ["exec", "-i", "hermes-hub"], stdout=json.dumps({"id": "vault_1"}) + "\n")
    marker = bridge.root / "stopped.txt"
    bridge.config["spaces"]["stop"] = f"echo {{agent}} >> {marker}"
    folder = hub / "profiles" / "fete"
    with sqlite3.connect(folder / "state.db") as db:   # a conversation of the hub, Hermes' owner record with it
        db.execute("CREATE TABLE messages (session_id TEXT, content TEXT)")
        db.execute("INSERT INTO messages VALUES ('api_1', 'Prévois la fête de samedi')")
        db.execute("CREATE TABLE state_meta (key TEXT PRIMARY KEY, value TEXT)")
        db.execute("INSERT INTO state_meta VALUES (?, ?)", (BINDING + "api_1", json.dumps(
            {"profile_id": str(folder.resolve()), "session_id": "api_1", "route": "agent:main:api_server:dm:api_1",
             "entry": {"session_id": "api_1"}})))
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])
    root = bridge.root / "spaces" / "fete"
    assert wait_until(lambda: section(bridge).get("space") == str(root), timeout=120), bridge.log_tail(4000)
    assert wait_until(lambda: not folder.exists(), timeout=20)   # the hub forgot it (its tombstone left)
    profile = root / "profiles" / "fete"
    # what it learned and said in its space
    (profile / "memories" / "MEMORY.md").write_text("Alex court le dimanche. La fête est samedi.")
    with sqlite3.connect(profile / "state.db") as db:
        db.execute("INSERT INTO messages VALUES ('api_1', 'Gâteau commandé')")
    made = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["fete"], "confirmed_others": True})
    assert made.status_code == 201, made.text
    assert own.hub_vault_calls(bridge)[-1]["home"] == str(profile)
    bridge.stop()
    return bridge, hub, root, marker


def now_shared(bridge):
    (bridge.state_dir / "project-agents.json").write_text(json.dumps({"projects": {}}))
    (bridge.state_dir / "households.json").write_text(json.dumps({"households": {"lea": {"name": "Léa"}}}))
    (bridge.state_dir / "agent-shares.json").write_text(json.dumps({"shared": {"fete": ["lea"]}}))


def vault_calls_in_hub(bridge, hub):
    return [c for c in own.hub_vault_calls(bridge) if c["home"] == str(hub / "profiles" / "fete")]


def back_in_hub(bridge, fakes):
    s = section(bridge)
    return s.get("hermes_url") == f"{fakes.hermes.url}/p/fete" and not s.get("space") and not s.get("isolated")


def test_an_agent_others_talk_to_comes_back_into_the_hub_with_everything(bridges, fakes):
    bridge, hub, root, marker = moved_out(bridges, fakes)
    now_shared(bridge)
    bridge.start(write=False)
    wait_registered(bridge, ["coach", "fete"])
    assert wait_until(lambda: back_in_hub(bridge, fakes), timeout=90), bridge.log_tail(4000)
    # its space stopped and removed once nothing goes on there
    assert wait_until(lambda: not root.exists(), timeout=30), bridge.log_tail(4000)
    assert marker.read_text() == "fete\n"
    folder = hub / "profiles" / "fete"
    assert (folder / "SOUL.md").read_text().startswith("Tu es Fete.")
    assert (folder / "memories" / "MEMORY.md").read_text() == "Alex court le dimanche. La fête est samedi."
    with sqlite3.connect(folder / "state.db") as db:
        said = [r[0] for r in db.execute("SELECT content FROM messages ORDER BY rowid")]
        owner = json.loads(db.execute("SELECT value FROM state_meta WHERE key = ?", (BINDING + "api_1",)).fetchone()[0])
    assert said == ["Prévois la fête de samedi", "Gâteau commandé"]
    assert owner["profile_id"] == str(folder.resolve())   # the hub's again: Hermes runs its conversations there
    assert not (hub / "profiles" / ".deleted" / "fete").exists()   # Hermes serves the name again
    assert not (bridge.state_dir / "own-space-back.json").exists()
    # its login written again into its hub profile's vault from the bridge's master copy (its copy there lacked it)
    calls = vault_calls_in_hub(bridge, hub)
    assert [c["op"] for c in calls] == ["list", "add"], calls
    assert calls[1]["label"] == "Traiteur" and calls[1]["secret"]["identifier"] == "alex@example.org"
    listing = bridge.get("/v1/vault", headers=AUTH).json()
    assert {a["id"]: a.get("why") for a in listing["agents"]}["fete"] is None
    assert [i["agents"] for i in listing["items"]] == [["fete"]]   # (the apps see names, never Hermes' handles)
    # and usable: a new login goes into the hub profile's vault, the hub stays the person's (Coach keeps holding)
    again = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "label": "Salle", "site": "salle.example",
                                                         "agents": ["fete", "coach"], "confirmed_others": True})
    assert again.status_code == 201, again.text
    assert [c["op"] for c in vault_calls_in_hub(bridge, hub)][-1] == "add"
    assert not section(bridge, "coach").get("space") and not (bridge.root / "spaces" / "coach").exists()
    # nothing moves it out again
    time.sleep(3)
    assert back_in_hub(bridge, fakes) and not root.exists()


def test_a_way_back_cut_off_before_the_switch_is_made_again(bridges, fakes):
    """The bridge stopped after the hub had its copy, before it switched: at start that copy is deleted through Hermes,
    then the agent comes back from its space, as it is there."""
    bridge, hub, root, marker = moved_out(bridges, fakes)
    now_shared(bridge)
    left = hub / "profiles" / "fete"
    left.mkdir(parents=True)
    (left / "SOUL.md").write_text("Une copie qu'un retour coupé a laissée.\n")
    (left / "config.yaml").write_text("terminal:\n  backend: docker\n")
    (hub / "profiles" / ".deleted" / "fete").unlink()
    (bridge.state_dir / "own-space-back.json").write_text(json.dumps({"agents": {"fete": str(root)}}))
    deletes = lambda: [e for e in bridge.commands.entries("hermes") if e["argv"] == ["profile", "delete", "fete", "--yes"]]
    before = len(deletes())
    bridge.start(write=False)
    wait_registered(bridge, ["coach", "fete"])
    assert wait_until(lambda: back_in_hub(bridge, fakes), timeout=90), bridge.log_tail(4000)
    assert wait_until(lambda: not root.exists(), timeout=30), bridge.log_tail(4000)
    assert len(deletes()) == before + 1 and deletes()[-1]["hermes_home"] == str(hub)
    assert (left / "SOUL.md").read_text().startswith("Tu es Fete.")
    assert (left / "memories" / "MEMORY.md").read_text() == "Alex court le dimanche. La fête est samedi."
    assert not (bridge.state_dir / "own-space-back.json").exists()


def test_a_way_back_cut_off_after_the_switch_is_finished(bridges, fakes):
    """The bridge stopped once it served the agent from the hub, its space still there: at start the rest is done (its
    tools, files, credentials in the hub) and the space removed."""
    bridge, hub, root, marker = moved_out(bridges, fakes)
    now_shared(bridge)
    shutil.copytree(root / "profiles" / "fete", hub / "profiles" / "fete", symlinks=True)
    (hub / "profiles" / ".deleted" / "fete").unlink()
    (bridge.state_dir / "own-space-back.json").write_text(json.dumps({"agents": {"fete": str(root)}}))
    bridge.start()   # the config as the test wrote it: Fête served by the hub, the switch made
    wait_registered(bridge, ["coach", "fete"])
    assert back_in_hub(bridge, fakes)
    assert wait_until(lambda: not root.exists(), timeout=30), bridge.log_tail(4000)
    assert marker.read_text() == "fete\n"
    assert wait_until(lambda: not (bridge.state_dir / "own-space-back.json").exists(), timeout=10)
    assert [c["op"] for c in vault_calls_in_hub(bridge, hub)] == ["list", "add"]
    assert (hub / "profiles" / "fete" / "memories" / "MEMORY.md").read_text().endswith("La fête est samedi.")


def test_a_project_agent_in_its_space_never_comes_back(bridges, fakes):
    bridge, hub, root, marker = moved_out(bridges, fakes)
    bridge.start(write=False)   # still a project agent: its guests' turns are its own
    wait_registered(bridge, ["coach", "fete"])
    time.sleep(4)
    assert section(bridge).get("space") == str(root) and (root / "profiles" / "fete" / "SOUL.md").exists()
    assert not (hub / "profiles" / "fete").exists() and not marker.exists()
