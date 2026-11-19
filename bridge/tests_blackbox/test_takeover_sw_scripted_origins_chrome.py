"""Follow-up of fix-takeover-not-fresh (review pass 2, residue): a service worker the agent's script registered on a
site no tab shows any more (the tab went elsewhere, the worker stopped) was removed by neither path (`Page::fresh`:
the tab's origin only; `Page::fresh_tab`: the origins Chrome lists). It could answer a later navigation to that site
under the person's hand. Before the person takes the browser, every site of the agent's scripts record (scripts.jsonl)
loses its service workers and cache storage, never its cookies nor localStorage (the person's logins stay).
Red on 92a48da97."""
import time

import pytest

from bbkit.servers import wait_until
from test_card_guard_hotfix_takeover_chrome import loader, page, scripted, take
from test_card_payments_chrome import CHROME
from test_desktop_page import browser_bridge
from test_review_takeover_fresh_tab_sw_chrome import chromium, shop  # noqa: F401 (fixtures)

pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")


def other_site(shop):
    # the same test server under another origin
    return shop.replace("127.0.0.1", "localhost")


def test_a_worker_on_a_site_no_tab_shows_is_removed_before_the_person_takes_the_browser(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    b = other_site(shop)
    # the agent's script on site B: a service worker registered, the person's own cookie and localStorage there
    page(chromium, f"location.href = {b + '/plain.html'!r}; 1")
    assert wait_until(lambda: page(chromium, "location.origin") == b and
                      page(chromium, "document.readyState") == "complete", timeout=15)
    page(chromium, "document.cookie = 'login=kept; max-age=3600'; localStorage.setItem('login', 'kept'); "
                   "navigator.serviceWorker.register('/sw.js'); 1")
    assert wait_until(lambda: page(chromium, "!!navigator.serviceWorker.controller") is True, timeout=15)
    page(chromium, "caches.open('agent').then(c => c.put('/x', new Response('x'))); 1")
    scripted(folder, b, doc=loader(chromium))
    # then the tab goes to site A: B's worker is shown nowhere
    page(chromium, f"location.href = {shop + '/plain.html'!r}; 1")
    assert wait_until(lambda: page(chromium, "location.origin") == shop and
                      page(chromium, "document.readyState") == "complete", timeout=15)
    answer = take(bridge, True)
    assert answer.status_code == 200, answer.text
    # the person goes to B: no worker of the agent's answers, its caches gone, the person's login there kept
    page(chromium, f"location.href = {b + '/hang.html'!r}; 1")
    assert wait_until(lambda: page(chromium, "location.origin") == b and
                      page(chromium, "document.readyState") == "complete", timeout=15)
    time.sleep(1)
    assert page(chromium, "window.__planted === undefined") is True, "the agent's service worker answered site B"
    page(chromium, "navigator.serviceWorker.getRegistrations().then(r => window.__regs = r.length); "
                   "caches.keys().then(k => window.__caches = k.length); 1")
    assert wait_until(lambda: page(chromium, "window.__regs") is not None, timeout=10)
    assert page(chromium, "window.__regs") == 0
    assert wait_until(lambda: page(chromium, "window.__caches") is not None, timeout=10)
    assert page(chromium, "window.__caches") == 0
    assert page(chromium, "document.cookie.includes('login=kept')") is True
    assert page(chromium, "localStorage.getItem('login')") == "kept"
    take(bridge, False)
