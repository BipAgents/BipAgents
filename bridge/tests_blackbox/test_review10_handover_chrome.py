"""Relecture 10 of fix-pay-total-check (20/11, on f4d65a5bb), product side: more GOOD payments (the page's total is
the approved amount), common layouts (structure only: no real brand, no real shop). Each asserts « approved » and
« Payer » pressed; a failure is a good payment handed over. A measurement, except the WooCommerce ones: a regression of
Correction 10 (the quantity rule's agreement), with a FALSE « higher amount » sentence. Real Chromium, test card."""
import pytest

from test_review_pay_total_chrome import APPROVED, page, start
from test_review8_handover_chrome import CSS, WOO as WOO_SPF, summary
import test_card_payments_chrome as c

pytestmark = c.pytestmark

# (« Crème solaire SPF50 × 1 »: « 50 × » reads as an instalment and the line is skipped; a name without digits)
WOO = WOO_SPF.replace("Crème solaire SPF50", "Téléviseur")

WOO2 = (
    '<h3>Votre commande</h3><table><thead><tr><th>Produit</th><th>Sous-total</th></tr></thead><tbody>'
    '<tr><td>Crème solaire SPF50&nbsp;<strong>×&nbsp;1</strong></td><td><bdi>129,00&nbsp;<span>€</span></bdi></td></tr>'
    '<tr><td>Gel douche&nbsp;<strong>×&nbsp;1</strong></td><td><bdi>120,00&nbsp;<span>€</span></bdi></td></tr>'
    '</tbody><tfoot><tr><th>Sous-total</th><td><bdi>249,00&nbsp;<span>€</span></bdi></td></tr>'
    '<tr><th>Expédition</th><td>Livraison gratuite</td></tr>'
    '<tr><th>Total</th><td><strong><bdi>249,00&nbsp;<span>€</span></bdi></strong></td></tr></tfoot></table>')

CASES = [
    # WooCommerce's order review, an item of 100–999 € and paid shipping: « Crème × 1 249,00 € » is read 1 249,00 €
    # (no labelled total is 249,00) → Doubt::Higher → « la page affichait un montant plus élevé (1 249,00 €) »: false
    ("woo_fr_item_249_paid_shipping", CSS + WOO.format(item="249,00", ship="Colissimo : 4,90 €", total="253,90", vat="42,32"),
     "253.90", "EUR", "253,90 €"),
    ("woo_fr_qty2_item_249_paid_shipping",
     CSS + WOO.format(item="249,00", ship="Colissimo : 4,90 €", total="253,90", vat="42,32").replace("×&nbsp;1", "×&nbsp;2"),
     "253.90", "EUR", "253,90 €"),
    ("woo_fr_two_items_over_100", CSS + WOO2, "249.00", "EUR", "249,00 €"),
    ("fr_free_shipping_pour_50", '<p style="background:#efe;padding:8px">Livraison offerte pour 50 € d\'achat</p>'
     + summary([("Sous-total", "27,97 €"), ("Total à payer", "27,97 €")]), "27.97", "EUR", "27,97 €"),
    ("en_free_shipping_orders_50_plus", '<p style="background:#efe;padding:8px">Free shipping on orders €50+</p>'
     + summary([("Subtotal", "€27.97"), ("Order total", "€27.97")]), "27.97", "EUR", "€27.97"),
    ("en_upsell_add_to_order", CSS + '<div class="row"><span>Gift box €39.90</span><button type="button">Add to order</button></div>'
     + summary([("Subtotal", "€27.97"), ("Order total", "€27.97")]), "27.97", "EUR", "€27.97"),
]


@pytest.mark.parametrize("case", CASES, ids=[k[0] for k in CASES])
def test_good_payment_is_approved_and_pressed(bridges, fakes, tmp_path, case):
    _, html, amount, currency, stated = case
    shop, browser, bridge = start(bridges, fakes, tmp_path, page(html))
    try:
        result, _ = c.pay(bridge, {**APPROVED, "amount": amount, "currency": currency})
        said = result["content"][0]["text"]
        assert result["isError"] is False and "the amount the person approved" in said, said
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": stated})["content"][0]["text"]
        assert "moved on" in confirmed, confirmed
    finally:
        browser.stop()
        shop.shutdown()
