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
        assert agent.vault_state(str(home)) == {}
        _Bridge.answer = {"ok": True, "vault": "locked", "vault_reason": "x" * 500}
        assert agent.vault_state(str(home)) == {"vault": "locked", "vault_reason": "x" * 120}
    finally:
        server.shutdown()
    # the bridge down: nothing said about the vault (the door's own check says it is down)
    assert agent.vault_state(str(home)) == {}
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
