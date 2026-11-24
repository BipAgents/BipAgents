"""Finitions 1.1 (passation-pro briefs: review-plans-cards.md points 2 and 5, review-subtasks-five.md A1): the top line
of the plan and offer cards comes from the bridge in the agent's language (`bip_plan.top`, `bip_offer.top`); « Prévoir »
answers how many moments were set and passed (`moments`, `of`, `passed`: the apps read them); a message shown « Pas
envoyé » can be deleted for good (`DELETE /v1/threads/{thread}/sends/{client_id}`), never one a turn took. Additions
only: an app 1.0 ignores the keys and never calls the route."""
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until
from test_offers import TEXTS as OFFER_TEXTS, make, offer_cards, search
from test_offers import lang_of as offer_lang
from test_plans import TEXTS as PLAN_TEXTS, answer, lang_of, place, plan_cards, propose, trip
from test_threads_send import page, send, setup, stored, user, agent as said


def test_f1_plan_and_offer_cards_carry_their_top_line_in_the_agent_s_language(bridges, fakes):
    bridge = place(bridges, fakes)
    propose(bridge, trip())
    [card] = plan_cards(bridge)
    texts = PLAN_TEXTS[lang_of(card)]
    assert card["bip_plan"]["top"] == texts["top"].format(agent="Coach", title="Lisbonne"), card["bip_plan"]
    # « Prévoir »: the answer says what was set (« un était passé » is read from it, never counted on the phone)
    done = answer(bridge, card, "once", skip=[card["bip_plan"]["moments"][0]["id"]])
    assert done.status_code == 200 and (done.json()["moments"], done.json()["of"], done.json()["passed"]) == (2, 3, 0)


def test_f1_an_offer_card_carries_its_top_line_in_the_agent_s_language(bridges, fakes):
    bridge = place(bridges, fakes)
    make(bridge, search())
    [offer] = offer_cards(bridge)
    assert offer["bip_offer"]["top"] == OFFER_TEXTS[offer_lang(offer)]["top"].format(agent="Coach", about="Lisbonne")


def test_f1_a_habit_s_top_line_says_a_habit(bridges, fakes):
    bridge = place(bridges, fakes)
    propose(bridge, {"kind": "habit", "title": "Boire plus d’eau",
                     "moments": [{"what": "Un verre d’eau", "say": "Un verre d’eau ?"}],
                     "repeat": {"days": "weekdays", "times": ["10:00", "15:00"], "weeks": 2}})
    [card] = plan_cards(bridge)
    texts = PLAN_TEXTS[lang_of(card)]
    assert card["bip_plan"]["top"] == texts["top_habit"].format(agent="Coach", title="Boire plus d’eau")


def forget(bridge, thread, client_id, agent="wellness"):
    return bridge.request("DELETE", f"/v1/threads/{thread}/sends/{client_id}", headers=AUTH, params={"agent": agent})


def test_f6_a_message_not_sent_is_deleted_for_good_and_nothing_else_ever_is(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.cancelled", run_id="r1")]
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="Pas encore.")]
    first, hello = "C1D00000-0000-4000-8000-0000000000F1", "C1D00000-0000-4000-8000-0000000000F2"
    assert send(bridge, thread, first, "Range le carnet").json()["state"] == "started"
    assert send(bridge, thread, hello, "Hello?").json()["state"] == "steered"
    # while its turn goes on, it is being sent: refused
    assert forget(bridge, thread, hello).status_code == 409
    assert bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={}).json()["stopped"] is True
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    stored(fakes, user("Range le carnet"))
    time.sleep(11)   # a while after it was sent, nothing running: « Pas envoyé »
    assert [i.get("not_sent") for i in page(bridge, thread)["items"] if i["id"] == f"s_{hello}"] == [True]
    # a message a turn took (Hermes stored its row) is never deleted, nor its reply
    assert forget(bridge, thread, first).status_code == 409
    # unknown here: 404 (another id, another conversation, a bad id 422)
    assert forget(bridge, thread, "C1D00000-0000-4000-8000-0000000000FF").status_code == 404
    assert forget(bridge, "main-of-nobody", hello).status_code == 404
    assert forget(bridge, thread, "x").status_code == 422
    version = bridge.get(f"/v1/threads/{thread}", headers=AUTH, params={"agent": "wellness"}).json().get("version")
    gone = forget(bridge, thread, hello)
    assert gone.status_code == 200 and gone.json() == {"client_id": hello, "thread": thread, "forgot": True}, gone.text
    items = page(bridge, thread)["items"]
    assert not [i for i in items if i["id"] == f"s_{hello}"], "still shown"
    assert [i["text"] for i in items if i.get("role") == "person"] == ["Bonjour", "Range le carnet"]
    if version is not None:   # the phones told (a `thread` hint)
        assert bridge.get(f"/v1/threads/{thread}", headers=AUTH, params={"agent": "wellness"}).json()["version"] > version
    # a second time: nothing there any more
    assert forget(bridge, thread, hello).status_code == 404
    # sent again under its id later (a phone that never knew): it goes for real, as a new message
    assert send(bridge, thread, hello, "Hello?").json()["state"] == "started"
    stored(fakes, user("Hello?"), said("Pas encore."))
    assert wait_until(lambda: page(bridge, thread)["items"][-1]["text"] == "Pas encore.", timeout=15)
