"""Review of instant-ack (19/11): failing tests that prove what the review found (see
passation-pro/briefs/status-instant-ack.md « Relecture 1 »). Hermes as it is: several steers held before the turn's
next step are joined with newlines into ONE user row (agent/interrupt_control.py `steer`), stored when the turn reads
them (its row's `timestamp`: that moment, not the send)."""
import threading
import time
from datetime import datetime

from bbkit.common import run_has
from bbkit.servers import wait_until
from test_instant_reply import MARK, SESSION, agent, busy_turn, instant, page, send, setup, user

A = "C1D00000-0000-4000-8000-0000000002A1"
B1 = "C1D00000-0000-4000-8000-0000000002A2"
B2 = "C1D00000-0000-4000-8000-0000000002A3"


def steer_row(text, at=None):
    row = {"role": "user", "display_kind": "steer",
           "content": f"[OUT-OF-BAND USER MESSAGE from the user]\n{text}\n[/OUT-OF-BAND USER MESSAGE]"}
    if at is not None:
        row["timestamp"] = at
    return row


def at_of(item):
    return datetime.fromisoformat(item["at"].replace("Z", "+00:00")).timestamp()


def two_steers_taken_together(bridges, fakes):
    bridge, thread = setup(bridges, fakes, model=False)
    gate = threading.Event()
    busy_turn(fakes, gate)
    send(bridge, thread, A, "Trouve-moi un dentiste à Lyon")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    send(bridge, thread, B1, "plutôt le samedi")
    send(bridge, thread, B2, "et pas trop loin")
    first, second = (s["body"]["input"] for s in fakes.hermes.steers[-2:])
    assert MARK in first and second == "et pas trop loin"
    # Hermes holds both until the step ends: one row, the two joined by a newline (each stripped)
    fakes.hermes.messages[("coach", SESSION)] += [user("Trouve-moi un dentiste à Lyon"),
                                                  steer_row(first.strip() + "\n" + second.strip()), agent("Fini.")]
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    return bridge, thread


def test_review_two_steers_taken_together_both_words_shown(bridges, fakes):
    """Regression of the branch: the person's row is cut at the note, so the second message's words vanish from it
    (on main the row shows « plutôt le samedi\\net pas trop loin »)."""
    bridge, thread = two_steers_taken_together(bridges, fakes)
    stored = [i for i in page(bridge, thread)["items"]
              if i["kind"] == "message" and i.get("role") == "person" and not i["id"].startswith("s_")]
    assert any("et pas trop loin" in i["text"] for i in stored), [i["text"] for i in stored]


def test_review_two_steers_taken_together_none_pas_envoye(bridges, fakes):
    """The second message the turn did read never comes back « Pas envoyé » (a tap would send it a second time).
    Fails on main too (one stored row claims one message): the branch makes it worse, its words are nowhere else."""
    bridge, thread = two_steers_taken_together(bridges, fakes)
    time.sleep(11)   # send.rs LOST_AFTER
    items = page(bridge, thread)["items"]
    assert not [i["id"] for i in items if i.get("not_sent")], [(i["id"], i.get("text")) for i in items]


def test_review_the_line_is_never_dated_before_the_message_it_answers(bridges, fakes):
    """The iPhone places a running turn's added messages and their lines by their `at` (ConversationModel+Timeline
    `placeAddedByTime`): a stored steer is dated when the turn read it, its line when it was said (a second after the
    send), so the line goes above its message until the turn ends. The bridge serves the line after its message in
    order: its date must not say otherwise."""
    bridge, thread = setup(bridges, fakes, model=False)
    gate = threading.Event()
    busy_turn(fakes, gate)
    send(bridge, thread, A, "Trouve-moi un dentiste à Lyon")
    assert run_has(bridge, "r1", "tool.started", agent="coach")
    send(bridge, thread, B1, "plutôt le samedi")
    assert wait_until(lambda: instant(bridge, thread, B1), timeout=5)
    steer = fakes.hermes.steers[-1]["body"]["input"]
    # the tool ends 40 s later in real life: Hermes stores the words then (here: dated 40 s ahead), the turn goes on
    now = time.time()
    fakes.hermes.messages[("coach", SESSION)] += [dict(user("Trouve-moi un dentiste à Lyon"), timestamp=now - 5),
                                                  steer_row(steer.strip(), at=now + 40)]

    def stored():
        return next((i for i in page(bridge, thread)["items"] if i.get("client_id") == B1 and not i["id"].startswith("s_")), None)

    # (the tip is read again within source.rs TIP_TTL, 30 s; a step event would do it at once)
    assert wait_until(stored, timeout=40), [(i['id'], i.get('client_id'), i.get('text'), i.get('at')) for i in page(bridge, thread)['items']]
    items = page(bridge, thread)["items"]
    ids = [i["id"] for i in items]
    message, line = stored(), instant(bridge, thread, B1)
    assert ids.index(line["id"]) == ids.index(message["id"]) + 1
    assert at_of(line) >= at_of(message), (message["at"], line["at"])
    gate.set()
