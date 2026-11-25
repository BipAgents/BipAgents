"""Alex's choices of 24/11 on the texts of 1.1 (passation-pro textes-1-1/decisions.md), each on the running bridge:

1. a watcher never stops without its person: its card « Réparer / Arrêter » unanswered is asked again, pushed, saying
   the day it stops (tf-ligne-panne-veilleur-remplacee, « prévenir avant de l'arrêter »);
2. an agent whose AI doesn't answer has ONE card for its routines, not one per routine (tf-carte-commune-ia-eteinte B);
3. what its agent set up on its own never stops at its 90 days without its person: « Garder ? » again, pushed, then it
   goes on (tf-fin-par-defaut-de-lui-meme B2);
4. a basket is offered without a payment card of the agent's: it waits for its person's payment (of-panier-exige-carte B);
5. a payment asked by the work of an offer's « Oui » is a plain notification, not an urgent one; the card stays
   (of-tour-du-oui-sans-personne B);
6. a long text's file is named with its spaces (lt-nom-du-fichier A).

No real data: invented names, the payment networks' published test numbers only (test_card_payments.py)."""
import json
import re
import threading
import time

import test_watchers as tw
from test_connectors import tool as connector_tool
from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered
from test_approval_level import with_device
from test_card_payments import TOKEN as PAY_TOKEN
from test_long_text import article, uploads
from test_offers import kinds, make, offer_cards
from test_plans import DEVICE, NOW, TOKEN, answer
from test_task_frame import approve, cards, edit_routine, jobs, made_by_agent, repair_runs, routine, started
from test_threads_send import page, send, setup as thread_setup
from test_watcher_urgent_pay import ARGS, HermesSide, answered, approvals, setup as pay_setup

DAY = 86400.0


def connectors_file(bridge):
    return bridge.data / "connectors" / "connectors.json"


def edit_fix(bridge, conn_id, **changes):
    """A watcher's card as the bridge's clock would find it days later (the bridge idle meanwhile)."""
    path = connectors_file(bridge)
    store = json.loads(path.read_text())
    for c in store["agents"]["vie"]:
        if c["id"] == conn_id:
            c["frame"]["fix"].update(changes)
    path.write_text(json.dumps(store))


def connector(bridge, conn_id):
    return next(c for c in bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"] if c["id"] == conn_id)


def pushed_with(fakes, run_id):
    return [p for p in fakes.apns.payloads() if run_id in json.dumps(p)]


# -- 1. a watcher is told before it stops --------------------------------------------------------------------------------

def test_1_a_watcher_unanswered_is_asked_again_saying_the_day_it_stops_then_stopped(bridges, fakes):
    bridge, value, conn, _ = tw.setup(bridges, fakes)
    assert tw.check(bridge, conn)["step"] == "baseline"
    value.write_text("fail")
    assert [tw.check(bridge, conn)["step"] for _ in range(3)] == ["failed", "failed", "broken"]
    assert wait_until(lambda: cards(bridge, "bip-fix-", "vie"), timeout=20)
    [first] = cards(bridge, "bip-fix-", "vie")
    # 4 days without an answer: the card again, pushed, saying the day it stops; still on
    edit_fix(bridge, conn["id"], at=time.time() - 4 * DAY - 60)
    assert wait_until(lambda: [c for c in cards(bridge, "bip-fix-", "vie") if c["run_id"] != first["run_id"]], timeout=60)
    [warned] = cards(bridge, "bip-fix-", "vie")
    assert re.search(r"\(No: it stops\.\) With no answer, it stops on \w{3} \d{1,2}\.$", warned["bip_summary"]), warned
    assert warned["bip_fix"]["text"].endswith(".") and "With no answer, it stops on " in warned["bip_fix"]["text"]
    assert warned["bip_choices"] == ["repair", "stop"] and warned["bip_fix"]["stops_at"] > time.time() + 2 * DAY
    assert wait_until(lambda: pushed_with(fakes, warned["run_id"]), timeout=20)
    assert connector(bridge, conn["id"])["enabled"] is True
    # its day come, still no answer: switched off, one line (as before)
    edit_fix(bridge, conn["id"], at=time.time() - 8 * DAY, warned_at=time.time() - 4 * DAY)
    line = "“Créneau médecin” is stopped: it hadn't worked for 7 days. It stays on its page."
    assert wait_until(lambda: [i for i in boite_items(bridge) if i["text"] == line], timeout=60)
    assert connector(bridge, conn["id"])["enabled"] is False and cards(bridge, "bip-fix-", "vie") == []


def test_1_its_agent_alone_never_removes_a_watcher(bridges, fakes):
    bridge, _, conn, _ = tw.setup(bridges, fakes)
    removed = connector_tool(bridge, "connector_remove", {"name": "Créneau médecin"})
    assert text_of(removed).startswith("Not done: ") and "only your person stops a watcher" in text_of(removed), \
        text_of(removed)
    assert connector(bridge, conn["id"])["enabled"] is True


# -- 2. one card per agent when its AI doesn't answer --------------------------------------------------------------------

def test_2_an_agent_whose_ai_does_not_answer_has_one_card_for_its_routines(bridges, fakes):
    bridge = started(with_device(bridges))
    hermes = fakes.hermes
    hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    for n in range(1, 7):
        hermes.run_info[f"r{n}"] = {"status": "failed", "error": "APIConnectionError: Connection error."}
    ids = {}
    for name in ("Courriels", "Météo"):
        ids[name] = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                                json={"name": name, "schedule": "every 2h", "prompt": "Regarde."}).json()["job"]["id"]
    n = 0
    for name in ("Courriels", "Météo"):
        for _ in range(3):
            n += 1
            assert bridge.post(f"/v1/routines/{ids[name]}/run", headers=AUTH).status_code == 200
            assert wait_until(lambda: len(hermes.started) == n and routine(bridge, ids[name]).get("last_status")
                              and jobs(bridge)[name]["state"] != "running"
                              and jobs(bridge)[name]["failures"] == (n - 1) % 3 + 1, timeout=60), jobs(bridge)
    assert wait_until(lambda: cards(bridge, "bip-fix-"), timeout=20)
    time.sleep(1.0)
    [card] = cards(bridge, "bip-fix-")
    fix = card["bip_fix"]
    assert (fix["variant"], fix["kind"], fix["routines"]) == ("ai", "agent_ai", ["Courriels", "Météo"]), fix
    assert fix["title"] == "Wellness · its AI isn't answering" and fix["buttons"] == ["Try again", "Stop"]
    assert fix["after"] == ["Its routines start again", "Stopped, on their page"], fix  # Alex 25/11, tf2-carte-ia-apres
    assert card["bip_summary"] == ("Wellness's AI isn't answering: its routines couldn't run (“Courriels”, “Météo”). "
                                   "Try again? (No: they stop.)")
    assert card["choices"] == ["once", "deny"]
    for name in ("Courriels", "Météo"):
        job = jobs(bridge)[name]
        assert (job["state"], job["fix"]) == ("paused", card["run_id"]), job
    assert wait_until(lambda: pushed_with(fakes, card["run_id"]), timeout=20)
    assert len(pushed_with(fakes, card["run_id"])) == 1
    # « Réessayer »: both on again, no turn
    assert approve(bridge, card, "once").status_code == 200
    for name in ("Courriels", "Météo"):
        job = jobs(bridge)[name]
        assert (job["state"], job["enabled"], job["failures"], job["fix"]) == ("scheduled", True, 0, None), job
    assert cards(bridge, "bip-fix-") == [] and repair_runs(fakes) == []


# -- 3. 90 days: « Garder ? », never stopped without its person ---------------------------------------------------------

def test_3_a_routine_of_its_agent_s_own_goes_on_past_its_90_days_and_is_asked_again(bridges, fakes):
    bridge = started(with_device(bridges))
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    _, card = made_by_agent(bridge, "Météo", "every day at 7:30")   # outside any turn: its own idea, 90 days
    assert approve(bridge, card, "once").status_code == 200
    rid = jobs(bridge)["Météo"]["id"]
    edit_routine(bridge, rid, frame={"until": time.time() - 60, "keep_asked": True})
    assert wait_until(lambda: cards(bridge, "bip-fix-"), timeout=60), jobs(bridge)
    [keep] = cards(bridge, "bip-fix-")
    job = jobs(bridge)["Météo"]
    assert job["enabled"] is True and job["ends"]["kind"] == "date"
    assert abs(job["ends"]["at"] - (time.time() + 14 * DAY)) < DAY, job["ends"]
    assert keep["bip_fix"]["variant"] == "keep" and keep["bip_choices"] == ["keep", "stop"]
    assert wait_until(lambda: pushed_with(fakes, keep["run_id"]), timeout=20)
    # unanswered again: on « until you stop it », the card gone
    edit_routine(bridge, rid, frame={"until": time.time() - 60})
    assert wait_until(lambda: jobs(bridge)["Météo"]["ends"]["kind"] == "never", timeout=60), jobs(bridge)["Météo"]
    assert jobs(bridge)["Météo"]["enabled"] is True and cards(bridge, "bip-fix-") == []


# -- 4. a basket without a card of the agent's ---------------------------------------------------------------------------

def browser_place(bridges, fakes):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (DEVICE, "sandbox", "[]", NOW, NOW))])
    home = managed_install(bridge, fakes, agents=("coach",))
    # its own browser on, no payment card anywhere
    (profile(home, "coach") / "config.yaml").write_text(
        "mcp_servers:\n  stealth:\n    url: http://127.0.0.1:9601/mcp\n    enabled: true\n")
    bridge.start()
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach", TOKEN)
    return bridge


def test_4_a_basket_is_offered_without_a_card_of_its_own_and_waits_for_its_person(bridges, fakes):
    bridge = browser_place(bridges, fakes)
    assert "cart" in kinds(bridge), kinds(bridge)
    make(bridge, {"kind": "cart", "label": "Je mets le vélo au panier ?", "about": "Vélo",
                  "prompt": "Mets le vélo de ville au panier, jusqu'à la page de paiement.", "site": "www.velo.example"})
    [card] = offer_cards(bridge)
    fakes.hermes.run_output = "Le panier est prêt : il attend ton paiement."
    assert answer(bridge, card, "once").status_code == 200
    assert wait_until(lambda: fakes.hermes.started, timeout=30)
    said = next(iter(fakes.hermes.started.values()))["input"]
    assert "never paying" in said and "waits for them" in said and "add a payment card" in said, said


# -- 5. the work of an offer's « Oui »: a payment is a plain notification ------------------------------------------------

def test_5_a_payment_asked_by_an_offer_s_work_is_pushed_plainly_and_its_card_stays(bridges, fakes):
    bridge, chrome = pay_setup(bridges, fakes)
    try:
        side = HermesSide(bridge, ARGS)
        assert side.got_question.wait(30)
        payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", side.asked).group(1)
        gate = threading.Event()
        fakes.hermes.runs["r1"] = [
            sse("approval.request", run_id="r1", request_id="q1", choices=["once", "deny"], command=side.asked,
                description="Fields requested by MCP server 'bipagents_pay'", pattern_key="mcp_elicitation"),
            gate, sse("run.completed", run_id="r1", output="Billet payé."),
        ]
        make(bridge, {"kind": "search", "label": "Je regarde les billets ?", "about": "Concert",
                      "prompt": "Regarde si les billets du concert sont en vente."}, token=PAY_TOKEN)
        [offer] = offer_cards(bridge, "vie")
        assert answer(bridge, offer, "once", agent="vie").status_code == 200
        assert wait_until(lambda: answered(fakes, "r1") == ["once"], timeout=30), fakes.hermes.approvals
        side.answer("accept")
        assert wait_until(lambda: [c for c in approvals(bridge) if c["run_id"] == f"bip-pay-{payment}"], timeout=10)
        assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("pay") == payment], timeout=10)
        [push] = [p for p in fakes.apns.payloads() if p.get("pay") == payment]
        assert push["aps"]["category"] == "PAYMENT" and push["t"] == "approval", push
        assert "urgent" not in push and "interruption-level" not in push["aps"], push
        deny = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": f"bip-pay-{payment}",
                                                              "request_id": payment, "choice": "deny"})
        assert deny.status_code == 200, deny.text
        gate.set()
    finally:
        chrome.stop()


# -- 6. a long text's file name keeps its spaces ------------------------------------------------------------------------

def test_6_a_long_text_s_file_is_named_with_its_spaces(bridges, fakes):
    bridge, thread = thread_setup(bridges, fakes)
    text = article(60_000)
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("run.completed", run_id="r1", output="Lu.")]
    client_id = "C1D00000-0000-4000-8000-00000000C601"
    assert send(bridge, thread, client_id, text).status_code == 200
    name = "Pourquoi le vélo électrique a gagné la ville.txt"
    given = fakes.hermes.started["r1"]["input"]
    assert f"[Pièce jointe : {name} (text/plain, " in given, given
    [attached] = page(bridge, thread)["items"][-1]["attachments"]
    assert attached["name"] == name, attached
    # on the disk: its safe form, `<month>/<id12>-<name>`
    path = given.split(" → ")[-1].rstrip("]").strip()
    assert re.fullmatch(r".*/\d{4}-\d{2}/[0-9a-f]{12}-Pourquoi_le_vélo_électrique_a_gagné_la_ville\.txt", path), path
    assert [p.name for p in uploads(bridge).rglob("*.txt")] == [path.rsplit("/", 1)[-1]]
