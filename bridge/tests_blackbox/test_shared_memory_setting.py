"""« Mémoire partagée », per agent, and the defaults for an agent on an outside model (bridge-rs/src/
agent_shared_memory.rs, outside_model.rs, shared_memory.rs; passation-pro briefs/status-shared-memory.md;
contracts/shared-memory/cases.json; Alex 23/11). Off: out of the common memory's circle, what it received taken out of
its USER.md (what it wrote itself kept), the day's copy of its prompt gone, Hermes told to show it no USER.md, its
conversations told to take a new prompt. An outside model (ChatGPT through Codex, Anthropic, OpenRouter…): shared
memory and Carnet off by default, at its creation and at the bridge's start for the agents of before. An app 1.0 sees
its settings as before, with one more key it ignores."""
import json
import time
from pathlib import Path

import yaml

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_agents_admin import hermes_args, managed

CASES = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "shared-memory" / "cases.json").read_text())
EUROUTER = ("model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n"
            "  key_env: CUSTOM_MODEL_API_KEY\n")
CODEX = "model:\n  provider: openai-codex\n  default: gpt-6.1\n"
DAY_COPY = ".prompt-day.json"


def settings(bridge, agent):
    resp = bridge.get(f"/v1/agents/{agent}/settings", headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()


def shared(values):
    """`shared_memory` of the settings, `outside.since` (when the protection applied: a time) checked and left out."""
    shown = values.get("shared_memory")
    if shown and shown.get("outside"):
        outside = dict(shown["outside"])
        assert isinstance(outside.pop("since"), (int, float)), shown
        shown = {**shown, "outside": outside}
    return shown


def put(bridge, agent, body):
    return bridge.put(f"/v1/agents/{agent}/shared-memory", headers=AUTH, json=body)


def memories(home, agent):
    return home / "profiles" / agent / "memories"


def user_md(home, agent):
    path = memories(home, agent) / "USER.md"
    return [e for e in path.read_text().split("\n§\n") if e] if path.exists() else []


def edit_memory(bridge, agent, entries):
    """The person's edit of « what it knows about you » in the agent's page: applied to all the person's own agents
    at once (memory_edit.rs → shared_memory::sync)."""
    resp = bridge.put(f"/v1/agents/{agent}/memory/user", headers=AUTH, json={"entries": entries})
    assert resp.status_code == 200, resp.text


def profile_yaml(home, agent):
    return yaml.safe_load((home / "profiles" / agent / "config.yaml").read_text()) or {}


def user_profile(home, agent):
    return (profile_yaml(home, agent).get("memory") or {}).get("user_profile_enabled", "absent")


def refreshed(bridge, agent):
    return ["-p", agent, "sessions", "refresh-prompts", "--json"] in hermes_args(bridge)


def place(bridges, fakes, hub_model=EUROUTER, agents=("Budget", "Coach")):
    """A managed place whose hub runs `hub_model`, with the person's own agents (cloned from the hub)."""
    bridge, home = managed(bridges, fakes, start=False)
    (home / "config.yaml").write_text(hub_model)
    bridge.start()
    for name in agents:
        made = bridge.post("/v1/agents", headers=AUTH, json={"name": name, "kind": "finance" if name == "Budget" else "wellness"})
        assert made.status_code == 201, made.text
    return bridge, home


def test_the_contract_s_settings_and_answers(bridges, fakes):
    bridge, home = place(bridges, fakes)
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Voyage à Lisbonne", "ends": time.time() + 86400 * 10,
                                                                   "lang": "fr", "category": "creative"})
    assert made.status_code == 201, made.text
    project = made.json()["agent"]
    # an agent created on an outside model: its hub's model changed first
    (home / "config.yaml").write_text(CODEX)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Gépété", "kind": "daily"}).status_code == 201
    agents = {"own": "budget", "outside": "gepete", "others_facing": project, "nobody": "nobody"}
    for case in CASES["put"]:
        resp = put(bridge, agents[case["agent"]], case["body"])
        assert resp.status_code == case["status"], (case["name"], resp.text)
        if case["status"] == 200:
            assert resp.json() == {"id": agents[case["agent"]], **case["answer"]}, case["name"]
        elif case["answer"] is not None:
            assert resp.json() == case["answer"], case["name"]
    stored = bridge.state_dir / "agent_shared_memory.json"
    assert stored.stat().st_mode & 0o777 == 0o600
    for case in CASES["settings"]:
        agent = agents[case["agent"]]
        if case["stored"] is not None:
            assert put(bridge, agent, {"on": case["stored"]}).status_code == 200, case["name"]
        shown = settings(bridge, agent)
        assert shared(shown) == case["shared_memory"], (case["name"], shown.get("shared_memory"))
        if case["notebook"] is not None:
            assert shown["notebook"] == case["notebook"], (case["name"], shown["notebook"])


def test_off_gives_back_what_it_received_and_hermes_shows_it_no_user_md(bridges, fakes):
    bridge, home = place(bridges, fakes)
    edit_memory(bridge, "budget", ["Alex habite Lyon."])             # the person, in Budget's page: every agent's
    assert wait_until(lambda: user_md(home, "coach") == ["Alex habite Lyon."])
    edit_memory(bridge, "coach", user_md(home, "coach") + ["Alex court le dimanche."])   # in Coach's own: Coach's
    assert wait_until(lambda: "Alex court le dimanche." in user_md(home, "budget"))
    (memories(home, "coach") / DAY_COPY).write_text(json.dumps({"day": "2026-11-23", "memory": [], "user": user_md(home, "coach")}))
    assert user_profile(home, "coach") == "absent"
    bridge.commands.clear()

    assert put(bridge, "coach", {"on": False}).json() == {"id": "coach", "on": False}
    assert user_md(home, "coach") == ["Alex court le dimanche."], "what it received out, its own kept"
    assert not (memories(home, "coach") / DAY_COPY).exists()
    assert user_profile(home, "coach") is False
    assert wait_until(lambda: refreshed(bridge, "coach"))
    assert shared(settings(bridge, "coach")) == {"on": False, "circle": False, "outside": None}
    soul = (home / "profiles" / "coach" / "SOUL.md").read_text()
    assert "bipagents:shared-memory" not in soul, "the shared memory block out of its personality"
    # nothing given any more
    edit_memory(bridge, "budget", user_md(home, "budget") + ["Alex déménage à Nantes."])
    time.sleep(0.5)
    assert user_md(home, "coach") == ["Alex court le dimanche."]
    # « Ce que tes agents savent de toi »: Coach shown, with what its prompt really carries
    known = bridge.get("/v1/notebook/known", headers=AUTH, params={"agent": "coach", "lang": "fr"}).json()
    assert "coach" in [a["id"] for a in known["agents"]]
    assert known["always"]["lines"] == [] and known["always"]["note"] == CASES["texts"]["fr"]["known_off"]

    # back on: Hermes' default again, the common list given again
    bridge.commands.clear()
    assert put(bridge, "coach", {"on": True}).json() == {"id": "coach", "on": True}
    assert user_profile(home, "coach") == "absent"
    assert wait_until(lambda: "Alex déménage à Nantes." in user_md(home, "coach"))
    assert user_md(home, "coach")[0] == "Alex court le dimanche."
    assert wait_until(lambda: refreshed(bridge, "coach"))


def test_the_carnet_off_gives_back_what_it_received_at_once(bridges, fakes):
    bridge, home = place(bridges, fakes)
    edit_memory(bridge, "budget", ["Alex habite Lyon."])
    assert wait_until(lambda: user_md(home, "coach") == ["Alex habite Lyon."])
    (memories(home, "coach") / DAY_COPY).write_text("{}")
    bridge.commands.clear()
    resp = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"notebook": False})
    assert resp.status_code == 200, resp.text
    assert user_md(home, "coach") == []
    assert not (memories(home, "coach") / DAY_COPY).exists()
    assert wait_until(lambda: refreshed(bridge, "coach"))
    # its shared memory switch is still on: Hermes still shows it its own USER.md
    assert user_profile(home, "coach") == "absent"
    # every agent on « Ce que tes agents savent de toi », its Carnet on or not
    known = bridge.get("/v1/notebook/known", headers=AUTH, params={"agent": "", "lang": "fr"}).json()
    assert {"budget", "coach"} <= {a["id"] for a in known["agents"]}


def test_health_never_joins_the_common_memory(bridges, fakes):
    bridge, home = place(bridges, fakes)
    edit_memory(bridge, "budget", ["Alex habite Lyon.", "Alex prend un traitement pour l'asthme."])
    assert wait_until(lambda: user_md(home, "coach") == ["Alex habite Lyon."])
    time.sleep(0.5)
    assert user_md(home, "coach") == ["Alex habite Lyon."]
    common = json.loads((bridge.state_dir / "notebook" / "shared-user.json").read_text())["common"]
    assert [c["text"] for c in common] == ["Alex habite Lyon."]
    assert len(user_md(home, "budget")) == 2, "it stays where it was written"


def test_an_agent_created_on_an_outside_model_starts_with_both_off(bridges, fakes):
    bridge, home = place(bridges, fakes, hub_model=CODEX, agents=("Coach",))
    shown = settings(bridge, "coach")
    assert shared(shown) == {"on": False, "circle": False, "outside": {"provider": "openai-codex", "label": "OpenAI"}}
    assert shown["notebook"] == {"on": False} and shown["health"]["on"] is False
    assert user_profile(home, "coach") is False
    rules = bridge.get("/v1/agent-rules", headers=AUTH).json()
    assert "coach" in rules["aside"], "its links with the other agents off (« à l'écart »)"
    # turned back on, knowingly: on, and kept at the next start
    assert put(bridge, "coach", {"on": True}).json() == {"id": "coach", "on": True}
    bridge.restart(write=False)   # (the agents it made are in its own config file)
    assert settings(bridge, "coach")["shared_memory"]["on"] is True
    assert user_profile(home, "coach") == "absent"


def test_the_next_update_cleans_an_agent_already_on_an_outside_model(bridges, fakes):
    """Alex's test agent on ChatGPT (openai-codex) had received his facts through the common memory: at the next
    start of the bridge, they go back, its Carnet and shared memory off, its prompts refreshed."""
    bridge, home = place(bridges, fakes)
    edit_memory(bridge, "budget", ["Alex habite Lyon.", "Alex a deux enfants."])
    assert wait_until(lambda: len(user_md(home, "coach")) == 2)
    (memories(home, "coach") / DAY_COPY).write_text(json.dumps({"day": "2026-11-23", "memory": [], "user": user_md(home, "coach")}))
    bridge.stop()
    folder = home / "profiles" / "coach"
    data = profile_yaml(home, "coach")
    data["model"] = {"provider": "openai-codex", "default": "gpt-6.1"}
    (folder / "config.yaml").write_text(yaml.safe_dump(data))
    bridge.commands.clear()
    bridge.start(write=False)   # (the agents it made are in its own config file)
    assert wait_until(lambda: user_md(home, "coach") == [], timeout=40)
    assert not (memories(home, "coach") / DAY_COPY).exists()
    shown = settings(bridge, "coach")
    assert shared(shown) == {"on": False, "circle": False, "outside": {"provider": "openai-codex", "label": "OpenAI"}}
    assert shown["notebook"] == {"on": False}
    assert wait_until(lambda: user_profile(home, "coach") is False)
    assert wait_until(lambda: refreshed(bridge, "coach"), timeout=40)
    # Budget, on the install's provider, untouched
    assert user_md(home, "budget") == ["Alex habite Lyon.", "Alex a deux enfants."]
    assert shared(settings(bridge, "budget")) == {"on": True, "circle": True, "outside": None}


def test_an_app_1_0_sees_its_settings_as_before(bridges, fakes):
    """What an app 1.0 reads of an agent's settings is unchanged but for one more key it ignores."""
    bridge, home = place(bridges, fakes)
    before = settings(bridge, "budget")
    assert put(bridge, "budget", {"on": False}).status_code == 200
    after = settings(bridge, "budget")

    def as_before(values):
        return {k: v for k, v in values.items() if k != "shared_memory"}

    assert as_before(after) == as_before(before)
    assert before["shared_memory"] == {"on": True, "circle": True, "outside": None} and after["shared_memory"]["on"] is False


def test_its_conversations_go_on_in_a_new_session(bridges, fakes):
    """Review B2: Hermes replays a conversation's history at every turn (the memory's turn notes, the Carnet's news
    joined to the person's words). Its shared memory turned off, or its model gone outside: each conversation goes on in
    a new Hermes session; the app keeps showing what was said (the same thread)."""
    from test_agent_create import idle, send
    bridge, home = place(bridges, fakes)
    send(bridge, fakes, "coach", "Bonjour Coach, je cours le dimanche.")
    idle(bridge, "coach")
    before = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "coach"}).json()
    assert put(bridge, "coach", {"on": False}).json() == {"id": "coach", "on": False}
    after = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "coach"}).json()
    assert after["id"] == before["id"] and after["tip"] != before["tip"], (before, after)
    segments = bridge.get(f"/v1/threads/{after['id']}", headers=AUTH, params={"agent": "coach"}).json()["segments"]
    assert [s["session_id"] for s in segments][-2:] == [before["tip"], after["tip"]] and segments[-1]["linked"] == "moved"
    timeline = bridge.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": "coach"}).json()
    assert any("je cours le dimanche" in (i.get("text") or "") for i in timeline["items"]), "what was said still shown"
    # the next turn runs in the new session
    from test_agent_create import next_run
    idle(bridge, "coach")
    run = next_run(fakes)
    sent = bridge.post("/v1/threads/main/messages", headers=AUTH,
                       json={"agent": "coach", "client_id": "c-after-off", "text": "Et maintenant ?"})
    assert sent.status_code == 200 and sent.json()["tip"] == after["tip"], sent.text
    assert fakes.hermes.started[run]["session_id"] == after["tip"]
    # on again: nothing moves (a new session only when it goes off)
    assert put(bridge, "coach", {"on": True}).status_code == 200
    idle(bridge, "coach")
    assert bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "coach"}).json()["tip"] == after["tip"]


def test_review2_a_message_waiting_behind_a_turn_never_goes_on_in_the_old_session(bridges, fakes):
    """Review 2 (red), B2: a turn going on when the shared memory goes off (by the person, or the protection of an
    outside model at a model change) keeps its conversation on the old session (`fresh_sessions`: « at the next
    look »). A message waiting behind it (« Après ») was queued for that session (msg_queue.rs `item.session_id`) and
    starts in it at the turn's end: Hermes replays the whole history there, memory notes and Carnet news included."""
    import threading
    import uuid
    from bbkit.common import sse
    from test_agent_create import idle, send as say
    bridge, home = place(bridges, fakes)
    say(bridge, fakes, "coach", "Bonjour Coach, je cours le dimanche.")
    idle(bridge, "coach")
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    going, waiting = f"r{len(fakes.hermes.started) + 1}", f"r{len(fakes.hermes.started) + 2}"
    fakes.hermes.runs[going] = [sse("run.started", run_id=going), gate, sse("run.completed", run_id=going, output="Voilà.")]
    fakes.hermes.runs[waiting] = [sse("run.started", run_id=waiting), sse("run.completed", run_id=waiting, output="Ajouté.")]

    def message(text, **extra):
        return bridge.post("/v1/threads/main/messages", headers=AUTH,
                           json={"agent": "coach", "client_id": uuid.uuid4().hex, "text": text, **extra})

    first = message("Prépare mon plan de la semaine.")
    assert first.status_code == 200 and first.json()["state"] == "started", first.text
    old = first.json()["tip"]
    later = message("Et ajoute le vélo.", after=True)
    assert later.status_code == 200 and later.json()["state"] == "queued", later.text
    assert put(bridge, "coach", {"on": False}).json() == {"id": "coach", "on": False}
    gate.set()
    assert wait_until(lambda: waiting in fakes.hermes.started, timeout=30)
    assert fakes.hermes.started[waiting]["session_id"] != old, "the waiting message went on in the old session"


def test_a_model_of_the_catalogue_on_an_outside_hub_keeps_the_carnet_off(bridges, fakes):
    """Review C4: Kerfy's catalogue says « Carnet on » for each of its agents; on a hub on an outside model the new
    agent is protected all the same, and it is never noted as the person's own gesture."""
    bridge, home = place(bridges, fakes, hub_model=CODEX, agents=())
    adopted = bridge.post("/v1/templates/import", headers=AUTH, json={"catalogue": "cuisine", "lang": "fr"})
    assert adopted.status_code == 201, adopted.text
    agent = adopted.json()["agent"]["id"]
    shown = settings(bridge, agent)
    assert shown["notebook"] == {"on": False} and shared(shown)["on"] is False
    kept = json.loads((bridge.state_dir / "agent_shared_memory.json").read_text())
    assert "notebook" in kept["outside"][agent]["off"], "the bridge's own: a return to our providers gives it back"


def test_custom_without_its_address_and_a_model_it_cannot_tell_are_protected(bridges, fakes):
    """Review C1: `custom` without its address goes to OpenRouter in Hermes; a profile with no model the bridge can
    read is protected too (« in doubt »), named so for the apps."""
    bridge, home = place(bridges, fakes, hub_model="model:\n  provider: custom\n  default: x\n", agents=("Coach",))
    shown = settings(bridge, "coach")
    assert shared(shown) == {"on": False, "circle": False, "outside": {"provider": "custom", "label": "OpenRouter"}}
    assert shown["notebook"] == {"on": False}
    # a profile whose model can't be read, for an agent never protected: protected as outside
    (home / "config.yaml").write_text("model:\n  provider: lmstudio\n")
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Budget", "kind": "finance"}).status_code == 201
    folder = home / "profiles" / "budget"
    (folder / "config.yaml").write_text("agent:\n  max_turns: 60\n")
    (home / "config.yaml").write_text("")
    assert wait_until(lambda: shared(settings(bridge, "budget"))["on"] is False, timeout=40)
    assert shared(settings(bridge, "budget"))["outside"]["provider"] == "unknown"


def test_review3_a_card_answered_during_the_switch_never_resumes_in_the_old_session(bridges, fakes):
    """Review 3 (red), B2-a: the send and the « Après » queue read the `fresh` mark where their turn starts
    (agent_shared_memory.rs `fresh_tip`), the bridge's own turns in a conversation do not: background_turn.rs
    `Pipeline::tip` / `wait_idle` take the thread's last session as it is (a card answered, card_place.rs
    `resume_turn`; a sub-agent's report, handback.rs; what was noted in a voice call). A card answered while the turn
    goes on, the shared memory turned off meanwhile (by the person, or the protection of an outside model at a model
    change): the turn keeps the old session (« a turn going on: at the next look »), and once it ends the card's turn
    starts in that old session, where Hermes replays the whole history."""
    from test_cards_resume import mail_in_a_turn, resumed
    from test_mail_cards import answer as mail_answer
    bridge, gate, [card] = mail_in_a_turn(bridges, fakes, "api_switch", "run_switch", [("move:Banque", "banque.example")])
    assert mail_answer(bridge, card).status_code == 200
    assert put(bridge, "coach", {"on": False}).json() == {"id": "coach", "on": False}
    gate.set()

    def card_turns():
        return [r for r in list(fakes.hermes.started.values()) if "<!--bip:card" in str(r.get("input"))]

    assert wait_until(card_turns, timeout=30), list(fakes.hermes.started.values())
    assert resumed(fakes, "api_switch") == [], "the card's turn went on in the old session"
