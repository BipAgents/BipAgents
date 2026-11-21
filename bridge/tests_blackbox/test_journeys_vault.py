"""The release journeys of « Le coffre rassure » (passation-pro briefs/design-coffre-rassurer.md; docs/release-journeys.md
J64–J65, column « Pont »), as the apps do them over HTTP, each page checked as the person SEES it
(bbkit/displayable.py):

- J64: the agent logs in with the vault (`#coffre`): under its answer, the vault's line « Rempli par le coffre sur
  fournisseur.example, sans le donner à l'agent », with the vault item a tap opens; the hidden mark never shown;
- J65: « Je le taperai moi-même » (`#connexion`): the card says the person may type it themselves; they answer `self`,
  take the agent's browser and hand it back; the agent goes on in the same turn; nothing is kept in the vault; the
  answered card reads `self` in the conversation.

The journeys' bridge has no Chrome: `vault_login`'s answer is the one the bridge gives on a page with a login form
(bbkit/journeys.py `vault_login`); its making is test_vault_login.py's. Each test is one journey `Jnn`."""
import json

import pytest

from bbkit import displayable
from bbkit.common import AUTH
from bbkit.journeys import AGENT, journey_bridge
from bbkit.servers import wait_until


@pytest.fixture
def journey(bridges, fakes):
    made = journey_bridge(bridges, fakes, browser=True)
    # the agent speaks French (its lines' words: handback::language_of)
    assert made.bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    return made


def cid(n: int) -> str:
    return f"J0URNEY0-0000-4000-8064-{n:012d}"


def started(resp):
    assert resp.status_code == 200, resp.text
    assert resp.json()["state"] in ("started", "steered", "queued"), resp.json()
    return resp.json()


def reply_done(journey, words, timeout=60):
    def found():
        page = journey.page()
        agents = [i for i in page["items"] if i["kind"] == "message" and i.get("role") == "agent"]
        return page if agents and words in agents[-1].get("text", "") and not page.get("run") else None
    assert wait_until(found, timeout=timeout), [(i["kind"], i.get("text", "")[:60]) for i in journey.page()["items"]]
    return found()


def secret_cards(journey):
    return [c for c in journey.page()["pending"].get("approvals") or [] if c.get("run_id", "").startswith("bip-secret-")]


def test_j64_the_vaults_line_under_the_answer_opens_the_login(journey):
    started(journey.send(cid(64), "Connecte-toi au fournisseur #coffre"))
    page = reply_done(journey, "Je suis connecté au fournisseur.")
    lines = [i for i in page["items"] if i["kind"] == "vault_line"]
    assert len(lines) == 1, [(i["kind"], i.get("text", "")[:40]) for i in page["items"]]
    line = lines[0]
    assert line["text"] == "Rempli par le coffre sur fournisseur.example, sans le donner à l'agent"
    assert line["sites"] == ["fournisseur.example"]
    item = line["item"]
    vault = {i["id"]: i for i in journey.bridge.get("/v1/vault", headers=AUTH).json()["items"]}
    assert vault[item]["label"] == "Fournisseur" and vault[item]["domain"] == "fournisseur.example"
    # under the answer, the hidden mark nowhere
    items = page["items"]
    assert items[items.index(line) - 1]["text"] == "Je suis connecté au fournisseur."
    assert "bip:filled" not in json.dumps(page)
    displayable.check(journey.bridge, page, AGENT)


def test_j65_i_ll_type_it_myself_the_agent_goes_on_once_handed_back(journey):
    started(journey.send(cid(65), "Connecte-toi au fournisseur #connexion"))
    assert wait_until(lambda: secret_cards(journey), timeout=30), journey.page()["pending"]
    [card] = secret_cards(journey)
    assert (card["bip_secret_use"], card["bip_secret_self"]) == ("browser", True)
    displayable.check(journey.bridge, journey.page(), AGENT)
    chosen = journey.bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                                 json={"allow": False, "way": "self"})
    assert chosen.status_code == 200 and chosen.json() == {"ok": True, "choice": "self"}, chosen.text
    assert wait_until(lambda: not secret_cards(journey), timeout=15)
    # the app opens the desktop with the hand taken; leaving it hands the browser back
    taken = journey.bridge.post(f"/v1/agents/{AGENT}/desktop/control", headers=AUTH, json={"take": True})
    assert taken.status_code == 200 and taken.json()["you_hold"] is True, taken.text
    back = journey.bridge.post(f"/v1/agents/{AGENT}/desktop/control", headers=AUTH, json={"take": False})
    assert back.status_code == 200, back.text
    page = reply_done(journey, "tu es connecté")
    answered = [i for i in page["items"] if i["kind"] == "approval"
                and (i.get("approval") or {}).get("run_id") == card["run_id"]]
    assert [a["approval"]["choice"] for a in answered] == ["self"], answered
    vault = journey.bridge.get("/v1/vault", headers=AUTH).json()["items"]
    assert not [i for i in vault if i["kind"] == "login"], "nothing kept in the vault"
    displayable.check(journey.bridge, page, AGENT)


def test_j65_the_agents_screen_is_live_in_the_bureau_and_the_hand_is_taken_there(journey):
    """What the apps' Bureau opens for J65 (served to them by serve_journeys.py): the agent's screen through the bridge
    (bbkit/fake_screen.py, the bridge logging in to it as to an agent's own VNC), a frame drawn; the hand taken there
    and given back."""
    import struct
    from bbkit.ws import open_ws
    # what the apps' Bureau reads first: it goes live, then takes the hand, only on a browser with a page open
    # (DesktopView / DesktopScreen); an idle one keeps its empty state, « Rendre la main » never shown (iPhone J65, 21/11)
    now = journey.bridge.get(f"/v1/agents/{AGENT}/desktop/state", headers=AUTH)
    assert now.status_code == 200, now.text
    assert {k: now.json()[k] for k in ("state", "browser", "page", "site", "update_browser")} == {
        "state": "active", "browser": True, "page": True, "site": "fournisseur.example", "update_browser": False}, now.json()
    desktop = journey.bridge.post(f"/v1/agents/{AGENT}/desktop", headers=AUTH)
    assert desktop.status_code == 200 and desktop.json()["kind"] == "novnc", desktop.text
    assert desktop.json()["page_path"], desktop.text     # a phone shows the page alone
    try:
        with open_ws(journey.bridge.ws_url("/" + desktop.json()["path"] + "ws")) as ws:
            held = bytearray()

            def recv_bytes(_ws, n, timeout=10):
                """Exactly `n` bytes of the stream, whatever messages they came in (the rest kept for the next read)."""
                while len(held) < n:
                    message = ws.recv(timeout=timeout)
                    held.extend(message if isinstance(message, bytes) else message.encode())
                out = bytes(held[:n])
                del held[:n]
                return out

            assert recv_bytes(ws, 12) == b"RFB 003.008\n"  # the bridge's own handshake to the app (Face ID passed)
            ws.send(b"RFB 003.008\n")
            assert recv_bytes(ws, 2) == b"\x01\x01"
            ws.send(b"\x01")
            assert recv_bytes(ws, 4) == b"\x00\x00\x00\x00"
            ws.send(b"\x00")                                     # ClientInit
            init = recv_bytes(ws, 24)
            width, height = struct.unpack(">HH", init[:4])
            assert (width, height) == (320, 200), init
            name = recv_bytes(ws, struct.unpack(">I", init[20:24])[0])
            assert name.startswith(b"Bureau de Vie"), name
            ws.send(b"\x03\x00" + struct.pack(">HHHH", 0, 0, width, height))   # FramebufferUpdateRequest
            update = recv_bytes(ws, 16 + width * height * 4, timeout=20)
            assert update[:4] == b"\x00\x00\x00\x01" and set(update[16:16 + 64]) == {0xD8}, update[:20]
            taken = journey.bridge.post(f"/v1/agents/{AGENT}/desktop/control", headers=AUTH, json={"take": True})
            assert taken.status_code == 200 and taken.json()["you_hold"] is True, taken.text
    finally:
        # given back even when a check above fails: the module's next journeys start with the hand free
        back = journey.bridge.post(f"/v1/agents/{AGENT}/desktop/control", headers=AUTH, json={"take": False})
    assert back.status_code == 200 and back.json()["you_hold"] is False, back.text
