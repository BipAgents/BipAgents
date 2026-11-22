"""« Mets à jour l'app » (BipAgents 1.1, bridge-rs/src/app_floor.rs): the oldest app builds a bridge works with
(`app_floor` in `GET /v1/status` and `GET /health`, for an app that sends `X-Bip-App`), the floor's next step turned
into the min by the server's own clock, and the 426 `app_too_old` of a path retired for old apps.

Contract: additive only. An app of before 1.1 sends no `X-Bip-App`: it reads exactly today's answers."""
import json
import os
import shlex
import time

import pytest

from bbkit.common import AUTH

IOS = {"X-Bip-App": "ios/202612011200/1.1"}
ANDROID = {"X-Bip-App": "android/263351200/1.1"}
FLOOR_FEATURE = "app_floor"
# what an app of before 1.1 reads, and what every app read before this change: the routes the home and the
# conversation list ask at each refresh
READ_ROUTES = ["/health", "/v1/status", "/v1/agents", "/v1/notifications", "/v1/approvals"]


def floored(bridges, floor):
    return bridges.new(change=lambda config: config.__setitem__("app_floor", floor)).start()


def status(bridge, headers=None):
    answer = bridge.get("/v1/status", headers={**AUTH, **(headers or {})})
    assert answer.status_code == 200, answer.text
    return answer.json()


def without_floor(body):
    body = dict(body)
    body.pop("app_floor", None)
    if isinstance(body.get("features"), list):
        body["features"] = [f for f in body["features"] if f != FLOOR_FEATURE]
    return body


def test_a_1_1_app_reads_the_floor_in_status_and_health(bridge):
    for app in (IOS, ANDROID):
        floor = status(bridge, app)["app_floor"]
        assert set(floor) == {"ios", "android"}
        for platform in ("ios", "android"):
            assert isinstance(floor[platform]["min"], int) and floor[platform]["min"] >= 0
            assert set(floor[platform]) in ({"min"}, {"min", "next", "next_from"})
        health = bridge.get("/health", headers=app).json()
        assert health["app_floor"] == floor
        assert health["features"][-1] == FLOOR_FEATURE and health["features"].count(FLOOR_FEATURE) == 1


def test_old_apps_read_exactly_todays_answers(bridges):
    plain = bridges.new("plain").start()
    raised = floored(bridges, {"ios": {"min": 209912311200, "next": 209912311300, "next_from": int(time.time()) + 90 * 86400},
                               "android": {"min": 999999999}})
    # today's answers, field by field
    health = plain.get("/health").json()
    assert FLOOR_FEATURE not in health["features"]   # (the list itself grows with the bridge: never written here)
    assert "app_floor" not in health
    assert set(status(plain)) == {"agents", "model", "protected", "jobs"}
    for path in ("/health", "/v1/status"):
        # a floor set on the server changes nothing for them, byte for byte
        headers = AUTH if path != "/health" else {}
        assert raised.get(path, headers=headers).content == plain.get(path, headers=headers).content, path
    for path in READ_ROUTES:
        headers = AUTH if path != "/health" else {}
        old = raised.get(path, headers=headers)
        assert old.status_code == 200, (path, old.text)
        assert "app_floor" not in old.json(), path
        # a malformed header is no header
        for bad in ("", "ios", "ios/x/1.1", "web/12/1.1", "ios/12/1.1/2", "IOS/12/1.1"):
            assert raised.get(path, headers={**headers, "X-Bip-App": bad}).content == old.content, (path, bad)
        # a 1.1 app reads the same answer, plus the floor
        new = raised.get(path, headers={**headers, **IOS}).json()
        assert without_floor(new) == old.json(), path
        if path in ("/health", "/v1/status"):
            assert new["app_floor"]["ios"]["min"] == 209912311200
            assert new["app_floor"]["android"] == {"min": 999999999}
        else:
            assert "app_floor" not in new, path


def test_the_next_floor_becomes_the_min_by_the_servers_clock(bridges):
    soon = int(time.time()) + 4
    later = int(time.time()) + 40 * 86400
    bridge = floored(bridges, {"ios": {"next": 202612011200, "next_from": soon},
                               "android": {"min": 5, "next": 263351200, "next_from": later}})
    before = status(bridge, IOS)["app_floor"]
    assert before["android"] == {"min": 5, "next": 263351200, "next_from": later}
    if time.time() < soon - 0.5:
        assert before["ios"] == {"min": 0, "next": 202612011200, "next_from": soon}
    time.sleep(max(0.0, soon - time.time()) + 1.0)
    # the same running bridge, its own clock past the date: the next is the min, and is no longer announced
    after = status(bridge, IOS)["app_floor"]
    assert after["ios"] == {"min": 202612011200}
    assert after["android"] == before["android"]
    assert bridge.get("/health", headers=ANDROID).json()["app_floor"] == after


def test_a_date_already_past_at_start_is_the_min(bridges):
    bridge = floored(bridges, {"android": {"min": 3, "next": 263351200, "next_from": int(time.time()) - 3600}})
    assert status(bridge, ANDROID)["app_floor"]["android"] == {"min": 263351200}


def test_a_lower_config_floor_never_lowers_the_codes(bridges):
    plain = bridges.new("plain").start()
    code = status(plain, IOS)["app_floor"]
    low = floored(bridges, {"ios": {"min": 1}, "android": {"min": 1}})
    floor = status(low, IOS)["app_floor"]
    assert floor["ios"]["min"] == max(code["ios"]["min"], 1)
    assert floor["android"]["min"] == max(code["android"]["min"], 1)


def test_a_retired_path_answers_426_app_too_old(bridges):
    bridge = floored(bridges, {"gone": ["/v1/retired-for-test", "/v1/agents/{agent}/retired"]})
    for method, path, headers in [("GET", "/v1/retired-for-test", AUTH), ("POST", "/v1/retired-for-test", AUTH),
                                  ("GET", "/v1/retired-for-test", {}), ("GET", "/v1/retired-for-test", {**AUTH, **IOS}),
                                  ("DELETE", "/v1/agents/wellness/retired", AUTH)]:
        answer = bridge.request(method, path, headers=headers)
        assert answer.status_code == 426, (method, path, answer.text)
        assert answer.json() == {"detail": "update the app", "code": "app_too_old"}
    for path in ("/v1/retired-for-tests", "/v1/agents/wellness/retired/x", "/v1/never-was"):
        answer = bridge.get(path, headers=AUTH)
        assert answer.status_code == 404, path
    # served routes are untouched
    assert bridge.get("/v1/agents", headers=AUTH).status_code == 200


@pytest.mark.parametrize("path", ["/v1/retired-for-test"])
def test_without_the_setting_a_missing_path_stays_a_404(bridge, path):
    answer = bridge.get(path, headers=AUTH)
    assert (answer.status_code, answer.json()) == (404, {"detail": "Not Found"})


@pytest.mark.skipif(not os.environ.get("BRIDGE_PREVIOUS_CMD"), reason="BRIDGE_PREVIOUS_CMD: the bridge before app_floor")
def test_old_apps_read_what_the_previous_bridge_said(bridges):
    """An installed 1.0 app (no `X-Bip-App`) against this bridge reads exactly what the previous release answered: the
    same JSON on the routes it asks at each refresh (`BRIDGE_PREVIOUS_CMD`: that release's `bipbridge serve`)."""
    new = bridges.new("new").start()
    old = bridges.new("old")
    old.command = shlex.split(os.environ["BRIDGE_PREVIOUS_CMD"])
    old.start()

    def answer(bridge, path):
        headers = AUTH if path != "/health" else {}
        got = bridge.get(path, headers=headers)
        assert got.status_code == 200, (path, got.text)
        return json.loads(got.text.replace(str(bridge.root), "<root>").replace(f":{bridge.port}", ":<port>"))

    for path in READ_ROUTES:
        assert answer(new, path) == answer(old, path), path
