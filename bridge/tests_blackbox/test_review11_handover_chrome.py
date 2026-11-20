"""Relecture 11 of fix-pay-total-check (20/11, on 221a9ea8e), Alex's criterion A: GOOD payments (the page's total is the
approved amount) that main (34809051d, no total check) pays and the branch hands over. Each asserts « approved » and
« Payer » pressed; a failure is a good payment blocked. Mostly Correction 11's new « orphan » rule (page_total.rs
`orphan_number`): any number with no mark beside it, on or next to a line holding a currency mark beside no number
(« Prix en € TTC », « Devise : EUR »), is read as an amount and blocks when above the approved one. Structure only, no
real brand. Real Chromium, test card."""
import pytest

from test_review_pay_total_chrome import APPROVED, page, start
from test_review8_handover_chrome import CSS, summary
import test_card_payments_chrome as c

pytestmark = c.pytestmark

SUMMARY = summary([("Sous-total", "27,97 €"), ("Livraison", "4,90 €"), ("Total à payer", "32,87 €")])

CASES = [
    # a shop's footer: « Prix en € TTC » then the customer service's phone number → 0 123 456 789 € « orphan »
    ("footer_phone_under_prix_en_euros", SUMMARY + '<footer class="muted"><p>Tous nos prix en € TTC</p>'
     '<p>Service client : 01 23 45 67 89</p></footer>', "32.87", "EUR", "32,87 €"),
    # a bank's hosted page (Monetico / Systempay-like): « Référence : 100045678 » beside « Devise : EUR »
    ("hosted_reference_beside_devise", '<div style="max-width:480px;margin:auto"><p>Boutique : Pharmacie Exemple</p>'
     '<p>Référence : 100045678</p><p>Devise : EUR</p><p>Montant à régler : 32,87 €</p></div>', "32.87", "EUR", "32,87 €"),
    # a delivery promise under « Prix en € TTC »: « 48h » → 48,00 € above the approved 32,87
    ("delivery_48h_under_prix_en_euros", SUMMARY + '<p class="muted">Prix en € TTC</p><p class="muted">Expédition sous 48h</p>',
     "32.87", "EUR", "32,87 €"),
    # PayPal-like « 32,87 € EUR » (the code after the amount is a « free » mark) above a delivery date
    ("paypal_eur_code_above_delivery_date", CSS + '<div style="max-width:480px;margin:auto"><h3>Récapitulatif</h3>'
     '<div class="row"><span>Crème solaire</span><span>27,97 €</span></div>'
     '<div class="row big"><span>Total</span><span>27,97 € EUR</span></div>'
     '<p>Livraison estimée le 28 novembre</p></div>', "27.97", "EUR", "27,97 €"),
    # a table with a quantity column and an item of 100–999 €: « Téléviseur 2 249,00 € » reads 2 249,00 € (pre-existing)
    ("qty_column_item_249", CSS + '<table><thead><tr><th>Produit</th><th>Qté</th><th>Prix</th></tr></thead><tbody>'
     '<tr><td>Téléviseur</td><td>2</td><td>249,00 €</td></tr><tr><td>Livraison</td><td></td><td>4,90 €</td></tr>'
     '<tr><th>Total à payer</th><td></td><th>502,90 €</th></tr></tbody></table>', "502.90", "EUR", "502,90 €"),
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
