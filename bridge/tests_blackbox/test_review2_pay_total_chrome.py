"""Relecture 2 of fix-pay-total-check (19/11, on 52c025772): pages that ask for more than the approved amount and
whose « Payer » is still pressed. Each test asserts the SAFE outcome: they fail on 52c025772.

The « agent » here follows the bridge's words exactly: told « If the total you read before was 27.97 EUR, call
confirm_payment », it had read 27,97 € on the shop's cart before, so it confirms (what an agent does; 19/11).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import functools
import http.server
import threading

from bbkit.mcp import not_done
from bbkit.common import AUTH
from test_card_payments_chrome import ARGS, CARD, Browser, pay, pay_bridge, turn_on
from test_review_pay_total_chrome import APPROVED, never_pressed, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

SHOP = """<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Paiement</title></head><body>
<p>Total panier 27,97 €</p>
<iframe id="psp" width="600" height="260"></iframe>
<button id="pay" onclick="location.href='done.html'">Payer</button>
<script>document.getElementById("psp").src = new URLSearchParams(location.search).get("psp");</script>
</body></html>"""

PSP = """<!doctype html><html><head><meta charset="utf-8"><title>Card</title></head><body>
<div>Pharmacie des Drakkars</div><div style="font-size:28px">€32.87</div>
<div>Card information</div>
<input id="number" autocomplete="cc-number" aria-label="Card number">
<input id="exp" autocomplete="cc-exp" aria-label="Expiry">
<input id="cvc" autocomplete="cc-csc" aria-label="CVC">
</body></html>"""


def served(folder):
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(folder))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def agent_confirms(bridge, browser, said):
    # the agent read « Total panier 27,97 € » before: told to confirm if so, it confirms
    if "If the total you read before was 27.97 EUR" in said or "Now call confirm_payment" in said:
        never_pressed(bridge, browser, said)


def test_q1_the_providers_own_higher_amount_is_left_to_the_agent_and_paid(bridges, fakes, tmp_path):
    (tmp_path / "shop.html").write_text(SHOP, encoding="utf-8")
    (tmp_path / "psp.html").write_text(PSP, encoding="utf-8")
    (tmp_path / "done.html").write_text("<!doctype html><title>Merci</title><p>Commande payée", encoding="utf-8")
    shop, provider = served(tmp_path), served(tmp_path)
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/shop.html?psp=http://localhost:{provider.server_port}/psp.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=c.test_providers("localhost"))
        turn_on(bridge)
        assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]}).status_code == 201
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        # Correction 2: the provider's own €32.87 is seen: stopped by the bridge, the person told
        assert not_done(result) and "32.87" in said, said
        agent_confirms(bridge, browser, said)
        never_pressed(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()
        provider.shutdown()


def test_q2_a_total_after_60000_characters_is_never_read(bridges, fakes, tmp_path):
    # the conditions of sale in full on the checkout page, the final total under them
    cgv = "<p>" + ("Conditions générales de vente : le client reconnaît avoir lu les présentes conditions. " * 700) + "</p>"
    html = page(f"<p>Total panier 27,97 €</p>{cgv}<p>Frais de livraison 4,90 €</p><p><b>Total à payer 32,87 €</b></p>")
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert "the amount the person approved. Now call confirm_payment" not in said, said
        agent_confirms(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()


def test_q3_a_total_drawn_by_css_is_never_read(bridges, fakes, tmp_path):
    html = page('<style>.p::after{content:"32,87 €"}</style><p>Total panier 27,97 €</p>'
                '<p>Frais de livraison 4,90 €</p><p><b>Total à payer <span class="p"></span></b></p>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert "the amount the person approved. Now call confirm_payment" not in said, said
        agent_confirms(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()
