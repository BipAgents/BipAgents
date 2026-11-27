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
EXEC_OK = {"python3", "python", "python3.12", "python3.13", "python3.14", "bipbridge", "hermes", "podman", "bash",
           "sh", "env", "uv", "systemctl", "true", "rm", "mkdir", "sleep", "restic", "bip-browser-mcp", "nice",
           "ionice", "flock", "timeout"}
# Processes of a place's account outside its containers (the host side): what a place runs.
PROC_OK = {"systemd", "(sd-pam)", "sd-pam", "bipbridge", "bip-browser-mcp", "hermes", "python3", "python",
           "python3.12", "python3.13", "python3.14", "podman", "conmon", "catatonit", "pasta", "pasta.avx2",
           "slirp4netns", "rootlessport", "rootlessport-ch", "dbus-daemon", "dbus-broker", "dbus-broker-lau",
           "restic", "systemctl", "journalctl", "uv", "git", "aardvark-dns", "netavark", "crun", "runc",
           "fuse-overlayfs"}
PROC_UNIT = re.compile(r"(init\.scope|(bipagents|hermes)-[A-Za-z0-9@._-]+\.(service|scope)|dbus(-broker)?\.service|"
                       r"podman-[A-Za-z0-9@._-]+\.scope|libpod-conmon-[0-9a-f]+\.scope)$")


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


def skel():
    """Each login file's fingerprints a place may have: the skeleton's, alone or with uv's line."""
    out = {}
    for name in LOGIN_FILES:
        try:
            with open(p("/etc/skel/" + name), "rb") as fh:
                data = fh.read(MAX)
        except OSError:
            continue
        out[name] = {short(data)} | {short(data + line) for line in UV_LINES}
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


def processes(uid):
    """The account's processes outside its containers: (comm, unit, exe folder) for those not expected."""
    odd = []
    proc = p("/proc")
    for pid in os.listdir(proc) if os.path.isdir(proc) else []:
        if not pid.isdigit():
            continue
        base = f"{proc}/{pid}"
        try:
            if os.stat(base).st_uid != uid:
                continue
            comm = open(f"{base}/comm", encoding="utf-8", errors="replace").read().strip()
            cgroup = open(f"{base}/cgroup", encoding="utf-8", errors="replace").read().strip().splitlines()[-1]
        except (OSError, IndexError):
            continue
        if "/libpod-" in cgroup and "libpod-conmon-" not in cgroup:
            continue   # in one of its containers (the sandbox, a browser, a connector)
        unit = cgroup.rsplit("/", 1)[-1]
        try:
            exe = os.readlink(f"{base}/exe")
        except OSError:
            exe = ""
        exe_dir = os.path.dirname(exe.replace(" (deleted)", ""))
        bad_dir = exe.endswith(" (deleted)") or re.match(r"/(tmp|var/tmp|dev/shm|run/user)/", exe or "")
        if comm not in PROC_OK or not PROC_UNIT.search(unit) or bad_dir:
            odd.append(f"{comm} [{unit}] {exe_dir or '?'}{' (supprimé)' if exe.endswith(' (deleted)') else ''}")
    return sorted(set(odd))


def place(user, uid, home, skel_prints, prints_seen):
    out, info = [], []
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
            got = short(read(home_fd, name) or b"")
            prints_seen.setdefault((name, got), []).append(user)
            if got in skel_prints.get(name, ()):
                continue
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
                text = (read(home_fd, rel) or b"").decode("utf-8", "replace")
                for line in text.splitlines():
                    m = re.match(r"\s*Exec[A-Za-z]*\s*=\s*[-@:+!]*\s*(\S+)", line)
                    if m and os.path.basename(m.group(1).strip("\"'")) not in EXEC_OK:
                        out.append(f"systemd {name}: lance {os.path.basename(m.group(1))}")
        # podman's own settings and quadlets (units made from ~/.config/containers/systemd at the manager's start:
        # their [Service] runs on the host; BipAgents writes none)
        for found in podman_findings(home_fd, home):
            out.append(found)
        # running units and scopes of its manager (cgroupfs, read as root)
        slice_ = p(f"/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service")
        for group in ("app.slice", ""):
            folder = os.path.join(slice_, group) if group else slice_
            for name in sorted(os.listdir(folder)) if os.path.isdir(folder) else []:
                if re.search(r"\.(service|scope)$", name) and not PROC_UNIT.search(name) and not name.startswith("libpod-"):
                    out.append(f"unité en marche {name}")
        # autostart
        for name in listdir(home_fd, ".config/autostart"):
            out.append(f"autostart {name}")
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
    for line in processes(uid):
        out.append(f"processus {line}")
    return out, info


def shown(text):
    """What a place named (files, units, profiles, agents, processes), never able to rewrite the terminal: every
    character that is not printable (a carriage return, an escape, U+007F, a direction mark) written as \\x… or \\u…."""
    return "".join(c if c.isprintable() else (f"\\x{ord(c):02x}" if ord(c) < 256 else f"\\u{ord(c):04x}")
                   for c in str(text))


# What the seat agent tells the guichet of each place: categories and numbers only, never a name or a content.
CATEGORIES = ("systemd_user", "crontab", "login_file", "ssh_key", "session", "hermes_import", "bridge_config",
              "podman", "quadlet", "process", "link", "error")
LOGIN_NAMES = LOGIN_FILES + (".local/bin/env",)


def category(finding):
    """The category of one finding, from its fixed beginning (what the check writes, never the place's names)."""
    for prefix, cat in (("systemd ", "systemd_user"), ("unité en marche ", "systemd_user"), ("crontab:", "crontab"),
                        ("tâches at:", "crontab"), (".ssh/authorized_keys", "ssh_key"), ("autostart ", "session"),
                        (".pam_environment", "session"), (".ssh/rc", "session"), (".ssh/environment", "session"),
                        (".config/environment.d", "session"), ("profil ", "hermes_import"), ("hub:", "hermes_import"),
                        ("bridge.toml", "bridge_config"), ("quadlet ", "quadlet"), ("podman ", "podman"),
                        ("processus ", "process"), ("erreur", "error"), ("dossier personnel", "link")):
        if finding.startswith(prefix):
            return cat
    if finding.endswith(" : non suivi)"):
        return "link"
    if finding.startswith(LOGIN_NAMES):
        return "login_file"
    return "error"


def collect(argv):
    """Every place of `argv` (all when empty): (accounts, {user: findings}, the login files' shared prints, the
    skeleton's). A place whose look fails gets an « erreur » finding; the others go on."""
    found = accounts(set(argv))
    skel_prints = skel()
    prints_seen, rows = {}, {}
    for user, (uid, home) in sorted(found.items(), key=lambda x: int(x[0][3:] or 1)):
        try:
            rows[user] = place(user, uid, home, skel_prints, prints_seen)
        except Exception as exc:   # never the others' loss
            rows[user] = ([f"erreur {type(exc).__name__}"], [])
    for user, (out, info) in rows.items():
        # a login file unlike the skeleton is said when no other place has the same (BipAgents' own line, uv's
        # `. "$HOME/.local/bin/env"`, is the same everywhere)
        for name, got in info:
            if name == "linger":
                continue
            if len(found) == 1 and name == ".local/bin/env":
                continue   # uv's own file: only told when it differs from the other places'
            if len(found) == 1 or len(prints_seen.get((name, got), [])) < 2:
                out.append(f"{name} unique (empreinte {got})")
    return found, rows, prints_seen, skel_prints


def counts(rows):
    """{user: {category: number}} for the guichet: every category, zero included."""
    return {user: {cat: sum(1 for o in out if category(o) == cat) for cat in CATEGORIES}
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
    common = sorted({(n, g, len(us)) for (n, g), us in prints_seen.items()
                     if len(us) > 1 and g not in skel_prints.get(n, ()) and n != ".local/bin/env"})
    if common:
        print("\nfichiers de connexion différents du squelette mais pareils sur plusieurs places (à regarder si inattendu) :")
        for name, got, count in common:
            print(f"  {shown(name)} empreinte {got} : {count} places")
    print("\nlecture seule ; contenus jamais affichés. « rien » : aucun piège trouvé. Linger actif : attendu.")
    return 1 if anything else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
