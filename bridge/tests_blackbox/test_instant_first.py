"""The line said at once to a message that STARTS a turn (instant_reply.rs `first`, Alex 20/11 « dès le premier
message », design « en parallèle »): the turn starts at once with a standing note (« a short line is being shown… go
straight to work »), the line is asked of the agent's own model beside it and shown only if it comes before the turn's
first words; its words are then steered into the turn's next step (a new message: the turn's prompt cache stays). None
for « merci » / « ok » (no call), for a « - » from the model, for an app that does not say it draws it."""
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


STANDING = "<!--bip:answered The app is showing the person a short line"


def words_of(fakes, run):
    words = fakes.hermes.started[run]["input"]
    return words if isinstance(words, str) else words[0]["content"][0]["text"]


def test_a_slow_turn_starts_at_once_shows_the_line_then_its_real_reply(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    slow_turn(fakes, gate)
    fakes.model.first_delay = 0.5
    cid = "F1D00000-0000-4000-8000-0000000001A1"
    started = time.time()
    answer = send(bridge, thread, cid, "Trouve-moi un train Lyon Paris samedi matin pas trop cher", instant_first=True)
    assert answer["state"] == "started" and time.time() - started < 0.45, "the turn never waits for the line"
    # the turn got the words and the standing note (no line yet)
    words = words_of(fakes, "r1")
    assert words.startswith("Trouve-moi un train Lyon Paris samedi matin pas trop cher\n\n" + STANDING)
    assert "call your first tool directly" in words and LINE not in words
    # the model was asked with the agent's key and name, the time, the conversation's last lines and the words
    chat = first_chats(fakes)[-1]
    assert chat["authorization"] == "Bearer sk-test-key" and "tools" not in chat["body"]
    system, prompt = chat["body"]["messages"][0]["content"], chat["body"]["messages"][1]["content"]
    assert system.startswith("You are Coach") and "It is now " in system and "single -" in system
    assert "Person: Bonjour" in prompt and prompt.endswith("Their new message: Trouve-moi un train Lyon Paris samedi matin pas trop cher")
    # the turn said nothing yet (a search going on): the line is shown under the message, its words steered in
    assert wait_until(lambda: instant(bridge, thread, cid), timeout=5)
    line = instant(bridge, thread, cid)
    assert (line["role"], line["instant"], line["text"], line["answers"]) == ("agent", True, LINE, cid)
    ids = [i["id"] for i in page(bridge, thread)["items"]]
    assert ids[ids.index(f"s_{cid}") + 1] == f"i_{cid}"
    assert wait_until(lambda: any(LINE in str(s["body"].get("input")) for s in fakes.hermes.steers), timeout=5)
    steer = next(s for s in fakes.hermes.steers if LINE in str(s["body"].get("input")))["body"]["input"]
    assert steer.startswith(MARK) and f"« {LINE} »" in steer and "don't repeat" in steer
    # then the real reply, after it; the line never in Hermes' history as the agent's, never the person's words
    gate.set()
    fakes.hermes.messages[("coach", "api_main")] += [{"role": "user", "content": words}, {"role": "assistant", "content": "Voici trois trains."}]
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    items = page(bridge, thread)["items"]
    person = next(i for i in items if i.get("client_id") == cid and i["id"] != f"i_{cid}")
    assert person["text"] == "Trouve-moi un train Lyon Paris samedi matin pas trop cher"
    order = [i["id"] for i in items]
    assert order[order.index(person["id"]) + 1] == f"i_{cid}" and items[-1]["text"] == "Voici trois trains."
    # the line is the agent's first word: no « Je lance… » of the app's on top of it
    assert not any("ack" in (i.get("layout") or {}) for i in items)


def test_a_reply_that_started_before_the_line_gets_none(bridges, fakes):
    # the turn's first words come before the line: never both, the line dropped for good, nothing steered
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.model.first_delay = 1.5
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("message.delta", run_id="r1", delta="Je regarde ça."),
                               gate, sse("run.completed", run_id="r1", output="Je regarde ça. Voici.")]
    cid = "F1D00000-0000-4000-8000-0000000001B1"
    send(bridge, thread, cid, "Cherche un dentiste ouvert le samedi à Nantes", instant_first=True)
    assert STANDING in words_of(fakes, "r1")
    assert wait_until(lambda: first_chats(fakes), timeout=3)
    time.sleep(2.5)   # the line has come by now
    assert instant(bridge, thread, cid) is None
    assert not any(MARK in str(s["body"].get("input")) for s in fakes.hermes.steers)
    gate.set()


def test_thanks_ok_and_a_short_question_get_no_line_and_no_call(bridges, fakes):
    # instant_reply::wants_line: « merci », « ok »…; a short question with no verb of work where turns are fast or
    # not measured yet: no line asked at all
    bridge, thread = setup(bridges, fakes)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Avec plaisir !")]
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="43,2.")]
    cid = "F1D00000-0000-4000-8000-0000000001C1"
    started = time.time()
    send(bridge, thread, cid, "Merci beaucoup !", instant_first=True)
    assert time.time() - started < 1.5, "nothing waited for"
    assert MARK not in str(fakes.hermes.started["r1"]["input"])
    time.sleep(1)
    assert instant(bridge, thread, cid) is None and not first_chats(fakes)
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=10)
    send(bridge, thread, "F1D00000-0000-4000-8000-0000000001C2", "Combien font 18 % de 240 ?", instant_first=True)
    assert "bip:answered" not in str(fakes.hermes.started["r2"]["input"])
    time.sleep(1)
    assert not first_chats(fakes)


def test_a_short_reply_the_model_has_nothing_to_say_first_gets_no_line(bridges, fakes):
    # « - »: the model finds nothing to say first: no line, nothing steered
    bridge, thread = setup(bridges, fakes)
    fakes.model.first_answer = "-"
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Ravi que ça te plaise.")]
    cid = "F1D00000-0000-4000-8000-0000000001D1"
    send(bridge, thread, cid, "Regarde quand tu veux, rien ne presse", instant_first=True)
    assert wait_until(lambda: len(first_chats(fakes)) == 1, timeout=3)
    time.sleep(0.5)
    assert not any(MARK in str(s["body"].get("input")) for s in fakes.hermes.steers)
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=10)
    assert instant(bridge, thread, cid) is None
    # and the conversation's slot is free again: a line for the next message (its turn still working)
    fakes.model.first_answer = LINE
    gate = threading.Event()
    slow_turn(fakes, gate, run="r2")
    send(bridge, thread, "F1D00000-0000-4000-8000-0000000001D2", "Trouve-moi un train pour samedi", instant_first=True)
    assert wait_until(lambda: instant(bridge, thread, "F1D00000-0000-4000-8000-0000000001D2"), timeout=5)
    gate.set()


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


def test_a_second_message_during_the_wait_never_starts_a_turn_of_its_own(bridges, fakes):
    # Relecture 1 (20/11): « Trouve-moi un train Lyon Paris » then « samedi matin » a second later — the usual way to
    # write. The first send waits up to 2.5 s for its line before its turn starts; the second one sees no turn going on
    # and nothing queued (and gets no line: the 20 s slot is taken), so it started its own turn at once, before the
    # first: two turns in the same conversation, the second message answered first.
    bridge, thread = setup(bridges, fakes)
    fakes.model.first_delay = 2.0
    gate = threading.Event()
    slow_turn(fakes, gate, run="r1")
    slow_turn(fakes, gate, run="r2", output="Samedi matin.")
    answers = {}
    first = threading.Thread(target=lambda: answers.__setitem__("first", send(
        bridge, thread, "F1D00000-0000-4000-8000-0000000001F1", "Trouve-moi un train Lyon Paris pas trop cher",
        instant_first=True)))
    first.start()
    time.sleep(0.6)
    answers["second"] = send(bridge, thread, "F1D00000-0000-4000-8000-0000000001F2", "samedi matin de préférence",
                             instant_first=True)
    first.join(timeout=10)
    gate.set()
    states = sorted(a["state"] for a in answers.values())
    assert states.count("started") == 1, (states, list(fakes.hermes.started))
    # and the first message's turn is the one that started first
    first_run = fakes.hermes.started[min(fakes.hermes.started)]["input"]
    assert "Trouve-moi un train Lyon Paris" in str(first_run), first_run


# Alex, 27/11 15:53 (Android): a question about the agent itself, and the line its small model guessed (« no tool to
# contact another agent »: false, the agent has one)
ABOUT_ITSELF = "Tes sûr que tu contactes le bon ? Son nom c'est Chasseur Mac mais son identifiant est agent_a8c8e457"
GUESSED = ("Je n'ai aucun outil pour contacter un autre agent, quel que soit son identifiant — je ne peux dialoguer "
           "qu'avec toi, ici. Si tu veux me faire passer un message à Chasseur Mac, il faudra le copier-coller toi-même "
           "de son côté.")


def test_a_question_about_the_agent_itself_gets_no_line(bridges, fakes):
    # fix-instant-capabilities: the line's model knows none of the agent's tools, accounts or other agents. Its
    # instructions say « - » for a message about the agent or questioning it, and a line about the agent's own tools,
    # access or limits is never shown, whatever the model says: nothing shown, nothing steered into the turn
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    slow_turn(fakes, gate, output="Oui : je lui écris avec son identifiant agent_a8c8e457.")
    fakes.model.first_answer = GUESSED
    cid = "F1D00000-0000-4000-8000-0000000002A1"
    send(bridge, thread, cid, ABOUT_ITSELF, instant_first=True)
    assert wait_until(lambda: len(first_chats(fakes)) == 1, timeout=3)
    time.sleep(1.0)
    assert instant(bridge, thread, cid) is None, instant(bridge, thread, cid)
    assert not any(MARK in str(s["body"].get("input")) for s in fakes.hermes.steers)
    system = first_chats(fakes)[-1]["body"]["messages"][0]["content"]
    assert "Never speak about yourself" in system and "questions or challenges something you did or said" in system
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    assert instant(bridge, thread, cid) is None
