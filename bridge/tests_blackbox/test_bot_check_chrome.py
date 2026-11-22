"""A bot check that still blocks the agent's visible browser is handed to its person (antibot-guard, 22/11;
passation-pro/briefs/enquete-antibot-1122.md §3-4): bip-browser-mcp (BIP_BROWSER_MCP) looks at each page it loads in
a real Chromium (BIP_CHROME), behind the stand-in for stealth-browser-mcp of test_signup_guard_chrome.py. Local fake
check pages only (fixtures/bot_check/): never a real shop, never a real challenge provider; nothing leaves this machine.

What happens (review 1, 22/11): a check that blocks what the agent does is handed over (the stop said to the agent,
`challenge.json` for the bridge beside `busy`, the site not opened again in the turn): an interstitial that stays, a
widget alone on its page (its closed shadow root's too), the widget of a form the agent sends (not sent); a widget on a
page it only reads is said, never handed over; the person's hand passes it; one the browser gets past by itself, an
answered widget and reCAPTCHA v3's badge trigger nothing."""
import json
import shutil
import threading
import time
from pathlib import Path

import pytest

import test_signup_guard_chrome as s
from test_signup_guard_chrome import Session

pytestmark = s.pytestmark
HERE = Path(__file__).parent / "fixtures" / "bot_check"
STOP = "Stopped: a bot check"
REFUSED = "Refused: a bot check"
PASSED = "Your person passed the site's bot check in your browser and handed it back"
CLEARED = "The site's bot check cleared by itself"
PRESENT = "A captcha ("
FORM = "Not done: this form's captcha"


def session(clear=1, wait=2):
    browser = Session(settings={"agent": "vie", "challenge_clear_seconds": clear, "hand_wait_seconds": wait})
    for page in HERE.iterdir():
        shutil.copy(page, browser.dir / "site" / page.name)
    return browser


@pytest.fixture
def browser():
    opened = []

    def make(**kwargs):
        opened.append(session(**kwargs))
        return opened[-1]

    yield make
    for b in opened:
        b.close()


def test_a_widget_on_a_page_read_is_said_sending_its_form_is_handed_over(browser):
    b = browser()
    said = b.tool("navigate", url=f"{b.base}/turnstile.html")
    assert PRESENT in said and "Cloudflare Turnstile" in said and STOP not in said, said
    assert not (b.dir / "challenge.json").exists()
    said = b.tool("click_element", selector="form button")
    assert FORM in said and STOP in said and "« Prendre la main »" in said, said
    assert b.page("location.pathname") == "/turnstile.html", "never sent"
    note = json.loads((b.dir / "challenge.json").read_text())
    assert note["agent"] == "vie" and note["site"] == "127.0.0.1" and note["kind"] == "Cloudflare Turnstile"
    # a second try in this turn: refused, the page left as it is
    again = b.tool("navigate", url=f"{b.base}/ordinary.html")
    assert again.startswith(REFUSED), again
    assert b.page("document.title") == "Connexion"
    clicked = b.tool("click_element", selector=".cf-turnstile")
    assert clicked.startswith(REFUSED), clicked


def test_an_answered_widget_the_v3_badge_and_an_ordinary_page_trigger_nothing(browser):
    b = browser()
    for page in ("turnstile_done.html", "recaptcha_v3.html", "ordinary.html"):
        started = time.time()
        said = b.tool("navigate", url=f"{b.base}/{page}")
        assert "bot check" not in said, (page, said)
        assert time.time() - started < 5, page
    assert not (b.dir / "challenge.json").exists()


def test_an_interstitial_the_browser_gets_past_by_itself_is_never_handed_over(browser):
    b = browser(clear=8, wait=30)
    started = time.time()
    said = b.tool("navigate", url=f"{b.base}/interstitial.html#clears")
    assert CLEARED in said and STOP not in said, said
    assert time.time() - started < 8
    assert not (b.dir / "challenge.json").exists()


def test_an_interstitial_that_stays_is_handed_over(browser):
    b = browser()
    said = b.tool("navigate", url=f"{b.base}/interstitial.html")
    assert STOP in said and "Cloudflare challenge" in said, said


def test_a_widget_alone_on_its_page_is_a_wall_its_closed_shadow_root_too(browser):
    for page in ("turnstile_wall.html", "turnstile_closed.html"):
        b = browser()
        said = b.tool("navigate", url=f"{b.base}/{page}")
        assert STOP in said and "Cloudflare Turnstile" in said, (page, said)


def test_the_person_takes_over_answers_the_box_and_hands_back_the_agent_goes_on(browser):
    b = browser(clear=0, wait=30)

    def person():
        time.sleep(1.5)
        (b.dir / "hold.token").write_text("t")   # « Prendre la main » (browsers.rs `hold`)
        time.sleep(1.0)
        b.page("document.getElementById('g-recaptcha-response').value = 'answered-by-the-person'")
        (b.dir / "hold.token").unlink()          # « Rendre la main »

    assert PRESENT in b.tool("navigate", url=f"{b.base}/recaptcha_v2.html")
    hand = threading.Thread(target=person)
    hand.start()
    started = time.time()
    said = b.tool("click_element", selector="form button")
    hand.join()
    assert FORM in said and PASSED in said, said
    assert time.time() - started < 20
    # the site open again
    assert "bot check" not in b.tool("navigate", url=f"{b.base}/ordinary.html")
