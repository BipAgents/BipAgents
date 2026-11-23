"""Brick S6 of the Carnet « Le sommaire » (briefs/plan-carnet-sommaire.md §4, contracts/carnet/sommaire.md §3d): the
family Carnet gets the same sommaire, over two linked bridges as its members use it. Alex's place holds the family's
agent and its Carnet (`family-notebook.json`); Marie's bridge relays her app's calls there, as today's family routes.

- **Categories**: the family's (`shopping, people, dates, home, papers, holidays, school, menus, other`), filed by the
  bridge's words at each write (a « Courses » list is Courses, « Inscrire Tom au judo » École), Dates computed; the
  person's correction wins (and teaches the next ones), undone in one gesture.
- **summary, category, search, index**: counted by the holder on the family's copy, relayed to the members; `no-store`.
- **Kept searches**: common to the household (6 at most, in `family-notebook.json`), seen by every member.
- **Searches made**: each person's own, kept by her own bridge, never on another member's place nor told to the
  agents.
- **« Passage de main »**: the member who takes the family's agent over gets its categories, kept searches and rules;
  her own searches were hers all along.
- **Leaving**: the phone's copy is dropped (`left: true`, another epoch), her searches made erased.
- **« Oublie ça »** on the holder: the person's own kept searches and searches made that say it go with her lines.
- **Old apps**: today's family routes answer exactly as before (JSON compared with the 1.0 bridge's).
"""
import datetime
import json
import os
import time
from pathlib import Path

import pytest

from bbkit.common import AUTH
from bbkit.kerfy_relay import KerfyRelay
from bbkit.mcp import call, give_token
from conftest import managed_install
from test_family_relay_anywhere import install as relay_install, link as relay_link, named
from test_households_and_peers import household
from test_shared_agents import link, wait

TOKEN = "agent-token-for-famille-0123456789"
FIXTURES = Path(__file__).with_name("fixtures") / "carnet_1_0"


def make_family_agent(bridge, agent="famille"):
    (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": agent, "created": 1.0, "lang": "fr"}))


def day(n):
    return (datetime.date.today() + datetime.timedelta(days=n)).isoformat()


def no_store(got):
    assert got.headers.get("cache-control") == "no-store", (got.request.url, got.headers)
    return got


def fam(bridge, route, method="get", **kw):
    """A route of the family Carnet's sommaire, as an app calls it (`?lang=fr`)."""
    params = kw.pop("params", {})
    return no_store(getattr(bridge, method)(f"/v1/family-agent/notebook/{route}", params={"lang": "fr", **params},
                                            headers={**AUTH, **kw.pop("headers", {})}, **kw))


def ok(got, status=200):
    assert got.status_code == status, (got.request.url, got.status_code, got.text)
    return got.json() if got.content else None


def change(bridge, **body):
    got = bridge.post("/v1/family-agent/notebook", headers=AUTH, json=body)
    assert got.status_code == 200, got.text
    return got.json()


def seed(b):
    """The family Carnet, written from Marie's app (relayed to Alex's place): three lists and two pages."""
    ids = {}
    for title, kind in (("Courses", "list"), ("Tâches", "tasks"), ("Dates", "dates")):
        view = change(b, op="list_save", title=title, kind=kind)
        ids[title] = next(x["id"] for x in view["lists"] if x["title"] == title)
    for list_title, text, due in (("Courses", "Lait", None), ("Courses", "Pâtes complètes", None),
                                  ("Tâches", "Inscrire Tom au judo", None), ("Tâches", "Rendez-vous chaudière", day(3)),
                                  ("Dates", "Anniversaire de Mamie", day(6))):
        view = change(b, op="item_add", list=ids[list_title], text=text, **({"due": due} if due else {}))
    for title, text in (("Vacances de février", "Gîte à Annecy, du 14 au 21 février.\nReste à réserver : le train aller."),
                        ("Menus de la semaine", "Lundi : pâtes au citron.\nMardi : soupe et tartines.")):
        view = change(b, op="page_save", title=title, text=text)
    for x in view["lists"]:
        for i in x["items"]:
            ids[i["text"]] = f"fam:list:{x['id']}/{i['id']}"
    for p in view["pages"]:
        ids[p["title"]] = f"fam:page:{p['id']}"
        ids["page " + p["title"]] = p["id"]
    return ids


def two(bridges, fakes):
    a = household(bridges, fakes, "alex", ["famille"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    make_family_agent(a)
    assert b.get("/v1/family-agent", headers=AUTH).json()["agent"]["id"] == "famille__alex"
    return a, b, seed(b)


def line(summary, cid):
    return next((x for x in summary["categories"] if x["id"] == cid), None)


def files_holding(bridge, word):
    """The files of a bridge's data folder that hold `word` (bytes, case ignored)."""
    out = []
    for path in bridge.state_dir.rglob("*"):
        if path.is_file() and word.lower().encode() in path.read_bytes().lower():
            out.append(path.relative_to(bridge.state_dir).as_posix())
    return sorted(out)


# -- the sommaire, relayed -------------------------------------------------------------------------------------------

def test_s6_the_family_carnet_has_its_sommaire_relayed(bridges, fakes):
    a, b, ids = two(bridges, fakes)
    # what each bridge's Carnet can do: S6 `family`
    for x in (a, b):
        assert x.get("/v1/notebook", headers=AUTH).json()["sommaire"][-1] == "family"

    want = [("shopping", 2), ("people", 1), ("dates", 2), ("home", 1), ("holidays", 1), ("school", 1), ("menus", 1)]
    theirs = ok(fam(b, "summary"))
    mine = ok(fam(a, "summary"))
    for s, others in ((theirs, "Avec Alex"), (mine, "Avec Marie")):
        assert s["v"] == 1 and s["lang"] == "fr" and s["title"] == "Carnet" and s["with"] == others
        assert [(x["id"], x["count"]) for x in s["categories"]] == want, s["categories"]
        assert {x["locked"] for x in s["categories"]} == {False} and not line(s, "health") and not line(s, "other")
        assert s["total"] == {"count": 7, "title": "Tout le Carnet"} and s["kept"] == []
        assert s["bar"] == {"placeholder": "Cherche pour la famille"}
        # the grey words: the latest notes' lines (no search made yet)
        assert line(s, "shopping")["hint"] == "Pâtes complètes, Lait"
        assert line(s, "school")["hint"] == "Inscrire Tom au judo"
        assert line(s, "dates")["first"]["line"] == "Rendez-vous chaudière"
    # the holder's copy, relayed as it is: the same version of the same epoch
    assert (theirs["version"], theirs["epoch"]) == (mine["version"], mine["epoch"])

    # the phone's copy: the family's notes, its categories without Dates (the phone adds it), an ETag, a diff
    got = fam(b, "index")
    index = ok(got)
    etag = got.headers["etag"]
    assert index["full"] is True and index["epoch"] == mine["epoch"] and "left" not in index
    items = {i["id"]: i for i in index["items"]}
    assert set(items) == {ids[k] for k in ("Lait", "Pâtes complètes", "Inscrire Tom au judo", "Rendez-vous chaudière",
                                           "Anniversaire de Mamie", "Vacances de février", "Menus de la semaine")}
    judo = items[ids["Inscrire Tom au judo"]]
    assert (judo["kind"], judo["title"], judo["line"], judo["categories"], judo["who"]) == (
        "list_item", "Tâches", "Inscrire Tom au judo", ["school"], "Marie")
    assert judo["fold"] == "inscrire tom au judo" and judo["open"]["item"] in ids["Inscrire Tom au judo"]
    assert items[ids["Rendez-vous chaudière"]]["date"] == day(3) and items[ids["Rendez-vous chaudière"]]["categories"] == ["home"]
    mamie = items[ids["Anniversaire de Mamie"]]
    assert mamie["yearly"] == day(6)[5:] and "date" not in mamie and mamie["categories"] == ["people"]
    assert items[ids["Vacances de février"]]["kind"] == "page" and items[ids["Vacances de février"]]["categories"] == ["holidays"]
    assert items[ids["Menus de la semaine"]]["categories"] == ["menus"]
    assert not any("dates" in i.get("categories", []) for i in index["items"])
    assert index["neighbours"] and index["dates"] and index["kept"] == []
    assert fam(b, "index", headers={"If-None-Match": etag}).status_code == 304
    # a change: only what changed (Marie's app, then Alex's)
    lid = ids["Lait"].split(":")[2].split("/")[0]
    view = change(a, op="item_add", list=lid, text="Citrons")
    citrons = next(i for i in view["lists"][0]["items"] if i["text"] == "Citrons")
    diff = ok(fam(b, "index", params={"since": str(index["version"]), "epoch": index["epoch"]}))
    assert diff["full"] is False and [i["line"] for i in diff["items"]] == ["Citrons"] and diff["removed"] == []
    assert diff["items"][0]["who"] == "Alex" and diff["items"][0]["categories"] == ["shopping"]
    change(b, op="item_delete", list=lid, id=citrons["id"])
    gone = ok(fam(b, "index", params={"since": str(diff["version"]), "epoch": diff["epoch"]}))
    assert gone["items"] == [] and gone["removed"] == [f"fam:list:{lid}/{citrons['id']}"]
    # another epoch, or none: everything
    assert ok(fam(b, "index", params={"since": "1", "epoch": "other"}))["full"] is True

    # the search, on the same copy (typos, a word's neighbours)
    found = ok(fam(b, "search", params={"q": "judo"}))
    assert found["groups"][0]["category"] == "school" and found["groups"][0]["title"] == "École"
    assert found["groups"][0]["top"][0]["line"] == "Inscrire Tom au judo" and found["counts"]["all"] == 1
    typo = ok(fam(b, "search", params={"q": "aniversaire mamie"}))
    assert any(t["line"] == "Anniversaire de Mamie" for g in typo["groups"] for t in g["top"]), typo
    assert ok(fam(b, "search", params={"q": "judo", "category": "menus"}))["counts"]["all"] == 0

    # a category's page
    school = ok(fam(b, "category/school"))
    assert (school["id"], school["title"], school["count"], school["searched"]) == ("school", "École", 1, [])
    assert [r["line"] for r in school["recent"]] == ["Inscrire Tom au judo"]
    assert school["bar"] == {"placeholder": "Cherche dans École"}
    dates = ok(fam(a, "category/dates"))
    assert [r["line"] for r in dates["recent"]] == ["Rendez-vous chaudière", "Anniversaire de Mamie"]
    for unknown in ("health", "money", "trips", "gens"):
        got = fam(b, f"category/{unknown}")
        assert got.status_code == 404 and got.json()["detail"]["code"] == "category_unknown", (unknown, got.text)

    # the selector of the person's own sommaire: « Moi · Famille » for a member of a family
    for x in (a, b):
        spaces = x.get("/v1/notebook/summary", params={"lang": "fr"}, headers=AUTH).json()["spaces"]
        assert [(s["id"], s["title"], s["selected"]) for s in spaces] == [("all", "Moi", True), ("family", "Famille", False)]


def test_s5_s6_the_selector_with_the_setting_on_is_perso_pro_then_famille(bridges, fakes):
    """S5 and S6 together (int-1.1d): the setting « Séparer Perso, Pro et Famille » on, the person's own selector is
    Perso, Pro when there is a Pro note, then Famille for a member of a family (never out of one); off again,
    « Moi · Famille » (contracts/carnet/sommaire.md §3c)."""
    a, b, _ = two(bridges, fakes)
    alone = household(bridges, fakes, "zoe", ["vie"])

    def selector(x, space):
        got = x.get("/v1/notebook/summary", params={"lang": "fr", "space": space}, headers=AUTH)
        assert got.status_code == 200, got.text
        return [(s["id"], s["title"], s["selected"]) for s in got.json()["spaces"]]

    def setting(x, on):
        got = x.patch("/v1/notebook/settings", json={"separate_spaces": on}, headers=AUTH)
        assert got.status_code == 200 and got.json()["separate_spaces"] is on, got.text

    assert selector(alone, "all") == []
    for x in (a, b, alone):
        setting(x, True)
    for x in (a, b):
        assert selector(x, "perso") == [("perso", "Perso", True), ("family", "Famille", False)]
    assert selector(alone, "perso") == [("perso", "Perso", True)]
    # a Pro note: Pro, then Famille
    made = b.post("/v1/notebook/page", json={"kind": "subject", "title": "Salon du meuble", "space": "pro"}, headers=AUTH)
    assert made.status_code == 201, made.text
    assert selector(b, "pro") == [("perso", "Perso", False), ("pro", "Pro", True), ("family", "Famille", False)]
    assert selector(b, "all") == [("perso", "Perso", False), ("pro", "Pro", False), ("family", "Famille", False)]
    en = b.get("/v1/notebook/summary", params={"lang": "en", "space": "perso"}, headers=AUTH).json()["spaces"]
    assert [s["title"] for s in en] == ["Personal", "Work", "Family"]
    # off again: « Moi · Famille » (the Pro note changes nothing), none out of a family
    for x in (b, alone):
        setting(x, False)
    assert selector(b, "all") == [("all", "Moi", True), ("family", "Famille", False)]
    assert selector(alone, "all") == []


def test_s6_the_bridge_files_the_family_carnet_and_the_person_wins(bridges, fakes):
    """At each write, by the bridge's words (the family's agent too, its tools unchanged); « Ranger ailleurs » is the
    person's and wins (the next writes keep it, it teaches the next notes), undone in one gesture."""
    a, b, ids = two(bridges, fakes)
    # the family's agent writes: filed by the same code
    give_token(a, "famille", TOKEN)
    wrote = call(a, "family_notebook_write", {"op": "page_save", "title": "Rentrée scolaire",
                                              "text": "Acheter les fournitures, la cantine à payer."}, token=TOKEN)
    assert wrote["isError"] is False and "categor" not in wrote["content"][0]["text"]
    s = ok(fam(b, "search", params={"q": "rentree"}))
    assert s["groups"][0]["category"] == "school"
    # Marie files « Menus de la semaine » in Maison
    page = ids["page Menus de la semaine"]
    done = ok(fam(b, "categorize", method="post", json={"page": page, "categories": ["home"]}))
    assert (done["categories"], done["categories_by"], done["before"]) == (
        ["home"], "person", {"categories": ["menus"], "categories_by": "auto"})
    assert done["target"] == {"page": page} and done["toast"] == "Rangé dans Maison." and done["undo"]
    stored = json.loads((a.state_dir / "family-notebook.json").read_text())
    menus = next(p for p in stored["pages"] if p["id"] == page)
    assert (menus["categories"], menus["categories_by"]) == (["home"], "person")
    # her choice stays when the page is written again
    change(a, op="page_save", id=page, text="Lundi : gratin.\nMardi : soupe.")
    assert line(ok(fam(a, "summary")), "home")["count"] == 2   # the boiler and the menus
    # and the word that had decided now files the next ones her way
    change(b, op="page_save", title="Menus de Noël", text="Huîtres, dinde.")
    noel = next(i for i in ok(fam(b, "index"))["items"] if i.get("title") == "Menus de Noël")
    assert noel["categories"] == ["home"]
    # an item by its index id; a list (its items follow it)
    item = ok(fam(b, "categorize", method="post", json={"item": ids["Inscrire Tom au judo"], "categories": ["other"]}))
    assert item["target"] == {"list": ids["Inscrire Tom au judo"].split(":")[2].split("/")[0],
                              "item": ids["Inscrire Tom au judo"].split("/")[1]}
    courses = ids["Lait"].split(":")[2].split("/")[0]
    moved = ok(fam(b, "categorize", method="post", json={"list": courses, "categories": ["menus", "home"]}))
    assert moved["before"] == {"categories": ["shopping"], "categories_by": "auto"}
    assert moved["toast"] == "Rangé dans Menus et Maison."
    assert line(ok(fam(b, "summary")), "shopping") is None
    # undone: as it was
    back = ok(fam(a, "categorize/undo", method="post", json={"undo": moved["undo"]}))
    assert back["categories"] == ["shopping"] and back["categories_by"] == "auto"
    assert line(ok(fam(b, "summary")), "shopping")["count"] == 2
    again = fam(a, "categorize/undo", method="post", json={"undo": moved["undo"]})
    assert again.status_code == 404 and again.json()["detail"]["code"] == "undo_gone"
    # refused (S1's codes): a personal category, Dates (computed), three, none, a note that isn't there
    for body, code in (({"page": page, "categories": ["money"]}, "category_unknown"),
                       ({"page": page, "categories": ["dates"]}, "category_not_given"),
                       ({"page": page, "categories": ["home", "menus", "school"]}, "category_count"),
                       ({"page": page, "categories": []}, "category_count"),
                       ({"page": "nope", "categories": ["home"]}, "not_found")):
        got = fam(b, "categorize", method="post", json=body)
        assert got.status_code in (400, 404) and got.json()["detail"]["code"] == code, (body, got.text)
    # (review 1 R6b) a list of tasks or of dates has no category of its own: its items are filed, not it
    tasks = ids["Inscrire Tom au judo"].split(":")[2].split("/")[0]
    got = fam(b, "categorize", method="post", json={"list": tasks, "categories": ["school"]})
    assert got.status_code == 400 and got.json()["detail"]["code"] == "list_not_filed", got.text
    # renamed, an auto list is filed again; the person's stays hers
    view = change(b, op="list_save", title="Bricolage", kind="list")
    tools = next(x["id"] for x in view["lists"] if x["title"] == "Bricolage")
    change(b, op="list_save", id=tools, title="Courses du week-end")
    change(b, op="item_add", list=tools, text="Pain")
    pain = next(i for i in ok(fam(b, "index"))["items"] if i.get("line") == "Pain")
    assert pain["categories"] == ["shopping"]


def test_s6_kept_searches_are_the_households(bridges, fakes):
    a, b, ids = two(bridges, fakes)
    before = fam(b, "index").headers["etag"]
    kept = ok(fam(b, "kept", method="post", json={"q": "courses"}))
    assert kept["max"] == 6 and [(k["q"], k["title"], k["count"], k["space"]) for k in kept["kept"]] == [
        ("courses", "courses", 2, None)]
    # common: Alex sees Marie's, and the other way round
    ok(fam(a, "kept", method="post", json={"q": "judo", "category": "school"}))
    for x in (a, b):
        assert [(k["q"], k.get("category"), k["count"]) for k in ok(fam(x, "kept"))["kept"]] == [
            ("courses", None, 2), ("judo", "school", 1)]
    # renamed by one, seen by the other; in the sommaire with their count
    jid = ok(fam(a, "kept"))["kept"][1]["id"]
    ok(fam(b, "kept", method="post", json={"id": jid, "title": "Le judo de Tom"}))
    assert [k["title"] for k in ok(fam(a, "summary"))["kept"]] == ["courses", "Le judo de Tom"]
    # in the phone's copy (a version of its own)
    got = fam(b, "index")
    assert got.headers["etag"] != before and [k["title"] for k in got.json()["kept"]] == ["courses", "Le judo de Tom"]
    # kept on the holder's place only, never in what the family's agent reads
    assert files_holding(a, "Le judo de Tom") == ["family-notebook.json"] and files_holding(b, "Le judo de Tom") == []
    give_token(a, "famille", TOKEN)
    assert "judo de Tom" not in call(a, "family_notebook_read", {}, token=TOKEN)["content"][0]["text"]
    # refused: a number, nothing, a personal category; the same words again: once
    for body, code in (({"q": "FR-2026-00123"}, "number_refused"), ({"q": " "}, "kept_empty"),
                       ({"q": "lait", "category": "money"}, "category_unknown")):
        got = fam(b, "kept", method="post", json=body)
        assert got.status_code == 400 and got.json()["detail"]["code"] == code, (body, got.text)
    assert len(ok(fam(a, "kept", method="post", json={"q": "Courses"}))["kept"]) == 2
    # 6 for the household
    for q in ("lait", "mamie", "chaudiere", "vacances"):
        ok(fam(a if q < "m" else b, "kept", method="post", json={"q": q}))
    full = fam(b, "kept", method="post", json={"q": "menus"})
    assert full.status_code == 409 and full.json()["detail"]["code"] == "kept_full"
    # deleted by one: gone for both
    assert [k["q"] for k in ok(fam(b, "kept", method="delete", params={"id": jid}))["kept"]] == [
        "courses", "lait", "mamie", "chaudiere", "vacances"]
    assert len(ok(fam(a, "kept"))["kept"]) == 5
    got = fam(a, "kept", method="delete", params={"id": "knope"})
    assert got.status_code == 404 and got.json()["detail"]["code"] == "kept_unknown"


def test_s6_searches_made_are_each_persons_own(bridges, fakes):
    a, b, ids = two(bridges, fakes)
    assert fam(b, "searched", method="post", json={"q": "karaté", "category": "school"}).status_code == 204
    # without a category, on a member's place: kept nowhere and sent nowhere (review 1 R2: her words never go to the
    # holder; the app sends the category of its best group)
    assert fam(b, "searched", method="post", json={"q": "natation"}).status_code == 204
    assert fam(b, "searched", method="post", json={"q": "judo", "category": "school"}).status_code == 204
    # never kept: a single letter, Santé open
    for body in ({"q": "x", "category": "school"}, {"q": "piscine", "category": "school", "private": True}):
        assert fam(b, "searched", method="post", json=body).status_code == 204
    got = fam(b, "searched", method="post", json={"q": "lait", "category": "money"})
    assert got.status_code == 400 and got.json()["detail"]["code"] == "category_unknown"
    assert [x["q"] for x in ok(fam(b, "category/school"))["searched"]] == ["judo", "karaté"]
    assert line(ok(fam(b, "summary")), "school")["hint"] == "judo, karaté"
    # Alex's own, on his place
    assert fam(a, "searched", method="post", json={"q": "chaudière", "category": "home"}).status_code == 204
    assert [x["q"] for x in ok(fam(a, "category/home"))["searched"]] == ["chaudière"]
    # never shown to another member
    assert ok(fam(a, "category/school"))["searched"] == []
    assert line(ok(fam(a, "summary")), "school")["hint"] == "Inscrire Tom au judo"
    assert ok(fam(b, "category/home"))["searched"] == []
    assert line(ok(fam(b, "summary")), "home")["hint"] == "Rendez-vous chaudière"
    # kept by her own bridge, nowhere else: not on the holder's place, not in the copy, not for the agent
    assert files_holding(b, "karat") == ["family-searched.json"] and files_holding(a, "karat") == []
    assert files_holding(a, "piscine") == [] and files_holding(b, "piscine") == []
    assert files_holding(b, "natation") == [] and files_holding(a, "natation") == []
    # on the holder's own place, without a category: under its best result (nothing leaves the place)
    assert fam(a, "searched", method="post", json={"q": "judo"}).status_code == 204
    assert [x["q"] for x in ok(fam(a, "category/school"))["searched"]] == ["judo"]
    assert fam(a, "searched", method="delete", params={"q": "judo"}).status_code == 204
    assert "karat" not in json.dumps(ok(fam(b, "index"))).lower()
    give_token(a, "famille", TOKEN)
    assert "karat" not in call(a, "family_notebook_read", {}, token=TOKEN)["content"][0]["text"].lower()
    assert oct(os.stat(b.state_dir / "family-searched.json").st_mode & 0o777) == "0o600"
    # dropped by her
    assert fam(b, "searched", method="delete", params={"q": "Karaté"}).status_code == 204
    assert [x["q"] for x in ok(fam(b, "category/school"))["searched"]] == ["judo"]
    assert fam(b, "searched", method="delete").status_code == 204
    assert ok(fam(b, "category/school"))["searched"] == []


# -- the passage de main, leaving ------------------------------------------------------------------------------------

def test_s6_the_passage_de_main_takes_the_categories_the_kept_searches_and_the_rules(bridges, fakes):
    a = household(bridges, fakes, "alex", ["famille"])
    b = bridges.new("marie")
    managed_install(b, fakes, agents=("vie",), public_hub_url="https://mini.ts.net:8642", isolated=True)
    b.config["public_url"] = b.url
    b.start()
    link(a, b)
    make_family_agent(a)
    assert b.get("/v1/family-agent", headers=AUTH).json()["agent"]["id"] == "famille__alex"
    ids = seed(b)
    page = ids["page Menus de la semaine"]
    ok(fam(b, "categorize", method="post", json={"page": page, "categories": ["home"]}))
    ok(fam(b, "kept", method="post", json={"q": "courses"}))
    ok(fam(a, "kept", method="post", json={"q": "lait"}))
    assert fam(b, "searched", method="post", json={"q": "karaté", "category": "school"}).status_code == 204
    first = ok(fam(b, "index"))

    # Alex leaves Marie's family: what is the family's goes to her place, waiting to be taken over
    assert a.delete("/v1/family/marie", headers=AUTH).status_code == 204
    assert wait(lambda: b.get("/v1/family-agent", headers=AUTH).json()["handover"] is not None)
    handed = json.loads((b.state_dir / "family-agent.json").read_text())["handover"]["notebook"]
    # (review 1 R3) each side keeps only its own household's kept searches: Marie's go with her, as hers; Alex's stay
    assert [(k["q"], k["by_household"]) for k in handed["kept"]] == [("courses", "self")] and handed["category_rules"]
    assert [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait"]
    assert b"courses" not in json.dumps(json.loads((a.state_dir / "family-notebook.json").read_text())["kept"]).encode()
    assert next(p for p in handed["pages"] if p["id"] == page)["categories_by"] == "person"
    # meanwhile she has no family Carnet: her phone drops its copy, her own searches wait for the taking over
    left = fam(b, "index")
    assert ok(left) == {**left.json(), "left": True, "full": True, "items": [], "kept": [], "removed": []}
    assert left.json()["epoch"] != first["epoch"]
    got = fam(b, "summary")
    assert got.status_code == 404 and got.json()["detail"]["code"] == "no_family"
    assert (b.state_dir / "family-searched.json").exists()

    # she takes it over: the same Carnet on her place, its categories, kept searches and rules with it
    took = b.post("/v1/family-agent", headers=AUTH, json={"take_over": True})
    assert took.status_code == 201, took.text
    now = ok(fam(b, "index"))
    assert now["full"] is True and now["epoch"] not in (first["epoch"], "left") and "left" not in now
    assert {i["id"] for i in now["items"]} == {i["id"] for i in first["items"]}
    assert next(i for i in now["items"] if i["id"] == ids["Menus de la semaine"])["categories"] == ["home"]
    assert [k["q"] for k in now["kept"]] == ["courses"]
    s = ok(fam(b, "summary"))
    # (`with`: the households her family agent is for; Alex stays linked on her side until she unlinks him too)
    assert [(k["q"], k["count"]) for k in s["kept"]] == [("courses", 2)] and s["with"] == "Avec Alex"
    assert [x["q"] for x in ok(fam(b, "category/school"))["searched"]] == ["karaté"]
    stored = json.loads((b.state_dir / "family-notebook.json").read_text())
    assert stored["category_rules"] == handed["category_rules"]


def test_s6_a_member_who_leaves_drops_the_copy_and_her_searches(bridges, fakes):
    a, b, ids = two(bridges, fakes)
    assert fam(b, "searched", method="post", json={"q": "karaté", "category": "school"}).status_code == 204
    ok(fam(b, "kept", method="post", json={"q": "pâtes"}))
    ok(fam(a, "kept", method="post", json={"q": "lait"}))
    first = ok(fam(b, "index"))
    assert first["items"]
    # taken out of the family's agent by its creator: out of its Carnet
    assert a.put("/v1/family-agent/famille/members", headers=AUTH, json={"members": []}).status_code == 200
    assert wait(lambda: fam(b, "index").json().get("left") is True)
    left = ok(fam(b, "index"))
    assert (left["full"], left["items"], left["kept"], left["version"]) == (True, [], [], 0)
    assert left["epoch"] == "left" and fam(b, "index").headers["etag"] == '"left"'
    assert not (b.state_dir / "family-searched.json").exists() and files_holding(b, "karat") == []
    for route in ("summary", "category/school", "kept"):
        got = fam(b, route)
        assert got.status_code == 404 and got.json()["detail"]["code"] == "no_family", (route, got.text)
    # (review 1 R6a) out of the family, a search made is not kept, even with its category
    got = fam(b, "searched", method="post", json={"q": "karaté", "category": "school"})
    assert got.status_code == 404 and got.json()["detail"]["code"] == "no_family"
    assert not (b.state_dir / "family-searched.json").exists()
    # (review 1 R3) her kept searches left with her; Alex's stay
    assert [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait"]
    # back in: a whole new copy
    assert a.put("/v1/family-agent/famille/members", headers=AUTH, json={"members": None}).status_code == 200
    b.get("/v1/family-agent", headers=AUTH)
    assert wait(lambda: fam(b, "index").json().get("left") is None)
    assert fam(b, "searched", method="post", json={"q": "karaté", "category": "school"}).status_code == 204
    ok(fam(b, "kept", method="post", json={"q": "citrons"}))
    assert [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait", "citrons"]
    # she unlinks Alex herself: the same; her bridge takes her kept searches off his place as she goes
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 204
    assert ok(fam(b, "index"))["left"] is True
    assert not (b.state_dir / "family-searched.json").exists()
    assert wait(lambda: [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait"])
    # nothing of hers was ever on Alex's place
    assert files_holding(a, "karat") == []


# -- « Oublie ça » ----------------------------------------------------------------------------------------------------

def test_s6_oublie_ca_takes_the_persons_kept_and_made_searches_of_the_family_carnet(bridges, fakes):
    a, b, ids = two(bridges, fakes)
    courses = ids["Lait"].split(":")[2].split("/")[0]
    change(a, op="item_add", list=courses, text="Cadeau pour Marc")
    change(b, op="item_add", list=courses, text="Raquette pour Marc")
    ok(fam(a, "kept", method="post", json={"q": "cadeau marc"}))
    ok(fam(b, "kept", method="post", json={"q": "marc tennis"}))
    ok(fam(a, "kept", method="post", json={"q": "lait"}))
    for x, q, cat in ((a, "marc", "people"), (a, "chaudière", "home"), (b, "marc", "school")):
        assert fam(x, "searched", method="post", json={"q": q, "category": cat}).status_code == 204
    before = ok(fam(a, "index"))
    preview = a.post("/v1/notebook/forget/preview", params={"lang": "fr"}, json={"texts": ["Marc"]}, headers=AUTH)
    assert preview.status_code == 200, preview.text
    assert "Carnet de la famille (visible par ta famille)" in [p["text"] for p in preview.json()["places"]]
    assert a.post("/v1/notebook/forget/confirm", json={"token": preview.json()["token"]}, headers=AUTH).status_code == 200
    # his line, his kept search, his search made: gone; Marie's are hers
    lines = [i["text"] for x in a.get("/v1/family-agent/notebook", headers=AUTH).json()["lists"] for i in x["items"]]
    assert "Cadeau pour Marc" not in lines and "Raquette pour Marc" in lines
    assert [k["q"] for k in ok(fam(b, "kept"))["kept"]] == ["marc tennis", "lait"]
    assert ok(fam(a, "category/people"))["searched"] == []
    assert [x["q"] for x in ok(fam(a, "category/home"))["searched"]] == ["chaudière"]
    assert [x["q"] for x in ok(fam(b, "category/school"))["searched"]] == ["marc"]
    assert b"marc" not in (a.state_dir / "family-searched.json").read_bytes().lower()
    # the phones drop them at their next read: a new version without them
    after = ok(fam(b, "index", params={"since": str(before["version"]), "epoch": before["epoch"]}))
    assert after["version"] > before["version"] and [k["q"] for k in after["kept"]] == ["marc tennis", "lait"]


def test_s6_review_1_b1_a_members_oublie_ca_takes_her_kept_searches_off_the_holder(bridges, fakes):
    """Review 1 of S6, B1: Marie forgets « Marc » on her place: her kept search that says it leaves Alex's place and the
    phones' copy; Alex's stays. Her bridge asks the holder for her own kept searches and matches them itself: the
    forgotten words never leave her place."""
    a, b, ids = two(bridges, fakes)
    ok(fam(b, "kept", method="post", json={"q": "marc tennis"}))
    ok(fam(b, "kept", method="post", json={"q": "lait"}))
    ok(fam(a, "kept", method="post", json={"q": "cadeau marc"}))
    before = ok(fam(b, "index"))
    page = b.state_dir / "notebook" / "carnet" / "Sujets" / "Tennis.md"
    page.parent.mkdir(parents=True, exist_ok=True)
    page.write_text("# Tennis\n\n- Tennis avec Marc le samedi\n")
    preview = b.post("/v1/notebook/forget/preview", params={"lang": "fr"}, json={"texts": ["Marc"]}, headers=AUTH)
    assert preview.status_code == 200, preview.text
    done = b.post("/v1/notebook/forget/confirm", json={"token": preview.json()["token"]}, headers=AUTH)
    assert done.status_code == 200 and done.json()["done"] is True and "family" not in done.json(), done.text
    assert [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait", "cadeau marc"]
    after = ok(fam(b, "index", params={"since": str(before["version"]), "epoch": before["epoch"]}))
    assert [k["q"] for k in after["kept"]] == ["lait", "cadeau marc"]
    # the holder keeps who kept each one by household (never a name sent by a bridge), told to the asking bridge only
    stored = json.loads((a.state_dir / "family-notebook.json").read_text())["kept"]
    assert [(k["q"], k["by_household"]) for k in stored] == [("lait", "marie"), ("cadeau marc", "self")]
    assert all("by" not in k for k in ok(fam(b, "kept"))["kept"])


def test_s6_review_1_r1_a_holder_that_removed_the_link_leaves_the_member_left(bridges, fakes):
    """Review 1 of S6, R1: Alex removes the link while Marie's bridge is off (the handover never arrives): her routes
    get `left` / `no_family`, never the holder's 401; her copy and her searches made go."""
    a, b, ids = two(bridges, fakes)
    assert fam(b, "searched", method="post", json={"q": "karaté", "category": "school"}).status_code == 204
    assert ok(fam(b, "index"))["items"]
    b.stop()
    assert a.delete("/v1/family/marie", headers=AUTH).status_code == 204
    b.start()
    got = fam(b, "index")
    assert got.status_code == 200 and got.json()["left"] is True and got.json()["items"] == [], got.text
    assert not (b.state_dir / "family-searched.json").exists()
    for route, params in (("summary", {}), ("category/school", {}), ("kept", {}), ("search", {"q": "lait"})):
        got = fam(b, route, params=params)
        assert got.status_code == 404 and got.json()["detail"]["code"] == "no_family", (route, got.status_code, got.text)
    got = fam(b, "categorize", method="post", json={"page": ids["page Menus de la semaine"], "categories": ["home"]})
    assert got.status_code == 404 and got.json()["detail"]["code"] == "no_family", got.text


# -- review 2 (23/11 10:31), done at int-1.1d ------------------------------------------------------------------------

@pytest.fixture
def relay(tmp_path):
    started = KerfyRelay(tmp_path).start()
    yield started
    started.stop()


def test_s6_review_2_r1_a_household_named_self_never_passes_for_the_holders_person(bridges, fakes):
    """Review 2 of S6, R1: a household's id is its name's slug, so a household named « Self » is `self` on the holder's
    place. Its kept searches are still its own, never the holder's person's: taken out of the family agent, they go
    with it; the holder's stay."""
    a = household(bridges, fakes, "alex", ["famille"])
    b = household(bridges, fakes, "self", ["vie"])
    named(a, "Alex")
    named(b, "Self")
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200, joined.text
    assert [h["id"] for h in a.get("/v1/family", headers=AUTH).json()["households"]] == ["self"]
    make_family_agent(a)
    assert b.get("/v1/family-agent", headers=AUTH).json()["agent"]["id"] == "famille__alex"
    seed(b)
    ok(fam(a, "kept", method="post", json={"q": "lait"}))
    ok(fam(b, "kept", method="post", json={"q": "pâtes"}))
    stored = json.loads((a.state_dir / "family-notebook.json").read_text())["kept"]
    assert [k["q"] for k in stored] == ["lait", "pâtes"]
    assert stored[0]["by_household"] != stored[1]["by_household"], stored
    # taken out of the family agent by its creator: the household « Self »'s kept searches go, Alex's stay
    assert a.put("/v1/family-agent/famille/members", headers=AUTH, json={"members": []}).status_code == 200
    assert wait(lambda: fam(b, "index").json().get("left") is True)
    assert [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait"]


def test_s6_review_2_r2_a_member_who_unlinks_through_the_relay_takes_her_kept_searches(bridges, fakes, relay):
    """Review 2 of S6, R2: Marie and Alex linked through Kerfy Studio's relay only (no address of one the other
    reaches). She removes the link: her kept searches leave Alex's family Carnet (her bridge takes them off while the
    link still carries the calls), his stay."""
    a = relay_install(bridges, fakes, "alex", ["famille"], relay.url)
    b = relay_install(bridges, fakes, "marie", ["vie"], relay.url)
    named(a, "Alex")
    named(b, "Marie")
    assert relay_link(b, a) == {"id": "alex", "name": "Alex"}
    make_family_agent(a)
    assert wait(lambda: (b.get("/v1/family-agent", headers=AUTH).json().get("agent") or {}).get("id") == "famille__alex",
                timeout=40), b.log_tail()
    ok(fam(b, "kept", method="post", json={"q": "pâtes"}))
    ok(fam(a, "kept", method="post", json={"q": "lait"}))
    assert [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["pâtes", "lait"]
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 204
    assert ok(fam(b, "index"))["left"] is True
    assert wait(lambda: [k["q"] for k in ok(fam(a, "kept"))["kept"]] == ["lait"], timeout=20), ok(fam(a, "kept"))


def test_s6_review_2_r3_the_temporary_files_a_hard_stop_left_go_at_start(bridges, fakes):
    """Review 2 of S6, R3: a hard stop between the write of `family-notebook.json` (or `family-agent.json`) and its
    rename leaves `family-notebook.<hex>.tmp`, the family Carnet of that moment (a line forgotten since included). The
    bridge takes those older than a minute out as it starts; a fresh one (a write going on) and other files stay."""
    a, b, _ = two(bridges, fakes)
    a.stop()
    stale = [a.state_dir / "family-notebook.0a1b2c3d.tmp", a.state_dir / "family-agent.9f8e7d6c.tmp"]
    fresh = a.state_dir / "family-notebook.11223344.tmp"
    other = a.state_dir / "notes.0a1b2c3d.tmp"
    for p in stale + [fresh, other]:
        p.write_text(json.dumps({"lists": [{"title": "Courses", "items": [{"text": "Ligne oubliée depuis"}]}]}))
    hour_ago = time.time() - 3600
    for p in stale + [other]:
        os.utime(p, (hour_ago, hour_ago))
    a.start()
    assert [p.exists() for p in stale] == [False, False]
    assert fresh.exists() and other.exists()
    # the family Carnet itself as it was
    assert [x["title"] for x in a.get("/v1/family-agent/notebook", headers=AUTH).json()["lists"]] == ["Courses", "Tâches", "Dates"]


# -- the 1.0 apps ------------------------------------------------------------------------------------------------------

def normalised(view):
    """A family Carnet view with its random ids and times made stable (in order of appearance)."""
    seen = {}

    def walk(v):
        if isinstance(v, dict):
            out = {}
            for k, x in v.items():
                if k in ("id", "list") and isinstance(x, str):
                    out[k] = seen.setdefault(x, f"<id{len(seen) + 1}>")
                elif k in ("updated", "at") and isinstance(x, (int, float)):
                    out[k] = 0
                elif k == "due" and isinstance(x, str):
                    out[k] = "<due>"
                else:
                    out[k] = walk(x)
            return out
        if isinstance(v, list):
            return [walk(x) for x in v]
        return v
    return json.dumps(walk(view), ensure_ascii=False, indent=1, sort_keys=True)


def test_s6_the_1_0_apps_see_todays_family_carnet(bridges, fakes):
    """Today's family routes, filed and corrected meanwhile: what a 1.0 app reads is what the 1.0 bridge answered
    (recorded on the bridge before S6: `BB_RECORD_FAMILY_1_0=<folder>`), the agent's tools as before."""
    a, b, ids = two(bridges, fakes)
    record = os.environ.get("BB_RECORD_FAMILY_1_0")
    if not record:
        # S6's own gestures: they must change nothing a 1.0 app reads
        ok(fam(b, "categorize", method="post", json={"page": ids["page Menus de la semaine"], "categories": ["home"]}))
        ok(fam(b, "kept", method="post", json={"q": "courses"}))
        assert fam(b, "searched", method="post", json={"q": "judo"}).status_code == 204
    give_token(a, "famille", TOKEN)
    answers = {"family-notebook-holder": a.get("/v1/family-agent/notebook", headers=AUTH),
               "family-notebook-member": b.get("/v1/family-agent/notebook", headers=AUTH)}
    for name, got in answers.items():
        assert got.status_code == 200 and got.headers.get("cache-control") == "no-store", got.text
        view = got.json()
        assert set(view) == {"pages", "lists", "updated"}
        assert {k for p in view["pages"] for k in p} == {"id", "title", "text", "updated", "by"}
        assert {k for x in view["lists"] for k in x} == {"id", "title", "kind", "items", "updated", "by"}
        assert {k for x in view["lists"] for i in x["items"] for k in i} == {"id", "text", "done", "due", "by", "at"}
        text = normalised(view)
        if record:
            Path(record).mkdir(parents=True, exist_ok=True)
            (Path(record) / f"{name}.json").write_text(text + "\n")
            continue
        assert text == (FIXTURES / f"{name}.json").read_text().rstrip("\n"), name
    read = call(a, "family_notebook_read", {}, token=TOKEN)["content"][0]["text"]
    assert read.splitlines()[0].startswith("## Courses (list, list id ") and "categor" not in read
    assert "- [ ] Lait (id " in read and "## Page « Menus de la semaine » (page id " in read
    assert set(b.get("/v1/family-agent", headers=AUTH).json()) == {"agent", "agents", "can_create", "handover", "members"}
