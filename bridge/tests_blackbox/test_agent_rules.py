"""« Entre tes agents » as rules (bridge-rs rules.rs, rules_live.rs; Alex 21/11, piste A): the screen and its small
changes, the migration at start, and two households agreeing way by way — a home (reached only through its own
long poll, family_relay.rs) and a place — each bridge telling the other its side (`POST /family/rules`, signed with
the link's own key), a proposal a card in the other person's Boîte."""
import json
import os

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_family_relay import echo, households, install, named
from test_households_and_peers import give_peer_token


def screen(bridge):
    answer = bridge.get("/v1/agent-rules", headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()


def links(view):
    return {(l["from"], l["to"]) for l in view["effective"]["links"]}


def rule_cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-rules-")]


def test_a_new_account_writes_to_all_and_reads_nothing_then_small_changes(bridges, fakes):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in ("vie", "budget", "compta")}
    bridge = bridges.new("alex", change=change).start()
    view = screen(bridge)
    assert (view["write"], view["read"], view["default"]) == ("all", "none", {"write": "all", "read": "none"})
    assert len(links(view)) == 6 and view["effective"]["reads"] == []
    assert oct(os.stat(bridge.state_dir / "agent-rules.json").st_mode & 0o777) == "0o600"
    view = bridge.put("/v1/agent-rules/aside", headers=AUTH, json={"agent": "compta", "on": True}).json()
    assert links(view) == {("vie", "budget"), ("budget", "vie")} and view["aside"] == ["compta"]
    view = bridge.put("/v1/agent-rules/write", headers=AUTH, json={"mode": "some"}).json()
    assert links(view) == set()
    view = bridge.put("/v1/agent-rules/pairs", headers=AUTH, json={"from": "vie", "to": "budget", "on": True}).json()
    assert links(view) == {("vie", "budget")}
    # an older app reads and writes through the rules
    assert bridge.get("/v1/agent-links", headers=AUTH).json()["links"] == [{"from": "vie", "to": "budget"}]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "budget", "to": "vie"}]}).status_code == 200
    assert links(screen(bridge)) == {("budget", "vie")}
    assert bridge.put("/v1/agent-rules/write", headers=AUTH, json={"mode": "most"}).status_code == 400


def test_the_pairs_of_before_become_rules_once_at_start(bridges, fakes):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in ("vie", "wellness")}
    bridge = bridges.new("alex", change=change)
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    before = {"links": [["vie", "wellness"]], "tokens": {}}
    (bridge.state_dir / "peers.json").write_text(json.dumps(before))
    (bridge.state_dir / "shares.json").write_text(json.dumps({"reads": [["vie", "wellness"]]}))
    bridge.start()
    view = screen(bridge)
    assert (view["write"], view["pairs"]) == ("some", [{"from": "vie", "to": "wellness"}])
    assert (view["read"], view["reads"]) == ("some", [{"reader": "vie", "source": "wellness"}])
    made = json.loads((bridge.state_dir / "agent-rules.json").read_text())
    assert made["migrated"]["links"] == 1 and made["migrated"]["reads"] == 1
    assert "agent rules made from the pairs" in bridge.log_tail(20000)
    # the files of before stay as they were (a bridge rolled back finds them)
    assert json.loads((bridge.state_dir / "peers.json").read_text())["links"] == before["links"]
    text = (bridge.state_dir / "agent-rules.json").read_text()
    bridge.stop()
    bridge.start()
    assert (bridge.state_dir / "agent-rules.json").read_text() == text


def test_a_home_and_a_place_agree_way_by_way_through_the_relay(bridges, fakes):
    fakes.hermes.override = echo
    home = install(bridges, fakes, "alex", ["vie", "budget"], place=False)
    place = install(bridges, fakes, "chi", ["cuisine"], place=True)
    named(home, "Alex")
    named(place, "Chi")
    invitation = place.post("/v1/family/invite", headers=AUTH).json()
    joined = home.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200 and households(home)["chi"]["pull"] is True
    assert wait_until(lambda: sorted(a["id"] for a in screen(place)["households"][0]["agents"]) == ["budget", "vie"],
                      timeout=20), place.log_tail()

    # Chi opens her Cuisine to Alex's agents: « en attente d'Alex » until he answers his card, through his long poll
    set_in = place.put("/v1/agent-rules/households/alex", headers=AUTH,
                       json={"way": "in", "mode": "only", "agents": ["cuisine"]})
    assert set_in.status_code == 200, set_in.text
    assert set_in.json()["households"][0]["in"]["state"] == "waiting"
    assert wait_until(lambda: rule_cards(home), timeout=20), home.log_tail()
    [card] = rule_cards(home)
    assert card["description"] == "Chi ouvre son agent Cuisine à tes agents"
    assert card["bip_rules"]["way"] == "out" and card["bip_rules"]["agents"] == [{"id": "cuisine", "name": "Cuisine"}]
    assert screen(home)["households"][0]["out"]["state"] == "proposed"
    # « D'accord »: both agree, Alex's agents write to Cuisine (through the place's door, directly)
    answered = home.post("/v1/approve", headers=AUTH, json={"agent": card["agent"], "run_id": card["run_id"], "choice": "once"})
    assert answered.status_code == 200, answered.text
    assert rule_cards(home) == []
    view = screen(home)
    assert view["households"][0]["out"]["state"] == "agreed"
    assert {("vie", "cuisine__chi"), ("budget", "cuisine__chi")} <= links(view)
    assert wait_until(lambda: screen(place)["households"][0]["in"]["state"] == "agreed", timeout=20)
    token = "vie-gateway-token-" + "y" * 20
    give_peer_token(home, "manual:vie", token)
    sent = home.post("/peer/p/cuisine__chi/v1/runs", headers={"Authorization": f"Bearer {token}"},
                     json={"session_id": "bot-1", "message": "Message from Vie (@vie): on mange quoi ?"})
    assert sent.status_code == 200 and sent.json() == {"ok": True, "path": "/p/cuisine/v1/runs"}

    # Chi proposes her agents write to Alex's Vie; he says « Non merci »: she sees it, nothing works that way
    place.put("/v1/agent-rules/households/alex", headers=AUTH, json={"way": "out", "mode": "only", "agents": ["vie"]})
    assert wait_until(lambda: rule_cards(home), timeout=20), home.log_tail()
    [card] = rule_cards(home)
    assert card["description"] == "Chi propose que ses agents écrivent à ton agent Vie"
    assert home.post("/v1/approve", headers=AUTH, json={"agent": card["agent"], "run_id": card["run_id"],
                                                        "choice": "deny"}).status_code == 200
    assert wait_until(lambda: screen(place)["households"][0]["out"]["state"] == "declined", timeout=20)
    cuisine = "cuisine-gateway-token-" + "z" * 20
    give_peer_token(place, "manual:cuisine", cuisine)
    refused = place.post("/peer/p/vie__alex/v1/runs", headers={"Authorization": f"Bearer {cuisine}"},
                         json={"session_id": "bot-1", "message": "Message from Cuisine (@cuisine): coucou"})
    assert refused.status_code == 403

    # a side not signed with the link's own key is refused
    key = households(home)["chi"]["key_out"]
    forged = place.post("/family/rules", headers={"Authorization": f"Bearer {key}"},
                        json={"rules": {"v": 1, "from_you": {"mode": "all"}, "to_you": {"mode": "all"}}})
    assert forged.status_code == 403, forged.text
    # unlinked: the rules go with the link
    assert place.delete("/v1/family/alex", headers=AUTH).status_code == 204
    assert screen(place)["households"] == []
