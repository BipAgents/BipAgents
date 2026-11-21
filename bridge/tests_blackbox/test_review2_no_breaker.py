"""Second review of bridge-tools-no-breaker (1b95a88ce..d00246c8d, 14/11): each finding as a failing test."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of

BRAKE = "you asked for exactly this"


def test_the_person_shares_her_reminders_and_the_same_call_goes_through(bridge):
    """reminders_add refused twice (« hasn't shared their reminders… turn on the « Rappels » tool on your page in the
    BipAgents app »), the person turns it on, the agent asks again with the same words: the call reaches the tool, not
    the brake's « you asked for exactly this 2 times »."""
    give_token(bridge)
    note = {"title": "Appeler Paul", "due": "2026-11-20T14:00"}
    first, second = (text_of(call(bridge, "reminders_add", note)) for _ in range(2))
    assert "hasn't shared their reminders" in first and "hasn't shared their reminders" in second, (first, second)
    shared = bridge.put("/v1/reminders/agents", json={"agents": ["wellness"]}, headers=AUTH)
    assert shared.status_code == 200, shared.text
    assert bridge.put("/v1/reminders/android", json={"calendar": True, "items": []}, headers=AUTH).status_code == 200
    third = text_of(call(bridge, "reminders_add", note))
    assert BRAKE not in third, third
