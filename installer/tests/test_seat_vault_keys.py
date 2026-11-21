"""The seat agent and the vault's sealed keys (design note « Clé du coffre », lot 3): the guichet's `vault_keys` order
to a place already installed (the server hardened first, then the place enrolled exactly once, whatever happens), and
the beat saying, for each place, whether its vault is sealed and open, closed (and why), or its sealing failed."""
import base64
import http.server
import json
import os
import threading
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SETTINGS = {"url": "https://keys.example", "grant": '{"v":2}', "signature": "c2ln", "token": "k" * 43}


def _seat_agent():
    import importlib.util
    spec = importlib.util.spec_from_file_location("seat_agent_vault", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _signed(key, body):
    text = json.dumps(body, sort_keys=True, separators=(",", ":"))
    return {"id": body["order"], "body": text, "signature": base64.b64encode(key.sign(text.encode())).decode()}


def test_a_vault_keys_order_carries_a_key_service_of_the_place_s_own():
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    agent = _seat_agent()
    key = Ed25519PrivateKey.generate()
    now = time.time()
    body = {"order": 21, "server": "vps-1", "seat": 2, "action": "vault_keys", "expires": int(now + 3600),
            "vault_keys": SETTINGS}
    assert agent.accepted(_signed(key, body), "vps-1", set(), now) == body
    for wrong in ({**SETTINGS, "url": "http://keys.example"}, {**SETTINGS, "token": "short"},
                  {**SETTINGS, "grant": ""}, {**SETTINGS, "signature": 7}, "x", None,
                  {**SETTINGS, "token": "k" * 42 + "\n"}):
        assert agent.accepted(_signed(key, {**body, "vault_keys": wrong}), "vps-1", set(), now) is None


@pytest.fixture
def place(tmp_path, monkeypatch):
    agent = _seat_agent()
    home = tmp_path / "home" / "bip2"
    (home / ".local" / "state" / "bipagents").mkdir(parents=True)
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "vault-keys-tried"))
    monkeypatch.setattr(agent, "HARDENED", str(tmp_path / "space-hardened"))
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": "bip2"})
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: entry)
    return agent, home, tmp_path


def test_the_order_hardens_the_server_first_and_waits_for_the_next_beat(place):
    agent, home, tmp = place
    hardened, enrolled = [], []
    harden = lambda: hardened.append(1) or (Path(agent.HARDENED).write_text("now") or True)
    run_as = lambda entry, args, timeout=900: enrolled.append(args) or {"state": "sealed"}
    assert agent.vault_keys_order(2, SETTINGS, 21, harden=harden, run_as=run_as) is False   # not done: next round
    assert hardened == [1] and enrolled == []
    # the next round: the beat said space_hardened; the place enrolled, once
    assert agent.vault_keys_order(2, SETTINGS, 21, harden=harden, run_as=run_as) is True
    assert hardened == [1] and len(enrolled) == 1
    args = enrolled[0]
    assert args[:2] == ["vault-keys", "enable"] and "--settings-file" in args
    assert SETTINGS["token"] not in " ".join(args), "never on a command line"
    settings_file = Path(args[args.index("--settings-file") + 1])
    assert not settings_file.exists(), "the settings never stay on the disk"


def test_the_order_never_enrolls_twice_whatever_happened(place):
    agent, home, tmp = place
    Path(agent.HARDENED).write_text("done")
    enrolled = []

    def run_as(entry, args, timeout=900):
        enrolled.append(args)
        written = Path(args[args.index("--settings-file") + 1])
        assert json.loads(written.read_text()) == SETTINGS
        assert oct(written.stat().st_mode & 0o777) == "0o600"
        raise agent.SeatError("vault-keys enable: the key service refused")

    assert agent.vault_keys_order(2, SETTINGS, 21, run_as=run_as) is True   # done: told to the guichet
    assert agent.vault_keys_order(2, SETTINGS, 21, run_as=run_as) is True   # the same order again: nothing run
    assert len(enrolled) == 1
    assert "21" in Path(agent.VAULT_TRIED).read_text().split()
    # what the beat tells the guichet: the place's own note (the installer writes it), else the agent's
    assert "refused" in agent.vault_failed(str(home))


def test_a_hardening_that_fails_ends_the_order_and_says_so(place):
    agent, home, tmp = place
    enrolled = []
    assert agent.vault_keys_order(2, SETTINGS, 22, harden=lambda: False,
                                  run_as=lambda e, a, timeout=900: enrolled.append(a)) is True
    assert enrolled == [] and "hardening" in agent.vault_failed(str(home))


class _Bridge(http.server.BaseHTTPRequestHandler):
    answer = {}

    def do_GET(self):
        body = json.dumps(self.answer).encode()
        self.send_response(200 if self.path == "/health" else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def test_the_beat_says_whether_each_vault_is_open_closed_or_failed(tmp_path):
    agent = _seat_agent()
    server = http.server.HTTPServer(("127.0.0.1", 0), _Bridge)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        home = tmp_path / "bip"
        data = home / ".local" / "share" / "bipagents"
        data.mkdir(parents=True)
        (home / ".config" / "bipagents").mkdir(parents=True)
        (home / ".config" / "bipagents" / "bridge.toml").write_text(
            f'bridge_key = "k"\nhost = "127.0.0.1"\nport = {server.server_port}\ndata_dir = "{data}"\n')
        _Bridge.answer = {"ok": True, "vault": "locked", "vault_reason": "unreachable"}
        assert agent.vault_state(str(home)) == {}, "not sealed: the bridge isn't even asked"
        (data / "keys.sealed.json").write_text("{}")
        assert agent.vault_state(str(home)) == {"vault": "locked", "vault_reason": "unreachable"}
        _Bridge.answer = {"ok": True, "vault": "open"}
        assert agent.vault_state(str(home)) == {"vault": "open"}
        _Bridge.answer = {"ok": True, "vault": "<script>", "vault_reason": "x" * 500}
        assert agent.vault_state(str(home)) == {"vault": "sealed"}, "an odd answer: sealed, nothing of it repeated"
        _Bridge.answer = {"ok": True, "vault": "locked", "vault_reason": "x" * 500}
        assert agent.vault_state(str(home)) == {"vault": "locked", "vault_reason": "x" * 120}
    finally:
        server.shutdown()
    # the bridge down: sealed all the same (second review, N3: never taken for a place whose sealing failed)
    assert agent.vault_state(str(home)) == {"vault": "sealed"}
    state = home / ".local" / "state" / "bipagents" / "vault-keys.json"
    state.parent.mkdir(parents=True)
    assert agent.vault_failed(str(home)) is None
    state.write_text(json.dumps({"state": "failed", "error": "the key service refused: HTTP 403", "at": 1.0}))
    assert agent.vault_failed(str(home)) == "the key service refused: HTTP 403"
    state.write_text(json.dumps({"state": "sealed", "at": 2.0}))
    assert agent.vault_failed(str(home)) is None


def test_health_adds_the_vault_only_for_places_that_have_one(tmp_path, monkeypatch):
    agent = _seat_agent()
    homes = {"bip": tmp_path / "bip", "bip2": tmp_path / "bip2"}
    for home in homes.values():
        home.mkdir()

    def entry(name):
        if name not in homes:
            raise KeyError(name)
        return type("Entry", (), {"pw_dir": str(homes[name]), "pw_uid": os.getuid()})

    monkeypatch.setattr(agent.pwd, "getpwnam", entry)
    monkeypatch.setattr(agent, "door_open", lambda port: True)
    monkeypatch.setattr(agent, "running", lambda uid, words: True)
    monkeypatch.setattr(agent, "vault_state", lambda home: {"vault": "open"} if home.endswith("bip2") else {})
    monkeypatch.setattr(agent, "vault_failed", lambda home: "hardening failed" if home.endswith("/bip") else None)
    seats = agent.health()
    assert "vault" not in seats["1"] and seats["1"]["vault_failed"] == "hardening failed"
    assert seats["2"]["vault"] == "open" and "vault_failed" not in seats["2"]


def test_every_beat_says_which_vault_orders_this_seat_agent_does(tmp_path):
    # F2 (final review of 20/11): the guichet orders vault_keys only to a server whose beat says a seat agent that notes
    # each order's outcome and cites it in its beats (870bc5931, `VAULT_AGENT` 3); an older one says nothing, and a
    # wave back to one stops saying it
    agent = _seat_agent()
    meminfo = tmp_path / "meminfo"
    meminfo.write_text("MemTotal:        8000000 kB\nMemAvailable:    3000000 kB\n")
    numbers = agent.vitals(str(meminfo), str(tmp_path), str(tmp_path / "up"), str(tmp_path / "hardened"))
    assert agent.VAULT_AGENT == 3
    assert numbers["vault_agent"] == 3
    assert json.loads(json.dumps(numbers))["vault_agent"] == 3


def _sealed_home(root, name, url, sealed=True):
    home = root / name
    data = home / ".local" / "share" / "hermes-bridge"
    data.mkdir(parents=True)
    (home / ".config" / "bipagents").mkdir(parents=True)
    (home / ".config" / "bipagents" / "bridge.toml").write_text(f'bridge_key = "k"\ndata_dir = "{data}"\n')
    if sealed:
        (data / "keys.sealed.json").write_text("{}")
    service = home / ".local" / "share" / "bipagents" / "space" / "keyservice.json"
    service.parent.mkdir(parents=True)
    service.write_text(json.dumps({"url": url, "token": "t" * 43}))
    return home


def test_a_planned_reboot_waits_while_the_key_service_of_a_sealed_place_does_not_answer(tmp_path, monkeypatch):
    # Alex 21/11: a sealed place is never restarted at night while the key service doesn't answer (its vault would
    # stay closed until it answers again): the night's reboot (unattended-upgrades' `shutdown -r 00:xx`) is called
    # off, the reboot still required, so the next night tries again
    agent = _seat_agent()
    homes = {"bip": _sealed_home(tmp_path, "bip", "https://keys.example"),
             "bip2": _sealed_home(tmp_path, "bip2", "https://keys.example", sealed=False)}

    def entry(name):
        if name not in homes:
            raise KeyError(name)
        return type("Entry", (), {"pw_dir": str(homes[name]), "pw_uid": os.getuid()})

    monkeypatch.setattr(agent.pwd, "getpwnam", entry)
    assert agent.sealed_key_services() == {"https://keys.example"}, "the sealed places' only"
    scheduled = tmp_path / "scheduled"
    asked, cancelled = [], []
    answers = {"https://keys.example": False}

    def answer(url):
        asked.append(url)
        return answers[url]

    def hold(**kw):
        return agent.hold_reboot(scheduled=str(scheduled), answers=answer, cancel=lambda: cancelled.append(1), **kw)

    # nothing scheduled: nobody asked
    assert hold() is False and asked == [] and cancelled == []
    # a reboot scheduled, the key service silent: called off
    scheduled.write_text("USEC=1700000000000000\nWARN_WALL=1\nMODE=reboot\n")
    assert hold() is True and cancelled == [1]
    # it answers: the reboot goes
    answers["https://keys.example"] = True
    assert hold() is False and cancelled == [1]
    # a power-off (Kerfy's own, by hand) is never touched
    answers["https://keys.example"] = False
    scheduled.write_text("USEC=1700000000000000\nMODE=poweroff\n")
    assert hold() is False and cancelled == [1]
    # no sealed place on the server: nothing asked, nothing held
    scheduled.write_text("USEC=1700000000000000\nMODE=reboot\n")
    homes["bip"] = _sealed_home(tmp_path, "bip3", "https://keys.example", sealed=False)
    asked.clear()
    assert hold() is False and asked == [] and cancelled == [1]


def test_the_key_service_answers_only_with_its_health(tmp_path):
    agent = _seat_agent()

    class Health(http.server.BaseHTTPRequestHandler):
        status = 200

        def do_GET(self):
            self.send_response(Health.status if self.path == "/health" else 404)
            self.end_headers()
            self.wfile.write(b'{"ok":true}')

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Health)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = "http://127.0.0.1:%d" % server.server_port
        assert agent.key_service_answers(url) is True
        Health.status = 503
        assert agent.key_service_answers(url) is False
    finally:
        server.shutdown()
    assert agent.key_service_answers(url, timeout=1) is False, "nobody listening"


def test_a_vault_keys_order_is_done_only_in_the_server_s_night(tmp_path, monkeypatch):
    # Alex 21/11: sealing restarts the place's bridge (and the first time hardens the server): only at night, like the
    # night's reboot. The order sent in the evening (or any time) waits, served again each round, valid two days.
    agent = _seat_agent()
    at = lambda hour, minute=0: time.struct_time((2026, 11, 21, hour, minute, 0, 5, 325, 0))
    monkeypatch.setattr(agent, "VAULT_ANYTIME", str(tmp_path / "vault-keys-anytime"))
    assert [h for h in range(24) if agent.vault_night(clock=lambda h=h: at(h))] == [1, 2, 3, 4]
    assert agent.vault_night(clock=lambda: at(4, 59)) and not agent.vault_night(clock=lambda: at(5, 0))
    assert not agent.vault_night(clock=lambda: at(0, 30)), "never during the night's reboot (00:00-00:44)"
    # a lab says « any time » by a file of root's
    (tmp_path / "vault-keys-anytime").write_text("")
    assert agent.vault_night(clock=lambda: at(15))


def test_the_round_leaves_a_vault_keys_order_for_the_night(tmp_path, monkeypatch):
    agent = _seat_agent()
    seen = []
    monkeypatch.setattr(agent, "vault_keys_order", lambda *a, **k: seen.append(a) or True)
    body = {"action": "vault_keys", "seat": 2, "vault_keys": SETTINGS}
    monkeypatch.setattr(agent, "vault_night", lambda clock=None: False)
    assert agent.vault_order_round(body, 21) is False and seen == [], "held: neither done nor tried"
    monkeypatch.setattr(agent, "vault_night", lambda clock=None: True)
    assert agent.vault_order_round(body, 21) is True and len(seen) == 1
