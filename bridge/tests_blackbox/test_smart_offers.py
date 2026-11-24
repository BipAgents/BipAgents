"""« Initiative » smart (proactive agents 3b: bridge-rs/src/offer_ideas.rs; passation-pro
briefs/design-agents-proactifs.md §3b; contracts/smart-offers/cases.json). A smart agent may offer any action it can
really do here: every `use` checked against what it has, the card says with what, what will ask its own OK later and a
site never allowed; never paying, sending, booking, deleting or sharing as the offer itself. A « Oui » starts one turn,
and the routine its card showed. A few times a day, when something happened and the conversation is quiet, ONE call
to the small model looks back over the day and may place one idea; a quiet day makes no pass, the place's cap holds.
A fake Hermes and a fake model only (the bench with real models is another branch)."""
import hashlib
import json
import time
from pathlib import Path

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, not_done, rpc, text_of
from bbkit.servers import wait_until
from test_offers import offer_cards, search, shape
from test_plans import answer, done, place, TOKEN

CASES = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "smart-offers" / "cases.json").read_text())
TEXTS = CASES["texts"]
LANGS = ("fr", "en", "de", "es")


def smart_place(bridges, fakes, **proactive):
    """Coach, set to « Intelligente », its passes' model the fake one (as `[notebook]` names it)."""
    def change(c):
        c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb", "model": "small-eu"},
                 proactive={"tick_seconds": 3600, **proactive})
    bridge = place(bridges, fakes, change=change)
    assert bridge.put("/v1/agents/coach/proactive", headers=AUTH, json={"level": "smart"}).json()["level"] == "smart"
    settle_token(bridge)
    return bridge


def settle_token(bridge, agent="coach"):
    """Smart changes offer_act (its free form): the bridge may register the agent's tools again in the background, with
    a token of its own (agent_tools.rs `register_at`); once it did (or didn't, within a few seconds), the test's back."""
    path = bridge.state_dir / "agent-tools.json"
    ours = hashlib.sha256(TOKEN.encode()).hexdigest()

    def theirs():
        try:
            return json.loads(path.read_text())["tokens"].get(agent) != ours
        except (OSError, ValueError, KeyError):
            return False
    wait_until(theirs, timeout=8)
    give_token(bridge, agent, TOKEN)


def tool(bridge):
    tools = rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]
    found = [t for t in tools if t["name"] == "offer_act"]
    return found[0] if found else None


def page_offer(**extra):
    return {"label": "Je te fais la liste des courses de la semaine ?", "topic": "Courses",
            "prompt": "Fais une page avec la liste des courses pour les menus de la semaine.",
            "uses": [{"tool": "create_page"}], **extra}


def any_of(key, **fields):
    return [TEXTS[lang][key].format(**fields) for lang in LANGS]


def look(bridge):
    response = bridge.post("/v1/proactive/passes", headers=AUTH)
    assert response.status_code == 200, response.text
    return response.json()["runs"]


def passes(fakes):
    return [c for c in fakes.model.chats if c["body"]["messages"][0]["content"].startswith("You look back over")]


def talk(n_person, minutes_ago=30.0, words="Je prépare le semi de Lyon en avril"):
    out = []
    for i in range(n_person):
        out.append({"role": "user", "content": f"{words} ({i})"})
        out.append({"role": "assistant", "content": f"D'accord ({i})."})
    return out


def add(fakes, session, messages, minutes_ago=30.0, **extra):
    fakes.hermes.add_session(session, messages, last_active=time.time() - minutes_ago * 60, agent="coach", **extra)


IDEA = {"label": "Je te cherche 3 idées de cadeau pour Léa, autour de 40 € ?", "topic": "Cadeau Léa",
        "prompt": "Cherche 3 idées de cadeau d'anniversaire pour Léa autour de 40 €, avec un lien chacune. N'achète rien.",
        "uses": [{"tool": "web_search"}], "conversation": "c1"}


def test_only_a_smart_agent_gets_the_free_form_and_a_basic_one_is_refused_it(bridges, fakes):
    bridge = place(bridges, fakes)
    basic = tool(bridge)
    assert basic is not None and "uses" not in basic["inputSchema"]["properties"]
    refused = call(bridge, "offer_act", page_offer(), token=TOKEN)
    assert not_done(refused) and text_of(refused).startswith("Not done: kind:"), text_of(refused)
    assert offer_cards(bridge) == []
    assert bridge.put("/v1/agents/coach/proactive", headers=AUTH, json={"level": "smart"}).status_code == 200
    settle_token(bridge)
    smart = tool(bridge)
    props = smart["inputSchema"]["properties"]
    assert {"uses", "later", "topic", "kind"} <= set(props) and props["later"]["items"]["enum"] == CASES["later"]
    assert "Your person set your « Initiative » to smart" in smart["description"]
    assert '{"tool": "create_page"} a page for them' in smart["description"], smart["description"]
    made = call(bridge, "offer_act", page_offer(), token=TOKEN)
    assert done(made), text_of(made)
    [card] = offer_cards(bridge)
    offer = card["bip_offer"]
    assert (offer["mode"], offer["kind"], offer["family"], offer["about"]) == ("smart", "smart", "page", "Courses")
    assert offer["uses_text"] in any_of("with", uses="une page") + any_of("with", uses="a page") \
        + any_of("with", uses="eine Seite") + any_of("with", uses="una página"), offer["uses_text"]
    assert (offer["later_text"], offer["site"], offer["site_text"], offer["idea"]) == (None, None, None, False)


def test_a_smart_offer_shows_only_if_every_use_is_present_and_says_what_is_missing(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    for uses, missing in [([{"connector": "garmin"}], "missing the connector « garmin »"),
                          ([{"tool": "image_generate_nope"}], "missing the tool image_generate_nope"),
                          ([{"mailbox": "", "do": "draft"}], "missing a mailbox drafts"),
                          ([{"calendar": "phone", "do": "write"}], "missing a calendar you may write in"),
                          ([{"skill": "recettes"}], "missing the skill « recettes »"),
                          ([{"browser": True, "site": "www.decathlon.fr"}], "missing your browser")]:
        refused = call(bridge, "offer_act", page_offer(uses=uses), token=TOKEN)
        assert not_done(refused) and missing in text_of(refused) and "don't offer it" in text_of(refused), text_of(refused)
    assert offer_cards(bridge) == []


def test_paying_or_sending_is_never_the_offer_only_a_later_step_said_on_the_card(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    for t in ("pay_with_card", "mail_send", "calendar_delete", "tell_family", "terminal"):
        refused = call(bridge, "offer_act", page_offer(uses=[{"tool": t}]), token=TOKEN)
        assert not_done(refused) and "is never the offer itself" in text_of(refused), text_of(refused)
    made = call(bridge, "offer_act", page_offer(later=["pay", "send"]), token=TOKEN)
    assert done(made), text_of(made)
    [card] = offer_cards(bridge)
    amber = card["bip_offer"]["later_text"]
    assert amber in ["Il te demandera ton accord avant de payer ou d’envoyer.", "It will ask you before it pays or sends.",
                     "Es fragt dich, bevor es bezahlt oder sendet.", "Te pedirá permiso antes de pagar o enviar."], amber
    # an app 1.0: the prompt, then the lines, under « i »; an app 1.1: its card draws them, `detail` under « i »
    prompt = page_offer()["prompt"]
    assert card["bip_detail"].startswith(f"{prompt}\n{card['bip_offer']['uses_text']}\n{amber}"), card["bip_detail"]
    assert card["bip_offer"]["detail"].startswith(prompt) and amber not in card["bip_offer"]["detail"]
    # its « Oui » grants nothing of it: the turn reads that those keep their own card
    assert answer(bridge, card, "once").json()["by"] == "agent"

    def turns():
        return [r for r in fakes.hermes.started.values() if "Your person said yes to your offer" in (r["input"] or "")]
    assert wait_until(turns, timeout=20)
    assert "keeps its own card later (your person's yes to this offer grants none of it)" in turns()[0]["input"]


def test_a_routine_offer_prints_its_rhythm_and_end_and_yes_starts_it(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    ends = time.strftime("%Y-%m-%d", time.localtime(time.time() + 40 * 86400))
    made = call(bridge, "offer_act", page_offer(
        label="Je te fais un plan d'entraînement, avec un point chaque dimanche ?", topic="Semi de Lyon",
        prompt="Fais une page avec un plan d'entraînement de 8 semaines, et une routine qui fait le point chaque dimanche.",
        uses=[{"tool": "create_page"}, {"routine": {"schedule": "every sunday at 19:00", "ends": ends}}]), token=TOKEN)
    assert done(made), text_of(made)
    [card] = offer_cards(bridge)
    with_line = card["bip_offer"]["uses_text"]
    assert any(w in with_line for w in ("une routine chaque dimanche vers 19 h, jusqu'au", "a routine every Sunday around 7 PM, until",
                                       "eine Routine jeden Sonntag gegen 19 Uhr, bis", "una rutina cada domingo hacia las 19, hasta el")), with_line
    assert answer(bridge, card, "once").status_code == 200

    def turns():
        return [r for r in fakes.hermes.started.values() if "Your person said yes to your offer" in (r["input"] or "")]
    assert wait_until(turns, timeout=20)
    assert "your person's yes starts it" in turns()[0]["input"]
    # in that turn, the agent makes the routine the card showed: on at once, no card of its own
    other = call(bridge, "routine_schedule", {"action": "create", "name": "Point du samedi", "schedule": "every saturday at 19:00",
                                              "prompt": "Fais le point de la semaine.", "until": ends}, token=TOKEN)
    assert "not running" in text_of(other), text_of(other)
    started = call(bridge, "routine_schedule", {"action": "create", "name": "Point du dimanche", "schedule": "every sunday at 19:00",
                                                "prompt": "Fais le point de la semaine d'entraînement.", "until": ends}, token=TOKEN)
    assert "Routine made and started" in text_of(started), text_of(started)
    jobs = {j["name"]: j for j in bridge.get("/v1/agents/coach/routines", headers=AUTH).json()["jobs"]}
    assert jobs["Point du dimanche"]["enabled"] is True and jobs["Point du dimanche"]["waits_for_answer"] is False
    assert jobs["Point du samedi"]["waits_for_answer"] is True
    # once
    again = call(bridge, "routine_schedule", {"action": "create", "name": "Point bis", "schedule": "every sunday at 19:00",
                                              "prompt": "Encore.", "until": ends}, token=TOKEN)
    assert "not running" in text_of(again)


def test_two_no_thanks_in_a_family_quiet_it_and_the_line_reaches_the_tool(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    for topic in ("Courses", "Menus"):
        assert done(call(bridge, "offer_act", page_offer(topic=topic), token=TOKEN))
        [card] = offer_cards(bridge)
        assert answer(bridge, card, "deny").status_code == 200
    refused = call(bridge, "offer_act", page_offer(topic="Budget"), token=TOKEN)
    assert not_done(refused) and "said no thanks to your pages" in text_of(refused), text_of(refused)
    settings = bridge.get("/v1/agents/coach/settings", headers=AUTH).json()
    assert settings["proactive"]["level"] == "smart"
    # the tool's description says it once a threshold passes (3 answers): not yet, and its pages are out of the learning
    assert "often" not in tool(bridge)["description"]
    kept = (bridge.state_dir / "offers.json").read_text()
    assert "Courses" not in kept and "Menus" not in kept


def test_a_pass_is_due_only_with_something_new_and_places_one_idea_at_once(bridges, fakes):
    bridge = smart_place(bridges, fakes, gap_hours=0)
    pushed = len(fakes.apns.payloads())
    assert look(bridge) == [], "the first look marks where reading starts"
    # a quiet day: two messages, nothing else
    add(fakes, "s1", talk(2, words="Il fait beau"))
    [run] = look(bridge)
    assert (run["reason"], run["model_called"]) == ("nothing new", False) and passes(fakes) == []
    # three more, the conversation quiet for 30 min
    first = talk(2, words="Il fait beau") + talk(3, words="C'est l'anniversaire de Léa dimanche")
    add(fakes, "s1", first)
    fakes.model.idea_lines = [dict(IDEA, uses=[{"connector": "amazon"}]), IDEA]
    [run] = look(bridge)
    assert (run["reason"], run["model_called"], run["candidates"], run["refused"], run["placed"]) == \
        ("placed", True, 2, 1, True), run
    [chat] = passes(fakes)
    sent = chat["body"]["messages"][1]["content"]
    assert chat["body"]["model"] == "small-eu" and chat["authorization"] == "Bearer sk-nb"
    assert "tools" not in chat["body"]
    assert "anniversaire de Léa dimanche (2)" in sent
    assert sent.index("<<<DATA") < sent.index("anniversaire") < sent.index("DATA>>>")
    # the idea: a card at once, in the conversation it is about, pushed
    [card] = offer_cards(bridge)
    assert card["bip_offer"]["idea"] is True and card["bip_offer"]["mode"] == "smart"
    assert card["bip_offer"]["label"] == IDEA["label"]
    assert wait_until(lambda: [p for p in fakes.apns.payloads()[pushed:] if p.get("r") == card["run_id"]], timeout=20)
    # what it cost, kept in numbers
    view = bridge.get("/v1/proactive/passes", headers=AUTH).json()
    assert (view["passes"], view["model_calls"]) == (1, 1) and view["spent"] > 0 and view["cap"] == 0.10
    assert view["runs"][-1]["tokens_in"] == run["tokens_in"] > 0
    kept = (bridge.state_dir / "offer_ideas.json").read_text()
    assert "Léa" not in kept and "anniversaire" not in kept, "numbers only"
    print(f"\nMEASURE one pass: {len(chat['body']['messages'][0]['content']) + len(sent)} chars sent, "
          f"{run['tokens_in']} tokens in, {run['tokens_out']} out, {run['eur']} €")
    # its card waiting: no other pass (« une seule en attente »)
    add(fakes, "s1", first + talk(3, words="Nouveau message"))
    fakes.model.idea_lines = []
    [run] = look(bridge)
    assert run["reason"] == "caps" and len(passes(fakes)) == 1, run
    # answered: the next pass reads only what is new since the last one
    assert answer(bridge, card, "deny").status_code == 200
    [run] = look(bridge)
    assert run["reason"] == "nothing" and len(passes(fakes)) == 2, run
    sent = passes(fakes)[-1]["body"]["messages"][1]["content"]
    assert "Nouveau message" in sent and "anniversaire" not in sent and "Il fait beau" not in sent, sent
    # nothing new since: no other call
    [run] = look(bridge)
    assert run["reason"] == "nothing new" and len(passes(fakes)) == 2


def test_never_a_pass_while_the_conversation_goes_on_and_never_more_than_the_day_allows(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    look(bridge)
    add(fakes, "s1", talk(4), minutes_ago=5)
    [run] = look(bridge)
    assert run["reason"] == "conversation going on" and passes(fakes) == []
    add(fakes, "s1", talk(4), minutes_ago=30)
    [run] = look(bridge)
    assert run["reason"] == "nothing", run   # the model answered nothing: the most common answer
    add(fakes, "s1", talk(8), minutes_ago=30)
    [run] = look(bridge)
    assert run["reason"] == "too soon" and len(passes(fakes)) == 1, run


def test_at_the_places_cap_passes_stop_silently_and_end_of_turn_offers_go_on(bridges, fakes):
    bridge = smart_place(bridges, fakes, passes_cap=0.00001)
    look(bridge)
    add(fakes, "s1", talk(4))
    pushed = len(fakes.apns.payloads())
    [run] = look(bridge)
    assert (run["reason"], run["model_called"]) == ("budget of the day", False) and passes(fakes) == []
    assert len(fakes.apns.payloads()) == pushed and offer_cards(bridge) == [], "nothing said to the person"
    assert done(call(bridge, "offer_act", page_offer(), token=TOKEN)), "the offers of a turn go on"


def test_a_pass_never_reads_health_rooms_or_mail_bodies_and_never_goes_to_a_model_outside(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    look(bridge)
    health = [{"role": "user", "content": "Ma tension ce matin ?"},
              {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "mcp_bipagents_health_read", "arguments": "{}"}}]},
              {"role": "assistant", "content": "Tension 13/8, pouls 61."}]
    mail = [{"role": "user", "content": "Lis le mail de la mairie"},
            {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "mcp_bipagents_mail_read", "arguments": "{}"}}]},
            {"role": "assistant", "content": "La mairie dit que le dossier 4471 est prêt."}]
    add(fakes, "s1", talk(3) + health + mail)
    add(fakes, "room", talk(5, words="Secret du salon"), title="BipAgents · discussion r1", hidden=True)
    [run] = look(bridge)
    assert run["model_called"] is True, run
    sent = passes(fakes)[0]["body"]["messages"][1]["content"]
    assert "Je prépare le semi" in sent and "Lis le mail de la mairie" in sent
    for secret in ("13/8", "pouls", "Tension", "4471", "Secret du salon"):
        assert secret not in sent, secret
    # its model outside our European providers or the person's own: no pass at all
    profile = bridge.root / ".hermes" / "profiles" / "coach" / "config.yaml"
    before_text = profile.read_text() if profile.exists() else ""
    assert "\nmodel:" not in "\n" + before_text
    profile.write_text(before_text + "\nmodel:\n  provider: openai\n  default: gpt-5\n")
    add(fakes, "s2", talk(5))
    before = len(passes(fakes))
    time.sleep(0.2)
    assert look(bridge) == [] and len(passes(fakes)) == before


def test_a_basic_or_off_agent_never_gets_a_pass(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    assert bridge.put("/v1/agents/coach/proactive", headers=AUTH, json={"level": "basic"}).status_code == 200
    add(fakes, "s1", talk(6))
    assert look(bridge) == [] and passes(fakes) == []
    assert bridge.put("/v1/agents/coach/proactive", headers=AUTH, json={"level": "off"}).status_code == 200
    assert look(bridge) == [] and passes(fakes) == []


def test_an_app_1_0_draws_a_smart_offer_as_the_generic_card_it_already_draws(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    assert done(call(bridge, "offer_act", search(), token=TOKEN))
    [basic] = offer_cards(bridge)
    assert basic["bip_offer"]["mode"] == "basic"
    assert answer(bridge, basic, "deny").status_code == 200
    assert done(call(bridge, "offer_act", page_offer(), token=TOKEN))
    [smart] = offer_cards(bridge)
    assert {k: v for k, v in shape(smart).items() if k != "bip_offer"} == {k: v for k, v in shape(basic).items() if k != "bip_offer"}
    assert set(basic["bip_offer"]) <= set(smart["bip_offer"]), "additions only"
    assert smart["choices"] == ["once", "deny"] and smart["bip_summary"].endswith(page_offer()["label"])
    assert answer(bridge, smart, "deny").status_code == 200
