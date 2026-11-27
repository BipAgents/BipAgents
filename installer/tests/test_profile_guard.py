"""profile_guard: what a place keeps of a profile it did not make, what it takes out of its own, and what the person
is told (27/11, security triage § 0, lot A0). The import itself: test_to_place.py (`traps`)."""
import json

import pytest
import yaml

from bipinstall import profile_guard, to_place
from test_to_place import traps, write


def test_a_place_s_own_profiles_lose_only_code_nothing_of_the_bridge(tmp_path):
    hub = tmp_path / ".hermes"
    traps(hub / "profiles" / "coach")
    cfg = yaml.safe_load((hub / "profiles" / "coach" / "config.yaml").read_text())
    cfg["mcp_servers"]["stealth"] = {"command": "/usr/bin/podman", "args": ["exec", "-i", "bipagents-browser-coach",
                                                                           "/opt/stealth/mcp.sh"]}
    cfg["mcp_servers"]["bipagents"] = {"url": "http://127.0.0.1:8673/v1/agent-tools/mcp"}
    cfg["plugins"] = {"enabled": ["disk-cleanup"]}
    write(hub / "profiles" / "coach" / "config.yaml", yaml.safe_dump(cfg))
    write(hub / "config.yaml", yaml.safe_dump({"terminal": {"backend": "docker"}, "gateway": {"strict": True}}))
    found = profile_guard.own_code(hub)
    coach = hub / "profiles" / "coach"
    assert set(found) == {str(coach)}
    for name in ("hooks", "plugins", "scripts", "shell-hooks-allowlist.json"):
        assert not (coach / name).exists()
    after = yaml.safe_load((coach / "config.yaml").read_text())
    assert set(after["mcp_servers"]) == {"stealth", "bipagents", "notes"}                 # the bridge's own stay
    assert "hooks" not in after and "hooks_auto_accept" not in after and "inline_shell" not in after["skills"]
    assert after["quick_commands"] == {"s": {"type": "alias", "target": "/status"}}
    assert after["plugins"] == {"enabled": ["disk-cleanup"]}       # only Hermes' own plugins can load now
    assert after["terminal"] == cfg["terminal"] and after["memory"] == cfg["memory"]    # the place's settings stay
    # programs named in settings go, the rest of their sections stays
    assert after["browser"] == {} and after["lsp"] == {} and after["tts"] == {}
    assert after["delegation"] == {} and after["vault"] == {"onepassword": {}}
    env = (coach / ".env").read_text()                       # the bridge's own names stay, by value
    assert env.startswith("API_SERVER_KEY=") and "OPENROUTER_API_KEY=sam-own" in env and "EMAIL_ADDRESS" in env
    for gone in ("TERMINAL_DOCKER_VOLUMES", "TERMINAL_DOCKER_EXTRA_ARGS", "TERMINAL_ENV=local", "HERMES_ACCEPT_HOOKS",
                 "LD_PRELOAD", "PATH=", "BROWSER_EXECUTABLE_PATH"):
        assert gone not in env, gone
    assert yaml.safe_load((hub / "config.yaml").read_text()) == {"terminal": {"backend": "docker"},
                                                                 "gateway": {"strict": True}}
    assert profile_guard.own_code(hub) == {}                                            # once is enough


def test_the_person_is_told_what_stayed_on_the_computer():
    labels = ["mcp:temoin", "mcp:notion", "hooks", "plugins", "sandbox"]
    for language, words in (("fr", "connecteurs installés sur l'ordinateur (notion, temoin)"),
                            ("en", "connectors installed on the computer (notion, temoin)"),
                            ("de", "Connectoren (notion, temoin)"), ("es", "conectores instalados en el ordenador")):
        said = profile_guard.left_out_labels(labels, language)
        assert words in said and said.count("{") == 0
    assert profile_guard.left_out_labels([], "fr") is None
    assert profile_guard.left_out_labels(["hooks"], "it") == profile_guard.left_out_labels(["hooks"], "en")


def test_the_computer_says_before_sending_and_changes_nothing(tmp_path):
    alone = tmp_path / "sam"
    hub = alone / ".hermes"
    traps(hub / "profiles" / "jardin")
    write(hub / "config.yaml", "model:\n  default: claude\n")
    before = {p: p.read_bytes() for p in hub.rglob("*") if p.is_file()}
    said = to_place.stays_here(to_place.LoneHermes(hub, alone))
    assert "connectors installed on the computer (temoin)" in said and "hooks, plugins, scripts" in said
    assert {p: p.read_bytes() for p in hub.rglob("*") if p.is_file()} == before
    assert to_place.stays_here(to_place.LoneHermes(tmp_path / "x" / ".hermes", tmp_path / "x")) is None


@pytest.mark.parametrize("name,kept", [("API_SERVER_KEY", True), ("OPENROUTER_API_KEY", True), ("EMAIL_IMAP_HOST", False),
                                       ("TELEGRAM_BOT_TOKEN", False), ("TERMINAL_DOCKER_VOLUMES", False),
                                       ("HERMES_DOCKER_BINARY", False), ("LD_PRELOAD", False), ("PATH", False),
                                       ("PYTHONPATH", False), ("NODE_OPTIONS", False), ("CUA_DRIVER_PATH", False),
                                       ("export TERMINAL_ENV", False), ("https_proxy", False), ("bad name", False)])
def test_an_env_name_is_kept_only_when_it_says_nothing_of_programs(name, kept):
    assert profile_guard.env_kept(name) is kept


def test_a_backup_put_back_brings_no_code_back_and_keeps_the_bridge_s(tmp_path, monkeypatch):
    """A restore (or a planned move: both go through backup.settle) over a fresh place: a profile's code that a backup
    still holds (an import made before 27/11) stays out; the bridge's own browser server stays."""
    import shutil
    from types import SimpleNamespace
    from bipinstall import backup, moving
    from test_restore import make_home
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    coach = old / ".hermes" / "profiles" / "coach"
    traps(coach)
    cfg = yaml.safe_load((coach / "config.yaml").read_text())
    cfg["mcp_servers"]["stealth"] = {"command": "/usr/bin/podman", "args": ["exec", "-i", "bipagents-browser-coach",
                                                                                "/opt/stealth/mcp.sh"]}
    write(coach / "config.yaml", yaml.safe_dump(cfg))
    inv = moving.read_inventory(old / ".config" / "bipagents" / "bridge.toml", old)
    manifest = moving.manifest(inv, SimpleNamespace(system="linux", tailscale=None, tailnet_name=None))
    manifest.update(stage=str(old / backup.STAGE), volumes=[])
    fetched = tmp_path / "fetched"
    shutil.copytree(old, fetched / str(old).lstrip("/"), symlinks=True)
    write(fetched / str(old / backup.STAGE).lstrip("/") / "manifest.json", json.dumps(manifest))
    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(tmp_path / "repo"), "new-seat-token")
    backup.put_back(fetched, new, tmp_path / "checkout", "198.51.100.7")
    back = new / ".hermes" / "profiles" / "coach"
    assert (back / "memories" / "MEMORY.md").read_text() == "Sam a deux chats."
    for name in ("hooks", "plugins", "scripts", "shell-hooks-allowlist.json"):
        assert not (back / name).exists(), name
    after = yaml.safe_load((back / "config.yaml").read_text())
    assert set(after["mcp_servers"]) == {"stealth", "notes"} and "hooks" not in after


def test_an_untrusted_archive_makes_no_link_and_writes_nothing_through_one(tmp_path):
    """bring_in unpacks with untrusted=True: a link member is not made at all (a later write, the agent's memory, the
    bridge's files, would go through it); a folder already swapped for a link is never written through."""
    import io
    import os
    import tarfile
    from bipinstall import moving
    place = tmp_path / "place"
    scratch = place / ".local" / "state" / "bipagents" / "import" / "in"
    outside = tmp_path / "outside"
    outside.mkdir()
    (scratch / "hermes" / "profiles").mkdir(parents=True)
    os.symlink(outside, scratch / "hermes" / "profiles" / "old")
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, link, data in (("hermes/profiles/coach/memories", "/Users/sam/.config", None),
                                 ("hermes/profiles/coach/memories/MEMORY.md", None, b"x"),
                                 ("hermes/profiles/old/handler.py", None, b"x"),
                                 ("hermes/profiles/coach/SOUL.md", None, "Tu es Coach.".encode())):
            info = tarfile.TarInfo(name)
            if link:
                info.type, info.linkname = tarfile.SYMTYPE, link
                tar.addfile(info)
            else:
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
    buf.seek(0)
    info = {"roots": {"hermes": str(scratch / "hermes")}, "source": {"repo": "/Users/sam/b", "home": "/Users/sam"}}
    target = moving.Target(home=place, repo=tmp_path / "repo", tailnet_name=None, lan_address=None)
    moving.restore(tarfile.open(fileobj=buf, mode="r|"), info, target, untrusted=True)
    coach = scratch / "hermes" / "profiles" / "coach"
    assert (coach / "SOUL.md").read_text() == "Tu es Coach."
    assert not (coach / "memories").is_symlink() and (coach / "memories" / "MEMORY.md").read_text() == "x"
    assert list(outside.iterdir()) == []


def test_goals_keep_their_text_but_not_their_gates(tmp_path):
    """A goal's quality gates are shell commands Hermes runs on the host after a messaging turn (an agent's e-mail on
    a place): emptied in an imported profile's state.db (and a restored one's)."""
    import sqlite3
    folder = tmp_path / "coach"
    folder.mkdir()
    db = sqlite3.connect(folder / "state.db")
    db.execute("CREATE TABLE state_meta (key TEXT PRIMARY KEY, value TEXT)")
    db.execute("INSERT INTO state_meta VALUES (?, ?)", ("goal:s1", json.dumps({"goal": "Finir", "gates": [
        {"command": "id > /tmp/temoin"}]})))
    db.execute("INSERT INTO state_meta VALUES ('other', 'x')")
    db.commit()
    db.close()
    assert "goal_gates" in profile_guard.foreign(folder)
    rows = dict(sqlite3.connect(folder / "state.db").execute("SELECT key, value FROM state_meta").fetchall())
    assert json.loads(rows["goal:s1"]) == {"goal": "Finir", "gates": []} and rows["other"] == "x"
    assert profile_guard.foreign(folder) == []


def test_a_routine_list_with_a_bom_loses_its_scripts_too(tmp_path):
    folder = tmp_path / "coach"
    write(folder / "cron" / "jobs.json", "﻿" + json.dumps({"jobs": [{"name": "w", "script": "x.sh", "no_agent": True}]}))
    assert profile_guard.foreign(folder) == ["routine:w"]
    assert json.loads((folder / "cron" / "jobs.json").read_text())["jobs"] == []


def test_a_latin1_env_neither_breaks_a_send_nor_comes_in(tmp_path):
    hub = tmp_path / "sam" / ".hermes"
    write(hub / "config.yaml", "model:\n  default: claude\n")
    (hub / ".env").write_bytes("NOTE=caf\xe9\nOPENAI_API_KEY=k\n".encode("latin-1"))
    assert to_place.stays_here(to_place.LoneHermes(hub, tmp_path / "sam")) is None
    profile_guard.foreign(hub)
    assert (hub / ".env").read_text() == "OPENAI_API_KEY=k\n"


def test_a_name_with_u007f_keeps_the_place_s_config_valid(tmp_path):
    import tomllib
    imported = ('[hermes]\nhub_url = "http://127.0.0.1:8672"\n[agents.coach]\ndisplay_name = "Co\\u007fach"\n'
                'hermes_url = "http://127.0.0.1:8672/p/coach"\nhermes_key = "c"\n[pronunciations.fr]\n"K\\u007f" = "k"\n')
    merged = to_place.merged_config('bridge_key = "k"\n', imported, ["coach"], None, None, "http://127.0.0.1:8672",
                                    None, {}, tmp_path / "profiles")
    data = tomllib.loads(merged)
    assert data["agents"]["coach"]["display_name"] == "Co\x7fach" and data["pronunciations"]["fr"] == {"K\x7f": "k"}


def test_a_restore_keeps_the_sandbox_settings_as_hermes_writes_them(tmp_path):
    """Hermes writes the bridge's `config set terminal.docker_…` into .env quoted, inner quotes escaped
    (hermes_cli/config.py _quote_env_value): read back as Hermes reads it (_parse_env_value), they stay."""
    hub = tmp_path / ".hermes"
    coach = hub / "profiles" / "coach"
    text = ("API_SERVER_KEY=k\nTERMINAL_ENV=docker\n"
            'TERMINAL_DOCKER_EXTRA_ARGS="[\\"--network=bipagents-sandbox\\", \\"--dns=127.0.0.1\\"]"\n'
            'TERMINAL_DOCKER_VOLUMES="[\\"bipagents-files-coach:/fichiers:ro\\"]"\n'
            'TERMINAL_DOCKER_ENV="{\\"HTTP_PROXY\\": \\"http://10.233.80.2:3128\\"}"\n'
            "TERMINAL_DOCKER_NETWORK=True\nTERMINAL_LIFETIME_SECONDS=300\n")
    write(coach / ".env", text, 0o600)
    assert profile_guard.own_code(hub) == {}
    assert (coach / ".env").read_text() == text
    write(coach / ".env", text.replace("--dns=127.0.0.1", "--privileged"), 0o600)
    assert profile_guard.own_code(hub) == {str(coach): ["sandbox"]}
    assert "TERMINAL_DOCKER_EXTRA_ARGS" not in (coach / ".env").read_text()
