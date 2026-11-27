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
    return root


def test_the_check_writes_numbers_per_place_and_category_only(server, tmp_path):
    agent = seat_agent()
    state = tmp_path / "state" / "place-check.json"
    assert agent.place_check_run(state_path=str(state), check=str(CHECK), now=1000.0, limits=False) == 0
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
    assert agent.place_check_run(state_path=str(state), check=str(broken), now=5.0, limits=False) == 0
    assert json.loads(state.read_text()) == {"at": 5.0, "version": agent.place_check_version(str(broken)),
                                             "error": "MemoryError"}
    slow = tmp_path / "slow.py"
    slow.write_text("def collect(argv):\n    raise TooLong()\nclass TooLong(BaseException):\n    pass\n")
    agent.place_check_run(state_path=str(state), check=str(slow), now=6.0, limits=False)
    assert json.loads(state.read_text())["error"] == "TooLong"


def test_the_check_starts_once_a_day_and_when_an_update_brings_a_new_one(tmp_path):
    agent = seat_agent()
    state = tmp_path / "place-check.json"
    started = []
    popen = lambda args, **kw: started.append(args)   # noqa: E731
    version = agent.place_check_version(str(CHECK))
    assert agent.place_check_start(now=100.0, state_path=str(state), check=str(CHECK), popen=popen)   # never ran
    assert started[-1][-1] == "--place-check" and started[-1][1] == "-I"
    state.write_text(json.dumps({"at": 100.0, "version": version}))
    assert not agent.place_check_start(now=200.0, state_path=str(state), check=str(CHECK), popen=popen)
    assert agent.place_check_start(now=100.0 + agent.PLACE_CHECK_EVERY, state_path=str(state), check=str(CHECK),
                                   popen=popen)
    state.write_text(json.dumps({"at": 200.0, "version": "an older one"}))
    assert agent.place_check_start(now=300.0, state_path=str(state), check=str(CHECK), popen=popen)   # the wave's
    assert not agent.place_check_start(now=300.0, state_path=str(state), check=str(tmp_path / "gone.py"), popen=popen)
    import fcntl
    with open(str(state) + ".lock", "a") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        assert not agent.place_check_start(now=400.0, state_path=str(state), check=str(CHECK), popen=popen)
    assert len(started) == 3


def test_import_leftovers_are_taken_out_only_when_the_guichet_says_so(tmp_path, monkeypatch):
    agent = seat_agent()
    state = tmp_path / "place-check.json"
    state.write_text(json.dumps({"at": 7.0, "places": {"2": {"hermes_import": 2}, "3": {"crontab": 1},
                                                       "4": {"hermes_import": 1}}}))
    asked = []

    def command(entry, args, timeout):
        asked.append((entry.pw_name, args))
        if entry.pw_name == "bip4":
            raise agent.SeatError("clean-imports: no such command (an older installer)")
        return {"profiles": 1, "labels": 3}
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: type("E", (), {"pw_name": name})())
    assert agent.clean_imports_round({}, state_path=str(state), command=command) == 0            # off by default
    assert agent.clean_imports_round({"clean_imports": False}, state_path=str(state), command=command) == 0
    assert asked == []
    assert agent.clean_imports_round({"clean_imports": True}, state_path=str(state), command=command) == 1
    assert asked == [("bip2", ["clean-imports"]), ("bip4", ["clean-imports"])]               # crontab: never
    assert agent.clean_imports_round({"clean_imports": True}, state_path=str(state), command=command) == 0  # once
    kept = tmp_path / "settings.json"
    agent.keep_settings({"settings": {"clean_imports": True}}, path=str(kept))
    assert json.loads(kept.read_text()) == {"clean_imports": True}
    agent.keep_settings({"settings": {"clean_imports": "yes"}}, path=str(kept))
    assert json.loads(kept.read_text()) == {"clean_imports": True}


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
