"""An agent's own browser turned off and on from the app (`PATCH /v1/agents/{id}/settings {"browser": …}`), and
removed with `bipbridge browser remove` (tests/test_browsers.py's routes; its container is the fake `podman`)."""
import json
import time

import yaml

from bbkit.common import AUTH, boite_items
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

STEALTH = ("mcp_servers:\n  stealth:\n    url: http://127.0.0.1:9601/mcp\n    enabled: true\n"
           "skills:\n  disabled:\n  - something-else\n")


def with_browser(bridges, fakes):
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(hub, "coach")
    (folder / "config.yaml").write_text(STEALTH)
    (bridge.data / "browsers" / "coach").mkdir(parents=True)
    (bridge.data / "browsers" / "coach" / "ports.json").write_text('{"devtools": 9600, "vnc": 9601}')
    bridge.start()
    wait_registered(bridge, ["coach"])
    return bridge, folder


def test_the_person_turns_the_agents_browser_off_and_on(bridges, fakes):
    bridge, folder = with_browser(bridges, fakes)
    assert bridge.get("/v1/agents/coach/settings", headers=AUTH).json()["browser"] == {"on": True, "ready": True}
    bridge.commands.clear()
    off = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"browser": False})
    assert off.status_code == 200 and off.json()["browser"] == {"on": False, "ready": True}
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["mcp_servers"]["stealth"]["enabled"] is False
    # Hermes' own powerpoint/docx skills stay off too (the styled decks and Word skills replace them)
    assert [s for s in data["skills"]["disabled"] if s not in ("powerpoint", "docx")] == ["something-else", "visible-browser"]
    assert bridge.commands.of("hermes") == [["-p", "coach", "tools", "disable", "browser", "--platform", "api_server"]]
    assert bridge.commands.of("systemctl") == [["--user", "stop", "bipagents-browser-coach"]]
    bridge.commands.clear()
    on = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"browser": True})
    assert on.json()["browser"] == {"on": True, "ready": True}
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["mcp_servers"]["stealth"]["enabled"] is True and [s for s in data["skills"]["disabled"] if s not in ("powerpoint", "docx")] == ["something-else"]
    assert bridge.commands.of("hermes") == [["-p", "coach", "tools", "enable", "browser", "--platform", "api_server"]]


def test_without_a_browser_and_none_to_give(bridges, fakes):
    bridge = bridges.new()
    managed_install(bridge, fakes, agents=("coach",))
    bridge.start()
    assert bridge.get("/v1/agents/coach/settings", headers=AUTH).json()["browser"] is None
    refused = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"browser": True})
    assert refused.status_code == 400 and refused.json() == {"detail": "this install has no browser for its agents"}


def test_the_browser_command_removes_one(bridges, fakes):
    bridge, folder = with_browser(bridges, fakes)
    bridge.stop()
    bridge.commands.clear()
    out = bridge.cli("browser", "remove", "--agent", "coach")
    assert out.returncode == 0, out.stderr
    assert [l for l in out.stdout.splitlines() if not l.startswith("{")] == ["coach: browser removed"]
    assert ["rm", "-f", "bipagents-browser-coach"] in bridge.commands.of("podman")
    assert not (bridge.data / "browsers" / "coach").exists()
    assert bridge.cli("browser").returncode == 2
    wrong = bridge.cli("browser", "remove", "--agent", "wellness")
    assert wrong.returncode == 1 and "wellness: not an agent this install manages" in wrong.stderr


def test_an_agent_with_its_browser_keeps_only_the_vault_of_hermes_browser_tools(bridges, fakes):
    """At the bridge's start, an agent whose visible browser is on gets `browser.vault_only` (hermes/patches 0007): it
    browses only with its `stealth` tools, in the browser « Bureau » shows; its skill is the bridge's."""
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(hub, "coach")
    (folder / "config.yaml").write_text(STEALTH)
    skill = folder / "skills" / "visible-browser" / "SKILL.md"
    skill.parent.mkdir(parents=True)
    skill.write_text("---\nname: visible-browser\nversion: 1.4.0\n---\nAn earlier skill.\n")
    (bridge.data / "browsers" / "coach").mkdir(parents=True)
    (bridge.data / "browsers" / "coach" / "ports.json").write_text('{"cdp": 9600, "vnc": 9601}')
    bridge.start()
    wait_registered(bridge, ["coach"])

    def vault_only():
        return (yaml.safe_load((folder / "config.yaml").read_text()).get("browser") or {}).get("vault_only")

    assert wait_until(lambda: vault_only() is True)
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["mcp_servers"]["stealth"]["enabled"] is True          # the rest as it was
    assert "`browser_vault`" in skill.read_text() and "An earlier skill" not in skill.read_text()


def test_a_bot_check_handed_to_the_person_reaches_them_once_per_site(bridges, fakes):
    """antibot-guard (22/11): when a bot check still blocks the agent's browser, bip-browser-mcp writes
    `challenge.json` beside its `busy` and waits for the person's hand; the bridge turns it into ONE Boîte line (with
    a push) per site, in the agent's language, and takes the file away."""
    bridge, _ = with_browser(bridges, fakes)
    note = bridge.data / "browsers" / "coach" / "challenge.json"
    note.write_text(json.dumps({"agent": "coach", "site": "decathlon.fr", "kind": "Cloudflare challenge",
                                "at": time.time()}))

    def lines():
        return [i for i in boite_items(bridge, "coach") if "decathlon.fr" in (i.get("text") or "")]

    assert wait_until(lambda: len(lines()) == 1, timeout=15)
    assert wait_until(lambda: not note.exists(), timeout=5)
    [line] = lines()
    assert line["text"] in (
        "Une vérification anti-robots bloque decathlon.fr. Prends la main dans le Bureau de Coach pour la passer, "
        "puis rends-la : Coach reprendra.",
        "A bot check is blocking decathlon.fr. Take over Coach's Desktop to pass it, then hand it back: Coach will "
        "carry on.",
    ), line
    # the same site again a moment later: no second line
    note.write_text(json.dumps({"agent": "coach", "site": "decathlon.fr", "kind": "Cloudflare challenge",
                                "at": time.time()}))
    assert wait_until(lambda: not note.exists(), timeout=10)
    time.sleep(1)
    assert len(lines()) == 1
