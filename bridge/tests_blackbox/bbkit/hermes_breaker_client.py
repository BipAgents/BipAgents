"""Hermes' own MCP code (tools/mcp_tool*: its registry, handlers and circuit breaker) calling one of the bridge's MCP
servers, as an agent's Hermes does, and what Hermes then reports for each call: what the model gets, `failed` (the
`error` of its run event `tool.completed`, agent/display.py `_detect_tool_failure`) and its breaker's count for that
server.

    python -I hermes_breaker_client.py <url> <token> [<server name>, default bipagents]

stdin: one call a line `{"name": <tool>, "arguments": {…}}`; stdout: `READY <tools>`, then for each call
`RESULT {"result": <what the model gets>, "failed": <bool>, "strikes": <int>}`. Run by test_bridge_tools_no_breaker.py
and test_review1_no_breaker.py with HERMES_SRC=<a Hermes checkout>, HERMES_HOME=<an empty folder> and
MCP_SDK_PYTHON=<a Python with that Hermes' dependencies>."""
import json
import logging
import os
import sys

sys.path.insert(0, os.environ["HERMES_SRC"])
url, token = sys.argv[1], sys.argv[2]
server = sys.argv[3] if len(sys.argv) > 3 else "bipagents"
logging.basicConfig(level=logging.WARNING, stream=sys.stderr)

from agent.display import _detect_tool_failure  # noqa: E402
from tools.mcp_tool_common import _core as core  # noqa: E402
from tools.mcp_tool_discovery import register_mcp_servers  # noqa: E402
from tools.mcp_tool_scope import _resolve_server_key  # noqa: E402
from tools.registry import registry  # noqa: E402

register_mcp_servers({server: {"url": url, "headers": {"Authorization": f"Bearer {token}"},
                               "timeout": 60, "connect_timeout": 30}})
print("READY", len([n for n in registry.get_all_tool_names() if n.startswith(f"mcp__{server}__")]), flush=True)
for line in sys.stdin:
    if not line.strip():
        continue
    call = json.loads(line)
    name = f"mcp__{server}__" + call["name"]
    result = registry.dispatch(name, call.get("arguments") or {})
    text = result if isinstance(result, str) else json.dumps(result, ensure_ascii=False)
    failed, _ = _detect_tool_failure(name, text)
    strikes = core._server_error_counts.get(_resolve_server_key(server), 0)
    print("RESULT", json.dumps({"result": text, "failed": bool(failed), "strikes": strikes}, ensure_ascii=False),
          flush=True)
os._exit(0)
