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
    (root / "state.json").write_text(json.dumps({"language": "fr", "outline": 1, "misplaced": {"items": [held]}}))
    bridge.start()
    give_token(bridge)
    shown = home(bridge)
    assert shown["misplaced"] is None and "Prednisolone" not in json.dumps(shown, ensure_ascii=False), shown["misplaced"]
    # answered by its id, or « Tout déplacer »: nothing of Santé moves (the answer keeps its shape)
    for body in ({"action": "move", "ids": ["mp0123456789ab"]}, {"action": "move"}):
        got = bridge.post("/v1/notebook/misplaced", params={"lang": "fr"}, headers=AUTH, json=body)
        assert got.status_code == 200 and got.json() == {"moved": 0, "toast": None, "card": None}, got.text
    # the nightly look never proposes them
    run = bridge.post("/v1/notebook/tidy", headers=AUTH)
    assert run.status_code == 200 and "misplaced" not in run.json()["run"], run.text
    shown = home(bridge)
    assert shown["misplaced"] is None and "Prednisolone" not in json.dumps(shown, ensure_ascii=False), shown["misplaced"]
    assert "Prednisolone le matin" in sante.read_text() and "Enceinte Bluetooth" in sante.read_text()
    shared = root / "carnet"
    assert not any("Prednisolone" in p.read_text() for p in shared.rglob("*.md")) if shared.exists() else True
