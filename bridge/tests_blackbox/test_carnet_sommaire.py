"""Carnet « Le sommaire » (plan piste 9, contracts/carnet/sommaire.md). Two things here:

1. **The installed 1.0 apps see exactly today's answers.** A Carnet like the demo's (people, subjects, areas, « Toi »,
   tasks, the five kinds of records, a received file, a page made in the app) is written through the agents' tools and
   the app's routes, then every route a 1.0 app reads is compared, as JSON, with its answer from the bridge before the
   sommaire (rc-1122 78f4553d6, kept in fixtures/carnet_1_0/), once the keys the contract adds are taken out
   (contracts/carnet/sommaire.json `additive`). Ids, dates, revisions and times are normalised first (a fact's date
   as days from the run's day). BB_RECORD_CARNET_1_0=<dir> writes the answers there instead (only from a bridge before
   the sommaire: that is the reference).
2. **The categories at the bridge (brick S1)**: what the pages, records, tasks and documents carry, the person's
   correction (`POST /v1/notebook/categorize`, its undo) that always wins and teaches the next writes, the agent's
   `category`."""
import json
import os
import re
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of

FIXTURES = Path(__file__).parent / "fixtures" / "carnet_1_0"
CONTRACT = Path(__file__).resolve().parents[2] / "contracts" / "carnet" / "sommaire.json"
MONTHS = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre",
          "décembre"]


def today():
    return datetime.now(timezone.utc).date()


def spoken(offset):
    d = today() + timedelta(days=offset)
    return f"{d.day} {MONTHS[d.month - 1]}"


def iso(offset):
    return (today() + timedelta(days=offset)).isoformat()


def wrote(result):
    assert result["isError"] is False, text_of(result)
    return result


# -- the Carnet, written as the agents and the person do ----------------------------------------------------------

def write_carnet(bridge):
    give_token(bridge)
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200
    w = lambda args: wrote(call(bridge, "notebook_write", args))  # noqa: E731
    w({"page": "Marc Durand", "kind": "person", "section": "dates", "facts": [f"Anniversaire : {spoken(23)}"],
       "summary": "Ton beau-frère, menuisier.", "close": True})
    w({"page": "Marc Durand", "section": "gifts", "facts": ["Un vinyle de Chet Baker"]})
    w({"page": "Marc Durand", "section": "work", "facts": ["Menuisier à son compte"]})
    w({"page": "Claire Durand", "kind": "person", "section": "who", "facts": ["Ta sœur"], "close": True})
    w({"page": "Claire Durand", "section": "dates", "facts": [f"Anniversaire : {spoken(41)}"]})
    w({"page": "Rénovation cuisine", "kind": "subject", "section": "status", "facts": ["Devis reçu"],
       "summary": "Devis de Marc reçu : 8 900 €."})
    w({"page": "Rénovation cuisine", "section": "budget", "facts": ["Devis : 8 900 €"]})
    w({"page": "Voyage Japon", "kind": "subject", "section": "decisions", "facts": ["Avril, 12 jours : Tokyo, Kyoto"]})
    w({"page": "Impôts 2026", "kind": "subject", "section": "next", "facts": ["Ajouter les dons"]})
    w({"page": "Maison", "kind": "area", "section": "habits", "facts": ["Poubelles le lundi soir"]})
    w({"page": "Finances", "kind": "area", "section": "matters", "facts": ["Épargne : 200 € par mois"]})
    w({"page": "Travail", "kind": "area", "section": "matters", "facts": [f"Entretien annuel le {spoken(5)}"]})
    w({"page": "Toi", "kind": "self", "section": "about", "facts": ["Vit à Nantes"]})
    t = lambda args: wrote(call(bridge, "notebook_task", args))  # noqa: E731
    t({"action": "add", "title": "Rappeler Marc pour le devis", "due": iso(2), "page": "Rénovation cuisine"})
    t({"action": "add", "title": "Trier les photos de l'été"})
    r = lambda kind, fields: wrote(call(bridge, "notebook_record", {"action": "add", "type": kind, "fields": fields}))  # noqa: E731
    r("purchase", {"date": iso(-120), "item": "Lave-linge", "store": "Électro Plus", "price": "499 €"})
    r("subscription", {"service": "Films en streaming", "price": "13,49 €", "period": "Mensuel", "renewal": iso(9)})
    r("contract", {"what": "Assurance habitation", "provider": "Assureur Horizon", "end": iso(95)})
    r("paper", {"document": "Passeport", "expires": iso(420)})
    r("booking", {"date": iso(12) + " 10:30", "what": "Vol pour Lisbonne", "reference": "ABC123"})
    # a file the agent was sent, and a page the person made in the app
    received = Path(bridge.config["agents"]["Wellness"]["upload_dir_host"]) / "2026-11"
    received.mkdir(parents=True, exist_ok=True)
    (received / "0123456789ab-Devis cuisine.pdf").write_bytes(b"%PDF-1.4 demo")
    made = bridge.post("/v1/notebook/page", json={"kind": "subject", "title": "Jardin"}, headers=AUTH)
    assert made.status_code == 201, made.text


# -- what a 1.0 app reads -----------------------------------------------------------------------------------------

READS = [
    ("home", "/v1/notebook?lang=fr"),
    ("pages-person", "/v1/notebook/pages?kind=person"),
    ("pages-subject", "/v1/notebook/pages?kind=subject"),
    ("pages-area", "/v1/notebook/pages?kind=area"),
    ("pages-records", "/v1/notebook/pages?kind=records"),
    ("search-marc", "/v1/notebook/search?q=marc&lang=fr"),
    ("search-devis", "/v1/notebook/search?q=devis%20cuisine&lang=fr"),
    ("search-anniversaire", "/v1/notebook/search?q=anniversaire%20claire&lang=fr"),
    ("page-marc", "/v1/notebook/page?path=Personnes/Marc%20Durand.md&lang=fr"),
    ("page-cuisine", "/v1/notebook/page?path=Sujets/R%C3%A9novation%20cuisine.md&lang=fr"),
    ("page-jardin", "/v1/notebook/page?path=Sujets/Jardin.md&lang=fr"),
    ("page-toi", "/v1/notebook/page?path=Toi.md&lang=fr"),
    ("page-achats", "/v1/notebook/page?path=Fiches/Achats.md&lang=fr"),
    ("page-taches", "/v1/notebook/page?path=T%C3%A2ches/T%C3%A2ches.md&lang=fr"),
    ("records", "/v1/notebook/records"),
    ("tasks", "/v1/notebook/tasks"),
    ("documents", "/v1/notebook/documents"),
    ("offline", "/v1/notebook/offline?lang=fr"),
    ("settings", "/v1/notebook/settings"),
]

ISO_DAY = re.compile(r"(?<![0-9])(\d{4}-\d{2}-\d{2})(?![0-9])")
SHORT_DAY = re.compile(r"\b\d{1,2} (?:janv\.|févr\.|mars|avr\.|mai|juin|juil\.|août|sept\.|oct\.|nov\.|déc\.)(?: \d{4})?")
EPOCH = re.compile(r'("[a-z_]*_at": )\d+(?:\.\d+)?')
STAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?")
IDS = [(re.compile(r"\bp[0-9a-f]{10}\b"), "P"), (re.compile(r"\bt[0-9a-f]{6,16}\b"), "T"),
       (re.compile(r"\br[0-9a-f]{6}\b"), "R"), (re.compile(r"\bc[0-9a-f]{12}\b"), "C"),
       (re.compile(r"\b[0-9a-f]{20}\b"), "V"), (re.compile(r"\b[0-9a-f]{16}\b"), "REV")]


def stable(name, answer):
    """What depends on the calendar day, not on the bridge, left out: the week's review card (offered once a week)
    and the standard dates (holidays…) of the next days."""
    if name == "home":
        answer.pop("review", None)

    def drop(v):
        if isinstance(v, dict):
            for x in v.values():
                drop(x)
        elif isinstance(v, list):
            v[:] = [x for x in v if not (isinstance(x, dict) and x.get("computed") is True)]
            for x in v:
                drop(x)
    drop(answer)
    return answer


def normalised(value):
    """The answer as text with what changes from run to run replaced: times, days (as « D+n »), ids (by order)."""
    text = json.dumps(value, ensure_ascii=False, sort_keys=True, indent=1)
    text = STAMP.sub("STAMP", text)
    text = EPOCH.sub(r'\1"STAMP"', text)
    text = ISO_DAY.sub(lambda m: "D%+d" % (date.fromisoformat(m.group(1)) - today()).days, text)
    for offset in (2, 5, 9, 12, 23, 41, 95, 420):
        text = text.replace(spoken(offset), "{date%+d}" % offset)
    # « noté le 22 nov. », « Noté par Wellness le 22 nov. 2026 »: the run's day, in words
    text = SHORT_DAY.sub("DAY", text)
    for pattern, mark in IDS:
        seen = {}
        text = pattern.sub(lambda m, seen=seen, mark=mark: seen.setdefault(m.group(0), f"{mark}{len(seen) + 1}"), text)
    return text


def contract_additive():
    return json.loads(CONTRACT.read_text())["additive"]


def strip(value, path):
    """`value` without the key at `path` (« pages[].categories », « frontmatter.space »)."""
    head, _, rest = path.partition(".")
    if isinstance(value, list):
        for item in value:
            strip(item, path)
        return
    if not isinstance(value, dict):
        return
    if head.endswith("[]"):
        strip(value.get(head[:-2]), rest)
    elif rest:
        strip(value.get(head), rest)
    else:
        value.pop(head, None)


FRONT_KEYS = re.compile(r"^(categories|categories_by|space):.*\n(?:  - .*\n)*", re.M)


def as_1_0(route, answer):
    """What a 1.0 app decodes of `answer`: the contract's additions taken out (the page's own text too: its
    frontmatter's new keys)."""
    for path in contract_additive().get(route, []):
        strip(answer, path)

    def texts(v):
        if isinstance(v, dict):
            for k, x in v.items():
                if k == "text" and isinstance(x, str) and x.startswith("---\n"):
                    head, sep, body = x[4:].partition("\n---\n")
                    v[k] = "---\n" + FRONT_KEYS.sub("", head + "\n").rstrip("\n") + sep + body
                else:
                    texts(x)
        elif isinstance(v, list):
            for x in v:
                texts(x)
    texts(answer)
    return answer


def route_of(path):
    return "GET " + path.split("?")[0]


def test_the_1_0_apps_see_exactly_todays_answers(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    record = os.environ.get("BB_RECORD_CARNET_1_0")
    differ = []
    for name, path in READS:
        got = bridge.get(path, headers=AUTH)
        assert got.status_code == 200, (path, got.text)
        answer = stable(name, got.json())
        if record:
            Path(record).mkdir(parents=True, exist_ok=True)
            (Path(record) / f"{name}.json").write_text(normalised(answer) + "\n")
            continue
        want = (FIXTURES / f"{name}.json").read_text().rstrip("\n")
        if normalised(as_1_0(route_of(path), answer)) != want:
            differ.append(name)
            (bridge.root / f"{name}.got.json").write_text(normalised(as_1_0(route_of(path), answer)) + "\n")
    assert not differ, f"not as before: {differ} (the answers, as a 1.0 app reads them, in {bridge.root})"
