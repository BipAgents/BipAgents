"""27/11 16:22, Alex's aibox (Vie, on the Spark): meaning to change its nightly routine's instruction, the agent called
give_file on a file in a hidden folder. The bridge answered « no file I can give (a hidden or secret-looking file (never
sent)…). Save the file you made in /envoi (or /workspace) and give that path. »; the model asked again, the brake
stopped the turn as « the same tool failing again and again (give_file) », and the rescue turn, told the tool « looks
unavailable », said « le pointeur est momentanément bloqué » and promised to apply it « à ma prochaine session ».

Now (bridge-rs/src/give_file.rs `never_given`, loop_brake.rs `Turns::check`, runs.rs `loop_rescue`): the refusal is
final and says so; the very same call again is « already refused » at once, its step still not done but no new failure
of the tool; the rescue turn gets what was answered, and is told to make up no cause and promise nothing."""
import threading
import time

from bbkit.common import AUTH, read_sse, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until

GIVE = "mcp_bipagents_give_file"
HIDDEN = "/root/.hermes/routines/scan-nocturne.md"
COPY = "/envoi/.routines/scan-nocturne.md"
OTHER = "/workspace/.config/consigne-scan.md"


def giving(run_id, count, tail):
    """`count` give_file calls, each held until its MCP answer came (a gate), then `tail`."""
    gates = [threading.Event() for _ in range(count)]
    events = []
    for gate in gates:
        events += [sse("tool.started", run_id=run_id, tool=GIVE, preview=""), gate,
                   sse("tool.completed", run_id=run_id, tool=GIVE, error=False, preview="")]
    return gates, events + tail


def follow_in_background(bridge, run_id, events):
    def follow():
        with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
            events.extend(read_sse(resp))

    follower = threading.Thread(target=follow)
    follower.start()
    return follower


def ask(bridge, gates, paths):
    """Each path given as Hermes would call it, once its call started; the answers."""
    answers = []
    time.sleep(1.5)   # the run's first call started, as Hermes calls the bridge's tool
    for gate, path in zip(gates, paths):
        answers.append(text_of(call(bridge, "give_file", {"path": path, "title": "Consigne du scan nocturne"})))
        gate.set()
        time.sleep(0.8)   # its completion relayed, the next call started
    return answers


def notices(events):
    return [data for name, data in events if name == "bridge.notice"]


def test_a_hidden_file_is_refused_for_good_and_asking_again_is_no_broken_tool(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff": False}
    bridge.config["brake"] = {"rescue": False}
    bridge.start()
    give_token(bridge)
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "Change la consigne de ton scan de nuit"}])
    reply = "Je ne peux pas t'envoyer ce fichier : c'est un fichier caché."
    gates, script = giving("run_hid", 3, [sse("message.delta", run_id="run_hid", delta=reply),
                                          sse("run.completed", run_id="run_hid", output=reply)])
    fakes.hermes.runs["run_hid"] = script
    events = []
    follower = follow_in_background(bridge, "run_hid", events)
    first, again, other = ask(bridge, gates, [HIDDEN, HIDDEN, OTHER])
    follower.join(30)
    # final and plain: never sent, under any name; not again; what give_file is for; what to do instead
    for said in (first, other):
        assert said.startswith("Not done, and it never will be: ") and "never handed to anyone" in said, said
        assert "Don't call give_file for it again" in said and "Save the file you made" not in said, said
    assert "routine_schedule" in first, "a routine's file: the tool that changes a routine, named"
    # the very same call: « already refused » at once, nothing broken
    assert again.startswith("Not done, already refused:") and "Nothing is broken" in again, again
    # no stop: two failures of the tool, not three (the turn ends on the agent's own words)
    assert notices(events) == [], notices(events)
    assert events[-1][0] == "run.completed" and events[-1][1]["output"] == reply
    assert not fakes.hermes.seen("/v1/runs/run_hid/stop", "POST")
    # each step still drawn not done in the apps
    completed = [d for e, d in events if e == "tool.completed"]
    assert [d["error"] for d in completed] == [True, True, True], completed
    assert bridge.logged("a call refused for good, asked again: already refused")


def test_the_rescue_turn_gets_what_was_refused_and_makes_up_nothing(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff": False}
    bridge.start()
    give_token(bridge)
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "Change la consigne de ton scan de nuit"}])
    gates, script = giving("run_vie", 3, [sse("run.cancelled", run_id="run_vie")])
    fakes.hermes.runs["run_vie"] = script
    reply = "Je n'ai pas pu changer la consigne : ce fichier est caché et je ne peux pas te l'envoyer. Tu veux que je modifie la routine elle-même ?"
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    events = []
    follower = follow_in_background(bridge, "run_vie", events)
    answers = ask(bridge, gates, [HIDDEN, COPY, OTHER])
    follower.join(40)
    said = notices(events)
    assert [n["code"] for n in said] == ["loop_stopped", "loop_rescue_started"], said
    assert said[0]["reason"] == "the same tool failing again and again (give_file)"
    assert wait_until(lambda: "r1" in fakes.hermes.started, timeout=10)
    rescue = fakes.hermes.started["r1"]["input"]
    # what the bridge answered the last time, as the only reason known
    assert "the only reason you know of" in rescue and OTHER in rescue and "hidden or secret-looking" in rescue, rescue
    assert answers[-1].split(":", 1)[1].strip()[:60] in rescue, (answers[-1], rescue)
    # never a cause made up nor a promise
    assert "looks unavailable" not in rescue, rescue
    for rule in ("no cause you don't know", "never that you noted, saved or changed something you didn't",
                 "no promise to do it later or at your next session", "Never name a tool or a limit"):
        assert rule in rescue, (rule, rescue)
    assert events[-1][0] == "run.completed" and events[-1][1]["output"] == reply
