"""The vault's keys sealed on a place of the offer (design note « Clé du coffre », lot 3: the installer's side).

- A new place whose setup carries `vault_keys` is enrolled once, right after its install (the bridge stopped,
  `bipbridge vault-keys enable` with the setup's key service, the bridge started again); a failure leaves today's
  layout, is noted for the seat agent (`vault-keys.json`), and is never tried again by itself (each try costs one of
  the place's 24 daily releases).
- Backups and planned moves carry this server's half and the key service's settings (`<space>/vault.share`,
  `<space>/keyservice.json`) with the data (`keys.sealed.json` in it), never the place key kept in memory; a restore
  puts them back over a fresh place, whatever that place was (the backup's layout wins: a fresh place's own sealing
  never stays over stores it doesn't open; it seals the person's stores again with its own enrolment, once).
- `bipagents export` of an enrolled place carries the keys the place key opens (asked of the bridge, refused while the
  vault is closed) and the agents' Hermes copies kept in memory, never the half nor the key service's token; the import
  goes back to today's layout (the key files beside their stores, no keys.sealed.json)."""
import json
import os
import stat
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import backup, cloud, hermes, moving, placemove, sealed, vault_keys

ROOT = Path(__file__).resolve().parents[2]
SETTINGS = {"url": "https://keys.example", "grant": '{"v":2}', "signature": "c2ln", "token": "k" * 43}
DATA_KEYS = {"vault": "V" * 43 + "=", "mail": "M" * 43 + "=", "connections": "C" * 43 + "="}


def write(path: Path, data: bytes = b"x", mode: int = 0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    os.chmod(path, mode)


# -- the setup -------------------------------------------------------------------------------------------------------

def test_the_setup_carries_vault_keys_like_keys_and_never_the_backups_token(tmp_path):
    base = {"v": 1, "pair": "A" * 43, "backup": {"url": "https://g.example", "token": "t" * 32}}
    path = tmp_path / "cloud.json"
    path.write_text(json.dumps(base))
    assert cloud.read(path).vault_keys is None   # off unless the guichet asks
    path.write_text(json.dumps({**base, "vault_keys": SETTINGS}))
    found = cloud.read(path)
    assert found.vault_keys == SETTINGS and found.keys is None   # the vault only: the space is not encrypted
    path.write_text(json.dumps({**base, "keys": SETTINGS, "vault_keys": SETTINGS}))
    assert cloud.read(path).keys == cloud.read(path).vault_keys == SETTINGS   # both: one grant, one token
    for wrong in ({**SETTINGS, "url": "http://keys.example"}, {"url": "https://k"}, "x", True,
                  {k: v for k, v in SETTINGS.items() if k != "token"}, {**SETTINGS, "token": "short"},
                  {**SETTINGS, "token": "t" * 32}):   # the backups' token: never at the key service
        path.write_text(json.dumps({**base, "vault_keys": wrong}))
        with pytest.raises(cloud.CloudError):
            cloud.read(path)


def test_the_cloud_install_seals_the_vault_once_after_the_space_and_keeps_its_status():
    text = (ROOT / "installer" / "bipinstall" / "__main__.py").read_text(encoding="utf-8")
    assert "setup.vault_keys" in text and "vault_keys.enable(" in text
    assert text.index("space.encrypt(None") < text.index("vault_keys.enable(")
    line = next(line for line in text.splitlines() if "vault_keys.enable(" in line)
    assert "status =" not in line, "a failed sealing leaves today's layout: the install itself worked"


def test_the_first_boot_and_add_seat_harden_the_server_for_vault_keys_too():
    for path in (ROOT / "installer" / "cloud" / "user-data.yaml", ROOT / "installer" / "cloud" / "add-seat.sh"):
        text = path.read_text(encoding="utf-8")
        assert "grep -qsE '\"(vault_)?keys\"'" in text, path
        assert "grep -qs '\"keys\"'" not in text, path


# -- enrolling: once ---------------------------------------------------------------------------------------------------

def _enrolling(tmp_path, monkeypatch, bridge_code, settings=SETTINGS):
    monkeypatch.setenv("HOME", str(tmp_path))
    for name in ("BIPAGENTS_KEYS_URL", "BIPAGENTS_KEYS_TOKEN", "BIPAGENTS_KEYS_GRANT", "BIPAGENTS_KEYS_SIGNATURE"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(vault_keys, "bridge_command", lambda base=None: ["/bin/bipbridge"])
    (tmp_path / ".config" / "bipagents").mkdir(parents=True, exist_ok=True)
    (tmp_path / ".config" / "bipagents" / "bridge.toml").write_text('port = 8643\n')
    calls = []

    def runner(args, env=None):
        calls.append((list(args), env))
        code = bridge_code if args[0] == "/bin/bipbridge" else 0
        said = f"bipbridge vault-keys enable: the key service refused (token {SETTINGS['token']})" if code else ""
        return SimpleNamespace(returncode=code, stdout="" if code else '{"state": "open"}', stderr=said)

    said = []
    status = vault_keys.enable(settings, runner=runner, say=said.append, base=tmp_path)
    return status, calls, said


def test_enable_stops_the_bridge_seals_with_the_setup_s_key_service_then_starts_it(tmp_path, monkeypatch):
    status, calls, said = _enrolling(tmp_path, monkeypatch, 0)
    assert status == 0
    commands = [c[0] for c in calls]
    stop = commands.index(["systemctl", "--user", "stop", "bipagents-bridge.service"])
    enable = commands.index(["/bin/bipbridge", "vault-keys", "enable", "--config",
                             str(tmp_path / ".config" / "bipagents" / "bridge.toml")])
    start = commands.index(["systemctl", "--user", "start", "bipagents-bridge.service"])
    assert stop < enable < start and len([c for c in commands if c[0] == "/bin/bipbridge"]) == 1
    env = calls[enable][1]
    assert env["BIPAGENTS_KEYS_URL"] == SETTINGS["url"] and env["BIPAGENTS_KEYS_TOKEN"] == SETTINGS["token"]
    assert env["BIPAGENTS_KEYS_GRANT"] == SETTINGS["grant"] and env["BIPAGENTS_KEYS_SIGNATURE"] == SETTINGS["signature"]
    assert SETTINGS["token"] not in " ".join(" ".join(c) for c in commands), "never on a command line"
    state = vault_keys.read_state(tmp_path)
    assert state["state"] == "sealed" and isinstance(state["at"], float)
    path = tmp_path / vault_keys.STATE
    assert stat.S_IMODE(path.stat().st_mode) == 0o600 and SETTINGS["token"] not in path.read_text()


def test_a_failed_enable_starts_the_bridge_again_notes_why_and_is_never_retried_by_itself(tmp_path, monkeypatch):
    status, calls, said = _enrolling(tmp_path, monkeypatch, 1)
    assert status == 1
    assert calls[-1][0] == ["systemctl", "--user", "start", "bipagents-bridge.service"]
    state = vault_keys.read_state(tmp_path)
    assert state["state"] == "failed" and "refused" in state["error"]
    assert SETTINGS["token"] not in state["error"] and SETTINGS["token"] not in " ".join(said)
    assert len([c for c in calls if c[0][0] == "/bin/bipbridge"]) == 1
    # the same settings again (a second order, a rerun of the install): refused without asking the key service
    calls.clear()
    again = vault_keys.enable(SETTINGS, runner=lambda args, env=None: calls.append(args), say=said.append,
                              base=tmp_path)
    assert again == 1 and calls == []
    assert "already tried" in said[-1]
    # an operator, by hand, after looking at why: --again
    status, calls, said = _enrolling(tmp_path, monkeypatch, 0)
    assert status == 1 and not [c for c in calls if c[0][0] == "/bin/bipbridge"]
    ran = []
    assert vault_keys.enable(SETTINGS, runner=lambda args, env=None: ran.append(args) or SimpleNamespace(
        returncode=0, stdout="", stderr=""), say=said.append, base=tmp_path, again=True) == 0
    assert vault_keys.read_state(tmp_path)["state"] == "sealed"


def test_enable_refuses_settings_that_aren_t_a_key_service_of_its_own(tmp_path, monkeypatch):
    for wrong in ({**SETTINGS, "url": "http://x"}, {**SETTINGS, "token": "short"}, {"url": "https://k"}):
        status, calls, said = _enrolling(tmp_path, monkeypatch, 0, settings=wrong)
        assert status == 1 and calls == []


def test_the_command_reads_its_settings_from_a_file_it_deletes(tmp_path, monkeypatch):
    seen = []
    monkeypatch.setattr(vault_keys, "enable", lambda settings, **kw: seen.append(settings) or 0)
    path = tmp_path / "order.json"
    write(path, json.dumps(SETTINGS).encode())
    assert vault_keys.main(["enable", "--settings-file", str(path), "--json"]) == 0
    assert seen == [SETTINGS] and not path.exists()


# -- backups, restores, planned moves ----------------------------------------------------------------------------------

def offer_place(home: Path, sealed_keys=True, data_dir=None):
    """A place as the installer lays it out (data in ~/.local/share/bipagents, the space's folder in it), its vault's
    keys sealed (lot 1) or in today's layout."""
    data = data_dir or home / ".local" / "share" / "bipagents"
    config = home / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(f'bridge_key = "k"\nport = 8643\ndata_dir = "{data}"\n\n[hermes]\nhome = "{home / ".hermes"}"\n'
                      f'hub_url = "http://127.0.0.1:8642"\n\n[agents.vie]\nhermes_url = "http://127.0.0.1:8642/p/vie"\n'
                      f'hermes_key = "vie-key"\n')
    write(home / ".hermes" / "profiles" / "vie" / "SOUL.md", b"Tu es Vie.")
    write(data / "vault" / "vault.json.enc", b"the-master-copy-" + home.name.encode())
    write(data / "mail" / "accounts.json.enc", b"the-mailboxes-" + home.name.encode())
    space = home / ".local" / "share" / "bipagents" / "space"
    if sealed_keys:
        write(data / "keys.sealed.json", b'{"v": 1, "of": "' + home.name.encode() + b'"}')
        write(space / "vault.share", b"half-of-" + home.name.encode())
        write(space / "keyservice.json", json.dumps({**SETTINGS, "token": "token-of-" + home.name}).encode())
        os.chmod(space, 0o700)
    else:
        write(data / "vault" / "vault.key", b"clear-vault-key-of-" + home.name.encode())
        write(data / "mail" / "mail.key", b"clear-mail-key-of-" + home.name.encode())
    return SimpleNamespace(home=home, config=config, data=data, space=space)


@pytest.fixture
def runtime(tmp_path, monkeypatch):
    """The memory folder ($XDG_RUNTIME_DIR): the place key and the agents' Hermes copies (lots 1 and 2)."""
    run = tmp_path / "run"
    write(run / "bipagents" / "vault-0123456789abcdef.sk", b"THE-PLACE-KEY-IN-MEMORY")
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(run))
    monkeypatch.delenv("BIPAGENTS_SPACE_DIR", raising=False)
    return run


def backed_up(p, tmp_path):
    stage = tmp_path / f"stage-{p.home.name}"
    stage.mkdir(exist_ok=True)
    return [Path(path) for path in backup.files(moving.read_inventory(p.config, p.home), stage, {})]


@pytest.mark.parametrize("layout", ["offer", "elsewhere"])
def test_backups_carry_the_half_and_the_key_service_but_never_the_place_key(tmp_path, runtime, layout):
    home = tmp_path / "home" / "bip"
    p = offer_place(home, data_dir=None if layout == "offer" else home / ".local" / "share" / "hermes-bridge")
    files = backed_up(p, tmp_path)
    for needed in (p.space / "vault.share", p.space / "keyservice.json", p.data / "keys.sealed.json",
                   p.data / "vault" / "vault.json.enc"):
        assert needed in files, needed
    assert not [f for f in files if str(f).startswith(str(runtime))], "the place key in memory never goes"
    assert len([f for f in files if f.name == "vault.share"]) == 1
    # the look every 5 minutes sees them too (a new half after a sealing calls for a backup)
    inv = moving.read_inventory(p.config, p.home)
    looked = {path for root in inv.roots().values() if root.is_dir() for path, _, _ in backup.stat_walk(inv, root)}
    looked |= {str(root) for root in inv.roots().values() if root.is_file()}
    assert str(p.space / "vault.share") in looked and str(p.space / "keyservice.json") in looked


def test_a_planned_move_carries_them_too(tmp_path, runtime):
    p = offer_place(tmp_path / "home" / "bip", data_dir=tmp_path / "home" / "bip" / "data")
    planned = placemove.plan(p.home)
    roots = [Path(r) for r in planned["roots"].values()]
    assert p.space / "vault.share" in roots and p.space / "keyservice.json" in roots
    assert not [r for r in roots if str(r).startswith(str(runtime))]


def _fetched(p, tmp_path, name):
    """What `restic restore` gives back of a backup of `p`: its files under their absolute paths."""
    fetched = tmp_path / name
    for path in backed_up(p, tmp_path):
        dest = fetched / str(path).lstrip("/")
        dest.parent.mkdir(parents=True, exist_ok=True)
        if path.is_symlink():
            os.symlink(os.readlink(path), dest)
        else:
            dest.write_bytes(path.read_bytes())
            os.chmod(dest, path.stat().st_mode & 0o777)
    return fetched


@pytest.mark.parametrize("fresh_sealed", [True, False])
def test_a_restore_of_a_sealed_place_brings_its_half_and_key_service_back(tmp_path, runtime, fresh_sealed):
    old = offer_place(tmp_path / "old" / "bip3")
    fetched = _fetched(old, tmp_path, "fetched")
    new = offer_place(tmp_path / "new" / "bip", sealed_keys=fresh_sealed)
    info = backup.put_back(fetched, new.home, tmp_path / "checkout", "198.51.100.7")
    assert (new.data / "keys.sealed.json").read_bytes() == b'{"v": 1, "of": "bip3"}'
    assert (new.space / "vault.share").read_bytes() == b"half-of-bip3"
    assert json.loads((new.space / "keyservice.json").read_text())["token"] == "token-of-bip3"
    assert stat.S_IMODE((new.space / "vault.share").stat().st_mode) == 0o600
    assert stat.S_IMODE((new.space / "keyservice.json").stat().st_mode) == 0o600
    assert stat.S_IMODE(new.space.stat().st_mode) == 0o700
    assert (new.data / "vault" / "vault.json.enc").read_bytes() == b"the-master-copy-bip3"
    # a fresh place's clear keys never stay beside the person's sealed stores
    assert not (new.data / "vault" / "vault.key").exists() and not (new.data / "mail" / "mail.key").exists()
    assert not info.get("reseal")


def test_a_restore_of_a_place_in_today_s_layout_over_a_sealed_fresh_one_seals_it_again_once(tmp_path, runtime,
                                                                                               monkeypatch):
    old = offer_place(tmp_path / "old" / "bip3", sealed_keys=False)
    fetched = _fetched(old, tmp_path, "fetched")
    new = offer_place(tmp_path / "new" / "bip", sealed_keys=True)
    info = backup.put_back(fetched, new.home, tmp_path / "checkout", "198.51.100.7")
    # the person's stores with their own clear keys: today's layout, readable
    assert (new.data / "vault" / "vault.key").read_bytes() == b"clear-vault-key-of-bip3"
    assert (new.data / "vault" / "vault.json.enc").read_bytes() == b"the-master-copy-bip3"
    # the fresh place's sealing (which opens none of them) gone; its key service kept, to seal them again
    assert not (new.data / "keys.sealed.json").exists() and not (new.space / "vault.share").exists()
    assert json.loads((new.space / "keyservice.json").read_text())["token"] == "token-of-bip"
    assert info["reseal"] is True
    # the restore's end: sealed again with this place's own enrolment, once, before the bridge comes back
    from bipinstall import update
    sealed_with = []
    monkeypatch.setattr(backup, "fetch", lambda base, key: fetched)
    monkeypatch.setattr(backup, "put_back", lambda *a, **k: {**info, "browsers": False})
    monkeypatch.setattr(update, "gateway_units", lambda: [])
    monkeypatch.setattr(vault_keys, "enable", lambda settings, **kw: sealed_with.append((settings, kw)) or 0)
    ran = []
    runner = SimpleNamespace(log=[], run=lambda args, **kw: ran.append(list(args)))
    from bipinstall import migrations
    monkeypatch.setattr(migrations, "run", lambda r: [])
    backup.restore("key", runner, base=new.home, repo=tmp_path / "checkout")
    assert len(sealed_with) == 1 and sealed_with[0][0] is None, "this place's keyservice.json, nothing new"
    assert sealed_with[0][1].get("base") == new.home


# -- export and import -------------------------------------------------------------------------------------------------

def _in_memory_copy(p, runtime):
    """Lot 2's layout: the agent's Hermes vault a link to its copy in memory."""
    memory = runtime / "bipagents" / "hermes-vault-0123456789abcdef" / "vie.a1b2c3d4"
    write(memory / "vault.key", b"vie-hermes-key-in-memory")
    write(memory / "vault.json.enc", b"vie-hermes-copy-in-memory")
    os.symlink(memory, p.home / ".hermes" / "profiles" / "vie" / "vault")


def _exported(p, tmp_path, monkeypatch, keys=DATA_KEYS):
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: None)
    monkeypatch.setattr(hermes, "hermes_checkout", lambda: tmp_path / "no-git")
    asked = []

    def export_keys(inv):
        asked.append(inv.data_dir)
        if keys is None:
            raise vault_keys.VaultClosed("the vault is closed")
        return dict(keys)

    monkeypatch.setattr(vault_keys, "export_keys", export_keys)
    phrase = sealed.new_passphrase()
    out = tmp_path / "export.bipagents"
    machine = SimpleNamespace(system="linux", tailnet_name=None, tailscale=None)
    moving.export(moving.read_inventory(p.config, p.home), out, phrase, machine)
    return out, phrase, asked


def _members(out, phrase):
    tar, info, handle = moving.open_archive(out, phrase)
    members = {}
    while (member := tar.next()) is not None:
        members[member.name] = (member, tar.extractfile(member).read() if member.isfile() else b"")
    tar.close()
    handle.close()
    return info, members


def test_an_export_of_a_sealed_place_carries_its_keys_and_copies_never_its_half(tmp_path, runtime, monkeypatch):
    p = offer_place(tmp_path / "home" / "bip")
    _in_memory_copy(p, runtime)
    out, phrase, asked = _exported(p, tmp_path, monkeypatch)
    assert asked == [p.data]
    info, members = _members(out, phrase)
    assert json.loads(members[vault_keys.EXPORT_MEMBER][1]) == {"v": 1, "keys": DATA_KEYS}
    assert stat.S_IMODE(members[vault_keys.EXPORT_MEMBER][0].mode) == 0o600
    datas = [data for _, data in members.values()]
    assert b"half-of-bip" not in datas and not [n for n in members if n.endswith(("vault.share", "keyservice.json"))]
    assert b"THE-PLACE-KEY-IN-MEMORY" not in datas
    # the agent's copy in memory goes as a folder (no link to a memory the other machine hasn't)
    vault = [n for n, (m, _) in members.items() if n.endswith("profiles/vie/vault")]
    assert vault and all(members[n][0].isdir() for n in vault)
    assert members[vault[0] + "/vault.json.enc"][1] == b"vie-hermes-copy-in-memory"
    assert members[vault[0] + "/vault.key"][1] == b"vie-hermes-key-in-memory"


def test_an_export_of_a_sealed_place_waits_for_its_vault_to_be_open(tmp_path, runtime, monkeypatch):
    p = offer_place(tmp_path / "home" / "bip")
    with pytest.raises(sealed.SealedError, match="closed"):
        _exported(p, tmp_path, monkeypatch, keys=None)
    assert not (tmp_path / "export.bipagents").exists() and not list(tmp_path.glob("export.bipagents*"))


def test_an_export_of_a_place_in_today_s_layout_is_as_before(tmp_path, runtime, monkeypatch):
    p = offer_place(tmp_path / "home" / "bip", sealed_keys=False)
    out, phrase, asked = _exported(p, tmp_path, monkeypatch)
    assert asked == []
    info, members = _members(out, phrase)
    assert vault_keys.EXPORT_MEMBER not in members
    assert b"clear-vault-key-of-bip" in [data for _, data in members.values()]


def test_the_import_goes_back_to_today_s_layout(tmp_path, runtime, monkeypatch):
    p = offer_place(tmp_path / "home" / "bip")
    _in_memory_copy(p, runtime)
    out, phrase, _ = _exported(p, tmp_path, monkeypatch)
    tar, info, handle = moving.open_archive(out, phrase)
    target_home = tmp_path / "elsewhere" / "alex"
    target = moving.Target(home=target_home, repo=tmp_path / "checkout", tailnet_name=None, lan_address=None)
    config = target_home / ".config" / "bipagents" / "bridge.toml"
    moving.unpack(tar, info, target, config)
    handle.close()
    data = target_home / ".local" / "share" / "bipagents"
    for store, name in (("vault", "vault.key"), ("mail", "mail.key"), ("connections", "connections.key")):
        key = data / store / name
        assert key.read_text() == DATA_KEYS[store]
        assert stat.S_IMODE(key.stat().st_mode) == 0o600 and stat.S_IMODE(key.parent.stat().st_mode) == 0o700
    assert not (data / "keys.sealed.json").exists()
    assert not (data / "space" / "vault.share").exists() and not (data / "space" / "keyservice.json").exists()
    copy = target_home / ".hermes" / "profiles" / "vie" / "vault"
    assert copy.is_dir() and not copy.is_symlink()
    assert (copy / "vault.json.enc").read_bytes() == b"vie-hermes-copy-in-memory"


def test_back_to_today_refuses_keys_that_aren_t_the_three(tmp_path):
    data = tmp_path / "data"
    write(data / "keys.sealed.json", b"{}")
    for wrong in ({}, {"vault": "x"}, {**DATA_KEYS, "vault": ""}, {**DATA_KEYS, "vault": "../x"}, None):
        with pytest.raises(vault_keys.VaultKeysError):
            vault_keys.back_to_today(data, wrong)
        assert (data / "keys.sealed.json").exists()
