"""Relecture 4 of fix-pay-total-check (19/11, on 0b9d8ca94). d1, d2: pages that charge more and are pressed (fail on
0b9d8ca94); d3, d5: good payments handed over (product side; fail on 0b9d8ca94); d4: the hand-over leaves the browser and
the card clean (control, plus the « Panier prêt » amount).
The « agent » gives confirm_payment the total the bridge told it (« its total is 27.97 EUR »).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import json

from bbkit.common import AUTH
from test_review_pay_total_chrome import APPROVED, never_pressed, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

APPROVED_TEXT = "the amount the person approved. Now call confirm_payment"


def lines(bridge):
    path = bridge.data / "pay-lines.jsonl"
    return [json.loads(l)["text"] for l in path.read_text().splitlines() if l.strip()] if path.exists() else []


def carts(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-cart-")]


def pressed_if_typed(bridge, browser, result):
    said = result["content"][0]["text"]
    if result["isError"] is False:
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "27,97 €"})["content"][0]["text"]
        pages = [u.split("/")[-1].split("?")[0] for u in browser.pages()]
        assert "moved on" not in confirmed and "done.html" not in pages, (
            f"approved 27.97 EUR, the page asks 32,87 €, « Payer » was pressed.\npay: {said}\nconfirm: {confirmed}")


def test_d1_a_cross_sell_in_the_summary_box_hides_the_real_total(bridges, fakes, tmp_path):
    # the basket's amount at the top; the order summary (with its own « Complétez votre commande » suggestion) holds the
    # real total: every amount of that box is marked « [suggestion] », so the 32,87 € is a mention
    html = page('<p>Montant de votre panier : 27,97 €</p>'
                '<aside><h3>Récapitulatif</h3><p>Frais de livraison 4,90 €</p><p><b>Total à payer 32,87 €</b></p>'
                '<div><p>Complétez votre commande : Pansements 3,90 €</p><button type="button">Ajouter au panier</button>'
                '</div></aside>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        pressed_if_typed(bridge, browser, result)
    finally:
        browser.stop()
        shop.shutdown()


def test_d2_a_total_in_a_frame_the_bridge_does_not_read_is_ignored(bridges, fakes, tmp_path):
    # the order summary in a frame of its own (about:srcdoc: not a frame the bridge reads); nothing says the read was
    # partial, so the basket line outside it is « the total »
    html = page('<p>Montant de votre panier : 27,97 €</p>'
                '<iframe width="400" height="120" srcdoc="<p>Frais de livraison 4,90 €</p><p><b>Total à payer 32,87 €</b></p>">'
                '</iframe>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        pressed_if_typed(bridge, browser, result)
    finally:
        browser.stop()
        shop.shutdown()


def test_d3_an_unrelated_closed_shadow_widget_does_not_hand_a_good_payment_over(bridges, fakes, tmp_path):
    # a chat / consent / express-checkout widget built as a closed web component, no amount in it
    script = """
      const root = document.getElementById("widget").attachShadow({mode: "closed"});
      root.innerHTML = "<button>Besoin d'aide ?</button>";"""
    html = page('<div><span>Total à payer:</span> <span>EUR 27.97</span></div><div id="widget"></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and APPROVED_TEXT in said, (said, lines(bridge))
    finally:
        browser.stop()
        shop.shutdown()


def test_d4_a_hand_over_leaves_nothing_typed_the_hold_released_and_the_cart_says_the_pages_amount(bridges, fakes, tmp_path):
    html = page('<p>Total panier 27,97 €</p><p>32,87 €</p>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, asked = c.pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        # Correction 4: the page is read before the person is asked: handed over before any approval (asked is None)
        assert asked is None and result["isError"] is True and "Panier prêt" in said, said
        values = browser.values()
        assert all(v == "" for v in values["pay.html"].values()), values
        assert "4242" not in json.dumps(values)
        assert bridge.get("/v1/vault/payments", headers=AUTH).json()["active"] == []
        spent = bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"].get("spent", {})
        assert spent.get("EUR", 0) == 0, spent
        # the browser is the agent's again: its page answers a script (no hold left)
        assert c.tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"].startswith("No payment")
        [cart] = carts(bridge)
        # the person's card: it should not offer « 27,97 € » for a page that showed 32,87 €
        assert "27,97" not in cart["command"] and "27.97" not in cart["command"], cart["command"]
    finally:
        browser.stop()
        shop.shutdown()


def test_d5_a_suggestion_with_a_short_add_button_does_not_hand_a_good_payment_over(bridges, fakes, tmp_path):
    # « + Ajouter » (not « Ajouter au panier »): the suggested product's 39,90 € is « a higher amount »
    html = page('<p><b>Total à payer 27,97 €</b></p>'
                '<div><p>Vous aimerez aussi : Coffret soin 39,90 €</p><button type="button">+ Ajouter</button></div>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and APPROVED_TEXT in said, (said, lines(bridge))
    finally:
        browser.stop()
        shop.shutdown()
