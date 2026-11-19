"""The page's total read by the bridge once the card is typed (fix-pay-total-check, 19/11), on a real Chromium
(test_card_payments_chrome.py's harness, BIP_CHROME), with a page written like CAWL's hosted payment page
(fixtures/checkout/cawl.html: « Total à payer: EUR 27.97 », « Payer », « Annuler »).

On 19/11, Vie had the card typed on CAWL's page for the approved 27.97 EUR, was told « If the total you read before
was 27.97 EUR, call confirm_payment… otherwise call cancel_payment and tell the person », cancelled, and told Alex
that the bank refused. Now the bridge reads the total itself:
- the approved amount on the page: the agent is told to confirm, no « if »;
- another total: the bridge cancels by itself (fields emptied, the card's spent corrected) and tells the person in
  the conversation with both numbers (pay_line.rs), and the agent that the person was told;
- no total readable: as before;
- cancel_payment: the bridge tells the person itself, with the agent's reason quoted.
Test card only (Visa's published 4242… number), never a real shop."""
import json

from bbkit.common import AUTH
from test_card_payments_chrome import ARGS, CARD, Browser, pay, pay_bridge, site, turn_on
import test_card_payments_chrome as c

pytestmark = c.pytestmark

CAWL = {**ARGS, "amount": "27.97", "currency": "EUR", "merchant": "Commande pharmacie"}


def lines(bridge):
    path = bridge.data / "pay-lines.jsonl"
    return [json.loads(l) for l in path.read_text().splitlines() if l.strip()] if path.exists() else []


def spent(bridge):
    return bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"].get("spent", {})


def start(bridges, fakes, page):
    shop = site()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/{page}")
    bridge = pay_bridge(bridges, fakes, browser)
    turn_on(bridge)
    assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]}).status_code == 201
    return shop, browser, bridge


def test_the_approved_total_on_the_page_means_confirm(bridges, fakes):
    shop, browser, bridge = start(bridges, fakes, "cawl.html")
    try:
        result, asked = pay(bridge, CAWL)
        said = result["content"][0]["text"]
        assert result["isError"] is False and " 27.97 EUR on 127.0.0.1" in asked, (result, asked)
        # no « if », no comparison asked: the hidden « EUR 50.00 » and the field's « Remise 99,99 € » never count
        assert "its total is 27.97 EUR, the amount the person approved. Now call confirm_payment" in said, said
        assert "If the total you read" not in said, said
        assert browser.values()["cawl.html"]["number"].replace(" ", "") == "4242424242424242"
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
        assert "moved on" in confirmed, confirmed
        assert lines(bridge) == []   # nothing to tell the person: the agent tells them it is paid
    finally:
        browser.stop()
        shop.shutdown()


def test_another_total_on_the_page_is_cancelled_by_the_bridge_and_the_person_told(bridges, fakes):
    shop, browser, bridge = start(bridges, fakes, "cawl.html")
    try:
        result, asked = pay(bridge, {**CAWL, "amount": "24.50"})
        said = result["content"][0]["text"]
        assert asked and " 24.50 EUR on 127.0.0.1" in asked, asked
        assert result["isError"] is True, result
        assert "its total is 27.97 EUR, but the person approved 24.50 EUR" in said, said
        assert "BipAgents cancelled the payment itself" in said and "nothing was paid" in said, said
        # read before the card was typed: nothing of it ever reached the page
        assert "call pay_with_card again" in said, said
        assert "never say the bank" in said, said
        # the person's line, from the bridge, with both numbers (the agent's language here is English)
        line = "Payment cancelled: the page asked for €27.97, you had approved €24.50. Nothing was paid."
        assert [l["text"] for l in lines(bridge)] == [line], lines(bridge)
        assert f"« {line} »" in said, said
        # never pressed, the card erased, the payment page shown again, nothing counted on the card
        values = browser.values()
        assert list(values) == ["cawl.html"] and values["cawl.html"]["number"] == "", values
        assert values["cawl.html"]["csc"] == "" and "4242" not in json.dumps(values), values
        assert spent(bridge).get("EUR", 0) == 0, spent(bridge)
        assert bridge.get("/v1/vault/payments", headers=AUTH).json()["active"] == []
        # the browser is the agent's again: a payment can be asked again
        assert c.tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"].startswith("No payment")
    finally:
        browser.stop()
        shop.shutdown()


def test_no_total_on_the_page_keeps_the_agents_reading(bridges, fakes):
    shop, browser, bridge = start(bridges, fakes, "selects.html")   # a form with no amount at all
    try:
        result, _ = pay(bridge, ARGS)
        said = result["content"][0]["text"]
        assert result["isError"] is False, result
        assert "could not read the total on the page. If the total you read before was 12.50 EUR" in said, said
        assert lines(bridge) == []
        assert c.tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
    finally:
        browser.stop()
        shop.shutdown()


def test_the_total_beside_the_providers_frame_is_read(bridges, fakes):
    # the card's fields in a payment provider's frame on another site, the amount in the shop's page
    shop, provider = site(), site()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/elements.html"
                      f"?psp=http://localhost:{provider.server_port}/provider.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=c.test_providers("localhost"))
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, _ = pay(bridge, ARGS)
        said = result["content"][0]["text"]
        assert result["isError"] is False and "the amount the person approved" in said, said
        assert c.tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
    finally:
        browser.stop()
        shop.shutdown()
        provider.shutdown()


def test_cancel_payment_tells_the_person_itself_never_in_the_agents_words(bridges, fakes):
    shop, browser, bridge = start(bridges, fakes, "cawl.html")
    try:
        result, _ = pay(bridge, CAWL)
        assert result["isError"] is False, result
        answer = c.tool(bridge, "cancel_payment", {"reason": "la banque refuse"})["content"][0]["text"]
        # the bridge's own words only: the agent's reason is never in the person's line (Relecture 1, 9)
        line = "Payment cancelled at Vie's request. Nothing was paid."
        assert [l["text"] for l in lines(bridge)] == [line], lines(bridge)
        assert "banque" not in json.dumps(lines(bridge)), lines(bridge)
        # no conversation followed here (no turn of the hub): the agent is told to say the sentence itself
        assert answer.startswith("The payment is given up") and f"start your next message to the person with exactly this sentence: « {line} »" in answer, answer
        assert "never say the bank, the card or the site refused" in answer, answer
        assert spent(bridge).get("EUR", 0) == 0 and "4242" not in json.dumps(browser.values())
        # nothing in progress: nothing said
        assert c.tool(bridge, "cancel_payment")["content"][0]["text"] == "No payment in progress."
        assert len(lines(bridge)) == 1
    finally:
        browser.stop()
        shop.shutdown()


SPLIT = """<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Paiement</title></head><body>
<div class="recap"><span>Total TTC</span> <strong><span>27</span>,<sup>97</sup>&nbsp;€</strong></div>
<div class="recap">Sous-total <span>24</span><span>,50</span> €</div>
<form onsubmit="event.preventDefault(); location.href='done.html'">
  <p><label>Numéro de carte <input id="numero" autocomplete="cc-number"></label></p>
  <p><label>Expiration <input id="exp" autocomplete="cc-exp" placeholder="MM/AA"></label></p>
  <p><label>Cryptogramme <input id="crypto" autocomplete="cc-csc"></label></p>
  <p><label>Titulaire <input id="titulaire" autocomplete="cc-name"></label></p>
  <p><button type="submit">Payer</button></p>
</form></body></html>"""


def test_an_amount_split_in_several_elements_is_read_whole(bridges, fakes, tmp_path):
    # « 27<span>,</span><sup>97</sup> € »: the pieces of one line glued back, never two numbers
    import functools
    import http.server
    import threading
    (tmp_path / "pay.html").write_text(SPLIT, encoding="utf-8")
    (tmp_path / "done.html").write_text("<!doctype html><title>Merci</title><p>Commande payée", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    handler.log_message = lambda *a: None
    shop = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=shop.serve_forever, daemon=True).start()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/pay.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, _ = pay(bridge, CAWL)
        said = result["content"][0]["text"]
        assert result["isError"] is False and "its total is 27.97 EUR, the amount the person approved" in said, said
        assert c.tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        # the subtotal approved instead: the page's total said (the tab was left on an empty page by the cancel)
        browser.open(f"http://127.0.0.1:{shop.server_port}/pay.html")
        result, _ = pay(bridge, {**CAWL, "amount": "24.50"})
        said = result["content"][0]["text"]
        assert result["isError"] is True and "its total is 27.97 EUR, but the person approved 24.50 EUR" in said, said
    finally:
        browser.stop()
        shop.shutdown()


def test_a_pay_button_in_another_currency_is_never_pressed(bridges, fakes, tmp_path):
    # the page offers to pay in dollars (DCC): « Payer 32,10 USD » for a payment approved in euros
    import functools
    import http.server
    import threading
    html = SPLIT.replace("<button type=\"submit\">Payer</button>", "<button type=\"submit\">Payer 32,10 USD</button>")
    (tmp_path / "pay.html").write_text(html, encoding="utf-8")
    (tmp_path / "done.html").write_text("<!doctype html><title>Merci</title><p>Commande payée", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    handler.log_message = lambda *a: None
    shop = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=shop.serve_forever, daemon=True).start()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/pay.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, _ = pay(bridge, CAWL)
        said = result["content"][0]["text"]
        # an amount in another currency on the page: never « approved »
        assert result["isError"] is False and "could not read the total" in said, said
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
        assert "not the payment's currency" in confirmed and "done.html" not in "".join(browser.pages()), confirmed
        line = "Payment stopped: the page asked for another currency ($32.10). Nothing was paid."
        assert [l["text"] for l in lines(bridge)] == [line], lines(bridge)
    finally:
        browser.stop()
        shop.shutdown()
