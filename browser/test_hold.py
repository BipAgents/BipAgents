"""The hold flag as the browser container's two doors read it (python3 -m pytest browser/test_hold.py): the person's
hold stops everything; a card payment's hold (« card <token> ») lets only the bridge's own DevTools connections
through, and tells the agent's tools why they wait."""
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import devtools_relay  # noqa: E402
import hold_gate  # noqa: E402

HEAD = b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9223\r\n"


def use_flag(monkeypatch, tmp_path, text):
    flag = tmp_path / "hold"
    if text is not None:
        flag.write_bytes(text)
    monkeypatch.setattr(hold_gate, "HOLD", str(flag))
    monkeypatch.setattr(devtools_relay, "HOLD", str(flag))
    return flag


def test_the_relay_tells_its_version_even_while_held(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"human 0123abcd")
    assert devtools_relay.VERSION_PATH.match(b"GET /bipagents/relay HTTP/1.1\r\nHost: x\r\n\r\n")
    assert not devtools_relay.VERSION_PATH.match(HEAD + b"\r\n")
    answer = devtools_relay.version_answer()
    head, _, body = answer.partition(b"\r\n\r\n")
    assert head.startswith(b"HTTP/1.1 200 OK") and json.loads(body)["relay"] == 3 and json.loads(body)["hold_token"] is True
    assert f"Content-Length: {len(body)}".encode() in head


def test_nothing_held_lets_everything_through(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, None)
    assert devtools_relay.let_through(HEAD + b"\r\n")
    assert not hold_gate.held()


def test_the_persons_hold_stops_everything(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"")
    assert hold_gate.held() and not hold_gate.paying()
    assert devtools_relay.payment_token() is None
    assert not devtools_relay.let_through(HEAD + b"X-Bip-Card: abc\r\n\r\n")
    refusal = json.loads(hold_gate.refusal({"id": 3}))
    assert refusal["result"]["content"][0]["text"] == hold_gate.WAIT


def test_a_payment_lets_only_the_bridge_through(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"card 0123abcd")
    assert hold_gate.held() and hold_gate.paying()
    assert devtools_relay.let_through(HEAD + b"x-bip-card: 0123abcd\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"X-Bip-Card: 0123abce\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"\r\n")
    refusal = json.loads(hold_gate.refusal({"id": 3}))
    assert "card payment" in refusal["result"]["content"][0]["text"]


def test_the_persons_hold_lets_only_the_bridges_view_through(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"human 77aa88bb")
    assert hold_gate.held() and not hold_gate.paying()
    assert devtools_relay.payment_token() is None and devtools_relay.hold_token() == b"77aa88bb"
    assert devtools_relay.let_through(HEAD + b"X-Bip-Hold: 77aa88bb\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"X-Bip-Hold: 77aa88bc\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"\r\n")
    assert json.loads(hold_gate.refusal({"id": 3}))["result"]["content"][0]["text"] == hold_gate.WAIT


def test_an_old_hold_of_the_person_lapses(monkeypatch, tmp_path):
    flag = use_flag(monkeypatch, tmp_path, b"human 77aa88bb")
    old = time.time() - hold_gate.LIMIT - 5
    os.utime(flag, (old, old))
    assert not hold_gate.held() and devtools_relay.let_through(HEAD + b"\r\n")


def test_a_payments_hold_never_lapses(monkeypatch, tmp_path):
    # Security check 11/11, N2: a bridge restarted mid-payment left the « card » flag, and after 15 minutes the gate
    # and the relay let the agent back in with the card still typed in the page.
    flag = use_flag(monkeypatch, tmp_path, b"card 0123abcd")
    old = time.time() - 20 * 60
    os.utime(flag, (old, old))
    assert hold_gate.held() and hold_gate.paying()
    assert not devtools_relay.let_through(HEAD + b"\r\n")
    assert devtools_relay.let_through(HEAD + b"X-Bip-Card: 0123abcd\r\n\r\n")   # the bridge, to leave the page
    assert "card payment" in json.loads(hold_gate.refusal({"id": 4}))["result"]["content"][0]["text"]
    # The person's hold of the same age, by contrast, has lapsed.
    flag.write_bytes(b"human 77aa88bb")
    os.utime(flag, (old, old))
    assert not hold_gate.held()
    # The bridge tells an image with this gate by this text (card_pay.rs `calls_done`): no payment is typed in one
    # whose gate still lets a payment's hold lapse (fix review 11/11, 3rd check, P6).
    with open(hold_gate.__file__) as source:
        assert "return paying() or" in source.read()


def test_a_refusal_while_held_by_a_leftover_says_to_wait(monkeypatch, tmp_path):
    # A hold whose payment is gone (the bridge gives it back within minutes): not only « use confirm_payment ».
    use_flag(monkeypatch, tmp_path, b"card 0123abcd")
    text = json.loads(hold_gate.refusal({"id": 5}))["result"]["content"][0]["text"]
    assert "confirm_payment" in text and "wait a few minutes" in text


def test_calls_let_through_are_counted_until_answered(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, None)
    calls = hold_gate.Calls(str(tmp_path / "calls"))
    count = tmp_path / "calls" / str(os.getpid())
    assert count.read_text() == "0"
    calls.start(7)
    calls.start("a")
    assert count.read_text() == "2"
    hold_gate.answered(calls, b'{"jsonrpc": "2.0", "id": 7, "result": {"content": []}}\n')
    hold_gate.answered(calls, b'{"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}\n')
    hold_gate.answered(calls, b'{"jsonrpc": "2.0", "id": "a", "method": "sampling/createMessage"}\n')  # a request
    assert count.read_text() == "1" and calls.count() == 1
    calls.end("a")
    assert count.read_text() == "0"
    # An answer lost: no longer counted after LIMIT, so it can't stop every payment.
    calls.running[json.dumps(9)] = time.time() - hold_gate.LIMIT - 1
    calls.start(10)
    assert count.read_text() == "1"
    calls.close()
    assert not count.exists()


def test_a_call_running_when_the_hold_starts_stays_counted_and_new_ones_are_refused(monkeypatch, tmp_path):
    """The gate between Hermes and a fake server that answers a call only when told: a call let through before the
    hold stays counted while it runs; one sent during the hold is refused at once and never counted."""
    import io
    import threading

    flag = use_flag(monkeypatch, tmp_path, None)
    calls = hold_gate.Calls(str(tmp_path / "calls"))
    server = tmp_path / "server.py"
    server.write_text(
        "import sys, json\n"
        "for line in sys.stdin:\n"
        "    m = json.loads(line)\n"
        "    if m.get('method') == 'answer':\n"
        "        print(json.dumps({'jsonrpc': '2.0', 'id': m['params']['id'], 'result': {}}), flush=True)\n")
    reader_in, writer_in = os.pipe()
    reader_out, writer_out = os.pipe()
    monkeypatch.setattr(sys, "stdin", io.TextIOWrapper(os.fdopen(reader_in, "rb")))
    monkeypatch.setattr(sys, "stdout", io.TextIOWrapper(os.fdopen(writer_out, "wb")))
    done = []
    gate = threading.Thread(target=lambda: done.append(hold_gate.main([sys.executable, str(server)], calls)),
                            daemon=True)
    gate.start()
    send = os.fdopen(writer_in, "wb", buffering=0)
    answers = os.fdopen(reader_out, "rb")

    def line(message):
        send.write((json.dumps(message) + "\n").encode())

    def counted(n):
        deadline = time.time() + 5
        while calls.count() != n and time.time() < deadline:
            time.sleep(0.02)
        return calls.count()

    line({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "wait_for_element"}})
    assert counted(1) == 1
    flag.write_bytes(b"card 0123abcd")                       # the hold starts while call 1 runs
    line({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "execute_script"}})
    refused = json.loads(answers.readline())
    assert refused["id"] == 2 and "card payment" in refused["result"]["content"][0]["text"]
    assert calls.count() == 1                                 # only call 1, still running
    line({"jsonrpc": "2.0", "method": "answer", "params": {"id": 1}})
    assert json.loads(answers.readline())["id"] == 1
    assert counted(0) == 0
    flag.unlink()
    line({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {}})
    assert counted(1) == 1
    line({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 3}})
    assert counted(0) == 0                                    # cancelled: the server drops it without an answer
    send.close()
    gate.join(10)
    assert done == [0] and not (tmp_path / "calls" / str(os.getpid())).exists()


def test_while_held_no_resource_is_read_nor_listed(monkeypatch, tmp_path):
    """Fix review 11/11, R5: the server serves the page's recorded traffic as the resource browser://<id>/network
    (and its console, state, cookies). While held, only the handshake, ping and the tool and prompt lists reach it."""
    import io
    import threading

    flag = use_flag(monkeypatch, tmp_path, b"card 0123abcd")
    calls = hold_gate.Calls(str(tmp_path / "calls"))
    server = tmp_path / "server.py"
    server.write_text(
        "import sys, json\n"
        "for line in sys.stdin:\n"
        "    m = json.loads(line)\n"
        "    if 'id' in m:\n"
        "        print(json.dumps({'jsonrpc': '2.0', 'id': m['id'], 'result': {'served': m['method']}}), flush=True)\n")
    reader_in, writer_in = os.pipe()
    reader_out, writer_out = os.pipe()
    monkeypatch.setattr(sys, "stdin", io.TextIOWrapper(os.fdopen(reader_in, "rb")))
    monkeypatch.setattr(sys, "stdout", io.TextIOWrapper(os.fdopen(writer_out, "wb")))
    gate = threading.Thread(target=lambda: hold_gate.main([sys.executable, str(server)], calls), daemon=True)
    gate.start()
    send = os.fdopen(writer_in, "wb", buffering=0)
    answers = os.fdopen(reader_out, "rb")

    def ask(n, method, params=None):
        send.write((json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params or {}}) + "\n").encode())
        return json.loads(answers.readline())

    for n, method in enumerate(["resources/read", "resources/list", "resources/templates/list", "prompts/get"], 1):
        answer = ask(n, method, {"uri": "browser://abc/network"})
        assert answer["id"] == n and "card payment" in answer["error"]["message"] and "result" not in answer, method
    for n, method in enumerate(["initialize", "ping", "tools/list", "prompts/list"], 10):
        assert ask(n, method)["result"] == {"served": method}
    flag.unlink()   # the hold over: resources pass again (the bridge's relay never offers them, browser_mcp.rs)
    assert ask(20, "resources/read", {"uri": "browser://abc/network"})["result"] == {"served": "resources/read"}
    send.close()
    gate.join(10)


def test_a_gate_that_cannot_count_refuses_tool_calls(monkeypatch, tmp_path):
    # Fix review 11/11, R3: a gate that can't write its count would leave the bridge reading none, and typing a card
    # while one of its calls runs. Its tool calls are refused instead (the person restarts the browser).
    use_flag(monkeypatch, tmp_path, None)
    taken = tmp_path / "not-a-folder"
    taken.write_text("")
    calls = hold_gate.Calls(str(taken))
    assert calls.counting is False and calls.path is None
    refused = json.loads(hold_gate.refusal({"id": 4}, hold_gate.UNCOUNTED))
    assert refused["id"] == 4 and refused["result"]["isError"] is True
    assert "restarted" in refused["result"]["content"][0]["text"]
    # One that could, until its folder could no longer be written: the same.
    counted = hold_gate.Calls(str(tmp_path / "calls"))
    assert counted.counting is True
    (tmp_path / "calls").chmod(0o500)
    try:
        counted.start(1)
        assert counted.counting is (os.geteuid() == 0)   # root writes anyway
    finally:
        (tmp_path / "calls").chmod(0o700)


WEDGED = """
import os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
for line in sys.stdin:   # a tools/call it never answers (a handler stuck on a frozen DevTools link)
    pass
time.sleep(3600)          # stdin closed: still busy with that call, so it never ends by itself
"""


def test_review6_a_wedged_server_ends_with_its_gate_when_the_bridge_lets_go(tmp_path):
    """Review 6 (9759e45d4, lab on the places' image): on a call's overrun bip-browser-mcp closes the server's stdin,
    waits 5 s, then kills `podman exec` — the client only: inside the container this gate and its stuck server.py
    stay (lab: pids 48/49 still there after the restart and after the proxy ended), the gate's count file at 1 for
    ever (`Calls._write` never runs again, so `LIMIT` never lapses it). Card payments in that browser then never
    start (card_pay.rs `calls_check_command`: a live gate with a count ≠ 0 = a call running), each overrun leaks a
    server, and the stuck call can still act in Chrome later (a frozen Chrome thawed). Expected: stdin closed, the
    gate ends its server within a few seconds (terminate, then kill) and its count file goes."""
    import subprocess
    folder = tmp_path / "calls"
    pid_file = tmp_path / "server.pid"
    (tmp_path / "wedged.py").write_text(WEDGED)
    here = os.path.dirname(os.path.abspath(__file__))
    runner = ("import sys; sys.path.insert(0, sys.argv[1]); import hold_gate; hold_gate.HOLD = sys.argv[2]; "
              "sys.exit(hold_gate.main(sys.argv[4:], hold_gate.Calls(sys.argv[3])))")
    gate = subprocess.Popen([sys.executable, "-c", runner, here, str(tmp_path / "no-hold"), str(folder),
                             sys.executable, str(tmp_path / "wedged.py"), str(pid_file)],
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    call = {"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "navigate", "arguments": {}}}
    gate.stdin.write((json.dumps(call) + "\n").encode())
    gate.stdin.flush()
    deadline = time.time() + 10
    while not (pid_file.exists() and pid_file.read_text()) and time.time() < deadline:
        time.sleep(0.05)
    server = int(pid_file.read_text())
    count = folder / str(gate.pid)
    deadline = time.time() + 10
    while count.read_text() != "1" and time.time() < deadline:
        time.sleep(0.05)
    assert count.read_text() == "1"
    gate.stdin.close()   # what stop_child does first
    try:
        gate.wait(timeout=15)
        ended = True
    except subprocess.TimeoutExpired:
        ended = False
    alive = os.path.exists(f"/proc/{server}") or _alive(server)
    if not ended:
        gate.kill()
    try:
        os.kill(server, 9)
    except OSError:
        pass
    assert ended, "the gate waits for ever on its stuck server once the bridge closed its stdin"
    assert not alive, "the stuck server outlives its gate"
    assert not count.exists(), "the gate's count (1) stays: card payments in this browser never start"


def _alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False
