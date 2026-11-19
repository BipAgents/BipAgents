"""Relecture 1 of fix-pay-total-check (19/11): pages on which the bridge's own reading of the total tells the agent
« its total is the amount the person approved. Now call confirm_payment » while the page asks for more, and the pay
button (« Payer », no amount) is pressed. Each test asserts the SAFE outcome: they fail on c56eda1cd.

Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import functools
import http.server
import threading
import time

from bbkit.common import AUTH
from test_card_payments_chrome import ARGS, CARD, Browser, pay, pay_bridge, turn_on
import test_card_payments_chrome as c

pytestmark = c.pytestmark

APPROVED = {**ARGS, "amount": "27.97", "currency": "EUR", "merchant": "Commande pharmacie"}

FORM = """<form onsubmit="event.preventDefault(); location.href='done.html'">
  <p><label>Numéro de carte <input id="numero" autocomplete="cc-number"></label></p>
  <p><label>Expiration <input id="exp" autocomplete="cc-exp" placeholder="MM/AA"></label></p>
  <p><label>Cryptogramme <input id="crypto" autocomplete="cc-csc"></label></p>
  <p><label>Titulaire <input id="titulaire" autocomplete="cc-name"></label></p>
  <p><button type="submit">Payer</button></p>
</form>"""


def page(body, script=""):
    return (f'<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Paiement</title></head><body>{body}'
            f"{FORM}<script>{script}</script></body></html>")


def serve(tmp_path, html):
    (tmp_path / "pay.html").write_text(html, encoding="utf-8")
    (tmp_path / "done.html").write_text("<!doctype html><title>Merci</title><p>Commande payée", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    handler.log_message = lambda *a: None
    shop = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=shop.serve_forever, daemon=True).start()
    return shop


def start(bridges, fakes, tmp_path, html):
    shop = serve(tmp_path, html)
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/pay.html")
    bridge = pay_bridge(bridges, fakes, browser)
    turn_on(bridge)
    assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]}).status_code == 201
    return shop, browser, bridge


def never_pressed(bridge, browser, said):
    confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
    pages = [u.split("/")[-1].split("?")[0] for u in browser.pages()]
    assert "moved on" not in confirmed and "done.html" not in pages, (
        f"approved 27.97 EUR, the page asked for more, « Payer » was pressed.\npay: {said}\nconfirm: {confirmed}")


def test_r1_a_higher_total_beside_the_approved_one_is_never_confirmed(bridges, fakes, tmp_path):
    # the basket approved, the shipping forgotten: « Total 32,87 € » on the page
    html = page("<p>Total panier 27,97 €</p><p>Frais de livraison 4,90 €</p><p><b>Total 32,87 €</b></p>")
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert "the amount the person approved. Now call confirm_payment" not in said, said
        never_pressed(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()


def test_r2_an_offscreen_to_pay_line_never_outvotes_the_visible_total(bridges, fakes, tmp_path):
    # what the person (and the agent's screenshot) sees: « Total 35,00 € »; off-screen text the reader still takes
    html = page('<p><b>Total 35,00 €</b></p>'
                '<p style="position:absolute;left:-10000px;top:0">Total à payer 27,97 €</p>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert "the amount the person approved. Now call confirm_payment" not in said, said
        never_pressed(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()


def test_r3_a_total_that_changes_after_the_read_is_never_pressed(bridges, fakes, tmp_path):
    # a card fee the provider adds once it knows the card (its number typed), a moment later; the button says « Payer »
    script = """
      let done = false;
      setInterval(() => {
        if (done || document.getElementById("numero").value.replace(/\\s/g, "").length < 12) return;
        done = true;
        setTimeout(() => {
          document.getElementById("total").textContent = "EUR 29.47";
          document.getElementById("fee").textContent = "Frais carte : EUR 1.50";
        }, 2500);
      }, 100);"""
    html = page('<p id="fee"></p><div><span>Total à payer:</span> <span id="total">EUR 27.97</span></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False, result
        time.sleep(4)   # the agent's next step: the page now says « Total à payer: EUR 29.47 »
        never_pressed(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()


def test_r4_a_card_number_the_page_shows_back_never_reaches_the_log(bridges, fakes, tmp_path):
    # a card preview right under an amount, filled by the page as the number is typed
    script = """
      setInterval(() => {
        document.getElementById("preview").textContent = document.getElementById("numero").value;
      }, 50);"""
    html = page('<div>27,97 €</div><div id="preview"></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, {**APPROVED, "amount": "12.50"})
        assert result["isError"] is False, result
        c.tool(bridge, "cancel_payment")
        log = bridge.log_path.read_text(encoding="utf-8", errors="replace")
        print("READ:", [l for l in log.splitlines() if "card payment" in l])
        assert "424200" not in log, [l for l in log.splitlines() if "424200" in l]
    finally:
        browser.stop()
        shop.shutdown()


def test_r5_control_the_pay_buttons_own_amount_still_stops_an_approved_page(bridges, fakes, tmp_path):
    # control: the page's « Total 27,97 € », the button « Payer 1 250,00 € »: never pressed (Correction 1: and never
    # « approved » either, an amount above the approved one is on the page)
    html = page("<p>Total 27,97 €</p>").replace("<button type=\"submit\">Payer</button>",
                                                 "<button type=\"submit\">Payer 1 250,00 €</button>")
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        # Correction 1: an amount above the approved one on the page (the button's) is a doubt: never « approved »
        assert "the amount the person approved. Now call confirm_payment" not in said, said
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer 1 250,00 €"})["content"][0]["text"]
        assert "was not pressed" in confirmed and "done.html" not in "".join(browser.pages()), confirmed
    finally:
        browser.stop()
        shop.shutdown()
