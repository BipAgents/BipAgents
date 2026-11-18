"""Kerfy Studio's remote switch for the agents' card payments on the offer's places (decided 18/11): `bipguichet cards
off` reaches each place's seat agent with the beat, which writes `{"offer_cards": false}` into the place's own file
(root's, outside its home: /var/lib/bipagents/offer-cards/<uid>.json, here `BIP_TEST_OFFER_SWITCH`). The bridge reads
it live: off wins over bridge.toml and over the person's switch (kept as they set it), no new payment is offered or
approved, a payment waiting for the person or already typed is stopped within seconds, and the agent and the person
are told in plain words. The file gone: payments as before, without a restart."""
import json
import re
import threading
import time

import httpx

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_card_payments import (ARGS, FROM_PHONE_A, NUMBER, PAY, PHONE_CARD, TOKEN, FakeChrome, pay_bridge, pay_from_phone,
                                tool, turn_on)

NOTICE = "Les paiements par carte sont suspendus pour le moment."
SAID = "Les paiements par carte sont suspendus pour le moment"   # what the agent is told to tell the person


def offer_place(bridges, fakes, chrome, tmp_path):
    switch = tmp_path / "offer-cards.json"

    def prepare(bridge):
        bridge.env_extra["BIP_TEST_OFFER_SWITCH"] = str(switch)

    bridge = pay_bridge(bridges, fakes, chrome, place="place", prepare=prepare,
                        change=lambda c: c.update({"payments": {"offer_cards": True}}))
    turn_on(bridge)
    made = bridge.post("/v1/vault", headers=AUTH, json=PHONE_CARD)
    assert made.status_code == 201, made.text
    return bridge, switch


def cut(switch, off=True):
    """As the seat agent writes it: a new file each time."""
    tmp = switch.with_suffix(".tmp")
    tmp.write_text(json.dumps({"offer_cards": not off}))
    tmp.replace(switch)


def overview(bridge):
    return bridge.get("/v1/vault/payments", headers=AUTH).json()


def test_kerfy_s_cut_wins_live_then_lifts_without_a_restart(bridges, fakes, tmp_path):
    chrome = FakeChrome()
    try:
        bridge, switch = offer_place(bridges, fakes, chrome, tmp_path)
        assert overview(bridge)["enabled"] is True and "suspended" not in overview(bridge)
        cut(switch)
        seen = overview(bridge)
        assert seen["enabled"] is False and seen["allowed"] is True   # the payments' page stays, its switch off
        assert seen["suspended"] is True and seen["notice"] == NOTICE
        listing = bridge.get("/v1/vault", headers=AUTH).json()
        assert listing["cards"] is False and listing["cards_suspended"] is True and listing["cards_notice"] == NOTICE
        # The person can't turn it back on meanwhile; their own switch is kept as they set it.
        again = bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True})
        assert again.status_code == 409 and again.json()["detail"] == {"code": "cards_suspended", "message": NOTICE}
        # The agent: no card listed, no payment asked, in plain words for the person.
        listed = tool(bridge, "list_cards")
        assert SAID in listed["content"][0]["text"]
        refused = tool(bridge, "pay_with_card", ARGS)
        assert refused["isError"] is True and SAID in refused["content"][0]["text"]
        assert chrome.typed == []
        # {"offer_cards": true} is no cut; the file gone neither: as before, no restart.
        cut(switch, off=False)
        assert overview(bridge)["enabled"] is True
        cut(switch)
        assert overview(bridge)["enabled"] is False
        switch.unlink()
        back = overview(bridge)
        assert back["enabled"] is True and "suspended" not in back
        result, _, answers = pay_from_phone(bridge, ARGS, [FROM_PHONE_A])
        assert answers == [(204, "")] and result["isError"] is False, result
        assert ("F-psp", "number", NUMBER) in chrome.typed
    finally:
        chrome.stop()


def test_a_payment_waiting_for_the_person_is_refused_once_cut(bridges, fakes, tmp_path):
    chrome = FakeChrome()
    try:
        bridge, switch = offer_place(bridges, fakes, chrome, tmp_path)
        body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "pay_with_card", "arguments": ARGS}}
        headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"}
        approval, result = None, None
        with httpx.Client(timeout=90) as client, client.stream("POST", bridge.url + PAY, json=body, headers=headers) as resp:
            for line in resp.iter_lines():
                if not line.startswith("data: "):
                    continue
                message = json.loads(line[6:])
                if message.get("method") == "elicitation/create":
                    payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", message["params"]["message"]).group(1)
                    cut(switch)
                    # the bridge's watcher (every 5 s) refuses what waits; an approval sent meanwhile is refused too
                    approval = bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json=FROM_PHONE_A)
                    answer = {"jsonrpc": "2.0", "id": message["id"], "result": {"action": "accept", "content": {}}}
                    assert client.post(bridge.url + PAY, json=answer, headers=headers).status_code == 202
                elif message.get("id") == 7:
                    result = message["result"]
                    break
        assert approval.status_code == 409, approval.text
        assert result["isError"] is True and SAID in result["content"][0]["text"], result
        assert chrome.typed == []
        assert overview(bridge)["journal"][0]["outcome"] == "suspended"
    finally:
        chrome.stop()


def test_a_payment_typed_but_not_paid_is_stopped_and_erased_once_cut(bridges, fakes, tmp_path):
    chrome = FakeChrome()
    try:
        bridge, switch = offer_place(bridges, fakes, chrome, tmp_path)
        result, _, answers = pay_from_phone(bridge, ARGS, [FROM_PHONE_A])
        assert answers == [(204, "")] and result["isError"] is False, result
        assert chrome.values[("F-psp", 0)] == NUMBER and overview(bridge)["active"] != []
        started = time.monotonic()
        cut(switch)
        # within the watcher's 5 s (and the release's own steps): given back, the card's fields emptied (the release
        # notes « suspended » in the journal once done)
        assert wait_until(lambda: overview(bridge)["active"] == [], timeout=20)
        assert wait_until(lambda: any(e["outcome"] == "suspended" for e in overview(bridge)["journal"][:3]), timeout=20)
        assert time.monotonic() - started < 20
        assert chrome.values[("F-psp", 0)] == "" and chrome.values[("F-psp", 2)] == ""
        assert chrome.navigated == ["about:blank"]
        assert chrome.clicks == []
        journal = overview(bridge)["journal"]
        assert any(e["outcome"] == "payments_suspended" and e.get("by") == "kerfy" for e in journal[:3]), journal[:3]
        assert any(e["outcome"] == "suspended" for e in journal[:3]), journal[:3]
        confirm = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})
        assert "No payment is waiting" in confirm["content"][0]["text"]
        assert chrome.clicks == []
    finally:
        chrome.stop()


def test_a_payment_typed_then_confirmed_before_the_watcher_looks_is_never_pressed(bridges, fakes, tmp_path):
    chrome = FakeChrome()
    try:
        bridge, switch = offer_place(bridges, fakes, chrome, tmp_path)
        result, _, _ = pay_from_phone(bridge, ARGS, [FROM_PHONE_A])
        assert result["isError"] is False, result
        cut(switch)
        confirm = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})   # read live, before the watcher
        assert confirm["isError"] is True and SAID in confirm["content"][0]["text"], confirm
        assert chrome.clicks == [] and chrome.values[("F-psp", 0)] == ""
    finally:
        chrome.stop()
