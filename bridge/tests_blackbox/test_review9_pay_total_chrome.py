"""Relecture 9 of fix-pay-total-check (20/11, on 1fe82c895). k1–k4: pages that show a higher total than the approved
amount; each asserts the SAFE outcome (not pressed) and fails on 1fe82c895 (rules added by Correction 9).
The « agent » gives confirm_payment the total the bridge told it (the approved amount). Real Chromium (BIP_CHROME).
Test card only (4242…), never a real shop."""
from test_review_pay_total_chrome import APPROVED, page, start
from test_review8_handover_chrome import CSS, row
from test_review8_pay_total_chrome import BIG, BIG_BASKET, run

pytestmark = __import__("test_card_payments_chrome").pytestmark


def test_k1_dutch_te_betalen_saldo_is_read_as_an_available_balance(bridges, fakes, tmp_path):
    # page_total.rs MENTION « saldo » (meant for « saldo disponible »), checked before TO_PAY: the amount due is ignored
    html = page(CSS + '<div style="width:360px">' + row("Totaal", "€ 27,97") + row("Transactiekosten", "€ 0,50")
                + row("Te betalen saldo", "€ 28,47", "big") + "</div>")
    run(bridges, fakes, tmp_path, html, APPROVED, "€ 27,97")


def test_k2_german_offener_saldo(bridges, fakes, tmp_path):
    html = page(CSS + '<div style="width:360px">' + row("Gesamtsumme", "27,97 €") + row("Bearbeitungsgebühr", "0,50 €")
                + row("Offener Saldo:", "28,47 €", "big") + "</div>")
    run(bridges, fakes, tmp_path, html, APPROVED, "27,97 €")


def test_k3_a_bottom_sheets_close_button_right_before_the_total_makes_it_a_quantity(bridges, fakes, tmp_path):
    # « Récapitulatif [×] » then the big « 1 249,00 € » (label under it): the quantity rule reads 249,00 €
    html = page(BIG_BASKET + '<div style="width:340px;border:1px solid #ccc;padding:8px">'
                '<div style="display:flex;justify-content:space-between"><b>Récapitulatif</b>'
                '<button type="button" aria-label="Fermer">×</button></div>'
                '<p style="font-size:28px;margin:4px 0 0">1 249,00 €</p><p style="margin:0;color:#666">Total TTC</p></div>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_k4_an_integer_total_with_its_euro_sign_and_ttc_under_it(bridges, fakes, tmp_path):
    # a marketplace price block: big « 1249 », small « € TTC » on the line under it: the € no longer counts (it shares
    # its line with words), « 1249 » has no cents → no amount at all
    html = page(BIG_BASKET + '<div style="display:flex;justify-content:space-between;width:340px">'
                '<span>Total à payer</span><span style="text-align:right"><span style="font-size:28px">1249</span><br>'
                '<small>€ TTC</small></span></div>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")
