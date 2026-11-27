"""What a place of the offer could have left behind to run outside its agents' sandbox, looked for READ ONLY on one
server of places (27/11, security triage section 0, lot A0). Until 26/11 a place had a shell (SSH): a `systemd --user`
unit, a crontab or a login file put there then outlives `DenyUsers`, and root's `sudo -iu <place>` (updates) runs its
login files. An import before 27/11 kept a profile's hooks, plugins, scripts and stdio MCP servers
(installer/bipinstall/profile_guard.py).

Run as root by the seat agent (seat-agent.py `place_check`: `--json`, numbers per category only, sent to the guichet in
its beat), and by scripts/places-check-persistence.sh for the lab and the tests (a line per place, names escaped).
Shows names, counts and fingerprints only, never what a file holds. Changes nothing, starts nothing as a place, follows
no link a place made (each file opened under its home without following links, never a FIFO, at most 1 MiB read, at
most MAX_ENTRIES names of a folder).

For its tests and the lab: PLACES_ROOT (a folder standing for /: its etc/passwd, etc/skel, home/, var/spool/cron,
var/lib/systemd/linger, sys/fs/cgroup and proc/); PLACES_NO_YAML=1 reads config.yaml by its words only, as on a server
whose python3 has no PyYAML.
"""
import hashlib, json, os, re, stat, sys, time

ROOT = os.environ.get("PLACES_ROOT", "/").rstrip("/") or ""
MAX = 1 << 20
NOFOLLOW = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | getattr(os, "O_CLOEXEC", 0)
LOGIN_FILES = (".bash_profile", ".bash_login", ".profile", ".bashrc", ".bash_logout")
# The user units BipAgents and Hermes write (installer/bipinstall, bridge-rs/src/browsers.rs, Hermes' gateway).
UNIT = re.compile(r"(bipagents-[A-Za-z0-9@._-]+|hermes-gateway[A-Za-z0-9@._-]*)\.(service|timer|path|socket)$")
WANTS = re.compile(r"[A-Za-z0-9@._-]+\.(wants|requires)$")
# What a unit of ours starts (the program's name, never its arguments).
EXEC_OK = {"python3", "python", "python3.12", "python3.13", "python3.14", "bipbridge", "hermes", "podman", "bash", "kill",
           "sh", "env", "uv", "systemctl", "true", "rm", "mkdir", "sleep", "restic", "bip-browser-mcp", "nice",
           "ionice", "flock", "timeout"}
# Processes of a place's account outside its containers (the host side): what a place runs.
PROC_OK = {"systemd", "(sd-pam)", "sd-pam", "bipbridge", "bip-browser-mcp", "hermes", "python3", "python",
           "gocryptfs", "fusermount3", "fusermount",   # an encrypted place's space (bridge-rs/src/space.rs)
           "python3.12", "python3.13", "python3.14", "podman", "conmon", "catatonit", "pasta", "pasta.avx2",
           "slirp4netns", "rootlessport", "rootlessport-ch", "dbus-daemon", "dbus-broker", "dbus-broker-lau",
           "restic", "systemctl", "journalctl", "uv", "git", "aardvark-dns", "netavark", "crun", "runc",
           "fuse-overlayfs"}

def p(path):
    return ROOT + path


def walk_open(home_fd, rel, flags=NOFOLLOW):
    """`rel` under the home's fd, each part opened without following a link; None if anything is not plain."""
    parts = [x for x in rel.split("/") if x]
    fd = os.dup(home_fd)
    try:
        for part in parts[:-1]:
            nxt = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0), dir_fd=fd)
            os.close(fd)
            fd = nxt
        last = os.open(parts[-1], flags, dir_fd=fd)
        return last
    except OSError:
        return None
    finally:
        os.close(fd)


def read(home_fd, rel):
    fd = walk_open(home_fd, rel)
    if fd is None:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        chunks, size = [], 0
        while size < MAX:
            b = os.read(fd, min(65536, MAX - size))
            if not b:
                break
            chunks.append(b)
            size += len(b)
        return b"".join(chunks)
    finally:
        os.close(fd)


def kind(home_fd, rel):
    """'file', 'dir', 'link', 'other' or None, without following a link."""
    parts = [x for x in rel.split("/") if x]
    parent = walk_open(home_fd, "/".join(parts[:-1]) + "/.", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW) \
        if len(parts) > 1 else os.dup(home_fd)
    if parent is None:
        return None
    try:
        st = os.stat(parts[-1], dir_fd=parent, follow_symlinks=False)
    except OSError:
        return None
    finally:
        os.close(parent)
    return "link" if stat.S_ISLNK(st.st_mode) else "dir" if stat.S_ISDIR(st.st_mode) else \
        "file" if stat.S_ISREG(st.st_mode) else "other"


MAX_ENTRIES = 2000   # a folder's names read at most (a place can fill one: never unbounded memory)


def listdir(home_fd, rel):
    fd = walk_open(home_fd, rel, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    if fd is None:
        return []
    names = []
    try:
        with os.scandir(fd) as entries:
            for entry in entries:
                names.append(entry.name)
                if len(names) >= MAX_ENTRIES:
                    break
        return sorted(names)
    except OSError:
        return names
    finally:
        os.close(fd)


def our_drop_in(home_fd, home, unit, name, rel):
    """A drop-in BipAgents writes itself, by its name, its unit and its exact text."""
    if kind(home_fd, rel) != "file":
        return False
    text = (read(home_fd, rel) or b"").decode("utf-8", "replace")
    if name == "60-bipagents-sandboxes.conf":
        return unit in GATEWAY_UNITS and text == SANDBOXES_DROPIN
    if name == "50-bipagents-space.conf":
        m = SPACE_DROPIN.fullmatch(text)
        return bool(m) and m.group(1).startswith(home.rstrip("/") + "/") and "/.." not in m.group(1)
    return False


def podman_findings(home_fd, home):
    """~/.config/containers: podman reads it when the place's agents' sandboxes start (a `mount_program`, a hook, a
    runtime of the place's would run on the host); BipAgents writes only its short names and its shared images'
    storage.conf, exactly. Quadlets (systemd/) are units of the place's: never ours."""
    out = []
    base = ".config/containers"
    for name in listdir(home_fd, base):
        rel = f"{base}/{name}"
        k = kind(home_fd, rel)
        if name == "registries.conf.d" and k == "dir":
            for extra in listdir(home_fd, rel):
                ok = extra == "50-bipagents.conf" and kind(home_fd, f"{rel}/{extra}") == "file" \
                    and (read(home_fd, f"{rel}/{extra}") or b"").decode("utf-8", "replace") == REGISTRIES
                if not ok:
                    out.append(f"podman {name}/{extra}")
        elif name == "storage.conf" and k == "file":
            if not STORAGE_CONF.fullmatch((read(home_fd, rel) or b"").decode("utf-8", "replace")):
                out.append("podman storage.conf (pas celui de BipAgents)")
        elif name == "systemd" and k == "dir":
            for extra in listdir(home_fd, rel):
                out.append(f"quadlet {extra}")
        else:
            out.append(f"podman {name}" + ("" if k == "file" else f" ({k})"))
    return out


def link_on_the_way(home_fd, rel):
    """The first part of `rel` (under the home) that is a link or not a folder, or None: what the check does not go
    through, said instead of skipped (systemd and Hermes would follow it)."""
    parts = [x for x in rel.split("/") if x]
    for i in range(1, len(parts) + 1):
        k = kind(home_fd, "/".join(parts[:i]))
        if k is None:
            return None
        if k != "dir":
            return ("/".join(parts[:i]), k)
    return None


def short(data):
    return hashlib.sha256(data).hexdigest()[:10]


def accounts(wanted):
    found = {}
    try:
        lines = open(p("/etc/passwd"), encoding="utf-8").read().splitlines()
    except OSError:
        return found
    for line in lines:
        f = line.split(":")
        if len(f) >= 7 and re.fullmatch(r"bip([2-9]|[1-3][0-9]|40)?", f[0]) and (not wanted or f[0] in wanted):
            found[f[0]] = (int(f[2]), f[5])
    return found


# The line uv's installer (install.sh) adds to the login files of every place.
UV_LINES = (b'. "$HOME/.local/bin/env"\n', b'\n. "$HOME/.local/bin/env"\n')
# What the installers append to a login file of a place: uv's line (install.sh), Hermes' (its scripts/install.sh
# append_shell_path: « # Hermes Agent command » and SHELL_PATH_LINE, Hermes v0.23.1). A login file is expected when,
# these blocks taken out, it is the skeleton's.
HERMES_PATH_LINE = (b'case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac')
KNOWN_APPENDS = (b"\n# Hermes Agent command\n" + HERMES_PATH_LINE + b"\n",) + UV_LINES
# uv's ~/.local/bin/env (its installer's), line by line: whatever its version, only these.
UV_ENV_LINES = re.compile(r'(\s*(#.*)?|case ":\$\{PATH\}:" in|\s*\*:"(\$HOME|/home/[a-z0-9]+)/\.local/bin":\*\)|\s*;;|\s*\*\)'
                          r'|\s*export PATH="(\$HOME|/home/[a-z0-9]+)/\.local/bin:\$PATH"|esac)')


# Hermes' older installers (before v0.23): `echo ""; echo "# Hermes Agent ..."; echo 'export PATH=...'`.
OLD_HERMES_APPEND = re.compile(rb'\n?# Hermes Agent[^\n]*\n(export PATH="(\$HOME/\.local/bin|/usr/local/bin):\$PATH"|'
                               + re.escape(HERMES_PATH_LINE) + rb')\n')


def expected_login(data, skeleton):
    """A login file as the installers leave it: the skeleton's, plus their blocks only."""
    rest = data
    for block in KNOWN_APPENDS:
        rest = rest.replace(block, b"")
    rest = OLD_HERMES_APPEND.sub(b"", rest)
    return skeleton is not None and rest.rstrip(b"\n") == skeleton.rstrip(b"\n")


def uv_env(data):
    return all(UV_ENV_LINES.fullmatch(line) for line in data.decode("utf-8", "replace").splitlines())


def skel():
    """Each login file of the skeleton, by name (its bytes)."""
    out = {}
    for name in LOGIN_FILES:
        try:
            with open(p("/etc/skel/" + name), "rb") as fh:
                out[name] = fh.read(MAX)
        except OSError:
            continue
    return out


def yaml_load(data):
    if os.environ.get("PLACES_NO_YAML"):   # its tests: the words-only reading, as on a server without PyYAML
        return None
    try:
        import yaml
    except ImportError:
        return None
    try:
        doc = yaml.safe_load(data)
    except Exception:
        return {"__unreadable__": True}
    return doc if isinstance(doc, dict) else {}


# What BipAgents writes there itself, exactly (kept in step by scripts/test_places_check_persistence.py):
# the gateway's drop-in (installer/bipinstall/migrations.py SANDBOXES_TEXT, every install and update), an encrypted
# place's (space.py drop_in), podman's short names (hermes.py podman_short_names) and its shared images (shared_images.py
# storage_conf, both versions in service).
SANDBOXES_DROPIN = '# BipAgents (installer/bipinstall/migrations.py): the agents\' sandboxes this gateway started die with it (their\n# fuse-overlayfs mounts live in its cgroup) and Hermes would keep the dead containers: every terminal and file tool then\n# fails (« transport endpoint is not connected », bench of 2026-10-31). Removed before it starts; Hermes makes them\n# again at their next use (the agents\' files are in their volumes, kept).\n[Service]\nExecStartPre=-/bin/sh -c \'podman ps -a --format "{{.Names}}" | grep "^hermes-" | xargs -r podman rm -f >/dev/null 2>&1; exit 0\'\n'
GATEWAY_UNITS = ('hermes-gateway.service', 'bipagents-hermes.service')
REGISTRIES = '# BipAgents: where Hermes\' sandbox images come from (podman won\'t guess in a service).\n[aliases]\n"nousresearch/hermes-sandbox" = "docker.io/nousresearch/hermes-sandbox"\n"nikolaik/python-nodejs" = "docker.io/nikolaik/python-nodejs"\n'
SPACE_DROPIN = re.compile(r"# BipAgents: this service waits for the encrypted space to be open \(bipagents encrypt\)\.\n"
                          r"\[Unit\]\nConditionPathExists=(/[^\n]*)/open/\.bipagents-space\n")
STORAGE_CONF = re.compile(r'# BipAgents: the server\'s shared images \(installer/bipinstall/shared_images\.py\), read-only for '
                          r'this account\.\n\[storage\]\ndriver = "overlay"\n\n\[storage\.options\]\n'
                          r'additionalimagestores = \["/[^"\n]*"\]\n'
                          r'(\n# The browser\'s keep-id mapping shifted as it is read, not the whole image copied at each start\.\n'
                          r'\[storage\.options\.overlay\]\nmount_program = "/usr/bin/fuse-overlayfs"\n)?')
BRIDGE_ARGS = re.compile(r"--network=bipagents-sandbox|--dns=127\.0\.0\.1|--cpus=[0-9.]+")
PROXY_ENV = {"HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "NO_PROXY", "no_proxy", "npm_config_proxy",
             "npm_config_https_proxy"}
ENV_ODD = ("HERMES_ACCEPT_HOOKS", "LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "PYTHONPATH", "PYTHONSTARTUP",
           "PYTHONHOME", "NODE_OPTIONS", "NODE_PATH", "BASH_ENV", "ENV", "PATH", "SHELLOPTS", "BASHOPTS", "PS4",
           "LUA_INIT", "JAVA_TOOL_OPTIONS", "LESSOPEN", "STORAGE_OPTS", "STORAGE_DRIVER", "CONTAINER_HOST",
           "CONTAINER_CONNECTION", "OPENSSL_MODULES", "OPENSSL_CONF", "GCONV_PATH")


def bridge_browser(entry, home):
    """The bridge's browser server exactly (bridge-rs/src/browsers.rs mcp_server): `podman|docker exec -i
    bipagents-browser-<agent> /opt/stealth/mcp.sh`, or its bip-browser-mcp (in the place's bridge folder) on one
    browsers/<agent>/mcp.json of the place. Paths compared as written, never resolved."""
    if not isinstance(entry, dict) or not isinstance(entry.get("command"), str):
        return False
    command, args = entry["command"], entry.get("args") or []
    if not isinstance(args, list) or not all(isinstance(a, str) for a in args) or set(entry) - BRIDGE_KEYS:
        return False   # an `env` or `cwd` of its own (LD_PRELOAD for the podman Hermes starts), or anything else
    if command in ("/usr/bin/podman", "/usr/bin/docker", "/usr/local/bin/podman", "/usr/local/bin/docker", "/bin/podman"):
        return len(args) == 4 and args[:2] == ["exec", "-i"] and args[3] == "/opt/stealth/mcp.sh" \
            and re.fullmatch(r"bipagents-browser-[a-z0-9][a-z0-9_-]*", args[2]) is not None
    bins = home.rstrip("/") + "/.local/share/bipagents/bridge/"
    data = home.rstrip("/") + "/.local/share/bipagents/browsers/"
    return (command.startswith(bins) and "/.." not in command and command.endswith("/bip-browser-mcp")
            and len(args) == 1 and args[0].startswith(data) and "/.." not in args[0]
            and re.fullmatch(r"[a-z0-9][a-z0-9_-]*/mcp\.json", args[0][len(data):]) is not None)


# What the bridge writes in its browser server's entry (browsers.rs mcp_server, tool_diet.rs: `enabled`).
BRIDGE_KEYS = {"command", "args", "connect_timeout", "timeout", "tools", "call_context", "enabled"}


def host_mount(volume):
    """A sandbox volume whose source is a folder of the host (Hermes' rule, sandbox.rs: `/`, `~`, `./`, `../`, a
    drive), not a named volume."""
    source = str(volume).split(":", 1)[0] if not re.match(r"^[A-Za-z]:[\\/]", str(volume)) else str(volume)
    return source.startswith(("/", "~", ".")) or re.match(r"^[A-Za-z]:", source) is not None


def terminal_findings(terminal):
    """config.yaml's `terminal:` (it wins over .env, tools/terminal_scope.py): what BipAgents never writes there."""
    found = []
    if str(terminal.get("backend", "docker")) != "docker":
        found.append("terminal hors bac à sable")
    volumes = terminal.get("docker_volumes") or []
    if not isinstance(volumes, list) or any(host_mount(v) for v in volumes):
        found.append("terminal monte un dossier de l'hôte (docker_volumes)")
    args = terminal.get("docker_extra_args") or []
    if not isinstance(args, list) or any(not BRIDGE_ARGS.fullmatch(str(a)) for a in args):
        found.append("terminal élargi (docker_extra_args)")
    env = terminal.get("docker_env") or {}
    if not isinstance(env, dict) or set(env) - PROXY_ENV:
        found.append("terminal élargi (docker_env)")
    for key in ("docker_mount_cwd_to_workspace", "docker_run_as_host_user", "docker_forward_env"):
        if terminal.get(key) and str(terminal.get(key)).lower() not in ("false", "0", "no", "[]"):
            found.append(f"terminal {key}")
    return found


def flow_items(inner):
    """The items of a YAML [flow] list or {flow} map's inside, split on commas outside quotes."""
    items, cur, quote = [], "", None
    for c in inner:
        if quote:
            cur += c
            if c == quote:
                quote = None
        elif c in "\"'":
            quote, cur = c, cur + c
        elif c == ",":
            items.append(cur)
            cur = ""
        else:
            cur += c
    items.append(cur)
    return [i for i in items if i.strip()]


def words_terminal(text):
    """Without PyYAML: config.yaml's top-level `terminal:` block as a small dict (scalars, [flow] lists, `- item`
    lists, {flow} maps by their keys only)."""
    lines = text.splitlines()
    unq = lambda s: s.strip().strip("\"'")
    out, inside, key = {}, False, None
    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not line.startswith(" "):
            inside = line.rstrip() == "terminal:" or line.startswith("terminal:") and line[9:].strip() == ""
            key = None
            continue
        if not inside:
            continue
        m = re.match(r"^  ([A-Za-z0-9_]+):\s*(.*?)\s*$", line)
        if m:
            key, value = m.group(1), m.group(2)
            if value.startswith("[") and value.endswith("]"):
                out[key] = [unq(x) for x in flow_items(value[1:-1])]
            elif value.startswith("{") and value.endswith("}"):
                out[key] = {unq(x.split(":", 1)[0]): True for x in flow_items(value[1:-1])}
            elif value == "":
                out[key] = []
            else:
                out[key] = unq(value)
            continue
        item = re.match(r"^\s+-\s+(.*?)\s*$", line)
        if item and key and isinstance(out.get(key), list):
            out[key].append(unq(item.group(1)))
            continue
        sub = re.match(r"^    ([A-Za-z0-9_]+):", line)
        if sub and key and out.get(key) == []:
            out[key] = {}
        if sub and key and isinstance(out.get(key), dict):
            out[key][sub.group(1)] = True
    return out


def host_programs(cfg):
    """Settings naming a program Hermes runs on the host (BipAgents writes none on a place): their names."""
    found = []
    sec = lambda k: cfg.get(k) if isinstance(cfg.get(k), dict) else {}
    if sec("secrets").get("command"):
        found.append("secrets.command")
    for name in ("onepassword", "bitwarden"):
        for top in ("secrets", "vault"):
            if isinstance(sec(top).get(name), dict) and sec(top)[name].get("binary_path"):
                found.append(f"{top}.{name}.binary_path")
    for voice in ("tts", "stt"):
        s = sec(voice)
        providers = s.get("providers") if isinstance(s.get("providers"), dict) else {}
        if s.get("command") or s.get("provider") == "command" or any(
                isinstance(v, dict) and (v.get("type") == "command" or "command" in v) for v in providers.values()):
            found.append(f"{voice} command")
    if sec("lsp").get("servers"):
        found.append("lsp.servers")
    for key in ("executable_path", "chromium_args"):
        if sec("browser").get(key):
            found.append(f"browser.{key}")
    if sec("delegation").get("backends"):
        found.append("delegation.backends")
    if isinstance(sec("plugins").get("host"), dict) and sec("plugins")["host"].get("launcher"):
        found.append("plugins.host.launcher")
    return found


def env_value(raw):
    """A .env value as Hermes reads it (agent/secret_scope.py `_parse_env_value`): bare, 'single', or "double" with
    \\" and \\\\ escapes (what its `_quote_env_value` writes for JSON values)."""
    value = raw.strip()
    if len(value) >= 2 and value[0] == value[-1] == '"':
        quoted, parsed, i = value[1:-1], [], 0
        while i < len(quoted):
            escaped = quoted[i] == "\\" and quoted[i + 1:i + 2] in ('"', "\\")
            parsed.append(quoted[i + 1] if escaped else quoted[i])
            i += 2 if escaped else 1
        return "".join(parsed)
    if len(value) >= 2 and value[0] == value[-1] == "'":
        return value[1:-1]
    return value


def sandbox_value_ok(name, value):
    """A TERMINAL_* line as BipAgents writes it (installer: backend docker; the bridge's `hermes config set
    terminal.…`, sandbox.rs, which Hermes copies into .env), by value."""
    try:
        parsed = json.loads(value) if value[:1] in "[{" else value
    except ValueError:
        return False
    if name == "TERMINAL_ENV":
        return value == "docker"
    if name == "TERMINAL_DOCKER_NETWORK":
        return value.lower() in ("true", "false")
    if name == "TERMINAL_DOCKER_IMAGE":
        return value in ("localhost/bipagents-sandbox:latest", "nousresearch/hermes-sandbox",
                         "docker.io/nousresearch/hermes-sandbox") or value.startswith("docker.io/nousresearch/hermes-sandbox:")
    if name == "TERMINAL_DOCKER_EXTRA_ARGS":
        return isinstance(parsed, list) and all(isinstance(a, str) and BRIDGE_ARGS.fullmatch(a) for a in parsed)
    if name == "TERMINAL_DOCKER_ENV":
        return isinstance(parsed, dict) and set(parsed) <= PROXY_ENV
    if name == "TERMINAL_DOCKER_VOLUMES":
        return isinstance(parsed, list) and all(isinstance(v, str) and v.startswith("bipagents-") for v in parsed)
    if name in ("TERMINAL_DOCKER_MOUNT_CWD_TO_WORKSPACE", "TERMINAL_DOCKER_RUN_AS_HOST_USER"):
        return value.lower() in ("", "false", "0", "no")
    return name in ("TERMINAL_TIMEOUT", "TERMINAL_LIFETIME_SECONDS", "TERMINAL_DOCKER_IMAGE_PINNED")


def words_commands(text):
    """Without PyYAML: each `command:` of config.yaml with the `args:` of the same entry (same indent, a [flow] list
    or `- item` lines), as {command, args}, quotes taken off. Enough to tell the bridge's browser server from anything
    else; args it can't read make the entry odd."""
    lines = text.splitlines()
    depth = lambda l: len(l) - len(l.lstrip(" "))
    unq = lambda s: s.strip().strip("\"'")
    found = []
    for i, line in enumerate(lines):
        m = re.match(r"^( +)command:\s*(.*?)\s*$", line)
        if not m:
            continue
        indent = len(m.group(1))
        start = i
        while start > 0 and (not lines[start - 1].strip() or depth(lines[start - 1]) >= indent):
            start -= 1
        end = i + 1
        while end < len(lines) and (not lines[end].strip() or depth(lines[end]) >= indent):
            end += 1
        block, args, ok, seen = lines[start:end], [], True, False
        for k, l in enumerate(block):
            a = re.match(r"^ {%d}args:\s*(.*?)\s*$" % indent, l)
            if not a:
                continue
            seen = True
            value = a.group(1)
            if value.startswith("[") and value.endswith("]") and "[" not in value[1:]:
                args = [unq(x) for x in value[1:-1].split(",") if x.strip()]
            elif value == "":
                for item in block[k + 1:]:
                    b = re.match(r"^ {%d,}-\s+(.*?)\s*$" % indent, item)
                    if not b:
                        break
                    args.append(unq(b.group(1)))
            else:
                ok = False
        keys = {k.group(1) for l in block if (k := re.match(r"^ {%d}([A-Za-z0-9_]+):" % indent, l))}
        entry = {key: True for key in keys}   # what else the entry holds (only its names matter here)
        entry.update({"command": unq(m.group(2)), "args": args})
        found.append(entry if ok and seen else {"command": None})
    return found


def hermes_findings(home_fd, rel, home, out):
    """A Hermes home (the hub's root or a profile): what runs outside the sandbox (profile_guard.py)."""
    where = rel.replace(".hermes/profiles/", "profil ").replace(".hermes", "hub")
    for name in ("hooks", "plugins", "scripts", "shell-hooks-allowlist.json"):
        k = kind(home_fd, f"{rel}/{name}")
        if k == "dir" and listdir(home_fd, f"{rel}/{name}"):
            out.append(f"{where}: {name}/ ({len(listdir(home_fd, f'{rel}/{name}'))})")
        elif k in ("file", "link", "other"):
            out.append(f"{where}: {name} ({k})")
    for name in ("config.yaml", ".env", "state.db"):
        k = kind(home_fd, f"{rel}/{name}")
        if k in ("link", "other"):
            out.append(f"{where}: {name} ({k})")
    data = read(home_fd, f"{rel}/config.yaml")
    if data is not None:
        cfg = yaml_load(data)
        if cfg is None:   # no PyYAML here: the words only
            text = data.decode("utf-8", "replace")
            for word in ("hooks_auto_accept: true", "inline_shell: true", "type: exec", "type: command",
                         "executable_path:", "chromium_args:", "binary_path:", "launcher:"):
                if re.search(r"^\s*" + re.escape(word) + (r"\s*\S" if word.endswith(":") else ""), text, re.M):
                    out.append(f"{where}: config « {word.split(':')[0]} »")
            if re.search(r"^hooks:\s*\S", text, re.M) or re.search(r"^hooks:\s*\n\s+\w", text, re.M):
                out.append(f"{where}: config hooks")
            if re.search(r"^(secrets|lsp|tts|stt|delegation):\s*\n(\s+.*\n)*?\s+(command|servers|backends):", text, re.M):
                out.append(f"{where}: config programme de l'hôte")
            for found in terminal_findings(words_terminal(text)):
                out.append(f"{where}: {found}")
            odd = [e for e in words_commands(text) if not bridge_browser(e, home)]
            if odd:
                out.append(f"{where}: MCP par commande ({len(odd)})")
        elif cfg.get("__unreadable__"):
            out.append(f"{where}: config.yaml illisible")
        else:
            servers = cfg.get("mcp_servers") if isinstance(cfg.get("mcp_servers"), dict) else {}
            odd = [n for n, e in servers.items() if not isinstance(e, dict) or (("command" in e or "url" not in e)
                   and not (n == "stealth" and bridge_browser(e, home)))]
            if odd:
                out.append(f"{where}: MCP par commande ({len(odd)})")
            if cfg.get("hooks"):
                out.append(f"{where}: config hooks ({len(cfg['hooks'])})")
            if cfg.get("hooks_auto_accept"):
                out.append(f"{where}: hooks_auto_accept")
            quick = cfg.get("quick_commands") if isinstance(cfg.get("quick_commands"), dict) else {}
            execs = [n for n, q in quick.items() if not isinstance(q, dict) or q.get("type") != "alias"]
            if execs:
                out.append(f"{where}: quick_commands exec ({len(execs)})")
            plugins = cfg.get("plugins") if isinstance(cfg.get("plugins"), dict) else {}
            if plugins.get("enabled") and listdir(home_fd, f"{rel}/plugins"):
                out.append(f"{where}: plugins.enabled ({len(plugins['enabled'])})")
            if isinstance(cfg.get("skills"), dict) and cfg["skills"].get("inline_shell"):
                out.append(f"{where}: skills.inline_shell")
            for found in host_programs(cfg):
                out.append(f"{where}: {found}")
            terminal = cfg.get("terminal") if isinstance(cfg.get("terminal"), dict) else {}
            for found in terminal_findings(terminal):
                out.append(f"{where}: {found}")
    env = read(home_fd, f"{rel}/.env")
    if env is not None:
        odd = []
        for line in env.decode("utf-8", "replace").splitlines():
            if "=" not in line or line.lstrip().startswith("#"):
                continue
            name, value = line.split("=", 1)
            name, value = name.strip().removeprefix("export ").strip(), env_value(value)
            if name.startswith("TERMINAL_"):
                if not sandbox_value_ok(name, value):
                    odd.append(name)
            elif name in ENV_ODD:
                odd.append(name)
        if odd:
            out.append(f"{where}: .env {','.join(sorted(set(odd)))}")


# Programs a transient scope of a place may hold (the bridge's registries' proxy: `systemd-run --user --scope` without a
# name, sandbox.rs in_own_scope: podman, then its conmon), from the system's folders only.
SYSTEMD_OWN = {"systemd-tmpfiles-clean.service", "systemd-tmpfiles-setup.service", "systemd-exit.service",
               "systemd-tmpfiles-setup-dev.service"}   # the user manager's own, passing
SCOPE_OK = {"podman", "conmon", "catatonit", "pasta", "pasta.avx2", "slirp4netns", "rootlessport", "rootlessport-ch",
            "aardvark-dns", "netavark"}
SYSTEM_BIN = ("/usr/bin/", "/usr/libexec/", "/usr/lib/", "/bin/", "/usr/sbin/", "/sbin/")
RUN_SCOPE = re.compile(r"run-[A-Za-z0-9]+\.scope")
# What a passing process or unit needs to last to be told: two looks this far apart (seconds; PLACES_SETTLE for tests).
SETTLE = float(os.environ.get("PLACES_SETTLE", "5"))
_LOOKS = {}


def _proc_look():
    """{pid: (uid, start, comm, unit, exe, cmdline)} of every process outside a container. `cmdline` None when /proc
    has none (the tests' fakes); its arguments are only matched here, never shown."""
    seen = {}
    proc = p("/proc")
    for pid in os.listdir(proc) if os.path.isdir(proc) else []:
        if not pid.isdigit():
            continue
        base = f"{proc}/{pid}"
        try:
            uid = os.stat(base).st_uid
            comm = open(f"{base}/comm", encoding="utf-8", errors="replace").read().strip()
            cgroup = open(f"{base}/cgroup", encoding="utf-8", errors="replace").read().strip().splitlines()[-1]
        except (OSError, IndexError):
            continue
        if "/libpod-" in cgroup and "libpod-conmon-" not in cgroup:
            continue   # in one of its containers (the sandbox, a browser, a connector)
        try:
            fields = open(f"{base}/stat", encoding="utf-8", errors="replace").read().rsplit(")", 1)[1].split()
            start, ppid = fields[19], fields[1]
        except (OSError, IndexError):
            start, ppid = "", ""
        try:
            exe = os.readlink(f"{base}/exe")
        except OSError:
            exe = ""
        try:
            with open(f"{base}/cmdline", "rb") as fh:
                cmdline = fh.read(65536).decode("utf-8", "replace").split("\0")
        except FileNotFoundError:
            cmdline = None
        except OSError:
            cmdline = []
        path = cgroup.split("::", 1)[-1] if "::" in cgroup else cgroup.split(":", 2)[-1]
        seen[pid] = (uid, start, comm, cgroup.rsplit("/", 1)[-1], exe, cmdline, path, ppid)
    return seen


def looks():
    """(first, second, lasting) of every process, two looks SETTLE apart; once for all the places of the server."""
    if "procs" not in _LOOKS:
        first = _proc_look()
        if SETTLE > 0:
            time.sleep(SETTLE)
        second = _proc_look()
        _LOOKS["first"], _LOOKS["second"] = first, second
        _LOOKS["procs"] = {pid: info for pid, info in second.items() if first.get(pid) == info}
    return _LOOKS["first"], _LOOKS["second"], _LOOKS["procs"]


def lasting():
    return looks()[2]


def program_is(comm, exe):
    """The process's binary is what its name says (a name is the process's to choose: prctl), a system one."""
    if not exe.startswith(SYSTEM_BIN) or exe.endswith(" (deleted)"):
        return False
    base = os.path.basename(exe)
    if comm.startswith("python"):
        return re.fullmatch(r"python3(\.\d+)?", base) is not None and re.fullmatch(r"python3?(\.\d+)?", comm) is not None
    return base[:15] == comm


# The units a place's account runs that BipAgents and Hermes make without a file in ~/.config/systemd/user: name ->
# the programs they hold (installer and bridge: bridge-rs/src/space.rs, place_import.rs, cloud_setup.rs; Hermes:
# tools/process_registry.py hermes-worker-*; podman's own scopes).
TRANSIENT_OURS = (
    (re.compile(r"bipagents-space-open\.service"), {"gocryptfs", "fusermount3", "fusermount"}),
    (re.compile(r"bipagents-(import|restore)\.service"), {"python3", "python", "systemctl", "podman", "conmon"}),
    (re.compile(r"hermes-worker-[A-Za-z0-9_-]+\.scope"), {"python3", "python", "podman", "conmon"}),
    (re.compile(r"libpod-conmon-[0-9a-f]+\.scope|podman-[0-9]+\.scope|podman-pause-[0-9a-f]+\.scope"),
     {"conmon", "podman", "catatonit", "pasta", "pasta.avx2", "slirp4netns", "rootlessport", "rootlessport-ch"}),
)
OWN_SYSTEM_UNITS = {"init.scope", "dbus.service", "dbus-broker.service"}


def transient_programs(unit):
    for pattern, programs in TRANSIENT_OURS:
        if pattern.fullmatch(unit):
            return programs
    return None


def scope_of_ours(unit, uid):
    """A nameless run-*.scope is the bridge's registries' proxy (sandbox.rs in_own_scope) only when it holds, lasting,
    podman's programs from the system's folders (each binary what its name says) and one of them names the proxy's
    container (bipagents-egress) in its command line. An empty one, or one whose processes don't last, is not."""
    found = [i for i in lasting().values() if i[0] == uid and i[3] == unit]
    if not found:
        return False
    if not all(i[2] in SCOPE_OK and program_is(i[2], i[4]) for i in found):
        return False
    commands = [i[5] for i in found]
    if all(cmd is None for cmd in commands):
        return True   # a /proc without command lines (the tests' fakes): the programs alone
    return any("bipagents-egress" in arg or "BipAgents registries' proxy" in arg
               for cmd in commands if cmd for arg in cmd)


def unit_of_ours(unit, uid, own_units):
    """A unit of the place's manager BipAgents or Hermes made: a file of ours in ~/.config/systemd/user (checked
    above), the user manager's own, or one of TRANSIENT_OURS by its exact name."""
    return unit in own_units or unit in OWN_SYSTEM_UNITS or unit in SYSTEMD_OWN or transient_programs(unit) is not None \
        or (RUN_SCOPE.fullmatch(unit) is not None and scope_of_ours(unit, uid))


# Root's units that run work as a place (seat-agent.py: its round, the check, the clean-up, the planned moves), by
# their exact cgroup: a place can make no process there.
ROOT_UNITS = re.compile(r"/system\.slice/(bipagents-seats|bipagents-place-check|bipagents-clean-imports|"
                        r"bipagents-move-[A-Za-z0-9_-]+)\.service")


def run_by_root(path, ppid, uid, everything):
    """A process of the place's uid that root's seat agent started (an update: `sudo -iu <place> ... install.sh
    update`): in one of ROOT_UNITS itself, or in a login session of the place (logind's session-*.scope, under its
    user-<uid>.slice, never under its own manager) whose chain of parents reaches a process in ROOT_UNITS."""
    if ROOT_UNITS.fullmatch(path):
        return True
    if not re.fullmatch(r"/user\.slice/user-%d\.slice/session-[A-Za-z0-9]+\.scope" % uid, path):
        return False
    seen = set()
    while ppid and ppid.isdigit() and ppid not in seen and len(seen) < 32:
        seen.add(ppid)
        parent = everything.get(ppid)
        if parent is None:
            return False
        if ROOT_UNITS.fullmatch(parent[6]):
            return True
        ppid = parent[7]
    return False


SHELLS = ("/usr/bin/dash", "/bin/dash", "/usr/bin/bash", "/bin/bash", "/usr/bin/sh", "/bin/sh")


def hermes_supervisor(comm, exe, cmdline, unit, own_units, home):
    """Hermes' resident `/bin/sh <its hermes_cli>/process_supervisor.sh <pgid>` (hermes_cli/process_identity.py
    _spawn_supervisor) in the place's gateway unit, exactly."""
    if comm != "sh" or exe not in SHELLS or unit not in own_units \
            or not re.fullmatch(r"hermes-gateway[A-Za-z0-9@._-]*\.service|bipagents-hermes\.service", unit):
        return False
    if cmdline is None:
        return True   # a /proc without command lines (the tests' fakes)
    args = [a for a in cmdline if a != ""]
    if len(args) != 3 or args[0] not in ("/bin/sh", "/system/bin/sh") or not args[2].isdigit():
        return False
    script = args[1]
    return script.endswith("/hermes_cli/process_supervisor.sh") and "/.." not in script and \
        (script.startswith(home.rstrip("/") + "/.hermes/") or script.startswith("/opt/bipagents/hermes/"))


def processes(uid, own_units=frozenset(), home=""):
    """The account's processes outside its containers that are not expected: in a unit not ours, a program not ours,
    a binary that is not what its name says or not from the system or the place's Hermes. A process seen once only is
    left out only in a unit of ours (a helper of its ExecStartPre); anywhere else a passing one counts."""
    first, second, last = looks()
    odd = []
    everything = {**first, **second}
    for pid, (uid_, start, comm, unit, exe, cmdline, path, ppid) in everything.items():
        if uid_ != uid:
            continue
        if run_by_root(path, ppid, uid, everything):
            continue   # the seat agent's work as the place (an update by sudo -iu), not the place's
        if hermes_supervisor(comm, exe, cmdline, unit, own_units, home):
            continue
        ours = unit_of_ours(unit, uid, own_units)
        if pid not in last and ours:
            continue
        exe_dir = os.path.dirname(exe.replace(" (deleted)", ""))
        bad_dir = exe.endswith(" (deleted)") or re.match(r"/(tmp|var/tmp|dev/shm|run/user)/", exe or "")
        programs = transient_programs(unit)
        expected = (programs is not None and comm in programs) or (programs is None and comm in PROC_OK)
        if RUN_SCOPE.fullmatch(unit):
            expected = ours
        if not ours or not expected or bad_dir:
            odd.append(f"{comm} [{unit}] {exe_dir or '?'}{' (supprimé)' if exe.endswith(' (deleted)') else ''}")
    return sorted(set(odd))


# -- the code the place owns (its Hermes when the server doesn't share one, its BipAgents, ~/.local/bin) -----------------
# Read only, never run (no git as root in a place's checkout: its config could run code). What a .pth or a
# sitecustomize in a venv the place's processes use would run, the names in ~/.local/bin, the launchers' text. Only
# these: a file of the checkout changed by the place is not seen (see the status: « rien » proves nothing of that).
VENVS = (".hermes/hermes-agent/venv", ".hermes/hermes-agent/.venv", ".hermes/venv", "BipAgents/installer/.venv",
         "BipAgents/bridge/.venv")
LOCAL_BIN = {"hermes", "hermes-acp", "hermes-agent", "uv", "uvx", "env", "env.fish", "bipagents"}
PTH_OK = re.compile(r"import __editable___[A-Za-z0-9_]+_finder; __editable___[A-Za-z0-9_]+_finder\.install\(\)\s*"
                    r"|import _virtualenv\s*"
                    r"|import os; var = 'SETUPTOOLS_USE_DISTUTILS'; enabled = os\.environ\.get\(var, 'local'\) == 'local'; "
                    r"enabled and __import__\('_distutils_hack'\)\.add_shim\(\); \s*")


def _site_packages(home_fd, venv):
    found = []
    for lib in ("lib", "lib64"):
        for py in listdir(home_fd, f"{venv}/{lib}"):
            if re.fullmatch(r"python3(\.\d+)?", py) and kind(home_fd, f"{venv}/{lib}/{py}/site-packages") == "dir":
                found.append(f"{venv}/{lib}/{py}/site-packages")
    return found


def pth_ok(data, home):
    """A .pth as installers write them: an editable install's finder, virtualenv's, setuptools' distutils shim, or
    folders (of the place's home or the system) only."""
    for line in data.decode("utf-8", "replace").splitlines():
        if not line.strip() or line.startswith("#") or PTH_OK.fullmatch(line):
            continue
        if line.startswith(("import ", "import\t")):
            return False
        if not (line.startswith(home.rstrip("/") + "/") or line.startswith(("/usr/", "/opt/"))) or "\n" in line:
            return False
    return True


def hermes_launcher(name, text, home):
    """~/.local/bin/<name> as Hermes 0.23.1 writes it (hermes_cli/_launchers.py `_write_shell`:
    `exec {shlex.join(command)} "$@"`, hermes-agent with `--run-module run_agent`) or as shared_hermes.py does
    (`exec "<home>/.hermes/shared/hermes-agent/.hermes/bin/<name>" "$@"`, hermes and hermes-acp), on the place's own
    checkout, the shared one or root's shared versions (/opt/bipagents/hermes/<version>). Exactly these."""
    import shlex
    found = re.fullmatch(r'#!/bin/sh\nexec (.+) "\$@"\n', text, re.S)
    if not found or "\n" in found.group(1):
        return False
    try:
        argv = shlex.split(found.group(1))
    except ValueError:
        return False
    if not argv:
        return False
    tail = ["--run-module", "run_agent"] if name == "hermes-agent" else []
    program = "hermes" if name == "hermes-agent" else name
    if argv[1:] != tail or not argv[0].endswith(f"/hermes-agent/.hermes/bin/{program}"):
        return False
    root = argv[0][:-len(f"/hermes-agent/.hermes/bin/{program}")]
    ours = root in (home.rstrip("/") + "/.hermes", home.rstrip("/") + "/.hermes/shared") or \
        re.fullmatch(r"/opt/bipagents/hermes/[A-Za-z0-9._-]+", root) is not None
    rendered = (f'#!/bin/sh\nexec {shlex.join(argv)} "$@"\n',
                f'#!/bin/sh\nexec "{argv[0]}" "$@"\n' if name in ("hermes", "hermes-acp") and not tail else None)
    return ours and text in rendered


def owned_code(home_fd, home):
    out = []
    venvs = list(VENVS)
    for tool in listdir(home_fd, ".hermes/tools"):   # a Python Hermes keeps of its own (its gateway may run on it)
        if tool.startswith("python"):
            venvs.append(f".hermes/tools/{tool}")
    for install in listdir(home_fd, ".hermes/installs"):   # Hermes' package manager's environments
        for env in listdir(home_fd, f".hermes/installs/{install}/environments"):
            venvs.append(f".hermes/installs/{install}/environments/{env}/venv")
        for gen in listdir(home_fd, f".hermes/installs/{install}/pm-runtime/generations"):
            venvs.append(f".hermes/installs/{install}/pm-runtime/generations/{gen}")
    for venv in venvs:
        if kind(home_fd, venv) != "dir":
            continue
        for site in _site_packages(home_fd, venv):
            for name in listdir(home_fd, site):
                if name in ("sitecustomize.py", "usercustomize.py"):
                    out.append(f"code {venv.split('/')[0]}: {name}")
                elif name.endswith(".pth"):
                    if kind(home_fd, f"{site}/{name}") != "file" or not pth_ok(read(home_fd, f"{site}/{name}") or b"", home):
                        out.append(f"code {venv.split('/')[0]}: .pth inattendu")
    launcher = lambda name, text: hermes_launcher(name, text, home)   # noqa: E731
    for name in listdir(home_fd, ".local/bin"):
        rel = f".local/bin/{name}"
        k = kind(home_fd, rel)
        if name not in LOCAL_BIN:
            out.append(f"code .local/bin: {name}")
        elif name.startswith("hermes"):
            if k == "link":
                target = ""
                bindir = walk_open(home_fd, ".local/bin/.", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                if bindir is not None:
                    try:
                        target = os.readlink(name, dir_fd=bindir)
                    except OSError:
                        pass
                    finally:
                        os.close(bindir)
                if not target.startswith(home.rstrip("/") + "/.hermes/"):
                    out.append(f"code .local/bin: {name} (lien)")
            elif k != "file" or not launcher(name, (read(home_fd, rel) or b"").decode("utf-8", "replace")):
                out.append(f"code .local/bin: {name} (pas le lanceur)")
        elif name == "bipagents":
            text = (read(home_fd, rel) or b"").decode("utf-8", "replace")
            if k != "file" or not text.startswith("#!/bin/sh\n# BipAgents' commands (") or not re.search(
                    r'\nexec bash "' + re.escape(home.rstrip("/")) + r'/BipAgents/install\.sh" "\$@"\n$', text):
                out.append("code .local/bin: bipagents (pas celui de BipAgents)")
    return out


def transient_units(uid):
    """The names in /run/user/<uid>/systemd/transient (the place's own folder: opened without following a link)."""
    try:
        fd = os.open(p("/run/user"), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError:
        return []
    try:
        return listdir(fd, f"{uid}/systemd/transient")
    finally:
        os.close(fd)


def place(user, uid, home, skel_prints, prints_seen):
    out, info, own_units = [], [], set()
    try:
        home_fd = os.open(p(home), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError:
        return ["dossier personnel absent ou lien"], info
    try:
        # login files, against the skeleton (fingerprints only)
        for name in LOGIN_FILES + (".local/bin/env",):
            k = kind(home_fd, name)
            if k is None:
                continue
            if k != "file":
                out.append(f"{name} ({k})")
                continue
            data = read(home_fd, name) or b""
            if name == ".local/bin/env":
                if not uv_env(data):
                    out.append(f"{name} (pas celui d'uv)")
                continue
            if expected_login(data, skel_prints.get(name)):
                continue
            got = short(data)
            prints_seen.setdefault((name, got), []).append(user)
            info.append((name, got))
        for name in (".pam_environment", ".ssh/rc", ".ssh/environment", ".config/environment.d"):
            k = kind(home_fd, name)
            if k and not (k == "dir" and not listdir(home_fd, name)):
                out.append(f"{name} ({k})")
        # SSH keys (removed at each start since 26/11)
        for name in (".ssh/authorized_keys", ".ssh/authorized_keys2"):
            data = read(home_fd, name)
            lines = [l for l in (data or b"").decode("utf-8", "replace").splitlines() if l.strip() and not l.startswith("#")]
            if lines or kind(home_fd, name) not in (None, "file"):
                out.append(f"{name}: {len(lines)} ligne(s)" + ("" if kind(home_fd, name) == "file" else f" ({kind(home_fd, name)})"))
        # folders it looks into: one that is a link (or a file) is said, never gone through
        looked = [".config/systemd/user", ".local/share/systemd/user", ".config/autostart", ".config/bipagents",
                  ".config/containers", ".config/containers/systemd", ".config/containers/registries.conf.d",
                  ".ssh", ".hermes", ".hermes/profiles"]
        looked += [f".hermes/profiles/{n}" for n in listdir(home_fd, ".hermes/profiles")]
        said = set()
        for rel in looked:
            got = link_on_the_way(home_fd, rel)
            if got and got[0] not in said:
                said.add(got[0])
                out.append(f"{got[0]} ({got[1]} : non suivi)")
        # systemd --user: unit files, drop-ins, links
        for folder in (".config/systemd/user", ".local/share/systemd/user"):
            for name in listdir(home_fd, folder):
                rel = f"{folder}/{name}"
                k = kind(home_fd, rel)
                if k == "dir" and WANTS.fullmatch(name):
                    for link in listdir(home_fd, rel):
                        if not UNIT.fullmatch(link):
                            out.append(f"systemd {name}/{link}")
                    continue
                if k == "dir" and name.endswith(".d"):
                    for extra in listdir(home_fd, rel):
                        if not our_drop_in(home_fd, home, name[:-2], extra, f"{rel}/{extra}"):
                            out.append(f"systemd complément {name}/{extra}")
                    continue
                if not (k == "file" and UNIT.fullmatch(name)):
                    out.append(f"systemd {name}" + ("" if k == "file" else f" ({k})"))
                    continue
                if folder == ".config/systemd/user":
                    own_units.add(name)
                text = (read(home_fd, rel) or b"").decode("utf-8", "replace")
                for line in text.splitlines():
                    m = re.match(r"\s*Exec[A-Za-z]*\s*=\s*[-@:+!]*\s*(\S+)", line)
                    if m and os.path.basename(m.group(1).strip("\"'")) not in EXEC_OK:
                        out.append(f"systemd {name}: lance {os.path.basename(m.group(1))}")
        # podman's own settings and quadlets (units made from ~/.config/containers/systemd at the manager's start:
        # their [Service] runs on the host; BipAgents writes none)
        for found in podman_findings(home_fd, home):
            out.append(found)
        # the code the place owns: its venvs' start-up files, ~/.local/bin
        out.extend(owned_code(home_fd, home))
        # running units and scopes of its manager (cgroupfs, read as root)
        slice_ = p(f"/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service")
        for group in ("app.slice", ""):
            folder = os.path.join(slice_, group) if group else slice_
            for name in sorted(os.listdir(folder)) if os.path.isdir(folder) else []:
                if re.search(r"\.(service|scope)$", name) and not re.fullmatch(r"libpod-[0-9a-f]+\.scope", name) \
                        and not unit_of_ours(name, uid, own_units):
                    out.append(f"unité en marche {name}")
        # transient units and timers its manager keeps until the server reboots (systemd-run): ours by their name
        for name in transient_units(uid):
            if re.search(r"\.(timer|path|socket)$", name) or not (transient_programs(name) is not None
                                                                   or RUN_SCOPE.fullmatch(name)
                                                                   or re.fullmatch(r"libpod-[0-9a-f]+\.scope", name)):
                out.append(f"systemd transitoire {name}")
        # autostart
        for name in listdir(home_fd, ".config/autostart"):
            out.append(f"autostart {name}")
        # an import's leftovers taken out while its gateway could not restart (profile_guard.py clean_main): still loaded
        guard = read(home_fd, ".local/state/bipagents/profile-guard.json")
        if guard is not None:
            try:
                if json.loads(guard.decode("utf-8", "replace")).get("restart_pending") is True:
                    out.append("profil (gateway à redémarrer)")
            except (ValueError, AttributeError):
                pass
        # its Hermes homes: what an import before 27/11 kept
        if kind(home_fd, ".hermes") == "dir":
            hermes_findings(home_fd, ".hermes", home, out)
            for name in listdir(home_fd, ".hermes/profiles"):
                if kind(home_fd, f".hermes/profiles/{name}") == "dir":
                    hermes_findings(home_fd, f".hermes/profiles/{name}", home, out)
        # its bridge's agents: programs the bridge would start (an import before 27/11 kept them)
        toml = read(home_fd, ".config/bipagents/bridge.toml")
        if toml is not None:
            try:
                import tomllib
                agents = tomllib.loads(toml.decode("utf-8", "replace")).get("agents") or {}
                for name, table in agents.items():
                    if not isinstance(table, dict):
                        continue
                    keys = [k for k in ("shares_apply", "host_home", "desktop_vnc_password_file", "hermes_root")
                            if table.get(k)]
                    py = table.get("python") or []
                    if py and os.path.basename(str(py[0])) not in ("python3", "python", "podman", "docker") \
                            and not re.fullmatch(r"python3(\.\d+)?", os.path.basename(str(py[0]))):
                        keys.append("python")
                    if keys:
                        out.append(f"bridge.toml agent {name}: {','.join(keys)}")
            except Exception:
                out.append("bridge.toml illisible")
    finally:
        os.close(home_fd)
    # crontab, at
    for spool in ("/var/spool/cron/crontabs/" + user, "/var/spool/cron/" + user):
        try:
            with open(p(spool), "rb") as fh:
                lines = [l for l in fh.read(MAX).decode("utf-8", "replace").splitlines() if l.strip() and not l.startswith("#")]
            out.append(f"crontab: {len(lines)} ligne(s)")
        except OSError:
            pass
    at = p("/var/spool/cron/atjobs")
    if os.path.isdir(at):
        jobs = [n for n in os.listdir(at) if not n.startswith(".") and os.lstat(os.path.join(at, n)).st_uid == uid]
        if jobs:
            out.append(f"tâches at: {len(jobs)}")
    if os.path.exists(p("/var/lib/systemd/linger/" + user)):
        info.append(("linger", "oui"))
    for line in processes(uid, frozenset(own_units), home):
        out.append(f"processus {line}")
    return out, info


def shown(text):
    """What a place named (files, units, profiles, agents, processes), never able to rewrite the terminal: every
    character that is not printable (a carriage return, an escape, U+007F, a direction mark) written as \\x… or \\u…."""
    return "".join(c if c.isprintable() else (f"\\x{ord(c):02x}" if ord(c) < 256 else f"\\u{ord(c):04x}")
                   for c in str(text))


# What the seat agent tells the guichet of each place: categories and numbers only, never a name or a content.
CATEGORIES = ("systemd_user", "crontab", "login_file", "ssh_key", "session", "hermes_import", "bridge_config",
              "podman", "quadlet", "process", "owned_code", "link", "error")
LOGIN_NAMES = LOGIN_FILES + (".local/bin/env",)
MAX_COUNT = 10000   # a number past it is told as it (the guichet takes 0..10 000)


def category(finding):
    """The category of one finding, from its fixed beginning (what the check writes, never the place's names)."""
    for prefix, cat in (("systemd ", "systemd_user"), ("unité en marche ", "systemd_user"), ("crontab:", "crontab"),
                        ("tâches at:", "crontab"), (".ssh/authorized_keys", "ssh_key"), ("autostart ", "session"),
                        (".pam_environment", "session"), (".ssh/rc", "session"), (".ssh/environment", "session"),
                        (".config/environment.d", "session"), ("profil ", "hermes_import"), ("hub:", "hermes_import"),
                        ("bridge.toml", "bridge_config"), ("quadlet ", "quadlet"), ("podman ", "podman"),
                        ("processus ", "process"), ("erreur", "error"), ("dossier personnel", "link"), ("code ", "owned_code")):
        if finding.startswith(prefix):
            return cat
    if finding.endswith(" : non suivi)"):
        return "link"
    if finding.startswith(LOGIN_NAMES):
        return "login_file"
    return "error"


PLACE_SECONDS = float(os.environ.get("PLACES_PLACE_SECONDS", "120"))   # one place's look at most


class PlaceTooLong(BaseException):
    """A place's look past PLACE_SECONDS: that place only is said « erreur », the others are looked at."""


def _budget(seconds):
    import signal
    import threading
    if threading.current_thread() is not threading.main_thread() or not hasattr(signal, "SIGALRM"):
        return None

    def stop(*_):
        raise PlaceTooLong()
    old = signal.signal(signal.SIGALRM, stop)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    return old


def _unbudget(old):
    import signal
    if old is not None:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, old)


def collect(argv):
    """Every place of `argv` (all when empty): (accounts, {user: findings}, the login files' shared prints, the
    skeleton's). A place whose look fails gets an « erreur » finding; the others go on."""
    _LOOKS.clear()
    found = accounts(set(argv))
    skel_prints = skel()
    prints_seen, rows = {}, {}
    for user, (uid, home) in sorted(found.items(), key=lambda x: int(x[0][3:] or 1)):
        lasting()   # the processes' two looks first, out of any place's budget
        old = _budget(PLACE_SECONDS)
        try:
            rows[user] = place(user, uid, home, skel_prints, prints_seen)
        except PlaceTooLong:
            rows[user] = (["erreur TooLong"], [])
        except Exception as exc:   # never the others' loss
            rows[user] = ([f"erreur {type(exc).__name__}"], [])
        finally:
            _unbudget(old)
    for user, (out, info) in rows.items():
        # a login file unlike the skeleton is said when no other place has the same (BipAgents' own line, uv's
        # `. "$HOME/.local/bin/env"`, is the same everywhere)
        for name, got in info:
            if name == "linger":
                continue
            if len(found) == 1 or len(prints_seen.get((name, got), [])) < 2:
                out.append(f"{name} unique (empreinte {got})")
    return found, rows, prints_seen, skel_prints


def counts(rows):
    """{user: {category: number}} for the guichet: every category, zero included."""
    return {user: {cat: min(MAX_COUNT, sum(1 for o in out if category(o) == cat)) for cat in CATEGORIES}
            for user, (out, _info) in rows.items()}


def main(argv):
    if argv[:1] == ["--json"]:   # the seat agent's (seat-agent.py place_check): numbers only
        started = time.time()
        found, rows, _, _ = collect(argv[1:])
        print(json.dumps({"places": counts(rows), "seconds": round(time.time() - started, 1),
                          "yaml": yaml_load(b"{}") is not None}))
        return 0
    found, rows, prints_seen, skel_prints = collect(argv)
    if not found:
        print("places-check-persistence: aucun compte de place ici" + (f" parmi {' '.join(argv)}" if argv else ""))
        return 2
    anything = False
    width = max(len(u) for u in rows)
    if yaml_load(b"{}") is None:
        print("(sans PyYAML ici : les config.yaml sont lus par mots seuls, moins sûrement ; "
              "installer python3-yaml pour une lecture complète)")
    print(f"{'place'.ljust(width)}  constats")
    for user, (out, info) in rows.items():
        anything = anything or bool(out)
        print(f"{user.ljust(width)}  {'; '.join(shown(o) for o in out) if out else 'rien'}")
    common = sorted({(n, g, len(us)) for (n, g), us in prints_seen.items() if len(us) > 1})
    if common:
        print("\nfichiers de connexion différents du squelette mais pareils sur plusieurs places (à regarder si inattendu) :")
        for name, got, count in common:
            print(f"  {shown(name)} empreinte {got} : {count} places")
    print("\nlecture seule ; contenus jamais affichés. « rien » : aucun piège trouvé. Linger actif : attendu.")
    return 1 if anything else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
