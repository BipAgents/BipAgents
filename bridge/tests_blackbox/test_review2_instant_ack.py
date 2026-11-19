"""Review 2 of instant-ack (19/11): a failing test for what « Relecture 2 » found (status-instant-ack.md). Two messages
steered more than 20 s apart within ONE step (a long browser action) both get their line; Hermes stores them in one
row (agent/interrupt_control.py `steer`), which the bridge claims for both (send.rs `joined`): the second line must
stay, right after that row, like the first."""
import threading
import time

from bbkit.common import run_has
from bbkit.servers import wait_until
from test_instant_reply import MARK, SESSION, agent, busy_turn, instant, page, send, setup, user
from test_review_instant_ack import A, B1, B2, steer_row


def test_review2_two_lines_for_one_joined_row_both_stay(bridges, fakes):
    bridge, thread = setup(bridges, fakes, model=False)
    gate = threading.Event()
    busy_turn(fakes, gate)
    send(bridge, thread, A, "Trouve-moi un dentiste à Lyon")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    send(bridge, thread, B1, "plutôt le samedi")
    assert wait_until(lambda: instant(bridge, thread, B1), timeout=5)
    time.sleep(21)   # the conversation's next line allowed (instant_reply.rs GAP_SECONDS, 20 s)
    send(bridge, thread, B2, "et pas trop loin")
    assert wait_until(lambda: instant(bridge, thread, B2), timeout=5), "the second message's line is said"
    first, second = (s["body"]["input"] for s in fakes.hermes.steers[-2:])
    assert MARK in first and MARK in second
    # the step ends: Hermes stores the two steers it held as one row
    fakes.hermes.messages[("coach", SESSION)] += [user("Trouve-moi un dentiste à Lyon"),
                                                  steer_row(first.strip() + "\n" + second.strip()), agent("Fini.")]
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    items = page(bridge, thread)["items"]
    ids = [i["id"] for i in items]
    assert f"i_{B1}" in ids, ids
    assert f"i_{B2}" in ids, f"the second message's line vanished once Hermes stored the joined row: {ids}"
