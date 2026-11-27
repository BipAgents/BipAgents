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
    cfg["mcp_servers"]["stealth"] = {"command": "/usr/bin/podman", "args": ["exec", "-i", "bip-browser-coach",
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
    assert after["browser"] == cfg["browser"] and after["terminal"] == cfg["terminal"]   # the place's settings stay
    assert (coach / ".env").read_text().startswith("API_SERVER_KEY=")                   # its .env untouched
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


@pytest.mark.parametrize("name,kept", [("API_SERVER_KEY", True), ("OPENROUTER_API_KEY", True), ("EMAIL_IMAP_HOST", True),
                                       ("TELEGRAM_BOT_TOKEN", True), ("TERMINAL_DOCKER_VOLUMES", False),
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
    cfg["mcp_servers"]["stealth"] = {"command": "/usr/bin/podman", "args": ["exec", "-i", "bip-browser-coach", "x"]}
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
