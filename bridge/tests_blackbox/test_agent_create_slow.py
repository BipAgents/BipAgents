"""A slow creation (26/11, Alex testing 1.1 on his own install, which keeps no spare agents): agents Bip proposed were
said « n'a pas pu être créé », then the home offered to link them a while later. The app had given up waiting, its
connection closed, and the creation stopped half way: its Hermes profile made, the bridge's config without it.

Now (agent_making.rs): the creation runs in a task of its own, never cut by the app; while it runs its profile is never
offered to link; past `[guide] answer_wait_seconds` the card answers « still being made » (202) and the same tap waits
for the same agent; a real failure is tried once more, then said as one, nothing of it left behind."""
import uuid

import httpx
import pytest

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_agent_create import GUIDE_TOKEN
from test_guide import place


def card_for(bridge, name, role="Mes recettes de la semaine"):
    """Bip (made first) proposes « Créer <name> »: the card, as the apps read it."""
    bip = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"}).json()["agent"]["id"]
    give_token(bridge, bip, GUIDE_TOKEN)
    made = call(bridge, "propose_agent", {"kind": "daily", "name": name, "role": role, "for": "me"}, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    return next(c for c in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if c.get("name") == name)


def accept_url(bridge, card):
    return f"{bridge.url}/v1/guide/proposals/{card['id']}/accept"


def listing(bridge):
    return bridge.get("/v1/agents", headers=AUTH).json()


def ids(bridge):
    return [a["id"] for a in listing(bridge)["agents"]]


def test_a_card_the_app_gave_up_on_is_made_all_the_same_and_never_offered_to_link(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    card = card_for(bridge, "Cuisine")
    go_on = bridge.root / "go-on"
    # Hermes slow (a busy machine): the profile is made, a command after it takes its time
    bridge.commands.rule("hermes", ["-p", "cuisine", "tools", "disable"], hold=str(go_on), times=1)
    # the app gives up waiting and its connection closes (Android at 60 s, the iPhone at 90 s)
    with pytest.raises(httpx.TimeoutException):
        httpx.post(accept_url(bridge, card), headers=AUTH, json={"lang": "fr"}, timeout=1.5)
    assert (home / "profiles" / "cuisine" / "config.yaml").is_file()
    # being made: never a profile « created on the server » to link on the home
    assert listing(bridge)["unclaimed"] == []
    go_on.touch()
    assert wait_until(lambda: "cuisine" in ids(bridge), timeout=30), bridge.log_tail()
    after = listing(bridge)
    assert after["unclaimed"] == []
    cuisine = next(a for a in after["agents"] if a["id"] == "cuisine")
    assert cuisine["requested"], cuisine   # on the home of each of the person's phones at once
    # the same tap again (its answer lost on the way): the same agent, never a second one
    again = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert again.status_code in (200, 201), again.text
    assert again.json()["agent"]["id"] == "cuisine"
    assert sorted(p.name for p in (home / "profiles").iterdir() if p.name.startswith("cuisine")) == ["cuisine"]
    assert not (home / "profiles" / "cuisine" / ".bipagents-making").exists()


def test_a_slow_card_answers_still_being_made_then_its_agent(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    bridge.config["guide"] = {"answer_wait_seconds": 1}
    bridge.restart()
    card = card_for(bridge, "Sport", role="Mon plan de course pour mars")
    go_on = bridge.root / "go-on"
    bridge.commands.rule("hermes", ["-p", "sport", "tools", "disable"], hold=str(go_on), times=1)
    first = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert first.status_code == 202, first.text
    assert first.json() == {"type": "agent", "creating": True, "name": "Sport", "kind": "daily", "agent": None}
    # tapped again while it is made: still the same agent being made, no second one started
    assert bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"}).status_code == 202
    assert len([c for c in bridge.commands.of("hermes") if c[:2] == ["profile", "create"] and "sport" in c]) == 1
    # the card stays until the agent is made (another phone shows it waiting, not gone)
    assert [c["id"] for c in bridge.get("/v1/guide", headers=AUTH).json()["proposals"]] == [card["id"]]
    go_on.touch()

    answers = []

    def done():
        answers.append(bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"}))
        return answers[-1].status_code in (200, 201)
    assert wait_until(done, timeout=30, interval=0.5), bridge.log_tail()
    out = answers[-1].json()
    assert out["type"] == "agent" and out["agent"]["id"] == "sport" and out["agent"]["name"] == "Sport"
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []


def test_a_creation_failing_once_is_tried_again_and_a_real_failure_leaves_nothing(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    card = card_for(bridge, "Budget")
    # once: a command that fails (a busy machine), then it works
    bridge.commands.rule("hermes", ["-p", "budget", "tools", "disable"], exit=1, stderr="Error: busy", times=1)
    made = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert made.status_code == 201, made.text
    assert made.json()["agent"]["id"] == "budget"
    # for good: said as a failure, the card still there to tap again, nothing of it left to « link »
    card = card_for(bridge, "Jardin", role="Mon potager")
    bridge.commands.rule("hermes", ["-p", "jardin", "tools", "disable"], exit=1, stderr="Error: broken")
    failed = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert failed.status_code == 502, failed.text
    assert "could not be created" in failed.json()["detail"]
    tries = [c for c in bridge.commands.of("hermes") if c[:4] == ["-p", "jardin", "tools", "disable"]]
    assert len(tries) == 2   # once more, then said
    assert not (home / "profiles" / "jardin").exists()
    view = listing(bridge)
    assert view["unclaimed"] == [] and "jardin" not in ids(bridge)
    assert card["id"] in [c["id"] for c in bridge.get("/v1/guide", headers=AUTH).json()["proposals"]]


def test_the_form_the_app_gave_up_on_is_made_once_and_put_on_the_home(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    go_on = bridge.root / "go-on"
    bridge.commands.rule("hermes", ["-p", "voyages", "tools", "disable"], hold=str(go_on), times=1)
    client = uuid.uuid4().hex
    body = {"name": "Voyages", "kind": "daily", "lang": "fr", "client_id": client}
    with pytest.raises(httpx.TimeoutException):
        httpx.post(f"{bridge.url}/v1/agents", headers=AUTH, json=body, timeout=1.5)
    assert listing(bridge)["unclaimed"] == []
    go_on.touch()
    assert wait_until(lambda: "voyages" in ids(bridge), timeout=30), bridge.log_tail()
    assert next(a for a in listing(bridge)["agents"] if a["id"] == "voyages")["requested"]
    # the same request again: the agent made, not a second one
    again = bridge.post("/v1/agents", headers=AUTH, json=body)
    assert again.status_code in (200, 201) and again.json()["id"] == "voyages", again.text
    assert "voyages-2" not in ids(bridge)


def test_a_creation_cut_by_a_stop_is_removed_at_the_next_start(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    # left by a creation the bridge's stop cut short: its mark, no section in the config
    left = home / "profiles" / "plantes"
    left.mkdir(parents=True)
    (left / "config.yaml").write_text("model:\n  provider: lmstudio\n")
    (left / ".bipagents-making").write_text("")
    # a profile the person made on the server (no mark): still offered
    theirs = home / "profiles" / "perso"
    theirs.mkdir(parents=True)
    (theirs / "config.yaml").write_text("model:\n  provider: lmstudio\n")
    bridge.restart()
    assert wait_until(lambda: not left.exists(), timeout=15), bridge.commands.of("hermes")
    assert [p["id"] for p in listing(bridge)["unclaimed"]] == ["perso"]
