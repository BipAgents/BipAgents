"""Small HTTP servers on 127.0.0.1 for the fakes: an ASGI app served by uvicorn in a thread of its own, and a TCP
server that accepts and drops every connection (an unreachable Hermes)."""
from __future__ import annotations

import asyncio
import hashlib
import json
import os
import socket
import struct
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Dict, Optional
from urllib.parse import parse_qs

import uvicorn


# Under pytest-xdist (`-n N`, PYTEST_XDIST_WORKER=gw<i>), each worker hands out ports from a block of its own, below
# Linux's ephemeral range (32768+, where the kernel puts every `bind(0)` and the local end of every outgoing
# connection): a port probed free here, then bound by a bridge a moment later, is never taken in between by another
# worker's probe or by a connection of any process. The block comes from the run's id (PYTEST_XDIST_TESTRUNUID) plus
# the worker's number, so two parallel runs on one machine mostly use different blocks; the probe still skips any
# port in use. Serial runs (no xdist) keep `bind(0)`, as before.
_PORT_LOW, _PORT_HIGH, _PORT_BLOCK = 12000, 32000, 500
_port_next: Dict[str, int] = {}
_port_lock = threading.Lock()


def _worker_block() -> Optional[range]:
    worker = os.environ.get("PYTEST_XDIST_WORKER", "")
    if not worker.startswith("gw") or not worker[2:].isdigit():
        return None
    blocks = (_PORT_HIGH - _PORT_LOW) // _PORT_BLOCK
    run = int(hashlib.sha256(os.environ.get("PYTEST_XDIST_TESTRUNUID", "").encode()).hexdigest()[:8], 16)
    first = _PORT_LOW + ((run + int(worker[2:])) % blocks) * _PORT_BLOCK
    return range(first, first + _PORT_BLOCK)


def _bindable(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        try:
            probe.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


def free_port() -> int:
    block = _worker_block()
    if block is None:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.bind(("127.0.0.1", 0))
            return probe.getsockname()[1]
    with _port_lock:   # in turn through the block: a port comes back only after the block's others
        at = _port_next.get("at", block.start)
        for step in range(len(block)):
            port = block.start + (at - block.start + step) % len(block)
            if _bindable(port):
                _port_next["at"] = port + 1
                return port
    raise RuntimeError(f"no free port in {block.start}-{block.stop - 1}")


async def wait_flag(flag: threading.Event, timeout: float = 30.0) -> None:
    """Waits for a flag set from another thread, without blocking the server's loop."""
    deadline = time.monotonic() + timeout
    while not flag.is_set() and time.monotonic() < deadline:
        await asyncio.sleep(0.01)


def wait_until(predicate, timeout: float = 10.0, interval: float = 0.05) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - a predicate reading state that is not there yet
            pass
        time.sleep(interval)
    return bool(predicate())


@dataclass
class Seen:
    """A request a fake received."""
    method: str
    path: str
    query: str
    headers: Dict[str, str]
    body: bytes
    at: float = field(default_factory=time.time)

    @property
    def params(self) -> Dict[str, str]:
        return {k: v[-1] for k, v in parse_qs(self.query, keep_blank_values=True).items()}

    def json(self) -> Any:
        return json.loads(self.body) if self.body else None

    def header(self, name: str) -> Optional[str]:
        return self.headers.get(name.lower())

    @property
    def url(self) -> str:
        return self.path + (f"?{self.query}" if self.query else "")


class Served:
    """An ASGI app on 127.0.0.1:<port>, in a thread with its own event loop."""

    def __init__(self, app: Any, port: Optional[int] = None):
        self.port = port or free_port()
        self.config = uvicorn.Config(app, host="127.0.0.1", port=self.port, log_level="warning", access_log=False,
                                     lifespan="off", timeout_graceful_shutdown=1, ws="none", log_config=None)
        self.server = uvicorn.Server(self.config)
        self.thread = threading.Thread(target=self.server.run, name=f"fake-{self.port}", daemon=True)

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def start(self) -> "Served":
        self.thread.start()
        deadline = time.monotonic() + 10
        while not self.server.started and time.monotonic() < deadline:
            time.sleep(0.01)
        if not self.server.started:
            raise RuntimeError(f"fake server on port {self.port} did not start")
        return self

    def stop(self) -> None:
        self.server.should_exit = True
        self.thread.join(timeout=5)


class Dropper:
    """Accepts each connection and closes it at once, counting them: what an HTTP client sees of a Hermes that is
    down (the connection fails before any answer)."""

    def __init__(self) -> None:
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(64)
        self.port = self.sock.getsockname()[1]
        self.connections = 0
        self._stop = False
        self.thread = threading.Thread(target=self._loop, name="dropper", daemon=True)

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def start(self) -> "Dropper":
        self.thread.start()
        return self

    def reset(self) -> None:
        self.connections = 0

    def _loop(self) -> None:
        self.sock.settimeout(0.2)
        while not self._stop:
            try:
                conn, _ = self.sock.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self.connections += 1
            try:
                conn.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            except OSError:
                pass
            conn.close()

    def stop(self) -> None:
        self._stop = True
        self.sock.close()
