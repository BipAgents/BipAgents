"""Relecture 9 of fix-pay-total-check (20/11, on 1fe82c895), product side: 14 more GOOD payments (the page's total is
the approved amount), other markets and shops (structure only: no real brand, no real shop). Each asserts « approved »
and « Payer » pressed; a failure is a good payment handed over. A measurement, not a blocker. Real Chromium, test card."""
import pytest

from test_review_pay_total_chrome import APPROVED, page, start
from test_review8_handover_chrome import CSS, summary, two_col, row
import test_card_payments_chrome as c

pytestmark = c.pytestmark

CASES = [
    ("ca_shopify_cad_dollar", summary([("Subtotal", "$27.97"), ("Shipping", "$4.90"), ("Total", "CAD $32.87")]),
     "32.87", "CAD", "CAD $32.87"),
    ("ca_fr_dollar_cad", summary([("Sous-total", "27,97 $"), ("Livraison", "4,90 $"), ("Total", "32,87 $ CAD")]),
     "32.87", "CAD", "32,87 $ CAD"),
    ("il_hebrew", '<div dir="rtl">' + summary([("סכום ביניים", "₪27.97"), ("משלוח", "₪4.90"), ("סה״כ לתשלום", "₪32.87")])
     + '</div>', "32.87", "ILS", "₪32.87"),
    ("sg_sgd", summary([("Subtotal", "S$27.97"), ("Delivery", "S$4.90"), ("Total", "S$32.87")]), "32.87", "SGD", "S$32.87"),
    ("us_tax_lines", summary([("Items:", "$27.97"), ("Shipping &amp; handling:", "$4.90"), ("Total before tax:", "$32.87"),
                               ("Estimated tax to be collected:", "$2.63"), ("Order total:", "$35.50")]),
     "35.50", "USD", "$35.50"),
    ("de_gesamtbetrag_vat_line", summary([("Warenwert", "27,97 €"), ("Versandkosten", "4,90 €"), ("Gesamtbetrag", "32,87 €")])
     + '<p class="muted">inkl. 19 % MwSt. 5,25 €</p>', "32.87", "EUR", "32,87 €"),
    ("es_total_iva_brackets", summary([("Subtotal", "27,97 €"), ("Gastos de envío", "4,90 €"), ("Total (IVA incluido)", "32,87 €")]),
     "32.87", "EUR", "32,87 €"),
    ("nl_te_betalen_bedrag", summary([("Subtotaal", "€ 27,97"), ("Verzendkosten", "€ 4,90"), ("Te betalen bedrag", "€ 32,87")]),
     "32.87", "EUR", "€ 32,87"),
    ("fr_marketplace_sup_cents", CSS + row("Sous-total", "27,97 €") + row("Livraison", "4,90 €")
     + '<div class="row big"><span>Total de votre commande</span><span style="font-size:28px">32€<sup style="font-size:55%">87</sup></span></div>'
     + '<p class="muted">Dont éco-participation 0,50 €</p>', "32.87", "EUR", "32,87 €"),
    ("uk_total_to_pay_incl_vat", summary([("Subtotal", "£27.97"), ("Delivery", "£4.90"), ("Total to pay", "£32.87")])
     + '<p class="muted">(incl. VAT £5.48)</p>', "32.87", "GBP", "£32.87"),
    ("nl_hosted_betalen_aan", '<div style="max-width:420px;margin:auto"><p>Betalen aan Webshop Exemple</p>'
     '<p style="font-size:32px;margin:0">€ 32,87</p><p class="muted">Bestelling 100045678</p></div>', "32.87", "EUR", "€ 32,87"),
    ("fr_montant_ttc_invoice", summary([("Montant HT", "27,39 €"), ("TVA 20 %", "5,48 €"), ("Montant TTC", "32,87 €")]),
     "32.87", "EUR", "32,87 €"),
    ("us_balance_due", summary([("Invoice #10045", "$32.87"), ("Balance due", "$32.87")]), "32.87", "USD", "$32.87"),
    ("fr_train_two_travellers", two_col('<h2>Voyageurs</h2><p>2 adultes</p>',
     row("Paris → Lyon, adulte 1", "64,00 €") + row("Paris → Lyon, adulte 2", "64,00 €")
     + row("Total pour 2 voyageurs", "128,00 €", "big")), "128.00", "EUR", "128,00 €"),
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
