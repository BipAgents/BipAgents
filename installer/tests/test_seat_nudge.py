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
    agent.nudge_units(str(folder), str(units), lambda *args: ran.append(args) or 0)
    agent.nudge_units(str(folder), str(units), lambda *args: ran.append(args) or 0)
    assert ran == [("systemctl", "daemon-reload"), ("systemctl", "enable", "--now", "bipagents-seats.path"),
                   ("systemctl", "is-active", "--quiet", "bipagents-seats.path")]   # each round looks: still there
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


# -- review 28/11 (red until fixed) ------------------------------------------------------------------------------------

def test_a_doorbell_unit_systemd_failed_is_started_again(tmp_path):
    """systemd leaves bipagents-seats.path failed for good once its service hits its start limit (5 starts in 10 s,
    path.c: PATH_FAILURE_UNIT_START_LIMIT_HIT; the timer keeps going): each round looks and starts it again, not only
    when its file changed."""
    agent = _seat_agent()
    folder, units = str(tmp_path / "nudge"), tmp_path / "units"
    units.mkdir()
    agent.nudge_units(folder, str(units), lambda *args: 0)
    ran = []

    def failed(*args):
        ran.append(args)
        return 3 if args[:2] == ("systemctl", "is-active") else 0   # `failed`: unit-start-limit-hit

    agent.nudge_units(folder, str(units), failed)
    assert ("systemctl", "reset-failed", "bipagents-seats.path") in ran
    assert ("systemctl", "start", "bipagents-seats.path") in ran
    ran.clear()
    agent.nudge_units(folder, str(units), lambda *args: ran.append(args) or 0)   # active: only the look
    assert ran == [("systemctl", "is-active", "--quiet", "bipagents-seats.path")]


def test_a_long_round_leaves_the_next_one_to_the_timer(tmp_path):
    """TimeoutStartSec=2h is the whole run's (guichet bootstrap.rs): a second round in the same run shares what is left
    of it, and an update or a move of that round is killed halfway. After a long round the timer (OnUnitActiveSec=2min,
    already past) starts the next run at once, with its own 2 hours."""
    agent = _seat_agent()
    folder = tmp_path / "nudge"
    me = (os.getuid(), os.getgid())
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    started = time.time() - 3600   # a round of an hour (a wave of updates, an install)
    os.utime(folder / "1", (started + 60, started + 60))   # rung during it
    assert agent.rung_since(started, str(folder))
    assert not agent.round_again([], started, str(folder))


def test_a_ring_set_in_the_future_is_no_ring(tmp_path):
    """The place owns its file: a time set ahead (utime) would make every run two rounds, for good."""
    agent = _seat_agent()
    folder = tmp_path / "nudge"
    me = (os.getuid(), os.getgid())
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    ahead = time.time() + 10 * 365 * 86400
    os.utime(folder / "1", (ahead, ahead))
    assert not agent.rung_since(time.time(), str(folder))


def test_a_doorbell_is_never_handed_to_another_account(tmp_path, monkeypatch):
    """A place installed again gets a new account, another uid (seat-agent.py drop_view): its doorbell is a new file,
    never the old account's inode chowned over (with its last ring in it, and the links that account made to it,
    which protected_hardlinks lets an owner make)."""
    agent = _seat_agent()
    folder = tmp_path / "nudge"
    me = (os.getuid(), os.getgid())
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    (folder / "1").write_text("1800000000\n")
    os.link(folder / "1", tmp_path / "kept")   # the account's own link to its doorbell
    linked = os.lstat(folder / "1").st_ino
    agent.doorbells("vps-1", {1: me[0]}, str(folder), me)
    assert os.lstat(folder / "1").st_ino != linked and os.lstat(folder / "1").st_nlink == 1, "a linked doorbell kept"
    assert (tmp_path / "kept").read_text() == "1800000000\n", "the link elsewhere untouched"
    (folder / "1").write_text("1800000001\n")
    # Held open, the old account's file keeps its inode: a file system that hands a freed inode number straight
    # back (ext4, the CI's runner) cannot make a new file look like the old one, nor the old one like a new one.
    old = os.open(folder / "1", os.O_RDONLY)
    chowned = []
    monkeypatch.setattr(os, "chown", lambda *args, **kw: chowned.append(args))   # not root here
    try:
        agent.doorbells("vps-1", {1: me[0] + 1}, str(folder), me)   # bip installed again: another uid
        assert os.lstat(folder / "1").st_ino != os.fstat(old).st_ino, "the old account's file chowned to the new one"
        assert os.read(old, 64) == b"1800000001\n", "the old account's file left as it was"
    finally:
        os.close(old)
    assert (folder / "1").read_text() == "" and os.lstat(folder / "1").st_mtime == 0
    assert chowned and chowned[-1][1] == me[0] + 1
