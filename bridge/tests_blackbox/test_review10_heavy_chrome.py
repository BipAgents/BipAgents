"""Relecture 10 of fix-pay-total-check (20/11, on f4d65a5bb), robustness: the reader's cost grows with the page's DEPTH
(page_snapshot.rs walks every ancestor of every node, and of every text piece several times: out_of_reach, struck,
hidden_by_ancestor with a format! per ancestor, in_scroller). e5 (40 000 elements) is flat and without text. Here: a
deep tree (built by script: the HTML parser stops nesting at 512) whose levels each hold a priced line, all inside a
folded menu. A good payment; prints the times, fails only past 30 s for pay_with_card (the agent's tool call).
Real Chromium, test card."""
import time

from test_review_pay_total_chrome import APPROVED, page, start
import test_card_payments_chrome as c

pytestmark = c.pytestmark

DEPTH = 1000


def test_t1_a_deep_page_with_priced_lines_is_read_in_time(bridges, fakes, tmp_path):
    script = f"""
      let at = document.getElementById("menu");
      for (let i = 0; i < {DEPTH}; i++) {{
        const d = document.createElement("div");
        const s = document.createElement("span");
        s.textContent = "Article " + i + " 1,00 €";
        d.appendChild(s);
        at.appendChild(d);
        at = d;
      }}"""
    html = page('<div id="menu" style="height:40px;overflow:hidden"></div>'
                '<div><span>Total à payer:</span> <span>EUR 27.97</span></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        started = time.monotonic()
        result, _ = c.pay(bridge, APPROVED)
        took = time.monotonic() - started
        said = result["content"][0]["text"]
        print(f"\npay_with_card on a {DEPTH}-deep page with priced lines: {took:.1f} s\n{said[:200]}")
        started = time.monotonic()
        if result["isError"] is False:
            confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "27,97 €"})["content"][0]["text"]
            print(f"confirm_payment: {time.monotonic() - started:.1f} s\n{confirmed[:120]}")
        assert took < 30, f"pay_with_card took {took:.1f} s"
    finally:
        browser.stop()
        shop.shutdown()
