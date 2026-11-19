"""Review pass 5 (19/11) of the offer's card payments kill switch: red on 356796ecc, green since pass 6.

Pass 6: the bridge reads its commit once at start and rewrites the file every minute with its process id
(`follow_remote_switch`); the seat agent trusts it only while it is fresh and that process is still a bridge of the
place's account (`cards_seen`, `bridge_alive`). The helpers below model that bridge; the scenarios are the review's.

Before pass 6:

The bridge writes `offer-cards-seen.json` only at start and when the switch changes (bridge-rs/src/cards.rs
`follow_remote_switch` / `tell_seen`), with `running_commit` read from HEAD at the moment it writes. An update
order removes the file (seat-agent.py `update` -> `forget_cards_seen`) and moves HEAD; the bridge restarts only when
the update touched bridge-rs/ (installer/bipinstall/upgrade.py, `touched("bridge-rs/")`).
"""
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _seat_agent():
    spec = importlib.util.spec_from_file_location("seat_agent", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _place(tmp_path, agent, commit):
    home = tmp_path / "bip"
    git = home / "BipAgents" / ".git"
    git.mkdir(parents=True)
    (git / "HEAD").write_text(commit + "\n")
    seen = home / agent.OFFER_CARDS_SEEN
    seen.parent.mkdir(parents=True)
    return home, git, seen


RUNNING = set()   # the bridges running now, by process id (what /proc says to `bridge_alive`)


def _alive(pid, uid):
    return pid in RUNNING


def _start_bridge(agent, home, pid):
    """A bridge starts: its commit read once, from HEAD (cards.rs `running_commit`, same reading as seat-agent.py
    `version`, proved by `the_commit_this_place_runs_as_the_seat_agent_reads_it`)."""
    RUNNING.add(pid)
    return {"pid": pid, "version": agent.version(str(home))}


def _bridge_tells(bridge, seen, off):
    """What `tell_seen` writes, at start, at each change and every minute: its start commit and its process id."""
    seen.write_text(json.dumps({"off": off, "version": bridge["version"], "pid": bridge["pid"]}))


def test_review5_a_wave_that_does_not_touch_the_bridge_keeps_a_cut_place_counted_as_cut(tmp_path):
    """A cut is on; a wave brings an installer-only commit (bridge-rs/ unchanged: the bridge is not restarted and keeps
    cutting). The update removes the seen file and moves HEAD; the running bridge never writes it again (nothing
    changed for it), so the beat says nothing and `cards status` lists the place « not yet » for as long as the cut
    lasts, and « update it » (the status' advice) changes nothing."""
    agent = _seat_agent()
    RUNNING.clear()
    a = "aaaaaaaaaaaa1111111111111111111111111111"
    home, git, seen = _place(tmp_path, agent, a)
    bridge = _start_bridge(agent, home, 4242)
    _bridge_tells(bridge, seen, True)
    assert agent.cards_seen(str(home), alive=_alive) == "off"
    # the update order: forget, then the place's updater checks out B (installer/ only): no bridge restart
    agent.forget_cards_seen(str(home))
    (git / "HEAD").write_text("bbbbbbbbbbbb2222222222222222222222222222\n")
    # the same bridge process still cuts card payments; within the minute it says so again (pass 6)
    _bridge_tells(bridge, seen, True)
    assert agent.cards_seen(str(home), alive=_alive) == "off", "a place whose bridge still cuts is reported as not yet switched"


def test_review5_a_cut_during_a_wave_back_is_never_counted_as_switched(tmp_path):
    """A wave back from N (has the switch) to O (older). install.sh checks out O first; the bridge N keeps running for
    minutes (release wait up to 300 s, drain up to 5 min) before O's bridge replaces it. Kerfy cuts meanwhile: bridge N
    writes {"off": true, "version": O} (HEAD read at write time). O's bridge starts, ignores the switch and never
    touches the file: the version matches HEAD, the beat says "off", `cards status` counts the place as cut while its
    agents pay."""
    agent = _seat_agent()
    RUNNING.clear()
    n = "nnnnnnnnnnnn1111111111111111111111111111"
    home, git, seen = _place(tmp_path, agent, n)
    bridge_n = _start_bridge(agent, home, 4242)
    _bridge_tells(bridge_n, seen, False)
    # the update order to O
    agent.forget_cards_seen(str(home))
    (git / "HEAD").write_text("oooooooooooo2222222222222222222222222222\n")   # install.sh to_version, first
    _bridge_tells(bridge_n, seen, True)   # bridge N, still running, sees `cards off`
    assert agent.cards_seen(str(home), alive=_alive) == "off"   # true: N still runs and cuts
    # bridge O starts: older than the switch, writes nothing, takes card payments
    RUNNING.discard(4242)
    RUNNING.add(5151)
    assert agent.cards_seen(str(home), alive=_alive) is None, "an older bridge counted as having cut card payments"
    # and a file nobody rewrites for 3 minutes says nothing either, whatever process has that id since
    RUNNING.add(4242)
    import os
    import time
    old = time.time() - agent.SEEN_FRESH - 1
    os.utime(seen, (old, old))
    assert agent.cards_seen(str(home), alive=_alive) is None
