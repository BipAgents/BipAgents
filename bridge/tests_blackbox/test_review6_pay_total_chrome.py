"""Relecture 6 of fix-pay-total-check (19/11, on ea52bd0fa). f1–f7: pages that show a higher total than the approved
27,97 € in a way the bridge's reader does not see, while « Montant de votre panier : 27,97 € » reads « approved »:
the card is typed and « Payer » pressed. Each test asserts the SAFE outcome (not pressed).
The « agent » gives confirm_payment the total the bridge told it (« its total is 27.97 EUR »).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import time

from test_review_pay_total_chrome import APPROVED, page, start
from test_review5_pay_total_chrome import BASKET, pressed_if_typed, run
import test_card_payments_chrome as c

pytestmark = c.pytestmark

SHIPPING = "<p>Frais de livraison 4,08 €</p>"


def test_f1_a_total_whose_cents_are_a_flex_item_is_read_as_its_cents(bridges, fakes, tmp_path):
    # « 32,05 € » drawn as two flex items (« 32 » and « ,05 € »: superscript cents by flex alignment, a common theme
    # style for the big final total): flex items are blocks for pageText, so two lines; « 32 » is no amount and
    # « ,05 € » reads 5,00 € → below the approved amount → « approved » on the basket line
    html = page(BASKET + SHIPPING + '<div><b>Total à payer</b> <span style="display:inline-flex;align-items:flex-start">'
                '<span style="font-size:32px">32</span><span style="font-size:14px">,05 €</span></span></div>')
    run(bridges, fakes, tmp_path, html)


def test_f2_a_total_shown_in_a_read_only_field_is_never_read(bridges, fakes, tmp_path):
    # older shop and bank pages show the amount in a read-only field; pageText never reads a field's value
    html = page(BASKET + '<p>Frais de livraison 4,90 €</p>'
                '<p><label>Total à payer <input readonly value="32,87 €" style="border:0;font-weight:bold"></label></p>')
    run(bridges, fakes, tmp_path, html)


def test_f3_a_closed_root_past_2000_texts_is_read_only_up_to_them(bridges, fakes, tmp_path):
    # card_fill.rs closed_texts keeps 2 000 text nodes and drops the rest without saying so: the order summary as a
    # closed web component with its lines, its total at the end
    script = """
      const root = document.getElementById("summary").attachShadow({mode: "closed"});
      let html = "";
      for (let i = 0; i < 2100; i++) html += "<div>Article réf. " + i + "</div>";
      root.innerHTML = "<div style='height:200px;overflow:auto'>" + html + "</div><p><b>Total à payer 32,87 €</b></p>";"""
    run(bridges, fakes, tmp_path, page(BASKET + '<div id="summary"></div>', script))


def test_f4_a_total_drawn_by_css_from_an_attribute_is_never_read(bridges, fakes, tmp_path):
    # `::after { content: attr(data-amount) }`: the computed `content` is « attr(data-amount) », not a quoted string
    html = page('<style>.amount::after{content: attr(data-amount)}</style>' + BASKET + SHIPPING
                + '<p><b>Total à payer <span class="amount" data-amount="32,87 €"></span></b></p>')
    run(bridges, fakes, tmp_path, html)


def test_f5_a_sandboxed_one_line_frame_is_neither_read_nor_counted(bridges, fakes, tmp_path):
    # a sandboxed frame (no scripts) of one line, 36 px high: just under the 120×40 « real size »
    summary = "<p style='margin:0;font:16px sans-serif'><b>Total à payer 32,87 €</b></p>"
    html = page(BASKET + f'<iframe sandbox width="400" height="36" style="border:0" srcdoc="{summary}"></iframe>')
    run(bridges, fakes, tmp_path, html)


def test_f6_a_total_in_a_closed_details_is_never_read(bridges, fakes, tmp_path):
    # mobile checkouts fold the order summary (« Récapitulatif ») with its labelled total
    html = page(BASKET + '<details><summary>Récapitulatif de la commande</summary>' + SHIPPING
                + '<p><b>Total à payer 32,87 €</b></p></details>')
    run(bridges, fakes, tmp_path, html)


def test_f7_a_total_under_content_visibility_auto_is_never_read(bridges, fakes, tmp_path):
    # `content-visibility: auto` (a long page's rendering shortcut): off-screen, its text has no layout box
    html = page(BASKET + '<div style="height:3000px"></div><section style="content-visibility:auto;'
                'contain-intrinsic-size:auto 200px">' + SHIPPING + '<p><b>Total à payer 32,87 €</b></p></section>')
    run(bridges, fakes, tmp_path, html)


def test_f8_a_sandboxed_frame_of_real_size_without_amount_hands_a_good_payment_over(bridges, fakes, tmp_path):
    # product side: an ad or a trust badge in a sandboxed frame (no scripts allowed) of real size
    html = page('<div><span>Total à payer:</span> <span>EUR 27.97</span></div>'
                '<iframe sandbox width="300" height="100" srcdoc="<p>Avis clients 4,8/5</p>"></iframe>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and "the amount the person approved" in said, said
    finally:
        browser.stop()
        shop.shutdown()


def test_f9_timing_of_a_page_with_many_priced_lines(bridges, fakes, tmp_path):
    # 8 000 priced lines (a long order, deep nesting, scrolled boxes): every read walks each line's ancestors
    script = """
      let html = "";
      for (let i = 0; i < 8000; i++) {
        let line = "<span>Article " + i + " : 1,00 €</span>";
        for (let d = 0; d < 25; d++) line = "<div style='overflow:hidden'>" + line + "</div>";
        html += line;
      }
      document.getElementById("lines").innerHTML = html;"""
    html = page('<div id="lines" style="height:100px;overflow:auto"></div>'
                '<div><span>Total à payer:</span> <span>EUR 27.97</span></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        started = time.monotonic()
        result, _ = c.pay(bridge, APPROVED)
        print(f"\npay_with_card, 8 000 priced lines 25 deep: {time.monotonic() - started:.1f} s")
        print(result["content"][0]["text"][:300])
    finally:
        browser.stop()
        shop.shutdown()


def test_f10_the_shops_own_srcdoc_frame_counts_as_another_sites(bridges, fakes, tmp_path):
    # a srcdoc / about:blank frame is the shop's own document (same origin), yet read as another site's text: its
    # visible unlabelled amount (the label in the page beside it) is ignored
    html = page(BASKET + SHIPPING + '<p><b>Total à payer</b></p>'
                '<iframe width="240" height="60" style="border:0" srcdoc="<b style=\'font-size:28px\'>32,05 €</b>"></iframe>')
    run(bridges, fakes, tmp_path, html)
