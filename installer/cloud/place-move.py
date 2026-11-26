#!/usr/bin/env python3
"""BipAgents: a planned move of one place to another server of the offer, while its agents keep working (2026-10-26).

Run as root by the seat agent (seat-agent.py) on the guichet's orders: `move_in` on the target server, `move_out` on
the source server, both with the same one-time key (32 random bytes the guichet made for this move). From that key
both servers derive two Ed25519 keys: the target's sshd for this move only (its host key) and the source's (its
client key). The target lets that client key in, from the source's address only, on the move's port, for one
command: this script's `serve`, which allows rsync writing into the move's own folder of root's in the target place's
home (STAGING, moved into the home at `finish`) and the move's few steps. The place's plan is followed only within its
real home, read through a view in which no link is followed (`checked_plan`, `VIEW`: security review of 26/11, E1).
The data goes from server to server inside that SSH channel, never through the guichet or any machine of Kerfy's.

    place-move.py in <order file>      (target) the listener: its sshd, its firewall rule, its keys
    place-move.py out <order file>     (source, a unit of its own: bipagents-move-<id>) the whole move:
        1. copy while running: rsync of the place's folders (installer/bipinstall/placemove.py says which, and what
           to leave out) again and again until what changed between two copies is small
        2. a quiet moment: in the guichet's window (or the next half hour), when the bridge (GET /v1/place/move) has
           no turn running; at the latest five minutes before the window ends
        3. drain: the bridge refuses the apps' writes (503 "moving": the apps keep the person's sends), pauses the
           background tasks at their next step, queues new ones, pauses the routines due in the window
        4. switch: the place stopped, its last changes copied (checksums: nothing missed), the target adjusts the
           settings to its own door and ports (as a restore does), starts it with the same keys, checks its bridge,
           each agent's gateway and the routines, and resumes the tasks and routines (those whose time passed run
           now). Any failure: the target stops its copy, the source starts again as it was.
        5. the source's copy is kept, stopped, seven days, then its account goes (`cleanup`)
        A place whose bridge is down (the order's `"drain": false`, 2026-10-30): no copies while it runs, no window,
        no drain: its services stopped (what still runs of its bridge or gateways killed), its files copied as they
        are, then the same switch. Any failure, at any step, ends in `failed` with a short reason (`out`).
    place-move.py serve <id>           (target, the forced command of the move's SSH key)
    place-move.py cancel <id>          either side: the move called off (the source resumes if it had stopped)
    place-move.py close <id>           (target) the listener gone: sshd stopped, firewall rule and keys removed
    place-move.py cleanup              each beat: listeners past their time closed, source copies past 7 days removed
    place-move.py states               what the beat reports: each move's phase and numbers, nothing of anyone

State: /var/lib/bipagents/moves/<id>/ (root only): order.json (with the key), state.json (what the beat says).
ASCII only, like seat-agent.py and user-data.yaml (installer/tests/test_cloud_init.py checks it).
"""
import base64
import hashlib
import hmac
import json
import os
import pwd
import re
import shlex
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

LIB = "/usr/local/lib/bipagents"
MOVES = "/var/lib/bipagents/moves"
MAX_SEATS = 40                  # places on one server at most: doors 443 to 482 (seat-agent.py)
KEEP_SECONDS = 7 * 86400
SMALL_DELTA = 32 * 2 ** 20      # bytes changed between two copies: small enough to switch
MAX_ROUNDS = 6                  # copies before the quiet moment, at most
ROUND_EVERY = 120               # seconds between copies while waiting for the window
DRAIN_WAIT = 150                # seconds the running turns and tasks get to finish or pause
AUTO_WINDOW = 1800              # no window from the guichet: the next half hour
LATEST_BEFORE_END = 300         # the switch starts at the latest this long before the window ends
SUB_RANGE = 65536
MOVED_AWAY = ".config/bipagents/moved-away.json"
STAGE = ".local/state/bipagents/move-stage"   # the source's manifest and volumes (bipinstall/placemove.py)
MOVE_IN = ".local/state/bipagents/move-in"    # the target's notes, and the stage once over (bipinstall/placemove.py)
# The target: root's own folder (0700) in the arriving place's home, on its disk, where every copy lands (roots/<name>,
# stage) and nothing else; moved into the home once the last copy is in (security review 26/11, E1)
STAGING = ".bipagents-move-in"
ROOT_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
# The source reads the place's home through a view of it no link is ever followed in (security review 26/11, E1): the
# home bound read-only, `nosymfollow` (Linux 5.10, util-linux 2.38), in a mount namespace of the copy's own.
VIEW = ["unshare", "--mount", "--propagation", "private", "--"]
# The view is checked before rsync runs: an older util-linux would drop nosymfollow without a word.
VIEW_SCRIPT = ('mount --bind -- "$1" "$2" && mount -o remount,bind,ro,nosymfollow -- "$2" && '
               'findmnt -n -o OPTIONS --mountpoint "$2" | tr , "\\n" | grep -qx nosymfollow && shift 2 && exec "$@"')
NO_LINK_DIR = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
ID = re.compile(r"^[a-z0-9][a-z0-9-]{5,62}$")
PKCS8_ED25519 = bytes.fromhex("302e020100300506032b657004220420")
STATE_FIELDS = ("id", "role", "seat", "phase", "round", "copied_bytes", "delta_bytes", "files", "started", "updated",
                "drain_seconds", "downtime_seconds", "error", "kept_until", "result", "verified")


def log(text):
    print("%s %s" % (time.strftime("%H:%M:%S"), text), flush=True)


def account(number):
    return "bip" if number == 1 else "bip%d" % number


class MoveError(Exception):
    """Safe to report (no key, nothing of the person)."""


# -- the one-time key and what both sides derive from it ----------------------------------------------------------------

def secret_of(key):
    raw = base64.urlsafe_b64decode(key + "=" * (-len(key) % 4))
    if len(raw) < 32:
        raise MoveError("the move's key is too short")
    return raw


def seed(key, label):
    """32 bytes for one of the move's Ed25519 keys ("host" or "client"), the same on both servers."""
    return hmac.new(secret_of(key), ("bipagents-move/" + label).encode(), hashlib.sha256).digest()


def ed25519_public(private_seed):
    """The public key of an Ed25519 seed (openssl does the curve)."""
    pem = "-----BEGIN PRIVATE KEY-----\n%s\n-----END PRIVATE KEY-----\n" % base64.b64encode(
        PKCS8_ED25519 + private_seed).decode()
    done = subprocess.run(["openssl", "pkey", "-pubout", "-outform", "DER"], input=pem.encode(), capture_output=True)
    if done.returncode != 0 or len(done.stdout) < 32:
        raise MoveError("openssl could not read the move's key")
    return done.stdout[-32:]


def _s(data):
    return struct.pack(">I", len(data)) + data


def openssh_public(public):
    return "ssh-ed25519 " + base64.b64encode(_s(b"ssh-ed25519") + _s(public)).decode()


def openssh_private(private_seed, public, comment="bipagents-move"):
    """OpenSSH's own format (openssh-key-v1), unencrypted: what ssh and sshd read."""
    check = os.urandom(4)
    inner = (check + check + _s(b"ssh-ed25519") + _s(public) + _s(private_seed + public) + _s(comment.encode()))
    pad = 1
    while len(inner) % 8:
        inner += bytes([pad])
        pad += 1
    body = (b"openssh-key-v1\0" + _s(b"none") + _s(b"none") + _s(b"") + struct.pack(">I", 1)
            + _s(_s(b"ssh-ed25519") + _s(public)) + _s(inner))
    text = base64.b64encode(body).decode()
    lines = [text[i:i + 70] for i in range(0, len(text), 70)]
    return "-----BEGIN OPENSSH PRIVATE KEY-----\n" + "\n".join(lines) + "\n-----END OPENSSH PRIVATE KEY-----\n"


def keypair(key, label):
    private_seed = seed(key, label)
    public = ed25519_public(private_seed)
    return openssh_private(private_seed, public), openssh_public(public)


# -- the move's folder and state ----------------------------------------------------------------------------------------

def folder(move_id, root=MOVES):
    if not isinstance(move_id, str) or not ID.match(move_id):
        raise MoveError("not a move id")
    return os.path.join(root, move_id)


def write_private(path, text):
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    fd = os.open(path + ".tmp", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        fh.write(text)
    os.replace(path + ".tmp", path)


def read_json(path):
    try:
        with open(path) as fh:
            data = json.load(fh)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


# -- root in a place's home: never through a link the place put there (security review 26/11; seat-agent.py has the same)

def home_folder(home, rel, make=None):
    """The folder `rel` under the place's home, by descriptor, no link followed on its way (OSError when one is
    there). `make`: (uid, gid, mode) to make the missing ones, given to the place by descriptor; else a missing one
    raises FileNotFoundError. The caller closes the descriptor."""
    parts = [part for part in rel.split("/") if part]
    if any(part in (".", "..") for part in parts) or rel.startswith("/"):
        raise OSError("not a folder of the place's home: %s" % rel)
    fd = os.open(home, NO_LINK_DIR)
    try:
        for part in parts:
            made = False
            try:
                child = os.open(part, NO_LINK_DIR, dir_fd=fd)
            except FileNotFoundError:
                if make is None:
                    raise
                os.mkdir(part, 0o700, dir_fd=fd)
                made = True
                child = os.open(part, NO_LINK_DIR, dir_fd=fd)
            os.close(fd)
            fd = child
            if made:
                os.fchown(fd, make[0], make[1])
                os.fchmod(fd, make[2])
        return fd
    except BaseException:
        os.close(fd)
        raise


def home_read(home, rel, limit=1 << 20):
    """The place's regular file `rel` (never through a link, never a pipe: opened without waiting), else None."""
    folder, name = os.path.split(rel)
    try:
        dir_fd = home_folder(home, folder)
    except OSError:
        return None
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
                     dir_fd=dir_fd)
    except OSError:
        return None
    finally:
        os.close(dir_fd)
    with os.fdopen(fd, "rb") as fh:
        if not stat.S_ISREG(os.fstat(fh.fileno()).st_mode):
            return None
        return fh.read(limit)


def home_json(home, rel):
    raw = home_read(home, rel)
    try:
        value = json.loads(raw.decode("utf-8")) if raw is not None else {}
    except (ValueError, RecursionError):
        return {}
    return value if isinstance(value, dict) else {}


def home_mark(home, rel, text):
    """Root's own file `rel` (0600) in the place's folder, which must be there: written beside under a name of its
    own, then moved over `rel` (a link there replaced, never followed), in the folder reached without a link."""
    folder, name = os.path.split(rel)
    dir_fd = home_folder(home, folder)
    try:
        tmp = "%s.%s.tmp" % (name, base64.b32encode(os.urandom(5)).decode().lower())
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600, dir_fd=dir_fd)
        with os.fdopen(fd, "w") as fh:
            fh.write(text)
        os.replace(tmp, name, src_dir_fd=dir_fd, dst_dir_fd=dir_fd)
    finally:
        os.close(dir_fd)


def home_rel(home, path):
    """`path` (absolute) relative to the place's home, None when it leads out of it (or names `..`)."""
    if not isinstance(path, str) or not os.path.isabs(path) or ".." in path.split("/"):
        return None
    rel = os.path.relpath(os.path.normpath(path), os.path.normpath(home))
    return None if rel == "." or rel.startswith("..") else rel


def home_kind(home, rel):
    """What `rel` is in the place's home, reached without a link: "dir", "file", or None (not there); MoveError for a
    link (or anything else) there or on its way."""
    folder, name = os.path.split(rel)
    try:
        dir_fd = home_folder(home, folder)
    except FileNotFoundError:
        return None
    except OSError:
        raise MoveError("a link is on the way to a folder of the place: %s" % rel.split("/")[0])
    try:
        found = os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
    except FileNotFoundError:
        return None
    finally:
        os.close(dir_fd)
    if stat.S_ISDIR(found.st_mode):
        return "dir"
    if stat.S_ISREG(found.st_mode):
        return "file"
    raise MoveError("a folder of the place is a link: %s" % rel.split("/")[-1])


def overlap(a, b):
    return a == b or a.startswith(b + "/") or b.startswith(a + "/")


def overlapping(roots):
    rels = list(roots.values())
    return any(overlap(a, b) for i, a in enumerate(rels) for b in rels[i + 1:])


def checked_plan(plan, home):
    """The place's plan (its own account's output: whatever it likes) as root follows it, else MoveError: each root a
    folder or file of the place's real home (pwd's, never the plan's word for it) by its path in it, no link on its
    way nor itself a link, none inside another; its stage its own; its rules exclusions only (`- pattern`: a rule
    can name files rsync then reads, as root). {"roots": {name: rel}, "rules": {name: [rule]}}."""
    if not isinstance(plan, dict) or not isinstance(plan.get("roots"), dict) or len(plan["roots"]) > 64:
        raise MoveError("the place's plan holds no roots")
    roots = {}
    for name, path in plan["roots"].items():
        if not isinstance(name, str) or not ROOT_NAME.match(name):
            raise MoveError("a folder of the place has an odd name")
        rel = home_rel(home, path)
        if rel is None:
            raise MoveError("a folder of the place is outside its home: %s" % name)
        home_kind(home, rel)
        roots[name] = rel
    if overlapping(roots):
        raise MoveError("two folders of the place's plan overlap")
    if plan.get("stage") != os.path.join(home, STAGE):
        raise MoveError("the place's stage is not its own")
    rules = plan.get("rules") or {}
    if not isinstance(rules, dict) or not set(rules) <= set(roots):
        raise MoveError("the place's rules are odd")
    for given in rules.values():
        if not isinstance(given, list) or not all(isinstance(rule, str) and rule.startswith("- ") and len(rule) < 1024
                                                  and rule.isprintable() for rule in given):
            raise MoveError("a rule of the place's plan is not an exclusion")
    return {"roots": roots, "rules": {name: list(given) for name, given in rules.items()}}


def checked_roots(roots):
    """The roots the source names to the target ({name: rel}), else MoveError: odd names, paths out of the home, or
    over the target's own folders for the move."""
    if not isinstance(roots, dict) or not roots or len(roots) > 64:
        raise MoveError("no folders named")
    for name, rel in roots.items():
        if not isinstance(name, str) or not ROOT_NAME.match(name) or not isinstance(rel, str) \
                or rel != os.path.normpath(rel) or rel.startswith(("/", "..")) or rel in ("", ".") \
                or ".." in rel.split("/") or "\0" in rel:
            raise MoveError("a folder named is not one of a home")
        if overlap(rel, STAGING) or overlap(rel, MOVE_IN):
            raise MoveError("a folder named is the move's own")
    if overlapping(roots):
        raise MoveError("two folders named overlap")
    return dict(roots)


def save(move_id, root=MOVES, **fields):
    path = os.path.join(folder(move_id, root), "state.json")
    state = read_json(path)
    state.update(fields)
    state["id"] = move_id
    state["updated"] = int(time.time())
    write_private(path, json.dumps(state))
    return state


def states(root=MOVES):
    """Each move's phase and numbers for the beat (times, counts, bytes, an error's words: nothing of anyone)."""
    found = []
    try:
        names = sorted(os.listdir(root))
    except OSError:
        return found
    for name in names:
        state = read_json(os.path.join(root, name, "state.json"))
        if state:
            found.append({k: state[k] for k in STATE_FIELDS if k in state})
    return found


def order_of(path):
    order = read_json(path)
    move = order.get("move") or {}
    if not isinstance(move, dict) or not ID.match(str(move.get("id", ""))) or not move.get("key"):
        raise MoveError("the order holds no move")
    return order, move


# -- the place's own account --------------------------------------------------------------------------------------------

def place(number):
    entry = pwd.getpwnam(account(number))
    return entry


def as_user(entry, args, timeout=900):
    """`bipinstall <args>` as the place's account (its installer, its systemd user manager); the last JSON line."""
    home = entry.pw_dir
    command = ["sudo", "-u", entry.pw_name, "env", "HOME=" + home, "USER=" + entry.pw_name,
               "XDG_RUNTIME_DIR=/run/user/%d" % entry.pw_uid,
               "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%d/bus" % entry.pw_uid,
               "PATH=%s/.local/bin:/usr/local/bin:/usr/bin:/bin" % home,
               "PYTHONPATH=%s/BipAgents/installer" % home,
               "%s/BipAgents/installer/.venv/bin/python" % home, "-m", "bipinstall"] + list(args)
    done = subprocess.run(command, capture_output=True, text=True, timeout=timeout, cwd=home)
    for line in reversed(done.stdout.strip().splitlines()):
        if line.startswith("{"):
            try:
                result = json.loads(line)
            except ValueError:
                continue
            if done.returncode != 0 or result.get("error"):
                raise MoveError("%s: %s" % (" ".join(args), str(result.get("error"))[:200]))
            return result
    raise MoveError("%s gave no answer (%d): %s" % (" ".join(args), done.returncode, done.stderr.strip()[-200:]))


def bridge_settings(home):
    """The place's bridge.toml, read as root: never through a link, never a pipe (OSError)."""
    import tomllib
    raw = home_read(home, ".config/bipagents/bridge.toml")
    if raw is None:
        raise OSError("the place's bridge.toml is not readable")
    try:
        return tomllib.loads(raw.decode("utf-8"))
    except RecursionError:
        raise ValueError("the place's bridge.toml is nested too deep")


def bridge(home, method, path, body=None, timeout=30):
    """A call to the place's own bridge on this server, with its key (root reads it from the place's settings)."""
    conf = bridge_settings(home)
    host = conf.get("host") or "127.0.0.1"
    host = "127.0.0.1" if host in ("0.0.0.0", "::") else host
    url = "http://%s:%d%s" % (host, int(conf.get("port", 8643)), path)
    data = json.dumps(body).encode() if body is not None else (b"{}" if method == "POST" else None)
    request = urllib.request.Request(url, data=data, method=method, headers={
        "Authorization": "Bearer " + str(conf.get("bridge_key", "")), "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read().decode() or "{}")


def gateways_answer(home, timeout=180):
    """Each agent's Hermes gateway answers on this server (GET /v1/capabilities with the agent's own key)."""
    deadline = time.time() + timeout
    waiting = None
    while time.time() < deadline:
        waiting = []
        for name, agent in (bridge_settings(home).get("agents") or {}).items():
            if not isinstance(agent, dict) or not agent.get("hermes_url"):
                continue
            request = urllib.request.Request(str(agent["hermes_url"]).rstrip("/") + "/v1/capabilities", headers={
                "Authorization": "Bearer " + str(agent.get("hermes_key", ""))})
            try:
                with urllib.request.urlopen(request, timeout=10):
                    pass
            except (OSError, ValueError):
                waiting.append(name)
        if not waiting:
            return []
        time.sleep(3)
    return waiting


def subids(user, path="/etc/subuid"):
    """The first of the account's subordinate ids (its agents' sandboxes), else None."""
    try:
        with open(path) as fh:
            for line in fh:
                parts = line.strip().split(":")
                if len(parts) == 3 and parts[0] == user:
                    return int(parts[1])
    except (OSError, ValueError):
        pass
    return None


def shifted(value, own_from, own_to, sub_from, sub_to):
    """An owner of the source's files as the target's: the place's own id, its sandboxes' range moved along."""
    if value == own_from:
        return own_to
    if sub_from is not None and sub_to is not None and sub_from <= value < sub_from + SUB_RANGE:
        return value - sub_from + sub_to
    return value


def _shift_one(dir_fd, name, src, dst):
    found = os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
    uid = shifted(found.st_uid, src["uid"], dst["uid"], src.get("sub_uid"), dst.get("sub_uid"))
    gid = shifted(found.st_gid, src["gid"], dst["gid"], src.get("sub_gid"), dst.get("sub_gid"))
    if (uid, gid) == (found.st_uid, found.st_gid):
        return 0
    os.chown(name, uid, gid, dir_fd=dir_fd, follow_symlinks=False)
    return 1


def shift_staged(home, src, dst):
    """The copies' owners given to the target's ids (rsync kept the source's numbers) while they are still in
    STAGING, root's alone (review of 59ee7434b: once back in the home they are the place's, and a folder it swaps for
    a link while root walks would lead root's chown to a neighbour's files). By descriptors only: each folder walked
    from its parent's (os.fwalk, no link followed), each entry changed by name in it, never through a path the place
    could rename. Returns how many were changed."""
    folder, kept = staging(home)
    changed = 0
    try:
        tops = [(kept, name) for name in sorted(os.listdir(kept))]
        if _lexists("stage", folder) is not None:
            tops.append((folder, "stage"))
        for parent, name in tops:
            changed += _shift_one(parent, name, src, dst)
            if stat.S_ISDIR(os.stat(name, dir_fd=parent, follow_symlinks=False).st_mode):
                for _, dirs, files, at in os.fwalk(name, dir_fd=parent, follow_symlinks=False):
                    for entry in dirs + files:
                        changed += _shift_one(at, entry, src, dst)
    finally:
        os.close(kept)
        os.close(folder)
    return changed


# -- the target's folder of root's for the copies (security review 26/11, E1) -------------------------------------------
#
# The copies never land in the arriving place's home itself: a link the first copy made there (the source place's,
# whatever it holds) would be followed by the next one, as root (the review's scenario A: root's own seat-agent.py
# rewritten). They land in STAGING, root's (0700), where only root's rsync writes, each through a descriptor of the
# folder checked first (`staged`): roots/<name> per root of the place, and stage. `prepare` moves the fresh place's
# own folders there first, so that each copy goes over them as before (what the rules leave out stays the target's:
# its bridge binaries, its config's lock); `finish` moves them back, by name in folders opened without a link.

def _root_owned(fd, what):
    found = os.fstat(fd)
    if not stat.S_ISDIR(found.st_mode) or found.st_uid != os.geteuid() or found.st_mode & 0o077:
        os.close(fd)
        raise MoveError("the move's folder is not root's alone: %s" % what)
    return fd


def staging(home, make=False):
    """(descriptor of STAGING, descriptor of its roots/): root's own, 0700, reached without a link, made when `make`.
    The caller closes both."""
    home_fd = os.open(home, NO_LINK_DIR)
    try:
        if make:
            try:
                os.mkdir(STAGING, 0o700, dir_fd=home_fd)
            except FileExistsError:
                pass
        folder = _root_owned(os.open(STAGING, NO_LINK_DIR, dir_fd=home_fd), STAGING)
    finally:
        os.close(home_fd)
    try:
        if make:
            try:
                os.mkdir("roots", 0o700, dir_fd=folder)
            except FileExistsError:
                pass
        return folder, _root_owned(os.open("roots", NO_LINK_DIR, dir_fd=folder), "roots")
    except BaseException:
        os.close(folder)
        raise


def _lexists(name, dir_fd):
    try:
        return os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
    except FileNotFoundError:
        return None


def set_aside(home, roots):
    """`prepare`: the fresh place's own folders and files at the roots' places moved into STAGING (a link there is
    dropped: a copy must never land on one). A root already set aside (prepare asked again) is left as it is."""
    folder, kept = staging(home, make=True)
    try:
        for name, rel in sorted(roots.items()):
            if _lexists(name, kept):
                continue
            try:
                parent = home_folder(home, os.path.dirname(rel))
            except FileNotFoundError:
                continue
            try:
                found = _lexists(os.path.basename(rel), parent)
                if found is None:
                    continue
                if stat.S_ISLNK(found.st_mode):
                    os.unlink(os.path.basename(rel), dir_fd=parent)
                else:
                    os.rename(os.path.basename(rel), name, src_dir_fd=parent, dst_dir_fd=kept)
            finally:
                os.close(parent)
    finally:
        os.close(kept)
        os.close(folder)


def staged(home, token, roots):
    """Where one copy writes: (descriptor of the folder of root's it writes in, the name rsync gets), for `token`
    roots/<name>[/] (a root named at `prepare`) or stage/. MoveError when anything but that, or when what is there is
    a link."""
    slash = token.endswith("/")
    kind, _, name = token.rstrip("/").partition("/")
    if not (kind == "roots" and name in roots) and not (token == "stage/"):
        raise MoveError("not a folder of the move")
    folder, kept = staging(home)
    if kind == "stage":
        os.close(kept)
        at, name = folder, "stage"
    else:
        os.close(folder)
        at = kept
    found = _lexists(name, at)
    if found is not None and not (stat.S_ISDIR(found.st_mode) or stat.S_ISREG(found.st_mode)):
        os.close(at)
        raise MoveError("a copy of the move would land on a link")
    return at, name + ("/" if slash else "")


def put_back(home, roots, uid, gid):
    """`finish`: each root from STAGING to its place in the home (the folders on its way made the place's when
    missing, none of them a link), the stage into MOVE_IN; then STAGING removed."""
    folder, kept = staging(home)
    try:
        aside = "old-%d" % int(time.time() * 1000)
        os.mkdir(aside, 0o700, dir_fd=folder)
        aside_fd = os.open(aside, NO_LINK_DIR, dir_fd=folder)
        try:
            moves = [(kept, name, rel, "root-" + name) for name, rel in sorted(roots.items())]
            for source, name, rel, aside_name in moves + [(folder, "stage", MOVE_IN + "/stage", "stage")]:
                if _lexists(name, source) is None:
                    continue
                parent = home_folder(home, os.path.dirname(rel), (uid, gid, 0o700))
                try:
                    base = os.path.basename(rel)
                    if _lexists(base, parent) is not None:   # something made there meanwhile: set aside, removed
                        os.rename(base, aside_name, src_dir_fd=parent, dst_dir_fd=aside_fd)
                    os.rename(name, base, src_dir_fd=source, dst_dir_fd=parent)
                finally:
                    os.close(parent)
        finally:
            os.close(aside_fd)
    finally:
        os.close(kept)
        os.close(folder)
    shutil.rmtree(os.path.join(home, STAGING))   # by descriptors: never through a link (shutil's safe rmtree)


# -- the target: its listener and the commands it accepts ---------------------------------------------------------------

USER = "bipmove"                  # the account the source logs in as (root's password may be expired: no root login)
KEYS = "/etc/bipagents/move-keys"   # its authorized keys, one file per move (sshd reads them as that account)
SUDOERS = "/etc/sudoers.d/bipagents-move"


def sshd_config(base, port, keys=KEYS, move_id="x"):
    return "\n".join([
        "Port %d" % port, "ListenAddress 0.0.0.0", "HostKey %s/host_key" % base, "PidFile %s/sshd.pid" % base,
        "AuthorizedKeysFile %s/%s" % (keys, move_id), "PermitRootLogin no",
        "AllowUsers %s" % USER, "PasswordAuthentication no", "KbdInteractiveAuthentication no", "PubkeyAuthentication yes",
        "UsePAM yes", "AllowTcpForwarding no", "AllowAgentForwarding no", "X11Forwarding no", "PermitTTY no",
        "PermitUserEnvironment no", "MaxAuthTries 2", "LoginGraceTime 30", "ClientAliveInterval 30", "LogLevel INFO",
        ""])


def authorized_line(public, source, move_id, script):
    return 'restrict,from="%s",command="/usr/bin/sudo -n /usr/bin/python3 %s serve %s" %s\n' % (
        source, script, move_id, public)


def sudoers(script):
    """`bipmove` may run this script's `serve` as root, nothing else; SSH_ORIGINAL_COMMAND goes through."""
    return ('Defaults:%s env_keep += "SSH_ORIGINAL_COMMAND"\nDefaults:%s !requiretty\n'
            "%s ALL=(root) NOPASSWD: /usr/bin/python3 %s serve *\n" % (USER, USER, USER, script))


def account_for_moves(script, run=subprocess.run):
    """The `bipmove` system account (no password, no home of its own) and its one sudo rule."""
    try:
        pwd.getpwnam(USER)
    except KeyError:
        run(["useradd", "--system", "--shell", "/bin/sh", "--home-dir", "/nonexistent", "--no-create-home", USER],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    text = sudoers(script)
    tmp = SUDOERS + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o440)
    with os.fdopen(fd, "w") as fh:
        fh.write(text)
    if run(["visudo", "-cf", tmp], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        os.replace(tmp, SUDOERS)
    else:
        os.unlink(tmp)
        raise MoveError("the move's sudo rule was refused")


def listen(order_path, root=MOVES, script=None, run=subprocess.run):
    """`move_in`: the move's sshd on its port, for the source's address only, with the move's keys."""
    order, move = order_of(order_path)
    move_id = move["id"]
    base = folder(move_id, root)
    seat = int(order["seat"])
    source = str(move["source"]["address"])
    port = int(move.get("port") or 2222)
    if not 1024 <= port <= 65535 or port in range(443, 443 + MAX_SEATS):
        raise MoveError("the move's port is not one this server may open")
    for other in states(root):   # an older listener on the same port (a move that ended badly) gives way
        if other.get("id") != move_id and read_json(os.path.join(root, other["id"], "state.json")).get("port") == port \
                and other.get("role") == "in" and other.get("phase") not in ("closed",):
            close(other["id"], root, run)
    host_private, _ = keypair(move["key"], "host")
    _, client_public = keypair(move["key"], "client")
    write_private(os.path.join(base, "host_key"), host_private)
    script = script or os.path.join(LIB, "place-move.py")
    account_for_moves(script, run)
    os.makedirs(KEYS, mode=0o755, exist_ok=True)
    keys = os.path.join(KEYS, move_id)
    with open(keys + ".tmp", "w") as fh:
        fh.write(authorized_line(client_public, source, move_id, script))
    os.chmod(keys + ".tmp", 0o644)
    os.replace(keys + ".tmp", keys)
    write_private(os.path.join(base, "sshd_config"), sshd_config(base, port, KEYS, move_id))
    os.makedirs("/run/sshd", mode=0o755, exist_ok=True)
    save(move_id, root, role="in", seat=seat, phase="listening", started=int(time.time()), source=source, port=port,
         expires=int(move.get("expires") or time.time() + 6 * 3600))
    run(["ufw", "allow", "from", source, "to", "any", "port", str(port), "proto", "tcp"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run(["systemd-run", "--unit", "bipagents-move-%s-sshd" % move_id, "--collect", "/usr/sbin/sshd", "-D", "-e",
         "-f", os.path.join(base, "sshd_config")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    log("move %s: listening on %d for %s (place %d)" % (move_id, port, source, seat))
    return 0


def close(move_id, root=MOVES, run=subprocess.run):
    base = folder(move_id, root)
    state = read_json(os.path.join(base, "state.json"))
    run(["systemctl", "stop", "bipagents-move-%s-sshd" % move_id], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if state.get("source") and state.get("port"):
        run(["ufw", "delete", "allow", "from", str(state["source"]), "to", "any", "port", str(state["port"]), "proto",
             "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for path in (os.path.join(base, "host_key"), os.path.join(base, "order.json"), os.path.join(KEYS, move_id)):
        try:
            os.unlink(path)
        except OSError:
            pass
    if state.get("phase") not in ("done", "failed", "cancelled"):
        save(move_id, root, phase="closed")
    return 0


def allowed_rsync(words, roots):
    """rsync as the receiving end only, into one of the move's folders: its last word a token (roots/<name>[/] for
    a root named at `prepare`, or stage/), never a path; the token, else None."""
    if len(words) < 3 or words[0] != "rsync" or "--server" not in words or "--sender" in words:
        return None
    if any(w.startswith(("--rsync-path", "--log-file", "--write-devices", "--remove-source", "--temp-dir",
                         "--partial-dir", "--backup-dir", "--copy-dest", "--link-dest", "--compare-dest",
                         "--files-from", "--include-from", "--exclude-from", "--filter", "--keep-dirlinks",
                         "--copy-dirlinks", "--copy-links", "--copy-unsafe-links", "--munge-links"))
           for w in words[1:]):
        return None
    token = words[-1]
    kind, _, name = token.rstrip("/").partition("/")
    if token == "stage/" or (kind == "roots" and name in roots and token in ("roots/" + name, "roots/" + name + "/")):
        return token
    return None


def serve(move_id, command=None, root=MOVES):
    """The forced command of the move's SSH key on the target: what the source asks, checked."""
    command = command if command is not None else os.environ.get("SSH_ORIGINAL_COMMAND", "")
    state = read_json(os.path.join(folder(move_id, root), "state.json"))
    if state.get("role") != "in" or state.get("phase") in ("closed", "cancelled"):
        print(json.dumps({"error": "no move here"}))
        return 1
    entry = place(int(state["seat"]))
    words = shlex.split(command)
    if not words:
        print(json.dumps({"error": "nothing asked"}))
        return 1
    if words[0] == "rsync":
        token = allowed_rsync(words, state.get("roots") or {}) if state.get("phase") == "receiving" else None
        try:
            if token is None:
                raise MoveError("not a copy of this move")
            at, name = staged(entry.pw_dir, token, state.get("roots") or {})
        except (MoveError, OSError):
            sys.stderr.write("refused\n")
            return 1
        os.fchdir(at)   # rsync writes from the folder checked, by a name in it: never a path through the home
        os.execvp("rsync", words[:-1] + [name])
    try:
        if words[0] == "info":
            out = {"home": entry.pw_dir, "user": entry.pw_name, "uid": entry.pw_uid, "gid": entry.pw_gid,
                   "sub_uid": subids(entry.pw_name), "sub_gid": subids(entry.pw_name, "/etc/subgid"),
                   "seat": int(state["seat"])}
        elif words[0] == "prepare" and len(words) == 2:
            roots = checked_roots(json.loads(base64.b64decode(words[1]).decode()).get("roots"))
            if state.get("roots") not in (None, roots):
                raise MoveError("the move's folders changed")
            save(move_id, root, phase="receiving", roots=roots)
            out = as_user(entry, ["move-in", "prepare"], timeout=300)
            set_aside(entry.pw_dir, roots)   # after the place's own look at its fresh files
        elif words[0] == "finish" and len(words) == 2:
            asked = json.loads(base64.b64decode(words[1]).decode())
            out = finish(move_id, entry, asked, root)
        elif words[0] == "abort":
            out = abort(move_id, entry, root)
        else:
            out = {"error": "not a move step"}
    except (MoveError, OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        save(move_id, root, phase="failed", error=str(exc)[:300])
        out = {"error": str(exc)[:300]}
    print(json.dumps(out), flush=True)
    return 0 if "error" not in out else 1


def finish(move_id, entry, asked, root=MOVES):
    """The target's switch: owners shifted, settings adjusted, started, checked, tasks and routines resumed."""
    save(move_id, root, phase="starting")
    home = entry.pw_dir
    dst = {"uid": entry.pw_uid, "gid": entry.pw_gid, "sub_uid": subids(entry.pw_name),
           "sub_gid": subids(entry.pw_name, "/etc/subgid")}
    roots = checked_roots(read_json(os.path.join(folder(move_id, root), "state.json")).get("roots"))
    shifted_count = shift_staged(home, asked["source"], dst)   # still root's alone: then into the home
    put_back(home, roots, entry.pw_uid, entry.pw_gid)
    out = as_user(entry, ["move-in", "finish"], timeout=900)
    deadline = time.time() + 120
    while True:
        try:
            bridge(home, "GET", "/v1/place/move", timeout=10)
            break
        except (OSError, ValueError) as exc:
            if time.time() > deadline:
                raise MoveError("the bridge did not start: %s" % type(exc).__name__)
            time.sleep(1)
    waiting = gateways_answer(home)
    if waiting:
        raise MoveError("%d agent(s) did not answer" % len(waiting))
    resumed = bridge(home, "POST", "/v1/place/move/resume", {}, timeout=120)
    status = bridge(home, "GET", "/v1/place/move", timeout=60)
    enabled = [r for r in status.get("routines", []) if r.get("enabled") and r.get("next_run_at")]
    if len(enabled) < int(asked.get("routines", 0)):
        raise MoveError("%d of %d routines scheduled" % (len(enabled), int(asked.get("routines", 0))))
    verified = {"bridge": True, "agents": True, "routines": len(enabled),
                "tasks_resumed": len(resumed.get("resumed_jobs", [])), "caught_up": len(resumed.get("caught_up", [])),
                "owners_shifted": shifted_count, "volumes": len(out.get("volumes", []))}
    save(move_id, root, phase="done", verified=verified, result="moved")
    later_close(move_id)
    return {"ok": True, "verified": verified}


def abort(move_id, entry, root=MOVES):
    """The move failed or was called off: the target's copy stopped (its account is reset by the guichet later)."""
    try:
        as_user(entry, ["move-out", "stop"], timeout=300)
    except MoveError:
        pass
    save(move_id, root, phase="cancelled", result="aborted")
    later_close(move_id)
    return {"ok": True}


def later_close(move_id):
    """The listener closed a minute later: this very command runs inside it."""
    subprocess.run(["systemd-run", "--on-active=60", "--collect", "/usr/bin/python3",
                    os.path.join(LIB, "place-move.py"), "close", move_id],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


# -- the source: the move itself ------------------------------------------------------------------------------------------

class Channel:
    """The source's end of the move's SSH channel."""

    def __init__(self, base, address, port, key):
        self.address, self.port = address, port
        client_private, _ = keypair(key, "client")
        _, host_public = keypair(key, "host")
        self.key_file = os.path.join(base, "client_key")
        self.known = os.path.join(base, "known_hosts")
        write_private(self.key_file, client_private)
        write_private(self.known, "[%s]:%d %s\n" % (address, port, host_public))

    def ssh_args(self):
        return ["ssh", "-p", str(self.port), "-i", self.key_file, "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
                "-o", "UserKnownHostsFile=" + self.known, "-o", "StrictHostKeyChecking=yes",
                "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ConnectTimeout=15", "-o", "ServerAliveInterval=15",
                "-o", "LogLevel=ERROR"]

    def ask(self, *words, timeout=900):
        done = subprocess.run(self.ssh_args() + ["%s@%s" % (USER, self.address), " ".join(words)], capture_output=True,
                              text=True, timeout=timeout)
        for line in reversed(done.stdout.strip().splitlines()):
            if line.startswith("{"):
                result = json.loads(line)
                if result.get("error"):
                    raise MoveError("target: " + str(result["error"])[:300])
                return result
        raise MoveError("the target did not answer %s (%d): %s" % (words[0], done.returncode,
                                                                   done.stderr.strip()[-200:]))

    def rsync(self, home, rel, dest, rules=None, checksum=False, timeout=6 * 3600):
        """One copy of the place's folder (`rel` ending in /) or file `rel` to the move's folder `dest` on the target
        (roots/<name>, stage); (bytes sent of changed files, files changed). Read through a view of the place's
        home in which no link is followed (`VIEW`): a link swapped in on the way, or for the root itself, fails the
        copy instead of leading root's rsync out of the home."""
        view = os.path.join(os.path.dirname(self.key_file), "view")
        os.makedirs(view, mode=0o700, exist_ok=True)
        args = ["rsync", "-aH", "--no-devices", "--numeric-ids", "--delete", "--partial", "--stats", "-z",
                "-e", " ".join(shlex.quote(a) for a in self.ssh_args())]
        if checksum:
            args.append("--checksum")
        rules_file = None
        if rules:
            fd, rules_file = tempfile.mkstemp(prefix="rules-", dir=os.path.dirname(self.key_file))
            with os.fdopen(fd, "w") as fh:
                fh.write("\n".join(rules) + "\n")
            args.append("--filter=merge " + rules_file)
        is_dir = rel.endswith("/")
        args += [os.path.join(view, rel.strip("/")) + ("/" if is_dir else ""),
                 "%s@%s:%s" % (USER, self.address, dest + ("/" if is_dir else ""))]
        command = VIEW + ["/bin/sh", "-c", VIEW_SCRIPT, "place-move-view", home, view] + args
        try:
            done = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
        finally:
            if rules_file:
                os.unlink(rules_file)
        if done.returncode not in (0, 24):   # 24: files vanished while copied (a running place)
            raise MoveError("rsync of %s failed (%d): %s" % (os.path.basename(rel.rstrip("/")), done.returncode,
                                                             done.stderr.strip()[-300:]))
        return stats(done.stdout)


def stats(output):
    def number(label):
        found = re.search(r"^%s:\s*([\d,]+)" % re.escape(label), output, re.M)
        return int(found.group(1).replace(",", "")) if found else 0
    return number("Total transferred file size"), number("Number of regular files transferred")


def copy_round(channel, plan, home, checksum=False, with_stage=False):
    """One copy of each root of the checked plan (`checked_plan`), looked at again first: the place's own code runs
    while it is copied, and a root that became a link (or has one on its way) stops the move."""
    sent = files = 0
    for name, rel in sorted(plan["roots"].items()):
        kind = home_kind(home, rel)
        if kind is None:
            continue
        size, count = channel.rsync(home, rel + ("/" if kind == "dir" else ""), "roots/" + name,
                                    plan["rules"].get(name), checksum=checksum)
        sent += size
        files += count
    if with_stage:
        if home_kind(home, STAGE) != "dir":
            raise MoveError("the place's stage is gone")
        size, count = channel.rsync(home, STAGE + "/", "stage", checksum=True)
        sent += size
        files += count
    return sent, files


def quiet_moment(home, start, end, keep_copying, now=time.time, sleep=time.sleep):
    """Waits for the window, then for a moment with no turn running (at the latest LATEST_BEFORE_END before its
    end). `keep_copying` is called every ROUND_EVERY meanwhile. Returns the bridge's status then."""
    last_round = now()
    while True:
        status = {}
        if now() >= start:
            try:
                status = bridge(home, "GET", "/v1/place/move", timeout=60)
            except (OSError, ValueError):
                status = {}
            if int(status.get("turns_running", 1)) == 0 or now() >= end - LATEST_BEFORE_END:
                return status
        if now() - last_round >= ROUND_EVERY:
            keep_copying()
            last_round = now()
        sleep(5)


def drained(home, timeout=DRAIN_WAIT, now=time.time, sleep=time.sleep):
    """The running turns ended and the running tasks paused (or the time is up). Returns the last status."""
    deadline = now() + timeout
    status = {}
    while now() < deadline:
        status = bridge(home, "GET", "/v1/place/move", timeout=60)
        if int(status.get("turns_running", 0)) == 0 and not status.get("jobs_running"):
            return status
        sleep(2)
    return status


def move_out(order_path, root=MOVES):
    order, move = order_of(order_path)
    move_id = move["id"]
    base = folder(move_id, root)
    seat = int(order["seat"])
    entry = place(seat)
    home = entry.pw_dir
    now = time.time()
    window = move.get("window") or {}
    start = float(window.get("start") or now)
    end = float(window.get("end") or start + AUTO_WINDOW)
    back_in = int(move.get("back_in") or 60)
    save(move_id, root, role="out", seat=seat, phase="connecting", started=int(now), round=0,
         drain=move.get("drain") is not False)
    target = move["target"]
    channel = Channel(base, str(target["address"]), int(target.get("port") or 2222), move["key"])
    deadline = min(end, now + 3600)
    while True:
        try:
            info = channel.ask("info", timeout=60)
            break
        except (MoveError, subprocess.SubprocessError, ValueError) as exc:
            if time.time() > deadline:
                raise MoveError("the target never answered: %s" % str(exc)[:200])
            save(move_id, root, error="target not answering yet: %s" % str(exc)[:200])
            time.sleep(15)
    save(move_id, root, error=None)
    # the place's plan, from its own account: followed only within its real home, by paths there (review 26/11, E1)
    plan = checked_plan(as_user(entry, ["move-out", "plan"], timeout=300), home)
    channel.ask("prepare", base64.b64encode(json.dumps({"roots": plan["roots"]}).encode()).decode(), timeout=300)
    if move.get("drain") is False:
        return stopped_out(move_id, root, base, entry, channel, plan, info, back_in)
    save(move_id, root, phase="copying")
    # 1. Copies while the agents work, until little changes between two.
    total, rounds, delta = 0, 0, None
    while rounds < MAX_ROUNDS:
        rounds += 1
        sent, files = copy_round(channel, plan, entry.pw_dir)
        total += sent
        delta = sent
        save(move_id, root, round=rounds, copied_bytes=total, delta_bytes=delta, files=files)
        log("move %s: copy %d, %d bytes in %d files" % (move_id, rounds, sent, files))
        if rounds > 1 and sent <= SMALL_DELTA:
            break

    def keep_copying():
        sent, files = copy_round(channel, plan, entry.pw_dir)
        save(move_id, root, round=read_json(os.path.join(base, "state.json")).get("round", 0) + 1,
             delta_bytes=sent, files=files)
    # 2. The quiet moment.
    save(move_id, root, phase="waiting")
    before = quiet_moment(home, start, end, keep_copying)
    routines = len([r for r in before.get("routines", []) if r.get("enabled") and r.get("next_run_at")])
    # 3. Drain. A bridge that stops answering here (it went down meanwhile) fails the move: the place resumes.
    t_drain = time.time()
    save(move_id, root, phase="draining")
    try:
        bridge(home, "POST", "/v1/place/move/drain", {"until": time.time() + 900, "back_in": back_in}, timeout=120)
        copy_round(channel, plan, entry.pw_dir)
        drained(home)
        bridge(home, "POST", "/v1/place/move/drain", {"switching": True, "back_in": back_in}, timeout=120)
    except Exception as exc:
        given_up(move_id, root, channel, entry, exc, stopped=False)
        raise
    # 4. Switch.
    t_switch = time.time()
    save(move_id, root, phase="switching", drain_seconds=round(t_switch - t_drain, 1))
    result = switch(move_id, root, channel, entry, plan, info, routines, back_in)
    return moved(move_id, root, base, home, t_switch, result)


def switch(move_id, root, channel, entry, plan, info, routines, back_in, stopped=False, drained_first=True):
    """Step 4: the place stopped (unless it already is), its last changes copied with checksums, the target asked to
    start it. Any failure rolls back (the target's copy stopped, the source as it was) and is raised again."""
    try:
        if not stopped:
            as_user(entry, ["move-out", "stop"], timeout=300)
            stopped = True
        copy_round(channel, plan, entry.pw_dir, checksum=True, with_stage=True)
        source = {"uid": entry.pw_uid, "gid": entry.pw_gid, "sub_uid": subids(entry.pw_name),
                  "sub_gid": subids(entry.pw_name, "/etc/subgid")}
        asked = base64.b64encode(json.dumps({"source": source, "routines": routines,
                                             "back_in": back_in}).encode()).decode()
        return channel.ask("finish", asked, timeout=1200)
    except Exception as exc:
        given_up(move_id, root, channel, entry, exc, stopped, drained_first)
        raise


def moved(move_id, root, base, home, t_switch, result):
    """Step 5: the place runs on the target; this copy is marked moved away and kept, stopped, seven days."""
    downtime = round(time.time() - t_switch, 1)
    try:   # root's mark, in the place's folder: never through a link put there (review 26/11)
        home_mark(home, MOVED_AWAY, json.dumps({"move": move_id, "at": int(time.time())}))
    except OSError as exc:
        log("move %s: the moved-away mark not written (%s)" % (move_id, type(exc).__name__))
    save(move_id, root, phase="done", result="moved", downtime_seconds=downtime, verified=result.get("verified"),
         kept_until=int(time.time() + KEEP_SECONDS))
    for name in ("client_key", "known_hosts", "order.json"):
        try:
            os.unlink(os.path.join(base, name))
        except OSError:
            pass
    log("move %s: done, %.1f s without the agents" % (move_id, downtime))
    return 0


# What runs a place and writes its files: its bridge, its agents' Hermes gateways (words of their command lines).
PLACE_PROGRAMS = (("bipbridge",), ("hermes", "gateway"))


def still_running(uid, proc="/proc"):
    """Which of PLACE_PROGRAMS still run as the place's account (their names, nothing more, in PLACE_PROGRAMS'
    order)."""
    found = []
    try:
        pids = os.listdir(proc)
    except OSError:
        return found
    for pid in pids:
        if not pid.isdigit():
            continue
        try:
            if os.stat(os.path.join(proc, pid)).st_uid != uid:
                continue
            with open(os.path.join(proc, pid, "cmdline"), "rb") as fh:
                command = fh.read().replace(b"\0", b" ").decode("utf-8", "replace")
        except OSError:
            continue
        for words in PLACE_PROGRAMS:
            if all(word in command for word in words) and " ".join(words) not in found:
                found.append(" ".join(words))
    return [" ".join(words) for words in PLACE_PROGRAMS if " ".join(words) in found]


def made_stopped(entry, proc="/proc", run=subprocess.run, sleep=time.sleep, wait=10):
    """The place's bridge and gateways not running any more: what `move-out stop` left running is killed (its
    services are disabled by then: nothing starts them again). Raises when something still runs."""
    left = still_running(entry.pw_uid, proc)
    if left:
        for words in PLACE_PROGRAMS:
            if " ".join(words) in left:
                run(["pkill", "-KILL", "-u", entry.pw_name, "-f", ".*".join(words)], stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL)
        waited = 0
        while left and waited < wait:
            sleep(1)
            waited += 1
            left = still_running(entry.pw_uid, proc)
    if left:
        raise MoveError("the place still runs: %s" % ", ".join(left))


def stopped_out(move_id, root, base, entry, channel, plan, info, back_in, proc="/proc", run=subprocess.run):
    """`"drain": false`, a place whose bridge is down (nothing to drain, nobody to ask): the place made sure to be
    stopped, its files copied as they are, then the same switch as a drained move. No window, no quiet moment: it is
    down already. Its routines aren't counted (its bridge can't say them): the target checks its bridge and agents."""
    t_switch = time.time()
    save(move_id, root, phase="stopping")
    stopped = False
    try:
        as_user(entry, ["move-out", "stop"], timeout=300)
        stopped = True
        made_stopped(entry, proc, run)
        save(move_id, root, phase="copying")
        sent, files = copy_round(channel, plan, entry.pw_dir)
        save(move_id, root, round=1, copied_bytes=sent, delta_bytes=sent, files=files)
        log("move %s: the stopped place copied, %d bytes in %d files" % (move_id, sent, files))
    except Exception as exc:
        given_up(move_id, root, channel, entry, exc, stopped, drained_first=False)
        raise
    save(move_id, root, phase="switching", drain_seconds=0)
    result = switch(move_id, root, channel, entry, plan, info, 0, back_in, stopped=True, drained_first=False)
    return moved(move_id, root, base, entry.pw_dir, t_switch, result)


def reason(exc):
    """A failure's words for the beat: the move's own, else the error's kind and first words (urllib and subprocess
    errors say a code, an address, a command's name: no key, nothing of the person)."""
    if isinstance(exc, MoveError):
        return str(exc)[:300]
    return ("%s: %s" % (type(exc).__name__, exc))[:300]


def given_up(move_id, root, channel, entry, exc, stopped, drained_first=True):
    """The move failed once the place was touched: rolled back, then said `failed`."""
    rollback(channel, entry, stopped, drained_first)
    save(move_id, root, phase="failed", error=reason(exc), result="rolled back")


def rollback(channel, entry, stopped, drained_first=True):
    """The switch failed: the target's copy stopped, the source as it was (its services, its tasks, its routines)."""
    try:
        channel.ask("abort", timeout=300)
    except Exception:
        pass
    if stopped:
        try:
            as_user(entry, ["resume"], timeout=300)
        except Exception:
            pass   # `bipinstall resume` prints words, not JSON: it ran
    if not drained_first:
        return   # nothing drained: no bridge to resume (it was down)
    deadline = time.time() + 120
    while time.time() < deadline:
        try:
            bridge(entry.pw_dir, "POST", "/v1/place/move/resume", {}, timeout=120)
            return
        except (OSError, ValueError):
            time.sleep(2)


FINAL = ("done", "failed", "cancelled", "removed")


def out(order_path, root=MOVES, mover=None):
    """`out`, the unit's whole run: any failure (a bridge gone down, the target gone, a bug) ends in `failed` with a
    short reason in the beat, never in a phase the guichet waits on, and the unit exits cleanly."""
    try:
        return (mover or move_out)(order_path, root)
    except Exception as exc:
        log("move failed: %s" % reason(exc))
        try:
            move_id = order_of(order_path)[1]["id"]
            if read_json(os.path.join(folder(move_id, root), "state.json")).get("phase") not in FINAL:
                save(move_id, root, phase="failed", error=reason(exc), result="not moved")
        except Exception as again:   # no move to say it of (the order unreadable)
            log("move state not written: %s" % reason(again))
        return 1


def cancel(move_id, root=MOVES):
    base = folder(move_id, root)
    state = read_json(os.path.join(base, "state.json"))
    subprocess.run(["systemctl", "stop", "bipagents-move-%s" % move_id], stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    if state.get("role") == "in":
        return close(move_id, root)
    if state.get("role") == "out" and state.get("phase") not in ("done", "failed"):
        entry = place(int(state["seat"]))
        stopped_here = state.get("drain") is False and state.get("phase") in ("stopping", "copying")
        if state.get("phase") in ("draining", "switching") or stopped_here:
            try:
                as_user(entry, ["resume"], timeout=300)
            except MoveError:
                pass
            try:
                bridge(entry.pw_dir, "POST", "/v1/place/move/resume", {}, timeout=120)
            except (OSError, ValueError):
                pass
        save(move_id, root, phase="cancelled")
    return 0


def cleanup(root=MOVES, now=None, wipe=None):
    """Listeners past their time closed; source copies kept 7 days, then their account removed (only when it still
    holds that move's mark: a seat installed again for someone else since is never touched)."""
    now = time.time() if now is None else now
    removed = []
    for state in states(root):
        move_id = state.get("id")
        full = read_json(os.path.join(root, str(move_id), "state.json"))
        if full.get("role") == "in" and full.get("phase") not in ("closed", "cancelled") \
                and now > float(full.get("expires") or 0):
            close(move_id, root)
        if full.get("role") == "out" and full.get("phase") == "done" and now > float(full.get("kept_until") or now + 1):
            try:
                entry = place(int(full["seat"]))
                mark = home_json(entry.pw_dir, MOVED_AWAY)
            except (KeyError, ValueError):
                mark = {}
            if mark.get("move") == move_id and wipe:
                wipe(entry.pw_name)
                removed.append(entry.pw_name)
            save(move_id, root, phase="removed")
    return removed


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    what, rest = argv[0], argv[1:]
    try:
        if what == "in" and rest:
            return listen(rest[0])
        if what == "out" and rest:
            return out(rest[0])
        if what == "serve" and rest:
            return serve(rest[0])
        if what == "cancel" and rest:
            return cancel(rest[0])
        if what == "close" and rest:
            return close(rest[0])
        if what == "states":
            print(json.dumps(states()))
            return 0
    except MoveError as exc:
        log("refused: %s" % exc)
        return 1
    print("usage: place-move.py in|out <order file> | serve|cancel|close <id> | states", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
