"""Hermes' own MCP code (tools/mcp_tool*: its registry, handlers and circuit breaker) calling the bridge's tools, as an
agent's Hermes does: each line of stdin is a call `{"name": <bipagents tool>, "arguments": {…}}`, each answer a line
`RESULT <what Hermes hands the model>`. Run by test_bridge_tools_no_breaker.py with HERMES_SRC=<a Hermes checkout>,
HERMES_HOME=<an empty folder> and MCP_SDK_PYTHON=<a Python with that Hermes' dependencies>."""
import json
import logging
import os
import sys

sys.path.insert(0, os.environ["HERMES_SRC"])
url, token = sys.argv[1], sys.argv[2]
logging.basicConfig(level=logging.WARNING, stream=sys.stderr)

from tools.mcp_tool_discovery import register_mcp_servers  # noqa: E402
from tools.registry import registry  # noqa: E402

register_mcp_servers({"bipagents": {"url": url, "headers": {"Authorization": f"Bearer {token}"},
                                    "timeout": 60, "connect_timeout": 30}})
print("READY", len([n for n in registry.get_all_tool_names() if n.startswith("mcp__bipagents__")]), flush=True)
for line in sys.stdin:
    if not line.strip():
        continue
    call = json.loads(line)
    result = registry.dispatch("mcp__bipagents__" + call["name"], call.get("arguments") or {})
    print("RESULT", result if isinstance(result, str) else json.dumps(result, ensure_ascii=False), flush=True)
os._exit(0)
