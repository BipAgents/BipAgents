"""Independent review of fix-import-exec, relecture 4 (85f20bca5): the seat agent's place check, red tests."""
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def user_systemd() -> bool:
    if sys.platform != "linux" or not shutil.which("systemd-run"):
        return False
    env = {**os.environ, "XDG_RUNTIME_DIR": f"/run/user/{os.getuid()}"}
    done = subprocess.run(["systemctl", "--user", "is-system-running"], capture_output=True, text=True, env=env)
    return done.stdout.strip() in ("running", "degraded")


@pytest.mark.skipif(not user_systemd(), reason="needs a systemd user manager (the build server's)")
def test_the_check_a_round_starts_outlives_the_round(tmp_path):
    """The seat agent runs as `bipagents-seats.service`, Type=oneshot (guichet bootstrap.rs seat_agent), KillMode left
    at control-group. place_check_start() Popens `--place-check` with start_new_session=True, which leaves the
    session, not the unit's cgroup: when the round's main process exits, systemd stops the unit and kills what is left
    in it. The check never writes its state; the next round (2 min) finds it due again and starts another, killed in
    turn. Here: the same round in a transient oneshot unit of the user manager, with a check that takes 3 s."""
    agent = tmp_path / "seat-agent.py"
    state = tmp_path / "state" / "place-check.json"
    fake = tmp_path / "fake_check.py"
    fake.write_text("import time\ndef collect(argv):\n    time.sleep(3)\n    return {}, {}, {}, {}\n"
                    "def counts(rows):\n    return {}\n")
    text = (ROOT / "installer" / "cloud" / "seat-agent.py").read_text(encoding="utf-8")
    for old, new in (('PLACE_CHECK = os.path.join(LIB, "place_check.py")', f"PLACE_CHECK = {str(fake)!r}"),
                     ('PLACE_CHECK_STATE = "/var/lib/bipagents/place-check.json"', f"PLACE_CHECK_STATE = {str(state)!r}")):
        assert old in text
        text = text.replace(old, new)
    agent.write_text(text, encoding="utf-8")
    driver = tmp_path / "round.py"
    driver.write_text("import importlib.util, sys\n"
                      f"spec = importlib.util.spec_from_file_location('seat', {str(agent)!r})\n"
                      "m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)\n"
                      "assert m.place_check_start()\n")
    env = {**os.environ, "XDG_RUNTIME_DIR": f"/run/user/{os.getuid()}"}
    unit = f"review-place-check-{os.getpid()}"
    subprocess.run(["systemd-run", "--user", "--quiet", "--collect", "--wait", f"--unit={unit}", "-p", "Type=oneshot",
                    sys.executable, str(driver)], check=True, env=env, timeout=60)
    deadline = time.time() + 20
    while time.time() < deadline and not state.is_file():
        time.sleep(0.5)
    assert state.is_file(), "the check started by the round was killed with it: no state written"
