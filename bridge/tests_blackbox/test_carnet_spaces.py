"""Carnet « Le sommaire », brick S5 « Perso / Pro » (plan piste 9 §2.3; the choices of 17/11: the setting « Séparer
Perso, Pro et Famille » in Réglages › Carnet, off at first; contracts/carnet/sommaire.md §3c, changes 41–):

- the setting (`separate_spaces`, `GET/PATCH /v1/notebook/settings`), the capability `spaces`;
- on, a page a Pro agent makes is Pro (`space: pro` in its header); off, nothing changes; a page that exists keeps its
  space; the agents of Pro read everything (7 B); switching off again loses nothing and shows everything together;
- « Ranger dans… Perso / Pro » (`POST /v1/notebook/categorize` with `space`, alone or with `categories`) and its
  undo (`/categorize/undo`), today's re-filings;
- each space answers for itself: the sommaire (`spaces`, the selector), a category's page, the search, the phone's
  copy (`?space=`), the kept searches (`all` shows every space's, `perso` also those kept before the setting) and the
  searches made (each space its own);
- Santé is in no space; the 1.0 apps see the same answers, the setting on and a Pro agent writing.
Every answer: `Cache-Control: no-store`."""
import json
from pathlib import Path

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_carnet_sommaire import (FIXTURES, READS, as_1_0, index_of, normalised, page_of, route_of, stable, undated,
                                  write_by_hand, write_carnet, wrote)
from test_carnet_summary import category, get, keep, line, searched, summary

STUDIO = "agent-token-for-studio-0123456789ab"


def with_studio(config):
    """A second agent, « Studio », that the person puts in their « Pro » group."""
    wellness = config["agents"]["Wellness"]
    config["agents"]["Studio"] = {**wellness, "display_name": "Studio",
                                  "upload_dir_host": str(Path(wellness["upload_dir_host"]).parent / "studio"),
                                  "upload_dir_container": "/home/hermes/.hermes/profiles/studio/uploads"}


def studio_bridge(bridges):
    bridge = bridges.new(change=with_studio).start()
    write_carnet(bridge)
    give_token(bridge, "studio", STUDIO)
    assert bridge.put("/v1/agents/studio/group", headers=AUTH, json={"group": "Pro"}).status_code == 200
    return bridge


def setting(bridge, on):
    got = bridge.patch("/v1/notebook/settings", json={"separate_spaces": on}, headers=AUTH)
    assert got.status_code == 200, got.text
    assert got.json()["separate_spaces"] is on
    return got.json()


def studio_writes(bridge, args):
    return wrote(call(bridge, "notebook_write", args, token=STUDIO))


def page_text(bridge, path):
    return (bridge.state_dir / "notebook" / "carnet" / path).read_text()


def categorize(bridge, status=200, **body):
    got = bridge.post("/v1/notebook/categorize", json=body, params={"lang": "fr"}, headers=AUTH)
    assert got.status_code == status, (body, got.status_code, got.text)
    assert got.headers.get("cache-control") == "no-store"
    return got.json()


def undo(bridge, token, status=200):
    got = bridge.post("/v1/notebook/categorize/undo", json={"undo": token}, params={"lang": "fr"}, headers=AUTH)
    assert got.status_code == status, got.text
    return got.json()


def item_of(bridge, path, kind="page", space=None):
    params = {"lang": "fr", **({"space": space} if space else {})}
    items = index_of(bridge, **params).json()["items"]
    return next((i for i in items if i["kind"] == kind and i.get("path") == path), None)


def test_s5_the_setting_off_at_first_then_a_pro_agent_writes_in_pro(bridges):
    bridge = studio_bridge(bridges)
    # the capability, the setting off at first (in the settings and in the home's)
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sommaire"][-1] == "spaces"
    assert get(bridge, "/v1/notebook/settings")["separate_spaces"] is False
    assert bridge.get("/v1/notebook", headers=AUTH).json()["settings"]["separate_spaces"] is False
    bad = bridge.patch("/v1/notebook/settings", json={"separate_spaces": "maybe"}, headers=AUTH)
    assert bad.status_code == 422, bad.text
    # off: a Pro agent's new page is like any other
    studio_writes(bridge, {"page": "Client Dupont", "kind": "subject", "section": "status", "facts": ["Devis envoyé"]})
    assert "space:" not in page_text(bridge, "Sujets/Client Dupont.md")
    assert page_of(bridge, "Sujets/Client Dupont.md")["space"] == "perso"
    # on: Pro, in its header (Obsidian), on its page and in the lists
    setting(bridge, True)
    assert bridge.get("/v1/notebook", headers=AUTH).json()["settings"]["separate_spaces"] is True
    studio_writes(bridge, {"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Contrat signé"]})
    text = page_text(bridge, "Sujets/Client Martin.md")
    assert "\nspace: pro\n" in text, text
    page = page_of(bridge, "Sujets/Client Martin.md")
    assert (page["space"], page["frontmatter"]["space"]) == ("pro", "pro")
    listed = {p["title"]: p for p in bridge.get("/v1/notebook/pages?kind=subject", headers=AUTH).json()["pages"]}
    assert (listed["Client Martin"]["space"], listed["Client Dupont"]["space"]) == ("pro", "perso")
    assert item_of(bridge, "Sujets/Client Martin.md")["space"] == "pro"
    # a page that exists keeps its space (nothing moves by itself); a Perso agent's new page is Perso
    studio_writes(bridge, {"page": "Client Dupont", "section": "status", "facts": ["Relance faite"]})
    assert "space:" not in page_text(bridge, "Sujets/Client Dupont.md")
    wrote(call(bridge, "notebook_write", {"page": "Potager", "kind": "subject", "section": "status", "facts": ["Tomates"]}))
    assert "space:" not in page_text(bridge, "Sujets/Potager.md")
    # the person's new page in the open tab
    made = bridge.post("/v1/notebook/page", json={"kind": "subject", "title": "Salon pro", "space": "pro"}, headers=AUTH)
    assert made.status_code == 201, made.text
    assert page_of(bridge, made.json()["path"])["space"] == "pro"
    wrong = bridge.post("/v1/notebook/page", json={"kind": "subject", "title": "Salon", "space": "family"}, headers=AUTH)
    assert wrong.status_code == 400 and wrong.json()["detail"]["code"] == "space_unknown", wrong.text


def test_s5_switching_off_loses_nothing_and_shows_everything_together(bridges):
    bridge = studio_bridge(bridges)
    everything = summary(bridge)["total"]["count"]
    setting(bridge, True)
    studio_writes(bridge, {"page": "Client Martin", "kind": "subject", "section": "status",
                           "facts": ["Contrat signé", "Facture envoyée"]})
    before = page_text(bridge, "Sujets/Client Martin.md")
    perso, pro, all_ = summary(bridge, space="perso"), summary(bridge, space="pro"), summary(bridge)
    assert all_["total"]["count"] == everything + 3
    assert (pro["total"]["count"], perso["total"]["count"] + pro["total"]["count"]) == (3, all_["total"]["count"])
    assert perso["spaces"] == [{"id": "perso", "title": "Perso", "selected": True},
                               {"id": "pro", "title": "Pro", "selected": False}], perso["spaces"]
    # kept searches: those of before the setting in Perso, Pro's apart
    keep(bridge, q="anniversaire")
    keep(bridge, q="facture", space="pro")
    keep(bridge, q="voyage", space="perso")
    assert [k["q"] for k in get(bridge, "/v1/notebook/kept", space="perso")["kept"]] == ["anniversaire", "voyage"]
    assert [k["q"] for k in get(bridge, "/v1/notebook/kept", space="pro")["kept"]] == ["facture"]
    # off again: nothing lost, everything together
    setting(bridge, False)
    assert page_text(bridge, "Sujets/Client Martin.md") == before
    again = summary(bridge)
    assert (again["total"]["count"], again["spaces"]) == (all_["total"]["count"], [])
    assert [k["q"] for k in again["kept"]] == ["anniversaire", "facture", "voyage"]
    # on again: Pro as it was
    setting(bridge, True)
    assert summary(bridge, space="pro")["total"]["count"] == 3


def test_s5_ranger_dans_perso_ou_pro_and_undo(bridges):
    bridge = studio_bridge(bridges)
    kitchen = "Sujets/Rénovation cuisine.md"
    before = page_text(bridge, kitchen)
    for body, status, code in [
        ({"path": kitchen, "space": "family"}, 400, "space_unknown"),
        ({"path": kitchen, "space": "all"}, 400, "space_unknown"),
        ({"path": kitchen}, 400, "category_count"),
        ({"path": kitchen, "private": True, "space": "pro"}, 400, "category_not_given"),
        ({"path": "Sujets/Rien.md", "space": "pro"}, 404, "not_found"),
        ({"space": "pro"}, 400, "target_missing"),
    ]:
        assert categorize(bridge, status, **body)["detail"]["code"] == code, body
    # from the page: only its `space` line changes
    done = categorize(bridge, path=kitchen, space="pro")
    assert (done["space"], done["before"]["space"], done["toast"], done["undo_title"]) == ("pro", "perso", "Rangé dans Pro.", "Annuler")
    assert done["categories"] == done["before"]["categories"]
    assert page_text(bridge, kitchen).replace("space: pro\n", "") == before
    assert page_of(bridge, kitchen)["space"] == "pro"
    assert item_of(bridge, kitchen)["space"] == "pro"
    today = get(bridge, "/v1/notebook/categorize/today", lang="fr")
    assert any(i["undo"] == done["undo"] and i["space"] == "pro" and i["text"] == "Rangé dans Pro." for i in today["items"]), today
    undone = undo(bridge, done["undo"])
    assert undone["space"] == "perso" and page_text(bridge, kitchen) == before
    assert undo(bridge, done["undo"], 404)["detail"]["code"] == "undo_gone"
    # « Ranger ailleurs » with a space: both, one undo
    both = categorize(bridge, path=kitchen, categories=["money"], space="pro")
    assert (both["categories"], both["space"], both["toast"]) == (["money"], "pro", "Rangé dans Argent et Pro."), both
    assert (page_of(bridge, kitchen)["categories"], page_of(bridge, kitchen)["space"]) == (["money"], "pro")
    undo(bridge, both["undo"])
    assert page_text(bridge, kitchen) == before
    # without a space: S1's answer as it was (no `space` key)
    only = categorize(bridge, path=kitchen, categories=["money"])
    assert "space" not in only and "space" not in only["before"], only
    undo(bridge, only["undo"])
    # a line alone, by its Item id: the line in Pro, the page in Perso, the page's text untouched
    items = index_of(bridge, lang="fr").json()["items"]
    vinyl = next(i for i in items if i["kind"] == "line" and i["line"] == "Un vinyle de Chet Baker")
    marked = categorize(bridge, item=vinyl["id"], space="pro")
    assert marked["target"]["item"] == vinyl["id"] and marked["space"] == "pro"
    after = index_of(bridge, lang="fr").json()["items"]
    assert next(i for i in after if i["id"] == vinyl["id"])["space"] == "pro"
    assert "space" not in next(i for i in after if i["kind"] == "page" and i["path"] == vinyl["path"])
    assert "space:" not in page_text(bridge, vinyl["path"])
    undo(bridge, marked["undo"])
    # a record
    record = next(i for i in items if i["kind"] == "record")
    assert categorize(bridge, item=record["id"], space="pro")["space"] == "pro"
    assert next(i for i in index_of(bridge, lang="fr").json()["items"] if i["id"] == record["id"])["space"] == "pro"


def test_s5_a_pro_agent_reads_everything(bridges):
    bridge = studio_bridge(bridges)
    setting(bridge, True)
    studio_writes(bridge, {"page": "Client Martin", "kind": "subject", "section": "status", "facts": ["Contrat signé"]})
    found = text_of(call(bridge, "notebook_search", {"words": "Marc"}, token=STUDIO))
    assert "Marc Durand" in found, found
    read = text_of(call(bridge, "notebook_read", {"page": "Marc Durand"}, token=STUDIO))
    assert "Chet Baker" in read, read
    assert "Client Martin" in text_of(call(bridge, "notebook_search", {"words": "Client Martin"}))


def test_s5_each_space_answers_for_itself(bridges):
    bridge = studio_bridge(bridges)
    setting(bridge, True)
    studio_writes(bridge, {"page": "Client Martin", "kind": "subject", "section": "status",
                           "facts": ["Facture envoyée le 3 novembre"]})
    studio_writes(bridge, {"page": "Sophie Leroy", "kind": "person", "section": "work", "facts": ["Acheteuse chez Martin"]})
    whole = index_of(bridge, lang="fr")
    pro_ids = {i["id"] for i in whole.json()["items"] if i.get("space") == "pro"}
    assert len(pro_ids) >= 4
    # the sommaire
    pro = summary(bridge, space="pro")
    assert pro["total"]["count"] == len(pro_ids) and pro["card"] is None and line(pro, "health") is None
    assert line(pro, "people")["first"]["space"] == "pro"
    assert summary(bridge, space="perso", lang="en")["spaces"][1]["title"] == "Work"
    # a category's page
    work = category(bridge, "work", space="pro")
    assert work["count"] >= 1 and all(i["space"] == "pro" for i in work["recent"])
    # the search (and a space that is not one: refused)
    found = get(bridge, "/v1/notebook/search", q="Martin", lang="fr", sommaire=1, space="pro")
    assert found["counts"]["all"] >= 1 and all(t["space"] == "pro" for g in found["groups"] for t in g["top"])
    refused = bridge.get("/v1/notebook/search", params={"q": "x", "sommaire": 1, "space": "family"}, headers=AUTH)
    assert refused.status_code == 400 and refused.json()["detail"]["code"] == "space_unknown", refused.text
    # the phone's copy of one space: its notes only, its ETag, 304
    got = index_of(bridge, lang="fr", space="pro")
    assert {i["id"] for i in got.json()["items"]} == pro_ids
    assert got.headers["etag"] != whole.headers["etag"]
    again = bridge.get("/v1/notebook/index", params={"lang": "fr", "space": "pro"}, headers={**AUTH, "If-None-Match": got.headers["etag"]})
    assert again.status_code == 304 and again.headers.get("cache-control") == "no-store"
    refused = bridge.get("/v1/notebook/index", params={"space": "family"}, headers=AUTH)
    assert refused.status_code == 400 and refused.json()["detail"]["code"] == "space_unknown", refused.text
    # kept searches count in their space; searches made stay in theirs
    keep(bridge, q="facture", space="pro")
    assert get(bridge, "/v1/notebook/kept", space="pro")["kept"][0]["count"] >= 1
    searched(bridge, q="facture envoyée", space="pro")
    assert [s["q"] for s in category(bridge, "work", space="pro")["searched"]] == ["facture envoyée"]
    assert category(bridge, "work", space="perso")["searched"] == []


def test_s5_sante_is_in_no_space(bridges):
    bridge = studio_bridge(bridges)
    write_by_hand(bridge)
    private = bridge.state_dir / "notebook" / "private" / "Domaines" / "Santé.md"
    private.write_text(private.read_text().replace("type: area\n", "type: area\nspace: pro\n"))
    setting(bridge, True)
    for space in ("perso", "pro"):
        assert "Prednisolone" not in json.dumps(summary(bridge, space=space), ensure_ascii=False)
        assert "Prednisolone" not in index_of(bridge, lang="fr", space=space).text
    assert line(summary(bridge, space="perso"), "health")["locked"] is True
    assert line(summary(bridge, space="pro"), "health") is None
    for space in ("all", "perso", "pro"):
        page = category(bridge, "health", private=1, space=space)
        assert any("Prednisolone" in i["line"] for i in page["recent"]), (space, page)
    assert categorize(bridge, 404, path="Domaines/Santé.md", space="pro")["detail"]["code"] == "not_found"


def test_s5_the_1_0_apps_see_the_same_with_the_setting_on_and_a_pro_agent(bridges):
    """The setting on and the agent that writes the Carnet in « Pro » (its pages Pro): a 1.0 app reads every route as
    before, the contract's added keys aside (a 1.0 app ignores them; nothing is hidden from it)."""
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.patch("/v1/notebook/settings", json={"separate_spaces": True}, headers=AUTH).status_code == 200
    assert bridge.put("/v1/agents/wellness/group", headers=AUTH, json={"group": "Pro"}).status_code == 200
    write_carnet(bridge)
    assert "\nspace: pro\n" in page_text(bridge, "Personnes/Marc Durand.md")
    differ = []
    for name, path in READS:
        got = bridge.get(path, headers=AUTH)
        assert got.status_code == 200, (path, got.text)
        want = undated((FIXTURES / f"{name}.json").read_text().rstrip("\n"))
        mine = normalised(as_1_0(route_of(path), stable(name, got.json())))
        if undated(mine) != want:
            differ.append(name)
            (bridge.root / f"{name}.got.json").write_text(mine + "\n")
    assert not differ, f"not as before: {differ} (in {bridge.root})"


def test_s5_renaming_or_deleting_a_kept_search_answers_the_view_on_screen(bridges):
    """Review 1, B1: the answer of a rename (`space` in the body) or a deletion (`?space=`) is the list of the view the
    app shows; without it, `all` (the view of the S4 apps)."""
    bridge = studio_bridge(bridges)
    keep(bridge, q="anniversaire")
    setting(bridge, True)
    keep(bridge, q="facture", space="pro")
    before = get(bridge, "/v1/notebook/kept", space="perso")["kept"][0]
    assert before["q"] == "anniversaire"
    # on, the Perso tab: one kept before the setting renamed → Perso's list (never Pro's), counted in Perso
    renamed = keep(bridge, id=before["id"], title="Les anniversaires", space="perso")["kept"]
    assert [(k["q"], k["title"]) for k in renamed] == [("anniversaire", "Les anniversaires")]
    assert renamed[0]["count"] == get(bridge, "/v1/notebook/kept", space="perso")["kept"][0]["count"]
    # off: the view `all` (no space sent): renaming or deleting Pro's answers everything
    setting(bridge, False)
    pro_id = get(bridge, "/v1/notebook/kept", space="pro")["kept"][0]["id"]
    assert [k["q"] for k in keep(bridge, id=pro_id, title="Factures")["kept"]] == ["anniversaire", "facture"]
    left = bridge.delete("/v1/notebook/kept", params={"id": pro_id}, headers=AUTH)
    assert left.status_code == 200 and [k["q"] for k in left.json()["kept"]] == ["anniversaire"], left.text
    # a deletion in the Pro tab: Pro's list
    keep(bridge, q="devis", space="pro")
    devis = get(bridge, "/v1/notebook/kept", space="pro")["kept"][0]["id"]
    gone = bridge.delete("/v1/notebook/kept", params={"id": devis, "space": "pro"}, headers=AUTH)
    assert gone.status_code == 200 and gone.json()["kept"] == [] and gone.headers.get("cache-control") == "no-store"
    bad = bridge.delete("/v1/notebook/kept", params={"id": devis, "space": "family"}, headers=AUTH)
    assert bad.status_code == 400 and bad.json()["detail"]["code"] == "space_unknown", bad.text
    # « Toi » stays in Perso (R4)
    assert categorize(bridge, 400, path="Toi.md", space="pro")["detail"]["code"] == "space_not_given"
