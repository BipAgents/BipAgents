"""A promise to tell later is kept or not made (proactive agents, part 1, Alex 21/11; bridge-rs/src/go_on.rs `nudge`,
runs.rs `go_on`): an agent never wakes up by itself, so « je te préviens dès que le prix baisse » holds only when the
turn set up what will notice it (a watcher, a routine, a trigger, a background task, another agent asked, a reminder).
A person's turn, working or not, that ends on such a promise with none of these gets the same ONE continuation as a
promise of work (test_promise_stop.py), told out of sight to set one up now or to say plainly it can't watch that."""
import json

from bbkit.common import AUTH, sse
from test_promise_stop import follow, notices, turn

WATCH = "Le vélo est à 899 € chez Decathlon. Je te préviens dès que le prix baisse."
KEPT = "C'est en place : un veilleur regarde le prix toutes les heures et te prévient s'il baisse."


def test_a_promise_to_tell_with_nothing_set_up_goes_on_once_to_set_it_up(bridges, fakes):
    bridge = bridges.new().start()
    # it searched the price, then promised to tell: nothing will notice the drop
    fakes.hermes.runs["run_bike"] = turn("run_bike", WATCH, tools=["web_search"])
    fakes.hermes.runs["r1"] = turn("r1", KEPT, tools=["mcp_bipagents_connectors_connector_register"])
    events = follow(bridge, "run_bike")
    said = notices(events)
    assert [n["code"] for n in said] == ["loop_rescue_started"] and said[0]["run_id"] == "r1"
    assert events[-1][0] == "run.completed" and events[-1][1]["output"] == KEPT
    assert [name for name, _ in events].count("run.completed") == 1
    assert list(fakes.hermes.started) == ["r1"]
    started = fakes.hermes.started["r1"]
    assert started["session_id"] == "api_c"
    note = started["input"]
    assert "[bipagents:go_on]" in note and f"« {WATCH} »" in note
    assert "nothing you set up will notice it" in note and "make a watcher" in note and "routine_schedule" in note
    assert "say plainly in one short sentence that you can't watch that" in note
    # review 1 (B3, R3): a one-off time is a reminder, not a routine; a watcher only where checks are allowed
    assert "reminders_add" in note and "notebook_task" in note and "allows automated checks" in note
    # not the work's note (« Do it now »), only its way out when the promise was in fact work
    assert "Do it now, in this answer" not in note and "If what you promised is in fact your own work" in note
    assert "Already set up" not in note
    assert bridge.logged("promise to tell later with nothing set up to notice it: continuation")


def test_a_turn_that_did_nothing_and_promises_to_tell_is_told_to_set_up_not_to_search(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_tell"] = turn("run_tell", "Je te tiens au courant dès que le prix baisse.", tools=["clarify"])
    fakes.hermes.runs["r1"] = turn("r1", "Je ne peux pas surveiller ce site : regarde-le de temps en temps.")
    events = follow(bridge, "run_tell")
    assert [n["code"] for n in notices(events)] == ["loop_rescue_started"]
    assert events[-1][1]["output"] == "Je ne peux pas surveiller ce site : regarde-le de temps en temps."
    assert "you can't watch that" in fakes.hermes.started["r1"]["input"]


def test_what_the_agent_already_set_up_is_named(bridges, fakes):
    bridge = bridges.new().start()
    body = {"name": "Point du matin", "schedule": "every day at 7:30", "prompt": "Résume mes e-mails."}
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH, json=body)
    assert made.status_code in (200, 201), made.text
    fakes.hermes.runs["run_bike"] = turn("run_bike", WATCH, tools=["web_search"])
    fakes.hermes.runs["r1"] = turn("r1", KEPT)
    follow(bridge, "run_bike")
    note = fakes.hermes.started["r1"]["input"]
    assert "Already set up: routine « Point du matin ». If one of these already notices exactly this" in note


def test_a_promise_kept_by_what_the_turn_set_up_ends_the_turn_as_before(bridges, fakes):
    bridge = bridges.new().start()
    kept = {
        "run_w": ["web_search", "mcp_bipagents_connectors_connector_register"],   # a watcher
        "run_u": ["mcp_bipagents_connectors_connector_update"],                   # a watcher changed
        "run_r": ["mcp_bipagents_routine_schedule"],                              # a routine
        "run_t": ["mcp_bipagents_routine_trigger"],                               # each time an e-mail arrives
        "run_s": ["mcp_bipagents_start_task"],                                    # a task that reports when done
        "run_a": ["mcp_bipagents_message_agent"],                                 # another agent's answer comes back
        # review 1 (B3): a dated task of the Carnet sets the phone's reminder, a calendar event its alert
        "run_n": ["mcp_bipagents_notebook_task"],
        "run_k": ["mcp_bipagents_calendar_add"],
    }
    for run, tools in kept.items():
        fakes.hermes.runs[run] = turn(run, WATCH, tools=tools)
    for run in kept:
        events = follow(bridge, run)
        assert notices(events) == [], run
        assert events[-1][1]["run_id"] == run and events[-1][1]["output"] == WATCH
    assert list(fakes.hermes.started) == []


def test_a_watcher_or_routine_set_up_through_the_tool_search_keeps_the_promise(bridges, fakes):
    # bench 22/11 (DeepSeek): connector_register and routine_schedule, behind the tool search, were called through
    # `tool_call`: the turn's tools must be read through it, else « je te préviens… » after a watcher made that way
    # would get the continuation
    bridge = bridges.new().start()
    wrapped = {
        "run_c": json.dumps({"name": "mcp__bipagents_connectors__connector_register", "arguments": {"name": "Gel"}}),
        "run_r": json.dumps({"calls": [{"name": "mcp_bipagents_routine_schedule", "arguments": {}}]}),
    }
    for run, preview in wrapped.items():
        fakes.hermes.runs[run] = ([sse("tool.started", run_id=run, tool="tool_call", preview=preview),
                                   sse("tool.completed", run_id=run, tool="tool_call")]
                                  + turn(run, WATCH, tools=["web_search"]))
    for run in wrapped:
        events = follow(bridge, run)
        assert notices(events) == [], run
        assert events[-1][1]["output"] == WATCH
    assert list(fakes.hermes.started) == []
    # the tool search used for something else keeps nothing
    fakes.hermes.runs["run_x"] = ([sse("tool.started", run_id="run_x", tool="tool_call",
                                       preview=json.dumps({"name": "mcp_bipagents_weather", "arguments": {}})),
                                   sse("tool.completed", run_id="run_x", tool="tool_call")]
                                  + turn("run_x", WATCH))
    fakes.hermes.runs["r1"] = turn("r1", KEPT)
    assert [n["code"] for n in notices(follow(bridge, "run_x"))] == ["loop_rescue_started"]


def test_a_question_an_offer_or_work_to_finish_is_no_promise_to_tell(bridges, fakes):
    bridge = bridges.new().start()
    said = {
        "run_q": "Le vélo est à 899 €. Tu veux que je te prévienne quand le prix baisse ?",
        "run_o": "Le vélo est à 899 €. Si tu veux, je te préviens dès que le prix baisse.",
        "run_d": "J'ai lancé la comparaison. Je te préviens quand c'est prêt.",
        "run_b": "Voilà le récap. Je te tiens au courant si besoin.",
        # review 1 (B2): a warning or a reminder given now
        "run_w1": "Je te préviens que le magasin ferme à 19 h.",
        "run_w2": "Je te préviens aussi : si tu commandes après 14 h, c'est livré lundi.",
        "run_w3": "Je te rappelle juste que la réunion est demain à 9 h.",
        "run_w4": "Je te préviens que si tu prends l'option, c'est 10 € de plus.",
        # review 2 (C2)
        "run_w5": "Te aviso que si pides después de las 14 h, llega el lunes.",
    }
    for run, reply in said.items():
        fakes.hermes.runs[run] = turn(run, reply, tools=["web_search"])
    continued = [run for run in said if notices(follow(bridge, run))]
    assert continued == [], {r: s["input"][:90] for r, s in fakes.hermes.started.items()}


def test_a_promise_kept_by_what_the_agent_already_has_is_not_continued(bridges, fakes):
    # review 1 (B1): the end names what already does it (a routine of its, « le veilleur » when it has one): no paid
    # continuation; « le veilleur est en place » with nothing set up is still not kept
    bridge = bridges.new().start()
    body = {"name": "Point du matin", "schedule": "every day at 7:30", "prompt": "Résume mes e-mails."}
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH, json=body)
    assert made.status_code in (200, 201), made.text
    said = {
        "run_n": "C'est la routine « Point du matin » qui s'en charge : je te préviens dès qu'il y a du nouveau.",
        "run_g": "Avec plaisir ! La routine est en place, je te redis ça demain matin.",
    }
    for run, reply in said.items():
        fakes.hermes.runs[run] = turn(run, reply, tools=["web_search"])
    for run in said:
        assert notices(follow(bridge, run)) == [], run
    assert list(fakes.hermes.started) == []


def test_a_claim_with_nothing_set_up_and_a_later_question_still_go_on(bridges, fakes):
    bridge = bridges.new().start()
    # nothing of the agent's: « le veilleur est en place » is not true
    fakes.hermes.runs["run_c"] = turn("run_c", "Le veilleur est en place, je te préviens dès que le prix baisse.")
    fakes.hermes.runs["r1"] = turn("r1", KEPT, tools=["mcp_bipagents_connectors_connector_register"])
    assert [n["code"] for n in notices(follow(bridge, "run_c"))] == ["loop_rescue_started"]
    # review 1 (R2): a question in a later sentence does not undo the promise to tell
    fakes.hermes.runs["run_q"] = turn("run_q", "Je te préviens dès que le prix baisse. Tu veux aussi que je regarde le "
                                               "modèle L ?", tools=["web_search"])
    fakes.hermes.runs["r2"] = turn("r2", KEPT, tools=["mcp_bipagents_connectors_connector_register"])
    assert [n["code"] for n in notices(follow(bridge, "run_q"))] == ["loop_rescue_started"]


def test_switched_off_with_go_on(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"go_on": False}
    bridge.start()
    fakes.hermes.runs["run_bike"] = turn("run_bike", WATCH, tools=["web_search"])
    events = follow(bridge, "run_bike")
    assert notices(events) == [] and events[-1][1]["output"] == WATCH
    assert list(fakes.hermes.started) == []
