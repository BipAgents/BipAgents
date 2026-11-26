"""Root never acts through a link a place put in its own home (security review of 26/11, C1 and E1).

The seat agent (installer/cloud/seat-agent.py) and place-move.py run as root and read and write in the places' homes,
which their accounts own: a place can put a link (or a pipe) anywhere there, at any time, from its own shell. C1: the
`reopen` order wrote ~/.config/bipagents/setup.json.tmp through such a link, then chowned the file it named to the
place (root's own seat-agent.py, run by root every 15 s). E1: a move's plan comes from the place's account; root
copied the roots it named (outside the home too), and the target's root rsync wrote through a link the first round
had made. Each root write, removal and read in a home is tried here with a link on its way, and each read with a pipe
in place of its file: nothing outside the home is touched or read, and nothing blocks."""
import base64
import importlib.util
import json
import os
import shutil
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
KEY = base64.urlsafe_b64encode(bytes(range(32))).decode().rstrip("=")
ROOTS_CODE = "# root's code, run by root every 15 s\n"


def _load(name, file):
    spec = importlib.util.spec_from_file_location(name, ROOT / "installer" / "cloud" / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


agent = _load("seat_agent_links", "seat-agent.py")
pm = _load("place_move_links", "place-move.py")


def _entry(home, name="bip2"):
    return type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": name})


@pytest.fixture
def place(tmp_path, monkeypatch):
    """(home of place 2, a folder of root's with its code in it): what the place's links point at."""
    home = tmp_path / "home" / "bip2"
    home.mkdir(parents=True)
    system = tmp_path / "usr-local-lib-bipagents"
    system.mkdir()
    (system / "seat-agent.py").write_text(ROOTS_CODE)
    monkeypatch.setattr(agent, "account", lambda number: "bip2")
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda user: _entry(home))
    monkeypatch.setattr(agent, "log", lambda *a, **k: None)
    return home, system


def _untouched(system, also=()):
    assert (system / "seat-agent.py").read_text() == ROOTS_CODE, "root wrote root's code through the place's link"
    assert sorted(os.listdir(system)) == sorted(["seat-agent.py", *also]), "root made a file in root's folder"


def _plant(home, system, rel, how):
    """The place's gesture for the file `rel` of its home: a link at its name (`final`), at the `.tmp` name root
    used to write first (`tmp`), or in place of the folder that holds it (`folder`) or of the first folder on its way
    (`top`). Returns the file a write through it would reach."""
    path = home / rel
    if how in ("final", "tmp"):
        path.parent.mkdir(parents=True, exist_ok=True)
        os.symlink(system / "seat-agent.py", str(path) + (".tmp" if how == "tmp" else ""))
        return system / "seat-agent.py"
    parts = Path(rel).parts
    at = home / Path(*parts[:-1]) if how == "folder" else home / parts[0]
    at.parent.mkdir(parents=True, exist_ok=True)
    os.symlink(system, at)
    return system / (parts[-1] if how == "folder" else Path(*parts[1:]))


def _chowns(monkeypatch, module):
    """Every chown root does, by name or by descriptor: (device, inode) of what it gave away."""
    given = []
    real_fchown = module.os.fchown

    def by_name(path, uid, gid, *, dir_fd=None, follow_symlinks=True):
        st = os.stat(path, dir_fd=dir_fd, follow_symlinks=follow_symlinks)
        given.append((st.st_dev, st.st_ino))

    def by_fd(fd, uid, gid):
        st = os.fstat(fd)
        given.append((st.st_dev, st.st_ino))
        real_fchown(fd, uid, gid)
    monkeypatch.setattr(module.os, "chown", by_name)
    monkeypatch.setattr(module.os, "lchown", lambda path, uid, gid: by_name(path, uid, gid, follow_symlinks=False))
    monkeypatch.setattr(module.os, "fchown", by_fd)
    return given


def _never_given(given, system):
    for name in os.listdir(system):
        st = os.lstat(system / name)
        assert (st.st_dev, st.st_ino) not in given, "root gave %s to the place" % name
    st = os.lstat(system)
    assert (st.st_dev, st.st_ino) not in given, "root gave its folder to the place"


# -- C1: the reopen order, and each other write of root's in a home ------------------------------------------------------

@pytest.mark.parametrize("how", ["tmp", "final", "folder", "top"])
def test_reopen_never_writes_or_gives_away_through_a_link_the_place_put(place, monkeypatch, how):
    home, system = place
    given = _chowns(monkeypatch, agent)
    _plant(home, system, agent.SETUP, how)
    try:
        agent.reopen(2, "p" * 48)
    except OSError:
        pass   # a link on its way: refused (the seat agent logs it and goes on)
    _untouched(system)
    _never_given(given, system)
    if how in ("tmp", "final"):   # the place's own folder: written there, a file of its own, the links gone
        setup = home / agent.SETUP
        assert not setup.is_symlink() and json.loads(setup.read_text()) == {"pair": "p" * 48, "reopen": True}
        assert stat.S_IMODE(setup.stat().st_mode) == 0o600


def test_reopen_makes_the_place_s_missing_folders_its_own(place, monkeypatch):
    home, system = place
    given = _chowns(monkeypatch, agent)
    agent.reopen(2, "p" * 48)
    for rel in (".config", ".config/bipagents", agent.SETUP):
        st = os.lstat(home / rel)
        assert (st.st_dev, st.st_ino) in given, "%s left root's" % rel


def _credit(home):
    agent.keep_credit({"credit": {"2": {"state": "stopped"}}}, {2: (str(home), os.getuid(), os.getgid())})


def _disk(home, tmp_path):
    agent.tell_place(str(home), os.getuid(), os.getgid(), {"state": "full", "pause": True},
                     folder=str(tmp_path / "run-disk"))


WRITES = [
    ("credit", agent.CREDIT, lambda home, tmp_path: _credit(home)),
    ("disk", agent.PLACE_DISK_FILE, lambda home, tmp_path: _disk(home, tmp_path)),
    ("moved-away", pm.MOVED_AWAY, lambda home, tmp_path: pm.moved(
        "m-0123456", str(tmp_path / "moves"), str(tmp_path / "moves" / "m-0123456"), str(home), time.time(), {})),
]


@pytest.mark.parametrize("how", ["tmp", "final", "folder", "top"])
@pytest.mark.parametrize("name,rel,write", WRITES, ids=[w[0] for w in WRITES])
def test_root_s_other_writes_in_a_home_never_go_through_a_link(place, monkeypatch, tmp_path, how, name, rel, write):
    home, system = place
    given = _chowns(monkeypatch, agent)
    _chowns(monkeypatch, pm)
    _plant(home, system, rel, how)
    try:
        write(home, tmp_path)
    except OSError:
        pass
    _untouched(system)
    _never_given(given, system)


@pytest.mark.parametrize("how", ["folder", "top"])
def test_root_s_removals_in_a_home_never_go_through_a_link(place, how):
    """keep_credit removes a place's credit state when the guichet leaves it out, an update its card switch's echo:
    the file of that name in root's folder, reached through the place's link, stays."""
    home, system = place
    for rel, remove in ((agent.CREDIT, lambda: agent.keep_credit(
            {"credit": {}}, {2: (str(home), os.getuid(), os.getgid())})),
            (agent.OFFER_CARDS_SEEN, lambda: agent.forget_cards_seen(str(home)))):
        reached = _plant(home, system, rel, how)
        reached.parent.mkdir(parents=True, exist_ok=True)
        reached.write_text("root's")
        remove()
        assert reached.read_text() == "root's", "root removed %s through the place's link" % rel
        shutil.rmtree(home)
        home.mkdir()
        shutil.rmtree(system)
        system.mkdir()
        (system / "seat-agent.py").write_text(ROOTS_CODE)


# -- what root reads in a home: never through a link, never a pipe ----------------------------------------------------------

SECRET = "SECRET-of-root-or-of-a-neighbour"


def _sealed_toml(home, data):
    folder = home / ".config" / "bipagents"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "bridge.toml").write_text('port = 1\ndata_dir = "%s"\n' % data)


def _git_ref(home, system):
    git = home / "BipAgents" / ".git"
    git.mkdir(parents=True)
    (git / "HEAD").write_text("ref: %s\n" % (system / "guichet.json"))   # a ref naming a file of root's
    (system / "guichet.json").write_text(SECRET + "\n")


def _fresh_cards(path):
    path.write_text(json.dumps({"off": True, "pid": 4242}))


READS = [
    # name, the file it reads, what root's file holds, what the reader says of it, what it must say instead
    ("used", agent.USED, "1762950000", lambda home: agent.used(str(home)), None),
    ("locked", agent.LOCKED, json.dumps({"reason": SECRET}), lambda home: agent.locked(str(home)), None),
    ("version", "BipAgents/.git/HEAD", "c0ffee" * 7, lambda home: agent.version(str(home)), None),
    ("backup", agent.BACKUP_STATE, json.dumps({"last_ok": 1790000000.5}), lambda home: agent.last_backup("bip2"),
     None),
    ("cards", agent.OFFER_CARDS_SEEN, json.dumps({"off": True, "pid": 4242}),
     lambda home: agent.cards_seen(str(home), alive=lambda pid, uid: True), None),
    ("key service", agent.SPACE_KEY_SERVICE, json.dumps({"token": SECRET, "url": "https://keys.bipagents.com"}),
     lambda home: agent.vault_key_service(str(home)), None),
    ("vault failed", agent.VAULT_STATE, json.dumps({"state": "failed", "error": SECRET}),
     lambda home: agent.vault_failed(str(home)), None),
    ("time zone", agent.BRIDGE_DATA + "/time.json", json.dumps({"time_zone": "Europe/Secret"}),
     lambda home: agent.activity(str(home), now=1793000000).get("tz"), None),
    ("measures", agent.MEASURES, json.dumps({"team_proposed": 7}), lambda home: agent.measures(str(home)), {}),
    ("routines", agent.HERMES_HOME + "/cron/jobs.json",
     json.dumps({"jobs": [{"next_run_at": "2026-10-27T06:00:00Z"}]}), lambda home: agent.next_routine(str(home)),
     None),
    ("bridge settings", ".config/bipagents/bridge.toml", 'bridge_key = "%s"\n' % SECRET,
     lambda home: pm.bridge_settings(str(home)).get("bridge_key"), None),
    ("moved-away mark", pm.MOVED_AWAY, json.dumps({"move": "m-0123456"}), lambda home: _updated(home), True),
]


def _updated(home):
    """An update order for the place: done (True), or dropped as for the stopped copy a completed move left."""
    calls = []
    agent.update_order(2, "a" * 40, moves=str(home.parent / "moves"), home=str(home),
                       updater=lambda *a, **k: calls.append(a))
    return bool(calls)


def _quick(read, home, seconds=5):
    """What `read` says, or the test fails when it blocks (a pipe opened by root waits for a writer forever: the seat
    agent's round, and the beat of every place of the server, with it)."""
    said = {}

    def run():
        try:
            said["value"] = read(home)
        except OSError:
            said["value"] = None   # refused: nothing read
    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    thread.join(seconds)
    if thread.is_alive():
        return "blocked"
    return said.get("value")


@pytest.mark.parametrize("name,rel,content,read,expected", READS, ids=[r[0] for r in READS])
def test_root_s_reads_in_a_home_never_follow_a_link(place, monkeypatch, tmp_path, name, rel, content, read, expected):
    home, system = place
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "tried"))
    reached = _plant(home, system, rel, "top")
    reached.parent.mkdir(parents=True, exist_ok=True)
    reached.write_text(content)
    assert _quick(read, home) == expected, "root read %s through the place's link" % rel


@pytest.mark.parametrize("name,rel,content,read,expected", [r for r in READS if r[0] != "moved-away mark"],
                         ids=[r[0] for r in READS if r[0] != "moved-away mark"])
def test_root_s_reads_in_a_home_never_wait_on_a_pipe(place, monkeypatch, tmp_path, name, rel, content, read, expected):
    home, system = place
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "tried"))
    path = home / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    os.mkfifo(path)
    try:
        assert _quick(read, home) == expected
    finally:
        try:   # whatever is still blocked on it goes on
            os.close(os.open(path, os.O_WRONLY | os.O_NONBLOCK))
        except OSError:
            pass


def test_root_reads_no_ref_the_place_s_head_names_outside_its_checkout(place):
    home, system = place
    _git_ref(home, system)
    assert agent.version(str(home)) is None


def test_root_never_looks_for_sealed_keys_outside_the_home(place):
    """The bridge's data_dir is the place's to write: root's look at keys.sealed.json stays in the home."""
    home, system = place
    (system / agent.SEALED_KEYS).write_text("{}")
    _sealed_toml(home, system)
    service = home / agent.SPACE_KEY_SERVICE
    service.parent.mkdir(parents=True)
    service.write_text(json.dumps({"url": "https://keys.bipagents.com", "token": "t" * 32}))
    assert agent.vault_state(str(home)) == {}
    assert agent.sealed_key_services() == set()


def test_own_spaces_never_counted_through_a_link(place):
    home, system = place
    (system / "a" / "profiles").mkdir(parents=True)
    _plant(home, system, agent.SPACES + "/x", "folder")
    assert agent.own_spaces(str(home)) == 0


def test_a_completed_move_s_mark_is_never_read_through_a_link(place, tmp_path, monkeypatch):
    """`remove` and place-move's `cleanup` wipe the account whose home holds the move's mark: never one a link
    names."""
    home, system = place
    moves = tmp_path / "moves"
    pm.save("m-0123456", str(moves), role="out", seat=2, phase="done", kept_until=1)
    reached = _plant(home, system, pm.MOVED_AWAY, "folder")
    reached.write_text(json.dumps({"move": "m-0123456"}))
    wiped = []
    assert not agent.remove(2, {"move": {"id": "m-0123456"}}, str(moves), wiped.append, lambda *a, **k: None)
    monkeypatch.setattr(pm, "place", lambda n: _entry(home))
    assert pm.cleanup(str(moves), now=10, wipe=wiped.append) == [] and wiped == []


# -- E1: a move copies the place's own home only, and the target receives into a folder of root's -------------------------

class _Channel:
    """The source's end of the SSH channel, faked: what was asked and copied."""
    asked, copied = [], []

    def __init__(self, base, address, port, key):
        type(self).asked, type(self).copied = [], []
        self.key_file = os.path.join(base, "client_key")

    def ask(self, *words, timeout=900):
        type(self).asked.append(words[0])
        if words[0] == "info":
            return {"home": "/home/bip3"}
        if words[0] == "finish":
            return {"ok": True, "verified": {"bridge": True}}
        return {}

    def rsync(self, *args, **kwargs):
        type(self).copied.append(" ".join(str(a) for a in args))
        return 10, 1


def _move_out(tmp_path, monkeypatch, plan_of):
    """A stopped place's move_out (no drain), its account and steps faked; `plan_of(home)` gives the place's plan."""
    home = tmp_path / "home" / "bip2"
    (home / ".hermes").mkdir(parents=True)
    (home / ".local" / "state" / "bipagents" / "move-stage").mkdir(parents=True)
    entry = _entry(home)
    monkeypatch.setattr(pm, "place", lambda n: entry)
    monkeypatch.setattr(pm, "Channel", _Channel)
    monkeypatch.setattr(pm, "made_stopped", lambda *a, **k: None)
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: {})

    def as_user(e, args, timeout=0):
        return plan_of(home) if args == ["move-out", "plan"] else {}
    monkeypatch.setattr(pm, "as_user", as_user)
    move = {"id": "m-0123456", "key": KEY, "target": {"address": "203.0.113.9", "port": 2222}, "back_in": 60,
            "drain": False}
    order = tmp_path / "order.json"
    order.write_text(json.dumps({"seat": 2, "action": "move_out", "move": move}))
    return str(order), str(tmp_path / "moves"), home


def _plan(home, roots, stage=None, rules=None, said_home=None):
    return {"home": said_home or str(home), "stage": stage or str(home / ".local/state/bipagents/move-stage"),
            "roots": roots, "rules": rules or {}}


def _refused(order, root):
    assert pm.out(order, root) == 1
    state = pm.states(root)[0]
    assert state["phase"] == "failed", state
    assert not any(str(path).rstrip("/").endswith("neighbour") or "elsewhere" in str(path)
                   for path in _Channel.copied), "root copied what the plan named outside the place's home"
    return state


def test_a_plan_s_roots_outside_the_place_s_real_home_are_never_copied(tmp_path, monkeypatch):
    """Scenario B: the plan says its home is « / » and names a neighbour's folder: root's rsync read it."""
    neighbour = tmp_path / "home" / "bip3" / ".config" / "neighbour"
    neighbour.mkdir(parents=True)
    (neighbour / "bridge.toml").write_text(SECRET)
    order, root, home = _move_out(tmp_path, monkeypatch, lambda home: _plan(
        home, {"hermes": str(home / ".hermes"), "x": str(neighbour)}, said_home="/"))
    _refused(order, root)
    assert _Channel.copied == []


@pytest.mark.parametrize("how", ["the root", "on its way"])
def test_a_plan_s_root_that_is_or_goes_through_a_link_is_never_copied(tmp_path, monkeypatch, how):
    elsewhere = tmp_path / "elsewhere"
    (elsewhere / "inner").mkdir(parents=True)

    def plan_of(home):
        if how == "the root":
            os.symlink(elsewhere, home / "d")
            return _plan(home, {"hermes": str(home / ".hermes"), "d": str(home / "d")})
        os.symlink(elsewhere, home / "via")
        return _plan(home, {"hermes": str(home / ".hermes"), "d": str(home / "via" / "inner")})
    order, root, home = _move_out(tmp_path, monkeypatch, plan_of)
    _refused(order, root)
    assert _Channel.copied == []


def test_a_root_swapped_for_a_link_between_two_copies_is_never_copied(tmp_path, monkeypatch):
    """Each copy looks again: the place's own code runs while it is copied."""
    def plan_of(home):
        (home / "d").mkdir()
        return _plan(home, {"d": str(home / "d")})
    order, root, home = _move_out(tmp_path, monkeypatch, plan_of)
    real_save = pm.save

    def save(move_id, root=pm.MOVES, **fields):
        if fields.get("round") == 1:   # between the first copy and the switch's
            shutil.rmtree(home / "d")
            os.symlink(tmp_path / "elsewhere", home / "d")
            (tmp_path / "elsewhere").mkdir()
        return real_save(move_id, root, **fields)
    monkeypatch.setattr(pm, "save", save)
    assert pm.out(order, root) == 1
    assert len(_Channel.copied) == 1, "copied again through the link"


def test_a_plan_s_stage_or_rules_are_checked_too(tmp_path, monkeypatch):
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    order, root, home = _move_out(tmp_path, monkeypatch, lambda home: _plan(
        home, {"hermes": str(home / ".hermes")}, stage=str(elsewhere)))
    _refused(order, root)
    assert _Channel.copied == []
    for rule in (". /etc/shadow", "merge /etc/shadow", "+ /x", "- x\n. /etc/shadow", ": /etc/shadow"):
        order, root, home = _move_out(tmp_path / rule.replace("/", "_").replace("\n", "_"), monkeypatch,
                                      lambda home: _plan(home, {"hermes": str(home / ".hermes")},
                                                         rules={"hermes": ["- /cache", rule]}))
        _refused(order, root)
        assert _Channel.copied == [], rule


# -- E1 end to end: real rsync, the target's forced command run in place of ssh ----------------------------------------------

def _rsync3():
    if not shutil.which("rsync") or not shutil.which("unshare"):
        return False
    return b"version 3." in subprocess.run(["rsync", "--version"], capture_output=True).stdout


def _view_prefix():
    """The moves' mount namespace: root's own on a server; a user namespace's here, when not root (the battery runs
    these under `unshare -rm`, root there)."""
    if os.geteuid() == 0:
        return ["unshare", "--mount", "--propagation", "private", "--"]
    return ["unshare", "--user", "--map-root-user", "--mount", "--propagation", "private", "--"]


_VIEWS = {}


def _need_views(tmp_path):
    """The source's view (a read-only nosymfollow bind in a mount namespace) can be made here, tried once; else the
    test is skipped: GitHub's runners refuse unprivileged user namespaces, and a test that copies nothing proves
    nothing."""
    if "ok" not in _VIEWS:
        a, b = tmp_path / "probe-a", tmp_path / "probe-b"
        a.mkdir()
        b.mkdir()
        try:
            done = subprocess.run(_view_prefix() + ["/bin/sh", "-c", 'mount --bind -- "$1" "$2" && mount -o '
                                                    'remount,bind,ro,nosymfollow -- "$2"', "probe", str(a), str(b)],
                                  capture_output=True, timeout=30)
            _VIEWS["ok"] = done.returncode == 0
        except (OSError, subprocess.SubprocessError):
            _VIEWS["ok"] = False
    if not _VIEWS["ok"]:
        pytest.skip("no mount namespace with a nosymfollow bind here (unprivileged user namespaces refused)")


FAKE_SSH = r'''
import importlib.util, json, os, sys
config = json.load(open(os.environ["MOVE_TEST_TARGET"]))
spec = importlib.util.spec_from_file_location("place_move_target", config["script"])
pm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pm)
entry = type("Entry", (), config["entry"])
pm.place = lambda number: entry
pm.as_user = lambda e, args, timeout=0: {"prepared": True} if args == ["move-in", "prepare"] else {"volumes": []}
pm.bridge = lambda *a, **k: {"routines": []}
pm.gateways_answer = lambda home, timeout=0: []
pm.later_close = lambda move_id: None
args = sys.argv[1:]
if args[0] == "-l":   # rsync names the user apart: -l bipmove <address>
    args = args[2:]
sys.exit(pm.serve(config["move"], " ".join(args[1:]), config["root"]))
'''


@pytest.fixture
def two_servers(tmp_path, monkeypatch):
    """A source place (bip2) and a fresh target place (bip3) on « two servers » of this machine: the source's real
    rsync, its SSH channel replaced by a program that runs the target's forced command (`serve`) in its place. Not
    root: the moves' mount namespace is a user namespace's here."""
    if not _rsync3():
        pytest.skip("no GNU rsync 3 or unshare here")
    source = tmp_path / "a" / "home" / "bip2"
    target = tmp_path / "b" / "home" / "bip3"
    for home in (source, target):
        (home / ".config" / "bipagents").mkdir(parents=True)
        (home / ".hermes").mkdir()
        (home / ".local" / "state" / "bipagents" / "move-stage").mkdir(parents=True)
    (target / ".local" / "state" / "bipagents" / "move-in").mkdir(parents=True)
    victim = tmp_path / "b" / "usr-local-lib-bipagents"
    victim.mkdir(parents=True)
    (victim / "seat-agent.py").write_text(ROOTS_CODE)
    (victim / "add-seat.sh").write_text("# root's too\n")
    pm.save("m-0123456", str(tmp_path / "b" / "moves"), role="in", seat=3, phase="listening")
    fake = tmp_path / "fake-ssh.py"
    fake.write_text(FAKE_SSH)
    config = tmp_path / "target.json"
    config.write_text(json.dumps({"script": str(ROOT / "installer" / "cloud" / "place-move.py"),
                                  "root": str(tmp_path / "b" / "moves"), "move": "m-0123456",
                                  "entry": {"pw_dir": str(target), "pw_uid": os.getuid(), "pw_gid": os.getgid(),
                                            "pw_name": "bip3"}}))
    monkeypatch.setenv("MOVE_TEST_TARGET", str(config))

    class Channel(pm.Channel):
        def __init__(self, base, address, port, key):
            self.address, self.port = address, port
            os.makedirs(base, mode=0o700, exist_ok=True)
            self.key_file = os.path.join(base, "client_key")

        def ssh_args(self):
            return [sys.executable, str(fake)]
    monkeypatch.setattr(pm, "Channel", Channel)
    monkeypatch.setattr(pm, "VIEW", _view_prefix(), raising=False)
    monkeypatch.setattr(pm, "place", lambda n: _entry(source))
    monkeypatch.setattr(pm, "made_stopped", lambda *a, **k: None)
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: {})
    plans = {}

    def as_user(e, args, timeout=0):
        return plans["plan"](source) if args == ["move-out", "plan"] else {}
    monkeypatch.setattr(pm, "as_user", as_user)
    order = tmp_path / "order.json"
    order.write_text(json.dumps({"seat": 2, "action": "move_out", "move": {
        "id": "m-0123456", "key": KEY, "target": {"address": "203.0.113.9", "port": 2222}, "back_in": 60,
        "drain": False}}))
    return {"tmp": tmp_path, "source": source, "target": target, "victim": victim, "plans": plans,
            "order": str(order),
            "root": str(tmp_path / "a" / "moves"), "target_root": str(tmp_path / "b" / "moves"), "config": config,
            "fake": fake}


def _victim_untouched(victim):
    assert (victim / "seat-agent.py").read_text() == ROOTS_CODE, "the target's root rsync wrote through the link"
    assert sorted(os.listdir(victim)) == ["add-seat.sh", "seat-agent.py"], "rsync --delete emptied root's folder"


@pytest.mark.linux
def test_scenario_a_a_link_made_by_the_first_copy_is_never_followed_by_the_next(two_servers, monkeypatch):
    """The review's scenario A, the whole move: a root that is a link to root's folder at the first copy, a real
    folder holding the place's own seat-agent.py at the next."""
    _need_views(two_servers["tmp"])
    s = two_servers
    os.symlink(s["victim"], s["source"] / "d")

    def plan(home):
        return _plan(home, {"config": str(home / ".config" / "bipagents"), "hermes": str(home / ".hermes"),
                            "d": str(home / "d")})
    s["plans"]["plan"] = plan
    real_save = pm.save

    def save(move_id, root=pm.MOVES, **fields):
        if fields.get("round") == 1 and (s["source"] / "d").is_symlink():
            os.unlink(s["source"] / "d")
            (s["source"] / "d").mkdir()
            (s["source"] / "d" / "seat-agent.py").write_text("# the place's code\n")
        return real_save(move_id, root, **fields)
    monkeypatch.setattr(pm, "save", save)
    pm.out(s["order"], s["root"])
    _victim_untouched(s["victim"])


@pytest.mark.linux
def test_scenario_b_a_neighbour_s_folder_never_reaches_the_place_s_new_home(two_servers, tmp_path):
    _need_views(two_servers["tmp"])
    s = two_servers
    neighbour = tmp_path / "a" / "home" / "bip3" / ".config" / "bipagents"
    neighbour.mkdir(parents=True)
    (neighbour / "bridge.toml").write_text('bridge_key = "%s"\n' % SECRET)
    s["plans"]["plan"] = lambda home: _plan(home, {"hermes": str(home / ".hermes"), "x": str(neighbour)},
                                            said_home=str(tmp_path / "a" / "home"))
    pm.out(s["order"], s["root"])
    found = subprocess.run(["grep", "-rl", SECRET, str(s["target"])], capture_output=True, text=True).stdout
    assert found == "", "a neighbour's file reached the moved place's home: %s" % found


def _ask(s, *words):
    """One of the move's steps asked of the target, as the source's channel does."""
    done = subprocess.run([sys.executable, str(s["fake"]), "bipmove@203.0.113.9", " ".join(words)],
                          capture_output=True, text=True)
    return json.loads(done.stdout.strip().splitlines()[-1])


def _send(s, source, token):
    """A copy sent as any sender could (the target must hold whatever the stream says): rsync -a, like the move's."""
    return subprocess.run(["rsync", "-aH", "--numeric-ids", "--delete", "-e", "%s %s" % (sys.executable, s["fake"]),
                           str(source), "bipmove@203.0.113.9:" + token], capture_output=True, text=True)


@pytest.mark.linux
def test_the_target_writes_only_into_its_folder_of_root_s_whatever_the_stream_says(two_servers, tmp_path):
    """The target's own guard, apart from the source's: a link the stream made at one copy is never followed by the
    next, at the root (refused) or inside it (replaced, as rsync does without --keep-dirlinks)."""
    s = two_servers
    roots = base64.b64encode(json.dumps({"roots": {"d": "d", "e": "e"}}).encode()).decode()
    assert "error" not in _ask(s, "prepare", roots)
    sender = tmp_path / "sender"
    sender.mkdir()
    os.symlink(s["victim"], sender / "d")
    assert _send(s, sender / "d", "roots/d").returncode == 0      # a link, as a link
    os.unlink(sender / "d")
    (sender / "d").mkdir()
    (sender / "d" / "seat-agent.py").write_text("# the place's code\n")
    assert _send(s, str(sender / "d") + "/", "roots/d/").returncode != 0, "a copy onto a link was let through"
    _victim_untouched(s["victim"])
    (sender / "e").mkdir()
    os.symlink(s["victim"], sender / "e" / "x")
    assert _send(s, str(sender / "e") + "/", "roots/e/").returncode == 0
    os.unlink(sender / "e" / "x")
    (sender / "e" / "x").mkdir()
    (sender / "e" / "x" / "seat-agent.py").write_text("# the place's code\n")
    assert _send(s, str(sender / "e") + "/", "roots/e/").returncode == 0
    _victim_untouched(s["victim"])
    landed = s["target"] / pm.STAGING / "roots" / "e" / "x"
    assert not landed.is_symlink() and (landed / "seat-agent.py").read_text() == "# the place's code\n"
    for token in (str(s["target"] / ".hermes") + "/", "roots/../../x/", "roots/f/", "/etc/"):
        assert _send(s, str(sender / "e") + "/", token).returncode != 0, token
    assert sorted(os.listdir(s["target"])) == sorted([".config", ".hermes", ".local", pm.STAGING])


@pytest.mark.linux
def test_the_source_reads_no_link_swapped_in_on_a_root_s_way(two_servers, tmp_path, monkeypatch):
    """The view of the home the source's rsync reads: the place swaps a folder on a root's way for a link to a
    neighbour's right after root looked (at a copy), before rsync reads; the copy fails, nothing of the neighbour is
    sent. (main, which never looks, gets the swap at its first copy's end: its next copy sent the neighbour's file.)"""
    s = two_servers
    _need_views(tmp_path)
    source = s["source"]
    (source / "via" / "inner").mkdir(parents=True)
    (source / "via" / "inner" / "mine.txt").write_text("the place's own")
    neighbour = tmp_path / "a" / "home" / "bip3"
    (neighbour / "inner").mkdir(parents=True)
    (neighbour / "inner" / "secret.txt").write_text(SECRET)
    swapped = []

    def swap():
        if not swapped:
            shutil.rmtree(source / "via")
            os.symlink(neighbour, source / "via")
            swapped.append(1)
    real_kind = getattr(pm, "home_kind", None)
    looks = []

    def home_kind(home, rel):
        found = real_kind(home, rel)
        if rel == "via/inner":
            looks.append(rel)
            if len(looks) == 2:   # the plan's look, then the first copy's: swapped right after it
                swap()
        return found
    monkeypatch.setattr(pm, "home_kind", home_kind, raising=False)
    real_save = pm.save

    def save(move_id, root=pm.MOVES, **fields):
        if fields.get("round") == 1:
            swap()
        return real_save(move_id, root, **fields)
    monkeypatch.setattr(pm, "save", save)
    s["plans"]["plan"] = lambda home: _plan(home, {"d": str(home / "via" / "inner")})
    pm.out(s["order"], s["root"])
    assert swapped
    found = subprocess.run(["grep", "-rl", SECRET, str(s["target"])], capture_output=True, text=True).stdout
    assert found == "", "the neighbour's file reached the moved place's home: %s" % found


@pytest.mark.linux
def test_the_view_is_refused_when_its_mount_ignores_nosymfollow(tmp_path):
    """An older util-linux would drop `nosymfollow` without a word: the view is checked (findmnt) before rsync runs,
    else nothing runs."""
    _need_views(tmp_path)
    real = shutil.which("mount")
    fakebin = tmp_path / "bin"
    fakebin.mkdir()
    (fakebin / "mount").write_text("#!%s\nimport os, sys\nos.execv(%r, ['mount'] + [a.replace(',nosymfollow', '') "
                                   "for a in sys.argv[1:]])\n" % (sys.executable, real))
    (fakebin / "mount").chmod(0o755)
    home, view = tmp_path / "home", tmp_path / "view"
    home.mkdir()
    view.mkdir()
    ran = tmp_path / "ran"

    def view_runs(path):
        command = _view_prefix() + ["/bin/sh", "-c", pm.VIEW_SCRIPT, "place-move-view", str(home), str(view),
                                    "touch", str(ran)]
        subprocess.run(command, env=dict(os.environ, PATH=path), capture_output=True, timeout=60)
        found = ran.exists()
        if found:
            ran.unlink()
        return found
    assert view_runs(os.environ["PATH"]), "the view itself doesn't run here"
    assert not view_runs("%s:%s" % (fakebin, os.environ["PATH"])), "rsync ran in a view that follows links"


@pytest.mark.linux
def test_a_move_lands_the_place_s_files_in_the_new_home_and_keeps_the_target_s_own(two_servers):
    """The whole move, as it should go: the person's files over the fresh place's, what the rules leave out kept from
    the target (its config's lock: bridgeconf), the fresh files the source doesn't have gone (--delete), the stage
    in MOVE_IN, root's folder for the copies gone."""
    _need_views(two_servers["tmp"])
    s = two_servers
    src, dst = s["source"], s["target"]
    (src / ".config" / "bipagents" / "bridge.toml").write_text('bridge_key = "old"\n')
    (src / ".hermes" / "memories").mkdir()
    (src / ".hermes" / "memories" / "MEMORY.md").write_text("Le chat s'appelle Pixel.\n")
    os.symlink("memories/MEMORY.md", src / ".hermes" / "link-inside")   # links inside a root move as links
    (src / pm.STAGE / "manifest.json").write_text("{}")
    (dst / ".config" / "bipagents" / "bridge.toml").write_text('bridge_key = "fresh"\n')
    (dst / ".config" / "bipagents" / "bridge.toml.lock").write_text("the target's own")
    (dst / ".hermes" / "fresh-only").write_text("x")
    s["plans"]["plan"] = lambda home: _plan(home, {"config": str(home / ".config" / "bipagents"),
                                                   "hermes": str(home / ".hermes")},
                                            rules={"config": ["- bridge.toml.*"]})
    assert pm.out(s["order"], s["root"]) == 0, pm.states(s["root"])
    assert pm.states(s["target_root"])[0]["phase"] == "done"
    assert (dst / ".config" / "bipagents" / "bridge.toml").read_text() == 'bridge_key = "old"\n'
    assert (dst / ".config" / "bipagents" / "bridge.toml.lock").read_text() == "the target's own"
    assert (dst / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"
    assert os.readlink(dst / ".hermes" / "link-inside") == "memories/MEMORY.md"
    assert not (dst / ".hermes" / "fresh-only").exists()
    assert (dst / pm.MOVE_IN / "stage" / "manifest.json").exists()
    assert not (dst / pm.STAGING).exists()
    assert json.loads((src / pm.MOVED_AWAY).read_text())["move"] == "m-0123456"


def test_owners_are_shifted_in_root_s_folder_before_the_place_can_touch_its_files(tmp_path, monkeypatch):
    """Review of 59ee7434b: `finish` shifted the owners by path after the folders were back in the home, the place's
    by then: it could swap a folder not walked yet for a link to a neighbour's (its ~/.ssh, given first, lets it in),
    and root gave it the neighbour's files. Now shifted in STAGING, root's alone, by descriptors, then put back."""
    home = tmp_path / "home" / "bip3"
    (home / pm.MOVE_IN).mkdir(parents=True)
    (home / ".hermes").mkdir()
    neighbour = tmp_path / "home" / "bip4" / "secrets"
    (neighbour / "sub").mkdir(parents=True)
    (neighbour / "sub" / "key").write_text(SECRET)
    root = str(tmp_path / "moves")
    roots = {"hermes": ".hermes"}
    pm.save("m-0123456", root, role="in", seat=3, phase="receiving", roots=roots)
    pm.set_aside(str(home), roots)
    copied = home / pm.STAGING / "roots" / "hermes"   # what the copies brought, owned by the source's ids
    (copied / "zzz" / "sub").mkdir(parents=True)
    (copied / "a").write_text("x")
    (copied / "zzz" / "sub" / "f").write_text("y")
    ours = {os.lstat(copied / rel).st_ino for rel in ("a", "zzz", "zzz/sub", "zzz/sub/f")}
    me, group = os.getuid(), os.getgid()
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": me + 1, "pw_gid": group + 1, "pw_name": "bip3"})
    monkeypatch.setattr(pm, "subids", lambda user, path="/etc/subuid": None)
    monkeypatch.setattr(pm, "as_user", lambda e, args, timeout=0: {"volumes": []})
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: {"routines": []})
    monkeypatch.setattr(pm, "gateways_answer", lambda home, timeout=0: [])
    monkeypatch.setattr(pm, "later_close", lambda move_id: None)
    swapped, given = [], []

    def the_place(path):
        """Owner of its new ~/.hermes, it swaps ~/.hermes/zzz for a link to a neighbour's folder that holds a
        `sub` too, once root has listed zzz and not walked zzz/sub yet (root gives zzz/sub away: its path is
        in the home)."""
        zzz = home / ".hermes" / "zzz"
        if not swapped and str(path).endswith("/zzz/sub") and zzz.is_dir() and not zzz.is_symlink():
            shutil.rmtree(zzz)
            os.symlink(neighbour, zzz)
            swapped.append(1)

    def chown(path, uid, gid, *, dir_fd=None, follow_symlinks=True):
        given.append(os.stat(path, dir_fd=dir_fd, follow_symlinks=follow_symlinks).st_ino)
        the_place(path)
    monkeypatch.setattr(pm.os, "chown", chown)
    monkeypatch.setattr(pm.os, "lchown", lambda path, uid, gid: chown(path, uid, gid, follow_symlinks=False))
    src = {"uid": me, "gid": group, "sub_uid": None, "sub_gid": None}
    out = pm.finish("m-0123456", entry, {"source": src, "routines": 0}, root)
    assert out.get("ok"), out
    assert os.lstat(neighbour / "sub" / "key").st_ino not in given, "root gave a neighbour's file to the place"
    assert ours <= set(given), "the copies' owners were not all shifted"
    assert (home / ".hermes" / "a").exists() and not (home / pm.STAGING).exists()


# -- review of 59ee7434b: a nesting deep enough for Python's recursion stops nothing ----------------------------------------

DEEP = "[" * 200000


def _deep_cards(home):
    return agent.cards_seen(str(home), alive=lambda pid, uid: True)


DEEP_READS = [
    ("locked", agent.LOCKED, DEEP, lambda home: agent.locked(str(home))),
    ("measures", agent.MEASURES, DEEP, lambda home: agent.measures(str(home))),
    ("key service", agent.SPACE_KEY_SERVICE, DEEP, lambda home: agent.vault_key_service(str(home))),
    ("vault failed", agent.VAULT_STATE, DEEP, lambda home: agent.vault_failed(str(home))),
    ("time zone", agent.BRIDGE_DATA + "/time.json", DEEP, lambda home: agent.activity(str(home), now=1793000000)),
    ("backup", agent.BACKUP_STATE, DEEP, lambda home: agent.last_backup("bip2")),
    ("cards", agent.OFFER_CARDS_SEEN, DEEP, _deep_cards),
    ("routines", agent.HERMES_HOME + "/cron/jobs.json", DEEP, lambda home: agent.next_routine(str(home))),
    ("vault state", ".config/bipagents/bridge.toml", "a = " + DEEP, lambda home: agent.vault_state(str(home))),
    ("sealed key services", ".config/bipagents/bridge.toml", "a = " + DEEP,
     lambda home: agent.sealed_key_services()),
    ("moved-away mark", pm.MOVED_AWAY, DEEP, lambda home: pm.home_json(str(home), pm.MOVED_AWAY)),
]


@pytest.mark.parametrize("name,rel,content,read", DEEP_READS, ids=[r[0] for r in DEEP_READS])
def test_a_file_nested_too_deep_stops_no_read_of_root_s(place, monkeypatch, tmp_path, name, rel, content, read):
    """json.loads / tomllib.loads on « [[[[… » raise RecursionError: one such file in one home stopped health(), and
    so every beat of the server."""
    home, system = place
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "tried"))
    monkeypatch.setattr(agent, "MAX_SEATS", 1)
    path = home / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    read(home)   # no RecursionError


@pytest.mark.linux   # health() reads /proc (seat-agent.py `running`): Linux only, as the servers run it
def test_a_file_nested_too_deep_never_stops_the_beat(place, monkeypatch, tmp_path):
    home, system = place
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "tried"))
    monkeypatch.setattr(agent, "MAX_SEATS", 1)
    monkeypatch.setattr(agent, "door_open", lambda port: False)
    path = home / agent.LOCKED
    path.parent.mkdir(parents=True)
    path.write_text(DEEP)
    assert agent.health()["1"]["locked"] is None


def test_the_place_s_bridge_settings_nested_too_deep_are_an_error_the_move_handles(place):
    home, system = place
    path = home / ".config" / "bipagents" / "bridge.toml"
    path.parent.mkdir(parents=True)
    path.write_text("a = " + DEEP)
    with pytest.raises((OSError, ValueError)):
        pm.bridge_settings(str(home))
