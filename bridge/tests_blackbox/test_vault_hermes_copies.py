"""The agents' Hermes copies of the vault, in memory (design note « Clé du coffre », lot 2; § 6 point 2), black-box
with the real Hermes v0.23.1 and BipAgents' patches.

A place enrolled for sealed keys (`bipbridge vault-keys enable`, lot 1) keeps no Hermes copy on its disk either: each
agent's `<HERMES_HOME>/vault` is a link into the memory folder (`$XDG_RUNTIME_DIR/bipagents/…`, a tmpfs), its copy
rebuilt there from the bridge's master copy once the vault is open, in one Python run per agent; the bridge's handles
follow; the copies left on the disk go only once that rebuild is checked. Hermes itself reads and fills through the
link (`browser_vault_fill`, patch 0013's scrub). While the vault is closed (a server restart: the tmpfs empty), no
copy is there to read, and the bridge's own vault tools say the lot-1 words. A place not enrolled keeps today's
layout: its copies on the disk, as Hermes writes them.

Needs: Linux (a tmpfs for the memory folder), a Hermes v0.23.1 checkout with the patches (`HERMES_SRC`) and a Python
that runs its vault code (`HERMES_PYTHON`: cryptography, ruamel.yaml, pydantic… as Hermes' own). Skipped without them.
The bridge runs the agents' Python through a wrapper that notes each run (its operation, never its secrets) and can be
told to fail or to answer short, as a broken Hermes would.
"""
import base64
import json
import os
import shutil
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from bbkit.common import AUTH
from bbkit.mcp import give_token

HERMES_SRC = os.environ.get("HERMES_SRC", "")
HERMES_PYTHON = os.environ.get("HERMES_PYTHON", "")

pytestmark = pytest.mark.skipif(
    not (HERMES_SRC and HERMES_PYTHON and sys.platform.startswith("linux")),
    reason="needs Linux, a Hermes v0.23.1 checkout with the patches (HERMES_SRC) and its Python (HERMES_PYTHON)")

TOKEN = "vie-agent-token-0123456789abcdef"
LOGINS = {
    "sncf": {"label": "SNCF", "site": "sncf-connect.com", "identifier": "alex@example.org",
             "password": "Tr41n-S3cret-sncf!", "agents": ["vie", "budget"]},
    "impots": {"label": "Impôts", "site": "impots.gouv.fr", "identifier": "1234567890123",
               "password": "Imp0ts-S3cret-dgfip#", "agents": ["vie"]},
}
LATER = {"label": "Ameli", "site": "ameli.fr", "identifier": "alex@example.org", "password": "Am3li-S3cret-cpam%",
         "agents": ["vie"]}
AGENTS = ("vie", "budget", "coach")
DONE = "vault: the agents' Hermes copies are in memory"

WRAPPER = r'''
import json, os, subprocess, sys
LOG, REAL, RULES = %r, %r, %r
data = sys.stdin.read()
req = json.loads(data)
agent = (req.get("home") or "").rstrip("/").rsplit("/", 1)[-1]
op = req.get("op") or "list"
with open(LOG, "a") as f:
    f.write(json.dumps({"op": op, "agent": agent, "items": len(req.get("items") or []),
                        "vault": req.get("vault")}) + "\n")
rules = json.load(open(RULES)) if os.path.exists(RULES) else {}
rule = rules.get(agent + ":" + op)
if rule == "fail":
    sys.stderr.write("ModuleNotFoundError: No module named 'cryptography' (a broken Hermes, test)\n")
    sys.exit(1)
done = subprocess.run([REAL] + sys.argv[1:], input=data, capture_output=True, text=True)
out = done.stdout
if rule == "short" and done.returncode == 0:
    answer = json.loads(out.strip().splitlines()[-1])
    answer["items"] = answer.get("items", [])[1:]
    out = json.dumps(answer) + "\n"
sys.stdout.write(out)
sys.stderr.write(done.stderr)
sys.exit(done.returncode)
'''


def b64url_decode(text: str) -> bytes:
    text = text.strip()
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


class KeyService:
    """keys-rs as the bridge asks it: POST /v1/share with the place's token; « down » answers 503."""

    def __init__(self):
        self.share = os.urandom(32)
        self.mode = "ok"
        self.asked = 0
        service = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                length = int(self.headers.get("content-length") or 0)
                self.rfile.read(length)
                service.asked += 1
                if self.path != "/v1/share" or self.headers.get("authorization") != f"Bearer {'t' * 40}":
                    code, body = 403, {"detail": "unknown place"}
                elif service.mode == "down":
                    code, body = 503, {"detail": "down"}
                else:
                    code, body = 200, {"share": base64.urlsafe_b64encode(service.share).decode().rstrip("=")}
                data = json.dumps(body).encode()
                self.send_response(code)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()


def memory_kind(path: Path) -> str:
    done = subprocess.run(["stat", "-f", "-c", "%T", str(path)], capture_output=True, text=True)
    return done.stdout.strip()


@pytest.fixture
def runtime():
    """A folder of this account's own in memory (tmpfs), standing for $XDG_RUNTIME_DIR."""
    for base in (Path(f"/run/user/{os.getuid()}"), Path("/dev/shm")):
        if base.is_dir() and os.access(base, os.W_OK) and memory_kind(base) in ("tmpfs", "ramfs"):
            folder = base / f"bb-copies-{uuid.uuid4().hex[:10]}"
            folder.mkdir(mode=0o700)
            os.chmod(folder, 0o700)
            yield folder
            shutil.rmtree(folder, ignore_errors=True)
            return
    pytest.skip("no folder in memory (tmpfs) here")


@pytest.fixture
def keys():
    service = KeyService()
    yield service
    service.close()


class Place:
    """A bridge whose agents Vie, Budget and Coach each have their own space and the real Hermes' Python."""

    def __init__(self, bridges, fakes, runtime: Path, keys: KeyService):
        self.bridge = bridge = bridges.new()
        self.runtime, self.keys = runtime, keys
        self.runs = bridge.root / "python-runs.jsonl"
        self.rules = bridge.root / "python-rules.json"
        wrapper = bridge.root / "hermes-python.py"
        wrapper.write_text(WRAPPER % (str(self.runs), HERMES_PYTHON, str(self.rules)))
        self.spaces = bridge.data / "spaces"
        self.space_dir = bridge.root / "space"
        self.hub = bridge.root / ".hermes"
        (self.hub / "profiles").mkdir(parents=True)
        for name in AGENTS:
            home = self.home(name)
            home.mkdir(parents=True)
            (home / "config.yaml").write_text("model:\n  default: test\n")
            bridge.config["agents"][name.title()] = {
                "hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True, "sandboxed": True,
                "space": str(self.spaces / name), "python": f"{HERMES_PYTHON} {wrapper}",
                "hermes_home": str(home), "hermes_root": HERMES_SRC}
        bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"),
                                   "hub_url": fakes.hermes.url, "home": str(self.hub)}
        bridge.env_extra.update({"XDG_RUNTIME_DIR": str(runtime), "BIPAGENTS_SPACE_DIR": str(self.space_dir)})

    def home(self, name: str) -> Path:
        return self.spaces / name / "profiles" / name

    def link(self, name: str) -> Path:
        return self.home(name) / "vault"

    def start(self):
        self.bridge.start()
        give_token(self.bridge, "vie", TOKEN)
        return self

    def add(self, login) -> dict:
        made = self.bridge.post("/v1/vault", headers=AUTH, json=login)
        assert made.status_code == 201, made.text
        return made.json()

    def enable(self):
        self.bridge.stop()
        self.bridge.env_extra.update({"BIPAGENTS_KEYS_URL": self.keys.url, "BIPAGENTS_KEYS_TOKEN": "t" * 40})
        done = self.bridge.cli("vault-keys", "enable")
        assert done.returncode == 0, done.stdout + done.stderr
        return json.loads(done.stdout)

    def python_runs(self) -> list:
        if not self.runs.exists():
            return []
        return [json.loads(line) for line in self.runs.read_text().splitlines() if line.strip()]

    def rule(self, **rules):
        self.rules.write_text(json.dumps({k.replace("__", ":"): v for k, v in rules.items()}))

    def wait_copies(self, timeout: float = 60.0) -> dict:
        deadline = time.time() + timeout
        while time.time() < deadline:
            found = self.bridge.logged(DONE)
            if found:
                return found[-1]
            time.sleep(0.2)
        raise AssertionError(f"the copies were never said in memory:\n{self.bridge.log_tail()}")

    def master(self) -> list:
        """The bridge's master copy, read the way someone holding both halves would (the test plays it): this
        server's half, the key service's, keys.sealed.json."""
        from cryptography.fernet import Fernet
        from cryptography.hazmat.primitives import hashes
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
        from cryptography.hazmat.primitives.kdf.hkdf import HKDF
        sealed = json.loads((self.bridge.data / "keys.sealed.json").read_text())
        local = b64url_decode((self.space_dir / "vault.share").read_text())
        both = bytes(a ^ b for a, b in zip(local, self.keys.share))
        kek = HKDF(algorithm=hashes.SHA256(), length=32, salt=None, info=b"bipagents-vault-v1").derive(both)

        def unseal(key, text, aad):
            raw = b64url_decode(text)
            return AESGCM(key).decrypt(raw[:12], raw[12:], aad)

        sk = unseal(kek, sealed["sk"], b"bipagents-vault-v1 sk")
        vault_key = unseal(sk, sealed["keys"]["vault"], b"bipagents-vault-v1 key vault")
        return json.loads(Fernet(vault_key).decrypt((self.bridge.data / "vault" / "vault.json.enc").read_bytes()))

    def handles(self, name: str, records=None) -> dict:
        """label -> this agent's handle in the master copy."""
        return {r["label"]: r["agents"][name] for r in (records or self.master()) if name in (r.get("agents") or {})}

    def hermes(self, code: str, name: str, stdin: str = "", args=()) -> subprocess.CompletedProcess:
        """Python run in the agent's Hermes (its code, its HERMES_HOME), as its gateway runs."""
        head = f"import sys, json\nsys.path.insert(0, {HERMES_SRC!r})\nimport hermes_bootstrap\n"
        env = {"PATH": "/usr/bin:/bin", "HOME": str(self.bridge.root / "hermes-run-home"),
               "HERMES_HOME": str(self.home(name))}
        return subprocess.run([HERMES_PYTHON, "-c", head + code, *args], input=stdin, capture_output=True,
                              text=True, env=env, timeout=120)

    def listed(self, name: str) -> list:
        done = self.hermes("from pathlib import Path\nfrom agent.vault_store import VaultStore\n"
                           f"print(json.dumps([m.to_dict() for m in VaultStore(base_dir=Path({str(self.link(name))!r}))"
                           ".list_items()]))", name)
        assert done.returncode == 0, done.stderr[-800:]
        return json.loads(done.stdout.strip().splitlines()[-1])


FILL = r'''
import tools.browser_vault_tool as tool
handle, origin = sys.argv[1], sys.argv[2]
scripts = []

class Supervisor:
    """The browser's side of a fill (its CDP supervisor), as Chrome would answer on the item's login page."""
    def evaluate_runtime(self, expression):
        if expression == "window.location.href":
            return {"ok": True, "result": origin + "/login"}
        if "expectedOrigin" in expression:
            scripts.append(expression)
            return {"ok": True, "result": json.dumps({"filled": 1})}
        return {"ok": True, "result": json.dumps([{"index": 0, "type": "password", "autocomplete": "current-password",
                                                   "name": "password", "label": "Mot de passe", "formIndex": 0}])}

    def focus_page(self, origin, accept=None):
        return {"ok": True, "url": origin}

supervisor = Supervisor()
tool._ensure_supervisor = lambda task_id: supervisor
tool._page_backend = lambda task_id: None
tool._bot_desktop_browser_session = lambda task_id: False
out = json.loads(tool.browser_vault_fill(handle, task_id="bb"))
print(json.dumps({"out": out, "scripts": scripts}))
'''

SCRUB = r'''
import tools.mcp_tool_handlers as handlers
secret = sys.stdin.read()
print(json.dumps(handlers._scrub_vault_values("the field holds " + secret + " now")))
'''


def disk_files(root: Path):
    """Every file under `root` on the disk (links not followed)."""
    for folder, dirs, files in os.walk(root, followlinks=False):
        for name in files:
            path = Path(folder) / name
            if not path.is_symlink():
                yield path


def vault_folders_on_disk(*roots: Path):
    """Every real folder named vault under these roots (a link is not one)."""
    found = []
    for root in roots:
        for folder, dirs, _ in os.walk(root, followlinks=False):
            for name in dirs:
                path = Path(folder) / name
                if name == "vault" and not path.is_symlink():
                    found.append(path)
    return found


def place(bridges, fakes, runtime, keys) -> Place:
    return Place(bridges, fakes, runtime, keys)


# -- not enrolled: today's layout ---------------------------------------------------------------------------------------

def test_a_place_not_enrolled_keeps_its_hermes_copies_on_its_disk_as_today(bridges, fakes, runtime, keys):
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    for name in ("vie", "budget"):
        assert p.link(name).is_dir() and not p.link(name).is_symlink()
        assert {f.name for f in p.link(name).iterdir()} >= {"vault.key", "vault.json.enc"}
    assert not p.link("coach").exists()
    p.bridge.restart()
    time.sleep(2)
    assert not (runtime / "bipagents").exists(), "nothing in the memory folder"
    assert [r["op"] for r in p.python_runs()] == ["add", "add", "add"], "one run per item given, as today"
    assert not p.bridge.logged(DONE)
    assert [m["label"] for m in p.listed("vie")] == ["SNCF", "Impôts"]
    assert "vault" not in p.bridge.get("/health").json()


# -- enrolled: the copies in memory -------------------------------------------------------------------------------------

def test_once_enrolled_the_copies_are_rebuilt_in_memory_and_none_is_left_on_the_disk(bridges, fakes, runtime, keys):
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    disk_keys = {name: (p.link(name) / "vault.key").read_bytes() for name in ("vie", "budget")}
    report = p.enable()
    assert report["state"] == "sealed"
    for name in ("vie", "budget"):
        assert (p.link(name) / "vault.json.enc").is_file(), "enabling alone leaves the Hermes copies (never before)"
    before = p.handles("vie")
    runs = len(p.python_runs())
    p.bridge.start()
    said = p.wait_copies()
    assert said.get("failed") in (None, [], "[]"), said
    assert p.bridge.get("/health").json()["vault"] == "open"
    # one grouped run per agent holding items, none for an agent holding none
    rebuilt = [r for r in p.python_runs()[runs:] if r["op"] == "add_many"]
    assert sorted((r["agent"], r["items"]) for r in rebuilt) == [("budget", 1), ("vie", 2)]
    assert not [r for r in p.python_runs()[runs:] if r["op"] == "add"]
    # each agent's vault is a link into the memory folder; nothing of a copy on the disk
    for name in AGENTS:
        link = p.link(name)
        assert link.is_symlink(), name
        target = Path(os.readlink(link))
        assert target.is_absolute() and target.is_dir(), target
        assert str(target).startswith(str(runtime / "bipagents") + "/"), target
        assert memory_kind(target) in ("tmpfs", "ramfs")
        assert oct(target.stat().st_mode & 0o777) == oct(0o700)
    assert vault_folders_on_disk(p.hub, p.spaces) == []
    assert not [f for f in disk_files(p.bridge.root) if f.name in ("vault.key", "vault.keyref")], \
        "no Hermes key (and no clear key of the bridge's) on the disk"
    secrets = [login["password"].encode() for login in LOGINS.values()]
    secrets += list(disk_keys.values())
    secrets += [(p.link(n) / "vault.key").read_bytes().strip() for n in ("vie", "budget")]
    for path in disk_files(p.bridge.root):
        data = path.read_bytes()
        assert not [s for s in secrets if s in data], f"a secret or a Hermes key in {path}"
    # the bridge's handles are the copies' own, new ones
    records = p.master()
    for name in ("vie", "budget"):
        listed = {m["label"]: m["id"] for m in p.listed(name)}
        assert listed == p.handles(name, records), name
    assert set(p.handles("vie", records).values()).isdisjoint(before.values()), "rebuilt: new handles"
    assert p.listed("coach") == []
    # Hermes fills through the link, and its patch 0013 scrubs a password read back from the page
    handle = p.handles("vie", records)["SNCF"]
    done = p.hermes(FILL, "vie", args=(handle, "https://sncf-connect.com"))
    assert done.returncode == 0, done.stderr[-1500:]
    filled = json.loads(done.stdout.strip().splitlines()[-1])
    assert filled["out"]["success"] is True and filled["out"]["filled_fields"] == 1, filled["out"]
    assert len(filled["scripts"]) == 1 and json.dumps(LOGINS["sncf"]["password"])[1:-1] in filled["scripts"][0]
    assert LOGINS["sncf"]["password"] not in json.dumps(filled["out"])
    scrubbed = p.hermes(SCRUB, "vie", stdin=LOGINS["impots"]["password"])
    assert scrubbed.returncode == 0, scrubbed.stderr[-1500:]
    text = json.loads(scrubbed.stdout.strip().splitlines()[-1])
    assert LOGINS["impots"]["password"] not in text and "redacted-vault-secret" in text
    # an item given now goes to the copy in memory, one taken back leaves it
    later = p.add(LATER)
    assert [m["label"] for m in p.listed("vie")] == ["SNCF", "Impôts", "Ameli"]
    assert p.handles("vie")["Ameli"] in {m["id"] for m in p.listed("vie")}
    assert p.bridge.delete(f"/v1/vault/{later['id']}", headers=AUTH).status_code == 204
    assert [m["label"] for m in p.listed("vie")] == ["SNCF", "Impôts"]
    assert vault_folders_on_disk(p.hub, p.spaces) == []
    assert not [f for f in disk_files(p.bridge.root) if LATER["password"].encode() in f.read_bytes()]


def test_after_a_bridge_restart_the_copies_in_memory_are_checked_not_rebuilt(bridges, fakes, runtime, keys):
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    p.enable()
    p.bridge.start()
    p.wait_copies()
    handles = p.handles("vie")
    runs = len(p.python_runs())
    p.bridge.restart()   # an update of the bridge: the tmpfs is still there
    said = p.wait_copies()
    assert said.get("failed") in (None, [], "[]"), said
    assert not [r for r in p.python_runs()[runs:] if r["op"] == "add_many"], "nothing rebuilt"
    assert p.handles("vie") == handles
    assert {m["id"] for m in p.listed("vie")} == set(handles.values())


def test_after_a_server_restart_nothing_is_read_until_the_vault_reopens_then_the_copies_are_back(
        bridges, fakes, runtime, keys):
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    p.enable()
    p.bridge.start()
    p.wait_copies()
    # the server restarts: the memory folder is empty, the key service out of reach for a while
    p.bridge.stop()
    shutil.rmtree(runtime / "bipagents")
    keys.mode = "down"
    runs = len(p.python_runs())
    p.bridge.start()
    give_token(p.bridge, "vie", TOKEN)
    health = p.bridge.get("/health").json()
    assert health["vault"] == "locked" and health["vault_reason"] in ("opening", "unreachable")
    # no copy to read: Hermes' vault finds nothing (never a stale one), the bridge's tools say the lot-1 words
    for name in ("vie", "budget"):
        assert p.link(name).is_symlink() and not p.link(name).exists()
    gone = p.hermes("from pathlib import Path\nfrom agent.vault_store import VaultStore\n"
                    f"print(VaultStore(base_dir=Path({str(p.link('vie'))!r})).list_items())", "vie")
    assert gone.returncode != 0 and "Tr41n" not in gone.stdout
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "vault_login", "arguments": {"site": "sncf-connect.com"}}}
    said = p.bridge.post("/v1/agent-tools/vault", json=body, headers={"Authorization": f"Bearer {TOKEN}"}).json()
    text = said["result"]["content"][0]["text"]
    assert "password" in text.lower() or "mot de passe" in text.lower()
    assert ("closed" in text or "fermé" in text) and "Tr41n" not in text
    assert not [r for r in p.python_runs()[runs:] if r["op"] in ("add", "add_many")], "nothing given while closed"
    # the service answers: the vault reopens by itself and the copies are rebuilt, one run per agent
    keys.mode = "ok"
    said = p.wait_copies(timeout=90)
    assert said.get("failed") in (None, [], "[]"), said
    assert p.bridge.get("/health").json()["vault"] == "open"
    rebuilt = [r for r in p.python_runs()[runs:] if r["op"] == "add_many"]
    assert sorted((r["agent"], r["items"]) for r in rebuilt) == [("budget", 1), ("vie", 2)]
    for name in ("vie", "budget"):
        assert {m["label"]: m["id"] for m in p.listed(name)} == p.handles(name)
    assert vault_folders_on_disk(p.hub, p.spaces) == []


def test_a_copy_on_the_disk_is_erased_only_after_a_checked_rebuild(bridges, fakes, runtime, keys):
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    p.enable()
    copy = {f.name: f.read_bytes() for f in p.link("vie").iterdir() if f.is_file()}
    handles = p.handles("vie")
    # Vie's Hermes can't run: its copy stays on the disk, untouched, its handles too; Budget's goes to memory
    p.rule(vie__add_many="fail")
    p.bridge.start()
    said = p.wait_copies()
    assert "vie" in json.dumps(said.get("failed")), said
    assert not p.link("vie").is_symlink()
    assert {f.name: f.read_bytes() for f in p.link("vie").iterdir() if f.is_file()} == copy
    assert p.handles("vie") == handles
    assert p.link("budget").is_symlink()
    assert p.bridge.get("/health").json().get("vault_copies_not_ready") == 1, "the place's keeper sees it"
    # its Hermes answers but its copy doesn't hold every item: not trusted either
    p.rule(vie__add_many="short")
    p.bridge.restart()
    said = p.wait_copies()
    assert "vie" in json.dumps(said.get("failed")), said
    assert not p.link("vie").is_symlink()
    assert {f.name: f.read_bytes() for f in p.link("vie").iterdir() if f.is_file()} == copy
    assert p.handles("vie") == handles
    # mended: rebuilt, checked, and only then the disk's copy erased
    p.rule()
    p.bridge.restart()
    said = p.wait_copies()
    assert said.get("failed") in (None, [], "[]"), said
    assert p.link("vie").is_symlink()
    assert {m["label"]: m["id"] for m in p.listed("vie")} == p.handles("vie")
    assert vault_folders_on_disk(p.hub, p.spaces) == []
    for path in disk_files(p.bridge.root):
        assert copy["vault.key"].strip() not in path.read_bytes(), path
    assert "vault_copies_not_ready" not in p.bridge.get("/health").json()


def test_a_swap_cut_off_before_the_erase_leaves_nothing_on_the_disk_after_the_next_start(bridges, fakes, runtime, keys):
    """Review of 17/11 (B1): the bridge stopped between the swap and the erase (an update, an OOM): the disk's copy set
    aside beside the link (`.vault-disk-<hex>`, its key in it), a link not renamed yet. At the next start the set-aside
    copy is read once (what only Hermes held taken into the vault), the copy rebuilt, and what was set aside erased."""
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    copy = {f.name: f.read_bytes() for f in p.link("vie").iterdir() if f.is_file()}
    p.enable()
    p.bridge.start()
    p.wait_copies()
    p.bridge.stop()
    aside = p.home("vie") / ".vault-disk-1a2b3c4d"
    aside.mkdir()
    for name, data in copy.items():
        (aside / name).write_bytes(data)
    os.symlink(os.readlink(p.link("budget")), p.home("budget") / ".vault-link-0badc0de")
    runs = len(p.python_runs())
    p.bridge.start()
    said = p.wait_copies()
    assert said.get("failed") in (None, [], "[]"), said
    # the set-aside copy read once (what only Hermes held taken into the vault), then Vie's copy rebuilt; Budget's
    # only looked at (a leftover link holds nothing)
    after = p.python_runs()[runs:]
    assert [r["agent"] for r in after if r["op"] == "export"] == ["vie"], after
    assert [r["agent"] for r in after if r["op"] == "add_many"] == ["vie"], after
    assert {m["label"]: m["id"] for m in p.listed("vie")} == p.handles("vie")
    left = [Path(d) / n for d, dirs, files in os.walk(p.bridge.root, followlinks=False)
            for n in dirs + files if n.startswith(".vault-")]
    assert left == [], left
    for path in disk_files(p.bridge.root):
        assert copy["vault.key"].strip() not in path.read_bytes(), path


def test_a_login_only_the_agent_s_hermes_holds_is_taken_into_the_vault_before_its_disk_copy_goes(
        bridges, fakes, runtime, keys):
    """Second review of 17/11 (A): a login Vie's Hermes saved itself (`browser_vault save_login`, or a Hermes sent to the
    place, to_place.py) is in no record of the bridge. Its rebuild in memory takes it into the master copy first (one
    `export` run on the disk copy), gives it back to Vie, and only then erases the disk copy."""
    p = place(bridges, fakes, runtime, keys).start()
    for login in LOGINS.values():
        p.add(login)
    saved = p.hermes("from pathlib import Path\nfrom agent.vault_store import VaultStore\n"
                     f"m = VaultStore(base_dir=Path({str(p.link('vie'))!r})).add_item('login', 'Forum', "
                     "{'identifier_type': 'username', 'identifier': 'alex', 'password': 'F0rum-only-in-Hermes!'}, "
                     "origin='https://forum.example')\nprint(json.dumps(m.id))", "vie")
    assert saved.returncode == 0, saved.stderr[-1500:]
    p.enable()
    runs = len(p.python_runs())
    p.bridge.start()
    said = p.wait_copies()
    assert said.get("failed") in (None, [], "[]"), said
    assert [r["agent"] for r in p.python_runs()[runs:] if r["op"] == "export"] == ["vie"]
    records = p.master()
    forum = [r for r in records if r["label"] == "Forum"]
    assert len(forum) == 1 and forum[0]["identifier"] == "alex" and forum[0]["origin"] == "https://forum.example"
    assert forum[0]["secret"]["password"] == "F0rum-only-in-Hermes!"
    listed = {m["label"]: m["id"] for m in p.listed("vie")}
    assert sorted(listed) == ["Forum", "Impôts", "SNCF"] and listed == p.handles("vie", records)
    assert vault_folders_on_disk(p.hub, p.spaces) == []
    for path in disk_files(p.bridge.root):
        assert b"F0rum-only-in-Hermes!" not in path.read_bytes(), path
