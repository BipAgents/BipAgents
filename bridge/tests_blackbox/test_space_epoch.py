"""An agent's space whose Hermes refuses every turn (409 `stale_epoch`) is healed by the bridge (bridge-rs/src/
space_heal.rs, investigation of 24/11): its gateway restarted once, the turn tried again once. Agents moved at their
first secret had the hub's runtime epoch copied over their space's (spaces.rs `resync_database`) and answered every
message 502 until someone restarted their space's unit by hand. At most one restart per agent per 10 minutes; a hub
agent's gateway (every agent of the hub) is never restarted for it; a refusal that stays is the same error as before."""
import uuid

from starlette.responses import JSONResponse

from bbkit.common import AUTH

STALE = {"error": {"message": "stale_epoch", "type": "invalid_request_error", "param": None, "code": "stale_epoch"}}


def place(bridges, fakes, space=True):
    bridge = bridges.new()
    bridge.config["hermes"] = {"bin": str(bridge.fakebin / "hermes")}
    bridge.config["agent_restart"] = {"enabled": False}   # only this module's restart
    home = bridge.root / "spaces" / "wellness"
    if space:
        (home / "profiles" / "wellness").mkdir(parents=True)
        bridge.config["agents"]["Wellness"].update({"space": str(home), "isolated": True})
    return bridge, home


def restarts(bridge):
    return [e for e in bridge.commands.entries("hermes") if e["argv"][-2:] == ["gateway", "restart"]] + \
        [e for e in bridge.commands.entries("systemctl") if e["argv"][:2] == ["--user", "restart"]]


def stale_until_restarted(bridge, fakes, stale):
    """Hermes answers every run's start 409 stale_epoch while `stale["on"]`; a restart of its gateway clears it."""
    seen = {"restarts": 0}

    def hermes(request):
        if request.method == "POST" and request.path.rstrip("/").endswith("/v1/runs"):
            done = len(restarts(bridge))
            if done > seen["restarts"]:
                seen["restarts"], stale["on"] = done, False
            if stale["on"]:
                return JSONResponse(STALE, status_code=409)
        return None

    fakes.hermes.override = hermes


def send(bridge, text):
    thread = bridge.get("/v1/threads/resolve", headers=AUTH,
                        params={"agent": "wellness", "session_id": "api_e"}).json()["thread"]["id"]
    return bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": str(uuid.uuid4()), "text": text}, timeout=120)


def test_a_space_refusing_every_turn_is_restarted_once_and_the_message_goes_through(bridges, fakes):
    bridge, home = place(bridges, fakes)
    stale = {"on": True}
    stale_until_restarted(bridge, fakes, stale)
    bridge.start()
    answer = send(bridge, "Tu es là ?")
    assert answer.status_code == 200, answer.text
    assert answer.json()["run_id"], answer.json()
    assert any("Tu es là ?" in str(r["input"]) for r in fakes.hermes.started.values())
    done = restarts(bridge)
    assert len(done) == 1, done
    assert done[0]["hermes_home"] == str(home)   # its space's gateway, not the hub's
    # stale again within the 10 minutes: no second restart, the error as before
    stale["on"] = True
    again = send(bridge, "Et maintenant ?")
    assert again.status_code == 502, again.text
    assert len(restarts(bridge)) == 1


def test_a_hub_agent_is_never_restarted_for_it(bridges, fakes):
    bridge, _ = place(bridges, fakes, space=False)
    stale_until_restarted(bridge, fakes, {"on": True})
    bridge.start()
    answer = send(bridge, "Tu es là ?")
    assert answer.status_code == 502, answer.text
    assert restarts(bridge) == []
