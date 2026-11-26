"""An agent's shown name everywhere the others read it (26/11: « I'm writing a message to agent_a8… », an agent taken
from the spare agents keeps an id that says nothing): renamed, the other agents' contacts name it as it is now, not
as it was (peers.rs `write_contacts`, refreshed by agents_admin.rs `save_agent`). The apps word a call's target by
the agent's name (AgentNames.kt / HermesKit's AgentNames)."""
from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile


def test_a_renamed_agent_is_named_as_it_is_now_in_the_others_contacts(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    hub = managed_install(bridge, fakes, agents=("budget", "maison"))
    for name in ("budget", "maison"):
        (profile(hub, name) / "SOUL.md").write_text(f"Tu es {name.title()}.\n")
    bridge.start()
    links = [{"from": "budget", "to": "maison"}]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).status_code == 200
    contacts = hub / "profiles" / "budget" / "skills" / "agent-contacts" / "SKILL.md"
    assert "`bip/maison`: Maison" in contacts.read_text()
    renamed = bridge.patch("/v1/agents/maison/settings", headers=AUTH, json={"name": "Maison familiale"})
    assert renamed.status_code == 200, renamed.text
    assert wait_until(lambda: "`bip/maison`: Maison familiale" in contacts.read_text(), timeout=10), contacts.read_text()
