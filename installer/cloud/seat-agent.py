#!/usr/bin/env python3
"""BipAgents: the guichet's orders for the places of this server (guichet/bipguichet/orders.py).

Run as root every two minutes by bipagents-seats.timer, set up at first boot. Its call to the guichet, with the token
its first boot gave it, is the server's heartbeat (guichet/bipguichet/watch.py): it says, for each place, whether its
bridge's door answers here, whether its Hermes runs (or sleeps), the BipAgents version it runs, when its last backup worked
and whether its encrypted space waits for its key ("locked" and why), whether its vault's sealed keys are open or
closed (and why) or its last sealing failed (the `vault_keys` order, below),
and for the server its memory, swap, disk and load, and for each place the memory it holds now and its highest since the
last beat ("places") -- yes or no, a version, a time and numbers, nothing of anyone. The
guichet answers with each place's AI credit state ("credit": ok, low, full, stopped; written where the place's bridge
reads it, which pauses its routines, browser and background tasks while the month's credit is used), whether each
place's agents may pay by card ("offer_cards": Kerfy Studio's remote switch, `bipguichet cards off`; a false written
for the place's bridge in OFFER_CARDS_DIR, which stops card payments there) and its orders, and the agent does one only when the guichet's Ed25519 key signed it (checked with
openssl against /etc/bipagents/guichet.pub), for this server, not expired, not done before. There are three orders:
install place n again for its next person -- its account bip<n> (bip for place 1), everything in it and its
services go, then installer/cloud/add-seat.sh sets it up again with the new pairing secret; bring place n to a
given BipAgents version (the guichet's update waves, or their way back), which its own updater does; and let place n
pair again with a new secret (its person recovered their account without any of their devices). The other places
are not touched, and nothing is ever read or sent back: only "done".

With --bootstrap, run once by the server's first-boot script (the guichet's guichet-rs/src/bootstrap.rs), before any
place is installed: the script, which the provider keeps and serves to anyone on the server through its metadata
service, holds no secret, only a one-time token. With it root fetches from the guichet, over HTTPS and from this
server's own address, each place's setup (its pairing secret, its backups' token, its key service grant) and the seat
agent's own token; the guichet then forgets the one-time token (pool.rs: bootstrap). The setups go to
/etc/bipagents/cloud.json (place 1) and cloud<n>.json (root's, 0600), which add-seat.sh and the installer read then
shred. A place installed again later gets its setup in a signed order (above), never through the first-boot script.

With --rebooting, run by bipagents-reboot-notice.service as the server shuts down (the night's reboot after a kernel
update, user-data.yaml), it only tells the guichet the server is rebooting when a reboot is under way: the guichet
then waits a quarter of an hour before taking its silence, or its places not answering yet, for an outage. The beat
says how long the server has been up for the same reason.

It also keeps each place's memory and CPU soft-capped (its user slice's MemoryHigh and CPUWeight, a MemoryMax only as a
last-resort safety: below), gives each place a disk of its own of 5 GB and watches it (below), samples each place's
memory every 15 s between beats (--sample, bipagents-memory.timer, which it sets up), and sets up zram on a server
that has none.

ASCII only, like user-data.yaml and add-seat.sh (installer/tests/test_cloud_init.py checks it).
"""
import base64
import contextlib
import json
import os
import pwd
import shutil
import socket
import stat as stat_mod
import subprocess
import sys
import tempfile
import threading
import time
import http.client
import urllib.error
import urllib.request

CONF = "/etc/bipagents/guichet.json"
PUBLIC_KEY = "/etc/bipagents/guichet.pub"
DONE = "/var/lib/bipagents/orders-done"
LIB = "/usr/local/lib/bipagents"
# Places one server may hold at most: doors 443 to 482, accounts bip, bip2 ... bip40. How many a server really holds
# comes from its size, decided by the guichet (pool.rs: its density rule, at most 40).
MAX_SEATS = 40
BACKUP_STATE = ".local/state/bipagents/backup.json"
BACKUP_CHECK = ".local/state/bipagents/backup-check.json"   # its backup timer's looks (backup.py: check)
# Every place of this server locks it around its backups: one at a time (backup.py: one_at_a_time). Root's, readable
# by all (flock needs no more).
BACKUP_LOCK = "/var/lib/bipagents/backup.lock"
ASLEEP = ".local/state/bipagents/asleep"   # its bridge put its Hermes to sleep (bridge-rs/src/sleep.rs)
USED = ".local/state/bipagents/used"       # its last person action, epoch seconds (bridge-rs/src/used.rs)
SETUP = ".config/bipagents/setup.json"     # its bridge's pairing secret while it waits for an app (cloud_setup.py)
LOCKED = ".local/share/bipagents/space/locked.json"   # its encrypted space not open yet, and why (space.rs)
CREDIT = ".local/state/bipagents/credit.json"   # its plan's AI credit as the guichet says it (bridge-rs/src/credit.rs)
CREDIT_STATES = ("ok", "low", "full", "stopped")
# Kerfy Studio's remote switch for the agents' card payments (guichet-rs/src/offer_cards.rs, bridge-rs/src/cards.rs
# `remote_off`): <uid>.json per place, root's, outside the places' homes (nothing of the place can remove it, never in
# its backups nor moved with it, kept across reboots)
OFFER_CARDS_DIR = "/var/lib/bipagents/offer-cards"
HARDENED = "/var/lib/bipagents/space-hardened"   # space-hardening.sh was done (user-data.yaml, add-seat.sh)
VAULT_TRIED = "/var/lib/bipagents/vault-keys-tried"   # the vault_keys orders tried: each place enrolled once at most
VAULT_STATE = ".local/state/bipagents/vault-keys.json"   # the place's own last try (installer/bipinstall/vault_keys.py)
SEALED_KEYS = "keys.sealed.json"   # in its bridge's data_dir: its vault's keys sealed (bridge-rs/src/sealed_keys.rs)
PAIR_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"


def log(text):
    print(text, flush=True)


def verified(body, signature, public_key=PUBLIC_KEY):
    """The guichet's signature over the order's exact text."""
    try:
        raw = base64.b64decode(signature, validate=True)
    except (ValueError, TypeError):
        return False
    with tempfile.TemporaryDirectory() as folder:
        body_path, sig_path = os.path.join(folder, "body"), os.path.join(folder, "sig")
        with open(body_path, "wb") as fh:
            fh.write(body.encode("utf-8"))
        with open(sig_path, "wb") as fh:
            fh.write(raw)
        result = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", public_key, "-rawin",
                                 "-in", body_path, "-sigfile", sig_path], capture_output=True)
    return result.returncode == 0


def accepted(order, server, done, now):
    """The order's content when it is one this server must do, else None (with why in the log)."""
    try:
        body = json.loads(order["body"])
        number = int(body["seat"])
        ok = (body.get("server") == server and int(body["order"]) == int(order["id"]) and 1 <= number <= MAX_SEATS
              and int(body["expires"]) > now)
        if body.get("action") == "reset":
            setup = body["setup"]
            ok = ok and isinstance(setup, dict) and isinstance(setup.get("pair"), str) \
                and int(setup.get("port", 0)) == 442 + number
        elif body.get("action") == "update":
            commit = body["commit"]
            ok = ok and isinstance(commit, str) and len(commit) == 40 and all(c in "0123456789abcdef" for c in commit)
            ok = ok and body.get("repo") in (None,) + KNOWN_REPOS   # where the commit lives: a known one, or unsaid
            ok = ok and body.get("pretest") in (None, True, False)   # a pre-test place's (below: update)
        elif body.get("action") == "reopen":
            pair = body["pair"]
            ok = ok and isinstance(pair, str) and 40 <= len(pair) <= 64 and all(c in PAIR_CHARS for c in pair)
        elif body.get("action") == "vault_keys":
            ok = ok and (key_service_valid(body["vault_keys"]) or body["vault_keys"] == {"reseal": True})
        elif body.get("action") in MOVE_ACTIONS:
            ok = ok and move_valid(body)
        elif body.get("action") == "remove":
            ok = ok and remove_valid(body)
        else:
            ok = False
    except (KeyError, TypeError, ValueError):
        ok = False
    if not ok:
        log("order %s refused: not for this server, expired or malformed" % order.get("id"))
        return None
    if int(order["id"]) in done:
        return None
    return body


def account(number):
    return "bip" if number == 1 else "bip%d" % number


def run(*args):
    subprocess.run(list(args), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def wipe(user):
    """The account, its services, its sandboxes and every file of it."""
    try:
        uid = pwd.getpwnam(user).pw_uid
    except KeyError:
        return
    run("loginctl", "disable-linger", user)
    run("systemctl", "stop", "user@%d.service" % uid)
    run("pkill", "-KILL", "-u", user)
    time.sleep(2)
    drop_view(user)
    drop_disk(user)
    run("userdel", "--remove", "--force", user)
    for path in ("/etc/subuid", "/etc/subgid"):
        try:
            with open(path) as fh:
                lines = [line for line in fh if not line.startswith(user + ":")]
            with open(path, "w") as fh:
                fh.writelines(lines)
        except OSError:
            pass
    run("rm", "-rf", "/home/" + user)


IMAGE_VIEWS = "/var/lib/bipagents/views"   # each account's view of the server's shared images (shared_images.py)


def drop_view(user, views=IMAGE_VIEWS, mountinfo="/proc/self/mountinfo", runner=None):
    """The account's view of the server's shared images unmounted with it: it maps the store to this account's ids,
    and a place installed again under the same name gets a new account (another uid), which an old view shows the
    images as nobody's (its install then fell back to its own copy, 2.3 GB of its 5: real-server test, 2026-10-31).
    The next `shared_images views` (the place's --prepare) mounts the new account's."""
    for point in reversed(mounted_at(os.path.join(views, user), mountinfo)):
        (runner or quietly)("umount", "-l", point)


def views_again(repo=None):
    """Each account's view of the shared images mounted (or mounted again when it is an earlier account's), by root's
    checkout: after a place's --prepare, whatever its pull did."""
    repo = repo or SHARED_REPO
    if not os.path.isdir(os.path.join(repo, "installer", "bipinstall")):
        return
    env = dict(os.environ, PYTHONPATH=os.path.join(repo, "installer"))
    with open("/var/log/bipagents-images.log", "a") as out:
        subprocess.run(["python3", "-m", "bipinstall.shared_images", "views"], env=env, stdout=out, stderr=out,
                       timeout=600, cwd=repo)


def reset(number, setup):
    user = account(number)
    log("place %d (%s): installed again" % (number, user))
    wipe(user)
    path = "/etc/bipagents/cloud%d.json" % number
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(setup, fh)
    subprocess.run(["bash", os.path.join(LIB, "add-seat.sh"), str(number), path])


SHARED_SETTING = "/etc/bipagents/shared-hermes.json"
SHARED_REPO = os.path.join(LIB, "repo")   # root's own BipAgents, never a place's (an agent could change those)
# A pre-test's checkout of root's, apart: a pre-test place's shared Hermes is made from it, and root's own checkout
# (the images' nightly pull, the seat tools) stays on production's version until the production waves (review R11)
PRETEST_REPO = os.path.join(LIB, "repo-pretest")
PUBLIC_REPO = "https://github.com/BipAgents/BipAgents.git"
# The public repositories an update order may name (installer/bipinstall/public_repo.py: KNOWN): the guichet moves the
# places to the BipAgents organisation's through an update, and back; never anywhere else, whatever an order says.
KNOWN_REPOS = ("https://github.com/Alexbob0/BipAgents.git",  # known-public-repos
               "https://github.com/BipAgents/BipAgents.git")  # known-public-repos


def shared_on():
    """The server's places share one read-only Hermes (installer/bipinstall/shared_hermes.py): on unless root wrote
    {"enabled": false} there; the first round writes the setting."""
    try:
        with open(SHARED_SETTING) as fh:
            return json.load(fh).get("enabled") is not False
    except OSError:
        os.makedirs(os.path.dirname(SHARED_SETTING), exist_ok=True)
        with open(SHARED_SETTING, "w") as fh:
            json.dump({"enabled": True, "root": "/opt/bipagents/hermes"}, fh)
        os.chmod(SHARED_SETTING, 0o644)
        return True
    except ValueError:
        return False


def prepare_pretest(commit, repo=None):
    """A pre-test place's version made ready without touching what the server's other places share: its shared
    Hermes (one per version, beside production's) built from a checkout of its own (PRETEST_REPO); root's checkout,
    the seat tools and the images' store stay as production has them. The place itself gets its browser from its own
    pull when the server's store doesn't have its image (shared_images.browser)."""
    if not os.path.isdir(os.path.join(PRETEST_REPO, ".git")):
        source = SHARED_REPO if os.path.isdir(os.path.join(SHARED_REPO, ".git")) else (repo or PUBLIC_REPO)
        subprocess.run(["git", "clone", "--quiet", "--filter=blob:none", source, PRETEST_REPO], check=True,
                       timeout=600)
    subprocess.run(["git", "-C", PRETEST_REPO, "remote", "set-url", "origin", repo if repo in KNOWN_REPOS
                    else PUBLIC_REPO], check=True, timeout=60)
    subprocess.run(["git", "-C", PRETEST_REPO, "fetch", "--quiet", "--prune", "origin"], check=True, timeout=600)
    subprocess.run(["git", "-C", PRETEST_REPO, "-c", "advice.detachedHead=false", "checkout", "--quiet", "--force",
                    commit], check=True, timeout=120)
    if shared_on():
        env = dict(os.environ, PYTHONPATH=os.path.join(PRETEST_REPO, "installer"))
        with open("/var/log/bipagents-shared-hermes.log", "a") as out:
            done = subprocess.run(["python3", "-m", "bipinstall.shared_hermes", "build"], env=env, stdout=out,
                                  stderr=out, timeout=3600, cwd=PRETEST_REPO)
        log("pre-test's shared Hermes for %s: %s" % (commit[:12], "ready" if done.returncode == 0 else "not built"))


def prepare_shared(commit, repo=None):
    """The shared Hermes that BipAgents `commit` pins, built by root before a place moves to it (its own update
    then points at it). Built from root's own checkout at that commit. A failure leaves the place on its current
    Hermes: its update says the version isn't ready and it keeps the one it has. Usually fetched ready-made (its
    release), not built. The agents' images of that version are pulled into the server's store too
    (installer/bipinstall/shared_images.py), with root's nightly pull set up the first time. `commit` "main": the
    latest (a place installed without a pinned version, add-seat.sh): `stable` where the repository has it, else
    `main`. `repo` (one of KNOWN_REPOS): the public repository the commit lives in, root's checkout moved there."""
    if not os.path.isdir(os.path.join(SHARED_REPO, ".git")):
        subprocess.run(["git", "clone", "--quiet", "--filter=blob:none", repo or PUBLIC_REPO, SHARED_REPO], check=True,
                       timeout=600)
    elif repo in KNOWN_REPOS:
        subprocess.run(["git", "-C", SHARED_REPO, "remote", "set-url", "origin", repo], check=True, timeout=60)
    subprocess.run(["git", "-C", SHARED_REPO, "fetch", "--quiet", "--prune", "origin"], check=True, timeout=600)
    if commit == "main":
        stable = subprocess.run(["git", "-C", SHARED_REPO, "rev-parse", "--verify", "--quiet", "origin/stable"],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
        commit = "origin/stable" if stable.returncode == 0 else "origin/main"
    subprocess.run(["git", "-C", SHARED_REPO, "-c", "advice.detachedHead=false", "checkout", "--quiet", "--force",
                    commit], check=True, timeout=120)
    env = dict(os.environ, PYTHONPATH=os.path.join(SHARED_REPO, "installer"))
    if shared_on():
        with open("/var/log/bipagents-shared-hermes.log", "a") as out:
            done = subprocess.run(["python3", "-m", "bipinstall.shared_hermes", "build"], env=env, stdout=out,
                                  stderr=out, timeout=3600, cwd=SHARED_REPO)
        log("shared Hermes for %s: %s" % (commit[:12], "ready" if done.returncode == 0 else "not built"))
    # --timer the first time: the nightly pull, and the places' views of the store mounted again at each boot.
    timer = [] if os.path.exists("/etc/systemd/system/bipagents-image-views.service") else ["--timer"]
    with open("/var/log/bipagents-images.log", "a") as out:
        done = subprocess.run(["python3", "-m", "bipinstall.shared_images", "pull"] + timer, env=env, stdout=out,
                              stderr=out, timeout=3600, cwd=SHARED_REPO)
    # (that pull also brings this agent and its scripts to the version, never back: bipinstall/server_tools.py)
    log("shared images for %s: %s" % (commit[:12], "ready" if done.returncode == 0 else "not all pulled"))


TOOLS = ("seat-agent.py", "add-seat.sh", "place-move.py")


def refresh_tools(repo=SHARED_REPO, lib=LIB):
    """This agent and the seat scripts taken from root's checkout once it is at the version the guichet's wave pins
    (`prepare_shared`): servers installed before get the new ones with their next update, as their places do. Root
    already runs that checkout's own code (shared_hermes build), so nothing new is trusted. The new agent runs from
    the next round."""
    for name in TOOLS:
        source, target = os.path.join(repo, "installer", "cloud", name), os.path.join(lib, name)
        try:
            with open(source, "rb") as fh:
                new = fh.read()
        except OSError:
            continue
        try:
            with open(target, "rb") as fh:
                if fh.read() == new:
                    continue
        except OSError:
            pass
        with open(target + ".tmp", "wb") as fh:
            fh.write(new)
        os.chmod(target + ".tmp", 0o755)
        os.replace(target + ".tmp", target)
        log("%s taken from BipAgents' checkout" % name)


def shared_gc():
    """The shared versions no place points at and no process runs any more, deleted."""
    if not shared_on() or not os.path.isdir(os.path.join(SHARED_REPO, "installer")):
        return
    env = dict(os.environ, PYTHONPATH=os.path.join(SHARED_REPO, "installer"))
    done = subprocess.run(["python3", "-m", "bipinstall.shared_hermes", "gc"], env=env, capture_output=True,
                          text=True, timeout=600, cwd=SHARED_REPO)
    # its own lines only (what it removed, or why it kept them all: place numbers, never a name); anything else it
    # printed (a traceback may end with a place's path) stays out of the journal
    for line in [line for line in done.stdout.splitlines()
                 if line.startswith(("Removed: ", "Nothing to remove.", "All kept: ", "Space stuck: "))][-5:]:
        log("shared Hermes gc: %s" % line)
    if done.returncode != 0:
        log("shared Hermes gc failed (exit %d)" % done.returncode)


def update(number, commit, repo=None, pretest=False):
    """Place n brought to BipAgents `commit` by its own updater, as its account (what changed restarts); from the
    public repository `repo` when the order names one (its install's origin moves there: install.sh). A pre-test
    place's order (`pretest`): that place alone, what the server shares left to production (prepare_pretest)."""
    user = account(number)
    try:
        uid = pwd.getpwnam(user).pw_uid
    except KeyError:
        return
    try:
        if pretest:
            prepare_pretest(commit, repo)
        else:
            prepare_shared(commit, repo)
    except (OSError, subprocess.SubprocessError) as exc:
        log("shared Hermes for %s not built: %s" % (commit[:12], exc))
    if not pretest:
        refresh_tools()
    # a place installed before its own disk existed moves into one now (its services restart for the update anyway)
    try:
        own_disk(user, read_json(SETTINGS))
    except OSError as exc:
        log("place %d: own disk not made: %s" % (number, exc))
    log("place %d (%s): BipAgents %s%s" % (number, user, commit[:12], " from " + repo if repo else ""))
    forget_cards_seen(pwd.getpwnam(user).pw_dir)   # the card payments switch: said again by the bridge that starts
    where = ["BIPAGENTS_TO_REPO=" + repo] if repo in KNOWN_REPOS else []
    with open("/var/log/bipagents-update%d.log" % number, "a") as out:
        subprocess.run(["sudo", "-iu", user, "env", "XDG_RUNTIME_DIR=/run/user/%d" % uid,
                        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%d/bus" % uid, "BIPAGENTS_TO=" + commit]
                       + where + ["bash", "BipAgents/install.sh", "update", "--auto"], stdout=out, stderr=out,
                       timeout=3600)
    shared_gc()


def reopen(number, pair, home=None):
    """Place n pairs again with `pair` (its person recovered their account without any of their devices): its bridge
    reads the secret, gives the device that proves it a new key and drops the others. Nothing else is touched."""
    user = account(number)
    try:
        entry = pwd.getpwnam(user)
    except KeyError:
        return
    log("place %d (%s): pairing reopened" % (number, user))
    # never through a link the place put on the way (security review 26/11, C1: root wrote and gave away the file a
    # link at setup.json.tmp named, its own seat-agent.py included)
    home_write(home or entry.pw_dir, SETUP, json.dumps({"pair": pair, "reopen": True}), entry.pw_uid, entry.pw_gid,
               0o600)


@contextlib.contextmanager
def ipv4_only():
    """Calls made in IPv4 inside it: the guichet accepts the one-time token only from the server's own IPv4 address
    (the one its provider gave it), and a server with IPv6 would otherwise call from that."""
    real = socket.getaddrinfo

    def only_v4(host, port, family=0, type=0, proto=0, flags=0):
        return real(host, port, socket.AF_INET, type, proto, flags)
    socket.getaddrinfo = only_v4
    try:
        yield
    finally:
        socket.getaddrinfo = real


def setup_path(number, folder="/etc/bipagents"):
    """Where a place's setup waits for its install: place 1's for user-data.yaml's own install, the others' for
    add-seat.sh."""
    return os.path.join(folder, "cloud.json" if number == 1 else "cloud%d.json" % number)


def good_setup(number, setup):
    """A place's setup as the guichet gives it: a pairing secret and the door of its number."""
    if not isinstance(setup, dict) or setup.get("v") != 1:
        return False
    pair = setup.get("pair")
    if not (isinstance(pair, str) and 40 <= len(pair) <= 64 and all(c in PAIR_CHARS for c in pair)):
        return False
    try:
        return int(setup.get("port") or 443) == 442 + number
    except (TypeError, ValueError):
        return False


def write_private(path, value):
    """`value` as JSON at `path`, root's only (0600), replaced in one step."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(value, fh)
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def bootstrap(conf_path=CONF, folder="/etc/bipagents", tries=20, wait=15, caller=None):
    """--bootstrap: the one-time token of the first-boot script exchanged at the guichet for each place's setup and the
    seat agent's own token (see above). 0 when done (or done before), 1 when the guichet refused or never answered
    (the script stops there; the guichet installs the server again once its install time is over)."""
    with open(conf_path) as fh:
        conf = json.load(fh)
    once = conf.get("bootstrap")
    if not once:
        return 0 if conf.get("token") else 1
    caller = caller or call
    answer = None
    for attempt in range(tries):
        try:
            with ipv4_only():
                answer = caller(dict(conf, token=once), "POST", "/v1/server/bootstrap")
            break
        except urllib.error.HTTPError as exc:
            # 401 too: the guichet may not have recorded this install yet (its round still going), so asked again;
            # a token used before, too late or not from this server stays refused all along.
            log("bootstrap: the guichet answered %d, again in %d s" % (exc.code, wait))
        except (OSError, ValueError) as exc:
            log("bootstrap: guichet unreachable (%s), again in %d s" % (exc, wait))
        if attempt + 1 < tries:
            time.sleep(wait)
    if not isinstance(answer, dict):
        return 1
    token, seats = answer.get("token"), answer.get("seats")
    try:
        numbers = sorted(int(n) for n in seats) if isinstance(seats, dict) else []
    except (TypeError, ValueError):
        numbers = []
    if answer.get("server") != conf.get("server") or not isinstance(token, str) or len(token) < 32 or not numbers \
            or numbers != list(range(1, len(numbers) + 1)) or len(numbers) > MAX_SEATS \
            or not all(good_setup(n, seats[str(n)]) for n in numbers):
        log("bootstrap: the guichet's answer is not this server's places")
        return 1
    for number in numbers:
        write_private(setup_path(number, folder), seats[str(number)])
    write_private(conf_path, {"url": conf["url"], "token": token, "server": conf["server"]})
    log("bootstrap: %d places' setups and the seat agent's token fetched" % len(numbers))
    return 0


METADATA_BLOCK = "/etc/bipagents/metadata-block.nft"


def metadata_block(rules=METADATA_BLOCK, runner=None):
    """The metadata service for root only (user-data.yaml: bipagents-metadata-block.service), loaded again when its
    table went (a flushed ruleset)."""
    if not os.path.exists(rules):
        return
    runner = runner or quietly
    if runner("nft", "list", "table", "inet", "bipagents_metadata") != 0:
        log("metadata block %s" % ("loaded again" if runner("nft", "-f", rules) == 0 else "not loaded"))


def call(conf, method, path, body=None):
    data = json.dumps(body).encode("utf-8") if body is not None else (b"" if method == "POST" else None)
    headers = {"Authorization": "Bearer " + conf["token"]}
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(conf["url"].rstrip("/") + path, method=method, data=data, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8") or "{}")


def door_open(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=5):
            return True
    except OSError:
        return False


def last_backup(user):
    """Until when the place's data is known to be in a backup that worked (installer/bipinstall/backup.py writes
    it), else None: its last backup's time, or later, the last time its timer looked and found nothing changed since
    (`current_at`: a place nobody uses backs up once a day only, and the guichet warns after 3 hours without)."""
    try:
        home = pwd.getpwnam(user).pw_dir
    except KeyError:
        return None
    found = None
    for name, key in ((BACKUP_STATE, "last_ok"), (BACKUP_CHECK, "current_at")):
        value = home_json(home, name).get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool) and (key == "last_ok" or found):
            found = max(found or 0.0, float(value))   # `current_at` only beside a backup that worked
    return found


def backup_lock(path=BACKUP_LOCK):
    """The server's backup lock, made once (a server installed before it: at its next beat)."""
    if os.path.exists(path):
        return
    folder = os.path.dirname(path)
    if not os.path.isdir(folder):
        os.makedirs(folder, mode=0o711)   # crossed by the places, never listed (shared_images.py does the same)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT, 0o644)
    os.close(fd)
    os.chmod(path, 0o644)


def locked(home):
    """Why the place's encrypted space is not open ("key service unreachable"...), else None: a place waiting
    for its key is locked, not down (bridge-rs/src/space.rs writes the file outside the encrypted folder)."""
    raw = home_read(home, LOCKED)
    try:
        reason = json.loads(raw.decode("utf-8")).get("reason")
        return str(reason)[:120] if reason else "locked"
    except (AttributeError, ValueError, RecursionError):   # no file (None), not a JSON object, nested too deep
        return None


def used(home):
    """When a person last used the place (epoch seconds its bridge writes, at most once a minute), else None: the
    guichet frees a beta place nobody used for 7 days, never one that never reported it (an old bridge)."""
    raw = home_read(home, USED, 64)
    try:
        return int(raw.decode("ascii").strip())
    except (AttributeError, ValueError):
        return None


def running(uid, words, proc="/proc"):
    """A process of this account whose command line holds all `words` (what runs, never what it does)."""
    for pid in os.listdir(proc):
        if not pid.isdigit():
            continue
        try:
            if os.stat(os.path.join(proc, pid)).st_uid != uid:
                continue
            with open(os.path.join(proc, pid, "cmdline"), "rb") as fh:
                command = fh.read().replace(b"\0", b" ").decode("utf-8", "replace")
        except OSError:
            continue
        if all(word in command for word in words):
            return True
    return False


def version(home):
    """The BipAgents commit the place runs (its repository's HEAD, read without git), else None. Its files only: a ref
    is a name under refs/, never a path elsewhere."""
    git = "BipAgents/.git/"
    head = (home_read(home, git + "HEAD", 4096) or b"").decode("utf-8", "replace").strip()
    if not head.startswith("ref: "):
        return head[:12] or None
    ref = head[5:].strip()
    if not ref.startswith("refs/") or ".." in ref.split("/") or not all(c.isalnum() or c in "._/-" for c in ref):
        return None
    found = home_read(home, git + ref, 4096)
    if found is not None:
        return found.decode("utf-8", "replace").strip()[:12] or None
    for line in (home_read(home, git + "packed-refs") or b"").decode("utf-8", "replace").splitlines():
        if line.strip().endswith(" " + ref):
            return line.split()[0][:12]
    return None


def uptime(path="/proc/uptime"):
    """Seconds since the server started, else None."""
    try:
        with open(path) as fh:
            return int(float(fh.read().split()[0]))
    except (OSError, ValueError, IndexError):
        return None


# What this seat agent does of the guichet's vault_keys orders, said in every beat (F2, final review of 20/11): 3 = it
# notes each order's outcome, tells it with the done and cites the last one in each place's beat (870bc5931). The
# guichet's `vault-enrol` orders a server only when its last beat says 3 or more; raise it when that protocol changes.
VAULT_AGENT = 3


def vitals(meminfo="/proc/meminfo", root="/", uptime_path="/proc/uptime", hardened=HARDENED):
    """The server's own numbers: memory, swap and disk in MB and GB, the load, how long it has been up; whether
    space-hardening.sh was done (encrypted places need it: swap encrypted, no core dumps, journal in memory); and which
    vault orders this agent does (`VAULT_AGENT`)."""
    info = {}
    try:
        with open(meminfo) as fh:
            for line in fh:
                key, _, value = line.partition(":")
                if value.split():
                    info[key] = int(value.split()[0]) // 1024
    except (OSError, ValueError):
        pass
    disk = shutil.disk_usage(root)
    load = os.getloadavg()
    return {"memory_mb": info.get("MemTotal"), "available_mb": info.get("MemAvailable"),
            "swap_mb": info.get("SwapTotal"), "swap_used_mb": info.get("SwapTotal", 0) - info.get("SwapFree", 0),
            "disk_gb": round(disk.total / 1e9, 1), "disk_free_gb": round(disk.free / 1e9, 1),
            "load": [round(load[0], 2), round(load[1], 2)], "uptime": uptime(uptime_path),
            "space_hardened": os.path.exists(hardened), "vault_agent": VAULT_AGENT,
            "reboot_held": reboot_held_nights()}


def health():
    """Each place of this server: its door answers, its Hermes runs, its version, its last backup."""
    seats = {}
    for number in range(1, MAX_SEATS + 1):
        user = account(number)
        try:
            entry = pwd.getpwnam(user)
        except KeyError:
            continue
        seats[str(number)] = {"up": door_open(442 + number), "hermes": running(entry.pw_uid, ("hermes", "gateway")),
                              "asleep": home_stat(entry.pw_dir, ASLEEP) is not None,
                              "version": version(entry.pw_dir), "backup": last_backup(user),
                              "locked": locked(entry.pw_dir)}
        last_used = used(entry.pw_dir)
        if last_used is not None:
            seats[str(number)]["used"] = last_used
        cards = cards_seen(entry.pw_dir, entry.pw_uid)
        if cards is not None:
            seats[str(number)]["cards"] = cards
        # the last vault_keys order ended here, read BEFORE the place's vault: what follows is at least that recent
        # (the guichet leaves out the vault fields of a beat citing an older one than it knows done)
        cited = vault_done_of(number)
        if cited is not None:
            seats[str(number)]["vault_done"] = cited
        seats[str(number)].update(vault_state(entry.pw_dir))
        failed = vault_failed(entry.pw_dir)
        if failed:
            seats[str(number)]["vault_failed"] = failed
        held = vault_key_service(entry.pw_dir)
        if held:
            seats[str(number)]["vault_key_service"] = held
    return seats


OFFER_CARDS_SEEN = ".local/state/bipagents/offer-cards-seen.json"   # what its bridge applies (bridge-rs/src/cards.rs)


# The bridge rewrites it every minute (bridge-rs/src/cards.rs `follow_remote_switch`): an older one is a bridge that
# stopped (review 18/11 pass 5, H1/H2), never trusted.
SEEN_FRESH = 180


def bridge_alive(pid, uid, proc="/proc"):
    """`pid` is a bridge of the account `uid` running now (its /proc entry, its owner, its command's name)."""
    try:
        if os.stat(os.path.join(proc, str(pid))).st_uid != uid:
            return False
        with open(os.path.join(proc, str(pid), "comm")) as fh:
            return fh.read().strip().startswith("bipbridge")
    except (OSError, ValueError):
        return False


def cards_seen(home, uid=None, now=None, alive=None):
    """What the place's bridge applies of Kerfy Studio's card payments switch: "off" (cut), "on", or None (a bridge
    that doesn't read the switch yet: `bipguichet cards status` says so). Only from the bridge running now (review
    18/11 pass 5, H1/H2): the file names its process, which must still be a bridge of this account, and is rewritten
    every minute, so a file left by a bridge that stopped (a wave back to an older one, even one cut in the middle of
    it) says nothing, while a bridge that kept running through an update (an installer-only wave) is still counted."""
    try:
        fd, info = home_open(home, OFFER_CARDS_SEEN)   # never through a link, a regular file only
    except OSError:
        return None
    with os.fdopen(fd, "rb") as fh:
        raw = fh.read(65536)
    age = (time.time() if now is None else now) - info.st_mtime
    if not -60 <= age < SEEN_FRESH:
        return None
    try:
        seen = json.loads(raw.decode("utf-8"))
    except (ValueError, RecursionError):
        return None
    if not isinstance(seen, dict):
        return None
    off, pid = seen.get("off"), seen.get("pid")
    if not isinstance(off, bool) or not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
        return None
    if not (alive or bridge_alive)(pid, info.st_uid if uid is None else uid):
        return None
    return "off" if off else "on"


def forget_cards_seen(home):
    """An update: what the place's bridge said it applies goes, until the bridge that starts says it again."""
    try:
        home_remove(home, OFFER_CARDS_SEEN)
    except OSError:
        pass


# -- the vault's keys sealed (design note "Cle du coffre", lot 3) ------------------------------------------------------
#
# The guichet's `vault_keys` order seals a place's vault, mailbox and connection keys with Kerfy's key service: the
# server hardened first when it is not yet (space-hardening.sh, as for an encrypted space: the order then waits a round,
# so that the beat says space_hardened before the guichet's grant is used), then the place's own installer, as its
# account, once (`bipinstall vault-keys enable`: each try costs one of the place's daily releases at the key service).
# The order is told done whatever happened: a failure is said in the next beats (`vault_failed`), never tried again.
# The settings go to the place in a file of its own (0600, deleted after), never on a command line.

class SeatError(Exception):
    """A place's command failed; its last words, short (no secret in them)."""


def key_service_valid(settings):
    """A key service's settings as the guichet gives them: an https address, a signed grant, a token of the place's
    own (printable, no spaces: it goes in a header)."""
    if not isinstance(settings, dict):
        return False
    url, grant, signature, token = (settings.get(k) for k in ("url", "grant", "signature", "token"))
    if not isinstance(url, str) or not url.startswith("https://") or len(url) > 300:
        return False
    if not all(isinstance(v, str) and v for v in (grant, signature)):
        return False
    return isinstance(token, str) and 16 <= len(token) <= 200 and token.isprintable() and token == token.strip() \
        and " " not in token


def tried_orders(path=None):
    try:
        with open(path or VAULT_TRIED) as fh:
            return set(fh.read().split())
    except OSError:
        return set()


def note_tried(order_id, path=None):
    path = path or VAULT_TRIED
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "a") as fh:
        fh.write("%d\n" % int(order_id))


def _agent_notes_path():
    return VAULT_TRIED + "-why.json"   # root's own: why an order of this agent's ended without sealing, by home


def note_vault_failed(home, why):
    notes = read_json(_agent_notes_path())
    notes = notes if isinstance(notes, dict) else {}
    notes[home] = {"error": str(why)[:120], "at": time.time()}
    write_json(_agent_notes_path(), notes)


def harden_server(lib=LIB):
    """space-hardening.sh, as root (done once: it marks HARDENED)."""
    with open("/var/log/bipagents-space-hardening.log", "a") as out:
        done = subprocess.run(["bash", os.path.join(lib, "space-hardening.sh")], stdout=out, stderr=out, timeout=1800)
    return done.returncode == 0 and os.path.exists(HARDENED)


def place_command(entry, args, timeout=900):
    """`bipinstall <args>` as the place's account (its installer, its systemd user manager): the last JSON line.
    SeatError when it failed or said an error."""
    home = entry.pw_dir
    command = ["sudo", "-u", entry.pw_name, "env", "HOME=" + home, "USER=" + entry.pw_name,
               "XDG_RUNTIME_DIR=/run/user/%d" % entry.pw_uid,
               "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%d/bus" % entry.pw_uid,
               "PATH=%s/.local/bin:/usr/local/bin:/usr/bin:/bin" % home,
               "PYTHONPATH=%s/BipAgents/installer" % home,
               "%s/BipAgents/installer/.venv/bin/python" % home, "-m", "bipinstall"] + list(args)
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=timeout, cwd=home)
    except (OSError, subprocess.SubprocessError) as exc:
        raise SeatError("%s: %s" % (" ".join(args[:2]), exc))
    for line in reversed(done.stdout.strip().splitlines()):
        if line.startswith("{"):
            try:
                result = json.loads(line)
            except ValueError:
                continue
            if done.returncode != 0 or result.get("error"):
                raise SeatError("%s: %s" % (" ".join(args[:2]), str(result.get("error") or "failed")[:200]))
            return result
    raise SeatError("%s gave no answer (%d)" % (" ".join(args[:2]), done.returncode))


# -- root in a place's home: never through a link the place put there (security review 26/11, C1) -----------------------
#
# A place's account owns its home and can put a link (or a pipe) anywhere in it, at any time, from its own shell: a
# path root opens there by name may lead to root's own files (root once wrote setup.json through a link to this very
# script, then gave it to the place). So root walks a home by descriptors only: each folder opened from its parent's
# descriptor without following a link, the file opened in the last one, never waiting, regular files only.

NO_LINK_DIR = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
HOME_READ_MAX = 1 << 20   # what root reads of one file of a place at most


def home_folder(home, rel, make=None):
    """The folder `rel` (relative) under the place's home, opened by descriptor without following any link on its way:
    OSError when a link, or anything but a folder, is there. `make`: (uid, gid, mode) to make the missing ones, each
    made in its parent held open and given to the place by its descriptor (fchown), never root's; else a missing one
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
                os.mkdir(part, 0o700, dir_fd=fd)   # in the parent held open: never where a link would point
                made = True
                child = os.open(part, NO_LINK_DIR, dir_fd=fd)   # what was made, or a link swapped in: refused (ELOOP)
            os.close(fd)
            fd = child
            if made:
                os.fchown(fd, make[0], make[1])
                os.fchmod(fd, make[2])
        return fd
    except BaseException:
        os.close(fd)
        raise


def place_folder(entry, rel):
    """`rel` under the place's home, by file descriptor (`home_folder`), the missing folders made the place's (0700).
    (descriptor, path); OSError when a link (or anything but a folder) is on the way. The caller closes the
    descriptor."""
    return home_folder(entry.pw_dir, rel, (entry.pw_uid, entry.pw_gid, 0o700)), os.path.join(entry.pw_dir, rel)


def home_open(home, rel):
    """The place's file `rel` opened for reading: (descriptor, its stat). Never through a link, never a pipe nor a
    device (opened without waiting, then refused): OSError."""
    folder, name = os.path.split(rel)
    dir_fd = home_folder(home, folder)
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
                     dir_fd=dir_fd)
    finally:
        os.close(dir_fd)
    found = os.fstat(fd)
    if not stat_mod.S_ISREG(found.st_mode):
        os.close(fd)
        raise OSError("not a file: %s" % rel)
    return fd, found


def home_read(home, rel, limit=HOME_READ_MAX):
    """The bytes of the place's file `rel` (`home_open`), at most `limit`; None when it can't be read."""
    try:
        fd, _ = home_open(home, rel)
    except OSError:
        return None
    with os.fdopen(fd, "rb") as fh:
        return fh.read(limit)


def home_json(home, rel):
    """The place's JSON file `rel` as a dict ({} when missing, unreadable or not a dict)."""
    raw = home_read(home, rel)
    try:
        value = json.loads(raw.decode("utf-8")) if raw is not None else {}
    except (ValueError, RecursionError):   # "[[[[...": never the beat's loss
        return {}
    return value if isinstance(value, dict) else {}


def home_toml(home, rel):
    """The place's TOML file `rel` (`home_read`); OSError when it can't be read, ValueError when it isn't TOML."""
    import tomllib
    raw = home_read(home, rel)
    if raw is None:
        raise OSError("%s not readable" % rel)
    try:
        return tomllib.loads(raw.decode("utf-8"))
    except RecursionError:   # nested too deep: an error like any other, never the beat's loss
        raise ValueError("%s nested too deep" % rel)


def home_stat(home, rel):
    """lstat of `rel` in the place's home, reached without following a link; None when it isn't there."""
    folder, name = os.path.split(rel)
    try:
        dir_fd = home_folder(home, folder)
    except OSError:
        return None
    try:
        return os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
    except OSError:
        return None
    finally:
        os.close(dir_fd)


def home_list(home, rel):
    """The names in the place's folder `rel` (never through a link); [] when it can't be listed."""
    try:
        fd = home_folder(home, rel)
    except OSError:
        return []
    try:
        return os.listdir(fd)
    except OSError:
        return []
    finally:
        os.close(fd)


def home_rel(home, path):
    """`path` (absolute, or ~/...) as a path relative to the place's home, None when it leads out of it."""
    if not isinstance(path, str) or not path:
        return None
    if path == "~" or path.startswith("~/"):
        path = home + path[1:]   # the place's ~, never root's
    if not os.path.isabs(path) or ".." in path.split("/"):
        return None
    rel = os.path.relpath(os.path.normpath(path), os.path.normpath(home))
    return None if rel == "." or rel.startswith("..") else rel


def home_write(home, rel, text, uid, gid, mode, folders_mode=0o700):
    """`text` written as the place's file `rel` (its account's, `mode`): the missing folders made the place's, the file
    written beside under a name of its own (O_EXCL), given by its descriptor, then moved over `rel`, which replaces a
    link put there without following it. OSError when a link is on the way."""
    folder, name = os.path.split(rel)
    dir_fd = home_folder(home, folder, (uid, gid, folders_mode))
    try:
        tmp = "%s.%s.tmp" % (name, base64.b32encode(os.urandom(5)).decode().lower())
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
                     | getattr(os, "O_CLOEXEC", 0), 0o600, dir_fd=dir_fd)
        try:
            with os.fdopen(fd, "w") as fh:
                fh.write(text)
                fh.flush()
                os.fchown(fh.fileno(), uid, gid)
                os.fchmod(fh.fileno(), mode)
            os.replace(tmp, name, src_dir_fd=dir_fd, dst_dir_fd=dir_fd)
        except BaseException:
            with contextlib.suppress(OSError):
                os.unlink(tmp, dir_fd=dir_fd)
            raise
    finally:
        os.close(dir_fd)


def home_remove(home, rel):
    """The place's file `rel` removed (a link there: the link itself), never one a link on its way names."""
    folder, name = os.path.split(rel)
    try:
        dir_fd = home_folder(home, folder)
    except FileNotFoundError:
        return
    try:
        os.unlink(name, dir_fd=dir_fd)
    except FileNotFoundError:
        pass
    finally:
        os.close(dir_fd)


VAULT_OUTCOMES_KEPT = 200   # the last vault_keys outcomes kept (with each place's last one, whatever its age)


def _vault_done_path():
    return VAULT_TRIED + "-done"   # root's own, beside VAULT_TRIED: each vault_keys order ended, one JSON line


def note_vault_outcome(order_id, number, sealed, error=None, key_service=None):
    """A vault_keys order ended: what it did, told with its done (the guichet marks the grant sealed from it) and
    cited by the beats (`vault_done`). Never the token, only its hash."""
    line = {"order": int(order_id), "seat": int(number), "sealed": bool(sealed)}
    if error:
        line["error"] = str(error)[:300]
    if key_service:
        line["key_service"] = key_service
    # kept: each place's last order (what its beats cite) and the last VAULT_OUTCOMES_KEPT (a done told again while
    # the guichet still lists it: two days at most); written whole, root's alone (0600 whatever the umask)
    items = _vault_outcomes() + [line]
    recent = items[-VAULT_OUTCOMES_KEPT:]
    last_of = {}
    for item in items:
        last_of[item.get("seat")] = item
    kept = [item for item in items if item in recent or last_of.get(item.get("seat")) is item]
    path = _vault_done_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    if os.path.lexists(tmp):
        os.unlink(tmp)
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "w") as fh:
        os.fchmod(fh.fileno(), 0o600)
        fh.write("".join(json.dumps(item) + "\n" for item in kept))
    os.replace(tmp, path)


def _vault_outcomes():
    try:
        with open(_vault_done_path()) as fh:
            lines = fh.read().splitlines()
    except OSError:
        return []
    found = []
    for line in lines:
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if isinstance(item, dict) and isinstance(item.get("order"), int):
            found.append(item)
    return found


def vault_outcome(order_id):
    """What a vault_keys order did ({"sealed": ..., "key_service" or "error"}), None while it hasn't ended."""
    for item in reversed(_vault_outcomes()):
        if item["order"] == int(order_id):
            return {k: v for k, v in item.items() if k in ("sealed", "error", "key_service")}
    return None


def vault_done_of(number):
    """The last vault_keys order ended for place n (what its beat cites), else None."""
    ids = [item["order"] for item in _vault_outcomes() if item.get("seat") == int(number)]
    return max(ids) if ids else None


# Alex 21/11: a vault_keys order (sealing restarts the place's bridge, and the first time hardens the server: swap
# emptied) is done only at night, server time, after the night's reboot window (00:00-00:44): from 01:00 to 05:00.
# Sent in the evening or any time, it waits (served again at each round; orders are valid two days). A lab says
# "any time" with this file of root's.
VAULT_NIGHT = (1, 5)
VAULT_ANYTIME = "/etc/bipagents/vault-keys-anytime"


def vault_night(clock=None):
    """Now is the server's night for the vault's orders (`VAULT_NIGHT`, server time), or a lab said any time."""
    if os.path.exists(VAULT_ANYTIME):
        return True
    hour = (clock or time.localtime)().tm_hour
    return VAULT_NIGHT[0] <= hour < VAULT_NIGHT[1]


def _held_said_path():
    return VAULT_TRIED + "-held.json"   # root's own: the orders said held, for which night's wait


def _held_said_first(order_id, now):
    """True the first time a round holds this order for the night (review 3 of 21/11: the rounds run every 2 min, the
    line was written at each one, all day long): once per order and night waited for (the night named as the reboot's
    are, the day it starts: 12 hours back). A note that can't be kept says it each time rather than never."""
    night = time.strftime("%Y-%m-%d", time.localtime(time.mktime(now) - 12 * 3600))
    path = _held_said_path()
    data = read_json(path)
    said = data.get("orders") if data.get("night") == night and isinstance(data.get("orders"), list) else []
    if int(order_id) in said:
        return False
    try:
        write_json(path, {"night": night, "orders": said + [int(order_id)]})
    except OSError:
        pass
    return True


def vault_order_round(body, order_id, clock=None):
    """The round's part of a vault_keys order: done at night only (`vault_night`); True: done (told), False: held."""
    now = (clock or time.localtime)()
    if not vault_night(clock=lambda: now):
        if _held_said_first(order_id, now):
            log("vault keys of place %s held until the night (%02d:00-%02d:00 server time; said once a night)"
                % (body.get("seat"), VAULT_NIGHT[0], VAULT_NIGHT[1]))
        return False
    return vault_keys_order(int(body["seat"]), body["vault_keys"], int(order_id))


def vault_keys_order(number, settings, order_id, harden=None, run_as=None, moves=None):
    """The guichet's `vault_keys` order for place n. True: done (told to the guichet, whatever happened); False: held
    (the server was just hardened, or the place moves: neither noted nor told done, served again next round). The
    stopped copy a completed move left is never enrolled (it would start again, sealed, beside the place that moved)."""
    harden = harden or harden_server
    run_as = run_as or place_command
    user = account(number)
    try:
        entry = pwd.getpwnam(user)
    except KeyError:
        return True
    under_way = moving(number, moves or MOVES)
    if under_way:
        log("vault keys of place %d held: its move %s is under way" % (number, under_way))
        return False
    if home_stat(entry.pw_dir, MOVED_AWAY) is not None:
        log("vault keys of place %d dropped: the place moved away (its copy stays stopped)" % number)
        note_vault_outcome(order_id, number, False, "the place moved away: not sealed here")
        return True
    if str(int(order_id)) in tried_orders():
        if vault_outcome(order_id) is None:   # cut off during its try: whatever the place says now
            note_vault_outcome(order_id, number, vault_state(entry.pw_dir).get("vault") is not None,
                               "cut off during its try", vault_key_service(entry.pw_dir))
        return True   # tried already (a run cut off after it, the same order again): never a second enrolment
    if not os.path.exists(HARDENED):
        try:
            hardened = harden()
        except (OSError, subprocess.SubprocessError):
            hardened = False
        if not hardened:
            note_tried(order_id)
            note_vault_failed(entry.pw_dir, "the server's hardening failed: not sealed")
            note_vault_outcome(order_id, number, False, "the server's hardening failed: not sealed")
            log("place %d (%s): vault keys not sealed, the server's hardening failed" % (number, user))
            return True
        log("place %d (%s): server hardened for the vault keys, sealed at the next round" % (number, user))
        return False
    note_tried(order_id)   # before the try: whatever happens next, never tried again by this agent
    folder_fd, name, path = None, None, None
    try:
        if settings == {"reseal": True}:
            # a place sealed once whose reseal failed: its own key service (keyservice.json), nothing to hand over
            args = ["vault-keys", "enable", "--json"]
        else:
            folder_fd, folder = place_folder(entry, os.path.dirname(VAULT_STATE))
            name = "vault-keys-order.%s.json" % base64.b32encode(os.urandom(10)).decode().lower()
            fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600,
                         dir_fd=folder_fd)
            with os.fdopen(fd, "w") as fh:
                os.fchmod(fh.fileno(), 0o600)
                try:
                    os.fchown(fh.fileno(), entry.pw_uid, entry.pw_gid)
                except PermissionError:
                    pass   # not root (tests): the file is already this account's
                json.dump(settings, fh)
            path = os.path.join(folder, name)
            args = ["vault-keys", "enable", "--settings-file", path, "--json"]
        result = run_as(entry, args, timeout=900)
        log("place %d (%s): vault keys %s" % (number, user, (result or {}).get("state", "sealed")))
        note_vault_outcome(order_id, number, True, key_service=vault_key_service(entry.pw_dir))
    except (SeatError, OSError) as exc:
        note_vault_failed(entry.pw_dir, str(exc))
        log("place %d (%s): vault keys not sealed: %s" % (number, user, str(exc)[:200]))
        note_vault_outcome(order_id, number, False, str(exc), vault_key_service(entry.pw_dir))
    finally:
        if folder_fd is not None:
            if name:
                try:
                    os.unlink(name, dir_fd=folder_fd)
                except OSError:
                    pass
            os.close(folder_fd)
    return True


def vault_state(home):
    """The place's vault, for a place whose keys are sealed only (keys.sealed.json): from its bridge's /health,
    {"vault": "open"} or {"vault": "locked", "vault_reason": ...}; {"vault": "sealed"} when the bridge doesn't say
    (mute, or an odd answer); {} when not sealed."""
    try:
        conf = home_toml(home, BRIDGE_TOML)
        if not sealed_here(home, conf):
            return {}
    except (OSError, ValueError, TypeError, ImportError, AttributeError):
        return {}
    try:
        port = int(conf.get("port", 8643))
        host = str(conf.get("host") or "127.0.0.1")
        if host in ("0.0.0.0", "::", ""):
            host = "127.0.0.1"
        with urllib.request.urlopen("http://%s:%d/health" % (host, port), timeout=3) as answer:
            said = json.loads(answer.read(65536).decode("utf-8"))
    except (OSError, ValueError, TypeError, http.client.HTTPException):
        # its bridge mute (its door's check says so): sealed all the same, never taken for one whose sealing failed
        return {"vault": "sealed"}
    vault = said.get("vault") if isinstance(said, dict) else None
    if vault == "open":
        return {"vault": "open"}
    if vault == "locked":
        reason = said.get("vault_reason")
        return {"vault": "locked", **({"vault_reason": reason[:120]} if isinstance(reason, str) and reason else {})}
    return {"vault": "sealed"}


SPACE_KEY_SERVICE = ".local/share/bipagents/space/keyservice.json"   # the place's key service (vault_keys.py)
BRIDGE_TOML = ".config/bipagents/bridge.toml"


def sealed_here(home, conf):
    """The place's vault keys are sealed: keys.sealed.json in its bridge's data_dir, looked for in its home only (the
    data_dir is the place's to write: root never looks elsewhere, nor through a link)."""
    data = home_rel(home, conf.get("data_dir") or os.path.join(home, BRIDGE_DATA))
    return data is not None and home_stat(home, os.path.join(data, SEALED_KEYS)) is not None


def vault_key_service(home):
    """The SHA-256 (hex) of the key-service token the place holds (its keyservice.json), what the guichet keeps of
    each grant: it tells a reseal with a token still known from one whose token was forgotten. Never the token."""
    import hashlib
    token = home_json(home, SPACE_KEY_SERVICE).get("token")
    return hashlib.sha256(token.encode()).hexdigest() if isinstance(token, str) and token else None


SHUTDOWN_SCHEDULED = "/run/systemd/shutdown/scheduled"   # systemd-logind's: a `shutdown -r HH:MM` waiting
# The key services a reboot waits for: the offer's only (review 2 of 21/11: keyservice.json is written by the place's
# own account, its url must never keep the server from rebooting).
KEY_SERVICES = ("https://keys.bipagents.com",)
REBOOT_HELD = "/var/lib/bipagents/reboot-held.json"   # root's: the nights in a row a reboot waited
REBOOT_HELD_MAX = 3                                    # then it goes (and the beat says it waited)


def sealed_key_services():
    """The key services (`KEY_SERVICES` only) of this server's places whose vault is sealed (keys.sealed.json in their
    bridge's data_dir): what a restart of them needs to reopen their vault."""
    urls = set()
    for number in range(1, MAX_SEATS + 1):
        try:
            home = pwd.getpwnam(account(number)).pw_dir
        except KeyError:
            continue
        try:
            if not sealed_here(home, home_toml(home, BRIDGE_TOML)):
                continue
            url = home_json(home, SPACE_KEY_SERVICE).get("url")
        except (OSError, ValueError, TypeError, AttributeError):
            continue
        if isinstance(url, str) and url.rstrip("/") in KEY_SERVICES:
            urls.add(url.rstrip("/"))
    return urls


def key_service_answers(url, timeout=10):
    """The key service's /health answers 200 (anything else, a non-HTTP answer included: no)."""
    import http.client
    try:
        with urllib.request.urlopen(url.rstrip("/") + "/health", timeout=timeout) as answer:
            return answer.status == 200
    except (OSError, ValueError, http.client.HTTPException):
        return False


def _night_label(now=None):
    """A night's name: the day it started (12 hours back), local time."""
    return time.strftime("%Y-%m-%d", time.localtime((time.time() if now is None else now) - 12 * 3600))


def _held_state():
    data = read_json(REBOOT_HELD)
    return data if isinstance(data, dict) else {}


def _held_write(data):
    os.makedirs(os.path.dirname(REBOOT_HELD), exist_ok=True)
    tmp = REBOOT_HELD + ".tmp"
    with open(tmp, "w") as fh:
        json.dump(data, fh)
    os.replace(tmp, REBOOT_HELD)


def _day_before(day):
    return time.strftime("%Y-%m-%d", time.localtime(time.mktime(time.strptime(day, "%Y-%m-%d")) - 12 * 3600))


def reboot_held_nights(today=None):
    """The nights in a row a reboot waited for the key service (said in the beat: `reboot_held`), 0 once it answered
    or a night passed without waiting."""
    data = _held_state()
    last, nights = data.get("last"), data.get("nights", 0)
    today = today or _night_label()
    if not isinstance(nights, int) or not isinstance(last, str):
        return 0
    return nights if last in (today, _day_before(today)) or data.get("went") == today else 0


def hold_reboot(scheduled=SHUTDOWN_SCHEDULED, answers=None, cancel=None, night=None):
    """Alex 21/11: a sealed place is never restarted while its key service doesn't answer (its vault would stay closed
    until it answers). A planned reboot waiting (unattended-upgrades' `shutdown -r 00:xx` at night) is called off when
    the key service of this server's sealed places (`KEY_SERVICES`) doesn't answer; the reboot stays required, so the
    next night tries again. `REBOOT_HELD_MAX` nights in a row at most (review 2 of 21/11), then the reboot goes and
    the beat says how many nights it waited (the guichet tells Kerfy Studio). A power-off is never touched. True when
    it was called off."""
    try:
        with open(scheduled) as fh:
            mode = dict(line.strip().partition("=")[::2] for line in fh if "=" in line).get("MODE")
    except OSError:
        return False
    if mode != "reboot":
        return False
    services = sealed_key_services()
    if not services:
        return False
    tonight = night or _night_label()
    silent = sorted(url for url in services if not (answers or key_service_answers)(url))
    data = _held_state()
    if not silent:
        if data:
            _held_write({})
        return False
    nights = reboot_held_nights(tonight)
    if data.get("last") == tonight:
        nights = max(nights, 1)
    elif nights >= REBOOT_HELD_MAX:
        _held_write({"last": data.get("last"), "nights": nights, "went": tonight})
        log("planned reboot let through after %d nights waiting for the key service %s" % (nights, ", ".join(silent)))
        return False
    else:
        nights += 1
    _held_write({"last": tonight, "nights": nights})
    (cancel or (lambda: subprocess.run(["shutdown", "-c", "BipAgents: the key service doesn't answer, the reboot "
                                         "waits for the next night"], check=False, timeout=30)))()
    log("planned reboot called off (night %d of %d): the key service %s doesn't answer (a sealed place would stay "
        "closed)" % (nights, REBOOT_HELD_MAX, ", ".join(silent)))
    return True


def vault_failed(home):
    """Why the place's last sealing failed, short, else None: its own note (the installer's), else this agent's."""
    state = home_json(home, VAULT_STATE)
    if state.get("state") == "sealed":
        return None
    if state.get("state") == "failed":
        return str(state.get("error") or "failed")[:120]
    notes = read_json(_agent_notes_path())
    note = notes.get(home) if isinstance(notes, dict) else None
    return str(note.get("error"))[:120] if isinstance(note, dict) and note.get("error") else None


# -- memory and CPU: each place's soft share, compressed swap, and what each place holds -------------------------------
#
# A place is one account; systemd keeps all its processes in user-<uid>.slice: its bridge and Hermes (user services
# under user@<uid>.service), its rootless podman containers (the agents' sandboxes and browsers: podman's systemd
# cgroup manager puts them under user@<uid>.service too, through the user bus dbus-user-session gives) and whatever
# its agents start in their terminals (inside those containers). Each slice gets (`limits`), with systemctl
# set-property (a drop-in in /etc/systemd/system.control that survives reboots), set again when one differs:
# - MemoryHigh: its share of the server's memory, 80 % of it divided among the server's places, never under
#   PLACE_FLOOR_MB (about 320 MB a person measured, plus an agent's browser), or the share the guichet sends
#   (`memory_high_share` in the beat's answer). Above it the kernel reclaims that place's own memory first (into zram)
#   and slows its allocations, so a place opening four or five browsers at once slows itself, not its neighbours.
#   Nothing is killed or refused there (Alex, 2026-10-21: no limits that block people).
# - MemoryMax: only a last-resort safety well above it (1.5x, at least 1 GB more, never above the memory less the
#   system's part), so that one place can't run the whole server out of memory; swap still comes first.
# - CPUWeight 100, the same for every place (an equal share when the processors are busy, all of them when not), and
#   TasksMax 8192 (a runaway fork stays in its place; a browser or Node takes a few hundred threads).
# The agents are never told numbers (Alex, 2026-10-30): their souls only say that big toolchains don't fit
# (bridge-rs/src/souls.rs, SERVER_RULE). A place still reads the server's total memory in /proc/meminfo, as any
# Linux account does; its own ceiling shows only in its slice (memory.high).

CGROUP_USERS = "/sys/fs/cgroup/user.slice"
SETTINGS = "/var/lib/bipagents/settings.json"   # what the guichet's beat answer set (`settings`)
SAMPLES = "/run/bipagents/memory.json"           # each place's highest memory since the last beat (--sample)
ZRAM_TRIED = "/var/lib/bipagents/zram-tried"
MEMORY_FOR_PLACES = 0.8      # of the server's memory, shared among its places (MemoryHigh)
PLACE_FLOOR_MB = 768         # never less a place: ~320 MB a person (measured 2026-10-19) and an agent's browser
SYSTEM_MB = 384              # the system's own part (sshd, journald, this agent): never in a place's MemoryMax
MEMORY_MAX_FACTOR = 1.5
CPU_WEIGHT = 100
TASKS_MAX = 8192
MB = 1024 * 1024
# The same text as user-data.yaml's (installer/tests/test_cloud_init.py): half the memory, zstd, before the disk swap.
ZRAM_CONF = "[zram0]\nzram-size = ram / 2\ncompression-algorithm = zstd\nswap-priority = 100\n"
SAMPLER_SERVICE = """[Unit]
Description=BipAgents: each place's memory, sampled between the guichet's beats

[Service]
Type=oneshot
ExecStart=/usr/bin/python3 %s/seat-agent.py --sample
"""
SAMPLER_TIMER = """[Unit]
Description=BipAgents: each place's memory every 15 s

[Timer]
OnBootSec=30s
OnUnitActiveSec=15s
AccuracySec=1s

[Install]
WantedBy=timers.target
"""


def read_json(path):
    try:
        with open(path) as fh:
            value = json.load(fh)
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def write_json(path, value):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "w") as fh:
        json.dump(value, fh)
    os.replace(path + ".tmp", path)


def good_share(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and 0.1 <= value <= 1.0


def keep_settings(answer, path=None):
    """The settings in the guichet's answer, kept when they are ones this agent knows and within reason."""
    settings = answer.get("settings") if isinstance(answer, dict) else None
    if not isinstance(settings, dict):
        return
    path = path or SETTINGS
    kept = read_json(path)
    changed = dict(kept)
    if good_share(settings.get("memory_high_share")):
        changed["memory_high_share"] = float(settings["memory_high_share"])
    if "place_disk_gib" in settings and disk_gib(settings) == settings["place_disk_gib"]:
        changed["place_disk_gib"] = settings["place_disk_gib"]
    if changed != kept:
        write_json(path, changed)


def write_credit(path, value, uid, gid, home):
    """`value` written at `path` for the place's account (0644): never through a link, never outside its home
    (`home_write`). Each folder missing on the way made the place's (0755), not root's: the seat agent tells a new
    place its disk level within 15 s of its own disk's mount, before its install, and making ~/.local/state/bipagents
    in one go left ~/.local root's, so the install could not make ~/.local/bin (real-server test, 2026-10-31)."""
    rel = home_rel(home, path)
    if rel is None:
        raise OSError("the credit's file leads out of the place's home")
    home_write(home, rel, json.dumps(value), uid, gid, 0o644, folders_mode=0o755)


def keep_credit(answer, homes=None):
    """Each place's AI credit state in the guichet's answer, written where its bridge reads it (credit.rs); a place the
    answer leaves out (no running subscription) has its file removed: ok. An older guichet's answer changes nothing.
    `homes`: number -> (home, uid, gid), the places' accounts by default."""
    credit = answer.get("credit") if isinstance(answer, dict) else None
    if not isinstance(credit, dict):
        return
    if homes is None:
        homes = {}
        for number in range(1, MAX_SEATS + 1):
            try:
                entry = pwd.getpwnam(account(number))
            except KeyError:
                continue
            homes[number] = (entry.pw_dir, entry.pw_uid, entry.pw_gid)
    for number, (home, uid, gid) in sorted(homes.items()):
        path = os.path.join(home, CREDIT)
        given = credit.get(str(number))
        try:
            if not isinstance(given, dict) or given.get("state") not in CREDIT_STATES:
                home_remove(home, CREDIT)
                continue
            value = {"state": given["state"]}
            resumes = given.get("resumes")
            if isinstance(resumes, (int, float)) and not isinstance(resumes, bool):
                value["resumes"] = float(resumes)
            if home_json(home, CREDIT) == value:   # a regular file of the place's, reached without a link
                continue
            write_credit(path, value, uid, gid, home)
        except OSError as exc:
            log("place %d: credit state not written: %s" % (number, exc))


def keep_offer_cards(answer, uids=None, folder=None, owner=(0, 0)):
    """Kerfy Studio's switch for the agents' card payments, per place, from the guichet's answer (`offer_cards`: number
    -> true/false): `{"offer_cards": false}` written where the place's bridge reads it, the file removed when true or
    when the answer leaves the place out; written or removed only when that changes. An older guichet's answer (no
    `offer_cards`) changes nothing. `uids`: number -> uid, the places' accounts by default; `owner`: the folder's
    (root's)."""
    given = answer.get("offer_cards") if isinstance(answer, dict) else None
    if not isinstance(given, dict):
        return
    folder = folder or OFFER_CARDS_DIR
    if uids is None:
        uids = {}
        for number in range(1, MAX_SEATS + 1):
            try:
                uids[number] = pwd.getpwnam(account(number)).pw_uid
            except KeyError:
                continue
    try:
        if not os.path.isdir(folder) or os.path.islink(folder):
            if os.path.lexists(folder):
                os.unlink(folder)
            os.makedirs(folder, mode=0o755, exist_ok=True)
        os.chown(folder, owner[0], owner[1], follow_symlinks=False)
        os.chmod(folder, 0o755)
    except OSError as exc:
        log("card payments switch: folder not ready: %s" % exc)
        return
    # a place installed again has a new account, another uid (review 18/11 F3): the old uid's file goes, never to cut
    # a later account given that uid
    known = set(uids.values())
    try:
        for name in os.listdir(folder):
            stem = name[:-len(".json")] if name.endswith(".json") else None
            if stem is not None and stem.isdigit() and int(stem) in known:
                continue
            try:
                os.unlink(os.path.join(folder, name))
            except IsADirectoryError:
                continue
            log("card payments switch: %s removed (no place has that account any more)" % name)
    except OSError as exc:
        log("card payments switch: old files not removed: %s" % exc)
    for number, uid in sorted(uids.items()):
        path = os.path.join(folder, "%d.json" % uid)
        off = given.get(str(number)) is False
        try:
            if not off:
                if os.path.lexists(path):
                    os.unlink(path)
                    log("place %d: card payments no longer cut by Kerfy Studio" % number)
                continue
            if not os.path.islink(path) and read_json(path) == {"offer_cards": False}:
                continue
            tmp = path + ".tmp"
            if os.path.lexists(tmp):
                os.unlink(tmp)
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o644)
            with os.fdopen(fd, "w") as fh:
                json.dump({"offer_cards": False}, fh)
                fh.flush()
                os.fsync(fh.fileno())
                os.fchmod(fh.fileno(), 0o644)
            os.replace(tmp, path)
            log("place %d: card payments cut by Kerfy Studio" % number)
        except OSError as exc:
            log("place %d: card payments switch not written: %s" % (number, exc))


def places():
    """Each place of this server: its number (as text) and its account's uid."""
    found = {}
    for number in range(1, MAX_SEATS + 1):
        try:
            found[str(number)] = pwd.getpwnam(account(number)).pw_uid
        except KeyError:
            continue
    return found


def slice_file(uid, name, root=CGROUP_USERS):
    """A number of the place's slice (memory.current...), else None (its services not started, "max")."""
    try:
        with open(os.path.join(root, "user-%d.slice" % uid, name)) as fh:
            value = fh.read().split()[0]
        return None if value == "max" else int(value)
    except (OSError, ValueError, IndexError):
        return None


def limits(memory_mb, count, settings):
    """A place's slice settings on a server with `memory_mb` of memory and `count` places: MemoryHigh and MemoryMax in
    MB, CPUWeight, TasksMax."""
    share = settings.get("memory_high_share")
    if good_share(share):
        high = int(memory_mb * share)
    else:
        high = max(PLACE_FLOOR_MB, int(memory_mb * MEMORY_FOR_PLACES / max(1, count)))
    top = max(high, memory_mb - SYSTEM_MB)
    return {"MemoryHigh": high, "MemoryMax": min(max(int(high * MEMORY_MAX_FACTOR), high + 1024), top),
            "CPUWeight": CPU_WEIGHT, "TasksMax": TASKS_MAX}


def ceiling_mb(memory_mb, settings, count=1):
    return limits(memory_mb, count, settings)["MemoryHigh"]


def slice_now(uid, root=CGROUP_USERS):
    """The slice's settings as the kernel has them, in the units of `limits` (None: not set, "max")."""
    def mb(value):
        return None if value is None else value // MB
    return {"MemoryHigh": mb(slice_file(uid, "memory.high", root)), "MemoryMax": mb(slice_file(uid, "memory.max", root)),
            "CPUWeight": slice_file(uid, "cpu.weight", root), "TasksMax": slice_file(uid, "pids.max", root)}


def ceiling(memory_mb, settings, root=CGROUP_USERS, setter=None):
    """Each running place's slice gets its limits when one of them differs (a place started or left, the share
    changed)."""
    if not memory_mb:
        return
    found = places()
    wanted = limits(memory_mb, len(found), settings)
    for number, uid in found.items():
        if not os.path.isdir(os.path.join(root, "user-%d.slice" % uid)):
            continue
        if slice_now(uid, root) == wanted:
            continue
        log("place %s: memory %d MB (at most %d MB), CPU weight %d" % (number, wanted["MemoryHigh"],
                                                                        wanted["MemoryMax"], wanted["CPUWeight"]))
        (setter or run)("systemctl", "set-property", "user-%d.slice" % uid, "MemoryHigh=%dM" % wanted["MemoryHigh"],
                        "MemoryMax=%dM" % wanted["MemoryMax"], "CPUWeight=%d" % wanted["CPUWeight"],
                        "TasksMax=%d" % wanted["TasksMax"])


def sample(root=CGROUP_USERS, path=SAMPLES):
    """--sample (every 15 s, bipagents-memory.timer): each place's highest memory.current since the last beat."""
    samples = read_json(path)
    for number, uid in places().items():
        current = slice_file(uid, "memory.current", root)
        if current is None:
            continue
        entry = samples.get(number) if isinstance(samples.get(number), dict) else {}
        entry["max"] = max(int(entry.get("max") or 0), current)
        samples[number] = entry
    write_json(path, samples)
    try:
        disk_quick()   # the own disks too: a reserve given back and a new level told within 15 s
    except OSError as exc:
        log("disks not checked: %s" % exc)
    return 0


def places_memory(root=CGROUP_USERS, path=SAMPLES):
    """For the beat: each place's memory now and its peak since the last beat, in MB. The peak is the highest of the
    samples taken between beats, now, and the kernel's memory.peak when it rose since the last beat (it never goes
    down: a new high since then is exact)."""
    samples = read_json(path)
    out, kept = {}, {}
    for number, uid in places().items():
        current = slice_file(uid, "memory.current", root) or 0
        entry = samples.get(number) if isinstance(samples.get(number), dict) else {}
        peak = max(current, int(entry.get("max") or 0))
        lifetime, before = slice_file(uid, "memory.peak", root), entry.get("lifetime")
        if lifetime is not None and isinstance(before, int) and lifetime > before:
            peak = max(peak, lifetime)
        out[number] = {"mem_mb": current // MB, "mem_peak_mb": peak // MB}
        kept[number] = {"max": 0, "lifetime": lifetime}
    try:
        write_json(path, kept)
    except OSError:
        pass
    return out


# -- disk: each place on a disk of its own (5 GB), watched ------------------------------------------------------------
#
# Alex, 2026-10-30: "5 Go par personne max", and the place sees that size, not the server's disk. Each place's home is
# a filesystem of its own: an ext4 image of PLACE_DISK_GIB (5 GiB, or `place_disk_gib` in the guichet's settings),
# sparse, at /var/lib/bipagents/places/<account>.img (on the server it takes only what the place wrote, given back as
# it deletes: mounted with discard), loop-mounted at /home/<account> by a mount unit at every boot, before the
# account's services (a drop-in on user@<uid>.service: RequiresMountsFor). Everything the place writes is in it: its
# bridge's and Hermes' data, its podman storage (its containers' layers: what an agent installs in its sandbox), its
# browsers' profiles, its encrypted space (gocryptfs keeps its files in the home). So `df` in the place, in its
# terminal or its containers says 5 GB, and a place that fills it gets "no space left" inside it only, never the
# server. No blocks kept for root (-m 0), one inode every 8 KB (node_modules, venvs), nosuid and nodev.
# A new place gets its disk before its install (add-seat.sh; user-data.yaml for place 1: --own-disk). A place
# installed before moves into one when the guichet next updates it (`update`, so the update waves pace it): its
# services stop for the copy (a minute or two), its old home is kept aside OLD_HOME_DAYS, then removed. A place whose
# files don't fit, or whose copy fails, stays as it was (logged; the beat says "own_disk": false) and is watched below
# with a hard line instead.
#
# Every DISK_EVERY (`disk_watch`), each place's use: its own filesystem's (statvfs, nothing walked), else du -sx of its
# home against the same 5 GB. From 80 % "low": its bridge tells its agents once (a short note: clean up or ask for
# room); from 95 % "full": the beat says so and the guichet alerts Kerfy once (watch.rs), and for a place without its
# own disk, its bridge pauses its agents' workspaces (never deleting anything) until it is back under 90 %.
# Written for its bridge at ~/.local/state/bipagents/disk.json (bridge-rs/src/storage_watch.rs), and in memory at
# /run/bipagents/disk/<uid>.json (root's, the place's group may read it): a full disk can't take the file that says
# so (real-server test, 2026-10-31: the bridge stayed on "low"). The bridge reads the newer of the two and ignores
# them after 30 minutes (a stopped seat agent never keeps a place paused). The server as a whole: the guichet's own
# alert under 10 % free and its placement rules (weights.rs, grow_disk_gb) already count it; the images being sparse,
# 5 GB a place is what a place may take, not what it takes.
#
# A reserve in each own disk (same test: at 100 % Hermes could not open a conversation, error 500, and the agents went
# silent, unable to help their person make room): RESERVE_MB taken by root at the disk's root, a file the place can't
# remove (immutable), not counted in its levels. When the place's free room falls under RELEASE_MB the reserve is given
# back at once, so Hermes and the bridge go on writing and the agents answer; checked every 15 s with the memory
# samples (`disk_quick`, statvfs only), which also tell the bridge a new level at once instead of at the next round.
# If the place fills that room too, its agents' workspaces (their terminals: where a runaway install or download
# runs) are paused under PAUSE_MB, keeping the rest for Hermes and the bridge, until it is back over the reserve's
# size. The reserve is taken again once the place is out of "full" with room for it twice.

PLACES_DISKS = "/var/lib/bipagents/places"
PLACE_DISK_GIB = 5
GIB = 1024 * 1024 * 1024
DISK_STATE = "/var/lib/bipagents/disk.json"            # each place's last measure and level (root's)
PLACE_DISK_FILE = ".local/state/bipagents/disk.json"   # what its bridge reads
DISK_EVERY = 300
DISK_LOW, DISK_FULL = 0.80, 0.95
DISK_LOW_BACK, DISK_FULL_BACK = 0.75, 0.90            # left only below these (no flapping)
RUN_DISK = "/run/bipagents/disk"                      # each place's level in memory: <uid>.json (above)
RESERVE = ".bipagents-reserve"                         # root's reserve at the root of each own disk (above)
RESERVE_MB, RELEASE_MB, PAUSE_MB = 192, 64, 96
FS_IOC_GETFLAGS, FS_IOC_SETFLAGS, FS_IMMUTABLE_FL = 0x80086601, 0x40086602, 0x10
OLD_HOME_DAYS = 7
MOUNT_UNIT = """[Unit]
Description=BipAgents: the own disk of the place %(user)s
Before=user@%(uid)d.service

[Mount]
What=%(image)s
Where=%(home)s
Type=ext4
Options=loop,discard,nosuid,nodev

[Install]
WantedBy=local-fs.target
"""
USER_DROP_IN = "[Unit]\nRequiresMountsFor=%s\n"


def disk_gib(settings):
    size = settings.get("place_disk_gib")
    return size if isinstance(size, int) and not isinstance(size, bool) and 1 <= size <= 100 else PLACE_DISK_GIB


def mount_unit(home):
    """The mount unit's name for a home: /home/bip2 -> home-bip2.mount (the accounts' names need no escaping)."""
    return home.strip("/").replace("/", "-") + ".mount"


def mounted_at(path, mountinfo="/proc/self/mountinfo"):
    """What is mounted at `path` or under it (the mount points), in the order of mountinfo."""
    path = os.path.normpath(path)
    found = []
    try:
        with open(mountinfo) as fh:
            for line in fh:
                fields = line.split()
                if len(fields) > 4:
                    point = fields[4].replace("\\040", " ")
                    if point == path or point.startswith(path + "/"):
                        found.append(point)
    except OSError:
        pass
    return found


def du_bytes(path, runner=None):
    """The space the files under `path` take on its filesystem (du -sx), 0 when it can't be told."""
    try:
        out = (runner or subprocess.run)(["nice", "-n", "19", "du", "-sxk", path], capture_output=True, text=True,
                                         timeout=900).stdout
        return int(out.split()[0]) * 1024
    except (OSError, subprocess.SubprocessError, ValueError, IndexError):
        return 0


def own_disk(user, settings=None, folder=PLACES_DISKS, units="/etc/systemd/system", mountinfo="/proc/self/mountinfo",
             runner=None, du=None, now=None, staging_root="/run/bipagents"):
    """The place `user` on a disk of its own (above): made, its files copied in when it had some, mounted at its home
    and at every boot. True when its home is its own disk (now or before); False when it stays as it was."""
    runner = runner or quietly
    try:
        entry = pwd.getpwnam(user)
    except KeyError:
        return False
    home, uid, gid = entry.pw_dir, entry.pw_uid, entry.pw_gid
    if home in mounted_at(home, mountinfo):
        return True
    size = disk_gib(settings or {}) * GIB
    used = (du or du_bytes)(home)
    if used > 0.9 * size:
        log("place %s: its files (%d MB) don't fit a disk of its own (%d MB): left as it is" % (user, used // MB,
                                                                                              size // MB))
        return False
    os.makedirs(folder, mode=0o700, exist_ok=True)
    image = os.path.join(folder, user + ".img")
    stamp = int(time.time() if now is None else now)
    if not os.path.exists(image):
        with open(image, "wb") as fh:
            fh.truncate(size)   # sparse: nothing taken yet
        os.chmod(image, 0o600)
        if runner("mkfs.ext4", "-q", "-F", "-m", "0", "-i", "8192", "-E", "root_owner=%d:%d" % (uid, gid),
                  image) != 0:
            log("place %s: its own disk not made (mkfs)" % user)
            os.unlink(image)
            return False
    # Its services stop while its files are copied (none run yet for a new place): nothing writes meanwhile.
    runner("systemctl", "stop", "user@%d.service" % uid)
    runner("pkill", "-KILL", "-u", user)
    for point in reversed(mounted_at(home, mountinfo)):   # its encrypted space, its containers' layers
        runner("umount", "-l", point)
    staging = os.path.join(staging_root, "disk-" + user)
    os.makedirs(staging, exist_ok=True)
    copied = runner("mount", "-o", "loop,nosuid,nodev", image, staging) == 0 \
        and runner("cp", "-a", "--sparse=always", home + "/.", staging + "/") == 0
    runner("umount", staging)
    old = os.path.join(folder, "%s.before-%d" % (user, stamp))
    if copied:
        try:
            os.rename(home, old)
        except OSError:
            copied = False
    if not copied:
        log("place %s: its files not copied to its own disk: left as it is" % user)
        os.unlink(image)
        runner("systemctl", "start", "user@%d.service" % uid)
        return False
    os.mkdir(home, 0o700)
    os.chown(home, uid, gid)
    unit = mount_unit(home)
    with open(os.path.join(units, unit), "w") as fh:
        fh.write(MOUNT_UNIT % {"user": user, "uid": uid, "image": image, "home": home})
    drop_in = os.path.join(units, "user@%d.service.d" % uid)
    os.makedirs(drop_in, exist_ok=True)
    with open(os.path.join(drop_in, "50-bipagents-disk.conf"), "w") as fh:
        fh.write(USER_DROP_IN % home)
    runner("systemctl", "daemon-reload")
    if runner("systemctl", "enable", "--now", unit) != 0 or home not in mounted_at(home, mountinfo):
        log("place %s: its own disk not mounted: its files back where they were" % user)
        runner("systemctl", "disable", unit)
        for path in (os.path.join(units, unit), os.path.join(drop_in, "50-bipagents-disk.conf")):
            with contextlib.suppress(OSError):
                os.unlink(path)
        runner("systemctl", "daemon-reload")
        os.rmdir(home)
        os.rename(old, home)
        os.unlink(image)
        runner("systemctl", "start", "user@%d.service" % uid)
        return False
    if os.path.exists("/var/lib/systemd/linger/" + user):
        runner("systemctl", "start", "user@%d.service" % uid)
    log("place %s: on its own disk (%d GB), its old home kept until %s" % (user, size // GIB,
                                                                          iso(stamp + OLD_HOME_DAYS * 86400)))
    return True


def drop_disk(user, folder=PLACES_DISKS, units="/etc/systemd/system", runner=None):
    """The place's own disk goes with its account (`wipe`): unmounted, its units and its image removed."""
    runner = runner or quietly
    try:
        entry = pwd.getpwnam(user)
    except KeyError:
        return
    unit = mount_unit(entry.pw_dir)
    runner("systemctl", "disable", "--now", unit)
    for path in (os.path.join(units, unit), os.path.join(units, "user@%d.service.d" % entry.pw_uid,
                                                          "50-bipagents-disk.conf"),
                 os.path.join(folder, user + ".img"), os.path.join(RUN_DISK, "%d.json" % entry.pw_uid)):
        with contextlib.suppress(OSError):
            os.unlink(path)
    runner("systemctl", "daemon-reload")


def old_homes(folder=PLACES_DISKS, now=None):
    """The homes kept aside by a move to an own disk, removed after OLD_HOME_DAYS."""
    now = time.time() if now is None else now
    try:
        names = os.listdir(folder)
    except OSError:
        return
    for name in names:
        stamp = name.rpartition(".before-")[2]
        if ".before-" in name and stamp.isdigit() and now - int(stamp) > OLD_HOME_DAYS * 86400:
            shutil.rmtree(os.path.join(folder, name), ignore_errors=True)
            log("old home %s removed" % name)


def disk_level(used, size, before):
    """ok, low or full for `used` of `size`, leaving a level only once clearly under it."""
    ratio = used / size if size else 0.0
    if ratio >= DISK_FULL or (before == "full" and ratio >= DISK_FULL_BACK):
        return "full"
    if ratio >= DISK_LOW or (before in ("low", "full") and ratio >= DISK_LOW_BACK):
        return "low"
    return "ok"


def place_homes():
    """number -> (home, uid, gid) of each place's account."""
    homes = {}
    for number in range(1, MAX_SEATS + 1):
        try:
            entry = pwd.getpwnam(account(number))
        except KeyError:
            continue
        homes[str(number)] = (entry.pw_dir, entry.pw_uid, entry.pw_gid)
    return homes


@contextlib.contextmanager
def state_lock(path):
    """The disk state's read-modify-write, one at a time (the round and the 15 s samples both write it)."""
    import fcntl
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".lock", "a") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)


def reserve_bytes(home, owner=0):
    """The room the place's reserve holds now (0: none, or not root's)."""
    import stat as stat_mod
    try:
        found = os.lstat(os.path.join(home, RESERVE))
    except OSError:
        return 0
    if not stat_mod.S_ISREG(found.st_mode) or found.st_uid != owner:
        return 0
    return found.st_blocks * 512


def immutable(fd, on):
    """The file's immutable flag set or cleared, through its open descriptor (never a name the place could swap)."""
    import fcntl
    import struct
    flags = bytearray(struct.pack("i", 0))
    fcntl.ioctl(fd, FS_IOC_GETFLAGS, flags, True)
    value = struct.unpack("i", bytes(flags))[0]
    value = value | FS_IMMUTABLE_FL if on else value & ~FS_IMMUTABLE_FL
    fcntl.ioctl(fd, FS_IOC_SETFLAGS, struct.pack("i", value))


def make_reserve(home, size=RESERVE_MB * MB):
    """The reserve taken in the place's own disk: allocated (nothing written: the server's image stays sparse), root's,
    immutable. False when it could not be (no room, a file of the place under that name)."""
    path = os.path.join(home, RESERVE)
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    except OSError:
        return False
    try:
        os.posix_fallocate(fd, 0, size)
    except (OSError, AttributeError):   # (no posix_fallocate: not Linux)
        os.close(fd)
        with contextlib.suppress(OSError):
            os.unlink(path)
        return False
    with contextlib.suppress(OSError):
        immutable(fd, True)
    os.close(fd)
    return True


def release_reserve(home, owner=0):
    """The reserve given back to the place: emptied and removed. Returns the room it held (0: there was none)."""
    import stat as stat_mod
    path = os.path.join(home, RESERVE)
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    except OSError:
        return 0
    try:
        found = os.fstat(fd)
        if not stat_mod.S_ISREG(found.st_mode) or found.st_uid != owner:
            return 0
        with contextlib.suppress(OSError):
            immutable(fd, False)
        held = found.st_blocks * 512
        try:
            writer = os.open(path, os.O_WRONLY | getattr(os, "O_NOFOLLOW", 0))
        except OSError:
            return 0
        try:
            again = os.fstat(writer)
            if (again.st_dev, again.st_ino) != (found.st_dev, found.st_ino):
                return 0
            os.ftruncate(writer, 0)   # the room back at once, whatever happens to the name
        finally:
            os.close(writer)
        with contextlib.suppress(OSError):
            if os.lstat(path).st_ino == found.st_ino:
                os.unlink(path)
        return held
    finally:
        os.close(fd)


def measure_disk(home, own, settings, statvfs=None, du=None):
    """(size, used, free, reserve) of the place in bytes: on its own disk, used leaves its reserve out and free is
    what its account may still write (free None without its own disk)."""
    if not own:
        return disk_gib(settings) * GIB, (du or du_bytes)(home), None, 0
    fs = (statvfs or os.statvfs)(home)
    size = fs.f_blocks * fs.f_frsize
    used = size - fs.f_bfree * fs.f_frsize
    free = getattr(fs, "f_bavail", fs.f_bfree) * fs.f_frsize
    reserve = reserve_bytes(home)
    return size, max(0, used - reserve), free, reserve


def disk_step(number, home, own, measured, told, make, release=None, make_one=None):
    """One place's level and pause from its measure, the reserve given back when its room runs out and taken again
    (`make`: the round's, not the 15 s samples') once it is out of "full" with room for it twice."""
    size, used, free, reserve = measured
    if own and reserve and free <= RELEASE_MB * MB:
        given = (release or release_reserve)(home)
        if given:
            log("place %s: its disk is full: its reserve given back (%d MB) so that its agents still answer"
                % (number, given // MB))
            free, reserve = free + given, 0
    level = disk_level(used, size, told.get("state"))
    if own:
        pause = level == "full" and not reserve and (free <= PAUSE_MB * MB
                                                      or (told.get("pause") is True and free < RESERVE_MB * MB))
        if make and not reserve and level != "full" and free >= 2 * RESERVE_MB * MB:
            if (make_one or make_reserve)(home):
                log("place %s: its disk's reserve taken again" % number)
    else:
        pause = level == "full"
    return level, pause, used, size


def tell_place(home, uid, gid, value, home_too=True, folder=RUN_DISK):
    """The place's level for its bridge: in memory first (always room there), then in its home."""
    try:
        os.makedirs(folder, mode=0o755, exist_ok=True)
        path = os.path.join(folder, "%d.json" % uid)
        fd = os.open(path + ".tmp", os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o640)
        with os.fdopen(fd, "w") as fh:
            json.dump(value, fh)
            os.fchown(fh.fileno(), -1, gid)
            os.fchmod(fh.fileno(), 0o640)
        os.replace(path + ".tmp", path)
    except OSError as exc:
        log("disk state not kept in memory for %s: %s" % (home, exc))
    if home_too:
        try:
            write_credit(os.path.join(home, PLACE_DISK_FILE), value, uid, gid, home)
        except OSError as exc:
            log("disk state not written in %s: %s" % (home, exc))


def disk_watch(settings, now=None, homes=None, path=DISK_STATE, mountinfo="/proc/self/mountinfo", statvfs=None,
               du=None, folder=RUN_DISK, release=None, make_one=None):
    """Each place's disk, measured every DISK_EVERY (above): its level written for its bridge, and for the beat
    {number: {disk_mb, disk_size_mb, disk, own_disk}}. A place entering "full" is logged once."""
    now = time.time() if now is None else now
    with state_lock(path):
        kept = read_json(path)
        if now - (kept.get("at") or 0) < DISK_EVERY and isinstance(kept.get("places"), dict):
            return kept["places"]
        before = kept.get("places") if isinstance(kept.get("places"), dict) else {}
        told_all = kept.get("told") if isinstance(kept.get("told"), dict) else {}
        out, told_now = {}, {}
        for number, (home, uid, gid) in sorted((homes if homes is not None else place_homes()).items()):
            own = home in mounted_at(home, mountinfo)
            told = told_all.get(number) if isinstance(told_all.get(number), dict) else {}
            told = dict(told, state=told.get("state") or (before.get(number) or {}).get("disk"))
            level, pause, used, size = disk_step(number, home, own, measure_disk(home, own, settings, statvfs, du),
                                                 told, True, release, make_one)
            was = told.get("state")
            if level == "full" and was != "full":
                log("place %s: disk full (%d of %d MB)%s" % (number, used // MB, size // MB,
                                                             ", its agents' workspaces paused" if pause else ""))
            elif level != "full" and was == "full":
                log("place %s: room on its disk again" % number)
            tell_place(home, uid, gid, {"state": level, "pause": pause, "at": int(now)}, folder=folder)
            out[number] = {"disk_mb": used // MB, "disk_size_mb": size // MB, "disk": level, "own_disk": own}
            told_now[number] = {"state": level, "pause": pause}
        try:
            write_json(path, {"at": now, "places": out, "told": told_now})
        except OSError as exc:
            log("disk state not kept: %s" % exc)
    return out


def disk_quick(now=None, homes=None, path=DISK_STATE, mountinfo="/proc/self/mountinfo", statvfs=None,
               folder=RUN_DISK, release=None):
    """--sample (every 15 s): the places on their own disk measured (statvfs only), the reserve given back as soon as
    their room runs out, a new level or pause told at once (its home's file only then), the in-memory one kept fresh.
    The round's measure (`disk_watch`) keeps its own pace."""
    now = time.time() if now is None else now
    homes = homes if homes is not None else place_homes()
    if not homes:
        return 0
    with state_lock(path):
        kept = read_json(path)
        places_now = kept.get("places") if isinstance(kept.get("places"), dict) else {}
        told_all = kept.get("told") if isinstance(kept.get("told"), dict) else {}
        changed = False
        for number, (home, uid, gid) in sorted(homes.items()):
            if home not in mounted_at(home, mountinfo):
                continue
            told = told_all.get(number) if isinstance(told_all.get(number), dict) else {}
            try:
                measured = measure_disk(home, True, {}, statvfs)
            except OSError:
                continue
            level, pause, used, size = disk_step(number, home, True, measured, told, False, release)
            new = {"state": level, "pause": pause}
            fresh = new != {"state": told.get("state"), "pause": told.get("pause")}
            if fresh:
                log("place %s: disk %s%s (%d of %d MB, told at once)" % (number, level, ", workspaces paused"
                                                                         if pause else "", used // MB, size // MB))
                told_all[number] = new
                entry = places_now.get(number) if isinstance(places_now.get(number), dict) else {}
                places_now[number] = dict(entry, disk_mb=used // MB, disk_size_mb=size // MB, disk=level,
                                          own_disk=True)
                changed = True
            tell_place(home, uid, gid, dict(new, at=int(now)), home_too=fresh, folder=folder)
        if changed:
            try:
                write_json(path, dict(kept, places=places_now, told=told_all))
            except OSError as exc:
                log("disk state not kept: %s" % exc)
    return 0


def with_disk(places_now, disk):
    """The beat's places with each one's disk."""
    for number, entry in disk.items():
        places_now.setdefault(number, {}).update(entry)
    return places_now


BUSY = ".local/state/bipagents/busy"                 # a run goes on (bridge-rs/src/runs.rs, rewritten every minute)
BUSY_FRESH = 180
BRIDGE_DATA = ".local/share/hermes-bridge"           # its bridge's data_dir: time.json, sleep.json
HERMES_HOME = ".hermes"


def iso(seconds):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(seconds))


def next_routine(home):
    """The next run of the place's routines (Hermes' cron jobs of every agent), as Unix seconds, else None."""
    from datetime import datetime, timezone
    soonest = None
    profiles = os.path.join(HERMES_HOME, "profiles")
    files = [os.path.join(profiles, name, "cron", "jobs.json") for name in sorted(home_list(home, profiles))]
    for rel in files + [os.path.join(HERMES_HOME, "cron", "jobs.json")]:
        raw = home_read(home, rel)
        try:
            data = json.loads(raw.decode("utf-8")) if raw is not None else None
        except (ValueError, RecursionError):
            continue
        jobs = data if isinstance(data, list) else data.get("jobs") if isinstance(data, dict) else None
        for job in jobs if isinstance(jobs, list) else []:
            if not isinstance(job, dict) or job.get("enabled") is False \
                    or job.get("state") in ("paused", "completed", "deleted"):
                continue
            try:
                at = datetime.fromisoformat(str(job.get("next_run_at")).replace("Z", "+00:00"))
            except ValueError:
                continue
            seconds = (at if at.tzinfo else at.replace(tzinfo=timezone.utc)).timestamp()
            soonest = seconds if soonest is None else min(soonest, seconds)
    return soonest


def activity(home, now=None):
    """What the place's bridge says of it in its files (nothing asked of it): `busy` (a turn, a background task or a
    Live call going on), `tz` (the person's zone the apps gave), `last_active_at` (its last request from an app, to ten
    minutes), `next_routine_at` (its routines' next run). What is not known is left out."""
    now = time.time() if now is None else now
    out = {}
    busy = home_stat(home, BUSY)
    out["busy"] = busy is not None and now - busy.st_mtime < BUSY_FRESH
    person = home_json(home, os.path.join(BRIDGE_DATA, "time.json"))
    zone = person.get("time_zone") or person.get("device")
    if isinstance(zone, str) and 0 < len(zone) <= 64 and all(c.isalnum() or c in "/_+-" for c in zone):
        out["tz"] = zone
    last = home_json(home, os.path.join(BRIDGE_DATA, "sleep.json")).get("last")
    if isinstance(last, (int, float)) and not isinstance(last, bool) and last > 0:
        out["last_active_at"] = iso(last)
    soonest = next_routine(home)
    if soonest is not None:
        out["next_routine_at"] = iso(soonest)
    out["own_spaces"] = own_spaces(home)
    found = measures(home)
    if found:
        out["measures"] = found
    return out


MEASURES = os.path.join(BRIDGE_DATA, "measures.json")   # the beta's measures (bridge-rs/src/beta_measures.rs)
MEASURE_TIMES = ("guide_message_at", "first_accept_at", "routine_at")
MEASURE_COUNTS = ("team_proposed", "team_accepted", "team_refused", "team_ticked")


def measures(home):
    """The beta's measures its bridge keeps: times and counts only, nothing of anyone; anything else is left out."""
    found = home_json(home, MEASURES)

    def number(value):
        return isinstance(value, (int, float)) and not isinstance(value, bool) and 0 <= value < 1e11

    out = {key: found[key] for key in MEASURE_TIMES if number(found.get(key)) and found[key] > 0}
    out.update({key: int(found[key]) for key in MEASURE_COUNTS if number(found.get(key)) and found[key] < 10000})
    hours = found.get("message_hours")
    if isinstance(hours, list):
        out["message_hours"] = sorted({int(h) for h in hours if number(h)})[-240:]
    return out


SPACES = os.path.join(BRIDGE_DATA, "spaces")         # its bridge's agents in their own space (bridge-rs/src/spaces.rs)


def own_spaces(home):
    """How many of the place's agents run in their own space (own Hermes and container): the guichet weighs each
    (weights.rs, `own_space_agent_mb`)."""
    found = 0
    for name in home_list(home, SPACES):
        profiles = home_stat(home, os.path.join(SPACES, name, "profiles"))
        found += profiles is not None and stat_mod.S_ISDIR(profiles.st_mode)
    return found


def places_activity(memory):
    """The beat's places: each one's memory and what its bridge's files say."""
    for number, entry in memory.items():
        try:
            home = pwd.getpwnam(account(int(number))).pw_dir
        except KeyError:
            continue
        try:
            entry.update(activity(home))
        except (OSError, ValueError, TypeError, AttributeError):
            pass
    return memory


def sampler(lib=LIB, units="/etc/systemd/system", runner=None):
    """bipagents-memory.timer, set up once (servers installed before 2026-10-26 included)."""
    wanted = {"bipagents-memory.service": SAMPLER_SERVICE % lib, "bipagents-memory.timer": SAMPLER_TIMER}
    changed = False
    for name, text in wanted.items():
        path = os.path.join(units, name)
        try:
            with open(path) as fh:
                if fh.read() == text:
                    continue
        except OSError:
            pass
        with open(path, "w") as fh:
            fh.write(text)
        changed = True
    if changed:
        (runner or run)("systemctl", "daemon-reload")
        (runner or run)("systemctl", "enable", "--now", "bipagents-memory.timer")


def zram_on(swaps="/proc/swaps"):
    try:
        with open(swaps) as fh:
            return any(line.startswith("/dev/zram") for line in fh)
    except OSError:
        return False


def quietly(*args):
    try:
        return subprocess.run(list(args), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=900,
                              env=dict(os.environ, DEBIAN_FRONTEND="noninteractive")).returncode
    except (OSError, subprocess.SubprocessError):
        return 1


def zram(swaps="/proc/swaps", tried=ZRAM_TRIED, conf="/etc/systemd/zram-generator.conf", runner=quietly):
    """Compressed swap in memory (zram, half the memory, ahead of the disk swap) on a server that has none yet
    (installed before 2026-10-26, or its first boot could not): as user-data.yaml does, tried again an hour later
    when it did not work (apt busy...)."""
    if zram_on(swaps):
        return True
    try:
        if time.time() - os.stat(tried).st_mtime < 3600:
            return False
    except OSError:
        pass
    write_json(tried, {"at": int(time.time())})
    # Ubuntu's cloud kernels keep zram in linux-modules-extra (DigitalOcean, 2026-10-26), for the kernel running now:
    # after a kernel update and the night's reboot it is set up again here.
    extra = "linux-modules-extra-" + os.uname().release
    loaded = runner("modprobe", "zram") == 0 or (runner("apt-get", "install", "-y", extra) == 0
                                                  and runner("modprobe", "zram") == 0)
    if not loaded or runner("apt-get", "install", "-y", "systemd-zram-generator") != 0:
        log("zram not set up yet (tried again in an hour)")
        return False
    with open(conf, "w") as fh:
        fh.write(ZRAM_CONF)
    runner("systemctl", "daemon-reload")
    runner("systemctl", "restart", "systemd-zram-setup@zram0.service")   # the device, with these settings
    runner("systemctl", "start", "dev-zram0.swap")                         # in use as swap (waited for)
    on = zram_on(swaps)
    log("zram %s" % ("on" if on else "not on yet"))
    return on


# -- planned moves (place-move.py, 2026-10-26): kept apart from the rest -----------------------------------------------

MOVE_ACTIONS = ("move_out", "move_in", "move_cancel")
MOVES = "/var/lib/bipagents/moves"
MOVE_ID_CHARS = "abcdefghijklmnopqrstuvwxyz0123456789-"


def move_valid(body):
    """A move order's own fields: its id, and for move_out / move_in the one-time key and the other server."""
    move = body.get("move")
    if not isinstance(move, dict):
        return False
    move_id = move.get("id")
    if not (isinstance(move_id, str) and 6 <= len(move_id) <= 63 and all(c in MOVE_ID_CHARS for c in move_id)
            and move_id[0] != "-"):
        return False
    if body["action"] == "move_cancel":
        return True
    key = move.get("key")
    if not (isinstance(key, str) and 43 <= len(key) <= 88 and all(c in PAIR_CHARS for c in key.rstrip("="))):
        return False
    other = move.get("target") if body["action"] == "move_out" else move.get("source")
    if not (isinstance(other, dict) and isinstance(other.get("address"), str) and _address(other["address"])):
        return False
    port = (other.get("port") if body["action"] == "move_out" else move.get("port")) or 2222
    window = move.get("window")
    if "drain" in move and not isinstance(move["drain"], bool):   # false: a place whose bridge is down, not drained
        return False
    return isinstance(port, int) and 1024 <= port <= 65535 and (window is None or (
        isinstance(window, dict) and all(isinstance(window.get(k), (int, float)) for k in ("start", "end"))))


def _address(text):
    import ipaddress
    try:
        ipaddress.ip_address(text)
        return True
    except ValueError:
        return False


def _place_move():
    import importlib.util
    spec = importlib.util.spec_from_file_location("place_move", os.path.join(LIB, "place-move.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def move(body, moves=MOVES, lib=LIB, run=subprocess.run):
    """The order to place-move.py: the target's listener at once, the source's move in a unit of its own (it lasts
    from minutes to hours), a cancel either side."""
    move_id = body["move"]["id"]
    script = os.path.join(lib, "place-move.py")
    if body["action"] == "move_cancel":
        run(["/usr/bin/python3", script, "cancel", move_id])
        return
    folder = os.path.join(moves, move_id)
    os.makedirs(folder, mode=0o700, exist_ok=True)
    path = os.path.join(folder, "order.json")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(body, fh)
    log("place %s: %s %s" % (body["seat"], body["action"], move_id))
    if body["action"] == "move_in":
        run(["/usr/bin/python3", script, "in", path])
    else:
        run(["systemd-run", "--unit", "bipagents-move-%s" % move_id, "--collect", "--property=Nice=5",
             "/usr/bin/python3", script, "out", path])


def marked_removed(body):
    """The guichet marks the place removed: `"removed": true` (place-moves.md), or `"remove": true`, what its
    orders::remove signed until 2026-10-30 (orders signed then are still served for two days)."""
    return body.get("removed") is True or body.get("remove") is True


def remove_valid(body):
    """`remove`: the place left behind by a move, named by that move's id, or marked removed by the guichet."""
    move = body.get("move")
    if isinstance(move, dict):
        return move_valid({"action": "move_cancel", "move": move})
    return marked_removed(body)


def moved_out(number, move_id, moves=MOVES):
    """This seat's move_out with that id completed here."""
    try:
        with open(os.path.join(moves, move_id, "state.json")) as fh:
            state = json.load(fh)
    except (OSError, ValueError):
        return False
    return (state.get("role") == "out" and int(state.get("seat", 0)) == number
            and state.get("phase") in ("done", "removed"))


MOVE_OVER = ("done", "failed", "cancelled", "closed", "removed")
MOVE_STALE = 8 * 3600   # a move silent this long is over (its listener closes after 6 h, the guichet gives up then)
MOVED_AWAY = ".config/bipagents/moved-away.json"   # the place's copy left by a move that completed (place-move.py)


def moving(number, moves=MOVES, now=None):
    """The id of a move under way here for place n (leaving or arriving), else None."""
    now = time.time() if now is None else now
    try:
        names = sorted(os.listdir(moves))
    except OSError:
        return None
    for name in names:
        state = read_json(os.path.join(moves, name, "state.json"))
        try:
            same = int(state.get("seat", 0)) == number and now - float(state.get("updated") or 0) < MOVE_STALE
        except (TypeError, ValueError):
            continue
        if same and state.get("role") in ("out", "in") and state.get("phase") not in MOVE_OVER:
            return name
    return None


def update_order(number, commit, moves=MOVES, home=None, updater=None, repo=None, pretest=False):
    """An update order while place n moves: held (False: not done, the guichet serves it again at the next beat, and
    it applies if the move fails or is called off); for the stopped copy a completed move left, dropped (True: an
    update would start it again beside the place that moved). Else done. True when the order is done with."""
    under_way = moving(number, moves)
    if under_way:
        log("update of place %d held: its move %s is under way" % (number, under_way))
        return False
    if home is None:
        try:
            home = pwd.getpwnam(account(number)).pw_dir
        except KeyError:
            home = None
    if home and home_stat(home, MOVED_AWAY) is not None:
        log("update of place %d dropped: the place moved away (its copy stays stopped)" % number)
        return True
    try:
        if pretest:
            (updater or update)(number, commit, repo, pretest=True)
        elif repo:
            (updater or update)(number, commit, repo)
        else:
            (updater or update)(number, commit)
    except (OSError, subprocess.SubprocessError) as exc:
        log("update of place %s stopped: %s" % (number, exc))
    return True


def remove(number, body, moves=MOVES, wipe_account=None, run=subprocess.run):
    """The place a move left on this server goes (the guichet's pool::vacate): its services and account, its door's
    firewall rule, its setup file. Only after its move_out completed here, and only while the account still holds
    that move's mark (a seat installed again since is someone else's), or when the guichet marks it removed. Done
    again, nothing more happens."""
    user = account(number)
    move = body.get("move")
    if isinstance(move, dict):
        move_id = move["id"]
        if not moved_out(number, move_id, moves):
            log("place %d: remove refused, its move %s did not complete here" % (number, move_id))
            return False
        try:
            mark = home_json(pwd.getpwnam(user).pw_dir, MOVED_AWAY).get("move")   # never through a link
        except KeyError:
            mark = move_id   # the account is already gone
        if mark != move_id:
            log("place %d: remove refused, the account is not the one moved by %s" % (number, move_id))
            return False
    elif not marked_removed(body):
        return False
    log("place %d (%s): removed" % (number, user))
    (wipe_account or wipe)(user)
    run(["ufw", "delete", "allow", "%d/tcp" % (442 + number)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        os.unlink("/etc/bipagents/cloud%d.json" % number)
    except OSError:
        pass
    if isinstance(move, dict):
        path = os.path.join(moves, move["id"], "state.json")
        try:
            with open(path) as fh:
                state = json.load(fh)
            state["phase"] = "removed"
            with open(path, "w") as fh:
                json.dump(state, fh)
        except (OSError, ValueError):
            pass
    return True


def add_moves(beat, lib=LIB):
    """The moves' phases and numbers in the beat (`moves`), and the moves' leftovers cleared (listeners past their
    time, source copies past their seven days)."""
    if not os.path.exists(os.path.join(lib, "place-move.py")):
        return
    try:
        module = _place_move()
        module.cleanup(wipe=wipe)
        found = module.states()
    except Exception as exc:   # never the beat's loss
        log("moves not read: %s" % exc)
        return
    if found:
        beat["moves"] = found


def read_done():
    try:
        with open(DONE) as fh:
            return {int(line) for line in fh if line.strip().isdigit()}
    except OSError:
        return set()


def reboot_queued():
    """The server is going down to start again (not to stop): systemd has a job for reboot.target."""
    try:
        jobs = subprocess.run(["systemctl", "list-jobs", "--no-legend", "--no-pager"], capture_output=True,
                              text=True, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return False
    return any(line.split()[1:2] == ["reboot.target"] for line in jobs.splitlines())


def rebooting(conf):
    """As the server shuts down: the guichet told when it is a reboot, nothing else (it is a planned one)."""
    if not reboot_queued():
        return 0
    try:
        # over IPv4: the guichet accepts the announcement only from the server's own address (audit #16)
        with ipv4_only():
            call(conf, "POST", "/v1/server/rebooting")
        log("the guichet knows this server is rebooting")
    except (OSError, ValueError) as exc:
        log("the guichet was not told of the reboot: %s" % exc)
    return 0


def disk_settled():
    """Each place's disk watched (and the old homes past their days removed), whatever the guichet answers."""
    try:
        old_homes()
        return disk_watch(read_json(SETTINGS))
    except OSError as exc:
        log("disks not watched: %s" % exc)
        return {}


def memory_settled(server):
    """Each place's memory ceiling and the sampler between beats, whatever the guichet answers."""
    try:
        ceiling(server.get("memory_mb"), read_json(SETTINGS))
        sampler()
    except OSError as exc:
        log("memory settings not applied: %s" % exc)


# -- every place's door on 443 ----------------------------------------------------------------------------------------
#
# Many networks let out only port 443 (offices, hotels, schools, home boxes with a strict firewall: Alex's own on
# 2026-10-30), so a place on door 444 and up looked offline from there. Each place keeps its own door (444, 445...),
# and root's router (HAProxy, its own unit bipagents-doors.service, nothing decrypted) also gives every place 443:
# the server's 443 is redirected to the router (an nftables table of its own, so ufw's rules are never touched), which
# reads the name the client asks for in its TLS hello (`<label>.p.bipagents.com`, the place's certificate: the
# guichet sends each place's name in its beat answer, "hosts") and passes the connection to that place's door on
# 127.0.0.1, after a PROXY v2 header with the client's real address (bridge-rs/src/proxy_header.rs: without it, every
# client would look like 127.0.0.1 to the bridge, which never counts wrong keys from there). Anything else (a hello
# without a name, from an app paired by the address; a name not routed) goes to place 1, whose door 443 was.
#
# A place's door is routed only once it understands the header (asked each round: a PROXY LOCAL header, then a TLS
# handshake; an older bridge fails it), and the redirect is on only while place 1's does, since all of 443 then goes
# through the router. The beat says which places 443 reaches ("door443"): the guichet then gives the apps 443 for
# them, and their old door again if that stops (an update's way back to an older bridge). The router's port is below
# 443, so no place's account can take it first (net.ipv4.ip_unprivileged_port_start = 443). It logs nothing.

DOORS_PORT = 440
DOORS_CONF = "/etc/bipagents/doors.cfg"
DOORS_NFT = "/etc/bipagents/doors.nft"
DOORS_STATE = "/var/lib/bipagents/doors.json"
DOORS_UNIT = "/etc/systemd/system/bipagents-doors.service"
DOORS_TABLE = "bipagents_doors"
HAPROXY = "/usr/sbin/haproxy"
HOST_CHARS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-.")
PROXY_LOCAL = b"\r\n\r\n\x00\r\nQUIT\n\x20\x00\x00\x00"   # a PROXY v2 header, command LOCAL, no address

DOORS_SERVICE = """[Unit]
Description=BipAgents: every place's door on 443 (HAProxy by the names asked, nothing decrypted)
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStartPre=%(haproxy)s -c -q -f %(conf)s
ExecStart=%(haproxy)s -Ws -f %(conf)s -p /run/bipagents-doors.pid
ExecStartPost=-/usr/sbin/nft -f %(nft)s
ExecReload=%(haproxy)s -c -q -f %(conf)s
ExecReload=/bin/kill -USR2 $MAINPID
ExecStopPost=-/bin/sh -c 'nft delete table inet %(table)s 2>/dev/null || true'
KillMode=mixed
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
""" % {"haproxy": HAPROXY, "conf": DOORS_CONF, "nft": DOORS_NFT, "table": DOORS_TABLE}


def good_host(host):
    """A place's name as the guichet gives it: a lowercase DNS name (anything else never reaches the router's
    text)."""
    return (isinstance(host, str) and 4 <= len(host) <= 253 and "." in host and set(host) <= HOST_CHARS
            and ".." not in host and host[0] not in ".-" and host[-1] not in ".-")


def door_hosts(answer):
    """{number: name} of the beat answer's "hosts": valid names only, and none given to two places."""
    given = answer.get("hosts") if isinstance(answer, dict) else None
    if not isinstance(given, dict):
        return {}
    hosts = {}
    for key, host in given.items():
        try:
            number = int(key)
        except (TypeError, ValueError):
            continue
        if 1 <= number <= MAX_SEATS and good_host(host):
            hosts[number] = host
    counts = {}
    for host in hosts.values():
        counts[host] = counts.get(host, 0) + 1
    return {n: h for n, h in hosts.items() if counts[h] == 1}


def proxy_probe(port, timeout=4):
    """Whether the door on `port` understands a PROXY header: True (a TLS handshake after a LOCAL header), False (it
    refuses that one but takes a plain handshake: an older bridge), None (no answer either way: down, starting)."""
    import ssl
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE

    def handshake(header):
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=timeout) as sock:
                if header:
                    sock.sendall(PROXY_LOCAL)
                with context.wrap_socket(sock) as tls:
                    tls.do_handshake()
            return True
        except (OSError, ValueError):
            return False

    if handshake(True):
        return True
    return False if handshake(False) else None


def doors_config(routes, default, ipv6):
    """The router's text: each name to its place's door, anything else to `default` (a number or None)."""
    lines = [
        "# BipAgents: every place's door on 443 (installer/cloud/seat-agent.py writes it, each round)",
        "global",
        "    user haproxy",
        "    group haproxy",
        "    maxconn 8000",
        "    nbthread 2",
        "",
        "defaults",
        "    mode tcp",
        "    timeout connect 5s",
        "    timeout client 6h",
        "    timeout server 6h",
        "    timeout client-fin 30s",
        "    timeout server-fin 30s",
        "",
        "frontend doors",
        "    bind 0.0.0.0:%d" % DOORS_PORT,
    ]
    if ipv6:
        lines.append("    bind [::]:%d v6only" % DOORS_PORT)
    lines += [
        "    tcp-request inspect-delay 5s",
        "    tcp-request content accept if { req.ssl_hello_type 1 }",
    ]
    for host, number in sorted(routes.items()):
        lines.append("    use_backend place%d if { req.ssl_sni -i %s }" % (number, host))
    if default:
        # everything else to place 1, as 443 did before the router: a hello without a name, a name not routed (yet:
        # a guichet that gives no names, a place whose bridge is older); a name of another place lands on place 1's
        # certificate there, which the client refuses, as before
        lines.append("    default_backend place%d" % default)
    for number in sorted(set(routes.values()) | ({default} if default else set())):
        lines += ["", "backend place%d" % number,
                  "    server door 127.0.0.1:%d send-proxy-v2" % (442 + number)]
    return "\n".join(lines) + "\n"


def doors_nft(active):
    """The redirect of 443 to the router, as a file `nft -f` loads (the table replaced at once; only removed when
    off)."""
    text = "table inet %s\ndelete table inet %s\n" % (DOORS_TABLE, DOORS_TABLE)
    if active:
        text += ("table inet %s {\n"
                 "  chain prerouting {\n"
                 "    type nat hook prerouting priority dstnat; policy accept;\n"
                 "    tcp dport 443 redirect to :%d\n"
                 "  }\n"
                 "}\n") % (DOORS_TABLE, DOORS_PORT)
    return text


def write_if_changed(path, text, mode=0o644):
    try:
        with open(path) as fh:
            if fh.read() == text:
                return False
    except OSError:
        pass
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "w") as fh:
        fh.write(text)
    os.chmod(path + ".tmp", mode)
    os.replace(path + ".tmp", path)
    return True


def haproxy_ready(runner=quietly, binary=HAPROXY):
    """HAProxy installed (once; the distribution's own service, unused, off)."""
    if os.path.exists(binary):
        return True
    if shutil.which("apt-get"):
        runner("apt-get", "install", "-y", "--no-install-recommends", "haproxy")
    elif shutil.which("dnf"):
        runner("dnf", "install", "-y", "haproxy")
    if not os.path.exists(binary):
        log("doors on 443: HAProxy not installed yet")
        return False
    runner("systemctl", "disable", "--now", "haproxy.service")
    return True


def has_ipv6(path="/proc/net/if_inet6"):
    try:
        with open(path) as fh:
            return any(line.split()[-1] != "lo" for line in fh if line.strip())
    except OSError:
        return False


def doors(answer, state_path=DOORS_STATE, runner=quietly, probe=proxy_probe, exists=None, ready=None):
    """The router and the redirect as this round's answer and the places' doors say; returns the numbers 443 reaches
    (kept for the next beat)."""
    exists = exists or (lambda n: _account_exists(account(n)))
    state = read_json(state_path)
    capable = state.get("proxy") if isinstance(state.get("proxy"), dict) else {}
    for number in range(1, MAX_SEATS + 1):
        if not exists(number):
            capable.pop(str(number), None)
            continue
        seen = probe(442 + number)
        if seen is not None:   # down or starting: what it said last time stays
            capable[str(number)] = seen
    hosts = door_hosts(answer)
    routes = {host: n for n, host in hosts.items() if capable.get(str(n)) is True}
    # all of 443 goes through the router once the redirect is on: only while place 1, whose door it was, takes it
    active = capable.get("1") is True
    if not active:
        reached = []
        if state.get("on"):
            write_if_changed(DOORS_NFT, doors_nft(False))
            runner("nft", "-f", DOORS_NFT)
            runner("systemctl", "disable", "--now", "bipagents-doors.service")
            log("doors on 443: off (place 1's door does not take the router's header)")
        write_json(state_path, {"proxy": capable, "on": False, "reached": reached})
        return reached
    if not (ready or haproxy_ready)(runner=runner):
        write_json(state_path, {"proxy": capable, "on": False, "reached": []})
        return []
    if write_if_changed(DOORS_UNIT, DOORS_SERVICE):
        runner("systemctl", "daemon-reload")
        runner("systemctl", "enable", "bipagents-doors.service")
    if runner("ufw", "status") == 0:
        runner("ufw", "allow", "%d/tcp" % DOORS_PORT)
    config_changed = write_if_changed(DOORS_CONF, doors_config(routes, 1, has_ipv6()))
    nft_changed = write_if_changed(DOORS_NFT, doors_nft(True))
    if runner("systemctl", "is-active", "--quiet", "bipagents-doors.service") != 0:
        started = runner("systemctl", "start", "bipagents-doors.service") == 0
    else:
        started = (runner("systemctl", "reload", "bipagents-doors.service") == 0) if config_changed else True
    if started and (nft_changed or runner("nft", "list", "table", "inet", DOORS_TABLE) != 0):
        runner("nft", "-f", DOORS_NFT)
    reached = sorted(set(routes.values()) | {1}) if started else []
    if config_changed or nft_changed:
        log("doors on 443: on, places %s" % (reached or "none"))
    write_json(state_path, {"proxy": capable, "on": bool(started), "reached": reached})
    return reached


def doors_reached(state_path=DOORS_STATE):
    """The places 443 reached at the last round (the beat tells the guichet)."""
    reached = read_json(state_path).get("reached")
    return {int(n) for n in reached if isinstance(n, int)} if isinstance(reached, list) else set()


def _account_exists(user):
    try:
        pwd.getpwnam(user)
        return True
    except KeyError:
        return False


def beat_of(server, disk, moves=True):
    """The beat's body: each place's health (and whether 443 reaches its door), the server's vitals, each place's
    memory, disk and what its bridge's files say, and (`moves`) the planned moves' phases."""
    seats = health()
    for number in doors_reached():
        if str(number) in seats:
            seats[str(number)]["door443"] = True
    beat = {"seats": seats, "server": server, "places": with_disk(places_activity(places_memory()), disk)}
    if moves:
        add_moves(beat)
    return beat


# A run whose orders take long (a place's update waiting up to 5 minutes for its turns to end, then restarting what
# changed; the next place's after it; an install; a move) beats again this often from a thread: the timer starts no
# other run meanwhile, and the guichet takes a server silent for its down_minutes (10) for down (2026-11-07).
BEAT_EVERY = 120.0


class Beating(object):
    """While the orders run: the beat again every `every` seconds, its answer left (the run's orders, settings and
    credit are its first beat's). Without the moves: their cleanup and states belong to the run's own thread, and
    the next run reports them."""

    def __init__(self, conf, disk, every=None, send=None):
        self.conf, self.disk = conf, disk
        self.every = BEAT_EVERY if every is None else every
        self.send = send or self._beat
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._loop, name="beat", daemon=True)

    def _beat(self):
        answer = call(self.conf, "POST", "/v1/server/beat", beat_of(vitals(), self.disk, moves=False))
        keep_offer_cards(answer)   # Kerfy Studio's cut of card payments, even while a long order runs

    def _loop(self):
        while not self.stop.wait(self.every):
            try:
                self.send()
            except Exception as exc:   # never the orders' loss
                log("beat between orders not sent: %s" % exc)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.stop.set()
        self.thread.join(timeout=1)   # a beat under way ends with the process (a daemon thread)
        return False


def main(argv=()):
    if "--sample" in argv:
        return sample()
    if "--own-disk" in argv:   # add-seat.sh and user-data.yaml, before a place's install: its own 5 GB disk
        at = list(argv).index("--own-disk")
        number = int(argv[at + 1]) if len(argv) > at + 1 and argv[at + 1].isdigit() else 0
        if not 1 <= number <= MAX_SEATS:
            return 2
        return 0 if own_disk(account(number), read_json(SETTINGS)) else 1
    if "--ceiling" in argv:   # add-seat.sh, as soon as a place's account starts
        ceiling(vitals().get("memory_mb"), read_json(SETTINGS))
        return 0
    if "--prepare" in argv:   # add-seat.sh, before a place's install: Hermes and the images of its version, shared
        at = list(argv).index("--prepare")
        commit = argv[at + 1] if len(argv) > at + 1 else "main"
        repo = argv[at + 2] if len(argv) > at + 2 and argv[at + 2] in KNOWN_REPOS else None   # add-seat.sh's setup
        try:
            prepare_shared(commit, repo)
        except (OSError, subprocess.SubprocessError) as exc:
            log("shared Hermes and images for %s not ready: %s" % (commit[:12], exc))
        try:
            views_again()   # a place installed again: its new account's view, whatever the pull did
        except (OSError, subprocess.SubprocessError) as exc:
            log("views of the shared images not mounted: %s" % exc)
        return 0
    if "--bootstrap" in argv:   # the first-boot script, before any place is installed
        return bootstrap()
    with open(CONF) as fh:
        conf = json.load(fh)
    if not conf.get("token"):   # its first boot never got as far as its own token: nothing to tell the guichet
        log("no token for the guichet yet (first boot not finished)")
        return 0
    if "--rebooting" in argv:
        return rebooting(conf)
    metadata_block()
    try:
        hold_reboot()   # first: even while the guichet doesn't answer
    except (OSError, subprocess.SubprocessError) as exc:
        log("planned reboot not looked at: %s" % exc)
    try:
        backup_lock()
    except OSError as exc:
        log("backup lock not made: %s" % exc)
    server = vitals()
    memory_settled(server)
    disk = disk_settled()
    try:
        answer = call(conf, "POST", "/v1/server/beat", beat_of(server, disk))
        orders = answer.get("orders") or []
    except (OSError, ValueError) as exc:
        log("guichet unreachable: %s" % exc)
        zram()
        return 0
    try:
        keep_settings(answer)
    except OSError as exc:
        log("settings not kept: %s" % exc)
    keep_credit(answer)
    keep_offer_cards(answer)
    try:
        doors(answer)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        log("doors on 443 not set: %s" % exc)
    memory_settled(server)
    done = read_done()
    with Beating(conf, disk):   # a long order (an update waiting for its turns...): the server still beats
        for order in orders:
            if int(order.get("id", 0)) in done:
                body = None
            elif not verified(str(order.get("body", "")), str(order.get("signature", ""))):
                log("order %s refused: not signed by the guichet" % order.get("id"))
                continue
            else:
                body = accepted(order, conf["server"], done, time.time())
                if body is None:
                    continue
                if body["action"] == "update":
                    where = {"repo": body["repo"]} if body.get("repo") else {}
                    if body.get("pretest") is True:
                        where["pretest"] = True
                    if not update_order(int(body["seat"]), body["commit"], **where):
                        continue   # held while the place moves: neither noted nor told done
                elif body["action"] in MOVE_ACTIONS:
                    try:
                        move(body)
                    except (OSError, ValueError, subprocess.SubprocessError) as exc:
                        log("move order %s stopped: %s" % (order.get("id"), exc))
                elif body["action"] == "remove":
                    try:
                        remove(int(body["seat"]), body)
                    except OSError as exc:
                        log("removing place %s stopped: %s" % (body["seat"], exc))
                elif body["action"] == "vault_keys":
                    if not vault_order_round(body, order["id"]):
                        continue   # not the night yet, or the server just hardened: sealed at a later round
                elif body["action"] == "reopen":
                    try:
                        reopen(int(body["seat"]), body["pair"])
                    except OSError as exc:
                        log("reopening place %s stopped: %s" % (body["seat"], exc))
                else:
                    reset(int(body["seat"]), body["setup"])
                os.makedirs(os.path.dirname(DONE), exist_ok=True)
                with open(DONE, "a") as fh:
                    fh.write("%d\n" % int(order["id"]))
            try:
                # a vault_keys order's outcome with its done (the guichet marks the grant sealed from it)
                call(conf, "POST", "/v1/server/orders/%d/done" % int(order["id"]), vault_outcome(order["id"]))
            except (OSError, ValueError) as exc:
                log("done not told yet (next round): %s" % exc)
    zram()   # last: installing it may take a minute, once
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
