"""Carnet « Le sommaire », brick S4 (plan piste 9 §1, §3.5, §3.6; contracts/carnet/sommaire.md §3, changes 29–36):
what the bridge serves ready to show, the apps computing nothing.

- `GET /v1/notebook/summary`: the kept searches with their count, « Tout le Carnet », the categories with their count
  and grey words (the latest searches of the category, else its latest titles), « Toi » first in « Personnes », the
  moment's card, Santé locked (no count, no word);
- `GET /v1/notebook/category/{id}`: the sub-filters with their counts, « Cherché ici » (3), the latest;
- `GET/POST/DELETE /v1/notebook/kept`: six a space, in state.json, in the phone's copy (its version moves);
- `POST /v1/notebook/searched`: a search made, for « Cherché ici » and the grey words, never Santé, never a number,
  never seen by an agent;
- `space`: `all` (or none) is the whole Carnet; until S5 no note is Pro: `perso` is everything, `pro` nothing.
Every answer: `Cache-Control: no-store`. The 1.0 apps' answers stay as they are (test_carnet_sommaire.py)."""
import json
from datetime import timedelta

from bbkit.common import AUTH
from bbkit.mcp import call, text_of
from test_carnet_sommaire import index_of, today, write_by_hand, write_carnet

CAPABILITIES = ["categories", "categorize", "sorted", "index", "summary", "category", "kept", "searched"]


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


def in_dates(item, day):
    """The phone's rule (contract §1): `yearly` always, a `date` from 90 days ago to 730 ahead."""
    if item.get("yearly"):
        return True
    if item.get("date"):
        d = (day.fromisoformat(item["date"][:10]) - day).days
        return -90 <= d <= 730
    return False


def test_s4_the_summary_is_ready_to_show(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    # listed now that the routes answer
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sommaire"] == CAPABILITIES
    s = summary(bridge)
    assert set(s) == {"v", "lang", "title", "space", "spaces", "kept", "card", "total", "categories", "bar", "version"}
    assert (s["v"], s["lang"], s["title"], s["space"], s["spaces"], s["kept"]) == (1, "fr", "Carnet", "all", [], [])
    assert s["card"] is None or s["card"]["kind"] in ("ask", "follow", "misplaced", "sorted", "review"), s["card"]
    copy = index_of(bridge, lang="fr").json()
    items = copy["items"]
    assert s["version"] == copy["version"]
    assert s["total"] == {"count": len(items), "title": "Tout le Carnet"}
    assert s["bar"] == {"placeholder": "Cherche dans tout le Carnet"}
    # each category counts the copy's notes (Dates as of today), the empty ones hidden, Santé last and locked
    ids = [x["id"] for x in s["categories"]]
    order = ["people", "dates", "money", "home", "papers", "trips", "work", "other", "health"]
    assert ids == [x for x in order if x in ids] and ids[-1] == "health", ids
    assert line(s, "health") == {"id": "health", "title": "Santé", "count": None, "hint": "", "locked": True}
    day = today()
    for id in order[:-1]:
        n = sum(1 for i in items if (in_dates(i, day) if id == "dates" else id in i.get("categories", [])))
        got = line(s, id)
        assert (got["count"] if got else 0) == n, (id, got, n)
        if got:
            assert got["locked"] is False and got["hint"] and len(got["hint"].split(", ")) <= 3, got
            assert id in got["first"]["categories"], got["first"]
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


def test_s4_a_category_page_with_its_filters_searches_and_latest(bridges):
    bridge = bridges.new().start()
    write_carnet(bridge)
    write_by_hand(bridge)
    s = summary(bridge)
    papers = category(bridge, "papers")
    assert set(papers) == {"v", "id", "title", "count", "space", "filters", "filter", "searched", "recent", "bar"}
    assert (papers["v"], papers["id"], papers["title"], papers["count"], papers["space"], papers["filter"]) == \
        (1, "papers", "Papiers", line(s, "papers")["count"], "all", None)
    assert papers["bar"] == {"placeholder": "Cherche dans Papiers"} and papers["searched"] == []
    assert [(f["id"], f["title"]) for f in papers["filters"]] == [("invoices", "Factures"), ("contracts", "Contrats"),
                                                                  ("warranties", "Garanties")]
    counts = {f["id"]: f["count"] for f in papers["filters"]}
    # the insurance contract; the washing machine bought with its warranty
    assert counts["contracts"] >= 1 and counts["warranties"] >= 1, counts
    assert all("papers" in i["categories"] for i in papers["recent"]) and 1 <= len(papers["recent"]) <= 20
    for fid, n in counts.items():
        narrowed = category(bridge, "papers", filter=fid)
        assert narrowed["filter"] == fid and len(narrowed["recent"]) == min(n, 20), (fid, n, narrowed["recent"])
    warranty = category(bridge, "papers", filter="warranties")["recent"]
    assert any(i.get("open", {}).get("record", {}).get("type") == "purchase" for i in warranty), warranty
    money = {f["id"]: f["count"] for f in category(bridge, "money")["filters"]}
    assert set(money) == {"to_pay", "subscriptions", "purchases"} and money["subscriptions"] >= 1, money
    # Dates: still to come this week (to Sunday), this month (to its last day), later
    dates = category(bridge, "dates")
    day = today()
    sunday = day + timedelta(days=6 - day.weekday())
    month_end = (day.replace(day=28) + timedelta(days=4)).replace(day=1) - timedelta(days=1)
    coming = []
    for i in dates["recent"]:
        when = i.get("date") or (f"{day.year}-{i['yearly']}" if i.get("yearly") else None)
        assert when, i
    week = {f["id"]: f["count"] for f in dates["filters"]}
    assert set(week) == {"this_week", "this_month", "later"}
    # (the task due in 2 days, the interview in 5, a renewal in 9, a flight in 12, the passport in 20, the birthdays in
    # 23 and 41 days, a contract's end in 95, a warranty's end in 600)
    for offset in (2, 5, 9, 12, 20, 23, 41, 95, 600):
        coming.append(day + timedelta(days=offset))
    assert week["this_week"] >= sum(1 for d in coming if d <= sunday)
    assert week["this_month"] >= sum(1 for d in coming if d <= month_end)
    assert week["later"] >= sum(1 for d in coming if d > month_end)
    later = [i["date"] or "" for i in category(bridge, "dates", filter="later")["recent"] if i.get("date")]
    assert later == sorted(later), later
    # Personnes: the close ones (Marc and Claire, « proches », and « Toi ») and the others
    people = category(bridge, "people")
    close = {f["id"]: f["count"] for f in people["filters"]}
    assert close["close"] + close["others"] == people["count"] and close["close"] >= 5, close
    titles = {i["title"] for i in category(bridge, "people", filter="close")["recent"]}
    assert {"Marc Durand", "Claire Durand", "Toi"} & titles, titles
    assert category(bridge, "home")["filters"] == []
    # refused: an unknown sub-filter or category, Santé without its lock
    for path, params, status, code in (("papers", {"filter": "to_pay"}, 400, "filter_unknown"),
                                       ("gens", {}, 404, "category_unknown"),
                                       ("health", {}, 403, "health_locked"),
                                       ("papers", {"space": "x"}, 400, "space_unknown")):
        got = bridge.get(f"/v1/notebook/category/{path}", params=params, headers=AUTH)
        assert got.status_code == status and got.json()["detail"]["code"] == code, (path, got.status_code, got.text)
        assert got.headers.get("cache-control") == "no-store"
    # Santé behind the lock: read now, kept nowhere
    health = category(bridge, "health", private="1")
    assert (health["title"], health["filters"], health["searched"]) == ("Santé", [], [])
    # (its page and its line)
    assert [i["line"] for i in health["recent"] if i["kind"] == "line"] == ["Prednisolone le matin"], health["recent"]
    assert all(i["part"] == "health" for i in health["recent"]) and health["count"] == len(health["recent"])
    # Pro before S5: nothing
    assert category(bridge, "papers", space="pro")["count"] == 0


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
    # another space, its own six (before S5: nothing is Pro, its count is 0)
    pro = keep(bridge, q="claire", space="pro")
    assert [(k["q"], k["space"], k["count"]) for k in pro["kept"]] == [("claire", "pro", 0)]
    assert len(get(bridge, "/v1/notebook/kept")["kept"]) == 6
    assert get(bridge, "/v1/notebook/kept", space="pro")["kept"] == pro["kept"]
    # deleted
    left = bridge.delete("/v1/notebook/kept", params={"id": first["id"]}, headers=AUTH)
    assert left.status_code == 200 and len(left.json()["kept"]) == 5 and left.headers.get("cache-control") == "no-store"
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
    assert [k["q"] for k in get(bridge, "/v1/notebook/kept")["kept"]] == ["à payer ce mois-ci", "voyage", "maison", "impôts", "marc"]


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
