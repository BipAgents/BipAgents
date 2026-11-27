"""What a place of the offer keeps of a Hermes profile it did not make (27/11, security triage § 0, lot A0).

Hermes treats a profile home as its owner's own code: whatever is written there runs as the account that runs the
gateway, outside the agents' sandbox. Tried with Hermes 0.23.1 on a place made as the installer makes one
(scripts/place-import-lab/lab.py): a stdio MCP server, a gateway hook (`hooks/`), a plugin (`plugins/` with
`plugins.enabled`), a shell hook (`hooks:` with `hooks_auto_accept`), a skill's inline shell (`skills.inline_shell`)
and a routine's script (`scripts/`) all ran as the place's account, and `.env` lines (`TERMINAL_DOCKER_VOLUMES`,
`TERMINAL_DOCKER_EXTRA_ARGS`) changed the sandbox itself. Hermes' own docs say so ("anyone who can write into it can
already run code as you", website/docs/user-guide/security.md, trusted-by-placement).

So a profile that comes from elsewhere (`bipagents send` → to_place.py) is cut down to what the place's own agents
have: `foreign()` keeps its personality, memory, skills, conversations, routines and files, and of its settings only
the sections listed in KEPT (the rest is the place's), its MCP servers reached by an address (never one started as a
command: the bridge's own come back with `adopt` and `browser add`), its keys and tokens in `.env` (never a
`TERMINAL_*`, `HERMES_*` or a process setting). `strip_code()` is the part that holds for a place's own profile
too (a backup put back, a place moved): BipAgents never writes any of it on a place, so anything found there is taken
out the same way. Both return what they took out, for the person (`left_out_labels`).

The agents of the place make their own connectors in their sandbox (bridge-rs/src/connectors.rs: approved, run in the
agent's container for each call): that is the way back for a connector that stays on the computer.
"""
from __future__ import annotations

import json
import os
import re
import shutil
from pathlib import Path
from typing import Any, Dict, List, Optional

# Folders and files of a Hermes home (its root or a profile) Hermes 0.23.1 loads or runs code from, or that hand it
# programs (get_hermes_home() / … in its code): gateway hooks, plugins and their state, routine scripts, programs
# put on the agents' PATH, language servers, MCP servers installed from its catalog, skill bundles, Python
# environments, npm and relay settings, the egress proxy's binary, shell hooks approved once.
CODE_PATHS = ("hooks", "plugins", "plugin-data", "plugin-update-checks", "scripts", "bin", "lib", "lsp",
              "mcp-installs", "skill-bundles", "environments", "installs", "pm-runtime", "node_modules", "npmrc",
              "relay-plugins.toml", "proxy", "shell-hooks-allowlist.json")

# The settings (config.yaml) a foreign profile keeps: what the agent is like and how it talks, its memory and tools.
# Everything else is the place's (its sandbox, model, gateway, browser, voices, vault, updates, paths, programs).
KEPT = ("_config_version", "agent", "display", "personalities", "memory", "timezone", "compression", "context",
        "human_delay", "streaming", "goals", "loops", "curator", "tool_output", "tool_loop_guardrails", "tools",
        "toolsets", "platform_toolsets", "approvals", "command_allowlist", "cron", "mcp_servers", "auxiliary",
        "bot_mode", "bot_peers", "platforms", "session", "sessions", "privacy", "skills",
        # replaced or completed for the place by to_place.place_profile
        "model", "terminal", "providers")
# In `skills`: never run a skill's shell snippets on the host, never read skills from outside the profile.
SKILLS_OUT = ("inline_shell", "inline_shell_timeout", "external_dirs", "create_dir", "trusted_project_dirs",
              "project_discovery")
# Settings of code in any profile (a place's own included): BipAgents writes none of them on a place.
CODE_SETTINGS = ("hooks", "hooks_auto_accept")

# `.env`: names that set where and how programs run, never a key of the person's.
ENV_OUT_PREFIXES = ("TERMINAL_", "HERMES_", "LD_", "PYTHON", "NODE_", "NPM_", "UV_", "PIP_", "GIT_", "SSH_",
                    "DOCKER_", "PODMAN_", "CONTAINERS_", "XDG_", "BASH_", "DYLD_", "PERL", "RUBY", "GCONV_")
ENV_OUT_NAMES = {"PATH", "HOME", "SHELL", "ENV", "BROWSER", "EDITOR", "VISUAL", "PAGER", "TMPDIR", "SUDO_PASSWORD",
                 "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "MALLOC_CONF", "OPENSSL_CONF", "SSL_CERT_FILE",
                 "SSL_CERT_DIR", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"}
ENV_OUT_SUFFIXES = ("_PATH", "_BIN", "_BINARY", "_COMMAND", "_CMD", "_EXECUTABLE", "_ARGS", "_DIR", "_DIRS",
                    "_HOME", "_LAUNCHER", "_PROXY")


def _read_yaml(path: Path) -> Dict[str, Any]:
    import yaml
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8")) if path.is_file() else {}
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


def stdio_servers(data: Dict[str, Any]) -> List[str]:
    """The MCP servers of a config Hermes starts as a program (an entry with `command`), by name."""
    servers = data.get("mcp_servers")
    if not isinstance(servers, dict):
        return []
    return [str(name) for name, entry in servers.items() if not isinstance(entry, dict) or "command" in entry
            or "url" not in entry]


def _code_settings(data: Dict[str, Any], out: List[str], keep: tuple = (), own: bool = False) -> None:
    """The settings of code taken out of `data` (in place), each said once in `out`. `keep`: MCP servers started as a
    program that stay (the bridge's own, on a place's own profile). `own`: a place's own profile, whose `plugins`
    section stays (only Hermes' bundled plugins can load once `plugins/` is gone) and whose skills keep their
    folders."""
    servers = data.get("mcp_servers")
    if servers is not None and not isinstance(servers, dict):
        data.pop("mcp_servers")
    for name in stdio_servers(data):
        if name not in keep:
            data["mcp_servers"].pop(name)
            out.append(f"mcp:{name}")
    for key in CODE_SETTINGS:
        if data.pop(key, None):
            out.append(key)
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


def _cron(folder: Path, out: List[str]) -> None:
    """Routines keep their prompt; a script they ran stays out (a routine that was only a script goes)."""
    path = folder / "cron" / "jobs.json"
    if not path.is_file():
        return
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
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


# What a place's own profile never holds (a backup put back or a place moved: backup.settle): BipAgents writes none of it there, the
# person has no shell, and Hermes makes none of it by itself; the rest of CODE_PATHS is state Hermes may keep.
OWN_CODE_PATHS = ("hooks", "plugins", "scripts", "shell-hooks-allowlist.json")
BRIDGE_SERVERS = ("stealth",)   # the bridge's own stdio MCP server (bridge-rs/src/browsers.rs `mcp_server`)


def strip_code(folder: Path, keep_servers: tuple = (), paths: tuple = CODE_PATHS, own: bool = False) -> List[str]:
    """The code of a Hermes home (a profile, or a hub's root) taken out: `paths`, MCP servers started as a program
    (but `keep_servers`), hooks, exec quick commands, plugins, skills' inline shell, routines' scripts. Returns what
    was taken out (`left_out_labels` says it in words). A place's own (`own`): see own_code()."""
    out: List[str] = []
    for name in paths:
        path = folder / name
        if path.exists() or path.is_symlink():
            _remove(path)
            out.append(name)
    config = folder / "config.yaml"
    if config.is_file():
        data = _read_yaml(config)
        before = json.dumps(data, sort_keys=True, default=str)
        _code_settings(data, out, keep_servers, own)
        if json.dumps(data, sort_keys=True, default=str) != before:
            _write_yaml(config, data)
    _cron(folder, out)
    return out


def own_code(hermes_home: Path) -> Dict[str, List[str]]:
    """A place's own Hermes home after its files came back (a restore, a planned move): what it never holds taken
    out of its root and of each profile (BRIDGE_SERVERS stay). Returns what was taken out, by folder."""
    found: Dict[str, List[str]] = {}
    profiles = hermes_home / "profiles"
    for folder in [hermes_home] + (sorted(p for p in profiles.iterdir() if p.is_dir()) if profiles.is_dir() else []):
        out = strip_code(folder, BRIDGE_SERVERS, OWN_CODE_PATHS, own=True)
        if out:
            found[str(folder)] = out
    return found


def env_kept(name: str) -> bool:
    """A `.env` name a foreign profile keeps: a key, token or setting of the person's, never one that says where or
    how programs run."""
    name = name.strip().removeprefix("export ").strip()
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
        return False
    upper = name.upper()
    return not (upper in ENV_OUT_NAMES or upper.startswith(ENV_OUT_PREFIXES) or upper.endswith(ENV_OUT_SUFFIXES))


def _env(folder: Path, out: List[str]) -> None:
    path = folder / ".env"
    if not path.is_file():
        return
    lines = path.read_text(encoding="utf-8").splitlines()
    kept = []
    for line in lines:
        bare = line.strip()
        if not bare or bare.startswith("#") or "=" not in bare:
            kept.append(line)
            continue
        name = bare.split("=", 1)[0]
        if env_kept(name):
            kept.append(line)
        elif name.strip().removeprefix("export ").strip().upper().startswith("TERMINAL_"):
            if "sandbox" not in out:
                out.append("sandbox")
    if kept != lines:
        path.write_text("\n".join(kept) + ("\n" if kept else ""), encoding="utf-8")
        os.chmod(path, 0o600)


def foreign(folder: Path) -> List[str]:
    """A profile that comes from elsewhere, cut down to what a place's agent has (module docstring). The caller then
    sets the place's model and sandbox (to_place.place_profile)."""
    out = strip_code(folder)
    config = folder / "config.yaml"
    if config.is_file():
        data = _read_yaml(config)
        kept = {key: value for key, value in data.items() if key in KEPT}
        if isinstance(kept.get("mcp_servers"), dict):
            for name, entry in list(kept["mcp_servers"].items()):
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
    _env(folder, out)
    return out


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
    "mcp": {"fr": "les connecteurs installés sur l'ordinateur ({names})", "en": "connectors installed on the computer "
            "({names})", "de": "die auf dem Computer installierten Connectoren ({names})",
            "es": "los conectores instalados en el ordenador ({names})"},
    "ext": {"fr": "les extensions et automatismes de Hermes (hooks, plugins, scripts)", "en": "Hermes extensions and "
            "automations (hooks, plugins, scripts)", "de": "Hermes-Erweiterungen und Automatisierungen (Hooks, "
            "Plugins, Skripte)", "es": "las extensiones y automatismos de Hermes (hooks, plugins, scripts)"},
    "sandbox": {"fr": "les réglages de bac à sable de l'ordinateur", "en": "the computer's sandbox settings",
                "de": "die Sandbox-Einstellungen des Computers", "es": "los ajustes de entorno aislado del ordenador"},
}


def left_out_labels(labels: List[str], language: str = "en") -> Optional[str]:
    """`labels` (strip_code / foreign) said to the person in `language` (fr, en, de, es); None when nothing."""
    lang = language if language in LEFT_OUT else "en"
    servers = sorted({label.split(":", 1)[1] for label in labels if label.startswith("mcp:")})
    parts = []
    if servers:
        parts.append(KINDS["mcp"][lang].format(names=", ".join(servers)))
    if any(not label.startswith("mcp:") and label != "sandbox" for label in labels):
        parts.append(KINDS["ext"][lang])
    if "sandbox" in labels:
        parts.append(KINDS["sandbox"][lang])
    return LEFT_OUT[lang].format(what="; ".join(parts)) if parts else None


def scan(folder: Path) -> List[str]:
    """What `foreign` would take out of this Hermes home, without changing it (the computer's side of a send)."""
    import tempfile
    with tempfile.TemporaryDirectory() as scratch:
        copy = Path(scratch) / "p"
        copy.mkdir()
        for name in ("config.yaml", ".env"):
            if (folder / name).is_file():
                shutil.copy2(folder / name, copy / name)
        if (folder / "cron" / "jobs.json").is_file():
            (copy / "cron").mkdir()
            shutil.copy2(folder / "cron" / "jobs.json", copy / "cron" / "jobs.json")
        found = [name for name in CODE_PATHS if (folder / name).exists() or (folder / name).is_symlink()]
        return found + foreign(copy)
