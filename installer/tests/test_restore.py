"""A place moved after an outage: the backup of the dead one (another account, other ports, another door) put back
over a fresh install, with the key the phone gives back."""
import errno
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import pytest

from bipinstall import backup

T = 1_790_000_000.0


def toml(home, port, door, address, browser_first, setup=True):
    text = (f'bridge_key = "old-bridge-key"\nhost = "127.0.0.1"\nport = {port}\ndata_dir = "{home / "data"}"\n'
            f'cloud = true\npublic_url = "https://{address}' + ("" if door == 443 else f":{door}") + '"\n\n'
            f'[hermes]\nbin = "{home}/.local/bin/hermes"\nhome = "{home / ".hermes"}"\n'
            f'hub_url = "http://127.0.0.1:{port - 1}"\n\n[browser]\nenabled = true\nfirst_port = {browser_first}\n'
            f'ports = 10\n\n[media.roots]\n"{home}/.hermes/media" = "{home}/.hermes/media"\n\n'
            f'[lan]\nenabled = true\nport = {door}\naddress = "{address}"\n')
    if setup:
        text += (f'\n[setup]\nfile = "{home}/.config/bipagents/setup.json"\n'
                 f'progress = "{home}/.config/bipagents/setup-progress.json"\n')
    text += (f'\n[agents.coach]\ndisplay_name = "Coach"\nhermes_url = "http://127.0.0.1:{port - 1}/p/coach"\n'
             f'hermes_key = "agent-key"\n')
    return text


def make_home(root, name, port, door, address, browser_first):
    home = root / name
    config = home / ".config" / "bipagents"
    config.mkdir(parents=True)
    (home / "data" / "lan").mkdir(parents=True)
    (home / ".hermes" / "memories").mkdir(parents=True)
    (config / "bridge.toml").write_text(toml(home, port, door, address, browser_first))
    (home / ".hermes" / ".env").write_text(f"API_SERVER_PORT={port - 1}\n")
    return home


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_new_place_gets_the_dead_one_s_agents_with_its_own_door(tmp_path, monkeypatch):
    repo_dir = tmp_path / "repo"
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    (old / ".hermes" / "memories" / "MEMORY.md").write_text("Le chat s'appelle Pixel.\n")
    (old / "data" / "lan" / "cert.pem").write_text("OLD CERTIFICATE")
    db = sqlite3.connect(old / ".hermes" / "state.db")
    db.execute("CREATE TABLE messages (text TEXT)")
    db.execute("INSERT INTO messages VALUES ('rappelle-moi le vétérinaire')")
    db.commit()
    db.close()
    monkeypatch.setenv("HOME", str(old))
    key = backup.configure(str(repo_dir), "old-seat-token")["password"]
    backup.run(old, now=T)

    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    (new / "data" / "lan" / "cert.pem").write_text("FRESH CERTIFICATE")
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(repo_dir), "new-seat-token")
    with pytest.raises(backup.BackupError, match="doesn't open"):
        backup.fetch(new, "not the key")
    fetched = backup.fetch(new, key)
    info = backup.put_back(fetched, new, tmp_path / "checkout", "198.51.100.7")
    assert info["roots"]["hermes"] == str(old / ".hermes")
    assert (new / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"
    assert (new / "data" / "lan" / "cert.pem").read_text() == "OLD CERTIFICATE"      # the app's pin holds
    rows = sqlite3.connect(new / ".hermes" / "state.db").execute("SELECT text FROM messages").fetchall()
    assert rows == [("rappelle-moi le vétérinaire",)]
    text = (new / ".config" / "bipagents" / "bridge.toml").read_text()
    assert 'bridge_key = "old-bridge-key"' in text                                    # the phones' key
    assert "port = 8643\n" in text and 'hub_url = "http://127.0.0.1:8642"' in text   # this account's ports
    assert 'hermes_url = "http://127.0.0.1:8642/p/coach"' in text
    assert "[lan]\nenabled = true\nport = 443\naddress = \"198.51.100.7\"" in text     # this place's door
    assert 'public_url = "https://198.51.100.7"' in text and "first_port = 9600" in text
    others = [line for line in text.splitlines() if str(old) in line]
    assert others == [f'"{old}/.hermes/media" = "{new}/.hermes/media"']   # old MEDIA: lines still found
    assert f'file = "{new}/.config/bipagents/setup.json"' in text
    assert (new / ".hermes" / ".env").read_text() == "API_SERVER_PORT=8642\n"
    kept = backup.settings(new)
    assert kept["token"] == "new-seat-token" and kept["password"] == key              # its own seat, the person's key


def test_the_restore_says_its_steps_and_its_failure(tmp_path):
    backup.progress(tmp_path, 2)
    state = json.loads((tmp_path / ".config" / "bipagents" / "setup-progress.json").read_text())
    assert state == {"step": 2, "total": 4, "label": "Putting your agents back", "done": False, "restoring": True,
                     "error": None}
    backup.progress(tmp_path, 0, error="this key doesn't open these backups")
    state = json.loads((tmp_path / ".config" / "bipagents" / "setup-progress.json").read_text())
    assert state["error"] == state["label"] == "this key doesn't open these backups" and not state["done"]


def test_a_restore_that_breaks_on_anything_says_so_and_stops_restoring(tmp_path, monkeypatch):
    from bipinstall import update

    class Runner:
        log = []

        def run(self, *args, **kwargs):
            raise AssertionError("nothing runs after the failure")

    marker = tmp_path / backup.MARKER
    marker.parent.mkdir(parents=True)
    marker.write_text("{}")
    monkeypatch.setattr(backup, "fetch", lambda base, key: tmp_path / "fetched")
    monkeypatch.setattr(update, "gateway_units", lambda: [])

    def broken(*args):
        raise KeyError("roots")

    monkeypatch.setattr(backup, "put_back", broken)
    with pytest.raises(KeyError):
        backup.restore("key", Runner(), base=tmp_path, repo=tmp_path / "checkout")
    assert not marker.exists()
    state = json.loads((tmp_path / ".config" / "bipagents" / "setup-progress.json").read_text())
    assert state["error"] == "KeyError: 'roots'" and not state["done"]


# -- B3 (lab round trip 19/11): a restore over a running bridge, and a restore tried again -----------------------------

KEY = "a541c5a6b27e"   # a bridge release key: the same on the dead place and the new one (same version)


def a_dead_place_backed_up(tmp_path, monkeypatch, old_skip=False):
    """The dead place (bip3) backed up; with `old_skip`, as a backup from before the fix did it (with the data
    folder's bridge binaries). Returns (repo_dir, old home, key)."""
    from bipinstall import moving
    repo_dir = tmp_path / "repo"
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    (old / ".hermes" / "memories" / "MEMORY.md").write_text("Le chat s'appelle Pixel.\n")
    (old / "data" / "lan" / "cert.pem").write_text("OLD CERTIFICATE")
    (old / "data" / "bridge" / KEY).mkdir(parents=True)
    (old / "data" / "bridge" / KEY / "bipbridge").write_bytes(b"OLD PLACE'S BRIDGE")
    (old / "data" / "bridge" / KEY / "bip-browser-mcp").write_bytes(b"OLD PLACE'S BROWSER MCP")
    (old / "data" / "tools").mkdir(parents=True)
    (old / "data" / "tools" / "helper").write_bytes(b"OLD HELPER")
    monkeypatch.setenv("HOME", str(old))
    key = backup.configure(str(repo_dir), "old-seat-token")["password"]
    if old_skip:
        monkeypatch.setattr(moving, "DATA_SKIP", {"archive-index"})
    backup.run(old, now=T)
    monkeypatch.undo()
    return repo_dir, old, key


def a_fresh_place(tmp_path, monkeypatch, repo_dir):
    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    (new / "data" / "lan" / "cert.pem").write_text("FRESH CERTIFICATE")
    (new / "data" / "bridge" / KEY).mkdir(parents=True)
    (new / "data" / "bridge" / KEY / "bipbridge").write_bytes(b"THIS SERVER'S BRIDGE")
    (new / "data" / "bridge" / KEY / "bip-browser-mcp").write_bytes(b"THIS SERVER'S BROWSER MCP")
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(repo_dir), "new-seat-token")
    return new


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_backup_never_carries_the_bridge_s_own_binaries(tmp_path, monkeypatch):
    """They come from the install (rustbridge.py, ~/.local/share/bipagents/bridge/<key>/), as for a planned move."""
    from bipinstall import moving
    repo_dir, old, key = a_dead_place_backed_up(tmp_path, monkeypatch)
    inv = moving.read_inventory(old / ".config" / "bipagents" / "bridge.toml", old)
    assert inv.skipped(old / "data" / "bridge")
    new = a_fresh_place(tmp_path, monkeypatch, repo_dir)
    fetched = backup.fetch(new, key)
    assert not list(fetched.rglob("bipbridge")) and not list(fetched.rglob("bip-browser-mcp"))
    assert list(fetched.rglob("helper"))   # the rest of the data folder goes


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_restore_of_an_older_backup_leaves_this_server_s_bridge_alone_and_replaces_files_by_rename(
        tmp_path, monkeypatch):
    """A backup made before the fix holds the dead place's bridge binaries: they never come over this server's
    (the running bridge's own file: « Text file busy » on Linux). Every other file is replaced by a rename, never
    written in place: a program running from it (or a reader holding it open) keeps its old file."""
    repo_dir, old, key = a_dead_place_backed_up(tmp_path, monkeypatch, old_skip=True)
    new = a_fresh_place(tmp_path, monkeypatch, repo_dir)
    (new / "data" / "tools").mkdir()
    (new / "data" / "tools" / "helper").write_bytes(b"FRESH HELPER")
    held = tmp_path / "held-helper"
    held.hardlink_to(new / "data" / "tools" / "helper")   # the file a running program would have open
    fetched = backup.fetch(new, key)
    assert list(fetched.rglob("bipbridge"))               # the older backup does hold them
    backup.put_back(fetched, new, tmp_path / "checkout", "198.51.100.7")
    assert (new / "data" / "bridge" / KEY / "bipbridge").read_bytes() == b"THIS SERVER'S BRIDGE"
    assert (new / "data" / "bridge" / KEY / "bip-browser-mcp").read_bytes() == b"THIS SERVER'S BROWSER MCP"
    assert (new / "data" / "tools" / "helper").read_bytes() == b"OLD HELPER"
    assert held.read_bytes() == b"FRESH HELPER"            # replaced by a rename, not written into
    assert (new / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"


class FakeRestic:
    """Just enough restic for a machine without it (the build server): a snapshot is a copy of the listed files at
    their full paths, restored the way restic does (under the target, at their full paths)."""

    def __init__(self, store):
        self.store = store

    def __call__(self, args, env, timeout=3600):
        done = lambda code=0, out="": subprocess.CompletedProcess(args, code, out, "")   # noqa: E731
        if args[0] == "backup":
            listing = Path(args[args.index("--files-from-raw") + 1]).read_bytes().split(b"\0")
            shutil.rmtree(self.store, ignore_errors=True)
            for raw in filter(None, listing):
                path = Path(os.fsdecode(raw))
                copy = self.store / str(path).lstrip("/")
                copy.parent.mkdir(parents=True, exist_ok=True)
                if path.is_symlink():
                    os.symlink(os.readlink(path), copy)
                else:
                    shutil.copy2(path, copy)
            return done(out=json.dumps({"message_type": "summary", "snapshot_id": "s1", "total_files_processed":
                                        len(listing), "total_bytes_processed": 1, "data_added": 1}))
        if args[0] == "restore":
            shutil.copytree(self.store, Path(args[args.index("--target") + 1]), symlinks=True, dirs_exist_ok=True)
        return done()


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="« Text file busy » is Linux's")
def test_a_restore_over_running_programs_works_on_linux(tmp_path, monkeypatch):
    """The lab's failure for real: the data folder's programs running while the restore puts the files back."""
    fake = FakeRestic(tmp_path / "snapshot")
    if shutil.which("restic") is None:
        monkeypatch.setattr(backup, "restic", fake)
    repo_dir, old, key = a_dead_place_backed_up(tmp_path, monkeypatch, old_skip=True)
    if shutil.which("restic") is None:
        monkeypatch.setattr(backup, "restic", fake)
    new = a_fresh_place(tmp_path, monkeypatch, repo_dir)
    # bash, not sleep: Ubuntu 26.04's sleep is one multi-call program that picks its job from its file name, so a
    # copy named bipbridge exits at once. The copied bash keeps running its own image while it waits on stdin.
    stand_in = shutil.which("bash")
    running = []
    (new / "data" / "tools").mkdir()
    for path in (new / "data" / "bridge" / KEY / "bipbridge", new / "data" / "tools" / "helper"):
        shutil.copy2(stand_in, path)
        running.append(subprocess.Popen([str(path), "-c", "read -r _"], stdin=subprocess.PIPE))
    try:
        time.sleep(0.2)
        assert all(proc.poll() is None for proc in running), "the stand-in programs must still be running"
        with pytest.raises(OSError) as busy:   # what the restore did before: written in place
            with open(new / "data" / "tools" / "helper", "wb") as fh:
                fh.write(b"x")
        assert busy.value.errno == errno.ETXTBSY
        fetched = backup.fetch(new, key)
        backup.put_back(fetched, new, tmp_path / "checkout", "198.51.100.7")
        assert (new / "data" / "tools" / "helper").read_bytes() == b"OLD HELPER"
        assert (new / "data" / "bridge" / KEY / "bipbridge").read_bytes() == Path(stand_in).read_bytes()
    finally:
        for proc in running:
            proc.kill()
            proc.wait()


class QuietRunner:
    def __init__(self):
        self.log, self.ran = [], []

    def run(self, cmd, **kwargs):
        self.ran.append(cmd)
        return ""


def quiet_restore(monkeypatch):
    from bipinstall import family, migrations, rustbridge, update
    monkeypatch.setattr(update, "gateway_units", lambda: [])
    monkeypatch.setattr(migrations, "run", lambda runner: [])
    monkeypatch.setattr(rustbridge, "cli", lambda home, repo=None: ["bipbridge"])
    monkeypatch.setattr(family, "bridge_restart", lambda home=None: ["true"])


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_restore_that_fails_halfway_can_be_tried_again(tmp_path, monkeypatch):
    """The lab's second failure: the half-done restore had put the dead seat's backup settings (its token, refused
    by the guichet: 401) and its bridge.toml in place of the fresh ones, so the app's new try could never fetch the
    backup. Now the fresh install's settings come back when a restore fails, and the second try works."""
    repo_dir, old, key = a_dead_place_backed_up(tmp_path, monkeypatch)
    new = a_fresh_place(tmp_path, monkeypatch, repo_dir)
    quiet_restore(monkeypatch)
    fresh_toml = (new / ".config" / "bipagents" / "bridge.toml").read_text()
    real_copy = backup._copy
    calls = []

    def breaks_on_the_data(src, dst, *args, **kwargs):
        calls.append(dst)
        if "data" in Path(dst).parts and "tools" in Path(dst).parts:   # after the door's certificate came
            raise OSError(26, "Text file busy", str(dst))
        return real_copy(src, dst, *args, **kwargs)

    monkeypatch.setattr(backup, "_copy", breaks_on_the_data)
    with pytest.raises(OSError):
        backup.restore(key, QuietRunner(), base=new, repo=tmp_path / "checkout", address="198.51.100.7")
    assert backup.settings(new)["token"] == "new-seat-token"                       # this seat's, not the dead one's
    assert (new / ".config" / "bipagents" / "bridge.toml").read_text() == fresh_toml
    assert (new / ".config" / "bipagents" / "setup.json").read_text() == '{"pair": "fresh"}'   # the app may claim
    assert (new / "data" / "lan" / "cert.pem").read_text() == "FRESH CERTIFICATE"   # the one the guichet lists
    state = json.loads((new / ".config" / "bipagents" / "setup-progress.json").read_text())
    assert state["error"] and not state["done"]
    monkeypatch.setattr(backup, "_copy", real_copy)
    backup.restore(key, QuietRunner(), base=new, repo=tmp_path / "checkout", address="198.51.100.7")
    assert (new / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"
    text = (new / ".config" / "bipagents" / "bridge.toml").read_text()
    assert 'bridge_key = "old-bridge-key"' in text and 'public_url = "https://198.51.100.7"' in text
    kept = backup.settings(new)
    assert kept["token"] == "new-seat-token" and kept["password"] == key
    assert not (new / backup.FRESH_COPY).exists()


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_restore_killed_halfway_is_undone_by_the_next_try(tmp_path, monkeypatch):
    """Killed with the dead seat's settings in place (no `except` ran): the next try first puts the fresh ones back,
    so it fetches the backup with this seat's token."""
    repo_dir, old, key = a_dead_place_backed_up(tmp_path, monkeypatch)
    new = a_fresh_place(tmp_path, monkeypatch, repo_dir)
    quiet_restore(monkeypatch)

    class Killed(BaseException):
        pass

    real_copy = backup._copy

    def killed_on_the_data(src, dst, *args, **kwargs):
        if "data" in Path(dst).parts:
            raise Killed()
        return real_copy(src, dst, *args, **kwargs)

    monkeypatch.setattr(backup, "_copy", killed_on_the_data)
    with pytest.raises(Killed):
        backup.restore(key, QuietRunner(), base=new, repo=tmp_path / "checkout", address="198.51.100.7")
    monkeypatch.setattr(backup, "_copy", real_copy)
    # what a kill leaves when the config folder came first: the dead seat's settings
    (new / ".config" / "bipagents" / "backup.json").write_text(json.dumps(
        {"url": str(repo_dir), "token": "old-seat-token", "password": key}))
    backup.restore(key, QuietRunner(), base=new, repo=tmp_path / "checkout", address="198.51.100.7")
    assert backup.settings(new)["token"] == "new-seat-token"
    assert (new / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_failed_restore_puts_the_fresh_folders_back_as_private_as_they_were(tmp_path, monkeypatch):
    """Review 1 of fix-restore-import (19/11): `keep_fresh` makes each kept folder with mkdir's default mode and
    `back_to_fresh` copies that mode over the real one (`_copy`'s copystat): the door's certificate folder (0700,
    bridge-rs/src/lan.rs) and the config folder come back 0755 after a failed restore."""
    import stat
    repo_dir, old, key = a_dead_place_backed_up(tmp_path, monkeypatch)
    new = a_fresh_place(tmp_path, monkeypatch, repo_dir)
    quiet_restore(monkeypatch)
    folders = (new / ".config" / "bipagents", new / "data" / "lan")
    for folder in folders:
        os.chmod(folder, 0o700)
    real_copy = backup._copy

    def breaks_on_the_data(src, dst, *args, **kwargs):
        if "tools" in Path(dst).parts:
            raise OSError(28, "No space left on device", str(dst))
        return real_copy(src, dst, *args, **kwargs)

    monkeypatch.setattr(backup, "_copy", breaks_on_the_data)
    with pytest.raises(OSError):
        backup.restore(key, QuietRunner(), base=new, repo=tmp_path / "checkout", address="198.51.100.7")
    assert [stat.S_IMODE(folder.stat().st_mode) for folder in folders] == [0o700, 0o700]


def test_a_file_left_half_written_by_a_killed_restore_is_cleaned_and_never_backed_up(tmp_path, monkeypatch):
    """Review 1 (F3): `_copy` writes beside the file then renames; killed in between, the partial file stays."""
    from bipinstall import moving
    home = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    partial = home / ".hermes" / "memories" / ".bip-restoring-123-abcd1234"
    partial.write_text("half")
    inv = moving.read_inventory(home / ".config" / "bipagents" / "bridge.toml", home)
    assert inv.skipped(partial) and backup._may_skip()(partial.name)
    assert backup.clean_partials(inv) == 1 and not partial.exists()
