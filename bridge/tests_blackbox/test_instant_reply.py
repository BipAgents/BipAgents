"""The agent's line said at once to a message sent while it works (instant_reply.rs, Alex 19/11 « comme Grok Bot »): one
short sentence from the agent's own model, shown as the agent's line right after the person's message, the turn told
it was answered already (a hidden `<!--bip:answered` note joined to its words); a sentence of the bridge's table
when the model can't answer in time; one per conversation every 20 s; never for a turn restarted with the words."""
import threading
import time

from bbkit.common import AUTH, run_has, sse
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

SESSION = "api_main"
MARK = "<!--bip:answered"


def user(text):
    return {"role": "user", "content": text}


def agent(text):
    return {"role": "assistant", "content": text}


def setup(bridges, fakes, model=True, name="coach"):
    fakes.hermes.v0225 = True
    fakes.hermes.rounds = 1000
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=(name,))
    folder = profile(home, name)
    if model:
        (folder / "config.yaml").write_text(f"model:\n  provider: custom\n  default: glm-5.3-flash\n"
                                            f"  base_url: {fakes.model.url}/v1\n  key_env: CUSTOM_MODEL_API_KEY\n")
        (folder / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-test-key\n")
    fakes.hermes.add_session(SESSION, [user("Bonjour"), agent("Salut")], title="Bot Chat", last_active=10,
                             started_at=1, agent=name)
    bridge.start()
    wait_registered(bridge, [name])
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": name}).json()
    return bridge, main["id"]


def send(bridge, thread, client_id, text="", agent_name="coach", **extra):
    body = {"agent": agent_name, "client_id": client_id, "text": text, **extra}
    answer = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH, json=body)
    assert answer.status_code == 200, answer.text
    return answer.json()


def page(bridge, thread, agent_name="coach"):
    answer = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": agent_name})
    assert answer.status_code == 200, answer.text
    return answer.json()


def instant(bridge, thread, client_id):
    return next((i for i in page(bridge, thread)["items"] if i["id"] == f"i_{client_id}"), None)


def busy_turn(fakes, gate, run="r1"):
    # a turn that acted already (a page made): words go in at its next step, never restarted
    fakes.hermes.runs[run] = [sse("run.started", run_id=run), sse("tool.started", run_id=run, tool="create_page"),
                              gate, sse("run.completed", run_id=run, output="Fini.")]


def test_words_steered_into_a_turn_get_the_agents_sentence_at_once_and_the_turn_is_told(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    busy_turn(fakes, gate)
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001A1", "Trouve-moi un dentiste à Lyon")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    started = time.time()
    steered = send(bridge, thread, "C1D00000-0000-4000-8000-0000000001A2", "plus de langue ni de dents, le samedi")
    assert steered["state"] == "steered" and time.time() - started < 1.0, "the send never waits for the sentence"
    # the turn gets the words and the note: answered already, no acknowledgement again
    steer = fakes.hermes.steers[-1]["body"]["input"]
    assert steer.startswith("plus de langue ni de dents, le samedi\n\n" + MARK)
    assert "Don't acknowledge" in steer
    assert wait_until(lambda: instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001A2"), timeout=5)
    line = instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001A2")
    assert (line["kind"], line["role"], line["instant"], line["text"]) == (
        "message", "agent", True, "Compris, plus de langue ni de dents, je l'intègre.")
    # right after the person's message it answers (still being sent: Hermes has not taken it yet)
    ids = [i["id"] for i in page(bridge, thread)["items"]]
    assert ids[-2:] == ["s_C1D00000-0000-4000-8000-0000000001A2", "i_C1D00000-0000-4000-8000-0000000001A2"]
    # the model was asked with the agent's key, its name, the last lines and the new words, few tokens, no tools
    chat = next(c for c in fakes.model.chats if "while you work" in c["body"]["messages"][0]["content"])
    assert chat["authorization"] == "Bearer sk-test-key"
    assert "tools" not in chat["body"] and "Coach" in chat["body"]["messages"][0]["content"]
    prompt = chat["body"]["messages"][1]["content"]
    assert "Person: Bonjour" in prompt and prompt.endswith("Their new message: plus de langue ni de dents, le samedi")
    # Hermes stores the steered words (with the note): the person's row never shows it, the sentence follows that row
    gate.set()
    fakes.hermes.messages[("coach", SESSION)] += [user("Trouve-moi un dentiste à Lyon"), user(steer), agent("Fini.")]
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    items = page(bridge, thread)["items"]
    texts = [(i["id"][:2], i.get("role"), i.get("text")) for i in items if i["kind"] == "message"]
    person = next(i for i in items if i.get("client_id") == "C1D00000-0000-4000-8000-0000000001A2")
    assert person["text"] == "plus de langue ni de dents, le samedi" and MARK not in person["text"]
    at = [i["id"] for i in items].index(person["id"])
    assert items[at + 1]["id"] == "i_C1D00000-0000-4000-8000-0000000001A2"
    assert texts[-1] == ("m_", "agent", "Fini."), texts
    # never in Hermes' history: only the bridge keeps it
    assert not any("je l'intègre" in str(m["content"]) for m in fakes.hermes.messages[("coach", SESSION)])


def test_a_model_too_slow_gets_a_sentence_of_the_table(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    fakes.model.instant_delay = 5.0
    gate = threading.Event()
    busy_turn(fakes, gate)
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001B1", "Compare trois écoles")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001B2", "seulement les publiques")
    started = time.time()
    assert wait_until(lambda: instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001B2"), timeout=6)
    assert time.time() - started < 4.5, "the model is given 3 s"
    said = instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001B2")["text"]
    assert said in ["Noted, I'm taking that into account.", "Got it, I'm adding that in.", "Noted, I'll factor that in.",
                    "Noté, j'en tiens compte.", "Bien reçu, je l'intègre.", "C'est noté, je le prends en compte."], said
    gate.set()


def test_one_sentence_per_conversation_every_twenty_seconds(bridges, fakes):
    bridge, thread = setup(bridges, fakes, model=False)
    gate = threading.Event()
    busy_turn(fakes, gate)
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001C1", "Un hôtel à Porto")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001C2", "près de la mer")
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001C3", "avec piscine")
    assert wait_until(lambda: instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001C2"), timeout=5)
    time.sleep(1)
    assert instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001C3") is None
    # the second has no note either: the turn takes it as before
    assert MARK in fakes.hermes.steers[-2]["body"]["input"]
    assert fakes.hermes.steers[-1]["body"]["input"] == "avec piscine"
    gate.set()


def test_a_message_waiting_behind_the_turn_is_answered_right_after(bridges, fakes):
    bridge, thread = setup(bridges, fakes, model=False)
    gate = threading.Event()
    busy_turn(fakes, gate)
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="La photo.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001D1", "Range mes photos")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    queued = send(bridge, thread, "C1D00000-0000-4000-8000-0000000001D2", "Et celle-ci",
                  attachments=[{"kind": "image", "mime": "image/png", "data": "AAAA"}])
    assert queued["state"] == "queued"
    assert wait_until(lambda: instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001D2"), timeout=5)
    said = instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001D2")["text"]
    assert "après" in said or "after" in said or "done" in said or "fini" in said, said
    # its turn, when it starts, is told so
    gate.set()
    assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=15)
    words = fakes.hermes.started["r2"]["input"][0]["content"][0]["text"]
    assert words.startswith("Et celle-ci") or "Et celle-ci\n\n" + MARK in words, words
    assert MARK in words and "right after" in words


def test_a_turn_restarted_with_the_words_answers_by_itself(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    # a turn that did nothing yet (it thinks): stopped and started again with the new words too (turn_restart.rs)
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.cancelled", run_id="r1")]
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="Les deux.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001E1", "Une recette")
    assert run_has(bridge, "r1", "run.started", agent="coach")
    answer = send(bridge, thread, "C1D00000-0000-4000-8000-0000000001E2", "sans gluten")
    assert answer["state"] == "steered" and answer.get("restarted") is True
    gate.set()
    time.sleep(2)
    assert instant(bridge, thread, "C1D00000-0000-4000-8000-0000000001E2") is None
    assert not any("while you work" in c["body"]["messages"][0]["content"] for c in fakes.model.chats)


def test_a_question_about_the_agent_during_its_turn_gets_no_line(bridges, fakes):
    # fix-instant-capabilities (Alex 27/11): the line's model knows none of the agent's tools; a sentence about the
    # agent's own tools, access or limits is never shown, and no sentence of the table in its place either
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    busy_turn(fakes, gate)
    fakes.model.instant_answer = "Je n'ai aucun outil pour contacter un autre agent."
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001F1", "Écris à Chasseur Mac pour lui demander son planning")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    cid = "C1D00000-0000-4000-8000-0000000001F2"
    steered = send(bridge, thread, cid, "Tes sûr que tu contactes le bon ? Son identifiant est agent_a8c8e457")
    assert steered["state"] == "steered"
    asked = lambda: [c for c in fakes.model.chats if "while you work" in c["body"]["messages"][0]["content"]]
    assert wait_until(asked, timeout=3)
    time.sleep(1.0)
    assert instant(bridge, thread, cid) is None, instant(bridge, thread, cid)
    system = asked()[-1]["body"]["messages"][0]["content"]
    assert "Never speak about yourself" in system and "single -" in system
    # the turn's note, joined before the line was asked, says such words get no line: the turn answers them
    steer = next(s for s in fakes.hermes.steers if "agent_a8c8e457" in str(s["body"].get("input")))["body"]["input"]
    assert MARK in steer and "no line if it asks about you or questions what you did or said: answer that yourself" in steer
    gate.set()


def test_words_the_model_gives_no_sentence_to_get_none_rather_than_the_tables(bridges, fakes):
    # its « - » (a message about the agent, or questioning what it did): no line at all, never « Noté, j'en tiens compte »
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    busy_turn(fakes, gate)
    fakes.model.instant_answer = "-"
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000001F3", "Réserve l'hôtel près de la gare")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    cid = "C1D00000-0000-4000-8000-0000000001F4"
    assert send(bridge, thread, cid, "Pourquoi tu as choisi celui-là ?")["state"] == "steered"
    assert wait_until(lambda: any("while you work" in c["body"]["messages"][0]["content"] for c in fakes.model.chats), timeout=3)
    time.sleep(1.0)
    assert instant(bridge, thread, cid) is None, instant(bridge, thread, cid)
    gate.set()
