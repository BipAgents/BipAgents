"""Relecture 10 of fix-pay-total-check (20/11, on f4d65a5bb). n1–n4: pages that show a higher total than the approved
amount; each asserts the SAFE outcome (not pressed) and fails on f4d65a5bb.
The « agent » gives confirm_payment the total the bridge told it (the approved amount). Real Chromium (BIP_CHROME).
Test card only (4242…), never a real shop."""
from test_review_pay_total_chrome import page
from test_review8_handover_chrome import CSS
from test_review8_pay_total_chrome import BIG, run

pytestmark = __import__("test_card_payments_chrome").pytestmark

BASKET_ONLY = "<p>Montant de votre panier : 1 199,00 €</p>"


def test_n1_a_quantity_column_before_a_thousands_total_reads_only_its_last_group(bridges, fakes, tmp_path):
    # an order table with a quantity column: the row reads « Total 2 1 249,00 € »; amounts() takes the group next to
    # the mark (« 249,00 ») when the whole number doesn't read (Relecture 7's « Qté 2 27,97 € » exception), so the
    # French thousands space cuts the total to 249,00 € — below the approved 1 199,00 → « approved » → pressed
    html = page(CSS + BASKET_ONLY + '<table><thead><tr><th>Article</th><th>Qté</th><th>Montant</th></tr></thead><tbody>'
                '<tr><td>Téléviseur</td><td>1</td><td>1 199,00 €</td></tr>'
                '<tr><td>Livraison</td><td>1</td><td>50,00 €</td></tr>'
                '<tr><th>Total</th><th>2</th><th>1 249,00 €</th></tr></tbody></table>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_n2_a_footnote_mark_on_the_total_label_cuts_its_thousands(bridges, fakes, tmp_path):
    # « Prix total¹ 1 249,00 € »: the footnote digit starts the number (« 1 1 249,00 ») → read 249,00 €
    html = page(CSS + BASKET_ONLY + '<p>Frais de livraison 50,00 €</p>'
                '<p style="font-size:22px">Prix total<sup>1</sup> 1 249,00 €</p>'
                '<p class="muted"><sup>1</sup> TVA incluse</p>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_n3_the_euro_ttc_line_under_an_integer_total_carries_the_vat_rate(bridges, fakes, tmp_path):
    # k4 again (big « 1249 », small « € TTC » under it), the small line saying its VAT rate: the orphan rule wants a
    # mark line with no digit → « 1249 » is no amount at all → « approved » on the basket → pressed
    html = page(CSS + BASKET_ONLY + '<p>Frais de livraison 50,00 €</p>'
                '<div style="display:flex;justify-content:space-between;width:340px">'
                '<span>Total à payer</span><span style="text-align:right"><span style="font-size:28px">1249</span><br>'
                '<small>€ TTC (TVA 20 %)</small></span></div>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_n4_the_label_line_above_an_integer_total_counts_its_items(bridges, fakes, tmp_path):
    # « Total TTC en € (2 articles) » then the big « 1249 »: the line above has a digit → no orphan → not read
    html = page(CSS + BASKET_ONLY + '<p>Frais de livraison 50,00 €</p>'
                '<div style="width:340px"><p style="margin:0">Total TTC en € (2 articles)</p>'
                '<p style="font-size:28px;margin:0">1249</p></div>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")
