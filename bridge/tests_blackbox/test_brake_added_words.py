"""Words a person adds while the agent answers, when the brake stops that turn (bridge-rs/src/runs.rs `loop_rescue`,
msg_queue.rs). 27/11, Alex's aibox (Famille): « Trouve-moi des gants en laine Merino pour Mamino » added during the
answer (« Ajouté pendant la réponse », its check « Pris en compte »), the bridge's line « je m'en occupe »… then the
brake stopped the turn and the request was lost. Now the rescue turn knows them, and they are taken up right after it
as the person's next message: words the turn never took (Hermes' `pending_steer`) as before, words it took too."""
import threading

from bbkit.common import AUTH, run_has, sse
from bbkit.servers import wait_until
from test_turn_restart import P1, Q1, SESSION, follow, page, send, setup

GLOVES = "Trouve-moi des gants en laine Merino pour Mamino"


def stopped_turn(run_id, gate, **end):
    """A turn that acted (a mail sent: the words go in by Hermes' steer), then a tool failing three times: the brake."""
    calls = [e for _ in range(3) for e in (
        sse("tool.started", run_id=run_id, tool="mcp__bipagents__calendar_list", preview="semaine"),
        sse("tool.completed", run_id=run_id, tool="mcp__bipagents__calendar_list", error=True,
            preview="Error: the calendar's file is unreadable"))]
    return ([sse("run.started", run_id=run_id),
             sse("tool.started", run_id=run_id, tool="mcp__bipagents__mail_send", preview="Léa"),
             sse("tool.completed", run_id=run_id, tool="mcp__bipagents__mail_send"), gate]
            + calls + [sse("run.cancelled", run_id=run_id, **end)])


def run_it(bridges, fakes, **end):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = stopped_turn("r1", gate, **end)
    sorry = "Je n'arrive pas à lire ton agenda pour l'instant. Veux-tu que je réessaie dans un moment ?"
    fakes.hermes.runs["r2"] = [sse("run.started", run_id="r2"), sse("message.delta", run_id="r2", delta=sorry),
                               sse("run.completed", run_id="r2", output=sorry)]
    found = "Voici trois paires de gants en mérinos."
    fakes.hermes.runs["r3"] = [sse("run.started", run_id="r3"), sse("message.delta", run_id="r3", delta=found),
                               sse("run.completed", run_id="r3", output=found)]
    assert send(bridge, thread, Q1, "Écris à Léa puis regarde ma semaine")["state"] == "started"
    app = []
    reader = threading.Thread(target=follow, args=(bridge, "r1", app), daemon=True)
    reader.start()
    assert run_has(bridge, "r1", "tool.completed")
    sent = send(bridge, thread, P1, GLOVES)
    assert (sent["state"], sent["run_id"]) == ("steered", "r1"), sent
    gate.set()
    reader.join(timeout=60)
    return bridge, thread, app


def test_words_the_stopped_turn_took_are_taken_up_right_after_its_rescue(bridges, fakes):
    bridge, thread, app = run_it(bridges, fakes)
    codes = [d["code"] for n, d in app if n == "bridge.notice"]
    assert codes[:2] == ["loop_stopped", "loop_rescue_started"], codes
    # the rescue turn knows them, and is told not to answer them itself
    rescue = fakes.hermes.started["r2"]["input"]
    assert rescue.startswith("[bipagents:loop_rescue]") and GLOVES in rescue, rescue
    # then they start as the next turn of the conversation, with a note saying why
    assert wait_until(lambda: "r3" in fakes.hermes.started, timeout=20), list(fakes.hermes.started)
    carried = fakes.hermes.started["r3"]
    assert carried["session_id"] == SESSION and carried["input"].rstrip().endswith(GLOVES), carried
    assert "<!--bip:carried" in carried["input"]


def test_words_the_stopped_turn_never_took_come_back_too(bridges, fakes):
    bridge, thread, app = run_it(bridges, fakes, pending_steer=GLOVES)
    assert GLOVES in fakes.hermes.started["r2"]["input"]
    assert wait_until(lambda: "r3" in fakes.hermes.started, timeout=20), list(fakes.hermes.started)
    assert fakes.hermes.started["r3"]["input"].rstrip().endswith(GLOVES)
    # once only
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=20)
    assert [r for r in fakes.hermes.started if r not in ("r1", "r2", "r3")] == []
