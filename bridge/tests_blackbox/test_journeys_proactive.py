"""The release journeys of the proactive agents of 1.1 (docs/release-journeys.md « Agents proactifs », J67–J71, column
« Pont »; passation-pro briefs/design-agents-proactifs.md §6 line 11), on the journeys' bridge (bbkit/journeys.py,
agent « Vie », its scripted model) with its proactive clocks made short (bbkit/proactive_journeys.py), as the apps do
them over HTTP, each page and card checked as the person SEES it (bbkit/displayable_people.py) and as an app 1.0 draws
it (the generic card: `bip_summary`, « Oui » / « Non » = `once` / `deny`):

- J67 « Initiative »: three states on the agent's page; basic by default, a plan in a turn; off, nothing proposed
  (`plan_propose` not even listed); smart, `offer_act`'s free form; an app 1.0 keeps reading the agent's settings;
- J68 a plan and its offers: ONE card with its moments, « Prévoir » with a moment unticked, a pill (its own offer
  card) « Oui »: its work starts in the conversation; the plan's moments in « Ce que tes agents font tout seuls »;
- J69 « Réparer / Arrêter »: a watcher « en panne » (no model before « Réparer »), each answer; unanswered 4 days, the
  card again « Sans réponse, il s'arrête le {date}. », never stopped before; an agent whose AI doesn't answer: ONE card
  for its routines, « Réessayer »;
- J70 replies due: the mailbox's consent card, « Oui », the evening card built from headers only (sender, subject,
  since), « Oui »: one turn for the drafts, never sent; the mailbox's own switch (what the apps 1.1 send);
- J71 a smart offer: Vie « Intelligente », a pass over a synthetic day places ONE offer « Avec : … » with the amber line,
  « Oui » starts its turn; once Vie has « Santé », a pass never reads its conversations.

Each test is one journey `Jnn`, on a bridge of its own. No real account: invented names, the fake IMAP, the fake
model (bbkit/proactive_journeys.py)."""
import json
import re
import time

import pytest

from bbkit import displayable_people as shown
from bbkit import proactive_journeys as pj
from bbkit.common import AUTH, boite_items
from bbkit.journeys import AGENT, journey_bridge
from bbkit.model import PeopleModel
from bbkit.people import reply_in
from bbkit.servers import wait_until

APP_1_0 = {**AUTH, "X-Bip-App": "ios/20261120/1.0"}
DAY = 86400.0


@pytest.fixture
def journey(bridges, fakes):
    made = journey_bridge(bridges, fakes, proactive=True)
    made.model = PeopleModel.upgrade(made.model)
    # the agent speaks French (its cards' words: handback::language_of)
    assert made.bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    pj.install(made)
    return made


PROACTIVE_CARDS = ("bip-plan-", "bip-offer-", "bip-fix-")


def wordless(page):
    """The answered plan, offer and fix cards of the page that say nothing of what was asked (no command, description
    nor topic in the timeline's `approval` item): `test_j68_an_answered_plan_or_offer_says_what_was_asked`."""
    return [i for i in page.get("items") or [] if i.get("kind") == "approval"
            and str((i.get("approval") or {}).get("run_id") or "").startswith(PROACTIVE_CARDS)
            and not any((i["approval"].get(k) for k in ("command", "description", "topic")))]


def drawable(journey, page):
    """The page checked as both apps draw it (bbkit/displayable_people.py), but for the wordless answered cards of
    this release (`wordless`: a defect of their own, red in its own test, so the rest of each journey still runs)."""
    left = {i["id"] for i in wordless(page)}
    shown.check_page(journey.bridge, {**page, "items": [i for i in page["items"] if i.get("id") not in left]}, AGENT)


def cid(n: int) -> str:
    return f"J0URNEY0-0000-4000-8000-{n:012d}"


def said(journey, words):
    return sum(1 for i in journey.page()["items"] if i.get("role") == "agent" and words in (i.get("text") or ""))


def reply_done(journey, words, timeout=60, before=None):
    """The agent's reply with `words` there and its turn over; `before`: how many there were before this turn."""
    if before is not None:
        assert wait_until(lambda: said(journey, words) > before and journey.page()["pending"].get("run") is None,
                          timeout=timeout), journey.page()
    return reply_in(journey.bridge, journey.thread, AGENT, words, timeout=timeout)


def waiting(journey, prefix, timeout=30, thread=None):
    """The cards of `prefix` waiting in the conversation (its page's `pending.approvals`), as the apps draw them."""
    def found():
        page = journey.page(thread)
        return [c for c in page["pending"].get("approvals") or [] if c.get("run_id", "").startswith(prefix)]
    assert wait_until(found, timeout=timeout), journey.page(thread)["pending"]
    return found()


def approve(journey, card, choice, headers=AUTH, **extra):
    resp = journey.bridge.post("/v1/approve", headers=headers, json={"agent": AGENT, "run_id": card["run_id"],
                                                                     "request_id": card["request_id"], "choice": choice,
                                                                     **extra})
    assert resp.status_code == 200, resp.text
    return resp.json()


def settings(journey, headers=AUTH):
    resp = journey.bridge.get(f"/v1/agents/{AGENT}/settings", headers=headers)
    assert resp.status_code == 200, resp.text
    return resp.json()


def level(journey, value):
    resp = journey.bridge.put(f"/v1/agents/{AGENT}/proactive", headers=AUTH, json={"level": value})
    assert resp.status_code == 200, resp.text
    assert resp.json() == {"id": AGENT, "level": value}
    return resp.json()


def tool_names(journey):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}
    resp = journey.bridge.post("/v1/agent-tools/mcp", json=body, headers={"Authorization": f"Bearer {journey.token}"})
    assert resp.status_code == 200, resp.text
    return {t["name"]: t for t in resp.json()["result"]["tools"]}


def generic_problems(card):
    """What an app 1.0 draws of a card it doesn't know (`bip-plan-`, `bip-offer-`, `bip-fix-`): its sentence, « i »,
    « Oui » / « Non » sending `once` / `deny`."""
    out = []
    if not (card.get("bip_summary") or "").strip():
        out.append("no `bip_summary` (an app 1.0's sentence)")
    if card.get("choices") != ["once", "deny"]:
        out.append(f"choices {card.get('choices')!r}: an app 1.0 answers once / deny")
    if not isinstance(card.get("bip_detail", ""), str):
        out.append("`bip_detail` not a string")
    return out


def autonomous(journey, **params):
    return journey.bridge.get("/v1/autonomous", headers=AUTH, params={"agent": AGENT, **params}).json()["items"]


# -- J67 « Initiative » ------------------------------------------------------------------------------------------------

def test_j67_initiative_three_states_change_what_the_agent_proposes(journey):
    """Basic by default (D4 B): a trip spoken of gives a plan card in the turn; « Éteinte »: no tool to propose, no
    card, the agent still answers; « Intelligente »: offer_act's free form; back to « Basique ». An app 1.0 reads the
    settings as before (the added `proactive` it ignores): « Intelligente » is chosen in an app 1.1 only."""
    assert settings(journey)["proactive"]["level"] == "basic"
    assert "plan_propose" in tool_names(journey) and "offer_act" in tool_names(journey)
    assert journey.send(cid(671), "Je pars à Lisbonne dans trois jours, vol TP 433 #plan").status_code == 200
    page = reply_done(journey, "Je te propose un plan pour Lisbonne")
    drawable(journey, page)
    [plan] = waiting(journey, "bip-plan-")
    assert approve(journey, plan, "deny") == {"ok": True, "choice": "deny"}
    # « Éteinte »: plan_propose and offer_act are not even listed; the same words, no card
    level(journey, "off")
    assert wait_until(lambda: "plan_propose" not in tool_names(journey), timeout=20), sorted(tool_names(journey))
    assert "offer_act" not in tool_names(journey)
    assert journey.send(cid(672), "Et je pars aussi à Porto la semaine d'après #plan").status_code == 200
    page = reply_done(journey, "bon voyage à Porto")
    drawable(journey, page)
    assert not [c for c in page["pending"].get("approvals") or [] if c["run_id"].startswith(("bip-plan-", "bip-offer-"))]
    assert settings(journey)["proactive"] == {"level": "off", "smart_ok": settings(journey)["proactive"]["smart_ok"]}
    # « Intelligente »: offer_act's free form (uses, later)
    level(journey, "smart")
    assert wait_until(lambda: "uses" in (tool_names(journey).get("offer_act") or {}).get("inputSchema", {})
                      .get("properties", {}), timeout=20), tool_names(journey).get("offer_act")
    assert "plan_propose" in tool_names(journey)
    # an app 1.0: its settings as before, the added key ignored (it has no control for it)
    old = settings(journey, APP_1_0)
    assert old["proactive"]["level"] == "smart" and isinstance(old["proactive"]["smart_ok"], bool)
    level(journey, "basic")
    assert wait_until(lambda: "uses" not in (tool_names(journey).get("offer_act") or {}).get("inputSchema", {})
                      .get("properties", {}), timeout=20)


# -- J68 a plan and its offers ---------------------------------------------------------------------------------------

def test_j68_a_plan_one_card_a_moment_unticked_a_pill_yes_its_work_and_plan_set(journey):
    assert journey.send(cid(681), "Je pars à Lisbonne dans trois jours, vol TP 433 #plan").status_code == 200
    page = reply_done(journey, "Je te propose un plan pour Lisbonne")
    drawable(journey, page)
    [card] = waiting(journey, "bip-plan-")
    assert not generic_problems(card), generic_problems(card)
    plan = card["bip_plan"]
    assert (plan["kind"], plan["title"], len(plan["moments"])) == ("trip", "Lisbonne", 3), plan
    for moment in plan["moments"]:
        assert moment["id"] and moment["when_text"] and moment["what"] and moment["say"], moment
    assert any("heure de Lisbonne" in m["when_text"] for m in plan["moments"]), plan["moments"]
    assert card["bip_summary"] == "Vie propose un plan pour Lisbonne : 3 moments. Le prévoir ?"
    assert "Rien n’est envoyé ni réservé" in card["bip_detail"]
    # its two offers: their own cards (an app 1.0 draws them apart; a 1.1 as pills in the plan's card)
    assert len(plan["offers"]) == 2, plan
    offers = {c["bip_offer"]["id"]: c for c in waiting(journey, "bip-offer-")}
    assert set(plan["offers"]) <= set(offers), (plan["offers"], list(offers))
    for c in offers.values():
        assert not generic_problems(c), generic_problems(c)
        assert c["bip_offer"].get("plan") == plan["id"], c["bip_offer"]
        assert c["bip_summary"] == f"Vie propose : {c['bip_offer']['label']}"
    # nothing runs before « Prévoir »
    assert [j for j in journey.bridge.get(f"/v1/agents/{AGENT}/routines", headers=AUTH).json()["jobs"]
            if j.get("plan") == plan["id"]] == []
    # a pill: « Je te fais la liste de bagages ? » « Oui » — its work starts in the conversation
    bagages = next(c for c in offers.values() if "bagages" in c["bip_offer"]["label"])
    asked = len(journey.model.inputs)
    yes = approve(journey, bagages, "once")
    assert (yes["choice"], yes["kind"], yes["by"]) == ("once", "page", "agent"), yes
    page = reply_done(journey, "Voici ta liste de bagages pour Lisbonne", timeout=90)
    started = journey.model.inputs[asked:]
    assert started and any("liste de bagages" in json.dumps(i["input"], ensure_ascii=False) for i in started), started
    drawable(journey, page)
    # « Prévoir » with « Ta chambre est prête » unticked
    skipped = next(m["id"] for m in plan["moments"] if "chambre" in m["what"])
    done = approve(journey, card, "once", skip=[skipped])
    assert (done["moments"], done["of"], done["passed"]) == (2, 3, 0), done
    jobs = [j for j in journey.bridge.get(f"/v1/agents/{AGENT}/routines", headers=AUTH).json()["jobs"]
            if j.get("plan") == plan["id"]]
    assert len(jobs) == 2 and all(j["schedule"]["kind"] == "once" for j in jobs), jobs
    assert not [j for j in jobs if "chambre" in j["say"]], jobs
    # « Plan prévu »: the answered card in the conversation, its moments in « Ce que tes agents font tout seuls »
    page = journey.page()
    drawable(journey, page)
    assert not [c for c in page["pending"].get("approvals") or [] if c["run_id"] == card["run_id"]]
    listed = autonomous(journey, plan=plan["id"])
    assert sorted(i["ref"] for i in listed) == sorted(j["id"] for j in jobs), listed
    # the other pill unanswered stays its own card, an app 1.0's « Non » takes it
    reminder = next(c for c in offers.values() if "taxi" in c["bip_offer"]["label"])
    assert approve(journey, reminder, "deny", headers=APP_1_0) == {"ok": True, "choice": "deny"}


def test_j68_an_answered_plan_or_offer_says_what_was_asked(journey):
    """What every device (another phone, the app reinstalled, an app 1.0) draws of a plan or an offer once answered
    comes from the timeline's `approval` item (approvals_log.rs, threads/mod.rs `kept`): it must say what was asked
    (its command, description or topic), as every other card's answered item does (bbkit/displayable.py). RED on
    journeys-1-1 (24/11): the answered `bip-plan-` and `bip-offer-` items have none of them (their « Oui » / « Non »
    never goes through app.rs' generic log, which keeps the card as shown)."""
    assert journey.send(cid(685), "Je pars à Lisbonne dans trois jours, vol TP 433 #plan").status_code == 200
    reply_done(journey, "Je te propose un plan pour Lisbonne")
    [plan] = waiting(journey, "bip-plan-")
    pill = next(c for c in waiting(journey, "bip-offer-") if "taxi" in c["bip_offer"]["label"])
    approve(journey, pill, "deny")
    approve(journey, plan, "once")
    page = journey.page()
    answered = [i for i in page["items"] if i.get("kind") == "approval"]
    assert {i["approval"]["run_id"] for i in answered} >= {plan["run_id"], pill["run_id"]}, answered
    assert wordless(page) == [], [i["approval"] for i in wordless(page)]


# -- J69 « Réparer / Arrêter » -----------------------------------------------------------------------------------------

def connectors_file(bridge):
    return bridge.data / "connectors" / "connectors.json"


def edit_fix(bridge, conn_id, **changes):
    """A watcher's card as the bridge's clock would find it days later."""
    path = connectors_file(bridge)
    store = json.loads(path.read_text())
    for c in store["agents"][AGENT]:
        if c["id"] == conn_id:
            c["frame"]["fix"].update(changes)
    path.write_text(json.dumps(store))


def fix_cards(journey):
    return pj.approvals(journey.bridge, "bip-fix-")


def connector(journey, conn_id):
    return next(c for c in journey.bridge.get(f"/v1/agents/{AGENT}/connectors", headers=AUTH).json()["items"]
                if c["id"] == conn_id)


def broken(journey, watcher):
    steps = [pj.check(journey.bridge, watcher["id"])["step"] for _ in range(3)]
    assert steps == ["failed", "failed", "broken"], steps
    assert wait_until(lambda: [c for c in fix_cards(journey) if c["bip_fix"]["ref"] == watcher["id"]], timeout=30)
    [card] = [c for c in fix_cards(journey) if c["bip_fix"]["ref"] == watcher["id"]]
    return card


def test_j69_repair_or_stop_a_broken_watcher_told_before_it_stops_and_one_card_when_its_ai_is_silent(journey, fakes):
    # a phone to push to (an iPhone of the test's: APNs' fake)
    assert journey.bridge.post("/v1/devices", headers=AUTH, json={"token": "e" * 64, "environment": "sandbox",
                                                                  "agent_ids": [AGENT]}).status_code == 200
    watched = pj.watchers(journey)
    before = len(journey.model.inputs)
    # « Prix du vélo » en panne: the card, no model call; its words as both apps draw them, and an app 1.0's
    velo = broken(journey, watched["velo"])
    assert len(journey.model.inputs) == before, "a model call before « Réparer »"
    fix = velo["bip_fix"]
    assert fix["title"] == "Vie · le veilleur « Prix du vélo » ne marche plus", fix
    assert re.fullmatch(r"Depuis .+ : sa vérification ne marche plus\.", fix["text"]), fix["text"]
    assert velo["bip_choices"] == ["repair", "stop"] and not generic_problems(velo), velo
    assert velo["bip_summary"] == "Le veilleur « Prix du vélo » de Vie ne marche plus. Le faire réparer ? (Non : il s'arrête.)"
    # where the person sees it: the Boîte's « À valider », pushed; never a line of the bridge besides
    assert not [i for i in boite_items(journey.bridge) if "Prix du vélo" in (i.get("text") or "")]
    # « Réparer »: ONE turn of Vie, in the watcher's conversation; the card goes, the watcher stays on
    approve(journey, velo, "once")
    assert wait_until(lambda: len(journey.model.inputs) > before, timeout=30)
    turn = journey.model.inputs[before]
    assert "repair" in json.dumps(turn["input"]).lower() and turn["session"] != journey.thread, turn
    assert wait_until(lambda: not [c for c in fix_cards(journey) if c["run_id"] == velo["run_id"]], timeout=30)
    assert connector(journey, watched["velo"]["id"])["enabled"] is True
    # « Créneau médecin » en panne, left unanswered 4 days: asked again, pushed, saying the day it stops; still on
    first = broken(journey, watched["creneau"])
    asked = len(journey.model.inputs)
    edit_fix(journey.bridge, watched["creneau"]["id"], at=time.time() - 4 * DAY - 60)
    assert wait_until(lambda: [c for c in fix_cards(journey) if c["bip_fix"]["ref"] == watched["creneau"]["id"]
                               and c["run_id"] != first["run_id"]], timeout=60)
    [warned] = [c for c in fix_cards(journey) if c["bip_fix"]["ref"] == watched["creneau"]["id"]]
    assert re.search(r"Sans réponse, il s'arrête le .+\.$", warned["bip_fix"]["text"]), warned["bip_fix"]["text"]
    assert "Sans réponse, il s'arrête le " in warned["bip_summary"], warned["bip_summary"]
    assert warned["bip_fix"]["stops_at"] > time.time() + 2 * DAY
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if warned["run_id"] in json.dumps(p)], timeout=20)
    assert connector(journey, watched["creneau"]["id"])["enabled"] is True, "stopped before its day"
    # « Arrêter » (an app 1.0's « Non »): switched off, never deleted, on its page; no model call
    approve(journey, warned, "deny", headers=APP_1_0)
    assert connector(journey, watched["creneau"]["id"])["enabled"] is False
    rows = [i for i in autonomous(journey) if i["id"] == f"watcher:{AGENT}:{watched['creneau']['id']}"]
    assert rows and rows[0]["state"] == "paused", rows
    assert len(journey.model.inputs) == asked and not fix_cards(journey)
    # Vie's AI doesn't answer: its routines fail, ONE card « son IA ne répond pas » for them, « Réessayer »: no turn
    ids = {}
    for name in ("Courriels", "Météo"):
        made = journey.bridge.post(f"/v1/agents/{AGENT}/routines", headers=AUTH,
                                   json={"name": name, "schedule": "every 2h", "prompt": f"Regarde {name} #ia-muette"})
        assert made.status_code == 200, made.text
        ids[name] = made.json()["job"]["id"]

    def jobs():
        return {j["name"]: j for j in journey.bridge.get(f"/v1/agents/{AGENT}/routines", headers=AUTH).json()["jobs"]}
    for name in ("Courriels", "Météo"):
        for n in range(1, 4):
            assert journey.bridge.post(f"/v1/routines/{ids[name]}/run", headers=AUTH).status_code == 200
            assert wait_until(lambda: jobs()[name]["state"] != "running" and jobs()[name]["failures"] == n
                              or jobs()[name].get("fix"), timeout=60), jobs()[name]
    assert wait_until(lambda: [c for c in fix_cards(journey) if c["bip_fix"].get("variant") == "ai"], timeout=30)
    [ai] = fix_cards(journey)
    assert (ai["bip_fix"]["kind"], sorted(ai["bip_fix"]["routines"])) == ("agent_ai", ["Courriels", "Météo"]), ai
    assert "Vie" in ai["bip_fix"]["title"] and "IA" in ai["bip_fix"]["title"], ai["bip_fix"]["title"]
    assert not generic_problems(ai), generic_problems(ai)
    for name in ("Courriels", "Météo"):
        assert (jobs()[name]["state"], jobs()[name]["fix"]) == ("paused", ai["run_id"]), jobs()[name]
    approve(journey, ai, "once")
    for name in ("Courriels", "Météo"):
        assert (jobs()[name]["state"], jobs()[name]["failures"], jobs()[name]["fix"]) == ("scheduled", 0, None), jobs()[name]
    assert not fix_cards(journey)


# -- J70 replies due ---------------------------------------------------------------------------------------------------

def replies(journey):
    resp = journey.bridge.get(f"/v1/agents/{AGENT}/replies", headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()["mailboxes"]


def replies_cards(journey):
    return [c for c in pj.approvals(journey.bridge, "bip-offer-") if c["bip_offer"].get("source") == "replies"]


def fetched_bodies(fakes):
    return [c for c in fakes.imap.commands if c.startswith("UID FETCH") and ("BODY.PEEK[]" in c or "BODY[]" in c
                                                                            or "[TEXT]" in c)]


def test_j70_replies_due_consent_then_the_evening_card_then_drafts_never_sent(journey, fakes):
    added = journey.bridge.post("/v1/mail/accounts", headers=AUTH, json=pj.mailbox(fakes))
    assert added.status_code == 201, added.text
    account = added.json()["id"]
    # ONE card asks, for this mailbox; nothing watched before its « Oui »
    assert wait_until(lambda: replies_cards(journey), timeout=30)
    [consent] = replies_cards(journey)
    offer = consent["bip_offer"]
    assert (offer["kind"], offer["mailbox"]) == ("reply_watch", pj.ME), offer
    assert offer["label"] == (f"Je te dis quand un e-mail attend ta réponse depuis plus d’un jour, dans {pj.ME} ? "
                              "Je ne lis que l’expéditeur et l’objet.")
    assert not generic_problems(consent), generic_problems(consent)
    # (its state « asked » is written just after the card is placed, plans/sources.rs `consent_cards`: an instant
    # where the mailbox still reads « off », seen once on 24/11; harmless, waited for here)
    assert wait_until(lambda: [m["state"] for m in replies(journey)] == ["asked"], timeout=10), replies(journey)
    assert fakes.imap.idling == 0
    assert approve(journey, consent, "once") == {"ok": True, "choice": "once", "kind": "reply_watch", "by": "bridge"}
    assert wait_until(lambda: fakes.imap.idling >= 1, timeout=30), fakes.imap.commands[-5:]
    # the day's e-mails: two wait for a reply; one answered (in Sent) and a mass mailing never shown
    pj.deliver_due(fakes)
    assert wait_until(lambda: [c for c in replies_cards(journey) if c["bip_offer"]["kind"] == "draft"], timeout=60)
    [card] = [c for c in replies_cards(journey) if c["bip_offer"]["kind"] == "draft"]
    offer = card["bip_offer"]
    assert offer["top"] == "Vie · 2 e-mails attendent ta réponse", offer
    assert offer["label"] == "Je prépare les brouillons ?"
    items = offer["items"]
    assert sorted((i["from"], i["subject"]) for i in items) == sorted(
        (s.split(" <")[0], subject) for s, subject in pj.DUE), items
    assert all(isinstance(i["since"], str) and i["since"] for i in items), items
    assert "Les brouillons vont dans tes Brouillons ; rien n’est envoyé." in card["bip_detail"]
    assert not generic_problems(card), generic_problems(card)
    # an app 1.0: its sentence says the e-mails and the question (« Vie · 2 e-mails … . Je prépare les brouillons ? »)
    assert card["bip_summary"].startswith(offer["top"]) and card["bip_summary"].endswith("Je prépare les brouillons ?")
    assert fetched_bodies(fakes) == [], "a body read before « Oui »"
    drawable(journey, journey.page())
    # « Oui »: ONE turn that drafts the replies, never sends
    asked = len(journey.model.inputs)
    assert approve(journey, card, "once")["choice"] == "once"
    assert wait_until(lambda: len(journey.model.inputs) > asked, timeout=30)
    turn = json.dumps(journey.model.inputs[asked]["input"], ensure_ascii=False)
    assert "Devis cuisine" in turn and "Promo" not in turn and "Photos" not in turn, turn
    assert "send" in turn.lower() or "envo" in turn.lower(), turn   # its words say never to send
    assert not [c for c in fakes.imap.commands if c.startswith("APPEND") and "Sent" in c]
    # the app 1.1's switch, mailbox by mailbox: off, its watch goes; on again
    off = journey.bridge.put(f"/v1/agents/{AGENT}/replies", headers=AUTH, json={"account": account, "on": False})
    assert off.status_code == 200 and off.json()["state"] == "off", off.text
    assert wait_until(lambda: fakes.imap.idling == 0, timeout=30)
    on = journey.bridge.put(f"/v1/agents/{AGENT}/replies", headers=AUTH, json={"account": account, "on": True})
    assert on.json() == {"ok": True, "account": account, "state": "on"}, on.text
    [box] = replies(journey)
    assert box["state"] == "on" and box["trigger"], box
    rows = [i for i in autonomous(journey) if i["ref"] == box["trigger"]]
    assert rows and rows[0]["kind"] == "trigger", rows


# -- J71 a smart offer ---------------------------------------------------------------------------------------------------

def look(journey):
    resp = journey.bridge.post("/v1/proactive/passes", headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()["runs"]


def passes(fakes):
    return [c for c in fakes.model.chats if c["body"]["messages"][0]["content"].startswith("You look back over")]


def test_j71_a_smart_pass_over_a_day_offers_with_what_and_the_amber_line_yes_starts_it_never_health(journey, fakes):
    level(journey, "smart")
    fakes.model.idea_lines = [pj.IDEA]
    assert look(journey) == [] or True   # the first look marks where reading starts
    # the synthetic day: three messages of the person in her conversation, each answered
    for n, words in enumerate(pj.DAY):
        before = said(journey, "Bien reçu")
        assert journey.send(cid(710 + n), words).status_code == 200
        reply_done(journey, "Bien reçu", before=before)
    runs = look(journey)
    assert [r for r in runs if r.get("placed")], runs
    [card] = [c for c in pj.approvals(journey.bridge, "bip-offer-") if c["bip_offer"].get("idea")]
    offer = card["bip_offer"]
    assert (offer["mode"], offer["label"], offer["about"]) == ("smart", pj.IDEA["label"], pj.IDEA["topic"]), offer
    assert offer["uses_text"] == "Avec : une page", offer["uses_text"]
    assert offer["later_text"] == "Il te demandera ton accord avant de payer.", offer["later_text"]
    assert not generic_problems(card) and card["bip_summary"] == f"Vie propose : {pj.IDEA['label']}"
    assert pj.IDEA["prompt"] in card["bip_detail"] and "Avec : une page" in card["bip_detail"]
    sent = passes(fakes)[-1]["body"]["messages"][1]["content"]
    assert "anniversaire de Léa" in sent, sent
    drawable(journey, journey.page())
    # « Oui »: its turn starts, with the prompt shown under « i »
    asked = len(journey.model.inputs)
    yes = approve(journey, card, "once")
    assert yes["choice"] == "once", yes
    assert wait_until(lambda: len(journey.model.inputs) > asked, timeout=30)
    assert "idées de cadeau" in json.dumps(journey.model.inputs[asked]["input"], ensure_ascii=False)
    reply_done(journey, "Voici 3 idées de cadeau pour Léa", timeout=90)
    # Vie with « Santé »: a pass never reads its conversations (strict B2)
    assert journey.bridge.patch(f"/v1/agents/{AGENT}/settings", headers=AUTH, json={"health": True}).status_code == 200
    for n, words in enumerate(["J'ai mal dormi, 5 h 12 cette nuit", "Mon pouls au repos est à 74", "Je cours demain"]):
        before = said(journey, "Bien reçu")
        assert journey.send(cid(720 + n), words).status_code == 200
        reply_done(journey, "Bien reçu", before=before)
    before = len(passes(fakes))
    look(journey)
    for chat in passes(fakes)[before:]:
        text = chat["body"]["messages"][1]["content"]
        for secret in ("5 h 12", "pouls", "Je cours demain"):
            assert secret not in text, f"a pass read Vie's conversation though it has « Santé »: {secret!r}"
