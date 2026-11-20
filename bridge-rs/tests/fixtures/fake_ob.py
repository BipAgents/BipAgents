#!/usr/bin/env python3
"""A stand-in for Obsidian Headless' `ob` (obsidian-headless 0.0.14) in the bridge's tests: the same questions on the
terminal, the same words, the same exit codes, a « server » in a folder.

    fake_ob.py <server dir> <ob arguments…>

The server folder holds `calls.log` (every command line, to check no password is ever on one), `remote/<vault id>/`
(each remote vault's notes) and `accounts.json` (optional: {"twofa": true}).
"""
import getpass, json, os, shutil, sys, time

server, args = sys.argv[1], sys.argv[2:]
os.makedirs(server, exist_ok=True)
with open(os.path.join(server, "calls.log"), "a") as log:
    log.write(json.dumps(args) + "\n")
config = os.path.join(os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")), "obsidian-headless")
token_file = os.path.join(config, "auth_token")
settings = {}
try:
    settings = json.load(open(os.path.join(server, "accounts.json")))
except OSError:
    pass
VAULTS = {"v1": {"name": "Perso", "e2e": False}, "v2": {"name": "Secret", "e2e": True}}


def opt(name, default=None):
    return args[args.index(name) + 1] if name in args else default


def signed_in():
    if not os.path.exists(token_file):
        print('No account logged in. Run "ob login" first.', file=sys.stderr)
        sys.exit(2)


def merge(src, dst):
    for base, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if not d.startswith(".") or d == ".obsidian"]
        for name in files:
            a = os.path.join(base, name)
            b = os.path.join(dst, os.path.relpath(a, src))
            if not os.path.exists(b) or os.path.getmtime(a) > os.path.getmtime(b) + 0.001:
                os.makedirs(os.path.dirname(b), exist_ok=True)
                shutil.copy2(a, b)


command = args[0] if args else ""
if command == "logout":
    if os.path.exists(token_file):
        os.remove(token_file)
        print("Logged out.")
    else:
        print("No account logged in.")
elif command == "login":
    if not sys.stdin.isatty():
        print("Login failed: no terminal", file=sys.stderr)
        sys.exit(2)
    email = input("Email: ")
    password = getpass.getpass("Password: ")
    if email != "alex@example.org" or password != "s3cret":
        print("Login failed: Ye { error: 'Login failed, please check your email and password' }")
        sys.exit(2)
    if settings.get("twofa"):
        code = input("2FA code: ")
        if code != "123456":
            print("Login failed: Ye { error: '2FA code is incorrect' }")
            sys.exit(2)
    os.makedirs(config, exist_ok=True)
    with open(token_file, "w") as fh:
        fh.write("token-123")
    print("Logged in as Alex (alex@example.org)")
elif command == "sync-list-remote":
    signed_in()
    print(json.dumps({"vaults": [{"id": k, "name": v["name"], "region": "eu"} for k, v in VAULTS.items()], "shared": []}))
elif command == "sync-setup":
    signed_in()
    vault, path = opt("--vault"), opt("--path", ".")
    if vault not in VAULTS:
        print(f'Vault "{vault}" not found.', file=sys.stderr)
        sys.exit(3)
    print("Fetching vault info...")
    if VAULTS[vault]["e2e"]:
        password = getpass.getpass("End-to-end encryption password: ")
        if password != "vaultpw":
            print("Failed to validate password.")
            sys.exit(2)
    os.makedirs(os.path.join(config, "sync"), exist_ok=True)
    with open(os.path.join(config, "sync", "local.json"), "w") as fh:
        json.dump({"vault": vault, "path": os.path.abspath(path)}, fh)
    os.makedirs(path, exist_ok=True)
    print("\nVault configured successfully!\n\nRun 'ob sync' to start syncing.")
elif command == "sync-config":
    print("ok")
elif command == "sync":
    signed_in()
    try:
        local = json.load(open(os.path.join(config, "sync", "local.json")))
    except OSError:
        print("No sync configuration found", file=sys.stderr)
        sys.exit(3)
    remote = os.path.join(server, "remote", local["vault"])
    os.makedirs(remote, exist_ok=True)
    path = opt("--path", ".")
    merge(remote, path)
    merge(path, remote)
    time.sleep(0.05)
    print("Fully synced")
elif command == "sync-unlink":
    try:
        os.remove(os.path.join(config, "sync", "local.json"))
    except OSError:
        pass
    print("Unlinked.")
else:
    print(f"unknown command {command}", file=sys.stderr)
    sys.exit(1)
