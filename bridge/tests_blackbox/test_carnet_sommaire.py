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
   `category`.
3. **The Carnet from before, sorted once (brick S2)**: the nightly pass writes the categories in the pages' headers and
   nothing else (nothing moves, the person's own left alone, once), the small model for what stays in « Autres »
   within its cap, the card « Voir · OK », a re-filing's toast and today's list.
4. **The search's ground at the bridge (brick S3)**: the phone's copy (`GET /v1/notebook/index`: never Santé, a
   number or a reference, an archived mail; ETag and 304, what changed since the phone's version, another epoch:
   everything; gzip; no HTTP cache), the sommaire's search on today's route (`groups`, `counts`, `category`, Santé
   behind its lock) and the online files the person allowed for the Carnet (`online=1`, a switch per drive)."""
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
    # (dates as days from the run's day only: a computed legal warranty or a paper's reminders « 3 months before »
    # would move with the calendar, and the comparison runs any day)
    r("purchase", {"date": iso(-120), "item": "Lave-linge", "store": "Électro Plus", "price": "499 €",
                   "warranty_end": iso(600)})
    r("subscription", {"service": "Films en streaming", "price": "13,49 €", "period": "Mensuel", "renewal": iso(9)})
    r("contract", {"what": "Assurance habitation", "provider": "Assureur Horizon", "end": iso(95)})
    r("paper", {"document": "Passeport", "expires": iso(20)})
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
    # (S3: what the new search reads differently — a typo and a neighbour, a date phrase, a spoken question — leaves
    # the 1.0 keys as they were; recorded from carnet-s1 8b2c516b4, whose 1.0 answers are rc-1122's, the additive
    # keys taken out)
    ("search-aniv", "/v1/notebook/search?q=aniv%20marc&lang=fr"),
    ("search-week", "/v1/notebook/search?q=la%20semaine%20prochaine&lang=fr"),
    ("search-question", "/v1/notebook/search?q=C%27est%20quand%20l%27anniversaire%20de%20Claire%20%3F&lang=fr"),
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
    for offset in (2, 5, 9, 12, 20, 23, 41, 95, 600):
        text = text.replace(spoken(offset), "{date%+d}" % offset)
    # « noté le 22 nov. », « Noté par Wellness le 22 nov. 2026 »: the run's day, in words
    text = SHORT_DAY.sub("DAY", text)
    for pattern, mark in IDS:
        seen = {}
        text = pattern.sub(lambda m, seen=seen, mark=mark: seen.setdefault(m.group(0), f"{mark}{len(seen) + 1}"), text)
    return text


MONTH_WORDS = {"janvier", "fevrier", "mars", "avril", "mai", "juin", "juillet", "aout", "septembre", "octobre",
               "novembre", "decembre"}


def day_words_out(words):
    """`words` without the numbers of days, and without the month that follows one: a date's words."""
    out, after_day = [], False
    for w in words:
        if isinstance(w, str) and w.isdigit() and len(w) <= 2:
            after_day = True
            continue
        if not (after_day and w in MONTH_WORDS):
            out.append(w)
        after_day = False
    return out


def undated(text):
    """The offline copy's search `words` without the days of its dates: « 28 novembre » (the run's day + 5) splits
    into words the day normalisation does not see, so the comparison held only on the day it was recorded (23/11:
    « 27 » recorded on 22/11). Their month too: from 26/11 the run's day + 5 is « 1 decembre », recorded
    « novembre » (a month with no day before it stays: « Avril, 12 jours »)."""
    try:
        value = json.loads(text)
    except ValueError:
        return text

    def walk(v):
        if isinstance(v, dict):
            for k, x in v.items():
                if k == "words" and isinstance(x, list):
                    v[k] = day_words_out(x)
                else:
                    walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)
    walk(value)
    return json.dumps(value, ensure_ascii=False, sort_keys=True, indent=1)


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
        want = undated((FIXTURES / f"{name}.json").read_text().rstrip("\n"))
        if undated(normalised(as_1_0(route_of(path), answer))) != want:
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
    # what this bridge's Carnet can do of the sommaire (contracts/carnet/sommaire.json `Capability`; S2 `sorted`, S3 `index`)
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sommaire"] == ["categories", "categorize", "sorted", "index", "summary", "category", "kept", "searched", "spaces", "family"]
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
    # (a paper's reminder, made by the bridge: Papiers)
    assert tasks == {"Rappeler Marc pour le devis": ["home", "dates"], "Trier les photos de l'été": ["other"],
                     "Renouveler Passeport": ["papers", "dates"]}, tasks
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
    assert {k: done["target"][k] for k in ("path", "private")} == {"path": kitchen, "private": False}
    assert done["target"]["item"].startswith("page:")
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
    by_item = post({"item": "task:" + task["id"], "categories": ["home"]})    # an Item's id, as the phone has it
    assert by_item.status_code == 200 and by_item.json()["target"]["task"] == task["id"], by_item.text
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
    # housekeeping, not news: « Nouveau » shows the page's last real change
    recent = bridge.get("/v1/notebook", headers=AUTH).json()["recent"]
    assert recent[0]["path"] == "Sujets/Cours de piano.md" and not recent[0]["summary"].startswith("categories"), recent[0]


# -- brick S2: the Carnet a person had before the sommaire, sorted once at night ------------------------------------

# Pages as an older bridge wrote them (or the person in Obsidian), in their outline already (the nightly pass's
# re-filing into the outlines leaves them as they are): no `categories` yet.
BEFORE = {
    "Personnes/Léa.md": "---\nid: p0000000001\ntype: person\naliases: []\nupdated: 2026-10-01\n---\n# Léa\n\n## Dates\n"
                        "- Anniversaire : 12 mars — *Wellness, 2026-10-01*\n",
    "Sujets/Rénovation cuisine.md": "---\nid: p0000000002\ntype: subject\naliases: []\nupdated: 2026-10-01\n---\n"
                                    "# Rénovation cuisine\n\n## Où on en est\n- Devis reçu — *Wellness, 2026-10-01*\n",
    "Sujets/Divers.md": "---\nid: p0000000003\ntype: subject\naliases: []\n---\n# Divers\n\n## Où on en est\n"
                        "- Idée en vrac — *Wellness, 2026-10-01*\n",
    "Sujets/Jazz.md": "# Jazz\n\nColtrane d'abord, puis Mingus.\n",
}
# The person's own, never touched: filed by them, and an Obsidian property the bridge does not read.
THEIRS = {
    "Domaines/Maison.md": "---\ntype: area\ncategories:\n  - money\ncategories_by: person\n---\n# Maison\n",
    "Sujets/Lectures.md": "---\ntype: subject\ncategories:\n  - \"[[Livres]]\"\n---\n# Lectures\n",
}
HEADER = re.compile(r"^categories:.*\n(?:  - .*\n)*|^categories_by:.*\n", re.M)


def without_header(text):
    """A page's text without the sommaire's header lines (and the frontmatter they alone make)."""
    if not text.startswith("---\n"):
        return text
    head, sep, body = text[4:].partition("\n---\n")
    if not sep:
        return text
    kept = HEADER.sub("", head + "\n").rstrip("\n")
    return body if not kept else "---\n" + kept + sep + body


def carnet_files(bridge):
    root = bridge.state_dir / "notebook" / "carnet"
    return {str(p.relative_to(root)): p.read_bytes().decode() for p in sorted(root.rglob("*")) if p.is_file()}


def older_carnet(bridges, fakes, pages, **notebook):
    """A bridge whose Carnet has `pages` written before it starts (state: French, the outlines done), its nightly pass
    on the fake model."""
    bridge = bridges.new(change=lambda c: c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb",
                                                             "model": "small-eu", **notebook}))
    root = bridge.state_dir / "notebook"
    for path, text in pages.items():
        (root / "carnet" / path).parent.mkdir(parents=True, exist_ok=True)
        (root / "carnet" / path).write_bytes(text.encode())
    (root / "state.json").write_text(json.dumps({"language": "fr", "outline": 1}))
    bridge.start()
    give_token(bridge)
    return bridge


def tidy(bridge):
    got = bridge.post("/v1/notebook/tidy", headers=AUTH)
    assert got.status_code == 200, got.text
    return got.json()["run"]


def test_s2_the_carnet_from_before_is_sorted_once_at_night_and_nothing_moves(bridges, fakes):
    bridge = older_carnet(bridges, fakes, {**BEFORE, **THEIRS})
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sorted"] is None
    before = carnet_files(bridge)
    run = tidy(bridge)
    assert (run["categorized"], run["categories_v"]) == (4, 2), run
    after = carnet_files(bridge)
    # nothing moved or gone; each page changed by its header lines only; the person's own as they were
    for path, text in before.items():
        assert path in after, path
        assert without_header(after[path]) == without_header(text), path
    for path, text in THEIRS.items():
        assert after[path] == text, path
    assert "categories:\n  - home\ncategories_by: auto\nupdated: 2026-10-01\n" in after["Sujets/Rénovation cuisine.md"]
    assert after["Sujets/Jazz.md"] == "---\ncategories:\n  - other\ncategories_by: auto\n---\n" + BEFORE["Sujets/Jazz.md"]
    pages = pages_by_title(bridge)
    assert {t: (pages[t]["categories"], pages[t]["categories_by"]) for t in ("Léa", "Rénovation cuisine", "Divers", "Maison")} == {
        "Léa": (["people"], "auto"), "Rénovation cuisine": (["home"], "auto"), "Divers": (["other"], "auto"),
        "Maison": (["money"], "person")}
    # in each page's history, by the « rangement »; never in « Nouveau »
    changes = bridge.get("/v1/notebook/history", params={"path": "Sujets/Divers.md"}, headers=AUTH).json()["changes"]
    assert [(c["action"], c["by"], c["name"], c["summary"]) for c in changes] == [("categorize", "nightly", "rangement", "")]
    assert all(c["action"] != "categorize" for c in bridge.get("/v1/notebook", headers=AUTH).json()["recent"])
    # the card, once: what is in « Autres », « Voir » and « OK »
    card = bridge.get("/v1/notebook", params={"lang": "fr"}, headers=AUTH).json()["sorted"]
    assert card["other"] >= 4, card
    assert card == {"title": "Ton Carnet est rangé par catégories", "text": f"{card['other']} notes dans Autres",
                    "other": card["other"], "see": "Voir", "ok": "OK", "open": {"category": "other"}}
    assert bridge.get("/v1/notebook", params={"lang": "en"}, headers=AUTH).json()["sorted"]["title"] == \
        "Your Notebook is sorted into categories"
    # the next night: nothing written again
    again = tidy(bridge)
    now = carnet_files(bridge)
    changed = {p: now.get(p) for p in set(now) | set(after) if now.get(p) != after.get(p)}
    assert not changed and "categorized" not in again and "categories_v" not in again, (changed, again)
    wrong = bridge.post("/v1/notebook/sorted", json={"action": "later"}, headers=AUTH)
    assert wrong.status_code == 422, wrong.text
    seen = bridge.post("/v1/notebook/sorted", json={"action": "see"}, headers=AUTH)
    assert seen.status_code == 200 and seen.json() == {"card": None}, seen.text
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sorted"] is None


def model_chats(fakes):
    return [c for c in fakes.model.chats if c["body"]["messages"][0]["content"].startswith("You file notes")]


def test_s2_what_stays_in_other_goes_to_the_small_model_within_its_cap(bridges, fakes):
    bridge = older_carnet(bridges, fakes, {"Sujets/Divers.md": BEFORE["Sujets/Divers.md"],
                                           "Sujets/Jazz.md": BEFORE["Sujets/Jazz.md"]})
    fakes.model.category_lines = [{"n": 1, "categories": ["home"]}, {"n": 2, "categories": ["health"]}]
    run = tidy(bridge)
    assert (run["categorized"], run["categorized_by_model"], run["model_called"]) == (2, 1, True), run
    # the fake model's usage (800 in, 40 out) at the pass's prices: in the run and in the month's spending
    assert (run["tokens_in"], run["tokens_out"], run["eur"]) == (800, 40, 0.00046), run
    assert bridge.get("/v1/notebook/settings", headers=AUTH).json()["spent_this_month"] == 0.00046
    [chat] = model_chats(fakes)
    assert chat["authorization"] == "Bearer sk-nb" and chat["body"]["model"] == "small-eu"
    sent = chat["body"]["messages"][1]["content"]
    assert sent.startswith("1. Divers (subject)") and "\n2. Jazz (page)" in sent and "Idée en vrac" in sent, sent
    assert "Wellness" not in sent, sent
    pages = pages_by_title(bridge)
    assert (pages["Divers"]["categories"], pages["Divers"]["categories_by"]) == (["home"], "agent")
    # health is never given: Jazz stays in « Autres », and is not sent again while it stays as it is
    jazz = page_of(bridge, "Sujets/Jazz.md")
    assert (jazz["categories"], jazz["categories_by"]) == (["other"], "auto")
    again = tidy(bridge)
    assert "categorized_by_model" not in again and len(model_chats(fakes)) == 1, again
    # the monthly cap: no call at all
    capped = older_carnet(bridges, fakes, {"Sujets/Jazz.md": BEFORE["Sujets/Jazz.md"]}, monthly_cap=0.00001)
    run = tidy(capped)
    assert run["categories_reason"] == "monthly cap reached" and len(model_chats(fakes)) == 1, run


def test_s2_a_refiling_shows_its_toast_then_stays_in_todays_list(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    kitchen = "Sujets/Rénovation cuisine.md"
    done = bridge.post("/v1/notebook/categorize", params={"lang": "fr"}, json={"path": kitchen, "categories": ["money"]},
                       headers=AUTH)
    assert done.status_code == 200, done.text
    done = done.json()
    assert (done["toast"], done["undo_title"]) == ("Rangé dans Argent.", "Annuler"), done
    line = bridge.post("/v1/notebook/categorize", params={"lang": "en"},
                       json={"line": {"path": "Personnes/Marc Durand.md", "text": "Un vinyle de Chet Baker"},
                             "categories": ["money", "home"]}, headers=AUTH).json()
    assert line["toast"] == "Filed in Money and Home." and line["undo_title"] == "Undo", line
    # after the toast: today's list, the newest first, each with its undo
    today = bridge.get("/v1/notebook/categorize/today", params={"lang": "fr"}, headers=AUTH)
    assert today.status_code == 200, today.text
    today = today.json()
    assert today["undo_title"] == "Annuler"
    assert [(i["title"], i["categories"], i["text"]) for i in today["items"]] == [
        ("Un vinyle de Chet Baker", ["money", "home"], "Rangé dans Argent et Maison."),
        ("Rénovation cuisine", ["money"], "Rangé dans Argent.")]
    assert today["items"][1]["undo"] == done["undo"] and today["items"][1]["target"] == done["target"]
    assert done["target"]["path"] == kitchen and done["target"]["item"].startswith("page:")
    assert today["items"][1]["before"] == {"categories": ["home"], "categories_by": "auto"}
    undone = bridge.post("/v1/notebook/categorize/undo", json={"undo": done["undo"]}, headers=AUTH)
    assert undone.status_code == 200, undone.text
    assert page_of(bridge, kitchen)["categories"] == ["home"]
    left = bridge.get("/v1/notebook/categorize/today", headers=AUTH).json()["items"]
    assert [i["title"] for i in left] == ["Un vinyle de Chet Baker"]


def test_no_http_cache_keeps_the_carnet(bridges):
    """Every Carnet answer says `Cache-Control: no-store` (review of carnet-a1, B3): the phones' URLCache keeps no
    readable copy of the Carnet, the installed 1.0 apps' included (today's /offline and /search too)."""
    bridge = bridges.new().start()
    write_carnet(bridge)
    for name, path in READS:
        got = bridge.get(path, headers=AUTH)
        assert got.headers.get("cache-control") == "no-store", (name, dict(got.headers))
    for got in [bridge.get("/v1/notebook/page?path=Sujets/Rien.md", headers=AUTH),
                bridge.post("/v1/notebook/categorize", json={"path": "Sujets/Jardin.md", "categories": ["home"]}, headers=AUTH),
                bridge.post("/v1/notebook/categorize/undo", json={"undo": "u000000000000"}, headers=AUTH)]:
        assert got.headers.get("cache-control") == "no-store", (got.status_code, dict(got.headers))


# -- brick S3: the phone's copy and the sommaire's search ---------------------------------------------------------

def index_of(bridge, headers=None, **params):
    got = bridge.get("/v1/notebook/index", params=params, headers={**AUTH, **(headers or {})})
    assert got.status_code == 200, got.text
    assert got.headers.get("cache-control") == "no-store", dict(got.headers)
    return got


def write_by_hand(bridge):
    """What Obsidian or a sync leaves: a page without id, a line with a reference, an archived mail's link, a
    quantity, a short date; and the private part's page (Santé)."""
    carnet = bridge.state_dir / "notebook" / "carnet" / "Sujets"
    carnet.mkdir(parents=True, exist_ok=True)
    (carnet / "Atelier.md").write_text(
        "---\ntype: subject\nupdated: 2025-03-01\n---\n# Atelier\n\n## Où on en est\n"
        "- Code client HR-55821\n- Le mail [[archive:m01]] du menuisier\n- 250g de colle à bois\n"
        "- Livraison le 3/11\n- 1/2 litre de vernis\n")
    private = bridge.state_dir / "notebook" / "private" / "Domaines"
    private.mkdir(parents=True, exist_ok=True)
    (private / "Santé.md").write_text("---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n"
                                      "- Prednisolone le matin — *Wellness, 2026-11-01*\n")


def test_the_index_is_the_phones_copy_without_sante_numbers_or_mails(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    # the capability is listed only now that the route answers
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sommaire"] == ["categories", "categorize", "sorted", "index", "summary", "category", "kept", "searched", "spaces", "family"]
    got = index_of(bridge, lang="fr")
    etag = got.headers["etag"]
    copy = got.json()
    # (the tables' languages in it too: review 1 of S3, R2)
    assert re.fullmatch(r'"e[0-9a-f]{8}-v\d+-fr"', etag) and etag == f'"{copy["epoch"]}-v{copy["version"]}-fr"'
    assert (copy["v"], copy["full"], copy["since"], copy["lang"], copy["truncated"], copy["online_only"]) == (1, True, None, "fr", False, None)
    assert copy["limits"] == {"lines": 20000, "bytes": 8000000} and copy["dated"] == {"before": 90, "after": 730}
    assert copy["removed"] == [] and copy["kept"] == []
    text = json.dumps(copy, ensure_ascii=False)
    # never Santé, a reference, a booking's or contract's number, an archived mail
    for never in ("Prednisolone", "HR-55821", "ABC123", "archive:", "Santé.md"):
        assert never not in text, never
    items = copy["items"]
    lines = {i.get("line", ""): i for i in items}
    # a quantity stays; « le 3/11 » is a day near the page's `updated`, « 1/2 litre » none (review 2 of S1, R10-R11)
    assert "250g de colle à bois" in lines
    assert lines["Livraison le 3/11"]["date"] == "2024-11-03"
    assert "date" not in lines["1/2 litre de vernis"] and "yearly" not in lines["1/2 litre de vernis"]
    # compact items: only `id`, `kind`, `open` always there; never a default value, never Dates or Santé
    for i in items:
        assert {"id", "kind", "open"} <= set(i) and i["kind"] in ("page", "line", "task", "record", "document"), i
        for k, v in i.items():
            assert v not in (None, False, "", []) and (k, v) not in (("part", "main"), ("space", "perso")), (k, i)
        assert not set(i.get("categories", [])) & {"dates", "health"}, i
    ids = {i["id"] for i in items}
    assert any(x.startswith("page:f") for x in ids), "a page made by hand: its key from its path"
    kinds = {i["kind"] for i in items}
    assert kinds == {"page", "line", "task", "record", "document"}, kinds
    claire = next(i for i in items if i.get("line", "").startswith("Anniversaire") and i.get("title") == "Claire Durand")
    assert claire["yearly"] == iso(41)[5:] and claire["label"] == "Anniversaire" and claire["page"].startswith("page:p")
    received = next(i for i in items if i["kind"] == "document")
    assert received["title"] == "Devis cuisine.pdf" and "pdf" in received["fold"]
    # the tables, whole
    assert len(copy["neighbours"]) >= 250 and "medecin" in copy["neighbours"]["toubib"]
    phrases = {p["words"]: p for p in copy["dates"]["phrases"]}
    assert phrases["la semaine prochaine"]["from"] == {"unit": "week", "n": 1, "edge": "start"}
    assert copy["dates"]["feasts"]["christmas_eve"] == {"month": 12, "day": 24}
    assert copy["dates"]["feasts"]["ascension"] == {"easter": 39} and len(copy["dates"]["easter"]) == 10
    assert {"le", "de", "euh"} <= set(copy["stop"])
    # the phone has it: 304, its ETag again
    same = bridge.get("/v1/notebook/index", params={"lang": "fr"}, headers={**AUTH, "If-None-Match": etag})
    assert same.status_code == 304 and same.headers["etag"] == etag and same.headers.get("cache-control") == "no-store"
    # the app's language changed: its tables come, never a 304 (review 1 of S3, R2)
    other = bridge.get("/v1/notebook/index", params={"lang": "en"}, headers={**AUTH, "If-None-Match": etag})
    assert other.status_code == 200 and other.headers["etag"] != etag and "doc" in other.json()["neighbours"]
    # gzip when the phone accepts it
    packed = index_of(bridge, headers={"Accept-Encoding": "gzip"}, lang="fr")
    assert packed.headers.get("content-encoding") == "gzip" and packed.json()["items"] == items
    # a word the bridge does not know: refused
    assert bridge.get("/v1/notebook/index", params={"since": "x"}, headers=AUTH).status_code == 400


def test_the_index_sends_only_what_changed_since_the_phones_version(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    first = index_of(bridge, lang="fr").json()
    epoch, version = first["epoch"], first["version"]
    # nothing changed: the same version, nothing to send
    again = index_of(bridge, since=version, epoch=epoch).json()
    assert (again["version"], again["full"], again["items"], again["removed"]) == (version, False, [], [])
    # a line written: only it, the tables whole
    wrote(call(bridge, "notebook_write", {"page": "Marc Durand", "section": "gifts", "facts": ["Une montre de plongée"]}))
    diff = index_of(bridge, since=version, epoch=epoch).json()
    assert (diff["full"], diff["since"], diff["version"]) == (False, version, version + 1)
    assert [i.get("line") for i in diff["items"] if i["kind"] == "line"] == ["Une montre de plongée"], diff["items"]
    assert diff["removed"] == [] and len(diff["neighbours"]) >= 250 and diff["dates"]["phrases"]
    watch = next(i for i in diff["items"] if i.get("line") == "Une montre de plongée")
    # « Oublie ça »: the line leaves the phone at its next read
    preview = bridge.post("/v1/notebook/forget/preview", params={"lang": "fr"},
                          json={"texts": ["Une montre de plongée"], "page": "Marc Durand"}, headers=AUTH)
    assert preview.status_code == 200, preview.text
    assert bridge.post("/v1/notebook/forget/confirm", json={"token": preview.json()["token"]}, headers=AUTH).status_code == 200
    gone = index_of(bridge, since=version + 1, epoch=epoch).json()
    assert watch["id"] in gone["removed"] and gone["version"] == version + 2, gone
    assert "montre de plongée" not in json.dumps(gone, ensure_ascii=False)
    # another epoch, a version the bridge never sent: everything
    for since, e in ((version, "e00000000"), (version + 50, epoch)):
        whole = index_of(bridge, since=since, epoch=e).json()
        assert whole["full"] is True and whole["since"] is None and len(whole["items"]) == len(first["items"])


def test_the_sommaire_search_groups_by_category_on_todays_route(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    ids = {i["id"]: i for i in index_of(bridge, lang="fr").json()["items"]}

    def search(q, **more):
        got = bridge.get("/v1/notebook/search", params={"q": q, "lang": "fr", "groups": "1", **more}, headers=AUTH)
        assert got.status_code == 200, got.text
        assert got.headers.get("cache-control") == "no-store"
        return got.json()

    # an installed 1.0 app's search: today's keys only, the copy not made for it (review 1 of S3, R3)
    plain = bridge.get("/v1/notebook/search", params={"q": "anniversiare claire", "lang": "fr"}, headers=AUTH).json()
    assert set(plain) == {"answers", "pages", "tasks"}, set(plain)
    found = search("anniversiare claire")
    # today's keys stay, the sommaire's are added
    assert {"answers", "pages", "tasks", "groups", "counts", "online"} <= set(found)
    assert found["online"] is None
    first = found["groups"][0]
    assert (first["category"], first["title"]) == ("people", "Personnes")
    top = first["top"][0]
    assert top["line"].startswith("Anniversaire") and top["title"] == "Claire Durand" and top["id"] in ids
    # an Item: every key, Dates as of today
    assert {"id", "kind", "part", "path", "title", "line", "categories", "space", "source", "who", "archived", "open"} <= set(top)
    assert "dates" in top["categories"] and top["space"] == "perso" and top["part"] == "main"
    assert found["counts"]["all"] >= 1 and found["counts"]["people"] >= 1
    # a neighbour and the start of a word
    assert search("aniv marc")["groups"][0]["top"][0]["title"] == "Marc Durand"
    # a date phrase alone: what is dated in it (the task due in two days, the interview in five)
    soon = search("bientôt")
    listed = [i["line"] for g in soon["groups"] for i in g["top"]]
    assert soon["counts"]["all"] == soon["counts"]["dates"] >= 2, soon["counts"]
    assert "Rappeler Marc pour le devis" in listed, listed
    # one category: only its notes
    home = search("devis", category="home")
    assert home["groups"] and all("home" in i["categories"] for g in home["groups"] for i in g["top"])
    assert search("marc", space="pro")["counts"] == {"all": 0}
    # a new app asking for the sommaire's keys only: today's are not made
    only = search("anniversiare claire", sommaire="1")
    assert set(only) == {"groups", "counts", "online"} and only["groups"] == found["groups"]
    # Santé: only behind its lock, never in the shared search
    assert search("prednisolone")["counts"] == {"all": 0}
    locked = search("prednisolone", private="1")
    assert [(g["category"], g["top"][0]["part"]) for g in locked["groups"]] == [("health", "health")], locked["groups"]
    # an agent's page (create_page) is found too, with the agent that made it
    pages = bridge.state_dir / "agent-pages" / "wellness"
    pages.mkdir(parents=True, exist_ok=True)
    (pages / "pg0123456789.html").write_text("<html><body><h1>Budget du voyage</h1><p>Tokyo, Kyoto</p></body></html>")
    (pages / "pg0123456789.json").write_text(json.dumps({"title": "Budget du voyage", "updated": "2026-11-20T10:00:00+00:00"}))
    made = search("Budget du voyage")
    first = made["groups"][0]["top"][0]
    assert (first["kind"], first["title"]) == ("document", "Budget du voyage"), made["groups"]
    assert first["source"] and first["open"]["document"] == {"agent": "wellness", "file": "pg0123456789.html"}


def test_the_online_files_only_from_the_drives_the_person_allowed(bridges, fakes):
    from test_drives import FakeDrives, connect, install
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        connect(bridge, service, "onedrive", client="ios")
        assert bridge.put("/v1/drives/onedrive/folders", headers=AUTH,
                          json={"folders": [{"id": "D", "agents": ["wellness"]}]}).status_code == 200

        def online(q):
            got = bridge.get("/v1/notebook/search", params={"q": q, "lang": "fr", "online": "1", "sommaire": "1"}, headers=AUTH)
            assert got.status_code == 200, got.text
            return got.json()["online"]

        # off by default: nothing searched online
        assert all(d["carnet"] is False for d in bridge.get("/v1/drives", headers=AUTH).json()["drives"])
        assert online("texte") is None and not service.downloaded("search")
        # the person's switch on OneDrive only
        shown = bridge.put("/v1/drives/onedrive/carnet", headers=AUTH, json={"carnet": True})
        assert shown.status_code == 200 and shown.json()["carnet"] is True
        found = online("texte")
        assert found["title"] == "Dans tes fichiers en ligne"
        assert [i["id"] for i in found["items"]] == ["drive:onedrive:F2"], found
        item = found["items"][0]
        assert (item["kind"], item["title"], item["categories"], item["source"]) == ("drive", "deep.txt", ["papers"], "OneDrive")
        assert item["open"] == {"drive": {"account": "onedrive", "id": "F2"}}
        # nothing of it is in the phone's copy
        assert "deep.txt" not in json.dumps(index_of(bridge).json())
        # signed in again to the same account: the switch stays (review 1 of S3, R1)
        connect(bridge, service, "onedrive", client="ios")
        onedrive = next(d for d in bridge.get("/v1/drives", headers=AUTH).json()["drives"] if d["service"] == "onedrive")
        assert onedrive["carnet"] is True and [f["id"] for f in onedrive["folders"]] == ["D"], onedrive
        # Dropbox too: its chosen folder only, never the look-alike (the fake service keeps one token at a time:
        # OneDrive's ends here, and its search is left out quietly)
        connect(bridge, service, "dropbox", client="ios")
        assert bridge.put("/v1/drives/dropbox/folders", headers=AUTH,
                          json={"folders": [{"id": "id:w", "agents": ["wellness"]}]}).status_code == 200
        assert bridge.get("/v1/drives", headers=AUTH).json()["drives"][2]["carnet"] is False
        bridge.put("/v1/drives/dropbox/carnet", headers=AUTH, json={"carnet": True})
        ids = {i["id"] for i in online("plan")["items"]}
        assert "drive:dropbox:id:f" in ids and "drive:dropbox:id:z" not in ids and "drive:onedrive:X" not in ids
        # without `online=1`: never the network
        plain = bridge.get("/v1/notebook/search", params={"q": "plan", "groups": "1"}, headers=AUTH).json()
        assert plain["online"] is None
        bridge.put("/v1/drives/onedrive/carnet", headers=AUTH, json={"carnet": False})
        bridge.put("/v1/drives/dropbox/carnet", headers=AUTH, json={"carnet": False})
        assert online("plan") is None
    finally:
        service.close()


def test_no_http_cache_keeps_the_family_carnet(bridges, fakes):
    """The family's Carnet, kept by no HTTP cache either (review 2 of S1): at the holder's bridge and relayed to a
    member's."""
    from test_family_agent import make_family_agent
    from test_households_and_peers import household
    from test_shared_agents import link
    a = household(bridges, fakes, "alex", ["famille"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    make_family_agent(a)
    for bridge in (a, b):
        assert bridge.get("/v1/family-agent", headers=AUTH).json()["agent"]["id"].startswith("famille")
        got = bridge.get("/v1/family-agent/notebook", headers=AUTH)
        assert got.status_code == 200, got.text
        assert got.headers.get("cache-control") == "no-store", dict(got.headers)
