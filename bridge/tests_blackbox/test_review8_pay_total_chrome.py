"""Relecture 8 of fix-pay-total-check (20/11, on bbc703ce4). h1–h4: pages that show a higher total than the approved
amount while the basket line reads « approved »; each asserts the SAFE outcome (not pressed) and fails on bbc703ce4.
The « agent » gives confirm_payment the total the bridge told it (the approved amount).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
from test_review_pay_total_chrome import APPROVED, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

BIG = {**APPROVED, "amount": "1199.00"}
BIG_BASKET = "<p>Montant de votre panier : 1 199,00 €</p><p>Frais de livraison 50,00 €</p>"
BASKET = "<p>Montant de votre panier : 27,97 €</p><p>Frais de livraison 4,90 €</p>"


def run(bridges, fakes, tmp_path, html, approved, stated):
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, approved)
        said = result["content"][0]["text"]
        if result["isError"] is False:
            confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": stated})["content"][0]["text"]
            pages = [u.split("/")[-1].split("?")[0] for u in browser.pages()]
            assert "moved on" not in confirmed and "done.html" not in pages, (
                f"approved {approved['amount']} EUR, the page asks more, « Payer » was pressed.\npay: {said}\n"
                f"confirm: {confirmed}")
    finally:
        browser.stop()
        shop.shutdown()


def test_h1_a_four_digit_total_with_sup_cents_is_dropped_as_a_card_group(bridges, fakes, tmp_path):
    # « 1249<sup>€00</sup> » (a French marketplace's price style): the text box « 1249 » is a « line of 4-digit groups »
    # for drop_card → removed; « €00 » is read 0,00 € → the basket line « approved »
    html = page(BIG_BASKET + '<p style="font-size:28px">Total <b>1249<sup style="font-size:55%">€00</sup></b></p>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_h2_a_four_digit_total_without_decimals_is_not_read_at_all(bridges, fakes, tmp_path):
    # WooCommerce with 0 decimals and no thousands mark: « <bdi>1249&nbsp;<span>€</span></bdi> »
    html = page(BIG_BASKET + '<p>Total <span class="amount"><bdi>1249&nbsp;<span class="symbol">€</span></bdi></span></p>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_h3_a_four_digit_integer_part_in_its_own_span(bridges, fakes, tmp_path):
    # the euros big, the cents small, each its own span (« 1249 » « ,00 € »)
    html = page(BIG_BASKET + '<p>Total <span style="font-size:28px">1249</span><span>,00 €</span></p>')
    run(bridges, fakes, tmp_path, html, BIG, "1 199,00 €")


def test_h4_a_promo_code_link_right_under_the_total_makes_it_a_monthly_price(bridges, fakes, tmp_path):
    # page_total.rs PER_AFTER: « €⏎Promo » reads as « € pro mo » (per month) → the real total is left out
    html = page(BASKET + '<p><b>Total à payer 32,87 €</b></p><p><a href="#promo">Promo code ?</a></p>')
    run(bridges, fakes, tmp_path, html, APPROVED, "27,97 €")
