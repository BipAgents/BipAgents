"""Review of lot 3, pass 4 (18/11): the order's key service over a leftover keyservice.json (the reviewer's)."""
import json

from bipinstall import vault_keys

from test_vault_keys_lot3 import SETTINGS, _enrolling, write


def _space(tmp_path):
    return tmp_path / ".local" / "share" / "bipagents" / "space"


def test_an_encrypted_space_whose_settings_can_t_be_read_keeps_its_own_key_service(tmp_path, monkeypatch):
    """space.json there but unreadable (half written, a disk error): the space may be level 1, its keyservice.json
    opens it. Never replaced on a doubt."""
    space = _space(tmp_path)
    own = {**SETTINGS, "token": "the-space-s-own-token-0123456"}
    write(space / "keyservice.json", json.dumps(own).encode())
    write(space / "space.json", b'{"enabled": tru')          # cut off
    (space / "cipher").mkdir(parents=True)
    _enrolling(tmp_path, monkeypatch, 0)
    assert json.loads((space / "keyservice.json").read_text())["token"] == own["token"], "level 1's token replaced"


def test_an_order_on_a_sealed_place_never_touches_its_key_service(tmp_path, monkeypatch):
    space = _space(tmp_path)
    own = {**SETTINGS, "token": "the-place-s-live-token-012345"}
    write(space / "keyservice.json", json.dumps(own).encode())
    data = tmp_path / ".local" / "share" / "hermes-bridge"
    write(data / vault_keys.SEALED, b"{}")
    status, calls, said = _enrolling(tmp_path, monkeypatch, 0)
    assert status == 0 and "already sealed" in " ".join(said)
    assert json.loads((space / "keyservice.json").read_text())["token"] == own["token"]
    assert not [c for c in calls if c[0][0] == "/bin/bipbridge"]
    # a sealing under way (its journal) counts as sealed too
    (data / vault_keys.SEALED).unlink()
    write(data / vault_keys.JOURNAL, b"{}")
    _enrolling(tmp_path, monkeypatch, 0)
    assert json.loads((space / "keyservice.json").read_text())["token"] == own["token"]


def test_a_failed_enrolment_after_the_replacement_puts_the_old_key_service_back(tmp_path, monkeypatch):
    """The reviewer's, turned (fix C): the leftover file is replaced for the bridge's try, and put back when it fails
    (the old token may have been a live one: a transient failure on a place whose half moved in); kept on success."""
    space = _space(tmp_path)
    old = {**SETTINGS, "token": "a-live-token-of-the-place-012"}
    write(space / "keyservice.json", json.dumps(old).encode())
    status, _, _ = _enrolling(tmp_path, monkeypatch, 1)
    assert status == 1
    assert json.loads((space / "keyservice.json").read_text())["token"] == old["token"]
    assert sorted(p.name for p in space.iterdir()) == ["keyservice.json"], "nothing left beside it"
    (tmp_path / vault_keys.STATE).unlink()
    status, _, _ = _enrolling(tmp_path, monkeypatch, 0)
    assert status == 0 and json.loads((space / "keyservice.json").read_text())["token"] == SETTINGS["token"]
    assert sorted(p.name for p in space.iterdir()) == ["keyservice.json"]


def test_the_replacement_never_writes_through_a_link_left_at_its_temporary_name(tmp_path, monkeypatch):
    space = _space(tmp_path)
    write(space / "keyservice.json", json.dumps({**SETTINGS, "token": "a-forgotten-token-0123456789"}).encode())
    victim = tmp_path / "victim"
    victim.write_text("untouched")
    import os
    os.symlink(victim, space / "keyservice.json.tmp")
    _enrolling(tmp_path, monkeypatch, 0)
    assert victim.read_text() == "untouched"


def test_the_beat_says_which_key_service_token_the_place_holds(tmp_path):
    """Fix A: the SHA-256 of keyservice.json's token (what the guichet keeps of each grant), never the token."""
    import hashlib
    from test_seat_vault_keys import _seat_agent
    agent = _seat_agent()
    home = tmp_path / "bip"
    assert agent.vault_key_service(str(home)) is None
    space = home / ".local" / "share" / "bipagents" / "space"
    write(space / "keyservice.json", json.dumps({**SETTINGS, "token": "the-place-s-token-0123456789"}).encode())
    said = agent.vault_key_service(str(home))
    assert said == hashlib.sha256(b"the-place-s-token-0123456789").hexdigest()
    write(space / "keyservice.json", b"{not json")
    assert agent.vault_key_service(str(home)) is None
