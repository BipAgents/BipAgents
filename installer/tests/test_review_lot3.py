"""The review of lot 3 of the vault's sealed keys (passation-pro/briefs/review-vault-keys-lot3.md), its red tests
rewritten from its annex, and the non-blocking points.

- B1: one's own Hermes sent to a sealed place (`bipagents send`, to_place.bring_in) or an export imported over one
  (`bipagents import`, moving.unpack) never leaves it with its keys in clear on the disk: its key service kept, the
  stale half gone, the export's keys beside their stores, then sealed again once (as a restore does). An export of a
  sealed place sent to a place brings its keys.
- B2: the seat agent's `vault_keys` order waits while the place moves and gives up on the copy a completed move left;
  `vault_keys.enable` starts the bridge again only when it was running.
- R3 (the merge with lot 2): an export of a sealed place keeps every agent's Hermes copy, in memory or still on the
  disk, and never what a cut-off swap set aside (`.vault-link-*`, `.vault-disk-*`).
- R4: a restore that fails after its files came leaves no clear key in its fetch folder; R5: `enable` refuses an
  account that doesn't own the data; R6: the seat agent never makes the place's state folder as root."""
import json
import os
import stat
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import backup, hermes, moving, to_place, vault_keys
from test_seat_vault_keys import SETTINGS, _seat_agent
from test_to_place import CODE, computer, export, place  # noqa: F401 (fixtures)
from test_vault_keys_lot3 import (DATA_KEYS, _enrolling, _exported, _in_memory_copy, _members, offer_place,  # noqa: F401
                                  runtime, write)


def _sealed(home: Path, token: str):
    """The place's vault keys sealed (lot 1) as the installer lays it out on the offer."""
    data = home / ".local" / "share" / "bipagents"
    write(data / "keys.sealed.json", b'{"v": 1}')
    space = data / "space"
    write(space / "vault.share", b"half-of-" + token.encode())
    write(space / "keyservice.json", json.dumps({**SETTINGS, "token": token}).encode())
    os.chmod(space, 0o700)
    return data, space


def _counting_enable(monkeypatch, runner=None):
    sealed_with = []

    def enable(settings, **kw):
        sealed_with.append((settings, kw))
        if runner is not None:
            runner.log.append("vault-keys enable (sealed again)")
        return 0

    monkeypatch.setattr(vault_keys, "enable", enable)
    return sealed_with


# -- B1: `bipagents send` ------------------------------------------------------------------------------------------

def test_a_sealed_place_stays_sealed_after_one_s_own_hermes_is_sent_to_it(computer, place, tmp_path, monkeypatch):
    home, config = computer
    _, out = export(to_place.what_to_send(str(config), home), tmp_path)
    new, _ = place
    data, space = _sealed(new, "token-of-the-place")
    runner = hermes.Runner(dry=True)
    sealed_with = _counting_enable(monkeypatch, runner)
    to_place.bring_in(out, CODE, runner, base=new, repo=tmp_path / "repo")
    # the person's stores with their keys (the computer's, in clear) until the sealing again, once, with the place's
    # own key service; the place's old sealing (which opens none of them) gone
    assert (data / "vault" / "vault.key").read_text() == "fernet"
    assert json.loads((space / "keyservice.json").read_text())["token"] == "token-of-the-place"
    assert not (space / "vault.share").exists() and not (data / "keys.sealed.json").exists()
    assert len(sealed_with) == 1 and sealed_with[0][0] is None and sealed_with[0][1].get("base") == new
    log = "\n".join(runner.log)
    assert log.index("vault-keys enable") < log.index(" adopt "), "sealed before the agents are served again"


def test_a_failed_send_leaves_the_sealed_place_as_it_was(computer, place, tmp_path, monkeypatch):
    home, config = computer
    _, out = export(to_place.what_to_send(str(config), home), tmp_path)
    new, _ = place
    data, space = _sealed(new, "token-of-the-place")
    sealed_with = _counting_enable(monkeypatch)

    def broken(*args):
        raise OSError("disk full")

    monkeypatch.setattr(to_place, "place_profile", broken)
    with pytest.raises(OSError):
        to_place.bring_in(out, CODE, hermes.Runner(dry=True), base=new, repo=tmp_path / "repo")
    assert (data / "keys.sealed.json").read_bytes() == b'{"v": 1}'
    assert (space / "vault.share").read_bytes() == b"half-of-token-of-the-place"
    assert json.loads((space / "keyservice.json").read_text())["token"] == "token-of-the-place"
    assert not (data / "vault").exists() and sealed_with == []


def test_an_export_of_a_sealed_place_sent_to_a_place_brings_its_keys(computer, place, tmp_path, monkeypatch, runtime):
    home, config = computer
    data_from = home / ".local" / "share" / "bipagents"
    (data_from / "vault" / "vault.key").unlink()
    write(data_from / "keys.sealed.json", b'{"v": 1}')
    monkeypatch.setattr(vault_keys, "export_keys", lambda inv: dict(DATA_KEYS))
    _, out = export(to_place.what_to_send(str(config), home), tmp_path)
    new, _ = place
    sealed_with = _counting_enable(monkeypatch)
    to_place.bring_in(out, CODE, hermes.Runner(dry=True), base=new, repo=tmp_path / "repo")
    data = new / ".local" / "share" / "bipagents"
    for store, name in vault_keys.KEY_FILES.items():
        assert (data / store / name).read_text() == DATA_KEYS[store]
    assert not (data / "keys.sealed.json").exists()
    assert sealed_with == [], "the place wasn't sealed: today's layout, as the person's stores came"


# -- B1 bis: `bipagents import` ------------------------------------------------------------------------------------

@pytest.mark.parametrize("target_sealed", [True, False])
def test_an_import_over_a_sealed_place_seals_it_again(tmp_path, runtime, monkeypatch, target_sealed):
    source = offer_place(tmp_path / "old" / "bip3")
    out, phrase, _ = _exported(source, tmp_path, monkeypatch)
    target_home = tmp_path / "new" / "bip"
    target_place = offer_place(target_home, sealed_keys=target_sealed)
    sealed_with = _counting_enable(monkeypatch)
    tar, info, handle = moving.open_archive(out, phrase)
    target = moving.Target(home=target_home, repo=tmp_path / "checkout", tailnet_name=None, lan_address=None)
    moving.unpack(tar, info, target, target_place.config)
    handle.close()
    data, space = target_place.data, target_place.space
    assert (data / "vault" / "vault.key").read_text() == DATA_KEYS["vault"]
    assert not (data / "keys.sealed.json").exists()
    if target_sealed:
        assert not (space / "vault.share").exists(), "a half that opens none of the person's stores"
        assert json.loads((space / "keyservice.json").read_text())["token"] == "token-of-bip"
        assert len(sealed_with) == 1 and sealed_with[0][0] is None and sealed_with[0][1].get("base") == target_home
    else:
        assert sealed_with == []


# -- B2: the seat agent's order and a planned move -----------------------------------------------------------------

@pytest.fixture
def seat(tmp_path, monkeypatch):
    agent = _seat_agent()
    home = tmp_path / "home" / "bip2"
    (home / ".local" / "state" / "bipagents").mkdir(parents=True)
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "vault-keys-tried"))
    monkeypatch.setattr(agent, "HARDENED", str(tmp_path / "space-hardened"))
    monkeypatch.setattr(agent, "MOVES", str(tmp_path / "moves"))
    Path(agent.HARDENED).write_text("done")
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": "bip2"})
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: entry)
    return agent, home, tmp_path


def test_a_vault_keys_order_never_enrols_the_copy_a_move_left(seat):
    agent, home, tmp = seat
    write(home / agent.MOVED_AWAY, b'{"to": "vps-2"}')
    enrolled = []
    assert agent.vault_keys_order(2, SETTINGS, 31, run_as=lambda e, a, timeout=900: enrolled.append(a)) is True
    assert enrolled == [], "the stopped copy never runs again, sealed, beside the place that moved"


def test_a_vault_keys_order_waits_while_the_place_moves(seat):
    agent, home, tmp = seat
    write(Path(agent.MOVES) / "mv-1" / "state.json",
          json.dumps({"seat": 2, "role": "out", "phase": "copying", "updated": time.time()}).encode())
    enrolled, hardened = [], []
    Path(agent.HARDENED).unlink()
    assert agent.vault_keys_order(2, SETTINGS, 32, harden=lambda: hardened.append(1) or True,
                                  run_as=lambda e, a, timeout=900: enrolled.append(a)) is False
    assert enrolled == [] and hardened == [] and "32" not in agent.tried_orders(), "held: neither tried nor done"


def test_the_seat_agent_never_makes_the_place_s_state_folder_as_root(seat, monkeypatch):
    agent, home, tmp = seat
    import shutil
    shutil.rmtree(home / ".local")
    owned = []
    real_chown = os.chown
    monkeypatch.setattr(agent.os, "chown", lambda path, uid, gid: owned.append(str(path)) or real_chown(path, uid, gid))
    seen = []
    assert agent.vault_keys_order(2, SETTINGS, 33, run_as=lambda e, a, timeout=900: seen.append(a) or {}) is True
    assert seen, "enrolled"
    for folder in (home / ".local", home / ".local" / "state", home / ".local" / "state" / "bipagents"):
        assert str(folder) in owned, f"{folder} made for the place, not left root's"


def test_enable_starts_the_bridge_again_only_when_it_was_running(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(vault_keys, "bridge_command", lambda base=None: ["/bin/bipbridge"])
    (tmp_path / ".config" / "bipagents").mkdir(parents=True)
    (tmp_path / ".config" / "bipagents" / "bridge.toml").write_text("port = 8643\n")
    calls = []

    def runner(args, env=None):
        calls.append(list(args))
        stopped = args[:3] == ["systemctl", "--user", "is-active"]
        return SimpleNamespace(returncode=3 if stopped else 0, stdout="inactive\n" if stopped else "", stderr="")

    assert vault_keys.enable(SETTINGS, runner=runner, say=lambda _: None, base=tmp_path) == 0
    assert ["/bin/bipbridge", "vault-keys", "enable", "--config",
            str(tmp_path / ".config" / "bipagents" / "bridge.toml")] in calls
    assert not [c for c in calls if c[:3] == ["systemctl", "--user", "start"]], "a stopped bridge stays stopped"


# -- R3: the export of a sealed place and lot 2's set-aside copies --------------------------------------------------

def test_an_export_of_a_sealed_place_keeps_every_agent_s_hermes_copy_but_what_a_cut_set_aside(
        tmp_path, runtime, monkeypatch):
    p = offer_place(tmp_path / "home" / "bip")
    _in_memory_copy(p, runtime)
    # Coach's copy still on the disk (not rebuilt in memory yet), and what a cut-off swap left beside Vie's link
    coach = p.home / ".hermes" / "profiles" / "coach"
    write(coach / "SOUL.md", b"Tu es Coach.")
    write(coach / "vault" / "vault.json.enc", b"coach-hermes-copy")
    write(coach / "vault" / "vault.key", b"coach-hermes-key")
    fresh = runtime / "bipagents" / "hermes-vault-0123456789abcdef" / "vie.ffffffff"
    write(fresh / "vault.key", b"vie-fresh-key")
    os.symlink(fresh, p.home / ".hermes" / "profiles" / "vie" / ".vault-link-0badc0de")
    write(coach / ".vault-disk-0badc0de" / "vault.key", b"coach-set-aside-key")
    out, phrase, _ = _exported(p, tmp_path, monkeypatch)
    _, members = _members(out, phrase)
    datas = [data for _, data in members.values()]
    assert b"coach-hermes-copy" in datas and b"vie-hermes-copy-in-memory" in datas
    assert not [n for n in members if ".vault-link-" in n or ".vault-disk-" in n], sorted(members)
    assert b"vie-fresh-key" not in datas and b"coach-set-aside-key" not in datas


# -- R4, R5 -------------------------------------------------------------------------------------------------------

def test_a_restore_that_fails_after_its_files_came_leaves_no_clear_key_in_its_fetch_folder(tmp_path, monkeypatch):
    base = tmp_path / "bip"
    fetched = base / backup.RESTORE_TMP
    write(fetched / "home" / "bip3" / ".local" / "share" / "bipagents" / "vault" / "vault.key", b"clear-key")
    monkeypatch.setattr(backup, "fetch", lambda b, key: fetched)
    monkeypatch.setattr(backup, "put_back", lambda *a, **k: {"reseal": False})
    from bipinstall import migrations, update
    monkeypatch.setattr(update, "gateway_units", lambda: [])

    def broken(runner):
        raise OSError("disk full")

    monkeypatch.setattr(migrations, "run", broken)
    runner = SimpleNamespace(log=[], run=lambda args, **kw: None)
    with pytest.raises(OSError):
        backup.restore("key", runner, base=base, repo=tmp_path / "checkout")
    assert not fetched.exists(), "the backup's files (its clear keys) never stay once put back"


def test_enable_refuses_an_account_that_doesn_t_own_the_place_s_data(tmp_path, monkeypatch):
    data = tmp_path / ".local" / "share" / "hermes-bridge"
    data.mkdir(parents=True)
    monkeypatch.setattr(vault_keys.os, "getuid", lambda: os.stat(data).st_uid + 1)
    status, calls, said = _enrolling(tmp_path, monkeypatch, 0)
    assert status == 1 and calls == [], "root (or anyone else) by hand: nothing stopped, nothing asked"


# -- B3: a sealed place never goes to a BipAgents without the vault's sealed keys (lots 1 and 2) ---------------------

def test_a_sealed_place_never_goes_to_a_version_without_its_sealed_keys(tmp_path):
    from test_channels import OLD, World, commit, git
    world = World(tmp_path)
    world.installed_from(OLD)
    before = world.head()
    older = commit(world.old, "a version before the vault's sealed keys")
    data = tmp_path / "data"
    write(tmp_path / ".config" / "bipagents" / "bridge.toml", f'port = 8643\ndata_dir = "{data}"\n'.encode())
    write(data / "keys.sealed.json", b'{"v": 1}')
    done = world.install_sh("update", "--auto", BIPAGENTS_TO=older)
    assert world.head() == before, "its sealed stores would be read with keys made again in clear"
    assert "sealed" in done.stderr
    # a version that carries them (lots 1 and 2): it goes
    for name in ("sealed_keys.rs", "hermes_copies.rs"):
        write(world.old / "bridge-rs" / "src" / name, b"// the vault's sealed keys")
    ready = commit(world.old, "with the vault's sealed keys")
    done = world.install_sh("update", "--auto", BIPAGENTS_TO=ready)
    assert world.head() == ready, done.stderr
    # a place not sealed goes anywhere, as before
    (data / "keys.sealed.json").unlink()
    assert world.install_sh("update", "--auto", BIPAGENTS_TO=older).returncode == 0 and world.head() == older
    assert git(world.dir, "rev-parse", "HEAD") == older
