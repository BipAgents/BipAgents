"""The guichet's doorbell (internal/next/seat-nudge.md, 28/11): the seat agent keeps a folder with one file per place,
its account's, that the place's bridge writes when the guichet rings (bridge-rs/src/seat_nudge.rs), and a path unit
that starts its round as soon as one changes: a reopened place takes its order in a second or two, not up to 2 minutes
later. The 2-minute timer stays, the way back when the bell doesn't ring."""
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "installer"))


def _seat_agent():
    import importlib.util
    spec = importlib.util.spec_from_file_location("seat_agent", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _seen(path):
    """What changes when the folder is written: a write, a chmod or chown (ctime) or a new file (inode)."""
    st = os.lstat(path)
    return (st.st_ino, st.st_mtime_ns, st.st_ctime_ns, st.st_mode, st.st_uid)


def test_setup_writes_the_nudge_path_unit_and_folder(tmp_path):
    agent = _seat_agent()
    assert agent.NUDGE_DIR == "/var/lib/bipagents/nudge"
    folder = tmp_path / "nudge"
    me = (os.getuid(), os.getgid())
    uids = {1: me[0], 2: me[0]}
    agent.doorbells("vps-1", uids, str(folder), me)
    # root's folder, the server's id the guichet signs (root's, read by every place), one file per place, its own
    assert oct(folder.stat().st_mode & 0o777) == "0o755"
    assert sorted(p.name for p in folder.iterdir()) == ["1", "2", "server"]
    assert (folder / "server").read_text() == "vps-1\n"
    assert oct((folder / "server").stat().st_mode & 0o777) == "0o644"
    for number in ("1", "2"):
        bell = folder / number
        assert bell.is_file() and not bell.is_symlink()
        assert oct(bell.stat().st_mode & 0o777) == "0o600" and bell.stat().st_uid == me[0]
        assert bell.stat().st_mtime == 0, "a new doorbell is no ring"
    # each change in the folder starts a round: a round that finds everything as it should be changes nothing
    before = {p.name: _seen(p) for p in [folder, *folder.iterdir()]}
    agent.doorbells("vps-1", uids, str(folder), me)
    assert {p.name: _seen(p) for p in [folder, *folder.iterdir()]} == before
    # a place gone: its file goes (a later account never gets another's); a link put in a file's stead is replaced,
    # never followed; a mode changed is set back
    target = tmp_path / "elsewhere"
    target.write_text("keep")
    (folder / "1").unlink()
    os.symlink(target, folder / "1")
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    assert sorted(p.name for p in folder.iterdir()) == ["1", "server"]
    assert (folder / "1").is_file() and not (folder / "1").is_symlink() and target.read_text() == "keep"
    os.chmod(folder / "1", 0o666)
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    assert oct((folder / "1").stat().st_mode & 0o777) == "0o600"
    # the bridge's ring (the time written in its own file) is kept as it is
    (folder / "1").write_text("1800000000\n")
    rung = _seen(folder / "1")
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    assert _seen(folder / "1") == rung
    # its path unit, written once, enabled once, beside bipagents-seats.timer
    units, ran = tmp_path / "units", []
    units.mkdir()
    agent.nudge_units(str(folder), str(units), lambda *args: ran.append(args))
    agent.nudge_units(str(folder), str(units), lambda *args: ran.append(args))
    assert ran == [("systemctl", "daemon-reload"), ("systemctl", "enable", "--now", "bipagents-seats.path")]
    text = (units / "bipagents-seats.path").read_text()
    assert "PathChanged=%s\n" % folder in text and "WantedBy=paths.target" in text


def test_a_nudge_file_change_starts_a_beat(tmp_path):
    agent = _seat_agent()
    # the generated unit starts the seat agent's own service, on the folder the bridge writes in
    units = tmp_path / "units"
    units.mkdir()
    agent.nudge_units(units=str(units), runner=lambda *args: None)
    text = (units / "bipagents-seats.path").read_text()
    assert "[Path]\nPathChanged=/var/lib/bipagents/nudge\nUnit=bipagents-seats.service\n" in text
    # a bell rung while a round runs (its orders already fetched): the round runs once more, never twice
    folder = tmp_path / "nudge"
    me = (os.getuid(), os.getgid())
    agent.doorbells("vps-1", {1: me[0], 2: me[0]}, str(folder), me)
    started = time.time()
    assert not agent.rung_since(started, str(folder))
    assert not agent.round_again([], started, str(folder))
    (folder / "2").write_text("%d\n" % int(started))   # what the bridge does
    os.utime(folder / "2", (started + 1, started + 1))
    assert agent.rung_since(started, str(folder))
    assert agent.round_again([], started, str(folder))
    assert not agent.round_again(["--again"], started, str(folder))
    # a bell rung before the round started was this round's own
    assert not agent.rung_since(started + 2, str(folder))
    # no folder (a server set up before 28/11, its first round with the bell): nothing rung
    assert not agent.rung_since(started, str(tmp_path / "none"))
