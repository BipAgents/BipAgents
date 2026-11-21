"""Correction 11 (Relecture 10, item 5): a 3000-level deep page (built by script: the HTML parser stops nesting at 512),
each level a priced line inside a folded menu, the total « EUR 27.97 » beside the form. A good payment. The bridge must
answer in bounded time: approved and pressed, or a clean hand-over (« could not check »), never a hang nor a browser
error with the card left half typed. Prints the times and the answers (run against main too: what today's flow does).
Real Chromium, test card."""
import time

from test_review_pay_total_chrome import APPROVED, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

DEPTH = 3000
SCRIPT = f"""
  let at = document.getElementById("menu");
  for (let i = 0; i < {DEPTH}; i++) {{
    const d = document.createElement("div");
    const s = document.createElement("span");
    s.textContent = "Article " + i + " 1,00 €";
    d.appendChild(s);
    at.appendChild(d);
    at = d;
  }}"""


def test_t3_a_3000_deep_page_pays_or_hands_over_in_bounded_time(bridges, fakes, tmp_path):
    html = page('<div id="menu" style="height:40px;overflow:hidden"></div>'
                '<div><span>Total à payer:</span> <span>EUR 27.97</span></div>', SCRIPT)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        started = time.monotonic()
        result, _ = c.pay(bridge, APPROVED)
        took = time.monotonic() - started
        said = result["content"][0]["text"]
        print(f"\npay_with_card on a {DEPTH}-deep page: {took:.1f} s, isError={result['isError']}\n{said[:300]}")
        confirmed = ""
        if result["isError"] is False and "the amount the person approved" in said:
            started = time.monotonic()
            confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "27,97 €"})["content"][0]["text"]
            print(f"confirm_payment: {time.monotonic() - started:.1f} s\n{confirmed[:200]}")
        pages = [u.split("/")[-1].split("?")[0] for u in browser.pages()]
        print(f"pages: {pages}")
        assert took < 30, f"pay_with_card took {took:.1f} s"
        # Measured 20/11 on main (no total check) and on this branch alike: Chromium is still laying the page out, the
        # bridge's first call (Page.getFrameTree, before any reading) gets no answer in 10 s → « Your browser did not
        # answer as expected », nothing typed, not pressed. Not a regression of the check. Here: bounded, and pressed only
        # after « approved ».
        # (browser.values() itself gets no answer from that page: the stop came before any typing)
        assert "moved on" in confirmed or ("done.html" not in pages and result["isError"] is True), said
    finally:
        browser.stop()
        shop.shutdown()
