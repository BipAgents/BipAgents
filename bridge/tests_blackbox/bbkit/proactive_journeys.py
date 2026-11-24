"""What the journeys' bridge needs for the proactive agents' journeys of 1.1 (J67–J71, docs/release-journeys.md
« Agents proactifs »; passation-pro briefs/design-agents-proactifs.md §6 line 11): the same for the black-box column
(test_journeys_proactive.py) and for the apps' columns (serve_journeys_people.py, the apps' ReleaseJourneysPeople…).

- the bridge's clocks made short (`configure`, before it starts): the plans' sources looked at every tick and an e-mail
  due at once (« vers 18 h » any time), the mail triggers fast, the watchers checked only when asked and « en panne »
  after 3 failures, the smart passes due without waiting 2 h nor 20 quiet minutes, their small model the fake one;
- Vie's turns that propose (`install`, bbkit/model.py `PeopleModel`: `#plan`, `#offre`, `#ia-muette`): a plan for
  Lisbon with two offers, an offer alone, a routine whose AI doesn't answer;
- two watchers of Vie's, approved and checked once (`watchers`): their check fails from then on (« Prix du vélo » for
  « Réparer », « Créneau médecin » for « Arrêter »), « Lancer maintenant » three times makes the card;
- a mailbox for the replies due (`MAILBOX`, the fake IMAP's): its e-mails come once its watch idles (`feed_replies`);
- the smart pass's idea (`IDEA`): what the fake small model answers a pass.

No real account: an invented person (camille@proton.me), the fake IMAP's password, the fake model."""
from __future__ import annotations

import datetime
import email.utils
import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Dict, List
from zoneinfo import ZoneInfo

from .common import AUTH
from .servers import wait_until

AGENT = "vie"
#: The plans' sources looked at every tick (never only « vers 8 h » / « vers 18 h »), an e-mail due at once.
SOURCES = {"morning": "any", "evening": "any", "due_hours": 0, "tick_seconds": 0.3, "follow_seconds": 1}
TRIGGERS = {"tick_seconds": 0.2, "batch_seconds": 0, "idle_minutes": 0.2, "poll_minutes": 0.05}
#: Checked only when asked (« Lancer maintenant »), « en panne » at the 3rd failure in a row.
WATCHERS = {"tick_seconds": 3600, "wake_gap_minutes": 0, "failures_max": 3, "run_now_gap_seconds": 0}
#: A pass looked at only when asked (`POST /v1/proactive/passes`), due without 2 h between two nor 20 quiet minutes.
PASSES = {"tick_seconds": 3600, "gap_hours": 0, "quiet_minutes": 0}

ME = "camille@proton.me"
MAIL_PASSWORD = "journeys-imap-1234"   # the fake IMAP's (bbkit/imap.py), nothing else
#: The e-mails waiting for a reply (headers only reach the card): they arrive once the mailbox is watched.
DUE = [("Marie Dupont <marie@example.org>", "Devis cuisine"), ("Paul Martin <paul@example.net>", "Dîner samedi ?")]
#: One already answered (its reply in Sent) and a mass mailing: never on the card.
ANSWERED = ("Léa <lea@example.net>", "Photos")
BULK = ("Shop <news@shop.example>", "Promo")

#: The smart pass's idea, as the fake small model answers it (offer_ideas.rs: label, prompt, uses, topic, later).
IDEA = {"label": "Je te cherche 3 idées de cadeau pour Léa, autour de 40 € ?", "topic": "Cadeau Léa",
        "prompt": "Cherche 3 idées de cadeau d'anniversaire pour Léa autour de 40 euros, avec un lien chacune. "
                  "N'achète rien.",
        "uses": [{"tool": "create_page"}], "later": ["pay"]}
#: What the person says during the synthetic day (three messages: a pass is due).
DAY = ["C'est l'anniversaire de Léa dimanche", "Elle adore la céramique", "Je n'ai pas encore d'idée de cadeau"]

WATCHED = {
    "velo": {"name": "Prix du vélo", "sites": ["www.velos.example"], "description": "Le prix du vélo Riverside",
             "icon": "fitness"},
    "creneau": {"name": "Créneau médecin", "sites": ["www.doctolib.example"],
                "description": "Les créneaux libres du Dr Martin", "icon": "health"},
}

CHECK = r'''
import json, sys
mode = open(__VALUE__).read().strip()
if mode.startswith("fail"):
    sys.stderr.write("the site answered 503\n")
    sys.exit(1)
print(json.dumps({"state": {"value": mode}, "summary": "Valeur : " + mode, "condition": False}))
'''


def configure(config: Dict[str, Any], fakes, passes_only: bool = False) -> None:
    """`passes_only`: the smart passes alone (serve_journeys.py's bridges, where the apps' J71 runs: the people bridge's
    J70 makes Vie's two unasked cards of the day, plans/offers.rs `unasked_refusal`, and a pass's idea is a third)."""
    if not passes_only:
        config["plans_sources"] = dict(SOURCES)
        config["triggers"] = {**(config.get("triggers") or {}), **TRIGGERS}
        config["watchers"] = dict(WATCHERS)
    config["proactive"] = dict(PASSES)
    config["notebook"] = {**(config.get("notebook") or {}), "base_url": fakes.model.url + "/v1", "key": "sk-journeys",
                          "model": "small-eu"}


def local(days: int, hour: int, minute: int = 0, zone: str = "Europe/Paris") -> str:
    day = datetime.datetime.now(ZoneInfo(zone)).date() + datetime.timedelta(days=days)
    return f"{day.isoformat()}T{hour:02d}:{minute:02d}"


def trip() -> Dict[str, Any]:
    """Lisbon in three days: three moments, two offers (a page and a reminder: what Vie can do here, plans/offers.rs)."""
    return {"kind": "trip", "title": "Lisbonne", "zone": "Europe/Lisbon", "city": "Lisbonne",
            "moments": [
                {"at": local(2, 9, 15), "zone": "Europe/Paris", "what": "Enregistrement en ligne",
                 "say": "L’enregistrement en ligne du TP 433 est ouvert", "link": "https://www.flytap.com/checkin"},
                {"at": local(3, 10, 50, "Europe/Lisbon"), "what": "Arrivée : métro rouge vers le centre",
                 "say": "Bienvenue à Lisbonne : le métro rouge va au centre en 20 min."},
                {"at": local(3, 15, 0, "Europe/Lisbon"), "what": "Ta chambre est prête",
                 "say": "Ta chambre est prête à partir de 15:00."}],
            "offers": [
                {"kind": "page", "label": "Je te fais la liste de bagages ?",
                 "prompt": "Fais une page avec la liste de bagages pour trois jours à Lisbonne. Ne réserve rien."},
                {"kind": "reminder", "label": "Je te mets un rappel pour le taxi ?",
                 "params": {"at": local(3, 7, 30), "say": "Pense au taxi pour Orly."}}]}


def offer() -> Dict[str, Any]:
    return {"kind": "page", "label": "Je te fais un petit suivi des menus ?", "about": "Menus de la semaine",
            "prompt": "Fais une page avec les menus de la semaine et la liste des courses. N'achète rien."}


def install(journey) -> None:
    """Vie's proposing turns (PeopleModel's `#plan`, `#offre`) call the bridge's own tools as Vie, in the turn."""
    from .people import mcp_as
    bridge, token = journey.bridge, journey.token
    journey.model.calls["plan"] = lambda run: mcp_as(bridge, token, "/v1/agent-tools/mcp", "plan_propose", trip())
    journey.model.calls["offer"] = lambda run: mcp_as(bridge, token, "/v1/agent-tools/mcp", "offer_act", offer())


def approvals(bridge, prefix: str) -> List[Dict[str, Any]]:
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": AGENT}).json()["items"]
    return [c for c in items if c["run_id"].startswith(prefix)]


def watchers(journey) -> Dict[str, Dict[str, Any]]:
    """The two watchers of Vie's, approved (their connector card), their first check done, failing from then on: the
    apps' journeys make them « en panne » with « Lancer maintenant » (`POST …/connectors/{id}/run`, three times)."""
    bridge = journey.bridge
    made: Dict[str, Dict[str, Any]] = {}
    for slug, about in WATCHED.items():
        value = bridge.root / f"veille-{slug}.txt"
        value.write_text("1 299 €" if slug == "velo" else "mardi 10:00")
        work = journey.profile / "sandboxes" / "docker" / "default" / "workspace" / "connecteurs" / slug
        work.mkdir(parents=True, exist_ok=True)
        (work / "check.py").write_text(CHECK.replace("__VALUE__", repr(str(value))))
        seen = {c["request_id"] for c in approvals(bridge, "bip-conn-")}
        arguments = {**about, "folder": f"/workspace/connecteurs/{slug}", "entry": "python3 check.py",
                     "watch": {"every": "15m", "notify": "on_change"}}
        with ThreadPoolExecutor(1) as pool:
            call = pool.submit(journey.mcp, "/v1/agent-tools/connectors", "connector_register", arguments)
            found: List[Dict[str, Any]] = []
            wait_until(lambda: found.extend(c for c in approvals(bridge, "bip-conn-")
                                            if c["request_id"] not in seen) or found or call.done(), timeout=30)
            if not found:   # its call ended without a card: why
                raise AssertionError(f"no connector card for {about['name']}: "
                                     f"{call.exception(timeout=1) if call.done() else 'still waiting'}")
            allowed = bridge.post(f"/v1/connectors/requests/{found[0]['request_id']}", headers=AUTH, json={"allow": True})
            assert allowed.status_code == 200, allowed.text
            assert "approved" in call.result(timeout=60)
        conn = next(c for c in bridge.get(f"/v1/agents/{AGENT}/connectors", headers=AUTH).json()["items"]
                    if c["name"] == about["name"])
        assert check(bridge, conn["id"])["step"] == "baseline"
        value.write_text("fail")
        made[slug] = {"id": conn["id"], "name": about["name"]}
    return made


def check(bridge, conn_id: str) -> Dict[str, Any]:
    """« Lancer maintenant » on a watcher, as the apps do it, waited for."""
    resp = bridge.post(f"/v1/agents/{AGENT}/connectors/{conn_id}/run", headers=AUTH, params={"wait": "true"}, timeout=120)
    assert resp.status_code == 200, resp.text
    return resp.json()["outcome"]


def mail(sender: str, subject: str, message_id: str, headers: str = "") -> bytes:
    return (f"From: {sender}\r\nTo: {ME}\r\nSubject: {subject}\r\nDate: {email.utils.formatdate()}\r\n"
            f"Message-ID: {message_id}\r\n{headers}Content-Type: text/plain; charset=utf-8\r\n\r\n"
            "Bonjour, peux-tu me répondre ?\r\n").encode()


def mailbox(fakes) -> Dict[str, Any]:
    """The body of `POST /v1/mail/accounts` for Camille's mailbox on the fake IMAP, given to Vie."""
    fakes.imap.users[ME] = MAIL_PASSWORD
    return {"provider": "proton", "username": ME, "password": MAIL_PASSWORD, "port": fakes.imap.port, "agents": ["Vie"]}


def deliver_due(fakes) -> None:
    """The e-mails of the day, at once (one list assignment): two waiting, one answered, a mass mailing."""
    imap = fakes.imap
    sent = [(301, mail(f"Camille <{ME}>", "Re: Photos", "<s1@proton.me>", headers="In-Reply-To: <lea1@example.net>\r\n"))]
    inbox = [(201, mail(*DUE[0], "<m1@example.org>")), (202, mail(*DUE[1], "<p1@example.net>")),
             (203, mail(*ANSWERED, "<lea1@example.net>")),
             (204, mail(*BULK, "<n1@shop.example>", headers="List-Unsubscribe: <mailto:u@shop.example>\r\n"))]
    for folder, mails in (("Sent", sent), ("INBOX", inbox)):
        for uid, _ in mails:
            imap.flags[(folder, uid)] = set()
        imap.folders[folder] = imap.folders.get(folder, []) + mails


def feed_replies(fakes) -> threading.Thread:
    """For the apps (they cannot reach the fake IMAP): the day's e-mails arrive once the replies watch idles, once."""
    def run():
        while fakes.imap.idling < 1:
            time.sleep(0.5)
        time.sleep(1.0)
        deliver_due(fakes)
    thread = threading.Thread(target=run, name="journeys-replies", daemon=True)
    thread.start()
    return thread


def out(journey, fakes, watched: Dict[str, Dict[str, Any]]) -> Dict[str, Any]:
    """What the apps' people journeys read in the served JSON (`proactive`, serve_journeys_people.py)."""
    return {"watchers": watched, "mailbox": mailbox(fakes), "me": ME,
            "due": [{"from": s.split(" <")[0], "subject": subj} for s, subj in DUE]}


def out_passes() -> Dict[str, Any]:
    """What the apps' J71 reads in the served JSON (`proactive`, serve_journeys.py)."""
    return {"idea": {"label": IDEA["label"], "topic": IDEA["topic"], "with": "Avec : une page",
                     "later": "Il te demandera ton accord avant de payer."}, "day": DAY}
