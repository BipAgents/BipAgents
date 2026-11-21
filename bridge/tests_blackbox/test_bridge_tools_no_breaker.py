"""bridge-tools-no-breaker (14/11). Hermes' MCP circuit breaker (v0.23.1, tools/mcp_tool_handlers.py
`_render_call_tool_result`, `_record_call_outcome`, `_check_circuit_breaker`; tools/mcp_tool.py
`_CIRCUIT_BREAKER_THRESHOLD, _CIRCUIT_BREAKER_COOLDOWN_SEC = 3, 60.0`) counts every tool answer that comes back with
`isError` (Hermes hands it to the model as `{"error": …}`): three in a row and every tool of that server is refused
for ~60 s (« MCP server 'bipagents' rejected the last 3 calls… Paused for ~58s »), re-armed by the next failure; any
other answer resets the count. On the 14/11 sign-up bench (DeepSeek, 2 runs of 9) the Carnet refused a note three
times (« Not saved: ID numbers are never kept »: a date read as an IBAN) and all the bridge's tools were paused.

Here: the bridge's answers through a client that keeps Hermes' rule (`HermesBreaker`), and through Hermes' own MCP
code when given (`HERMES_SRC`, `MCP_SDK_PYTHON`: bbkit/hermes_breaker_client.py)."""
import json
import os
import subprocess
import time
from pathlib import Path

import pytest

from bbkit.mcp import TOKEN, give_token, not_done, rpc, text_of

#: Three notes holding a real ID number: refused, whatever the agent changes around it.
PASSPORT_NOTES = (
    {"page": "Camille Testeur", "kind": "person", "facts": ["Passeport : 12AB34567"]},
    {"page": "Camille Testeur", "kind": "person", "facts": ["Passeport n° 12 AB 34567, valable jusqu'au 2031-03-12"]},
    {"page": "Camille Testeur", "kind": "person", "facts": ["Compte client créé", "Numéro de passeport : 12AB34567"]},
)


class HermesBreaker:
    """One agent's Hermes calling the bridge's `bipagents` server, its breaker as v0.23.1 keeps it: an `isError`
    answer (or no answer: an HTTP or JSON-RPC error) is a strike, anything else resets the count; at 3 the server is
    paused and its calls never reach the bridge (`None`)."""

    THRESHOLD = 3

    def __init__(self, bridge, path="/v1/agent-tools/mcp", token=TOKEN):
        self.bridge, self.path, self.token = bridge, path, token
        self.strikes = 0

    @property
    def paused(self):
        return self.strikes >= self.THRESHOLD

    def call(self, name, arguments):
        if self.paused:
            return None
        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
        resp = self.bridge.post(self.path, json=body, timeout=120,
                                headers={"Authorization": f"Bearer {self.token}", "Accept": "application/json"})
        answer = resp.json() if resp.status_code == 200 else {}
        result = answer.get("result")
        if result is None or result.get("isError"):
            self.strikes += 1
        else:
            self.strikes = 0
        return result


def test_three_refused_notes_in_a_row_then_another_bridge_tool_answers_at_once(bridge):
    give_token(bridge)
    hermes = HermesBreaker(bridge)
    for note in PASSPORT_NOTES:
        answer = hermes.call("notebook_write", note)
        assert answer is not None and not_done(answer), answer
        assert text_of(answer).startswith("Not saved: ID numbers are never kept, and fact "), answer
    assert not hermes.paused and hermes.strikes == 0
    started = time.monotonic()
    found = hermes.call("notebook_search", {"words": "Camille"})
    assert found is not None and found["isError"] is False, "paused: the next bridge tool was refused"
    pages = hermes.call("list_pages", {})
    assert pages is not None and pages["isError"] is False
    assert time.monotonic() - started < 10
    # nothing of the passport reached the Carnet
    carnet = bridge.state_dir / "notebook"
    assert not any("12AB34567" in p.read_text() or "34567" in p.read_text() for p in carnet.rglob("*.md"))


def test_the_benchs_notes_are_kept(bridge):
    """DeepSeek's notes after the account was made (bench 14/11, a fictional person): an e-mail as the login, dates."""
    give_token(bridge)
    hermes = HermesBreaker(bridge)
    saved = hermes.call("notebook_write", {
        "page": "Pharmacie des Essais", "kind": "subject", "section": "status",
        "facts": ["Pharmacie des Essais : compte client créé le 14/11/2026 (Camille Testeur, "
                  "camille.testeur@example.org) — mot de passe dans le coffre",
                  "Identifiant : camille.testeur@example.org — mot de passe gardé dans le coffre"],
        "summary": "Compte client créé le 14 novembre 2026 sur la Pharmacie des Essais, pour commander en ligne."})
    assert saved["isError"] is False and text_of(saved).startswith("Saved: Pharmacie des Essais"), saved
    page = next((bridge.state_dir / "notebook").rglob("Pharmacie des Essais.md")).read_text()
    assert "camille.testeur@example.org" in page and "14 novembre 2026" in page


def test_refusals_and_policy_answers_are_plain_on_every_server_of_the_bridge(bridge):
    give_token(bridge)
    for name, arguments, says in [
        ("weather", {"place": "Lyon"}, "Weather"),                         # off for this agent
        ("no_such_tool", {}, "unknown tool"),                              # not a tool of the bridge
        ("notebook_read", {"page": "Personne Inconnue"}, "No such page"),  # not found
        ("notebook_write", {"page": "Marc Dupont", "facts": ["x"], "private": True}, "health"),  # health filter
        ("read_page", {"page": "nope"}, "unknown page id"),
    ]:
        answer = rpc(bridge, "tools/call", {"name": name, "arguments": arguments}).json()["result"]
        assert answer is not None and not_done(answer) and says in text_of(answer), (name, answer)
    # the other servers of the bridge (the same token): the files' and the vault's
    for path, name, arguments in [("/v1/agent-tools/files", "shared_files_get", {"agent": "personne", "path": "x.txt"}),
                                  ("/v1/agent-tools/vault", "vault_confirm_login", {"site": "https://shop.example"})]:
        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
        answer = bridge.post(path, json=body, headers={"Authorization": f"Bearer {TOKEN}"}).json()["result"]
        assert answer.get("isError") is False, (path, name, answer)


@pytest.mark.skipif(os.geteuid() == 0, reason="root writes into a read-only folder")
def test_a_real_failure_keeps_is_error_says_dont_retry_and_never_three_in_a_row(bridge):
    give_token(bridge)
    hermes = HermesBreaker(bridge)
    assert hermes.call("notebook_write", {"page": "Léa Martin", "kind": "person", "facts": ["Aime le jazz"]})["isError"] is False
    carnet = bridge.state_dir / "notebook"
    folders = [carnet] + [p for p in carnet.rglob("*") if p.is_dir()]
    files = [p for p in carnet.rglob("*") if p.is_file()]
    for p in files:
        p.chmod(0o444)
    for p in folders:
        p.chmod(0o555)
    try:
        answers = [hermes.call("notebook_write", {"page": f"Paul {n}", "kind": "person", "facts": ["Voisin"]})
                   for n in range(3)]
    finally:
        for p in folders:
            p.chmod(0o755)
        for p in files:
            p.chmod(0o644)
    assert all(a is not None for a in answers), "paused by the bridge's own failures"
    assert [a["isError"] for a in answers] == [True, True, False], answers
    for a in answers:
        assert "Don't retry" in text_of(a) and text_of(a).startswith("Not done: "), a
    assert not hermes.paused
    assert hermes.call("notebook_search", {"words": "Léa"})["isError"] is False


# --- Hermes' own MCP code -----------------------------------------------------------------------------------------

PYTHON = os.environ.get("MCP_SDK_PYTHON")
HERMES_SRC = os.environ.get("HERMES_SRC")
CLIENT = str(Path(__file__).parent / "bbkit" / "hermes_breaker_client.py")
hermes = pytest.mark.skipif(not (PYTHON and HERMES_SRC), reason="no Hermes checkout and Python given (HERMES_SRC, MCP_SDK_PYTHON)")


def through_hermes(tmp_path, url, token, calls, server="bipagents"):
    """Each call through Hermes' own MCP client (bbkit/hermes_breaker_client.py): what the model gets (`result`),
    `failed` (its `tool.completed` error) and its breaker's count for that server after it (`strikes`). A callable
    among `calls` is done between two calls (the person's switch…)."""
    home = tmp_path / "hermes-home"
    home.mkdir(exist_ok=True)
    client = subprocess.Popen([PYTHON, "-I", CLIENT, url, token, server], env={**os.environ, "HERMES_HOME": str(home)},
                              text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        ready = client.stdout.readline()
        assert ready.startswith("READY ") and int(ready.split()[1]) > 0, ready
        out = []
        for step in calls:
            if callable(step):
                step()
                continue
            name, arguments = step
            client.stdin.write(json.dumps({"name": name, "arguments": arguments}) + "\n")
            client.stdin.flush()
            line = client.stdout.readline()
            assert line.startswith("RESULT "), line
            out.append(json.loads(line[len("RESULT "):]))
        return out
    finally:
        client.kill()


@hermes
def test_hermes_own_breaker_never_pauses_the_bridge_after_three_refused_notes(bridge, tmp_path):
    give_token(bridge)
    calls = [("notebook_write", note) for note in PASSPORT_NOTES]
    calls += [("notebook_search", {"words": "Camille"}), ("list_pages", {})]
    results = through_hermes(tmp_path, bridge.url + "/v1/agent-tools/mcp", TOKEN, calls)
    answers = [json.loads(r["result"]) for r in results]
    assert not any("error" in a for a in answers), f"Hermes counted strikes, paused the bridge: {answers}"
    assert all(a["result"].startswith("Not saved: ID numbers are never kept, and fact ") for a in answers[:3]), answers
    assert [r["strikes"] for r in results] == [0] * 5, results
