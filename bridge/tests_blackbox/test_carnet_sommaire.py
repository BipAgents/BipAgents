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
from bbkit.mcp import call, give_token, rpc, text_of

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


# -- brick S1: the categories at the bridge -----------------------------------------------------------------------

def pages_by_title(bridge):
    out = {}
    for kind in ("person", "subject", "area", "self", "records"):
        for p in bridge.get(f"/v1/notebook/pages?kind={kind}", headers=AUTH).json()["pages"]:
            out[p["title"]] = p
    return out


def page_of(bridge, path):
    got = bridge.get("/v1/notebook/page", params={"path": path}, headers=AUTH)
    assert got.status_code == 200, got.text
    return got.json()


def facts_of(page):
    return {f["text"]: f["categories"] for s in page["sections"] for f in s["facts"]}


def test_the_bridge_files_pages_records_tasks_and_documents_by_their_words(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sommaire"] == 1
    # the pages, by their kind and their words, written in their frontmatter (the app's new page too)
    pages = pages_by_title(bridge)
    want = {"Marc Durand": ["people"], "Claire Durand": ["people"], "Rénovation cuisine": ["home"],
            "Voyage Japon": ["trips"], "Impôts 2026": ["money", "papers"], "Maison": ["home"], "Finances": ["money"],
            "Travail": ["work"], "Jardin": ["home"], "Achats": ["money", "papers"], "Abonnements": ["money"],
            "Réservations": ["trips"]}
    assert {t: pages[t]["categories"] for t in want} == want
    assert {pages[t]["categories_by"] for t in want} == {"auto"}
    text = (bridge.state_dir / "notebook" / "carnet" / "Sujets" / "Rénovation cuisine.md").read_text()
    assert "\ncategories:\n  - home\ncategories_by: auto\nupdated: " in text, text
    # a page's lines: its categories, and Dates when they carry a date
    marc = page_of(bridge, "Personnes/Marc Durand.md")
    assert (marc["categories"], marc["categories_by"]) == (["people"], "auto")
    facts = facts_of(marc)
    assert facts[f"Anniversaire : {spoken(23)}"] == ["people", "dates"]
    assert facts["Un vinyle de Chet Baker"] == ["people"]
    # the records by their type (Dates: their dates), the tasks by their page, the documents by their words
    records = {t["type"]: t for t in bridge.get("/v1/notebook/records", headers=AUTH).json()["types"]}
    assert records["purchase"]["categories"] == ["money", "papers"]
    assert [(i["item"], i["categories"]) for i in records["purchase"]["items"]] == [("Lave-linge", ["money", "papers", "dates"])]
    assert records["subscription"]["items"][0]["categories"] == ["money", "dates"]
    assert records["contract"]["items"][0]["categories"] == ["papers", "dates"]
    assert records["paper"]["items"][0]["categories"] == ["papers", "dates"]
    assert records["booking"]["items"][0]["categories"] == ["trips", "dates"]
    tasks = {t["title"].split(" [[")[0]: t["categories"] for t in bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"]}
    # (a paper's two reminders, made by the bridge: Papiers)
    assert tasks == {"Rappeler Marc pour le devis": ["home", "dates"], "Trier les photos de l'été": ["other"],
                     "Penser à renouveler Passeport": ["papers", "dates"], "Renouveler Passeport": ["papers", "dates"]}, tasks
    documents = bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"]
    assert [(d["file"], d["categories"], d["categories_by"]) for d in documents] == [("Devis cuisine.pdf", ["home"], "auto")]
    # the search: the same categories on what it finds
    found = bridge.get("/v1/notebook/search", params={"q": "anniversaire claire", "lang": "fr"}, headers=AUTH).json()
    assert found["answers"] and found["answers"][0]["categories"] == ["people", "dates"], found["answers"]
    assert found["pages"][0]["categories"] == ["people"]


def test_the_person_files_elsewhere_and_the_next_write_follows(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    post = lambda body: bridge.post("/v1/notebook/categorize", json=body, headers=AUTH)  # noqa: E731
    kitchen = "Sujets/Rénovation cuisine.md"
    for body, status, code in [
        ({"path": kitchen, "categories": ["dates"]}, 400, "category_not_given"),
        ({"path": kitchen, "categories": ["health"]}, 400, "category_not_given"),
        ({"path": kitchen, "categories": ["maison"]}, 400, "category_unknown"),
        ({"path": kitchen, "categories": []}, 400, "category_count"),
        ({"path": kitchen, "categories": ["home", "money", "work"]}, 400, "category_count"),
        ({"categories": ["home"]}, 400, "target_missing"),
        ({"path": kitchen, "private": True, "categories": ["home"]}, 400, "category_not_given"),
        ({"path": "Sujets/Rien.md", "categories": ["home"]}, 404, "not_found"),
        ({"task": "t0000000", "categories": ["home"]}, 404, "not_found"),
    ]:
        got = post(body)
        assert got.status_code == status and got.json()["detail"]["code"] == code, (body, got.text)
    done = post({"path": kitchen, "categories": ["money"]})
    assert done.status_code == 200, done.text
    done = done.json()
    assert done["before"] == {"categories": ["home"], "categories_by": "auto"}
    assert (done["categories"], done["categories_by"]) == (["money"], "person")
    assert done["learnt"] == {"word": "renovation", "categories": ["money"]} and done["undo"].startswith("u")
    assert done["target"] == {"path": kitchen, "private": False}
    assert (page_of(bridge, kitchen)["categories"], page_of(bridge, kitchen)["categories_by"]) == (["money"], "person")
    # an agent writes there again, saying another category: the person's stays; the next page the word decides follows
    wrote(call(bridge, "notebook_write", {"page": "Rénovation cuisine", "section": "status", "facts": ["Travaux en mars"],
                                          "category": ["home"]}))
    assert page_of(bridge, kitchen)["categories"] == ["money"]
    wrote(call(bridge, "notebook_write", {"page": "Rénovation salle de bain", "kind": "subject", "section": "status",
                                          "facts": ["Devis demandé"]}))
    assert page_of(bridge, "Sujets/Rénovation salle de bain.md")["categories"] == ["money"]
    # not news: « Nouveau » never shows a correction
    assert all(c["action"] != "categorize" for c in bridge.get("/v1/notebook", headers=AUTH).json()["recent"])
    # « Annuler »: as it was, the rule forgotten, once
    undone = bridge.post("/v1/notebook/categorize/undo", json={"undo": done["undo"]}, headers=AUTH)
    assert undone.status_code == 200, undone.text
    assert (undone.json()["categories"], undone.json()["unlearnt"]) == (["home"], "renovation")
    assert page_of(bridge, kitchen)["categories"] == ["home"]
    again = bridge.post("/v1/notebook/categorize/undo", json={"undo": done["undo"]}, headers=AUTH)
    assert again.status_code == 404 and again.json()["detail"]["code"] == "undo_gone", again.text
    # a record, a task, a document and a line: the person's choice kept for them
    purchase = next(t for t in bridge.get("/v1/notebook/records", headers=AUTH).json()["types"] if t["type"] == "purchase")
    rid = purchase["items"][0]["id"]
    assert post({"record": {"type": "purchase", "id": rid}, "categories": ["home"]}).status_code == 200
    purchase = next(t for t in bridge.get("/v1/notebook/records", headers=AUTH).json()["types"] if t["type"] == "purchase")
    assert (purchase["items"][0]["categories"], purchase["items"][0]["categories_by"]) == (["home", "dates"], "person")
    task = next(t for t in bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"] if t["title"].startswith("Trier"))
    assert post({"task": task["id"], "categories": ["home"]}).status_code == 200
    task = next(t for t in bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"] if t["title"].startswith("Trier"))
    assert task["categories"] == ["home"]
    doc = bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"][0]
    filed = post({"document": {"agent": doc["agent"], "file": doc["file"]}, "categories": ["papers"]})
    assert filed.status_code == 200 and filed.json()["learnt"] == {"word": "devis", "categories": ["papers"]}, filed.text
    assert bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"][0]["categories"] == ["papers"]
    line = post({"line": {"path": "Personnes/Marc Durand.md", "text": "Un vinyle de Chet Baker"}, "categories": ["money"]})
    assert line.status_code == 200, line.text
    assert facts_of(page_of(bridge, "Personnes/Marc Durand.md"))["Un vinyle de Chet Baker"] == ["money"]
    gone = post({"line": {"path": "Personnes/Marc Durand.md", "text": "Rien de tel"}, "categories": ["money"]})
    assert gone.status_code == 404, gone.text


def test_an_agent_may_say_the_category(bridges):
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200
    tools = {t["name"]: t for t in rpc(bridge, "tools/list").json()["result"]["tools"]}
    assert tools["notebook_write"]["inputSchema"]["properties"]["category"] == {
        "type": "array", "items": {"enum": ["people", "money", "home", "papers", "trips", "work"]}, "maxItems": 2,
        "description": "only if sure"}
    path = "Sujets/Atelier poterie.md"
    wrote(call(bridge, "notebook_write", {"page": "Atelier poterie", "kind": "subject", "section": "status",
                                          "facts": ["Inscrit le jeudi"], "category": ["home"]}))
    assert (page_of(bridge, path)["categories"], page_of(bridge, path)["categories_by"]) == (["home"], "agent")
    # only the categories it may give: anything else is left to the bridge
    wrote(call(bridge, "notebook_write", {"page": "Cours de piano", "kind": "subject", "section": "status",
                                          "facts": ["Le mardi"], "category": ["dates"]}))
    assert page_of(bridge, "Sujets/Cours de piano.md")["categories_by"] == "auto"
    # the category alone, on a page that exists
    said = wrote(call(bridge, "notebook_write", {"page": "Atelier poterie", "category": ["work", "money"]}))
    assert text_of(said) == "Saved: Atelier poterie.", text_of(said)
    assert page_of(bridge, path)["categories"] == ["work", "money"]
