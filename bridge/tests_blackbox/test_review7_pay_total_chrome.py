"""Relecture 7 of fix-pay-total-check (20/11, on 9b87714a5): the reader on DOMSnapshot.captureSnapshot (page_snapshot.rs).
g1–g6: pages that show a higher total than the approved 27,97 € while « Montant de votre panier : 27,97 € » reads
« approved »; each asserts the SAFE outcome (not pressed). g7–g9: controls / product side.
The « agent » gives confirm_payment the total the bridge told it (« its total is 27.97 EUR »).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import functools
import http.server
import threading

from test_review_pay_total_chrome import APPROVED, page, start
from test_review5_pay_total_chrome import BASKET, run
import test_card_payments_chrome as c

pytestmark = c.pytestmark

SHIPPING = "<p>Frais de livraison 4,08 €</p>"


def test_g1_raised_cents_with_a_small_margin_are_read_as_the_cents_alone(bridges, fakes, tmp_path):
    # « 32⁰⁵ € » as many themes draw the big total: flex top alignment, the cents a little apart (margin-left 2px), no
    # comma. The boxes don't touch → « 32 05 € » → the last group read: 5,00 € → under the approved amount
    html = page(BASKET + SHIPPING + '<div><b>Total à payer</b> <span style="display:inline-flex;align-items:flex-start">'
                '<span style="font-size:32px">32</span><span style="font-size:14px;margin-left:2px">05</span>'
                '<span style="font-size:14px;margin-left:2px">€</span></span></div>')
    run(bridges, fakes, tmp_path, html)


def test_g2_sup_cents_with_a_margin_are_read_as_the_cents_alone(bridges, fakes, tmp_path):
    # the same with <sup> (« 32<sup>05</sup> € », `sup { margin-left: .1em }`), no flex
    html = page('<style>sup{font-size:55%;margin-left:.15em}</style>' + BASKET + SHIPPING
                + '<p style="font-size:28px"><b>Total à payer 32<sup>05</sup> €</b></p>')
    run(bridges, fakes, tmp_path, html)


def test_g3_a_total_drawn_as_svg_text(bridges, fakes, tmp_path):
    # an order summary drawn in SVG (<text>): the old script read its text nodes; does the snapshot give its boxes?
    html = page(BASKET + SHIPPING + '<svg width="360" height="40"><text x="0" y="28" font-size="22">'
                'Total à payer 32,87 €</text></svg>')
    run(bridges, fakes, tmp_path, html)


def test_g4_a_total_one_character_per_box_a_pixel_apart(bridges, fakes, tmp_path):
    # an odometer-like total (each character its own inline-block, 1 px apart) under « Votre commande »
    digits = "".join(f'<span style="display:inline-block;margin-right:1px">{ch}</span>' for ch in "32,87€")
    html = page(BASKET + '<p>Votre commande</p><p style="font-size:28px">' + digits + '</p>')
    run(bridges, fakes, tmp_path, html)


def test_g5_a_summary_card_with_rounded_clip_path_is_not_shown(bridges, fakes, tmp_path):
    # `clip-path: inset(0 round 12px)` (rounded corners) on the summary card: every clip-path makes its text « not shown »,
    # so its unlabelled big amount is ignored
    html = page(BASKET + '<div style="clip-path:inset(0 round 12px);background:#eef;padding:12px">'
                '<p>Votre commande</p><p style="font-size:28px">32,87 €</p></div>')
    run(bridges, fakes, tmp_path, html)


def test_g6_a_fixed_bar_outside_a_short_overflow_hidden_wrapper_is_not_shown(bridges, fakes, tmp_path):
    # `#page { overflow-x: hidden }` (to stop sideways scrolling) and a fixed bottom bar inside it: the bar is drawn at
    # the viewport's bottom, outside the short wrapper's box → « clipped » → not shown
    html = page('<div style="overflow-x:hidden">' + BASKET
                + '<div style="position:fixed;bottom:0;left:0;right:0;background:#fff;font-size:24px">'
                'Votre commande 32,87 €</div></div>')
    run(bridges, fakes, tmp_path, html)


def test_g7_control_labels_and_amounts_in_two_columns_block(bridges, fakes, tmp_path):
    # control: labels in one column, amounts in another (DOM order: every label, then every amount)
    html = page(BASKET + '<div style="display:flex;gap:24px"><div><p>Sous-total</p><p>Livraison</p><p>Total à payer</p>'
                '</div><div><p>27,97 €</p><p>4,90 €</p><p>32,87 €</p></div></div>')
    run(bridges, fakes, tmp_path, html)


def test_g8_control_a_frame_of_another_process_inside_the_shops_srcdoc_frame(bridges, fakes, tmp_path):
    # control: the summary in another site's frame (another process) inside the shop's own srcdoc frame
    other = tmp_path / "other"
    other.mkdir()
    (other / "sum.html").write_text("<!doctype html><meta charset=utf-8><p>Total à payer 32,87 €</p>", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(other))
    handler.log_message = lambda *a: None
    side = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=side.serve_forever, daemon=True).start()
    inner = f"<iframe src='http://localhost:{side.server_port}/sum.html' width='400' height='120'></iframe>"
    html = page(BASKET + f'<iframe width="460" height="160" srcdoc="{inner}"></iframe>')
    try:
        run(bridges, fakes, tmp_path, html)
    finally:
        side.shutdown()


def test_g9_product_a_screen_reader_twin_of_the_right_total_is_approved(bridges, fakes, tmp_path):
    # product: the drawn price aria-hidden, its screen-reader twin clipped (Amazon-like `a-offscreen`): a right page
    sr = "position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap"
    html = page(f'<p>Total à payer <span aria-hidden="true">27,97 €</span><span style="{sr}">27,97 €</span></p>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and "the amount the person approved" in said, said
    finally:
        browser.stop()
        shop.shutdown()


def test_g10_the_new_total_under_a_struck_old_total_is_a_mention(bridges, fakes, tmp_path):
    # a promo total on two rows: the old total struck out, the new one big under it. The new one has no label of its
    # own on its row, so the row above is taken as its label; it holds « [barré] » → the real total is a « mention »
    html = page(BASKET + '<p>Frais de livraison 4,90 €</p><p>Total à payer</p><p><del>35,00 €</del></p>'
                '<p style="font-size:28px;margin:0"><b>32,87 €</b></p>')
    run(bridges, fakes, tmp_path, html)


def test_g11_a_right_floated_total_under_a_struck_delivery_fee(bridges, fakes, tmp_path):
    # Bootstrap-like summary rows, the amount floated right and first in the DOM; the delivery fee struck (« offerte »)
    rows = ('<ul style="width:360px;list-style:none;padding:0">'
            '<li><span style="float:right"><del>4,90 €</del> Offerte</span>Livraison</li>'
            '<li><span style="float:right"><b>32,87 €</b></span><b>Total à payer</b></li></ul>')
    run(bridges, fakes, tmp_path, page(BASKET + rows))
