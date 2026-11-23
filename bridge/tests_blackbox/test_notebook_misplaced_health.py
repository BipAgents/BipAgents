"""« N points semblent mal rangés » (misplaced.rs, on the Carnet's home of the 1.0 apps since 10/11) never shows nor
moves a point of the private health part (Santé), whatever its words (review 1 of carnet-s4, B1): « Prednisolone le
matin » has no word of the strict health filter, and the card showed it, with its page « Santé », outside the lock."""
import json

from bbkit.common import AUTH
from bbkit.mcp import give_token

SANTE = ("---\ntype: area\n---\n# Santé\n\n## Ce qui compte\n- Prednisolone le matin — *Wellness, 2026-11-01*\n"
         "- Enceinte Bluetooth : à acheter pour le salon — *Wellness, 2026-11-08*\n")


def home(bridge):
    got = bridge.get("/v1/notebook", params={"lang": "fr"}, headers=AUTH)
    assert got.status_code == 200, got.text
    return got.json()


def test_the_misplaced_card_never_shows_nor_moves_the_health_part(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb",
                                                             "model": "small-eu"}))
    root = bridge.state_dir / "notebook"
    sante = root / "private" / "Domaines" / "Santé.md"
    sante.parent.mkdir(parents=True, exist_ok=True)
    sante.write_text(SANTE)
    # a card an older bridge kept (state.json), with a point of Santé in it
    held = {"id": "mp0123456789ab", "part": "health", "path": "Domaines/Santé.md", "title": "Santé",
            "text": "Prednisolone le matin", "by": "Wellness", "reason": "health",
            "to": {"part": "main", "path": "", "title": "À ranger", "kind": "inbox", "section": ""}}
    # and one of Famille whose destination is a page of Santé (a correction towards Santé: review of this branch, B1)
    famille = root / "carnet" / "Domaines" / "Famille.md"
    famille.parent.mkdir(parents=True, exist_ok=True)
    famille.write_text("---\ntype: area\n---\n# Famille\n\n## Ce qui compte\n"
                       "- Psy de Zoé : jeudi 18 h — *Wellness, 2026-11-05*\n")
    towards = {"id": "mp0123456789ac", "part": "main", "path": "Domaines/Famille.md", "title": "Famille",
               "text": "Psy de Zoé : jeudi 18 h", "by": "Wellness", "reason": "moved",
               "to": {"part": "health", "path": "Domaines/Psy de Zoé.md", "title": "Psy de Zoé", "kind": "subject",
                      "section": ""}}
    (root / "state.json").write_text(json.dumps({"language": "fr", "outline": 1,
                                                 "misplaced": {"items": [held, towards]}}))
    bridge.start()
    give_token(bridge)
    shown = home(bridge)
    assert shown["misplaced"] is None and "Prednisolone" not in json.dumps(shown, ensure_ascii=False), shown["misplaced"]
    # answered by its id, or « Tout déplacer »: nothing of Santé moves (the answer keeps its shape)
    assert "Psy de Zoé" not in json.dumps(shown, ensure_ascii=False)
    for body in ({"action": "move", "ids": ["mp0123456789ab"]}, {"action": "move", "ids": ["mp0123456789ac"]},
                 {"action": "move"}):
        got = bridge.post("/v1/notebook/misplaced", params={"lang": "fr"}, headers=AUTH, json=body)
        assert got.status_code == 200 and got.json() == {"moved": 0, "toast": None, "card": None}, got.text
    assert "Psy de Zoé : jeudi 18 h" in famille.read_text()
    # the nightly look never proposes them
    run = bridge.post("/v1/notebook/tidy", headers=AUTH)
    assert run.status_code == 200 and "misplaced" not in run.json()["run"], run.text
    shown = home(bridge)
    assert shown["misplaced"] is None and "Prednisolone" not in json.dumps(shown, ensure_ascii=False), shown["misplaced"]
    assert "Prednisolone le matin" in sante.read_text() and "Enceinte Bluetooth" in sante.read_text()
    shared = root / "carnet"
    assert not any("Prednisolone" in p.read_text() for p in shared.rglob("*.md")) if shared.exists() else True
