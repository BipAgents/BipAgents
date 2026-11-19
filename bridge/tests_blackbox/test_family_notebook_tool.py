"""The family agent writing in the family Carnet with the ids `family_notebook_read` gave it (filming 18/11: every
`family_notebook_write` answered « no such list » until Hermes' loop brake). Made from the app (the Carnet's three
lists seeded, in English), called over MCP the way the agent's Hermes calls it."""
import re

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_agents_admin import managed

TOKEN = "agent-token-for-family-0123456789ab"


def family(bridges, fakes):
    bridge, _ = managed(bridges, fakes, isolated=True)
    made = bridge.post("/v1/family-agent", headers=AUTH, json={"name": "Family", "lang": "en"})
    assert made.status_code == 201, made.text
    agent = made.json()["id"]
    give_token(bridge, agent, TOKEN)
    return bridge


def list_ids(bridge):
    read = call(bridge, "family_notebook_read", {}, token=TOKEN)
    assert read["isError"] is False, read
    ids = re.findall(r"list id (\w+)\)", text_of(read))
    assert len(ids) == 3, text_of(read)
    return text_of(read), ids


def items(bridge):
    return [[i["text"] for i in x["items"]] for x in bridge.get("/v1/family-agent/notebook", headers=AUTH).json()["lists"]]


def test_the_agent_writes_with_the_ids_it_read(bridges, fakes):
    bridge = family(bridges, fakes)
    _, (shopping, tasks, dates) = list_ids(bridge)
    wrote = call(bridge, "family_notebook_write", {"op": "item_add", "list": shopping, "text": "Milk"}, token=TOKEN)
    assert wrote["isError"] is False, text_of(wrote)
    wrote = call(bridge, "family_notebook_write", {"op": "item_add", "list": tasks, "text": "Call the plumber",
                                                   "due": "2026-11-20"}, token=TOKEN)
    assert wrote["isError"] is False, text_of(wrote)
    assert items(bridge) == [["Milk"], ["Call the plumber"], []]


def test_the_list_named_the_ways_a_model_names_it(bridges, fakes):
    """The read says « list id l… »: a model sends `list_id` (or `listId`, or `id` like the list ops, or the list's
    title); each lands in the list instead of « no such list » again and again."""
    bridge = family(bridges, fakes)
    text, (shopping, tasks, dates) = list_ids(bridge)
    assert "## Shopping (list, list id " in text
    for arguments in [
        {"op": "item_add", "list_id": shopping, "text": "Eggs"},
        {"op": "item_add", "listId": shopping, "text": "Bread"},
        {"op": "item_add", "id": shopping, "text": "Butter"},
        {"op": "item_add", "list": "Shopping", "text": "Apples"},
        {"op": "item_add", "list": f"list id {shopping}", "text": "Pears"},
    ]:
        wrote = call(bridge, "family_notebook_write", arguments, token=TOKEN)
        assert wrote["isError"] is False, (arguments, text_of(wrote))
    assert items(bridge)[0] == ["Eggs", "Bread", "Butter", "Apples", "Pears"]
    # an item op names its item by `id` and its list by `list` (or by the item alone: its list is found)
    view = bridge.get("/v1/family-agent/notebook", headers=AUTH).json()
    eggs = view["lists"][0]["items"][0]["id"]
    ticked = call(bridge, "family_notebook_write", {"op": "item_save", "id": eggs, "done": True}, token=TOKEN)
    assert ticked["isError"] is False, text_of(ticked)
    ticked = call(bridge, "family_notebook_write", {"op": "item_save", "list_id": shopping, "item_id": eggs,
                                                    "done": False}, token=TOKEN)
    assert ticked["isError"] is False, text_of(ticked)
    # still refused when there is truly no such list, and the answer says where the ids are
    wrong = call(bridge, "family_notebook_write", {"op": "item_add", "list": "l00000000", "text": "x"}, token=TOKEN)
    assert wrong["isError"] is True and "family_notebook_read" in text_of(wrong), text_of(wrong)


def test_a_page_the_family_agent_makes_opens_for_its_creator(bridges, fakes):
    """Filming 18/11: the family agent's page showed « File unavailable » in the family conversation (one place, the
    creator's own phone)."""
    from bbkit.mcp import media_of

    bridge = family(bridges, fakes)
    made = call(bridge, "create_page", {"html": "<h1>Packing list</h1><ul><li>Tent</li></ul>", "title": "Packing list"},
                token=TOKEN)
    assert made["isError"] is False, text_of(made)
    path = media_of(made)
    agent = path.split("/")[3]
    for params in ({"path": path, "agent": agent}, {"path": path}):
        shown = bridge.get("/v1/media", headers=AUTH, params=params)
        assert shown.status_code == 200 and "Packing list" in shown.text, (params, shown.status_code, shown.text[:300])
