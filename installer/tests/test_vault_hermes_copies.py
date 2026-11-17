"""The agents' Hermes copies of the vault are never carried away from a place whose vault keys are sealed (design note
« Clé du coffre », lot 2): neither the export (`bipagents export`), nor the backups (backup.py), nor a planned move
(placemove.py) takes `<HERMES_HOME>/vault` or `<HERMES_HOME>/profiles/<agent>/vault` there; the bridge rebuilds them in
memory from its master copy, which goes as before. A link to the memory folder is never carried anywhere (it would
point at nothing on another server). A place not enrolled keeps today's layout: its copies go as they always did."""
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import backup, hermes, moving, placemove, sealed


def write(path: Path, data: bytes = b"x", mode: int = 0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    os.chmod(path, mode)


@pytest.fixture
def place(tmp_path, monkeypatch):
    """A place of the offer: Vie in the hub, Coach in its own space, each with its Hermes copy on the disk (today's
    layout), the bridge's master copy in its data folder."""
    home = tmp_path / "home" / "bip"
    data, hub = home / ".local" / "share" / "bipagents", home / ".hermes"
    config = home / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text(f'''bridge_key = "k"
port = 8643
data_dir = "{data}"

[hermes]
home = "{hub}"
hub_url = "http://127.0.0.1:8642"
isolated = true

[agents.vie]
hermes_url = "http://127.0.0.1:8642/p/vie"
hermes_key = "vie-key"

[agents.coach]
hermes_url = "http://127.0.0.1:8660/p/coach"
hermes_key = "coach-key"
space = "{data}/spaces/coach"
''')
    write(hub / "profiles" / "vie" / "SOUL.md", b"Tu es Vie.")
    write(hub / "profiles" / "vie" / "vault" / "vault.key", b"vie-hermes-key")
    write(hub / "profiles" / "vie" / "vault" / "vault.json.enc", b"vie-hermes-copy")
    write(hub / "vault" / "vault.json.enc", b"default-profile-copy")
    space = data / "spaces" / "coach"
    write(space / "profiles" / "coach" / "SOUL.md", b"Tu es Coach.")
    write(space / "profiles" / "coach" / "vault" / "vault.key", b"coach-hermes-key")
    write(space / "profiles" / "coach" / "vault" / "vault.json.enc", b"coach-hermes-copy")
    write(data / "vault" / "vault.json.enc", b"the-master-copy")
    write(data / "vault" / "vault.key", b"the-master-key")
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: "/usr/bin/podman")
    monkeypatch.setattr(hermes, "hermes_checkout", lambda: tmp_path / "no-git")
    return SimpleNamespace(home=home, config=config, data=data, hub=hub, space=space, tmp=tmp_path)


def seal(p):
    """The place enrolled (lot 1: keys.sealed.json, no clear key) and its copies moved into memory by the bridge:
    Vie's vault a link to the memory folder; Coach's still on the disk (not rebuilt yet: the bridge erases it once
    its rebuild is checked)."""
    (p.data / "vault" / "vault.key").unlink()
    write(p.data / "keys.sealed.json", b'{"v": 1}')
    memory = p.tmp / "run" / "bipagents" / "hermes-vault-0123456789abcdef" / "vie.a1b2c3d4"
    write(memory / "vault.key", b"vie-key-in-memory")
    write(memory / "vault.json.enc", b"vie-copy-in-memory")
    vault = p.hub / "profiles" / "vie" / "vault"
    for f in vault.iterdir():
        f.unlink()
    vault.rmdir()
    os.symlink(memory, vault)


def inventory(p):
    return moving.read_inventory(p.config, p.home)


def exported(p):
    """The export's file names and contents, read back from the sealed archive."""
    phrase = sealed.new_passphrase()
    out = p.tmp / "export.bipagents"
    machine = SimpleNamespace(system="linux", tailnet_name=None, tailscale=None)
    moving.export(inventory(p), out, phrase, machine)
    tar, opened, handle = moving.open_archive(out, phrase)
    members = {}
    for member in tar:
        data = tar.extractfile(member).read() if member.isfile() else b""
        members[member.name] = (member, data)
    tar.close()
    handle.close()
    return members


def backed_up(p):
    stage = p.tmp / "stage"
    stage.mkdir(exist_ok=True)
    return [str(path) for path in backup.files(inventory(p), stage, {})]


def looked(p):
    inv = inventory(p)
    return {path for root in inv.roots().values() if root.is_dir() for path, _, _ in backup.stat_walk(inv, root)}


def test_today_s_layout_still_carries_the_hermes_copies(place):
    members = exported(place)
    datas = [data for _, data in members.values()]
    assert b"vie-hermes-copy" in datas and b"coach-hermes-copy" in datas and b"default-profile-copy" in datas
    assert b"the-master-copy" in datas and b"the-master-key" in datas
    files = backed_up(place)
    for path in (place.hub / "profiles" / "vie" / "vault" / "vault.key",
                 place.space / "profiles" / "coach" / "vault" / "vault.json.enc", place.hub / "vault" / "vault.json.enc"):
        assert str(path) in files
    inv = inventory(place)
    for root in inv.roots().values():
        if root.is_dir():
            assert not [r for r in placemove.rules(inv, root) if r.endswith("/vault")]


def test_a_sealed_place_carries_no_hermes_copy_in_its_export_backups_or_move(place):
    seal(place)
    members = exported(place)
    names = sorted(members)
    datas = [data for _, data in members.values()]
    for secret in (b"vie-key-in-memory", b"vie-copy-in-memory", b"coach-hermes-key", b"coach-hermes-copy",
                   b"default-profile-copy"):
        assert secret not in datas
    assert not [n for n in names if n.endswith("/vault") and "/profiles/" in n], names
    assert not [n for n in names if n.split("/")[0] == "hermes" and n.split("/")[-1] == "vault"]
    # the bridge's master copy and its sealed keys go as before
    assert b"the-master-copy" in datas and b'{"v": 1}' in datas
    files = backed_up(place)
    assert not [f for f in files if "/profiles/" in f and "/vault" in f], files
    assert not [f for f in files if f.startswith(str(place.hub / "vault"))]
    assert str(place.data / "vault" / "vault.json.enc") in files and str(place.data / "keys.sealed.json") in files
    seen = looked(place)
    assert not [f for f in seen if "/profiles/" in f and "/vault" in f]
    assert str(place.data / "vault" / "vault.json.enc") in seen
    inv = inventory(place)
    hub_rules = placemove.rules(inv, inv.hermes_home)
    assert "- /vault" in hub_rules and "- /profiles/*/vault" in hub_rules
    data_rules = placemove.rules(inv, inv.data_dir)
    assert "- /spaces/coach/vault" in data_rules and "- /spaces/coach/profiles/*/vault" in data_rules
    assert "- /vault" not in data_rules, "the bridge's own master copy is moved"


def test_a_link_to_the_memory_folder_is_never_carried(place):
    """Even on a place not (or no longer) enrolled: carried, the link would point at nothing on the other server."""
    seal(place)
    (place.data / "keys.sealed.json").unlink()
    write(place.data / "vault" / "vault.key", b"the-master-key")
    members = exported(place)
    assert not [n for n, (m, _) in members.items() if m.issym() and n.endswith("/vault")]
    assert b"vie-key-in-memory" not in [data for _, data in members.values()]
    assert b"coach-hermes-copy" in [data for _, data in members.values()], "a copy on the disk goes, as today"
    assert not [f for f in backed_up(place) if f == str(place.hub / "profiles" / "vie" / "vault")]
