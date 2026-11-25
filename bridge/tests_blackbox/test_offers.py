"""Offers to act in basic « Initiative » (proactive agents, branch 6: bridge-rs/src/plans/offers.rs; passation-pro
briefs/design-agents-proactifs.md §3.3; contracts/offers/cases.json): « Je regarde les hôtels ? », « Je te mets un
rappel ? », « Je cherche le trajet ? ». ONE card (`bip-offer-<id>`), « Oui » / « Non merci »; only the kinds the agent
can do here, now (a closed list of seven); nothing done nor paid for before « Oui »; then the bridge does it itself
(route, reminder, calendar: no model) or the agent gets one turn with exactly the words under « i » (search, draft,
page, cart: drafts only); the caps and the learning per kind keep it from bothering; only the person answers; an app
1.0 sees its generic Yes / No card."""
import json
import os
import re
from zoneinfo import ZoneInfo
import threading
import time
from pathlib import Path

from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token, not_done, rpc, text_of
from bbkit.servers import wait_until
from test_plans import answer, done, epoch, home_place, local, place, plan_cards, plan_jobs, propose, trip, TOKEN

CASES = json.loads((Path(__file__).resolve().parents[2] / "contracts" / "offers" / "cases.json").read_text())
TEXTS = CASES["texts"]
LANGS = ("fr", "en", "de", "es")
#: What an app 1.0 already draws as its generic card: a plan's, recorded on the base (task-frame e049880ec, the bridge
#: of this branch's first commit) with OFFERS_RECORD=1.
RECORDED = Path(__file__).parent / "fixtures" / "offers_app_1_0.json"
PROMPT = "Cherche 3 hôtels près de Baixa à Lisbonne, moins de 150 € la nuit, avec les avis. Ne réserve rien."
LABEL = "Je regarde les hôtels près de Baixa ?"
VIE = "vie-token-" + "v" * 30
BUDGET = "budget-token-" + "b" * 30


def search(label=LABEL, about="Lisbonne", prompt=PROMPT):
    return {"kind": "search", "label": label, "about": about, "prompt": prompt}


def offer_cards(bridge, agent="coach"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [c for c in items if c["run_id"].startswith("bip-offer-")]


def offer_tool(bridge, token=TOKEN):
    tools = rpc(bridge, "tools/list", token=token).json()["result"]["tools"]
    found = [t for t in tools if t["name"] == "offer_act"]
    return found[0] if found else None


def kinds(bridge, token=TOKEN):
    tool = offer_tool(bridge, token)
    return tool["inputSchema"]["properties"]["kind"]["enum"] if tool else []


def make(bridge, arguments, token=TOKEN, **kw):
    made = call(bridge, "offer_act", arguments, token=token, **kw)
    assert done(made), text_of(made)
    return made


def lang_of(card):
    """The card's language: the words between the agent's name and the question in its summary."""
    for lang in LANGS:
        if TEXTS[lang]["summary"].split("{agent}")[1].split("{question}")[0] in card["bip_summary"]:
            return lang
    raise AssertionError(card["bip_summary"])


def three(bridges, fakes, change=None):
    """A place with Coach, Vie and Budget, each with its own token."""
    bridge = place(bridges, fakes, agents=("coach", "vie", "budget"), change=change)
    give_token(bridge, "vie", VIE)
    give_token(bridge, "budget", BUDGET)
    return bridge


def test_offer_act_lists_only_the_kinds_this_agent_can_do_and_refuses_the_others(bridges, fakes):
    bridge = place(bridges, fakes)
    tool = offer_tool(bridge)
    assert tool is not None
    listed = kinds(bridge)
    # here: no street routing (the tests' default), no calendar, no payments; routines, the web, drafts and pages
    assert set(listed) == {"reminder", "search", "draft", "page"}, listed
    for kind in ("route", "calendar", "cart"):
        assert f"- {kind}:" not in tool["description"], tool["description"]
        refused = call(bridge, "offer_act", {"kind": kind, "label": "Je le fais ?", "about": "Test",
                                             "prompt": "x", "params": {"to": "Lyon"}, "site": "a.fr"}, token=TOKEN)
        assert not_done(refused) and "you can't" in text_of(refused) and "don't offer it" in text_of(refused), \
            text_of(refused)
    other = call(bridge, "offer_act", dict(search(), kind="book"), token=TOKEN)
    assert not_done(other) and text_of(other).startswith("Not done: kind:")
    assert offer_cards(bridge) == []
    # its phone calendar shared with it: it may add an event now (its tools' list changed: registered again)
    assert bridge.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["coach"]}).status_code == 200
    give_token(bridge, "coach", TOKEN)
    assert "calendar" in kinds(bridge)
    # a plan may carry offers of those kinds (plans.rs, after this branch)
    tools = rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]
    [plan_tool] = [t for t in tools if t["name"] == "plan_propose"]
    assert "offers" in plan_tool["inputSchema"]["properties"]


def test_an_offer_is_one_card_and_nothing_runs_before_yes(bridges, fakes):
    bridge = place(bridges, fakes)
    pushed = len(fakes.apns.payloads())
    made = make(bridge, search())
    assert "Offer card made" in text_of(made) and "nothing is done before" in text_of(made), text_of(made)
    [card] = offer_cards(bridge)
    texts = TEXTS[lang_of(card)]
    offer = card["bip_offer"]
    assert card["choices"] == ["once", "deny"] and card["request_id"] == offer["id"]
    assert card["bip_summary"] == texts["summary"].format(agent="Coach", question=LABEL)
    assert card["description"] == card["bip_summary"] and card["command"] == f"offer_act · {LABEL}"
    # under « i »: exactly what the agent will be asked, then where its suggestions are set (its first card)
    assert card["bip_detail"] == f"{PROMPT}\n{texts['first_card']}"
    assert (offer["kind"], offer["about"], offer["label"], offer["agent"], offer["plan"], offer["pill"],
            offer["mode"]) == ("search", "Lisbonne", LABEL, "Coach", None, False, "basic")
    assert abs(offer["expires_at"] - (time.time() + 86400)) < 60
    # pushed as a request (the neutral line), its run id to answer it
    assert wait_until(lambda: [p for p in fakes.apns.payloads()[pushed:] if p.get("r") == card["run_id"]], timeout=20)
    [push] = [p for p in fakes.apns.payloads()[pushed:] if p.get("r") == card["run_id"]]
    assert push["t"] == "mail_card", push
    # nothing ran, nothing was paid for
    time.sleep(0.5)
    assert fakes.hermes.started == {}
    # neither « Oui » nor « Non merci »: nothing
    assert answer(bridge, card, "maybe").status_code == 422
    assert answer(bridge, card, "deny").json() == {"ok": True, "choice": "deny"}
    assert offer_cards(bridge) == [] and answer(bridge, card, "once").status_code == 404
    time.sleep(0.5)
    assert fakes.hermes.started == {}
    kept = json.loads((bridge.state_dir / "offers.json").read_text())
    assert (bridge.state_dir / "offers.json").stat().st_mode & 0o777 == 0o600
    assert PROMPT not in json.dumps(kept, ensure_ascii=False)   # counters, no text kept once answered


def test_route_reminder_and_calendar_offers_run_without_the_model(bridges, fakes):
    def streets(c):
        c["maps"].update(streets="osrm", streets_url=fakes.maps.url)
    bridge = three(bridges, fakes, change=streets)
    # a reminder: a one-time routine whose words are the offer's
    at = local(2, 19, 0)
    make(bridge, {"kind": "reminder", "label": "Je te mets un rappel ?", "about": "Dentiste",
                  "params": {"at": at, "say": "Dentiste demain à 15:00."}})
    [card] = offer_cards(bridge)
    texts = TEXTS[lang_of(card)]
    assert card["bip_detail"].startswith(texts["detail_reminder"].split("{when}")[0]), card["bip_detail"]
    assert "Dentiste demain à 15:00." in card["bip_detail"]
    yes = answer(bridge, card, "once")
    assert yes.status_code == 200, yes.text
    body = yes.json()
    assert (body["ok"], body["choice"], body["kind"], body["by"]) == (True, "once", "reminder", "bridge"), body
    jobs = bridge.get("/v1/agents/coach/routines", headers=AUTH).json()["jobs"]
    [job] = [j for j in jobs if j["id"] == body["routine"]]
    assert job["say"] == "Dentiste demain à 15:00." and job["schedule"]["kind"] == "once", job
    # an event of its phone calendar: exactly the one on the card, through the phone's queue
    assert bridge.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["vie"]}).status_code == 200
    give_token(bridge, "vie", VIE)
    start = local(4, 15, 0)
    make(bridge, {"kind": "calendar", "label": "Je l’ajoute à ton agenda ?", "about": "Dentiste",
                  "params": {"title": "Dentiste", "start": start, "duration_minutes": 45,
                             "location": "Cabinet du Dr Martin"}}, token=VIE)
    [card] = offer_cards(bridge, "vie")
    texts = TEXTS[lang_of(card)]
    assert texts["phone_calendar"] in card["bip_detail"] and "Dentiste" in card["bip_detail"], card["bip_detail"]
    assert bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []
    assert answer(bridge, card, "once", agent="vie").json()["by"] == "bridge"
    assert wait_until(lambda: bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"], timeout=20)
    [queued] = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]
    assert queued["op"] == "add" and queued["args"]["title"] == "Dentiste", queued
    assert queued["args"]["start"].startswith(start), queued
    # a route: timed by the streets, one line where the card showed
    make(bridge, {"kind": "route", "label": "Je cherche le trajet ?", "about": "Dentiste",
                  "params": {"to": "Cabinet du Dr Martin", "from": "12 rue Haute, Lyon", "mode": "walking"}},
         token=BUDGET)
    [card] = offer_cards(bridge, "budget")
    texts = TEXTS[lang_of(card)]
    # nothing goes to a geocoder or a router before « Oui »
    assert fakes.maps.seen("/search") == [] and fakes.maps.seen("/routed-") == []
    assert answer(bridge, card, "once", agent="budget").json()["by"] == "bridge"
    line = texts["route_done"].format(place="Cabinet du Dr Martin", n=25, mode=texts["modes"]["walking"])
    assert wait_until(lambda: [m for m in boite_items(bridge, "budget") if m.get("text") == line], timeout=30), \
        [m.get("text") for m in boite_items(bridge, "budget")]
    # none of them called the model
    time.sleep(0.5)
    assert fakes.hermes.started == {}


def test_a_search_offer_starts_one_turn_with_its_prompt_and_drafts_only(bridges, fakes):
    bridge = place(bridges, fakes)
    make(bridge, search())
    [card] = offer_cards(bridge)
    yes = answer(bridge, card, "once")
    assert yes.json() == {"ok": True, "choice": "once", "kind": "search", "by": "agent"}, yes.text

    def turns():
        return [r for r in fakes.hermes.started.values() if "Your person said yes to your offer" in (r["input"] or "")]
    assert wait_until(turns, timeout=20)
    [turn] = turns()
    assert f"Your person said yes to your offer: « {PROMPT} ». Do exactly that now." in turn["input"], turn["input"]
    assert "Drafts only: never book, pay or send; anything like that goes through its own card." in turn["input"]
    # one turn, never twice (the card's own answer resumes nothing)
    time.sleep(3)
    assert len(turns()) == 1 and len(fakes.hermes.started) == 1


def test_two_no_thanks_in_a_row_quiet_the_kind_and_initiative_stays_basic(bridges, fakes):
    bridge = place(bridges, fakes)
    for about in ("Bagages", "Courses"):
        make(bridge, {"kind": "page", "label": "Je te fais la liste ?", "about": about,
                      "prompt": f"Fais une page avec la liste « {about} »."})
        [card] = offer_cards(bridge)
        assert answer(bridge, card, "deny").status_code == 200
    # that kind is quiet for this agent: out of its list, refused; the others stay
    assert "page" not in kinds(bridge) and "search" in kinds(bridge)
    refused = call(bridge, "offer_act", {"kind": "page", "label": "Je te fais les menus ?", "about": "Menus",
                                         "prompt": "Fais une page des menus de la semaine."}, token=TOKEN)
    assert not_done(refused) and "said no thanks" in text_of(refused), text_of(refused)
    # « Non merci » is never « Éteinte »
    settings = bridge.get("/v1/agents/coach/settings", headers=AUTH).json()
    assert settings["proactive"]["level"] == "basic", settings["proactive"]
    # no text of those offers kept, counters only
    kept = (bridge.state_dir / "offers.json").read_text()
    assert "Bagages" not in kept and "liste" not in kept


def test_caps_two_offers_per_reply_one_unasked_waiting_shared_with_the_plans(bridges, fakes):
    bridge = place(bridges, fakes)
    # asked, in the person's own conversation: two offers per reply
    gate = threading.Event()
    fakes.hermes.runs["run_o"] = [sse("tool.started", run_id="run_o", tool="mcp_bipagents_offer_act", preview="x"),
                                  gate, sse("run.completed", run_id="run_o", output="Bon voyage !")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "coach", "run_id": "run_o", "session_id": "api_o"}).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/p/coach/v1/runs/run_o/events"))
    time.sleep(0.3)
    try:
        make(bridge, search(about="Lisbonne"), session="api_o")
        make(bridge, {"kind": "page", "label": "Je te fais la liste de bagages ?", "about": "Bagages",
                      "prompt": "Fais une page avec la liste de bagages pour Lisbonne."}, session="api_o")
        third = call(bridge, "offer_act", {"kind": "draft", "label": "Je prépare le message ?", "about": "Hôtel",
                                           "prompt": "Écris un brouillon pour demander une chambre calme."},
                     token=TOKEN, session="api_o")
        assert not_done(third) and "two offers per reply" in text_of(third), text_of(third)
    finally:
        gate.set()
    # shown where the person asked
    cards = offer_cards(bridge)
    assert len(cards) == 2
    for card in cards:
        assert answer(bridge, card, "deny").status_code == 200
    # unasked (from a conversation no turn of the person's runs in: a routine's, a watcher's…): one card waiting at a
    # time, plans and offers together
    unasked = "api_unasked"
    propose(bridge, trip(), session=unasked)
    waiting = call(bridge, "offer_act", search(about="Porto"), token=TOKEN, session=unasked)
    assert not_done(waiting) and "already waits" in text_of(waiting), text_of(waiting)
    [plan] = plan_cards(bridge)
    assert answer(bridge, plan, "deny").status_code == 200
    # the same offer is not made twice in 14 days, whatever the answer
    again = call(bridge, "offer_act", search(about="  lisbonne "), token=TOKEN, session=unasked)
    assert not_done(again) and "you offered" in text_of(again), text_of(again)
    # a plan and an offer unasked today: 2 for this agent, then no more before tomorrow
    make(bridge, search(about="Porto"), session=unasked)
    [card] = offer_cards(bridge)
    assert answer(bridge, card, "deny").status_code == 200
    third = call(bridge, "offer_act", {"kind": "draft", "label": "Je prépare la réponse ?", "about": "Madrid",
                                       "prompt": "Écris un brouillon de réponse à Madrid."}, token=TOKEN,
                 session=unasked)
    assert not_done(third) and "already made 2" in text_of(third), text_of(third)


def test_an_offer_expires_quietly_and_folds_into_passee(bridges, fakes):
    bridge = place(bridges, fakes)
    make(bridge, search())
    [card] = offer_cards(bridge)
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("r") == card["run_id"]], timeout=20)
    pushed = len(fakes.apns.payloads())
    # its 24 hours gone (as if): it goes without a word
    path = bridge.state_dir / "offers.json"
    kept = json.loads(path.read_text())
    for offer in kept["offers"]:
        offer["expires"] = time.time() - 1
    path.write_text(json.dumps(kept))
    assert wait_until(lambda: offer_cards(bridge) == [], timeout=20)
    time.sleep(1)
    assert len(fakes.apns.payloads()) == pushed
    assert answer(bridge, card, "once").status_code == 404
    # kept for the conversation as « expired » (an app 1.1 folds it into « Passée »), counted, nothing run
    kept = bridge.state_dir / "approvals" / "coach.jsonl"

    def rows():
        lines = kept.read_text().splitlines() if kept.exists() else []
        return [json.loads(line) for line in lines if card["run_id"] in line]
    assert wait_until(rows, timeout=20)
    [row] = rows()
    assert row["choice"] == "expired", row
    assert fakes.hermes.started == {}


def test_only_the_person_answers_an_offer(bridges, fakes):
    bridge = place(bridges, fakes)
    # « Seulement le critique »: an offer is critical, never answered for them
    assert bridge.put("/v1/approval-level", headers=AUTH, json={"level": "critical"}).status_code == 200
    make(bridge, search())
    time.sleep(1.5)
    [card] = offer_cards(bridge)
    assert "bip_always" not in card
    autos = bridge.get("/v1/approvals/auto", headers=AUTH).json()["items"]
    assert not [a for a in autos if a.get("run_id") == card["run_id"]]
    assert fakes.hermes.started == {}
    # « Initiative » off: the card waiting goes without a word, the tool is gone, a call refused
    assert bridge.put("/v1/agents/coach/proactive", headers=AUTH, json={"level": "off"}).status_code == 200
    give_token(bridge, "coach", TOKEN)
    assert wait_until(lambda: offer_cards(bridge) == [], timeout=20)
    assert offer_tool(bridge) is None
    refused = call(bridge, "offer_act", search(about="Porto"), token=TOKEN)
    assert not_done(refused) and "turned your suggestions off" in text_of(refused)
    # an agent other people talk to (a project agent): never listed, never made
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Voyage à Lisbonne", "lang": "fr",
                                                                  "ends": time.time() + 86400 * 10, "category": "creative"})
    assert made.status_code == 201, made.text
    project = made.json()["agent"]
    other = "project-token-" + "x" * 30
    give_token(bridge, project, other)
    assert offer_tool(bridge, other) is None
    assert not_done(call(bridge, "offer_act", search(), token=other)) and offer_cards(bridge, project) == []


def persons_turn(bridge, fakes, run="run_p", session="api_p"):
    """A turn of the person's own conversation, going on until the returned gate is set: calls in `session` are
    asked."""
    gate = threading.Event()
    fakes.hermes.runs[run] = [sse("tool.started", run_id=run, tool="mcp_bipagents_plan_propose", preview="x"),
                              gate, sse("run.completed", run_id=run, output="Bon voyage !")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "coach", "run_id": run, "session_id": session}).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen(f"/p/coach/v1/runs/{run}/events"))
    time.sleep(0.3)
    return gate


def test_a_plans_offers_are_pills_on_its_card_and_their_own_cards_for_an_app_1_0(bridges, fakes):
    """Asked in the person's own turn (unasked, a plan and its two pills would be 3 cards nobody asked for: choice 4)."""
    bridge = place(bridges, fakes)
    pushed = len(fakes.apns.payloads())
    gate = persons_turn(bridge, fakes)
    try:
        propose(bridge, trip(offers=[{"kind": "search", "label": "Je regarde les hôtels ?", "prompt": PROMPT},
                                     {"kind": "page", "label": "Je te fais la liste de bagages ?",
                                      "prompt": "Fais une page avec la liste de bagages pour 3 jours à Lisbonne."}]),
                session="api_p")
    finally:
        gate.set()
    [plan] = plan_cards(bridge)
    pills = offer_cards(bridge)
    assert sorted(plan["bip_plan"]["offers"]) == sorted(c["bip_offer"]["id"] for c in pills) and len(pills) == 2
    for pill in pills:
        assert pill["bip_offer"]["plan"] == plan["bip_plan"]["id"] and pill["bip_offer"]["pill"] is True
        assert pill["bip_offer"]["about"] == "Lisbonne"
    # one push: the plan's (an app 1.0 shows its offers as their own generic cards just below it)
    time.sleep(1)
    rs = [p.get("r") for p in fakes.apns.payloads()[pushed:] if p.get("t") == "mail_card"]
    assert rs == [plan["run_id"]], rs
    # a wrong kind refuses the plan
    wrong = call(bridge, "plan_propose", trip(title="Porto", offers=[{"kind": "cart", "label": "Panier ?",
                                                                      "prompt": "x", "site": "a.fr"}]), token=TOKEN,
                 session="api_unasked")
    assert not_done(wrong) and "offers" in text_of(wrong), text_of(wrong)
    # « Non merci » to the plan: its offers go with it
    assert answer(bridge, plan, "deny").status_code == 200
    assert wait_until(lambda: offer_cards(bridge) == [], timeout=10)


def test_a_moment_offer_places_its_card_at_its_time_without_the_model(bridges, fakes):
    bridge = place(bridges, fakes)
    label = "Il pleuvra à Lisbonne. Je te fais la liste de bagages ?"
    arguments = trip()
    arguments["moments"].append({"at": local(2, 18, 0), "what": "Liste de bagages",
                                 "offer": {"kind": "page", "label": label,
                                           "prompt": "Fais une page avec la liste de bagages pour 3 jours de pluie."}})
    propose(bridge, arguments)
    [plan] = plan_cards(bridge)
    assert f"Liste de bagages : {label}" in plan["bip_detail"], plan["bip_detail"]
    assert offer_cards(bridge) == []
    assert answer(bridge, plan, "once").status_code == 200
    [job] = [j for j in plan_jobs(bridge, plan["bip_plan"]["id"]) if "Liste de bagages" in j["name"]]
    # its time come (« Lancer maintenant » here): its card, no turn of the model
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: offer_cards(bridge), timeout=20)
    [card] = offer_cards(bridge)
    assert card["bip_offer"]["label"] == label and card["bip_offer"]["plan"] == plan["bip_plan"]["id"]
    assert card["bip_offer"]["about"] == "Lisbonne" and card["bip_offer"]["pill"] is False
    time.sleep(0.5)
    assert fakes.hermes.started == {}


VOLATILE = {"run_id", "request_id", "created_at", "id", "at", "session_id", "expires_at"}
HEX = re.compile(r"\b[0-9a-f]{12}\b")
#: A moment's day and time (« mar. 24 · 09:15 »): the day the tests run.
WHEN = re.compile(r"\S+ \d{1,2} · (\d{2}:\d{2}|\d{1,2}\u00a0h(\u00a0\d{2})?)")


def normal(value, key=None):
    if key in VOLATILE and value is not None:
        return "<v>"
    if isinstance(value, dict):
        return {k: normal(v, k) for k, v in value.items()}
    if isinstance(value, list):
        return [normal(v) for v in value]
    if isinstance(value, str):
        return WHEN.sub("<when>", HEX.sub("<hex>", value))
    if isinstance(value, (int, float)) and not isinstance(value, bool) and value > 1_600_000_000:
        return "<t>"
    return value


def shape(card):
    """What an app 1.0 reads of a card it doesn't know: its keys and their types."""
    return {k: type(v).__name__ for k, v in card.items()}


def test_an_app_1_0_draws_an_offer_as_the_generic_card_it_already_draws(bridges, fakes):
    """An app 1.0 draws any unknown `bip-` card as its generic card (its sentence, « i », « Oui » / « Non »): a plan's
    card today. The plan's card is the base's, an offer's has the same keys and types (but its own `bip_offer`)."""
    bridge = three(bridges, fakes)
    propose(bridge, trip())
    [plan] = plan_cards(bridge)
    seen = {"plan_card": normal(plan), "plan_shape": shape(plan)}
    if os.environ.get("OFFERS_RECORD"):
        RECORDED.write_text(json.dumps(seen, ensure_ascii=False, indent=1, sort_keys=True) + "\n")
        return
    base = json.loads(RECORDED.read_text())
    # the plan's card as an app 1.0 reads it: unchanged (`bip_plan` it ignores)
    mine = {k: v for k, v in seen["plan_card"].items() if k != "bip_plan"}
    assert mine == {k: v for k, v in base["plan_card"].items() if k != "bip_plan"}
    make(bridge, search(), token=VIE)
    [card] = offer_cards(bridge, "vie")
    generic = {k: v for k, v in base["plan_shape"].items() if k != "bip_plan"}
    assert {k: v for k, v in shape(card).items() if k != "bip_offer"} == generic
    assert card["choices"] == base["plan_card"]["choices"] == ["once", "deny"]
    # « Oui » / « Non » as an app 1.0 sends them, with nothing else
    assert answer(bridge, card, "once", agent="vie").status_code == 200


def test_review_a_moment_offer_answered_starts_no_turn_of_the_model(bridges, fakes):
    """Review of offers (23/11, R1): a moment's offer card, placed by the bridge at its time with no turn
    (plans/offers.rs `at_moment`), then answered « Non merci »: nothing to tell the agent, no turn of the model, as for
    offer_act's own cards (card_place.rs `taken`). Today `taken` is called before the card has a place (a no-op), so
    card_place.rs `watch` places it untaken and `settle` resumes the agent in a paid turn for that answer (the same
    for « Oui » to a route, a reminder or an event, and for its expiry)."""
    bridge = place(bridges, fakes)
    label = "Il pleuvra à Lisbonne. Je te fais la liste de bagages ?"
    arguments = trip()
    arguments["moments"].append({"at": local(2, 18, 0), "what": "Liste de bagages",
                                 "offer": {"kind": "page", "label": label,
                                           "prompt": "Fais une page avec la liste de bagages pour 3 jours de pluie."}})
    propose(bridge, arguments)
    [plan] = plan_cards(bridge)
    assert answer(bridge, plan, "once").status_code == 200
    [job] = [j for j in plan_jobs(bridge, plan["bip_plan"]["id"]) if "Liste de bagages" in j["name"]]
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: offer_cards(bridge), timeout=20)
    [card] = offer_cards(bridge)
    time.sleep(2)   # card_place.rs `watch` (every second) has seen it
    assert fakes.hermes.started == {}
    assert answer(bridge, card, "deny").status_code == 200
    # « Non merci »: 0 model (design §3.8); card_place.rs `settle` would resume a turn within a few seconds
    assert not wait_until(lambda: fakes.hermes.started, timeout=10), \
        [r.get("input", "")[:200] for r in fakes.hermes.started.values()]


# -- review of offers (23/11): the author's red tests -----------------------------------------------------------------


def moment_offer(kind="page", label="Il pleuvra à Lisbonne. Je te fais la liste de bagages ?"):
    """The Lisbon trip with a moment that offers, 2 days ahead at 18:00 (Lisbon's clock)."""
    arguments = trip()
    at = local(2, 18, 0, ZoneInfo("Europe/Lisbon"))
    arguments["moments"].append({"at": at, "what": "Liste de bagages",
                                 "offer": {"kind": kind, "label": label,
                                           "prompt": "Fais une page avec la liste de bagages pour 3 jours de pluie."}})
    return arguments, at


def test_r1_a_moment_offer_that_expires_starts_no_turn_of_the_model(bridges, fakes):
    bridge = place(bridges, fakes)
    arguments, _ = moment_offer()
    propose(bridge, arguments)
    [plan] = plan_cards(bridge)
    assert answer(bridge, plan, "once").status_code == 200
    [job] = [j for j in plan_jobs(bridge, plan["bip_plan"]["id"]) if "Liste de bagages" in j["name"]]
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: offer_cards(bridge), timeout=20)
    [card] = offer_cards(bridge)
    time.sleep(2)
    path = bridge.state_dir / "offers.json"
    kept = json.loads(path.read_text())
    for offer in kept["offers"]:
        offer["expires"] = time.time() - 1
    path.write_text(json.dumps(kept))
    log = bridge.state_dir / "approvals" / "coach.jsonl"
    assert wait_until(lambda: log.exists() and card["run_id"] in log.read_text(), timeout=20)
    assert not wait_until(lambda: fakes.hermes.started, timeout=10), \
        [r.get("input", "")[:200] for r in fakes.hermes.started.values()]


def test_r2_an_offer_card_takes_yes_or_no_never_toujours(bridges, fakes):
    """With calendar-always merged, an event's own card offers « Toujours autoriser »; an offer's card never does, an
    event's offer neither, and its answer is only « Oui » or « Non merci »."""
    bridge = place(bridges, fakes)
    assert bridge.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["coach"]}).status_code == 200
    give_token(bridge, "coach", TOKEN)
    make(bridge, {"kind": "calendar", "label": "Je l’ajoute à ton agenda ?", "about": "Dentiste",
                  "params": {"title": "Dentiste", "start": local(4, 15, 0), "duration_minutes": 45}})
    [card] = offer_cards(bridge)
    assert "bip_always" not in card and "bip_always_kind" not in card, card
    for word in ("always", "session"):
        assert answer(bridge, card, word).status_code == 422, word
    assert offer_cards(bridge) == [card]
    assert answer(bridge, card, "once").json()["by"] == "bridge"
    settings = bridge.get("/v1/agents/coach/settings", headers=AUTH).json()
    assert "calendar_write" not in json.dumps(settings.get("always", {})), settings.get("always")


def test_choice2_a_moment_offer_held_by_the_caps_waits_for_the_next_day(bridges, fakes):
    bridge = place(bridges, fakes)
    arguments, at = moment_offer()
    propose(bridge, arguments, session="api_unasked")
    [plan] = plan_cards(bridge)
    assert answer(bridge, plan, "once").status_code == 200
    # another card nobody asked for waits: the moment's offer may not come now
    make(bridge, search(), session="api_unasked")
    [job] = [j for j in plan_jobs(bridge, plan["bip_plan"]["id"]) if "Liste de bagages" in j["name"]]
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200

    import datetime
    tomorrow = epoch(at, ZoneInfo("Europe/Lisbon")) + 86400

    def moved():
        [j] = [j for j in plan_jobs(bridge, plan["bip_plan"]["id"]) if j["id"] == job["id"]]
        nxt = j.get("next_run_at")
        return nxt and abs(datetime.datetime.fromisoformat(nxt).timestamp() - tomorrow) < 60
    assert wait_until(moved, timeout=20), plan_jobs(bridge, plan["bip_plan"]["id"])
    assert [c["bip_offer"]["kind"] for c in offer_cards(bridge)] == ["search"]
    assert fakes.hermes.started == {}


def test_choice4_an_unasked_plans_pills_count_in_the_day(bridges, fakes):
    bridge = place(bridges, fakes)
    pills = [{"kind": "search", "label": "Je regarde les hôtels ?", "prompt": PROMPT},
             {"kind": "page", "label": "Je te fais la liste de bagages ?", "prompt": "Fais la liste de bagages."}]
    over = call(bridge, "plan_propose", trip(offers=pills), token=TOKEN, session="api_unasked")
    assert not_done(over) and "offers" in text_of(over), text_of(over)
    propose(bridge, trip(offers=pills[:1]), session="api_unasked")
    [plan] = plan_cards(bridge)
    assert answer(bridge, plan, "deny").status_code == 200
    third = call(bridge, "offer_act", search(about="Porto"), token=TOKEN, session="api_unasked")
    assert not_done(third) and "already made 2" in text_of(third), text_of(third)


def test_r8_a_route_from_home_is_found_only_at_yes(bridges, fakes):
    bridge = home_place(bridges, fakes)
    home = {"kind": "address", "label": "Domicile", "full_name": "Camille Exemple", "address_line1": "3 rue du Test",
            "city": "Lyon", "postal_code": "69001", "country": "fr", "agents": ["coach"]}
    assert bridge.post("/v1/vault", headers=AUTH, json=home).status_code == 201

    def searched():
        return [s.params.get("q", "") for s in fakes.maps.seen("/search")]
    make(bridge, {"kind": "route", "label": "Je cherche le trajet ?", "about": "Dentiste",
                  "params": {"to": "Cabinet du Dr Martin", "mode": "walking"}})
    [card] = offer_cards(bridge)
    texts = TEXTS[lang_of(card)]
    assert texts["home"] in card["bip_detail"] and "3 rue du Test" not in card["bip_detail"], card["bip_detail"]
    assert searched() == [] and not fakes.maps.seen("/routed-")
    assert answer(bridge, card, "once").json()["by"] == "bridge"
    start = texts["route_done"].split("{place}")[0] + "Cabinet du Dr Martin"
    assert wait_until(lambda: [m for m in boite_items(bridge) if (m.get("text") or "").startswith(start)], timeout=30)
    assert [q for q in searched() if "3 rue du Test" in q], searched()
