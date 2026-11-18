"""18/11 (Alex's aibox, agent Vie): « Prendre la main » answered 409 « page_not_fresh » again and again, the bridge's
log saying only « the page a script ran on could not be reloaded ». The page a script of the agent's ran on is made
fresh before the person's hand (hotfix of 15/11), but a page that can't be reloaded must never lock the person out:
a page that keeps navigating (a script's redirect loop, a form the page posts by itself again and again), one with a
dialog open (alert), one whose script never yields. When the reload can't be done, the tab's address is opened again
in a fresh tab (a GET) and every older tab closed: no document a script ran on is left under the person's hand.
Every path that fails says why in the log.

Also through the container's DevTools relay (browser/devtools_relay.py), as on a place: the bridge's connections show
the person's hold token (`X-Bip-Hold`) and its secret.

A real Chromium (`BIP_CHROME`); the bridge on its DevTools, directly or through the relay."""
import http.server
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from test_card_guard_hotfix_takeover_chrome import loader, page, scripted, take
from test_card_payments_chrome import CHROME, tab as is_tab
from test_desktop_page import browser_bridge

pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")
RELAY = Path(__file__).resolve().parents[2] / "browser"

PAGES = {
    # a script's redirect loop between two pages (Alex's « la page boucle »)
    "/loop-a.html": "<title>A</title><p>A</p><script>setTimeout(() => location.replace('/loop-b.html?' + Date.now()), 80)</script>",
    "/loop-b.html": "<title>B</title><p>B</p><script>setTimeout(() => location.replace('/loop-a.html?' + Date.now()), 80)</script>",
    # a page that posts its form by itself on load, and the answer is the same page (an SSO form_post, a 3-D Secure
    # step, a « continue » page sent again and again)
    "/autopost.html": "<title>Post</title><form id=f method=post action=/autopost.html><input name=x value=1></form>"
                      "<script>setTimeout(() => document.getElementById('f').submit(), 80)</script>",
    # a dialog left open
    "/alert.html": "<title>Alert</title><p>alert</p>",
    # a script that never yields
    "/hang.html": "<title>Hang</title><p>hang</p>",
    "/plain.html": "<title>Plain</title><p>plain</p>",
}


class Shop(http.server.BaseHTTPRequestHandler):
    def answer(self):
        body = PAGES.get(self.path.split("?")[0])
        if body is None:
            self.send_response(404)
            self.end_headers()
            return
        data = ("<!doctype html><meta charset=utf-8>" + body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.answer()

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        self.answer()

    def log_message(self, *args):
        pass


@pytest.fixture
def shop():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Shop)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


@pytest.fixture
def chromium(shop):
    port = free_port()
    profile = tempfile.mkdtemp(prefix="bip-chrome-")
    proc = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
                             "--no-first-run", "--no-default-browser-check", "--window-size=1366,768",
                             shop + "/plain.html"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert wait_until(lambda: page(port, "document.readyState") == "complete", timeout=30)
    yield port
    proc.terminate()
    proc.wait(timeout=10)
    shutil.rmtree(profile, ignore_errors=True)


def tabs(port):
    return [t for t in httpx.get(f"http://127.0.0.1:{port}/json/list", timeout=5).json() if is_tab(t)]


def browser_call(port, method, params=None, url=None):
    """One DevTools command through the browser's endpoint, in the tab whose address holds `url` when given; sent
    without waiting for its answer when `url` is given (a dialog or a busy page may never answer)."""
    version = httpx.get(f"http://127.0.0.1:{port}/json/version", timeout=5).json()
    with connect(version["webSocketDebuggerUrl"], max_size=None) as ws:
        n = 0

        def send(method, params=None, session=None, wait=True):
            nonlocal n
            n += 1
            ws.send(json.dumps({"id": n, "method": method, "params": params or {},
                                **({"sessionId": session} if session else {})}))
            while wait:
                answer = json.loads(ws.recv(timeout=10))
                if answer.get("id") == n:
                    return answer.get("result", {})
            time.sleep(0.5)

        if url is None:
            return send(method, params)
        target = next(t for t in send("Target.getTargets")["targetInfos"] if t["type"] == "page" and url in t["url"])
        session = send("Target.attachToTarget", {"targetId": target["targetId"], "flatten": True})["sessionId"]
        return send(method, params, session, wait=False)


def go(port, url, wait=1.0):
    tab = tabs(port)[0]
    with connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
        ws.send(json.dumps({"id": 1, "method": "Page.navigate", "params": {"url": url}}))
        ws.recv(timeout=10)
    time.sleep(wait)


def run_in_page(port, url, expression):
    """`expression` run in the page's own world (as an agent's execute_script), not waiting for it to end."""
    browser_call(port, "Runtime.evaluate", {"expression": expression}, url=url)


def taken(bridge, port=None):
    answer = take(bridge, True)
    shown = tabs(port) if port else None
    assert answer.status_code == 200 and answer.json()["you_hold"] is True, (answer.text, shown)
    return answer


def test_a_page_that_keeps_navigating_is_taken_over(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    go(chromium, shop + "/loop-a.html")
    # an earlier proxy's line (no document: « unknown » site), and this very document's
    scripted(folder, "unknown")
    try:
        scripted(folder, shop, doc=loader(chromium))
    except Exception:   # noqa: BLE001 - the page moved on meanwhile
        pass
    taken(bridge)
    # the tab shows the loop's site again (the person's hand fixes the rest)
    assert wait_until(lambda: any("/loop-" in t["url"] for t in tabs(chromium)), timeout=10)
    take(bridge, False)


def test_a_page_that_posts_itself_again_and_again_is_taken_over(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    go(chromium, shop + "/autopost.html")
    scripted(folder, "unknown")
    taken(bridge)
    assert wait_until(lambda: any("/autopost.html" in t["url"] for t in tabs(chromium)), timeout=10)
    take(bridge, False)


def test_a_page_with_a_dialog_open_is_taken_over_and_its_document_is_gone(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    go(chromium, shop + "/alert.html")
    doc = loader(chromium)
    scripted(folder, shop, doc=doc)
    run_in_page(chromium, "/alert.html", "window.__reader = 1; alert('agent')")
    old = {t["id"] for t in tabs(chromium)}
    taken(bridge)
    # the document the script ran on is gone: its tab reloaded, or closed for a fresh one at the same address
    assert wait_until(lambda: any("/alert.html" in t["url"] for t in tabs(chromium)), timeout=10)
    assert wait_until(lambda: page(chromium, "window.__reader === undefined") is True, timeout=10)
    assert wait_until(lambda: loader(chromium) != doc, timeout=10) or {t["id"] for t in tabs(chromium)} != old
    take(bridge, False)


def test_a_page_whose_script_never_yields_is_taken_over_and_closed(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    go(chromium, shop + "/hang.html")
    scripted(folder, shop, doc=loader(chromium))
    old = {t["id"] for t in tabs(chromium)}
    run_in_page(chromium, "/hang.html", "window.__reader = 1; while (true) {}")
    print("before", tabs(chromium))
    started = time.monotonic()
    taken(bridge, chromium)
    assert time.monotonic() - started < 60
    # the busy tab closed, a fresh one at its address in its place
    assert wait_until(lambda: not ({t["id"] for t in tabs(chromium)} & old), timeout=15), tabs(chromium)
    assert any("/hang.html" in t["url"] for t in tabs(chromium))
    take(bridge, False)


# -- through the container's relay, as on a place ------------------------------------------------------------------

RELAY_MAIN = r"""
import asyncio, os, sys
sys.path.insert(0, sys.argv[1])
import devtools_relay, hold_gate
token_file, flag, chrome_port, port, secret = sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), sys.argv[6]
devtools_relay.CHROME = ("127.0.0.1", chrome_port)
devtools_relay.SECRET_FILE = secret
devtools_relay.HOLD = hold_gate.HOLD = flag

async def mirror():
    # what the bridge's `podman exec … printf 'human <token>' > /tmp/bipagents-hold` does in the container
    while True:
        try:
            with open(token_file) as fh:
                text = "human " + fh.read().strip()
            with open(flag, "w") as fh:
                fh.write(text)
        except OSError:
            try:
                os.remove(flag)
            except OSError:
                pass
        await asyncio.sleep(0.05)

async def main():
    server = await asyncio.start_server(devtools_relay.relay, "127.0.0.1", port)
    asyncio.create_task(devtools_relay.watch_hold())
    asyncio.create_task(mirror())
    async with server:
        await server.serve_forever()

asyncio.run(main())
"""


@pytest.fixture
def relayed(chromium, tmp_path):
    port = free_port()
    secret = "ab" * 32
    (tmp_path / "cdp.token").write_text(secret + "\n")
    folder_holder = {}

    def start(folder):
        script = tmp_path / "relay_main.py"
        script.write_text(RELAY_MAIN)
        proc = subprocess.Popen([sys.executable, "-I", str(script), str(RELAY), str(folder / "hold.token"),
                                 str(tmp_path / "flag"), str(chromium), str(port), str(tmp_path / "cdp.token")])
        folder_holder["proc"] = proc

        def up():
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return True
        assert wait_until(up, timeout=10)
        return port, secret

    yield start
    if "proc" in folder_holder:
        folder_holder["proc"].terminate()
        folder_holder["proc"].wait(10)


def test_through_the_relay_the_scripted_page_is_reloaded_under_the_persons_hold(bridges, fakes, chromium, shop,
                                                                               relayed, tmp_path):
    bridge = bridges.new()
    folder = bridge.data / "browsers" / "vie"
    folder.mkdir(parents=True)
    port, secret = relayed(folder)
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_vnc": f"tcp://127.0.0.1:{free_port()}"}
    (folder / "ports.json").write_text(json.dumps({"cdp": port, "vnc": free_port()}))
    (folder / "cdp.token").write_text(secret + "\n")
    os.chmod(folder / "cdp.token", 0o600)
    bridge.start()
    page(chromium, "window.__reader = 1")
    scripted(folder, shop, doc=loader(chromium))
    taken(bridge)
    assert page(chromium, "window.__reader") is None and page(chromium, "location.pathname") == "/plain.html"
    take(bridge, False)
    # a looping page through the relay too
    go(chromium, shop + "/loop-a.html")
    scripted(folder, "unknown")
    taken(bridge)
    take(bridge, False)


def test_a_slow_page_no_script_ran_on_keeps_its_tabs(bridges, fakes, chromium, shop):
    # review of 18/11, F4: a script ran once on another site's page; this page only answers slowly: its document can't
    # be read, but no script of the agent's ran on its site, so its tabs stay as they are
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    go(chromium, shop + "/hang.html")
    scripted(folder, "https://other.example", doc="0123456789ABCDEF0123456789ABCDEF")
    old = {t["id"] for t in tabs(chromium)}
    run_in_page(chromium, "/hang.html", "while (true) {}")
    taken(bridge, chromium)
    assert {t["id"] for t in tabs(chromium)} == old, "its tab kept"
    take(bridge, False)


def test_a_document_scripted_more_than_a_day_ago_is_not_reloaded_any_more(bridges, fakes, chromium, shop):
    # review of 18/11, F4: the documents scripts ran on are kept a day, not for ever
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    assert page(chromium, "window.__mark = 1") == 1
    scripted(folder, shop, ago=25 * 3600, doc=loader(chromium))
    taken(bridge, chromium)
    assert page(chromium, "window.__mark") == 1, "left as it is"
    take(bridge, False)
