"""Relecture 5 of fix-pay-total-check (19/11, on 9d7767289). e1–e3: pages that charge more and are pressed (fail on
9d7767289); e4: a captcha-like frame hands a good payment over (product side); e5: a heavy page (the closed-root DOM read
of every read; timing printed).
The « agent » gives confirm_payment the total the bridge told it (« its total is 27.97 EUR »).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import functools
import http.server
import threading
import time

from test_review_pay_total_chrome import APPROVED, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

APPROVED_TEXT = "the amount the person approved. Now call confirm_payment"
BASKET = "<p>Montant de votre panier : 27,97 €</p>"
SUMMARY = "<p>Frais de livraison 4,90 €</p><p><b>Total à payer 32,87 €</b></p>"


def pressed_if_typed(bridge, browser, result):
    said = result["content"][0]["text"]
    if result["isError"] is False:
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "27,97 €"})["content"][0]["text"]
        pages = [u.split("/")[-1].split("?")[0] for u in browser.pages()]
        assert "moved on" not in confirmed and "done.html" not in pages, (
            f"approved 27.97 EUR, the page asks 32,87 €, « Payer » was pressed.\npay: {said}\nconfirm: {confirmed}")


def run(bridges, fakes, tmp_path, html):
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        pressed_if_typed(bridge, browser, result)
    finally:
        browser.stop()
        shop.shutdown()


def test_e1_a_tiny_read_frame_makes_up_for_a_big_unread_one(bridges, fakes, tmp_path):
    # frames are counted, not matched: a hidden same-site helper frame (read: the page's own site) and the order summary
    # in a frame the bridge does not read (srcdoc) → 1 read ≥ 1 big → « all read »
    (tmp_path / "helper.html").write_text("<!doctype html><title>h</title>", encoding="utf-8")
    html = page(BASKET + '<iframe src="helper.html" width="1" height="1" style="border:0"></iframe>'
                f'<iframe width="400" height="120" srcdoc="{SUMMARY}"></iframe>')
    run(bridges, fakes, tmp_path, html)


def test_e2_a_frame_inside_a_shadow_root_is_neither_read_nor_counted(bridges, fakes, tmp_path):
    # `document.querySelectorAll("iframe")` does not look into shadow roots: the summary frame there is never counted
    script = f"""
      const root = document.getElementById("summary").attachShadow({{mode: "open"}});
      const f = document.createElement("iframe");
      f.width = 400; f.height = 120; f.srcdoc = "{SUMMARY}";
      root.appendChild(f);"""
    run(bridges, fakes, tmp_path, page(BASKET + '<div id="summary"></div>', script))


def test_e3_an_unlabelled_amount_in_a_closed_root_beside_its_label_is_ignored(bridges, fakes, tmp_path):
    # « Total à payer » in the page, the amount drawn by a closed web component: its text is read apart, with no label,
    # as hidden text → an unlabelled hidden amount blocks nothing
    script = """
      const root = document.getElementById("price").attachShadow({mode: "closed"});
      root.innerHTML = "<b>32,87 €</b>";"""
    html = page(BASKET + '<p>Frais de livraison 4,90 €</p><p><b>Total à payer <span id="price"></span></b></p>', script)
    run(bridges, fakes, tmp_path, html)


def test_e4_a_captcha_like_frame_without_any_amount_hands_a_good_payment_over(bridges, fakes, tmp_path):
    # reCAPTCHA v3's badge (256×60) or v2's box (304×78), hCaptcha, Turnstile, PayPal's buttons (≈750×45): a visible
    # frame of another site with no amount; here a frame of another local site of the same size
    other = tmp_path / "other"
    other.mkdir()
    (other / "badge.html").write_text("<!doctype html><p>protected by reCAPTCHA — Privacy - Terms</p>", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(other))
    handler.log_message = lambda *a: None
    side = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=side.serve_forever, daemon=True).start()
    html = page('<div><span>Total à payer:</span> <span>EUR 27.97</span></div>'
                f'<iframe src="http://localhost:{side.server_port}/badge.html" width="256" height="60"></iframe>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = c.pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and APPROVED_TEXT in said, said
    finally:
        browser.stop()
        shop.shutdown()
        side.shutdown()


def test_e5_a_heavy_page_is_still_read(bridges, fakes, tmp_path):
    # 40 000 elements without text (a big shop theme's checkout; text kept under pageText's 200 000-character cap, which
    # hands over by design), a closed widget; the total read before asking, under
    # the hold, and before the press: each read takes the whole DevTools tree
    script = """
      const box = document.getElementById("menu");
      let html = "";
      for (let i = 0; i < 40000; i++) html += "<i class='r'></i>";
      box.innerHTML = html;
      document.getElementById("w").attachShadow({mode: "closed"}).innerHTML = "<button>Aide</button>";"""
    html = page('<div id="menu" style="height:40px;overflow:hidden"></div><div id="w"></div>'
                '<div><span>Total à payer:</span> <span>EUR 27.97</span></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        started = time.monotonic()
        result, _ = c.pay(bridge, APPROVED)
        took = time.monotonic() - started
        said = result["content"][0]["text"]
        print(f"\npay_with_card on a 40 000-element page: {took:.1f} s")
        assert result["isError"] is False and APPROVED_TEXT in said, said
        started = time.monotonic()
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "27,97 €"})["content"][0]["text"]
        print(f"confirm_payment: {time.monotonic() - started:.1f} s")
        assert "moved on" in confirmed, confirmed
    finally:
        browser.stop()
        shop.shutdown()
