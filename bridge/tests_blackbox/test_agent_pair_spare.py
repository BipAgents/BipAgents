"""Bip and an agent it made from a spare (27/11, Alex's place of the offer, 1.1 final): « Chasseur Mac », made by Bip's
card and launched, took a spare agent (agent_pool.rs: its id stays `agent_<hex>`, its name is « Chasseur Mac »). With
the links both ways, Chasseur Mac reached Bip, but Bip's message to Chasseur Mac came back « couldn't answer »
(agent_pairs.rs `Answer::Unreachable`)."""
import json
import os

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_agent_create import GUIDE_TOKEN, next_run
from test_cloud_setup import claim_body, cloud_bridge

CHASSEUR_TOKEN = "chasseur-token-0123456789abcdefghijkl"


def made_by_bip(bridges, fakes):
    """A place of the offer with spares; Bip (the guide) makes « Chasseur Mac » from its card, launched."""
    bridge, cloud = cloud_bridge(bridges, fakes, start=False)
    bridge.config["place"] = {"spare_agents": 2}
    cloud.done()
    bridge.start()
    pool = bridge.root / "spare-agents.json"
    spares = lambda: [s["id"] for s in json.loads(pool.read_text())] if pool.exists() else []   # noqa: E731
    assert wait_until(lambda: len(spares()) == 2, timeout=30)
    claimed = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge()))
    assert claimed.status_code == 200, claimed.text
    bip = claimed.json()["agents"][0]["id"]
    assert bridge.get("/v1/guide", headers=AUTH).json()["guide"] == bip
    assert wait_until(lambda: len(spares()) == 2, timeout=30)
    ready = spares()
    give_token(bridge, bip, GUIDE_TOKEN)
    made = call(bridge, "propose_agent", {"kind": "daily", "name": "Chasseur Mac", "for": "me",
                                          "role": "Me trouver un Mac reconditionné sous 900 €",
                                          "first": "Chercher les offres du jour"}, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    card = next(c for c in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if c.get("name") == "Chasseur Mac")
    next_run(fakes)
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201 and accepted.json()["launched"] is True, accepted.text
    chasseur = accepted.json()["agent"]["id"]
    assert chasseur in ready, (chasseur, ready)   # a spare's id, « Chasseur Mac » its name
    # its launch turn over
    assert wait_until(lambda: any(r["agent"] == chasseur for r in fakes.hermes.started.values()), timeout=30)
    give_token(bridge, chasseur, CHASSEUR_TOKEN)
    # « Qui peut écrire à qui » as the app sets it: neither agent « à l'écart » (an outside model's default), all write
    for agent in (bip, chasseur):
        assert bridge.put("/v1/agent-rules/aside", headers=AUTH, json={"agent": agent, "on": False}).status_code == 200
    screen = bridge.put("/v1/agent-rules/write", headers=AUTH, json={"mode": "all"})
    assert screen.status_code == 200, screen.text
    give_token(bridge, bip, GUIDE_TOKEN)   # registered again (its tools changed)
    give_token(bridge, chasseur, CHASSEUR_TOKEN)
    return bridge, bip, chasseur


def test_bip_and_the_agent_it_made_from_a_spare_reach_each_other(bridges, fakes):
    bridge, bip, chasseur = made_by_bip(bridges, fakes)
    seen = {}
    fakes.hermes.run_output = "Trois offres sous 900 €."
    for target in ("Chasseur Mac", f"bip/{chasseur}", "chasseur-mac"):
        result = call(bridge, "message_agent", {"target": target, "message": "Tu as trouvé quoi ?"}, token=GUIDE_TOKEN)
        seen[f"Bip -> {target}"] = text_of(result)
    fakes.hermes.run_output = "Le budget est de 900 €."
    result = call(bridge, "message_agent", {"target": f"bip/{bip}", "message": "Quel budget ?"}, token=CHASSEUR_TOKEN)
    seen["Chasseur Mac -> Bip"] = text_of(result)
    pairs = bridge.get("/v1/agent-pairs", params={"agent": chasseur}, headers=AUTH).json()["pairs"]
    thread = bridge.get(f"/v1/agent-pairs/{pairs[0]['id']}", headers=AUTH).json()["thread"] if pairs else []
    out = os.environ.get("PAIR_REPORT")
    if out:
        with open(out, "w") as f:
            json.dump({"bip": bip, "chasseur": chasseur, "seen": seen,
                       "thread": [(m["from"], m["to"], m["kind"], m["text"]) for m in thread]}, f, ensure_ascii=False,
                      indent=1)
    assert "answered:\nTrois offres" in seen["Bip -> Chasseur Mac"], seen
    assert "answered:\nTrois offres" in seen[f"Bip -> bip/{chasseur}"], seen
    assert "answered:\nLe budget" in seen["Chasseur Mac -> Bip"], seen
