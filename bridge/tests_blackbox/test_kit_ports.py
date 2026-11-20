"""The harness itself: each bridge a test talks to is its own (a once-failure, 20/11: test_group_names_checked got an
httpx error mid-test in a 16-worker run, never again alone). A port probed free can be bound by another process
before the bridge binds it; the bridge then exits (`[Errno 98]`), but `/health` on that port answered 200 from the
other process, so the test went on talking to someone else's bridge until that one stopped."""
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from bbkit.bridge import BridgeFailed

HERE = Path(__file__).resolve().parent


def test_a_bridge_whose_port_is_held_by_another_bridge_fails_to_start(bridges):
    first = bridges.new("first").start()
    second = bridges.new("second")
    second.config["port"] = first.port   # what a probe-then-bind race between two runs leaves
    with pytest.raises(BridgeFailed) as failed:
        second.start()
    assert "bind" in str(failed.value), str(failed.value)
    assert first.get("/health").status_code == 200   # the other one is left as it was


@pytest.mark.skipif(sys.platform == "win32", reason="flock")
def test_two_runs_whose_blocks_would_meet_take_different_ones(tmp_path):
    """Two runs on one machine whose run ids lead to the same block (here: the same id) still get blocks of their
    own, held while each worker lives."""
    script = textwrap.dedent("""
        import sys
        sys.path.insert(0, sys.argv[1])
        from bbkit import servers
        block = servers._worker_block()
        print(block.start, flush=True)
        sys.stdin.read()   # holds the block until told to end
    """)
    env = {**os.environ, "PYTEST_XDIST_WORKER": "gw3", "PYTEST_XDIST_TESTRUNUID": "same-run",
           "BB_PORT_LOCKS": str(tmp_path / "locks")}
    runs = [subprocess.Popen([sys.executable, "-c", script, str(HERE)], env=env, stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True) for _ in range(2)]
    try:
        starts = [int(run.stdout.readline()) for run in runs]
    finally:
        for run in runs:
            run.communicate("", timeout=10)
    assert starts[0] != starts[1], starts
    assert abs(starts[0] - starts[1]) >= 500, starts
