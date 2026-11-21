"""Relecture 8 of fix-pay-total-check (20/11, on bbc703ce4), product side: GOOD payments (the page's total is the
approved amount) on checkout layouts modelled on common platforms (structure only: no real brand, no real shop). Each
asserts the payment is approved and « Payer » pressed; a failure is a good payment handed over (« Panier prêt »).
A measurement, not a blocker. Real Chromium (BIP_CHROME), test card only."""
import pytest

from test_review_pay_total_chrome import APPROVED, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

CSS = ("<style>body{font-family:sans-serif;margin:0}.grid{display:grid;grid-template-columns:1fr 420px;gap:32px;"
       "padding:16px}.row{display:flex;justify-content:space-between;margin:6px 0}.big{font-size:22px;font-weight:700}"
       ".muted{color:#666;font-size:12px}td,th{padding:4px 8px;text-align:left}</style>")


def two_col(left, right):
    return CSS + f'<div class="grid"><main>{left}</main><aside>{right}</aside></div>'


def row(label, amount, cls=""):
    return f'<div class="row {cls}"><span>{label}</span><span>{amount}</span></div>'


SHOPIFY = two_col(
    '<h2>Contact</h2><p>E-mail</p><h2>Livraison</h2><p>Colissimo — 4,90 €</p>',
    '<div class="row"><span style="position:relative;display:inline-block;width:64px;height:64px;background:#eee">'
    '<span style="position:absolute;top:-8px;right:-8px;background:#777;color:#fff;border-radius:9px;padding:0 6px;'
    'font-size:12px">1</span></span><span>Crème solaire SPF50<br><span class="muted">50 ml</span></span>'
    '<span>27,97 €</span></div>'
    '<div class="row"><input placeholder="Code de réduction ou carte-cadeau"><button type="button">Appliquer</button></div>'
    + row("Sous-total", "27,97 €") + row("Expédition", "4,90 €")
    + '<div class="row big"><span>Total</span><span><small class="muted">EUR</small> 32,87 €</span></div>'
    '<p class="muted">Comprend 5,48 € de taxes</p>')

WOO = (
    '<h3>Votre commande</h3><table><thead><tr><th>Produit</th><th>Sous-total</th></tr></thead><tbody>'
    '<tr><td>Crème solaire SPF50&nbsp;<strong>×&nbsp;1</strong></td><td><bdi>{item}&nbsp;<span>€</span></bdi></td></tr>'
    '</tbody><tfoot><tr><th>Sous-total</th><td><bdi>{item}&nbsp;<span>€</span></bdi></td></tr>'
    '<tr><th>Expédition</th><td>{ship}</td></tr>'
    '<tr><th>Total</th><td><strong><bdi>{total}&nbsp;<span>€</span></bdi></strong> <small>(dont <bdi>{vat}&nbsp;€</bdi> TVA)</small></td></tr>'
    '</tfoot></table>')

STRIPE_ITEMS = two_col(
    '<p>Pay Pharmacie Exemple</p><p style="font-size:36px;font-weight:600">€32.87</p>'
    '<div class="row"><span>Crème solaire SPF50<br><span class="muted">Qty 1</span></span><span>€27.97</span></div>'
    + row("Subtotal", "€27.97") + row("Shipping", "€4.90") + row("Total due", "€32.87", "big"),
    '<h3>Pay with card</h3><p>Email</p><p>Card information</p>')

STRIPE_SINGLE = two_col(
    '<p>Pay Pharmacie Exemple</p><p style="font-size:36px;font-weight:600">€32.87</p><p class="muted">Ordonnance</p>',
    '<h3>Pay with card</h3><p>Email</p><p>Card information</p>')

CAWL = ('<div style="max-width:480px;margin:auto"><p>Pharmacie Exemple</p><p>Référence : 100045678</p>'
        '<div><span>Total à payer:</span> <span>EUR 27.97</span></div></div>')

PAYPAL = CSS + ('<div style="max-width:480px;margin:auto"><h3>Récapitulatif de la commande</h3>'
                + row("Crème solaire SPF50", "27,97 €") + row("Livraison", "4,90 €")
                + '<div class="row big"><span>Total</span><span>32,87 € EUR</span></div>'
                '<p>Payer par carte bancaire ou carte de crédit</p></div>')

BOOKING = two_col(
    '<h2>Vos informations</h2><p>Nom</p><p>Arrivée 12 déc. 2026 — Départ 14 déc. 2026</p>',
    '<h3>Hôtel Exemple</h3>' + row("Chambre double, 2 nuits", "210,70 €") + row("Taxe de séjour", "3,30 €")
    + row("Montant à régler", "214,00 €", "big"))

BOOKING_DEPOSIT = two_col(
    '<h2>Vos informations</h2>',
    '<h3>Hôtel Exemple</h3>' + row("Prix total", "214,00 €", "big") + row("À payer maintenant (acompte 30 %)", "64,20 €")
    + row("À payer sur place", "149,80 €"))


def summary(rows):
    return two_col('<h2>Adresse</h2><p>Rue</p>', "".join(row(l, a, "big" if i == len(rows) - 1 else "")
                                                       for i, (l, a) in enumerate(rows)))


CASES = [
    # (id, html, amount, currency, stated)
    ("shopify_fr", SHOPIFY, "32.87", "EUR", "32,87 €"),
    ("woo_fr_item_under_100", CSS + WOO.format(item="27,97", ship="Colissimo : 4,90 €", total="32,87", vat="5,48"),
     "32.87", "EUR", "32,87 €"),
    ("woo_fr_item_249", CSS + WOO.format(item="249,00", ship="Livraison gratuite", total="249,00", vat="41,50"),
     "249.00", "EUR", "249,00 €"),
    ("stripe_checkout_items", STRIPE_ITEMS, "32.87", "EUR", "€32.87"),
    ("stripe_checkout_single_amount", STRIPE_SINGLE, "32.87", "EUR", "€32.87"),
    ("cawl_hosted", CAWL, "27.97", "EUR", "EUR 27.97"),
    ("paypal_card_step", PAYPAL, "32.87", "EUR", "32,87 €"),
    ("booking_montant_a_regler", BOOKING, "214.00", "EUR", "214,00 €"),
    ("booking_deposit_now", BOOKING_DEPOSIT, "64.20", "EUR", "64,20 €"),
    ("de_gesamtsumme", summary([("Zwischensumme", "27,97 €"), ("Versand", "4,90 €"), ("Gesamtsumme", "32,87 €")]),
     "32.87", "EUR", "32,87 €"),
    ("de_gesamt_inkl_mwst", summary([("Zwischensumme", "27,97 €"), ("Versandkosten", "4,90 €"),
                                     ("Gesamt inkl. MwSt.", "32,87 €")]), "32.87", "EUR", "32,87 €"),
    ("es_total_a_pagar", summary([("Subtotal", "27,97 €"), ("Envío", "4,90 €"), ("Total a pagar", "32,87 €")]),
     "32.87", "EUR", "32,87 €"),
    ("es_total_iva_incluido", summary([("Subtotal", "27,97 €"), ("Envío", "4,90 €"), ("Total IVA incluido", "32,87 €")]),
     "32.87", "EUR", "32,87 €"),
    ("nl_totaal", summary([("Subtotaal", "€ 27,97"), ("Verzendkosten", "€ 4,90"), ("Totaal", "€ 32,87")]),
     "32.87", "EUR", "€ 32,87"),
    ("nl_totaal_incl_btw", summary([("Subtotaal", "€ 27,97"), ("Verzendkosten", "€ 4,90"), ("Totaal incl. btw", "€ 32,87")]),
     "32.87", "EUR", "€ 32,87"),
    ("en_order_total_gbp", summary([("Subtotal", "£27.97"), ("Delivery", "£4.90"), ("Order total", "£32.87")]),
     "32.87", "GBP", "£32.87"),
    ("en_total_incl_vat_gbp", summary([("Subtotal", "£27.97"), ("Delivery", "£4.90"), ("Total incl. VAT", "£32.87")]),
     "32.87", "GBP", "£32.87"),
    ("en_shopify_usd", summary([("Subtotal", "$27.97"), ("Shipping", "$4.90"), ("Total", "USD $32.87")]),
     "32.87", "USD", "$32.87"),
    ("fr_net_a_payer", summary([("Montant des articles", "27,97 €"), ("Frais de port", "4,90 €"), ("Net à payer", "32,87 €")]),
     "32.87", "EUR", "32,87 €"),
    ("fr_promo_code_discount", summary([("Sous-total", "35,00 €"), ("Code promo BIENVENUE", "−7,03 €"), ("Total", "27,97 €")]),
     "27.97", "EUR", "27,97 €"),
    ("fr_free_shipping_banner", '<p style="background:#efe;padding:8px">Livraison offerte pour toute commande supérieure à '
     '49 €</p>' + summary([("Sous-total", "27,97 €"), ("Livraison", "Calculée à l'étape suivante"), ("Total à payer", "27,97 €")]),
     "27.97", "EUR", "27,97 €"),
    ("en_free_shipping_over", '<p style="background:#efe;padding:8px">Free shipping on orders over €50</p>'
     + summary([("Subtotal", "€27.97"), ("Order total", "€27.97")]), "27.97", "EUR", "€27.97"),
    ("travel_total_for_passengers", summary([("Paris → Lyon, aller simple", "64,00 €"), ("Total pour 2 passagers :", "64,00 €")]),
     "64.00", "EUR", "64,00 €"),
    ("gift_card_applied", summary([("Total", "50,00 €"), ("Carte cadeau", "−22,03 €"), ("Reste à payer", "27,97 €")]),
     "27.97", "EUR", "27,97 €"),
    ("pharmacy_cross_sell", CSS + '<p>Crème solaire SPF50 <del>35,00 €</del> 27,97 €</p>'
     '<div style="border:1px solid #ccc;padding:8px;width:300px"><p>Vous aimerez aussi</p><div class="row"><span>Coffret soin '
     '39,90 €</span><button type="button">+ Ajouter</button></div></div>'
     + row("Sous-total", "27,97 €") + row("Livraison", "Offerte") + row("Total TTC", "27,97 €", "big")
     + '<p class="muted">dont TVA 1,47 €</p>', "27.97", "EUR", "27,97 €"),
    ("spa_scroll_shell_total_below", '<div style="height:100vh;overflow:auto"><div style="height:2200px">Étape 3/3 — Paiement'
     '</div>' + summary([("Sous-total", "27,97 €"), ("Livraison", "4,90 €"), ("Total à payer", "32,87 €")]) + '</div>',
     "32.87", "EUR", "32,87 €"),
    ("adyen_hosted", summary([("Total", "€32.87")]) + '<p><button type="button">Pay €32.87</button></p>',
     "32.87", "EUR", "€32.87"),
    ("lyra_hosted", '<div style="max-width:480px;margin:auto"><p>Boutique : Exemple</p><p>Référence de commande : 100045678</p>'
     '<p>Montant : 32,87 EUR</p></div>', "32.87", "EUR", "32,87 EUR"),
    ("total_label_under_amount", CSS + '<div style="width:300px"><p style="font-size:30px;margin:0">32,87 €</p>'
     '<p class="muted" style="margin:0">Total TTC</p></div>', "32.87", "EUR", "32,87 €"),
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
