"""The vault's keys sealed on a place of the offer (design note « Clé du coffre », lot 3; the bridge's side is
bridge-rs/src/sealed_keys.rs): the installer's part.

    bipagents vault-keys enable [--settings-file F] [--again] [--json]   enrol this place once (the bridge stopped)
    bipagents vault-keys status                                          the bridge's `vault-keys status`

Enrolling (`enable`): the bridge stopped, `bipbridge vault-keys enable` seals the vault's, the mailboxes' and the
connections' keys with a place key kept in memory, itself sealed by this server's half and the key service's
(`<space>/vault.share`, `<space>/keyservice.json`), then the bridge started again. The key service's settings come
from the space's keyservice.json when there is one (the encrypted space's enrolment), else from the guichet's: a new
place's setup (`vault_keys` in cloud.py) or the seat agent's order (a file read then deleted, never a command line).
Each try costs one of the place's 24 daily releases at the key service (shared with the encrypted space): it is made
once. A failure leaves today's layout (the bridge put everything back), is noted here for the seat agent (it tells the
guichet) and is never tried again by itself: `--again`, by hand, once its cause is known.

    ~/.local/state/bipagents/vault-keys.json    {state: sealed | failed, at, error?, settings?}: the last try (0600;
                                                `settings` is the token's SHA-256, never the token)

The export (`bipagents export`, moving.py) of an enrolled place carries the keys the place key opens, asked of the
bridge (`bipbridge vault-keys export`: only while the vault is open), never the place key, this server's half or the
key service's token; the import puts them back beside their stores (`back_to_today`): today's layout."""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, Dict, List, Optional

STATE = Path(".local") / "state" / "bipagents" / "vault-keys.json"
SPACE = Path(".local") / "share" / "bipagents" / "space"
SPACE_ENV = "BIPAGENTS_SPACE_DIR"
# In the space's folder (outside an encrypted space's open folder): this server's half and the key service's settings.
SPACE_FILES = ("vault.share", "keyservice.json")
SEALED = "keys.sealed.json"
JOURNAL = Path(".keys-migration") / "journal.json"
# Each store's folder under the bridge's data folder, and its key file in today's layout.
KEY_FILES = {"vault": "vault.key", "mail": "mail.key", "connections": "connections.key"}
BRIDGE_UNIT = "bipagents-bridge.service"
EXPORT_MEMBER = "vault-keys.json"   # in the export (moving.py): {"v": 1, "keys": {vault, mail, connections}}
ENV = {"url": "BIPAGENTS_KEYS_URL", "token": "BIPAGENTS_KEYS_TOKEN", "grant": "BIPAGENTS_KEYS_GRANT",
       "signature": "BIPAGENTS_KEYS_SIGNATURE"}


class VaultKeysError(Exception):
    """Safe to show (no key, no token in it)."""


class VaultClosed(VaultKeysError):
    """The vault of an enrolled place is closed: its keys can't be given now."""


def home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def space_dir(base: Optional[Path] = None) -> Path:
    """The space's folder, as the bridge finds it (space.rs `default_dir`)."""
    found = os.environ.get(SPACE_ENV)
    return Path(found).expanduser() if found else (base or home()) / SPACE


def sealed(data_dir: Path) -> bool:
    """The place's vault keys are sealed, or a sealing is under way (sealed_keys.rs `enrolled`)."""
    return (data_dir / SEALED).exists() or (data_dir / JOURNAL).exists()


def data_dir(config: Path, base: Optional[Path] = None) -> Path:
    """The bridge's data folder, as its config says (moving.py's Inventory.data_dir)."""
    import tomllib
    try:
        found = tomllib.loads(config.read_text(encoding="utf-8")).get("data_dir")
    except (OSError, ValueError):
        found = None
    return Path(found).expanduser() if isinstance(found, str) and found else \
        (base or home()) / ".local" / "share" / "hermes-bridge"


def valid(settings) -> Optional[Dict[str, str]]:
    """The key service's settings as the guichet gives them (cloud.py checks a setup's the same way), else None."""
    if not isinstance(settings, dict):
        return None
    url, grant, signature, token = (settings.get(k) for k in ("url", "grant", "signature", "token"))
    if not isinstance(url, str) or not url.startswith("https://") or len(url) > 300:
        return None
    if not all(isinstance(v, str) and v for v in (grant, signature)):
        return None
    if not isinstance(token, str) or not 16 <= len(token) <= 200 or not token.isprintable() or token != token.strip():
        return None
    return {"url": url, "grant": grant, "signature": signature, "token": token}


def _token_sha(settings: Optional[Dict[str, str]]) -> Optional[str]:
    return hashlib.sha256(settings["token"].encode()).hexdigest() if settings else None


def read_state(base: Optional[Path] = None) -> Dict:
    try:
        data = json.loads(((base or home()) / STATE).read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def _write_state(base: Path, data: Dict) -> None:
    path = base / STATE
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.replace(tmp, path)


def bridge_command(base: Optional[Path] = None) -> List[str]:
    from .rustbridge import cli
    return cli(base or home())


def run(args: List[str], env: Optional[Dict[str, str]] = None) -> subprocess.CompletedProcess:
    return subprocess.run(list(args), text=True, capture_output=True, env={**os.environ, **(env or {})}, check=False,
                          timeout=600)


def _short(text: str, settings: Optional[Dict[str, str]]) -> str:
    """The bridge's last words, without the token (it never prints it; this is in case)."""
    lines = [line for line in (text or "").strip().splitlines() if line.strip()]
    said = (lines[-1] if lines else "no output")[-300:]
    if settings:
        said = said.replace(settings["token"], "…")
    return said


def enable(settings: Optional[Dict], runner: Callable = run, say: Callable[[str], None] = print,
           base: Optional[Path] = None, again: bool = False) -> int:
    """This place enrolled, once: 0 when its keys are sealed (or were already), 1 when not (today's layout stays).
    `settings`: the guichet's (a setup's `vault_keys`, an order's), None for the space's keyservice.json only."""
    base = base or home()
    given = None
    if settings is not None:
        given = valid(settings)
        if given is None:
            say("The key service's settings aren't an https address, a signed grant and a token of this place's own: "
                "nothing changed.")
            return 1
    config = base / ".config" / "bipagents" / "bridge.toml"
    if sealed(data_dir(config, base)):
        say("The vault's keys are already sealed: nothing to do.")
        return 0
    last = read_state(base)
    if last.get("state") == "failed" and last.get("settings") == _token_sha(given) and not again:
        # never a loop: each try is one of the place's daily releases
        say(f"This place's vault keys were already tried with these settings and it failed ({last.get('error')}): "
            "nothing tried again (`bipagents vault-keys enable --again` once the cause is known).")
        return 1
    env = {ENV[k]: v for k, v in (given or {}).items()}
    env["BRIDGE_CONFIG"] = str(config)
    runner(["systemctl", "--user", "stop", BRIDGE_UNIT])
    try:
        done = runner(bridge_command(base) + ["vault-keys", "enable", "--config", str(config)], env=env)
    finally:
        runner(["systemctl", "--user", "start", BRIDGE_UNIT])
    now = time.time()
    if getattr(done, "returncode", 1) != 0:
        why = _short(getattr(done, "stderr", "") or getattr(done, "stdout", ""), given)
        _write_state(base, {"state": "failed", "at": now, "error": why, "settings": _token_sha(given)})
        say(f"The vault's keys are not sealed, everything is as before: {why}")
        return 1
    _write_state(base, {"state": "sealed", "at": now, "settings": _token_sha(given)})
    say("The vault's keys are sealed: their key is never written on this server's disk.")
    return 0


# -- the export and the import (moving.py) ------------------------------------------------------------------------------

def export_keys(inv, runner: Callable = run) -> Dict[str, str]:
    """The keys the place key opens, from the bridge (`bipbridge vault-keys export`): {vault, mail, connections}.
    VaultClosed when the bridge can't give them (the vault closed, a sealing under way)."""
    done = runner(bridge_command(inv.home) + ["vault-keys", "export", "--config", str(inv.config)],
                  env={"BRIDGE_CONFIG": str(inv.config)})
    if getattr(done, "returncode", 1) != 0:
        raise VaultClosed(_short(getattr(done, "stderr", ""), None))
    try:
        keys = json.loads(done.stdout).get("keys")
    except (ValueError, AttributeError):
        keys = None
    return _checked(keys)


def _checked(keys) -> Dict[str, str]:
    if not isinstance(keys, dict) or set(keys) != set(KEY_FILES):
        raise VaultKeysError("the vault's keys aren't the three stores' keys")
    for name, key in keys.items():
        if not isinstance(key, str) or not 40 <= len(key) <= 64 or not all(c.isalnum() or c in "-_=" for c in key):
            raise VaultKeysError(f"the {name} key isn't a key")
    return dict(keys)


def back_to_today(data_dir: Path, keys) -> List[Path]:
    """An enrolled place's export imported: each store's key beside it (0600, its folder 0700), then keys.sealed.json
    removed: today's layout. Nothing changed when the keys aren't right. Returns the key files written."""
    keys = _checked(keys)
    written = []
    for name, key_file in KEY_FILES.items():
        folder = data_dir / name
        folder.mkdir(parents=True, exist_ok=True)
        os.chmod(folder, 0o700)
        path = folder / key_file
        tmp = path.with_name(path.name + ".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(keys[name])
        os.replace(tmp, path)
        written.append(path)
    (data_dir / SEALED).unlink(missing_ok=True)
    return written


# -- the command line ------------------------------------------------------------------------------------------------

def main(argv: Optional[List[str]] = None) -> int:
    import argparse
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == "status":
        config = home() / ".config" / "bipagents" / "bridge.toml"
        done = run(bridge_command() + ["vault-keys", "status", "--config", str(config)], env={"BRIDGE_CONFIG": str(config)})
        print(done.stdout, end="")
        if done.returncode != 0:
            print(done.stderr, end="", file=sys.stderr)
        return done.returncode
    parser = argparse.ArgumentParser(prog="bipagents vault-keys", description="The vault's keys sealed with the key "
                                     "service (a place of the offer)")
    parser.add_argument("action", choices=["enable"])
    parser.add_argument("--settings-file", help="the guichet's key service settings (JSON; deleted once read)")
    parser.add_argument("--again", action="store_true", help="try again after a failure (each try costs a release)")
    parser.add_argument("--json", action="store_true", help="one line of JSON at the end (the seat agent reads it)")
    args = parser.parse_args(argv)
    settings = None
    if args.settings_file:
        path = Path(args.settings_file)
        try:
            settings = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            settings = {}
        finally:
            path.unlink(missing_ok=True)
    said: List[str] = []
    status = enable(settings, say=(said.append if args.json else print), again=args.again)
    if args.json:
        state = read_state()
        print(json.dumps({"state": state.get("state") or ("sealed" if status == 0 else "failed"),
                          **({"error": said[-1][:300]} if status and said else {})}))
    return status
