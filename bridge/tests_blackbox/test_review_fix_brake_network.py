"""Independent review of fix-brake-network (27/11): red tests on 1d4df2027, kept as tests.

ask_secret on a name already in the vault now opens a card to type it anew (connectors.rs `ask_secret_tool`), the
kept item's identifier and label put back in the card."""
import json

from bbkit.mcp import not_done
from test_connectors import EMAIL, PASSWORD, ask, cards, setup, text, tool, vault_records

FIRST = {"name": "garmin", "site": "connect.garmin.example", "kind": "password",
         "identifier_label": "E-mail de ton compte Garmin", "reason": "Pour tes séances"}
NEW = "Nouveau-fake-Pa55/2026"


def test_typing_a_kept_secret_again_never_tells_the_agent_its_identifier(bridges, fakes):
    """E1. The identifier the person typed never reaches the agent (the first card's answer leaves it out). Taken back
    from the vault into the card, it counts as one « the agent gave » (`gave_identifier`), and the answer says it:
    « Saved as « garmin » (identifier alex@example.org) »."""
    bridge, _, _ = setup(bridges, fakes)
    first, _ = ask(bridge, FIRST, {"value": PASSWORD, "identifier": EMAIL})
    assert EMAIL not in json.dumps(first)
    again, card = ask(bridge, {"name": "garmin", "site": "connect.garmin.example", "kind": "password",
                               "reason": "Garmin a refusé le mot de passe"}, {"value": NEW})
    assert card["bip_secret_identifier"] == EMAIL
    assert "garmin" in text(again) and NEW not in json.dumps(again)
    assert EMAIL not in json.dumps(again), text(again)


def test_typing_a_kept_secret_again_keeps_its_site(bridges, fakes):
    """E2. The person's own « Modifier le mot de passe » keeps the item's site (connectors.rs, `edit: true`). The
    agent's ask on the same name took the site from its own arguments: the card showed another site beside the real
    account's identifier, and the kept item's origin was replaced (or erased when `site` is left out). Now (the
    coordinator, 27/11): another site is refused, no card; without `site` (or with the same), the kept site stays."""
    bridge, _, _ = setup(bridges, fakes)
    ask(bridge, FIRST, {"value": PASSWORD, "identifier": EMAIL})
    other = tool(bridge, "ask_secret", {"name": "garmin", "site": "autre.example", "kind": "password",
                                        "reason": "Garmin a refusé le mot de passe"})
    assert not_done(other) and "another site" in text(other) and EMAIL not in json.dumps(other), text(other)
    assert cards(bridge, "bip-secret-") == []
    # its kind kept too, whatever the agent says (review 2)
    _, card = ask(bridge, {"name": "garmin", "kind": "token", "reason": "Garmin a refusé le mot de passe"},
                  {"value": NEW, "identifier": EMAIL})
    assert "connect.garmin.example" in card["description"], card["description"]
    assert card["bip_secret_kind"] == "password", card
    [kept] = [r for r in vault_records(bridge) if r["label"] == "garmin"]
    assert kept["origin"] == "https://connect.garmin.example", (kept["origin"], card["description"])
    assert kept["secret_info"]["kind"] == "password", kept["secret_info"]
