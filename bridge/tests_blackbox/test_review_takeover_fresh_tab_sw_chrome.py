"""Review of fix-takeover-not-fresh (18/11): the fallback (`Page::fresh_tab`) reopens the scripted page's address in a
fresh tab but never removes the site's service workers, as `Page::fresh` does before its reload. A service worker the
agent's script registered then answers the reopened page and plants its script under the person's hand.
Red on 6775bf48a (fallback path), green on the reload path (control)."""
import http.server
import shutil
import subprocess
import tempfile
import threading
import time

import httpx
import pytest

from bbkit.servers import free_port, wait_until
from test_card_guard_hotfix_takeover_chrome import loader, page, scripted, take
from test_card_payments_chrome import CHROME, tab as is_tab
from test_desktop_page import browser_bridge
from test_takeover_never_locked_chrome import run_in_page

pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")

SW = """
self.addEventListener('install', e => self.skipWaiting());
self.addEventListener('activate', e => e.waitUntil(self.clients.claim()));
self.addEventListener('fetch', e => {
  if (e.request.mode !== 'navigate') return;
  e.respondWith(fetch(e.request).then(r => r.text()).then(t => new Response(
    t + '<script>window.__planted = 1</script>', {headers: {'Content-Type': 'text/html; charset=utf-8'}})));
});
"""
PAGES = {
    "/plain.html": ("text/html", "<!doctype html><title>Plain</title><p>plain</p>"),
    "/hang.html": ("text/html", "<!doctype html><title>Hang</title><p>hang</p>"),
    "/sw.js": ("text/javascript", SW),
}


class Shop(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        item = PAGES.get(self.path.split("?")[0])
        if item is None:
            self.send_response(404)
            self.end_headers()
            return
        kind, body = item
        data = body.encode()
        self.send_response(200)
        self.send_header("Content-Type", f"{kind}; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

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
                             "--no-first-run", "--no-default-browser-check", shop + "/plain.html"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert wait_until(lambda: page(port, "document.readyState") == "complete", timeout=30)
    yield port
    proc.terminate()
    proc.wait(timeout=10)
    shutil.rmtree(profile, ignore_errors=True)


def tabs(port):
    return [t for t in httpx.get(f"http://127.0.0.1:{port}/json/list", timeout=5).json() if is_tab(t)]


def planted_sw(port, shop):
    # the agent's execute_script registers a service worker of the site's origin
    page(port, "navigator.serviceWorker.register('/sw.js'); 1")
    assert wait_until(lambda: page(port, "!!navigator.serviceWorker.controller") is True, timeout=15)
    page(port, f"location.href = {shop + '/hang.html'!r}; 1")
    assert wait_until(lambda: page(port, "window.__planted") == 1, timeout=15), "the worker answers the page"


def test_control_reload_path_removes_the_agents_service_worker(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    planted_sw(chromium, shop)
    scripted(folder, shop, doc=loader(chromium))
    answer = take(bridge, True)
    assert answer.status_code == 200, answer.text
    assert wait_until(lambda: page(chromium, "document.readyState") == "complete", timeout=15)
    assert page(chromium, "window.__planted === undefined") is True
    take(bridge, False)


def test_fallback_fresh_tab_removes_the_agents_service_worker_too(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    planted_sw(chromium, shop)
    scripted(folder, shop, doc=loader(chromium))
    old = {t["id"] for t in tabs(chromium)}
    run_in_page(chromium, "/hang.html", "while (true) {}")
    answer = take(bridge, True)
    assert answer.status_code == 200, answer.text
    assert wait_until(lambda: not ({t["id"] for t in tabs(chromium)} & old), timeout=15), "fallback taken"
    assert wait_until(lambda: page(chromium, "document.readyState") == "complete", timeout=15)
    time.sleep(1)
    # what the agent's worker planted must not be under the person's hand
    assert page(chromium, "window.__planted === undefined") is True, "the agent's service worker answered the page"
    take(bridge, False)
