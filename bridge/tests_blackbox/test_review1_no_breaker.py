"""First review of bridge-tools-no-breaker (9050256d8, 14/11): each finding as a failing test. Hermes' own MCP client
where it matters (HERMES_SRC, MCP_SDK_PYTHON: test_bridge_tools_no_breaker.py `through_hermes`), skipped without."""
import threading
import time

from bbkit.common import AUTH, read_sse, sse
from bbkit.mcp import TOKEN, call, give_token, not_done, text_of
from test_bridge_tools_no_breaker import hermes, through_hermes

# --- 1, 7. A connector whose tools declare an outputSchema (FastMCP does for every typed tool) -------------------------

SCHEMA_SERVER = r'''
import json, sys
SCHEMA = {"type": "object", "properties": {"result": {"type": "string"}}, "required": ["result"]}
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "garmin"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "push_workouts", "description": "Pushes workouts",
                             "inputSchema": {"type": "object", "properties": {"day": {"type": "string"}}},
                             "outputSchema": SCHEMA}]}
    elif msg["params"]["arguments"].get("day") == "fail":
        result = {"content": [{"type": "text", "text": "Garmin did not answer"}], "isError": True}
    elif msg["params"]["arguments"].get("day") == "mute":
        result = {"content": [], "isError": True}
    else:
        result = {"content": [{"type": "text", "text": json.dumps({"result": "pushed"})}],
                  "structuredContent": {"result": "pushed"}}
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
'''


def schema_connector(bridges, fakes):
    from test_connectors import EMAIL, PASSWORD, REGISTER, approve_next, ask, setup
    bridge, hub, work = setup(bridges, fakes)
    (work / "server.py").write_text(SCHEMA_SERVER)
    ask(bridge, {"name": "garmin", "site": "connect.garmin.example", "kind": "password", "reason": "r"},
        {"value": PASSWORD, "identifier": EMAIL})
    approve_next(bridge, "connector_register", REGISTER)
    conn = bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"][0]
    return bridge, conn


def connector_entry(bridge, conn):
    return bridge.url + f"/v1/agent-tools/connector/{conn['id']}"


def test_a_connectors_tools_are_listed_without_their_output_schema(bridges, fakes):
    from test_connectors import TOKEN as VIE_TOKEN
    bridge, conn = schema_connector(bridges, fakes)
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}
    listed = bridge.post(f"/v1/agent-tools/connector/{conn['id']}", json=body,
                         headers={"Authorization": f"Bearer {VIE_TOKEN}"}).json()["result"]["tools"]
    assert [t["name"] for t in listed] == ["push_workouts"] and "outputSchema" not in listed[0], listed


@hermes
def test_a_connector_refusal_reaches_the_model_and_is_no_strike(bridges, fakes, tmp_path):
    """The connector switched off: the bridge's refusal, plain, on a tool that declares an outputSchema."""
    from test_connectors import TOKEN as VIE_TOKEN
    bridge, conn = schema_connector(bridges, fakes)
    switch_off = lambda: bridge.patch(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH, json={"enabled": False})
    ok, off = through_hermes(tmp_path, connector_entry(bridge, conn), VIE_TOKEN,
                             [("push_workouts", {"day": "lundi"}), switch_off, ("push_workouts", {"day": "lundi"})],
                             server="bipconn_garmin")
    assert "pushed" in ok["result"] and ok["strikes"] == 0, ok
    assert "turned this connector off" in off["result"] and off["strikes"] == 0, off


@hermes
def test_a_connectors_failures_never_pause_its_entry(bridges, fakes, tmp_path):
    """The connector's own failures, four in a row (one without any text): never three strikes for Hermes, and the
    third, sent plain, still says it failed."""
    from test_connectors import TOKEN as VIE_TOKEN
    bridge, conn = schema_connector(bridges, fakes)
    days = ["fail", "mute", "fail", "fail"]
    out = through_hermes(tmp_path, connector_entry(bridge, conn), VIE_TOKEN,
                         [("push_workouts", {"day": d}) for d in days], server="bipconn_garmin")
    assert not any("Paused for" in o["result"] or "unreachable" in o["result"] for o in out), out
    assert [o["strikes"] for o in out] == [1, 2, 0, 1], out
    assert "Not done:" in out[2]["result"] and "failed" in out[2]["result"], out[2]


# --- 3. No ID number by a page's name, an alias or a contact ---------------------------------------------------------

def test_no_id_number_reaches_the_carnet_by_a_page_name_an_alias_or_a_contact(bridge):
    give_token(bridge)
    for arguments in [
        {"page": "Passeport de Léa 12AB34567", "kind": "person", "facts": ["Valable jusqu'en 2031"]},
        {"page": "Léa Martin", "kind": "person", "facts": ["Aime le jazz"], "aliases": ["IBAN FR76 3000 6000 0112 3456 7890 189"]},
        {"page": "Léa Martin", "kind": "person", "facts": ["Aime le jazz"], "contact": "Carte bancaire 4970 1012 3456 7890"},
    ]:
        assert not_done(call(bridge, "notebook_write", arguments)), arguments
    held = [str(p.relative_to(bridge.state_dir)) for p in (bridge.state_dir / "notebook").rglob("*")
            if p.is_file() and any(n in p.read_text(errors="ignore") or n in p.name
                                   for n in ("12AB34567", "0112 3456 7890 189", "4970 1012 3456 7890"))]
    assert held == [], f"an ID number kept in the Carnet: {held}"


# --- 4. The person's own « Modifier » ----------------------------------------------------------------------------------

def test_the_person_keeps_an_order_number_a_book_and_what_was_already_there(bridge):
    """Her edit of a section: an order and a book are no ID numbers; a line already on the page (an agent's, written
    before this update) is never judged again; only a new ID number is refused."""
    made = bridge.post("/v1/notebook/page", json={"kind": "subject", "title": "Achats"}, headers=AUTH)
    assert made.status_code == 201, made.text
    path, rev = made.json()["path"], made.json()["rev"]

    def edit(text):
        nonlocal rev
        saved = bridge.put("/v1/notebook/section", headers=AUTH,
                           json={"path": path, "section": "status", "text": text, "rev": rev})
        if saved.status_code == 200:
            rev = saved.json()["rev"]
        return saved

    for line in ["- Commande Amazon 402-9384756-1029384, livrée jeudi", "- Livre pour Léa : ISBN 978-2-07-036822-8"]:
        assert edit(line).status_code == 200, line
    # a line an older bridge kept (its filter let it through), then the person adds one beside it
    page = next((bridge.state_dir / "notebook").rglob("Achats.md"))
    page.write_text(page.read_text().replace("- Livre pour Léa", "- Ancien n° client 1234567890123 — *Vie, 2026-11-01*\n- Livre pour Léa"))
    rev = bridge.get("/v1/notebook/page", headers=AUTH, params={"path": path}).json()["rev"]
    kept = edit("- Ancien n° client 1234567890123\n- Livre pour Léa : ISBN 978-2-07-036822-8\n- Commande livrée")
    assert kept.status_code == 200, kept.text
    refused = edit("- Commande livrée\n- Passeport : 12AB34567")
    assert refused.status_code == 422 and refused.json()["detail"]["code"] == "number_refused", refused.text


# --- 6. The same refused write again and again ------------------------------------------------------------------------

def test_the_same_refused_write_again_and_again_is_stopped_early(bridge):
    """A routine's (no run followed: the task's limits). Before the branch Hermes' breaker cut it after 3, pausing
    everything; now the brake stops the very same refused write at its third ask."""
    give_token(bridge)
    note = {"page": "Camille Testeur", "kind": "person", "facts": ["Passeport : 12AB34567"]}
    answers = [text_of(call(bridge, "notebook_write", note)) for _ in range(8)]
    assert [a.startswith("Not saved: ID numbers") for a in answers] == [True, True] + [False] * 6, answers
    assert all(a.startswith("Not done: you asked for exactly this") for a in answers[2:]), answers[2]


# --- 5. The apps see a refused call as a step not done -----------------------------------------------------------------

def test_a_refused_call_reaches_the_apps_as_a_failed_step(bridges, fakes):
    """Hermes' `tool.completed` for a call the bridge refused says `error: false` now: the bridge, which refused it,
    relays it failed (the apps draw a step not done, never ✓)."""
    bridge = bridges.new().start()
    give_token(bridge)
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "Note le passeport de Camille"}])
    gate = threading.Event()
    fakes.hermes.runs["run_ref"] = [
        sse("tool.started", run_id="run_ref", tool="mcp_bipagents_notebook_write", preview=""), gate,
        sse("tool.completed", run_id="run_ref", tool="mcp_bipagents_notebook_write", error=False, preview=""),
        sse("tool.started", run_id="run_ref", tool="mcp_bipagents_notebook_search", preview=""),
        sse("tool.completed", run_id="run_ref", tool="mcp_bipagents_notebook_search", error=False, preview=""),
        sse("run.completed", run_id="run_ref", output="Je ne peux pas noter ce numéro.")]
    events = []

    def follow():
        with bridge.stream("GET", "/v1/runs/run_ref/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
            events.extend(read_sse(resp))

    follower = threading.Thread(target=follow)
    follower.start()
    time.sleep(1.5)   # the run's first call started, as Hermes calls the bridge's tool
    refused = call(bridge, "notebook_write", {"page": "Camille Testeur", "kind": "person", "facts": ["Passeport : 12AB34567"]})
    assert not_done(refused), refused
    gate.set()
    follower.join(30)
    completed = [d for e, d in events if e == "tool.completed"]
    assert [(d["tool"], d["error"]) for d in completed] == [("mcp_bipagents_notebook_write", True),
                                                          ("mcp_bipagents_notebook_search", False)], completed
