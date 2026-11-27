"""What a place of the offer keeps of a Hermes profile it did not make, and what it takes out of its own (27/11,
security triage § 0, lot A0).

Hermes treats a profile home as its owner's own code: whatever is written there runs as the account that runs the
gateway, outside the agents' sandbox. Tried with Hermes 0.23.1 on a place made as the installer makes one
(scripts/place-import-lab/lab.py): a stdio MCP server, a gateway hook (`hooks/`), a plugin (`plugins/` with
`plugins.enabled`), a shell hook (`hooks:` with `hooks_auto_accept`), a skill's inline shell (`skills.inline_shell`)
and a routine's script (`scripts/`) all ran as the place's account, and `.env` lines (`TERMINAL_DOCKER_VOLUMES`,
`TERMINAL_DOCKER_EXTRA_ARGS`) changed the sandbox itself. Hermes' own docs say so ("anyone who can write into it can
already run code as you", website/docs/user-guide/security.md, trusted-by-placement). Read in its code too: the
`secrets.command` source (/bin/sh -c), `command` TTS/STT providers, language servers (`lsp.servers`), the browser's
binary and flags, delegation backends, password managers' binaries, the plugin host's launcher, and a goal's quality
gates (`subprocess.run(shell=True)` after a messaging turn, kept in state.db).

- `foreign()`: a profile from elsewhere (`bipagents send` → to_place.py). Its personality, memory, skills,
  conversations, routines (their prompt) and files stay; of its settings only the sections in KEPT, laid over the
  place's hub's (to_place.place_profile), its MCP servers reached by an address only; of its `.env` only its keys
  (ENV_KEPT); its goals lose their gates.
- `own_code()`: a place's own Hermes home after its files came back (a restore, a planned move: backup.settle). What
  BipAgents never writes on a place is taken out (an import before 27/11 kept it); what the bridge writes, compared by
  value, stays (its browser server, its sandbox settings).

Both say what they took out (`left_out_labels`: what the person reads). The agents of the place make their own
connectors in their sandbox (bridge-rs/src/connectors.rs: approved, run in the agent's container for each call): that
is the way back for a connector that stays on the computer.
"""
from __future__ import annotations

import copy
import json
import os
import re
import shutil
import sqlite3
from pathlib import Path
from typing import Any, Dict, List, Optional

# Folders and files of a Hermes home (its root or a profile) Hermes 0.23.1 loads or runs code from, or that hand it
# programs (get_hermes_home() / … in its code): gateway hooks, plugins and their state, routine scripts, programs
# put on the agents' PATH, language servers, MCP servers installed from its catalog, skill bundles, Python
# environments, npm and relay settings, the egress proxy's binary, shell hooks approved once. Hermes makes some of
# them empty in every home (`hooks/`): an empty folder is no code and is not said.
CODE_PATHS = ("hooks", "plugins", "plugin-data", "plugin-update-checks", "scripts", "bin", "lib", "lsp",
              "mcp-installs", "skill-bundles", "environments", "installs", "pm-runtime", "node_modules", "npmrc",
              "relay-plugins.toml", "proxy", "shell-hooks-allowlist.json")
# What a place's own profile never holds (BipAgents writes none of it, the person has no shell); the rest of
# CODE_PATHS is state Hermes itself may keep there.
OWN_CODE_PATHS = ("hooks", "plugins", "scripts", "shell-hooks-allowlist.json")

# The settings (config.yaml) a foreign profile keeps over the hub's: what the agent is like and how it talks, its
# memory and tools. Everything else is the place's hub's (its sandbox, model, gateway and media rule, browser, voices,
# vault, updates, paths, programs).
KEPT = ("_config_version", "agent", "display", "personalities", "memory", "timezone", "compression", "context",
        "human_delay", "streaming", "goals", "loops", "curator", "tool_output", "tool_loop_guardrails", "tools",
        "toolsets", "platform_toolsets", "approvals", "command_allowlist", "cron", "mcp_servers", "auxiliary",
        "bot_mode", "bot_peers", "platforms", "session", "sessions", "privacy", "skills")
# In `skills`: never run a skill's shell snippets on the host, never read skills from outside the profile.
SKILLS_OUT = ("inline_shell", "inline_shell_timeout", "external_dirs", "create_dir", "trusted_project_dirs",
              "project_discovery")

# `.env` of a foreign profile: its keys only (an API key, the agent's own key). Hermes hands a routed profile's .env
# to the programs it starts (tools/environments/local.py), so nothing else of it comes: not a TERMINAL_* (the
# sandbox), a HERMES_* (Hermes' own switches), nor a messaging platform's token (a platform started on the place).
ENV_KEPT = re.compile(r"API_SERVER_KEY|[A-Z][A-Z0-9_]*_API_KEY")
ENV_NEVER = ("TERMINAL_", "HERMES_", "EMAIL_")

# `.env` of a place's own profile: what the bridge writes stays (its keys, its e-mail, Hermes' copy of the sandbox
# settings it sets with `hermes config set terminal.…`, sandbox.rs), compared by value; these never.
OWN_ENV_NEVER = re.compile(r"(LD_|DYLD_|PYTHON|NODE_OPTIONS|NODE_PATH|BASH_ENV|ENV$|PATH$|SHELLOPTS|BASHOPTS|PS4$|"
                           r"LUA_INIT|JAVA_TOOL_OPTIONS|LESSOPEN|STORAGE_OPTS|STORAGE_DRIVER|CONTAINER_HOST|"
                           r"CONTAINER_CONNECTION|OPENSSL_MODULES|OPENSSL_CONF|GCONV_PATH|HERMES_ACCEPT_HOOKS)")
BRIDGE_ARGS = re.compile(r"--network=bipagents-sandbox|--dns=127\.0\.0\.1|--cpus=[0-9.]+")
PROXY_ENV = {"HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "NO_PROXY", "no_proxy", "npm_config_proxy",
             "npm_config_https_proxy"}   # sandbox.rs PROXY_ENV
IMAGES = {"localhost/bipagents-sandbox:latest", "nousresearch/hermes-sandbox", "docker.io/nousresearch/hermes-sandbox"}
# what widens the sandbox, said to the person when it stays on the computer
SANDBOX_WIDE = ("TERMINAL_DOCKER_VOLUMES", "TERMINAL_DOCKER_EXTRA_ARGS", "TERMINAL_DOCKER_MOUNT_CWD_TO_WORKSPACE",
                "TERMINAL_DOCKER_RUN_AS_HOST_USER", "TERMINAL_DOCKER_FORWARD_ENV", "TERMINAL_DOCKER_ENV")


def _read_yaml(path: Path) -> Dict[str, Any]:
    import yaml
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8-sig", errors="replace")) if path.is_file() else {}
    except (OSError, ValueError, yaml.YAMLError):
        return {}
    return data if isinstance(data, dict) else {}


def _write_yaml(path: Path, data: Dict[str, Any]) -> None:
    import yaml
    path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")


def _remove(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def _holds_something(path: Path) -> bool:
    """A file, a link, or a folder with anything in it (Hermes makes some folders empty in every home)."""
    if path.is_symlink() or not path.is_dir():
        return path.exists() or path.is_symlink()
    return any(_holds_something(child) for child in path.iterdir())


def stdio_servers(data: Dict[str, Any]) -> List[str]:
    """The MCP servers of a config Hermes starts as a program (an entry with `command`, or no address), by name."""
    servers = data.get("mcp_servers")
    if not isinstance(servers, dict):
        return []
    return [str(name) for name, entry in servers.items() if not isinstance(entry, dict) or "command" in entry
            or "url" not in entry]


# What the bridge writes in its browser server's entry (browsers.rs mcp_server; `enabled` from tool_diet.rs).
BRIDGE_KEYS = {"command", "args", "connect_timeout", "timeout", "tools", "call_context", "enabled"}


def host_mount(volume: Any) -> bool:
    """A sandbox volume whose source is a folder of the host (Hermes' rule, sandbox.rs: `/`, `~`, `./`, `../`, a
    drive letter), not a named volume."""
    text = str(volume)
    source = text if re.match(r"^[A-Za-z]:[\\/]", text) else text.split(":", 1)[0]
    return source.startswith(("/", "~", ".")) or re.match(r"^[A-Za-z]:", source) is not None


def _own_terminal(data: Dict[str, Any], out: List[str], hub_backend: Optional[str]) -> None:
    """A place's own profile's `terminal:` (it wins over .env, tools/terminal_scope.py) as BipAgents writes it: the
    bridge's named volumes and arguments (sandbox.rs), its proxy env; never a host folder, other container
    arguments, the working folder mounted, the host's user or environment, nor another backend than the hub's."""
    terminal = data.get("terminal")
    if not isinstance(terminal, dict):
        return
    changed = False
    if hub_backend and terminal.get("backend", hub_backend) != hub_backend:
        terminal["backend"] = hub_backend
        changed = True
    volumes = terminal.get("docker_volumes")
    if volumes is not None:
        kept = [v for v in volumes if not host_mount(v)] if isinstance(volumes, list) else []
        if kept != volumes:
            terminal["docker_volumes"] = kept
            changed = True
    args = terminal.get("docker_extra_args")
    if args is not None:
        kept = [a for a in args if isinstance(a, str) and BRIDGE_ARGS.fullmatch(a)] if isinstance(args, list) else []
        if kept != args:
            terminal["docker_extra_args"] = kept
            changed = True
    env = terminal.get("docker_env")
    if env is not None:   # Hermes puts it in the environment of the podman it starts on the host (docker.py)
        kept = {k: v for k, v in env.items() if k in PROXY_ENV and isinstance(v, str)} if isinstance(env, dict) else {}
        if kept != env:
            terminal["docker_env"] = kept
            changed = True
    for key in ("docker_mount_cwd_to_workspace", "docker_run_as_host_user", "docker_forward_env"):
        if terminal.get(key) not in (None, False, [], "", "false"):
            terminal.pop(key)
            changed = True
    if changed and "sandbox" not in out:
        out.append("sandbox")


def bridge_browser(entry: Any, home: Optional[Path]) -> bool:
    """The bridge's own browser server (bridge-rs/src/browsers.rs `mcp_server`), exactly: `podman|docker exec -i
    bipagents-browser-<agent> /opt/stealth/mcp.sh`, or its bip-browser-mcp (in the place's bridge folder,
    rustbridge.py BINARIES) on one `…/browsers/<agent>/mcp.json` of the place."""
    if not isinstance(entry, dict) or not isinstance(entry.get("command"), str):
        return False
    command, args = entry["command"], entry.get("args") or []
    if not isinstance(args, list) or not all(isinstance(a, str) for a in args) or set(entry) - BRIDGE_KEYS:
        return False   # an `env` or `cwd` of its own (LD_PRELOAD for the podman Hermes starts), or anything else
    if os.path.isabs(command) and os.path.basename(command) in ("podman", "docker") \
            and os.path.dirname(command) in ("/usr/bin", "/usr/local/bin", "/bin"):
        return len(args) == 4 and args[:2] == ["exec", "-i"] and args[3] == "/opt/stealth/mcp.sh" \
            and re.fullmatch(r"bipagents-browser-[a-z0-9][a-z0-9_-]*", args[2]) is not None
    if home is None or os.path.basename(command) != "bip-browser-mcp" or len(args) != 1:
        return False
    bins = os.path.realpath(home / ".local" / "share" / "bipagents" / "bridge")
    data = os.path.realpath(home / ".local" / "share" / "bipagents")
    real, settings = os.path.realpath(command), os.path.realpath(args[0])
    return real.startswith(bins + os.sep) and settings.startswith(data + os.sep) \
        and os.path.basename(settings) == "mcp.json" and os.path.basename(os.path.dirname(os.path.dirname(settings))) == "browsers"


def _host_programs(data: Dict[str, Any], out: List[str]) -> None:
    """Settings naming a program Hermes runs on the host (BipAgents writes none): taken out, said once each."""
    def drop(section: Any, key: str, label: str) -> None:
        if isinstance(section, dict) and section.pop(key, None) not in (None, "", [], {}):
            out.append(label)

    secrets = data.get("secrets")
    drop(secrets, "command", "secrets.command")
    for name in ("onepassword", "bitwarden"):
        for parent, label in ((secrets, "secrets"), (data.get("vault"), "vault")):
            if isinstance(parent, dict):
                drop(parent.get(name), "binary_path", f"{label}.{name}.binary_path")
    for voice in ("tts", "stt"):
        section = data.get(voice)
        if not isinstance(section, dict):
            continue
        drop(section, "command", f"{voice}.command")
        providers = section.get("providers")
        if isinstance(providers, dict):
            for name in [n for n, p in providers.items() if isinstance(p, dict) and (p.get("type") == "command"
                                                                                       or "command" in p)]:
                providers.pop(name)
                out.append(f"{voice}.providers.{name}")
                if section.get("provider") == name:
                    section.pop("provider")
        if section.get("provider") == "command":
            section.pop("provider")
            out.append(f"{voice}.provider")
    drop(data.get("lsp"), "servers", "lsp.servers")
    browser = data.get("browser")
    for key in ("executable_path", "chromium_args"):
        drop(browser, key, f"browser.{key}")
    drop(data.get("delegation"), "backends", "delegation.backends")
    plugins = data.get("plugins")
    if isinstance(plugins, dict):
        drop(plugins.get("host"), "launcher", "plugins.host.launcher")
    desktop = data.get("desktop")
    drop(desktop, "ssh_path", "desktop.ssh_path")


def _code_settings(data: Dict[str, Any], out: List[str], own: bool, home: Optional[Path]) -> None:
    """The settings of code taken out of `data` (in place), each said once in `out`. `own`: a place's own profile,
    whose bridge browser server and `plugins` section stay (only Hermes' bundled plugins load once `plugins/` is
    gone) and whose skills keep their folders."""
    servers = data.get("mcp_servers")
    if servers is not None and not isinstance(servers, dict):
        data.pop("mcp_servers")
    for name in stdio_servers(data):
        if own and name == "stealth" and bridge_browser(data["mcp_servers"][name], home):
            continue
        data["mcp_servers"].pop(name)
        out.append(f"mcp:{name}")
    if data.pop("hooks", None):
        out.append("hooks")
    if data.pop("hooks_auto_accept", None):
        out.append("hooks_auto_accept")
    quick = data.get("quick_commands")
    if isinstance(quick, dict):
        for name in [n for n, q in quick.items() if not isinstance(q, dict) or q.get("type") != "alias"]:
            quick.pop(name)
            out.append(f"quick_command:{name}")
    elif quick is not None:
        data.pop("quick_commands")
    plugins = data.get("plugins")
    if not own and plugins is not None:
        if isinstance(plugins, dict) and plugins.get("enabled"):
            out.append("plugins.enabled")
        data.pop("plugins")
    skills = data.get("skills")
    if isinstance(skills, dict):
        if skills.get("inline_shell"):
            out.append("skills.inline_shell")
        for key in (("inline_shell",) if own else SKILLS_OUT):
            skills.pop(key, None)
    elif skills is not None:
        data.pop("skills")
    _host_programs(data, out)


def _json_file(path: Path) -> Any:
    try:
        return json.loads(path.read_bytes().decode("utf-8-sig", errors="replace"))
    except (OSError, ValueError):
        return None


def _cron(folder: Path, out: List[str]) -> None:
    """Routines keep their prompt; a script they ran stays out (a routine that was only a script goes)."""
    path = folder / "cron" / "jobs.json"
    doc = _json_file(path) if path.is_file() else None
    jobs = doc.get("jobs") if isinstance(doc, dict) else doc
    if not isinstance(jobs, list):
        return
    kept, changed = [], False
    for job in jobs:
        if not isinstance(job, dict):
            kept.append(job)
            continue
        scripted = [key for key in ("script", "monitor_script") if job.get(key)]
        if not scripted:
            kept.append(job)
            continue
        changed = True
        out.append(f"routine:{job.get('name') or job.get('id') or '?'}")
        for key in scripted:
            job[key] = None
        if job.get("no_agent") or not str(job.get("prompt") or "").strip() and not job.get("skills") \
                and not job.get("skill"):
            continue   # nothing left but the script
        kept.append(job)
    if changed:
        if isinstance(doc, dict):
            doc["jobs"] = kept
        else:
            doc = kept
        path.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")


def _goal_gates(folder: Path, out: List[str]) -> None:
    """A goal's quality gates (shell commands Hermes runs on the host after a messaging turn, hermes_cli/goals.py
    `run_gate`), kept in state.db's `state_meta` under `goal:<session>`: emptied, the goal itself kept."""
    path = folder / "state.db"
    if not path.is_file() or path.is_symlink():
        return
    try:
        db = sqlite3.connect(f"file:{path}?mode=rw", uri=True, timeout=10)
    except sqlite3.Error:
        return
    try:
        if not db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='state_meta'").fetchone():
            return
        rows = db.execute("SELECT key, value FROM state_meta WHERE key LIKE 'goal:%'").fetchall()
        for key, value in rows:
            try:
                state = json.loads(value)
            except (TypeError, ValueError):
                continue
            if isinstance(state, dict) and state.get("gates"):
                state["gates"] = []
                db.execute("UPDATE state_meta SET value = ? WHERE key = ?", (json.dumps(state), key))
                if "goal_gates" not in out:
                    out.append("goal_gates")
        db.commit()
    except sqlite3.Error:
        return
    finally:
        db.close()


def strip_code(folder: Path, paths: tuple = CODE_PATHS, own: bool = False, home: Optional[Path] = None,
               hub_backend: Optional[str] = None) -> List[str]:
    """The code of a Hermes home (a profile, or a hub's root) taken out: `paths` (an empty folder is left, unsaid),
    MCP servers started as a program (on a place's own, `own`, but the bridge's browser), hooks, exec quick commands,
    plugins, skills' inline shell, programs named in settings, routines' scripts, goals' gates. Returns what was taken
    out (`left_out_labels` says it in words)."""
    out: List[str] = []
    for name in paths:
        path = folder / name
        if path.is_symlink() or path.exists():
            if _holds_something(path):
                out.append(name)
                _remove(path)
            elif path.is_symlink():
                _remove(path)
    config = folder / "config.yaml"
    if config.is_file():
        data = _read_yaml(config)
        before = json.dumps(data, sort_keys=True, default=str)
        _code_settings(data, out, own, home)
        if own:
            _own_terminal(data, out, hub_backend)
        if json.dumps(data, sort_keys=True, default=str) != before:
            _write_yaml(config, data)
    _cron(folder, out)
    _goal_gates(folder, out)
    return out


def _env_lines(path: Path) -> List[str]:
    return path.read_bytes().decode("utf-8", errors="replace").splitlines() if path.is_file() else []


def _name(line: str) -> Optional[str]:
    bare = line.strip()
    if not bare or bare.startswith("#") or "=" not in bare:
        return None
    return bare.split("=", 1)[0].strip().removeprefix("export ").strip()


def env_value(raw: str) -> str:
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


def _value(line: str) -> str:
    return env_value(line.split("=", 1)[1])


def env_kept(name: str) -> bool:
    """A `.env` name a foreign profile keeps: one of its keys, nothing that says where or how programs run."""
    name = name.strip().removeprefix("export ").strip()
    return ENV_KEPT.fullmatch(name) is not None and not name.startswith(ENV_NEVER)


def _wide(name: str, value: str) -> bool:
    """Whether a TERMINAL_* line widens the sandbox (said to the person when it stays on the computer)."""
    return name in SANDBOX_WIDE and value.strip().lower() not in ("", "[]", "{}", "false", "0", "no")


def own_sandbox_value(name: str, value: str) -> bool:
    """A TERMINAL_* line of a place's own profile as BipAgents writes it (installer `terminal.backend docker`, the
    bridge's `hermes config set terminal.…`, sandbox.rs), by value."""
    try:
        parsed = json.loads(value) if value[:1] in "[{" else value
    except ValueError:
        return False
    if name == "TERMINAL_ENV":
        return value == "docker"
    if name == "TERMINAL_DOCKER_NETWORK":
        return value.lower() in ("true", "false")
    if name == "TERMINAL_DOCKER_IMAGE":
        return value in IMAGES or value.startswith("docker.io/nousresearch/hermes-sandbox:")
    if name == "TERMINAL_DOCKER_EXTRA_ARGS":
        return isinstance(parsed, list) and all(isinstance(a, str) and BRIDGE_ARGS.fullmatch(a) for a in parsed)
    if name == "TERMINAL_DOCKER_ENV":
        return isinstance(parsed, dict) and set(parsed) <= PROXY_ENV
    if name == "TERMINAL_DOCKER_VOLUMES":   # the bridge's named volumes only (bipagents-files-, -share-…)
        return isinstance(parsed, list) and all(isinstance(v, str) and v.startswith("bipagents-") for v in parsed)
    return name in ("TERMINAL_TIMEOUT", "TERMINAL_LIFETIME_SECONDS", "TERMINAL_DOCKER_IMAGE_PINNED")


def _env(folder: Path, out: List[str], own: bool) -> None:
    path = folder / ".env"
    lines = _env_lines(path)
    if not lines:
        return
    kept = []
    for line in lines:
        name = _name(line)
        if name is None:
            kept.append(line)
            continue
        value = _value(line)
        if own:
            if name.startswith("TERMINAL_"):
                keep = own_sandbox_value(name, value)
            elif name == "HERMES_DOCKER_BINARY":   # the installer's (hermes.py setup_hub): the system's engine
                keep = value in ("/usr/bin/podman", "/usr/bin/docker", "/usr/local/bin/podman", "/usr/local/bin/docker")
            else:
                keep = OWN_ENV_NEVER.match(name) is None and not re.search(
                    r"_(PATH|BIN|BINARY|COMMAND|CMD|EXECUTABLE|LAUNCHER|ARGS)$", name)
        else:
            keep = env_kept(name)
        if keep:
            kept.append(line)
            continue
        if name.startswith("TERMINAL_"):
            if (own or _wide(name, value)) and "sandbox" not in out:
                out.append("sandbox")
        elif own or re.search(r"_(TOKEN|SECRET|PASSWORD|KEY)$", name):
            out.append(f"env:{name}")
    if kept != lines:
        path.write_text("\n".join(kept) + ("\n" if kept else ""), encoding="utf-8")
        os.chmod(path, 0o600)


def own_code(hermes_home: Path, home: Optional[Path] = None) -> Dict[str, List[str]]:
    """A place's own Hermes home after its files came back (a restore, a planned move): what it never holds taken
    out of its root and of each profile; what the bridge writes stays (its browser server, compared with what it
    writes; its sandbox settings, by value). `home`: the place's home (where its bridge's programs are). Returns
    what was taken out, by folder."""
    found: Dict[str, List[str]] = {}
    home = home or hermes_home.parent
    profiles = hermes_home / "profiles"
    hub_terminal = _read_yaml(hermes_home / "config.yaml").get("terminal")
    hub_backend = hub_terminal.get("backend") if isinstance(hub_terminal, dict) else None
    hub_backend = hub_backend if hub_backend == "docker" else None   # a place's sandbox; else nothing forced
    for folder in [hermes_home] + (sorted(p for p in profiles.iterdir() if p.is_dir() and not p.is_symlink())
                                   if profiles.is_dir() else []):
        out = strip_code(folder, OWN_CODE_PATHS, own=True, home=home, hub_backend=hub_backend)
        _env(folder, out, own=True)
        if out:
            found[str(folder)] = out
    return found


def foreign(folder: Path) -> List[str]:
    """A profile that comes from elsewhere, cut down to what a place's agent has (module docstring): its code out,
    its settings to KEPT (the caller lays them over the hub's: to_place.place_profile), its `.env` to its keys."""
    out = strip_code(folder)
    config = folder / "config.yaml"
    if config.is_file():
        data = _read_yaml(config)
        kept = {key: value for key, value in data.items() if key in KEPT}
        if isinstance(kept.get("mcp_servers"), dict):
            for entry in kept["mcp_servers"].values():
                if isinstance(entry, dict):   # an address only: no program, no environment of its own
                    for key in ("env", "cwd", "args"):
                        entry.pop(key, None)
        memory = kept.get("memory")
        if isinstance(memory, dict):
            memory.pop("provider", None)   # an external memory runs code of its own; the place's is Hermes'
        platforms = kept.get("platforms")
        api = platforms.get("api_server") if isinstance(platforms, dict) else None
        kept.pop("platforms", None)
        if isinstance(api, dict) and isinstance(api.get("extra"), dict):
            kept["platforms"] = {"api_server": {"extra": api["extra"]}}
        if kept != data:
            _write_yaml(config, kept)
    _env(folder, out, own=False)
    return out


def over_hub(hub: Dict[str, Any], imported: Dict[str, Any]) -> Dict[str, Any]:
    """The hub's settings (as the place's own agents are cloned from it) with a foreign profile's KEPT sections laid
    over them, section by section (a key of the profile's wins inside a section)."""
    def merge(base: Any, over: Any) -> Any:
        if isinstance(base, dict) and isinstance(over, dict):
            merged = dict(base)
            for key, value in over.items():
                merged[key] = merge(base.get(key), value)
            return merged
        return copy.deepcopy(over)
    data = copy.deepcopy(hub)
    for key in KEPT:
        if key in imported:
            data[key] = merge(data.get(key), imported[key])
    return data


# What the person reads about what stayed on their computer (to be validated before the apps say it).
LEFT_OUT = {
    "fr": ("Pour la sécurité de ton serveur, certaines choses restent sur ton ordinateur : {what}. "
           "Tes agents gardent leur mémoire, leurs conversations, leurs savoir-faire et leurs routines. "
           "Pour un connecteur, demande à ton agent de le recréer : il tournera dans son bac à sable."),
    "en": ("To keep your server safe, a few things stay on your computer: {what}. "
           "Your agents keep their memory, conversations, skills and routines. "
           "For a connector, ask your agent to make it again: it will run in its sandbox."),
    "de": ("Zum Schutz deines Servers bleibt einiges auf deinem Computer: {what}. "
           "Deine Agenten behalten ihr Gedächtnis, ihre Gespräche, ihre Fähigkeiten und ihre Routinen. "
           "Für einen Connector: Bitte deinen Agenten, ihn neu anzulegen – er läuft dann in seiner Sandbox."),
    "es": ("Para proteger tu servidor, algunas cosas se quedan en tu ordenador: {what}. "
           "Tus agentes conservan su memoria, sus conversaciones, sus habilidades y sus rutinas. "
           "Para un conector, pide a tu agente que lo vuelva a crear: funcionará en su entorno aislado."),
}
KINDS = {   # label kind → words, per language
    "links": {"fr": "les liens (raccourcis) de tes dossiers", "en": "your folders' links (shortcuts)",
              "de": "die Verknüpfungen deiner Ordner", "es": "los enlaces (accesos directos) de tus carpetas"},
    "mcp": {"fr": "les connecteurs installés sur l'ordinateur ({names})", "en": "connectors installed on the computer "
            "({names})", "de": "die auf dem Computer installierten Connectoren ({names})",
            "es": "los conectores instalados en el ordenador ({names})"},
    "ext": {"fr": "les extensions et automatismes de Hermes (hooks, plugins, scripts)", "en": "Hermes extensions and "
            "automations (hooks, plugins, scripts)", "de": "Hermes-Erweiterungen und Automatisierungen (Hooks, "
            "Plugins, Skripte)", "es": "las extensiones y automatismos de Hermes (hooks, plugins, scripts)"},
    "sandbox": {"fr": "les réglages de bac à sable de l'ordinateur", "en": "the computer's sandbox settings",
                "de": "die Sandbox-Einstellungen des Computers", "es": "los ajustes de entorno aislado del ordenador"},
    "env": {"fr": "les jetons de messageries et d'outils de l'ordinateur (seules les clés d'API viennent)",
            "en": "the computer's messaging and tool tokens (only API keys come along)",
            "de": "die Messenger- und Tool-Tokens des Computers (nur API-Schlüssel kommen mit)",
            "es": "los tokens de mensajería y de herramientas del ordenador (solo vienen las claves de API)"},
}


def left_out_labels(labels: List[str], language: str = "en") -> Optional[str]:
    """`labels` (strip_code / foreign) said to the person in `language` (fr, en, de, es); None when nothing."""
    lang = language if language in LEFT_OUT else "en"
    servers = sorted({label.split(":", 1)[1] for label in labels if label.startswith("mcp:")})
    parts = []
    if servers:
        parts.append(KINDS["mcp"][lang].format(names=", ".join(servers)))
    if any(not label.startswith(("mcp:", "env:")) and label not in ("sandbox", "links") for label in labels):
        parts.append(KINDS["ext"][lang])
    if "sandbox" in labels:
        parts.append(KINDS["sandbox"][lang])
    if any(label.startswith("env:") for label in labels):
        parts.append(KINDS["env"][lang])
    if "links" in labels:
        parts.append(KINDS["links"][lang])
    return LEFT_OUT[lang].format(what="; ".join(parts)) if parts else None


def scan(folder: Path) -> List[str]:
    """What `foreign` would take out of this Hermes home, without changing it (the computer's side of a send)."""
    import tempfile
    with tempfile.TemporaryDirectory() as scratch:
        copy_ = Path(scratch) / "p"
        copy_.mkdir()
        for name in ("config.yaml", ".env"):
            if (folder / name).is_file() and not (folder / name).is_symlink():
                shutil.copy2(folder / name, copy_ / name)
        if (folder / "cron" / "jobs.json").is_file():
            (copy_ / "cron").mkdir()
            shutil.copy2(folder / "cron" / "jobs.json", copy_ / "cron" / "jobs.json")
        found = [name for name in CODE_PATHS if _holds_something(folder / name)]
        return found + foreign(copy_)
