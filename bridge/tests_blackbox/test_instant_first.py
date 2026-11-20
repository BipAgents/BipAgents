"""The line said at once to a message that STARTS a turn (instant_reply.rs `first`, Alex 20/11 « dès le premier
message »): asked at once from the agent's own model, the turn started once it is there (or without it past 2.5 s), its
words in the turn's hidden note so the real reply continues from it; shown right under the message, before any word of
the turn's. None for « merci » / « ok » (no call), for a « - » from the model, for an app that does not say it draws it,
and none at all once the turn started without it."""
import threading
import time

from bbkit.common import run_has, sse
from bbkit.servers import wait_until
from test_instant_reply import MARK, instant, page, send, setup

LINE = "Je compare les trains Lyon–Paris de samedi matin, les moins chers d'abord."


def first_chats(fakes):
    return [c for c in fakes.model.chats if "Your full answer (with your tools" in c["body"]["messages"][0]["content"]]


def slow_turn(fakes, gate, run="r1", output="Voici trois trains."):
    fakes.hermes.runs[run] = [sse("run.started", run_id=run), sse("tool.started", run_id=run, tool="web_search"), gate,
                              sse("message.delta", run_id=run, delta=output), sse("run.completed", run_id=run, output=output)]


def test_a_slow_turn_shows_the_line_at_once_then_its_real_reply(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    slow_turn(fakes, gate)
    cid = "F1D00000-0000-4000-8000-0000000001A1"
    started = time.time()
    answer = send(bridge, thread, cid, "Trouve-moi un train Lyon Paris samedi matin pas trop cher", instant_first=True)
    assert answer["state"] == "started" and time.time() - started < 2.5
    # the turn got the words with the line in the note: it continues from it, never says it again
    words = fakes.hermes.started["r1"]["input"]
    words = words if isinstance(words, str) else words[0]["content"][0]["text"]
    assert "Trouve-moi un train Lyon Paris samedi matin pas trop cher\n\n" + MARK in words
    assert f"« {LINE} »" in words and "don't repeat" in words
    # the model was asked with the agent's key and name, the time, the conversation's last lines and the words
    chat = first_chats(fakes)[-1]
    assert chat["authorization"] == "Bearer sk-test-key" and "tools" not in chat["body"]
    system, prompt = chat["body"]["messages"][0]["content"], chat["body"]["messages"][1]["content"]
    assert system.startswith("You are Coach") and "It is now " in system and "single -" in system
    assert "Person: Bonjour" in prompt and prompt.endswith("Their new message: Trouve-moi un train Lyon Paris samedi matin pas trop cher")
    # shown at once right under the message, while the turn works
    line = instant(bridge, thread, cid)
    assert line and (line["role"], line["instant"], line["text"], line["answers"]) == ("agent", True, LINE, cid)
    ids = [i["id"] for i in page(bridge, thread)["items"]]
    assert ids[ids.index(f"s_{cid}") + 1] == f"i_{cid}"
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    # then the real reply, after it; the line never in Hermes' history, never the person's words
    gate.set()
    fakes.hermes.messages[("coach", "api_main")] += [{"role": "user", "content": words}, {"role": "assistant", "content": "Voici trois trains."}]
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    items = page(bridge, thread)["items"]
    person = next(i for i in items if i.get("client_id") == cid and i["id"] != f"i_{cid}")
    assert person["text"] == "Trouve-moi un train Lyon Paris samedi matin pas trop cher"
    order = [i["id"] for i in items]
    at = order.index(person["id"])
    assert order[at + 1] == f"i_{cid}" and items[-1]["text"] == "Voici trois trains."
    # the line is the agent's first word: no « Je lance… » of the app's on top of it
    assert not any("ack" in (i.get("layout") or {}) for i in items)
    assert not any(LINE in str(m["content"]) and m["role"] == "assistant" for m in fakes.hermes.messages[("coach", "api_main")])


def test_a_line_too_slow_is_never_shown_and_the_turn_starts_without_it(bridges, fakes):
    # « the real reply may start before the line »: past 2.5 s the turn starts as before, the line dropped for good
    bridge, thread = setup(bridges, fakes)
    fakes.model.first_delay = 4.0
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("message.delta", run_id="r1", delta="Voici."),
                               sse("run.completed", run_id="r1", output="Voici.")]
    cid = "F1D00000-0000-4000-8000-0000000001B1"
    started = time.time()
    answer = send(bridge, thread, cid, "Cherche un dentiste ouvert le samedi à Nantes", instant_first=True)
    took = time.time() - started
    assert answer["state"] == "started" and 2.2 < took < 3.6, took
    words = fakes.hermes.started["r1"]["input"]
    assert MARK not in str(words)
    time.sleep(3.5)   # the model's answer would have come by now
    assert instant(bridge, thread, cid) is None


def test_thanks_and_ok_get_no_line_and_no_call(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Avec plaisir !")]
    cid = "F1D00000-0000-4000-8000-0000000001C1"
    started = time.time()
    send(bridge, thread, cid, "Merci beaucoup !", instant_first=True)
    assert time.time() - started < 1.5, "nothing waited for"
    assert MARK not in str(fakes.hermes.started["r1"]["input"])
    time.sleep(1)
    assert instant(bridge, thread, cid) is None and not first_chats(fakes)


def test_a_short_reply_the_model_has_nothing_to_say_first_gets_no_line(bridges, fakes):
    # « - »: the message needs no words yet (a reaction): no line, no note
    bridge, thread = setup(bridges, fakes)
    fakes.model.first_answer = "-"
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Ravi que ça te plaise.")]
    cid = "F1D00000-0000-4000-8000-0000000001D1"
    send(bridge, thread, cid, "ah génial, ça me va très bien", instant_first=True)
    assert len(first_chats(fakes)) == 1
    assert MARK not in str(fakes.hermes.started["r1"]["input"])
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=10)
    assert instant(bridge, thread, cid) is None
    # and the conversation's slot is free again: a line for the next message
    fakes.model.first_answer = LINE
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="Voici.")]
    send(bridge, thread, "F1D00000-0000-4000-8000-0000000001D2", "Trouve-moi un train pour samedi", instant_first=True)
    assert wait_until(lambda: instant(bridge, thread, "F1D00000-0000-4000-8000-0000000001D2"), timeout=5)


def test_an_older_app_and_a_read_only_share_get_none(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Voici.")]
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="Voici.")]
    send(bridge, thread, "F1D00000-0000-4000-8000-0000000001E1", "Trouve-moi un train pour samedi")
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=10)
    send(bridge, thread, "F1D00000-0000-4000-8000-0000000001E2", "Et pour dimanche", instant_first=True,
         instructions="Answer from the shared conversation only.")
    time.sleep(1)
    assert not first_chats(fakes)
    assert instant(bridge, thread, "F1D00000-0000-4000-8000-0000000001E1") is None
    assert instant(bridge, thread, "F1D00000-0000-4000-8000-0000000001E2") is None
