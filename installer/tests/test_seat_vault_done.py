"""The seat agent and a vault_keys order's outcome (review of the merged vault-keys, pass 3, C1 ter): what the order
did is noted once it ended (sealed and the hash of the token the place holds, or failed and why), told with its done
(the guichet marks the grant sealed from it, whatever a beat says later), and each beat cites the last such order done
when it read the place (the guichet leaves out the vault fields of a beat read before a done it knows)."""
import hashlib
import json
import os
from pathlib import Path

import pytest

from test_seat_vault_keys import SETTINGS, _seat_agent


@pytest.fixture
def seat(tmp_path, monkeypatch):
    agent = _seat_agent()
    home = tmp_path / "home" / "bip2"
    (home / ".local" / "state" / "bipagents").mkdir(parents=True)
    for name, file in (("VAULT_TRIED", "tried"), ("HARDENED", "hardened"), ("MOVES", "moves")):
        monkeypatch.setattr(agent, name, str(tmp_path / file))
    Path(agent.HARDENED).write_text("done")
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": "bip2"})
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: entry)
    return agent, home


def _holds(home, token):
    space = home / ".local" / "share" / "bipagents" / "space"
    space.mkdir(parents=True, exist_ok=True)
    (space / "keyservice.json").write_text(json.dumps({**SETTINGS, "token": token}))


def test_a_sealed_order_is_noted_with_the_token_the_place_holds_then(seat):
    agent, home = seat
    _holds(home, "the-new-token-0123456789abc")
    assert agent.vault_keys_order(2, SETTINGS, 41, run_as=lambda e, a, timeout=900: {"state": "sealed"}) is True
    said = agent.vault_outcome(41)
    assert said == {"sealed": True, "key_service": hashlib.sha256(b"the-new-token-0123456789abc").hexdigest()}
    assert SETTINGS["token"] not in Path(agent.VAULT_TRIED + "-done").read_text()


def test_a_failed_order_is_noted_with_why(seat):
    agent, home = seat

    def run_as(entry, args, timeout=900):
        raise agent.SeatError("vault-keys enable: the key service can't be reached")

    assert agent.vault_keys_order(2, SETTINGS, 42, run_as=run_as) is True
    said = agent.vault_outcome(42)
    assert said["sealed"] is False and "can't be reached" in said["error"]
    # a hardening that failed, a copy a move left: failed too, never « sealed »
    Path(agent.HARDENED).unlink()
    assert agent.vault_keys_order(2, SETTINGS, 43, harden=lambda: False, run_as=run_as) is True
    assert agent.vault_outcome(43)["sealed"] is False
    assert agent.vault_outcome(44) is None, "an order not ended says nothing"


def test_each_beat_cites_the_last_vault_keys_order_done_for_its_place(seat, monkeypatch):
    agent, home = seat
    monkeypatch.setattr(agent, "door_open", lambda port: True)
    monkeypatch.setattr(agent, "running", lambda uid, words: True)
    assert "vault_done" not in agent.health()["2"], "none done: nothing cited"
    agent.vault_keys_order(2, SETTINGS, 41, run_as=lambda e, a, timeout=900: {"state": "sealed"})
    agent.vault_keys_order(2, SETTINGS, 45, run_as=lambda e, a, timeout=900: {"state": "sealed"})
    assert agent.health()["2"]["vault_done"] == 45
