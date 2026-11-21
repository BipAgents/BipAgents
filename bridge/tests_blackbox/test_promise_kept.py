"""A promise to tell later is kept or not made (proactive agents, part 1, Alex 21/11; bridge-rs/src/go_on.rs `nudge`,
runs.rs `go_on`): an agent never wakes up by itself, so « je te préviens dès que le prix baisse » holds only when the
turn set up what will notice it (a watcher, a routine, a trigger, a background task, another agent asked, a reminder).
A person's turn, working or not, that ends on such a promise with none of these gets the same ONE continuation as a
promise of work (test_promise_stop.py), told out of sight to set one up now or to say plainly it can't watch that."""
from bbkit.common import AUTH
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
    assert "you never wake up by yourself" in note and "make a watcher" in note and "routine_schedule" in note
    assert "say plainly in one short sentence that you can't watch that" in note
    # not the work's continuation: no background task to start
    assert "start_task" not in note and "Already set up" not in note
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
    }
    for run, tools in kept.items():
        fakes.hermes.runs[run] = turn(run, WATCH, tools=tools)
    for run in kept:
        events = follow(bridge, run)
        assert notices(events) == [], run
        assert events[-1][1]["run_id"] == run and events[-1][1]["output"] == WATCH
    assert list(fakes.hermes.started) == []


def test_a_question_an_offer_or_work_to_finish_is_no_promise_to_tell(bridges, fakes):
    bridge = bridges.new().start()
    said = {
        "run_q": "Le vélo est à 899 €. Tu veux que je te prévienne quand le prix baisse ?",
        "run_o": "Le vélo est à 899 €. Si tu veux, je te préviens dès que le prix baisse.",
        "run_d": "J'ai lancé la comparaison. Je te préviens quand c'est prêt.",
        "run_b": "Voilà le récap. Je te tiens au courant si besoin.",
    }
    for run, reply in said.items():
        fakes.hermes.runs[run] = turn(run, reply, tools=["web_search"])
    continued = [run for run in said if notices(follow(bridge, run))]
    assert continued == [], {r: s["input"][:90] for r, s in fakes.hermes.started.items()}


def test_switched_off_with_go_on(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"go_on": False}
    bridge.start()
    fakes.hermes.runs["run_bike"] = turn("run_bike", WATCH, tools=["web_search"])
    events = follow(bridge, "run_bike")
    assert notices(events) == [] and events[-1][1]["output"] == WATCH
    assert list(fakes.hermes.started) == []
