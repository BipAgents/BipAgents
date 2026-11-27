"""The seat agent's check of its places (27/11, security triage section 0, lot A0): installer/cloud/place_check.py run
as root in a process of its own, once a day and when an update brings a new one; its numbers per category (never a
name, never a content) joined to each place in the beat; nothing cleaned, but an import's leftovers in a place's Hermes
profiles when the guichet's `clean_imports` says so (off by default)."""
import importlib.util
import json
import os
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[2]
CHECK = ROOT / "installer" / "cloud" / "place_check.py"
SECRET = "the-person-s-name-4242"


def seat_agent():
    spec = importlib.util.spec_from_file_location("seat_agent_check", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def put(path, text, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    os.chmod(path, mode)


@pytest.fixture
def server(tmp_path, monkeypatch):
    """A server of two places (PLACES_ROOT): bip clean, bip3 with a crontab and an import's hooks, named by the place."""
    root = tmp_path / "server"
    put(root / "etc" / "passwd", "bip:x:5001:5001::/home/bip:/bin/bash\nbip3:x:5003:5003::/home/bip3:/bin/bash\n")
    for user in ("bip", "bip3"):
        (root / "home" / user / ".hermes" / "profiles" / "assistant").mkdir(parents=True)
    put(root / "home" / "bip3" / ".hermes" / "profiles" / "assistant" / "hooks" / SECRET / "handler.py", "x\n")
    put(root / "var" / "spool" / "cron" / "crontabs" / "bip3", f"* * * * * {SECRET}\n")
    monkeypatch.setenv("PLACES_ROOT", str(root))
    monkeypatch.setenv("PLACES_SETTLE", "0")
    return root


def test_the_check_writes_numbers_per_place_and_category_only(server, tmp_path):
    agent = seat_agent()
    state = tmp_path / "state" / "place-check.json"
    assert agent.place_check_run(state_path=str(state), check=str(CHECK), now=1000.0) == 0
    kept = json.loads(state.read_text())
    assert kept["at"] == 1000.0 and kept["version"] == agent.place_check_version(str(CHECK))
    assert set(kept["places"]) == {"1", "3"}
    assert not any(kept["places"]["1"].values())
    assert kept["places"]["3"]["crontab"] == 1 and kept["places"]["3"]["hermes_import"] == 1
    assert sum(kept["places"]["3"].values()) == 2
    assert SECRET not in state.read_text()


def test_the_beat_carries_each_place_s_numbers_while_they_are_recent(tmp_path, monkeypatch):
    agent = seat_agent()
    state = tmp_path / "place-check.json"
    state.write_text(json.dumps({"at": 1000.0, "version": "v", "places": {"3": {"crontab": 1, "process": 0}}}))
    places = agent.place_check_beat({"3": {"mem_mb": 10}}, now=2000.0, state_path=str(state))
    assert places["3"] == {"mem_mb": 10, "check": {"at": 1000, "found": {"crontab": 1, "process": 0}}}
    assert agent.place_check_beat({}, now=1000.0 + agent.PLACE_CHECK_FRESH + 1, state_path=str(state)) == {}
    state.write_text(json.dumps({"at": 1000.0, "error": "MemoryError"}))
    monkeypatch.setattr(agent, "place_homes", lambda: {"1": ("/home/bip", 5001, 5001)})
    assert agent.place_check_beat({}, now=2000.0, state_path=str(state)) == {"1": {"check": {"at": 1000,
                                                                                            "error": "MemoryError"}}}


def test_a_check_that_fails_writes_its_error_and_never_raises(tmp_path):
    agent = seat_agent()
    broken = tmp_path / "place_check.py"
    broken.write_text("raise MemoryError()\n")
    state = tmp_path / "place-check.json"
    assert agent.place_check_run(state_path=str(state), check=str(broken), now=5.0) == 0
    version = agent.place_check_version(str(broken))
    assert json.loads(state.read_text()) == {"at": 5.0, "version": version, "tried": 5.0, "tried_version": version,
                                             "error": "MemoryError"}
    slow = tmp_path / "slow.py"
    slow.write_text("def collect(argv):\n    raise TooLong()\nclass TooLong(BaseException):\n    pass\n")
    agent.place_check_run(state_path=str(state), check=str(slow), now=6.0)
    assert json.loads(state.read_text())["error"] == "TooLong"


def test_the_check_starts_in_a_unit_of_its_own_once_a_day_and_when_an_update_brings_a_new_one(tmp_path):
    """Out of the round's oneshot unit (systemd kills what is left in it when the round ends): systemd-run, bounded by
    systemd, never a second while its unit runs; an attempt that dies counts (not started again every round)."""
    agent = seat_agent()
    state = tmp_path / "place-check.json"
    asked, active = [], []

    def run(command):
        asked.append(command)
        if command[0] == "systemctl":
            return 0 if active else 3
        return 0
    version = agent.place_check_version(str(CHECK))
    assert agent.place_check_start(now=100.0, state_path=str(state), check=str(CHECK), run=run)   # never ran
    started = asked[-1]
    assert started[0] == "systemd-run" and "--unit=bipagents-place-check" in started and "--collect" in started
    assert "MemoryMax=512M" in started and any(a.startswith("RuntimeMaxSec=") for a in started) and "Nice=19" in started
    assert started[-1] == "--place-check" and started[-3] == "-I"
    assert json.loads(state.read_text()) == {"tried": 100.0, "tried_version": version}
    assert not agent.place_check_start(now=200.0, state_path=str(state), check=str(CHECK), run=run)   # tried today
    assert agent.place_check_start(now=100.0 + agent.PLACE_CHECK_EVERY, state_path=str(state), check=str(CHECK),
                                   run=run)
    state.write_text(json.dumps({"at": 200.0, "version": "an older one"}))
    active.append(1)                                                       # its unit runs: never a second
    assert not agent.place_check_start(now=300.0, state_path=str(state), check=str(CHECK), run=run)
    active.clear()
    assert agent.place_check_start(now=300.0, state_path=str(state), check=str(CHECK), run=run)       # the wave's
    assert not agent.place_check_start(now=300.0, state_path=str(state), check=str(tmp_path / "gone.py"), run=run)
    assert sum(1 for a in asked if a[0] == "systemd-run") == 3


def test_import_leftovers_are_taken_out_only_when_the_guichet_says_so(tmp_path, monkeypatch):
    agent = seat_agent()
    state = tmp_path / "place-check.json"
    state.write_text(json.dumps({"at": 7.0, "places": {"2": {"hermes_import": 2}, "3": {"crontab": 1},
                                                       "4": {"hermes_import": 1}}}))
    asked, units = [], []

    def command(entry, args, timeout):
        asked.append((entry.pw_name, args))
        if entry.pw_name == "bip4":
            raise agent.SeatError("clean-imports: gateway not restarted")
        return {"profiles": 1, "labels": 3}
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: type("E", (), {"pw_name": name})())
    run = lambda c: units.append(c) or (3 if c[0] == "systemctl" else 0)   # noqa: E731
    assert not agent.clean_imports_start({}, state_path=str(state), run=run)                    # off by default
    assert not agent.clean_imports_start({"clean_imports": False}, state_path=str(state), run=run)
    assert units == []
    assert agent.clean_imports_start({"clean_imports": True}, state_path=str(state), run=run)   # out of the round
    assert "--unit=bipagents-clean-imports" in units[-1] and units[-1][-1] == "--clean-imports"
    assert agent.clean_imports_run({"clean_imports": True}, state_path=str(state), command=command) == 1
    assert asked == [("bip2", ["clean-imports"]), ("bip4", ["clean-imports"])]               # crontab: never
    assert json.loads(state.read_text())["cleaned"] == {"2": 7.0}                             # bip4 failed: not marked
    asked.clear()
    assert agent.clean_imports_run({"clean_imports": True}, state_path=str(state), command=command) == 0
    assert asked == [("bip4", ["clean-imports"])]                                              # tried again
    kept = tmp_path / "settings.json"
    agent.keep_settings({"settings": {"clean_imports": True}}, path=str(kept))
    assert json.loads(kept.read_text()) == {"clean_imports": True}
    agent.keep_settings({"settings": {"clean_imports": "yes"}}, path=str(kept))
    assert json.loads(kept.read_text()) == {"clean_imports": True}


def test_one_place_never_blinds_the_look_at_its_neighbours(server, monkeypatch):
    """Each place within its own time (place_check.py PLACE_SECONDS): past it, that place alone is « error »."""
    import time as _time
    monkeypatch.setenv("PLACES_PLACE_SECONDS", "0.5")
    spec = importlib.util.spec_from_file_location("place_check_budget", CHECK)
    check = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(check)
    real = check.place

    def slow(user, *args):
        if user == "bip3":
            while True:
                _time.sleep(0.05)
        return real(user, *args)
    monkeypatch.setattr(check, "place", slow)
    _, rows, _, _ = check.collect([])
    numbers = check.counts(rows)
    assert numbers["bip3"]["error"] == 1 and sum(numbers["bip3"].values()) == 1
    assert not any(numbers["bip"].values())


def test_the_agent_takes_the_check_from_root_s_checkout_with_itself():
    agent = seat_agent()
    assert "place_check.py" in agent.TOOLS and agent.PLACE_CHECK.endswith("/place_check.py")
    assert (ROOT / "installer" / "cloud" / "seat-agent.py").read_text(encoding="utf-8").isascii()


def test_clean_imports_as_the_place_says_numbers_and_keeps_the_labels_for_its_person(tmp_path, monkeypatch, capsys):
    from bipinstall import profile_guard, update
    home = tmp_path / "bip3"
    hub = home / ".hermes"
    put(home / ".config" / "bipagents" / "bridge.toml", f'data_dir = "{home}/.local/share/bipagents"\n[hermes]\n'
                                                         f'home = "{hub}"\nhub_url = "http://127.0.0.1:8672"\n')
    put(hub / "config.yaml", yaml.safe_dump({"terminal": {"backend": "docker"}}))
    put(hub / "profiles" / "coach" / "config.yaml", yaml.safe_dump({"mcp_servers": {SECRET: {"command": "/bin/sh"}}}))
    put(hub / "profiles" / "coach" / "hooks" / "x" / "handler.py", "x\n")
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(update, "gateway_units", lambda: [])
    assert profile_guard.clean_main([]) == 0
    said = capsys.readouterr().out.strip().splitlines()[-1]
    assert json.loads(said) == {"profiles": 1, "labels": 2} and SECRET not in said
    kept = json.loads((home / profile_guard.STATE).read_text())
    assert kept["left_out"] == sorted(["hooks", f"mcp:{SECRET}"])
    assert not (hub / "profiles" / "coach" / "hooks").exists()
    assert profile_guard.clean_main([]) == 0 and json.loads(capsys.readouterr().out.strip()) == {"profiles": 0,
                                                                                               "labels": 0}


def test_a_gateway_that_does_not_restart_leaves_the_clean_up_not_done(tmp_path, monkeypatch, capsys):
    """What a gateway loaded (a hook, a plugin) stays in it until it restarts: a failed restart is an error (the seat
    agent tries again), kept in the state, which the check counts as an import's leftover until it restarts."""
    from bipinstall import hermes, profile_guard, update
    home = tmp_path / "bip3"
    hub = home / ".hermes"
    put(home / ".config" / "bipagents" / "bridge.toml", f'data_dir = "{home}/.local/share/bipagents"\n[hermes]\n'
                                                         f'home = "{hub}"\nhub_url = "http://127.0.0.1:8672"\n')
    put(hub / "profiles" / "coach" / "hooks" / "x" / "handler.py", "x\n")
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    fails = [True]

    def run(self, args, *, check=True, **kw):
        if fails[0]:
            raise RuntimeError("systemctl failed (1)")
        return ""
    monkeypatch.setattr(hermes.Runner, "run", run)
    assert profile_guard.clean_main([]) == 1
    assert json.loads(capsys.readouterr().out.strip().splitlines()[-1])["error"] == "gateway not restarted"
    assert json.loads((home / profile_guard.STATE).read_text())["restart_pending"] is True
    fails[0] = False
    assert profile_guard.clean_main([]) == 0                                  # nothing left, the restart tried again
    assert json.loads((home / profile_guard.STATE).read_text())["restart_pending"] is False


def test_the_check_counts_a_gateway_still_to_restart(server):
    import importlib.util as iu
    put(server / "home" / "bip" / ".local" / "state" / "bipagents" / "profile-guard.json",
        json.dumps({"at": 1, "left_out": ["hooks"], "restart_pending": True}))
    spec = iu.spec_from_file_location("place_check_pending", CHECK)
    check = iu.module_from_spec(spec)
    spec.loader.exec_module(check)
    _, rows, _, _ = check.collect([])
    assert check.counts(rows)["bip"]["hermes_import"] == 1
