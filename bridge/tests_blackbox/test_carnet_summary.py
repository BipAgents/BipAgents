"""Carnet « Le sommaire », brick S4 (plan piste 9 §1, §3.5, §3.6; contracts/carnet/sommaire.md §3, changes 29–36):
what the bridge serves ready to show, the apps computing nothing.

- `GET /v1/notebook/summary`: the kept searches with their count, « Tout le Carnet », the categories with their count
  (the pages their screens list, Alex 26/11) and grey words (the latest searches of the category, else the titles of
  its pages), « Toi » first in « Personnes », the moment's card, Santé locked (no count, no word);
- `GET /v1/notebook/category/{id}`: its pages (people by their link in Personnes, subjects elsewhere, never a line or a
  file alone), Papiers' sub-filters counted in pages, « Cherché ici » (3), the latest notes (the 1.1 apps');
- `GET/POST/DELETE /v1/notebook/kept`: six a space, in state.json, in the phone's copy (its version moves);
- `POST /v1/notebook/searched`: a search made, for « Cherché ici » and the grey words, never Santé, never a number,
  never seen by an agent;
- `space`: `all` (or none) is the whole Carnet; until S5 no note is Pro: `perso` is everything, `pro` nothing.
Every answer: `Cache-Control: no-store`. The 1.0 apps' answers stay as they are (test_carnet_sommaire.py)."""
import json
from pathlib import Path

from bbkit.common import AUTH
from bbkit.mcp import call, text_of
from test_carnet_sommaire import index_of, write_by_hand, write_carnet

CAPABILITIES = ["categories", "categorize", "sorted", "index", "summary", "category", "kept", "searched", "spaces", "family"]


def get(bridge, path, status=200, **params):
    got = bridge.get(path, params=params, headers=AUTH)
    assert got.status_code == status, (path, got.status_code, got.text)
    assert got.headers.get("cache-control") == "no-store", (path, dict(got.headers))
    return got.json() if got.content else None


def summary(bridge, **params):
    return get(bridge, "/v1/notebook/summary", **{"lang": "fr", **params})


def category(bridge, id, status=200, **params):
    return get(bridge, f"/v1/notebook/category/{id}", status, **{"lang": "fr", **params})


def keep(bridge, status=200, **body):
    got = bridge.post("/v1/notebook/kept", json=body, headers=AUTH)
    assert got.status_code == status, (body, got.status_code, got.text)
    assert got.headers.get("cache-control") == "no-store"
    return got.json()


def searched(bridge, **body):
    got = bridge.post("/v1/notebook/searched", json=body, headers=AUTH)
    assert got.status_code == 204 and not got.content, (body, got.status_code, got.text)
    assert got.headers.get("cache-control") == "no-store"


def line(answer, id):
    return next((x for x in answer["categories"] if x["id"] == id), None)


def test_s4_the_summary_is_ready_to_show(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    # listed now that the routes answer
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sommaire"] == CAPABILITIES
    s = summary(bridge)
    assert set(s) == {"v", "lang", "title", "space", "spaces", "kept", "card", "total", "categories", "bar", "version", "epoch"}
    assert (s["v"], s["lang"], s["title"], s["space"], s["spaces"], s["kept"]) == (1, "fr", "Carnet", "all", [], [])
    assert s["card"] is None or s["card"]["kind"] in ("ask", "follow", "misplaced", "sorted", "review"), s["card"]
    copy = index_of(bridge, lang="fr").json()
    items = copy["items"]
    assert s["version"] == copy["version"]
    assert s["total"] == {"count": len(items), "title": "Tout le Carnet"}
    assert s["bar"] == {"placeholder": "Cherche dans tout le Carnet"}
    # each category counts the pages its screen lists (Alex 26/11), the empty ones hidden, Santé last and locked
    ids = [x["id"] for x in s["categories"]]
    order = ["people", "dates", "money", "home", "papers", "trips", "work", "other", "health"]
    assert ids == [x for x in order if x in ids] and ids[-1] == "health", ids
    assert line(s, "health") == {"id": "health", "title": "Santé", "count": None, "hint": "", "locked": True}
    for id in order[:-1]:
        got = line(s, id)
        rows = category(bridge, id)["pages"]
        assert (got["count"] if got else 0) == len(rows), (id, got, rows)
        if got:
            assert got["locked"] is False and got["hint"] and len(got["hint"].split(", ")) <= 3, got
            assert all(t in [r["title"] for r in rows] for t in got["hint"].split(", ")), (id, got["hint"])
    # a page, not its notes: « Personnes » counts Marc, Claire and « Toi »; « Maison » its subjects
    assert line(s, "people")["count"] == 3 and line(s, "home")["count"] == len(category(bridge, "home")["pages"])
    people = line(s, "people")
    assert (people["title"], people["first"]["title"], people["first"]["kind"]) == ("Personnes", "Toi", "page")
    # Santé: never a count, a word, a note outside its lock
    assert "Prednisolone" not in json.dumps(s, ensure_ascii=False)
    # the app's language: the names and texts, the Carnet's notes as they are
    en = summary(bridge, lang="en")
    assert (en["lang"], en["title"], line(en, "people")["title"], en["total"]["count"]) == ("en", "Notebook", "People", len(items))
    # a space: until S5 no note is Pro (`perso` the whole Carnet, `pro` nothing); another word is refused
    perso = summary(bridge, space="perso")
    assert (perso["space"], perso["total"], perso["categories"]) == ("perso", s["total"], s["categories"])
    pro = summary(bridge, space="pro")
    assert (pro["total"]["count"], pro["categories"], pro["kept"], pro["card"]) == (0, [], [], None)
    refused = bridge.get("/v1/notebook/summary", params={"space": "family"}, headers=AUTH)
    assert refused.status_code == 400 and refused.json()["detail"]["code"] == "space_unknown", refused.text


def test_s4_a_category_page_lists_its_pages(bridges):
    """Alex 26/11 (« chaque point est une liste de notes sans rapport »): a category lists its pages — the people in
    Personnes, its subjects elsewhere —, each with its number of notes, then a tap opens the page; never a line, a task
    or a file on its own."""
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    # a photo a person sent, about Marc: never a row (« photo.jpg » in Personnes, 26/11)
    received = Path(bridge.config["agents"]["Wellness"]["upload_dir_host"]) / "2026-11"
    (received / "0123456789ac-photo.jpg").write_bytes(b"\xff\xd8\xff demo")
    s = summary(bridge)
    papers = category(bridge, "papers")
    assert set(papers) == {"v", "id", "title", "count", "space", "filters", "filter", "searched", "recent", "pages", "groups", "bar"}
    assert (papers["v"], papers["id"], papers["title"], papers["count"], papers["space"], papers["filter"]) == \
        (1, "papers", "Papiers", line(s, "papers")["count"], "all", None)
    assert papers["count"] == len(papers["pages"])
    assert papers["bar"] == {"placeholder": "Cherche dans Papiers"} and papers["searched"] == []
    # Papiers keeps its sub-filters, counted in pages: the contracts' page, the purchases' (their warranties)
    assert [(f["id"], f["title"]) for f in papers["filters"]] == [("invoices", "Factures"), ("contracts", "Contrats"),
                                                                  ("warranties", "Garanties")]
    counts = {f["id"]: f["count"] for f in papers["filters"]}
    for fid, n in counts.items():
        narrowed = category(bridge, "papers", filter=fid)
        assert narrowed["filter"] == fid and len(narrowed["pages"]) == n, (fid, n, narrowed["pages"])
    assert [p["title"] for p in category(bridge, "papers", filter="contracts")["pages"]] == ["Contrats et assurances"]
    assert "Achats" in [p["title"] for p in category(bridge, "papers", filter="warranties")["pages"]]
    # the 1.1 apps' « Les plus récents » stays: the category's notes
    assert all("papers" in i["categories"] for i in papers["recent"]) and 1 <= len(papers["recent"]) <= 20
    # Personnes: « Toi », then the people, each with their link and their notes
    people = category(bridge, "people")
    rows = [(p["title"], p.get("link"), p["kind"], p["page_kind"]) for p in people["pages"]]
    assert rows == [("Toi", None, "page", "self"), ("Claire Durand", "Sœur", "page", "person"),
                    ("Marc Durand", "Beau-frère", "page", "person")], rows
    # (one group only, the family: no header)
    assert people["groups"] == [] and all(p["group"] is None for p in people["pages"])
    marc = next(p for p in people["pages"] if p["title"] == "Marc Durand")
    assert marc["notes"] == 3 and marc["close"] is True and marc["open"] == {"path": "Personnes/Marc Durand.md"}
    # Maison: its subjects by name, each with its notes
    home = category(bridge, "home")
    assert [p["title"] for p in home["pages"]] == sorted((p["title"] for p in home["pages"]), key=str.casefold)
    kitchen = next(p for p in home["pages"] if p["title"] == "Rénovation cuisine")
    assert kitchen["notes"] == 2 and kitchen["kind"] == "page"
    # Dates: the pages with a date to come, the soonest first, the date on each
    dates = category(bridge, "dates")
    days = [p["next"]["in_days"] for p in dates["pages"]]
    assert days == sorted(days) and all(d >= 0 for d in days), dates["pages"]
    assert ("Marc Durand", 23, "Anniversaire") in [(p["title"], p["next"]["in_days"], p["next"]["what"]) for p in dates["pages"]]
    assert ("Travail", 5) in [(p["title"], p["next"]["in_days"]) for p in dates["pages"]]
    # never a file, a task or a line alone, whatever the category
    for id in ("people", "dates", "money", "home", "papers", "trips", "work", "other"):
        listed = category(bridge, id)
        assert all(p["kind"] in ("page", "records") for p in listed["pages"]), (id, listed["pages"])
        assert not [p for p in listed["pages"] if p["title"].endswith((".jpg", ".pdf"))], (id, listed["pages"])
        assert not [p for p in listed["pages"] if p["title"] in ("Rappeler Marc pour le devis", "Trier les photos de l'été")]
    # the sub-filters that meant nothing for a list of pages are gone (Argent, Dates, Personnes)
    for id in ("money", "dates", "people", "home"):
        assert category(bridge, id)["filters"] == [], id
    # refused: an unknown sub-filter or category, Santé without its lock
    for path, params, status, code in (("papers", {"filter": "to_pay"}, 400, "filter_unknown"),
                                       ("people", {"filter": "close"}, 400, "filter_unknown"),
                                       ("gens", {}, 404, "category_unknown"),
                                       ("health", {}, 403, "health_locked"),
                                       ("papers", {"space": "x"}, 400, "space_unknown")):
        got = bridge.get(f"/v1/notebook/category/{path}", params=params, headers=AUTH)
        assert got.status_code == status and got.json()["detail"]["code"] == code, (path, got.status_code, got.text)
        assert got.headers.get("cache-control") == "no-store"
    # Santé behind the lock: its page and its latest, read now, kept nowhere
    health = category(bridge, "health", private="1")
    assert (health["title"], health["filters"], health["searched"]) == ("Santé", [], [])
    assert [i["line"] for i in health["recent"] if i["kind"] == "line"] == ["Prednisolone le matin"], health["recent"]
    assert all(i["part"] == "health" for i in health["recent"]) and health["count"] == len(health["pages"]) == 1
    # Pro before S5: nothing
    pro = category(bridge, "papers", space="pro")
    assert (pro["count"], pro["pages"]) == (0, [])


def test_s4_kept_searches_six_a_space_with_their_count(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    assert get(bridge, "/v1/notebook/kept") == {"kept": [], "max": 6}
    before = index_of(bridge, lang="fr")
    copy = before.json()

    def count(q, **more):
        found = get(bridge, "/v1/notebook/search", q=q, lang="fr", sommaire="1", **more)
        return found["counts"]["all"]

    kept = keep(bridge, q="anniversaire")
    assert kept["max"] == 6 and len(kept["kept"]) == 1
    first = kept["kept"][0]
    assert (first["q"], first["title"], first["space"], first["count"]) == ("anniversaire", "anniversaire", None, count("anniversaire"))
    assert first["count"] >= 2 and first["id"].startswith("k")
    # the words and the date phrase: counted again each day by the bridge (and by the phone, offline)
    paying = keep(bridge, q="à payer ce mois-ci", category="money")["kept"][1]
    assert (paying["category"], paying["count"]) == ("money", count("à payer ce mois-ci", category="money"))
    # the same search again: kept once
    assert len(keep(bridge, q=" Anniversaire ")["kept"]) == 2
    # renamed
    renamed = keep(bridge, id=first["id"], title="Les anniversaires")["kept"][0]
    assert (renamed["id"], renamed["title"], renamed["q"]) == (first["id"], "Les anniversaires", "anniversaire")
    # on the sommaire with their count; a new note found by one: its count moves
    assert summary(bridge)["kept"][0] == renamed
    wrote = call(bridge, "notebook_write", {"page": "Lucas Bernard", "kind": "person", "section": "dates",
                                            "facts": ["Anniversaire : 2 mars"]})
    assert wrote["isError"] is False, text_of(wrote)
    assert summary(bridge)["kept"][0]["count"] == renamed["count"] + 1
    # in the phone's copy: its version moves, the kept searches come whole (the words, never a count)
    after = index_of(bridge, lang="fr", since=str(copy["version"]), epoch=copy["epoch"]).json()
    assert after["version"] > copy["version"]
    assert [(k["q"], k["title"], k.get("category")) for k in after["kept"]] == \
        [("anniversaire", "Les anniversaires", None), ("à payer ce mois-ci", "à payer ce mois-ci", "money")]
    assert all(set(k) <= {"id", "q", "title", "category", "space"} for k in after["kept"])
    same = bridge.get("/v1/notebook/index", params={"lang": "fr"}, headers={**AUTH, "If-None-Match": before.headers["etag"]})
    assert same.status_code == 200
    # six a space
    for q in ("voyage", "maison", "impôts", "marc"):
        keep(bridge, q=q)
    full = bridge.post("/v1/notebook/kept", json={"q": "claire"}, headers=AUTH)
    assert full.status_code == 409 and full.json()["detail"]["code"] == "kept_full", full.text
    # another space, its own six (nothing is Pro here, its count is 0)
    pro = keep(bridge, q="claire", space="pro")
    assert [(k["q"], k["space"], k["count"]) for k in pro["kept"]] == [("claire", "pro", 0)]
    # (S5: `all` shows every space's kept searches: the setting off shows everything together; six a view, the newest)
    assert [k["q"] for k in get(bridge, "/v1/notebook/kept")["kept"]][-1] == "claire"
    assert len(get(bridge, "/v1/notebook/kept")["kept"]) == 6
    assert get(bridge, "/v1/notebook/kept", space="pro")["kept"] == pro["kept"]
    # deleted
    left = bridge.delete("/v1/notebook/kept", params={"id": first["id"]}, headers=AUTH)
    assert left.status_code == 200 and len(left.json()["kept"]) == 6 and left.headers.get("cache-control") == "no-store"
    gone = bridge.delete("/v1/notebook/kept", params={"id": first["id"]}, headers=AUTH)
    assert gone.status_code == 404 and gone.json()["detail"]["code"] == "kept_unknown"
    # refused: nothing typed, a number or a reference (never kept, never on the phone), Santé, an unknown category,
    # an unknown space
    for body, status, code in (({"q": "  "}, 400, "kept_empty"), ({"q": "dossier HR-55821", "space": "pro"}, 400, "number_refused"),
                               ({"q": "allergie", "category": "health", "space": "pro"}, 400, "category_not_given"),
                               ({"q": "x", "category": "gens", "space": "pro"}, 400, "category_unknown"),
                               ({"q": "x", "space": "family"}, 400, "space_unknown"),
                               ({"id": "k000000", "title": "x"}, 404, "kept_unknown")):
        got = bridge.post("/v1/notebook/kept", json=body, headers=AUTH)
        assert got.status_code == status and got.json()["detail"]["code"] == code, (body, got.status_code, got.text)
    # kept on the bridge: there after a restart
    bridge.restart()
    # (S5: `all` lists every space's, Pro's « claire » too)
    assert [k["q"] for k in get(bridge, "/v1/notebook/kept")["kept"]] == ["à payer ce mois-ci", "voyage", "maison", "impôts", "marc", "claire"]


def test_s4_a_search_made_feeds_searched_here_and_never_reaches_an_agent(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    etag = index_of(bridge, lang="fr").headers["etag"]
    searched(bridge, q="garantie lave-linge", category="papers")
    for q in ("contrat", "passeport", "assurance", "contrat"):
        searched(bridge, q=q, category="papers")
    papers = category(bridge, "papers")
    assert [x["q"] for x in papers["searched"]] == ["contrat", "assurance", "passeport"], papers["searched"]
    assert all(x["at"].endswith("Z") for x in papers["searched"])
    # the sommaire's grey words: the category's latest searches
    assert line(summary(bridge), "papers")["hint"] == "contrat, assurance, passeport"
    # without a category: under its best result's (the gift idea, on Marc's page)
    word = "qzxwvyk"
    searched(bridge, q=f"vinyle {word}")
    assert category(bridge, "people")["searched"][0]["q"] == f"vinyle {word}"
    assert line(summary(bridge), "people")["hint"].startswith(f"vinyle {word}")
    # on disk: its own file only (never a page, the state, the history, what the agents' tools read)
    holding = [str(p.relative_to(bridge.state_dir)) for p in bridge.state_dir.rglob("*")
               if p.is_file() and word.encode() in p.read_bytes()]
    assert holding == ["notebook/searched.json"], holding
    # never kept: Santé, a number or a reference, what finds nothing
    searched(bridge, q="prednisolone", category="health")
    searched(bridge, q="dossier HR-55821", category="papers")
    searched(bridge, q="zzqqxxw")
    assert category(bridge, "health", private="1")["searched"] == []
    assert "HR-55821" not in json.dumps(category(bridge, "papers"))
    for body, code in (({"q": "x", "category": "gens"}, "category_unknown"), ({"q": "x", "space": "x"}, "space_unknown")):
        got = bridge.post("/v1/notebook/searched", json=body, headers=AUTH)
        assert got.status_code == 400 and got.json()["detail"]["code"] == code, got.text
    # a search made never changes the phone's copy
    assert index_of(bridge, lang="fr").headers["etag"] == etag
    # never seen by an agent: its tools, the Carnet's pages, the history, the home, « Ce que tes agents savent »
    for name, args in (("notebook_search", {"words": word}), ("notebook_search", {"words": "marc"}),
                       ("notebook_read", {"page": "Marc Durand"})):
        assert word not in text_of(call(bridge, name, args)), name
    for path in ("/v1/notebook", "/v1/notebook/history", "/v1/notebook/known", f"/v1/notebook/search?q={word}&groups=1",
                 "/v1/notebook/index", "/v1/notebook/offline"):
        got = bridge.get(path, headers=AUTH)
        assert got.status_code == 200 and word not in got.text, path


def test_s4_oublie_ca_takes_the_searches_with_it(bridges):
    """« Oublie Marc »: his kept searches (every space) and the searches made about him (« Cherché ici », the grey
    words) go with the rest; the others stay; the phone's copy moves to a new version without them."""
    bridge = bridges.new().start()
    write_carnet(bridge)
    for q, space in (("marc", None), ("anniversaire", None), ("Marc", "pro")):
        keep(bridge, q=q, **({"space": space} if space else {}))
    for q, cat in (("claire", "people"), ("marc", "people"), ("Marc Durand vinyle", "money")):
        searched(bridge, q=q, category=cat)
    assert line(summary(bridge), "people")["hint"] == "marc, claire"
    before = index_of(bridge, lang="fr").json()
    preview = bridge.post("/v1/notebook/forget/preview", params={"lang": "fr"}, json={"page": "Marc Durand"}, headers=AUTH)
    assert preview.status_code == 200, preview.text
    assert bridge.post("/v1/notebook/forget/confirm", json={"token": preview.json()["token"]}, headers=AUTH).status_code == 200
    assert [k["q"] for k in get(bridge, "/v1/notebook/kept")["kept"]] == ["anniversaire"]
    assert get(bridge, "/v1/notebook/kept", space="pro")["kept"] == []
    assert [x["q"] for x in category(bridge, "people")["searched"]] == ["claire"]
    assert category(bridge, "money")["searched"] == []
    s = summary(bridge)
    assert line(s, "people")["hint"] == "claire" and "marc" not in json.dumps(s["kept"]).lower()
    after = index_of(bridge, lang="fr", since=str(before["version"]), epoch=before["epoch"]).json()
    assert after["version"] > before["version"] and [k["q"] for k in after["kept"]] == ["anniversaire"]
    assert b"marc" not in (bridge.state_dir / "notebook" / "searched.json").read_bytes().lower()


def test_s4_a_search_made_can_be_dropped_sante_open_keeps_none_and_the_version_has_its_epoch(bridges):
    """Review 1 of S4: B2 (`DELETE /v1/notebook/searched`: a search whose words are nowhere in the Carnet, that
    « Oublie ça » cannot find), R3 (a search made with Santé open is never kept), R4 (`epoch` with `version`)."""
    bridge = bridges.new().start()
    write_carnet(bridge)
    for q, cat in (("avocat divorce", "papers"), ("garantie", "papers"), ("claire", "people")):
        searched(bridge, q=q, category=cat)
    got = bridge.delete("/v1/notebook/searched", params={"q": "Avocat divorce"}, headers=AUTH)
    assert got.status_code == 204 and not got.content and got.headers.get("cache-control") == "no-store", got.text
    assert [x["q"] for x in category(bridge, "papers")["searched"]] == ["garantie"]
    got = bridge.delete("/v1/notebook/searched", headers=AUTH)
    assert got.status_code == 204 and got.headers.get("cache-control") == "no-store"
    assert category(bridge, "papers")["searched"] == [] and category(bridge, "people")["searched"] == []
    for params, code in (({"space": "x"}, "space_unknown"), ({"category": "gens"}, "category_unknown")):
        got = bridge.delete("/v1/notebook/searched", params=params, headers=AUTH)
        assert got.status_code == 400 and got.json()["detail"]["code"] == code, (params, got.status_code, got.text)
    # Santé open: never kept, whatever it finds in the shared Carnet
    searched(bridge, q="marc", private=True)
    searched(bridge, q="marc", category="people", private=True)
    assert category(bridge, "people")["searched"] == []
    # the sommaire's version with its epoch: the copy's
    copy = index_of(bridge, lang="fr").json()
    s = summary(bridge)
    assert (s["epoch"], s["version"]) == (copy["epoch"], copy["version"])
