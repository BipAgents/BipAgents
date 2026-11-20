"""Relecture 3 of fix-pay-total-check (19/11, on a502ce997). Each test asserts the expected outcome; they fail on
a502ce997. c1: a page that charges more and is pressed; c2: a correct payment the bridge had verified, cancelled on
the agent's word; c3: the card's digits in the person's line; c4, c5: the product side effect (good payments stopped).
Same harness as test_pay_total_chrome.py (real Chromium, BIP_CHROME). Test card only (4242…), never a real shop."""
import json

from test_review_pay_total_chrome import APPROVED, never_pressed, page, start
import test_card_payments_chrome as c
from test_card_payments_chrome import Browser, pay, pay_bridge, site, turn_on, CARD
from bbkit.common import AUTH

pytestmark = c.pytestmark


def lines(bridge):
    path = bridge.data / "pay-lines.jsonl"
    return [json.loads(l)["text"] for l in path.read_text().splitlines() if l.strip()] if path.exists() else []


def test_c1_a_total_in_a_closed_shadow_root_is_never_read_and_the_page_is_approved(bridges, fakes, tmp_path):
    # a web component (closed shadow root) shows the final total; the basket line outside it says the approved amount.
    # The bridge says « its total is 27.97 EUR »; the agent gives that total as told; « Payer » is pressed.
    script = """
      const host = document.getElementById("summary");
      const root = host.attachShadow({mode: "closed"});
      root.innerHTML = "<p>Frais de livraison 4,90 €</p><p><b>Total à payer 32,87 €</b></p>";"""
    html = page('<p>Total panier 27,97 €</p><div id="summary"></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        if result["isError"] is False:
            never_pressed(bridge, browser, said)
    finally:
        browser.stop()
        shop.shutdown()


def test_c2_a_page_the_bridge_verified_is_not_cancelled_on_the_agents_misreading(bridges, fakes):
    # cawl.html: « Total à payer: EUR 27.97 », read « approved » by the bridge. The agent writes the subtotal it saw on
    # the shop before (24,50 €): the bridge cancels a correct payment and tells the person « le total lu sur la page
    # (24,50 €) n'est pas celui que tu avais accepté » — the 19/11 failure (a model's reading cancels a right payment),
    # with a false sentence about the page.
    shop = site()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/cawl.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, _ = pay(bridge, {**APPROVED})
        said = result["content"][0]["text"]
        assert "the amount the person approved" in said, said
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "24,50 €"})["content"][0]["text"]
        assert "moved on" in confirmed, (confirmed, lines(bridge))
    finally:
        browser.stop()
        shop.shutdown()


def test_c3_the_cards_digits_never_reach_the_persons_line_nor_the_log(bridges, fakes, tmp_path):
    # a card preview in a flex row (each group its own block for pageText: no 12-digit run to drop), right under the
    # amount; it fills once the card is typed, so the read right before the press sees « 27,97 € » then « 4242 »…
    script = """
      setInterval(() => {
        const v = document.getElementById("numero").value.replace(/\\D/g, "");
        const groups = v.match(/.{1,4}/g) || [];
        document.getElementById("preview").innerHTML = groups.map((g) => "<span>" + g + "</span>").join("");
      }, 50);"""
    # (Correction 8: « approved » needs a labelled total, so the amount is labelled; the point stays the card's digits)
    html = page('<div>Total à payer 27,97 €</div><div id="preview" style="display:flex;gap:8px"></div>', script)
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        assert result["isError"] is False, result
        confirmed = c.tool(bridge, "confirm_payment", {"button": "Payer", "total": "27,97 €"})["content"][0]["text"]
        log = bridge.log_path.read_text(encoding="utf-8", errors="replace")
        said = " ".join(lines(bridge)) + " " + confirmed
        assert "4242" not in said.replace(" ", "").replace(" ", "").replace(".", "").replace(",", ""), said
        assert "424200" not in log, [l for l in log.splitlines() if "424200" in l]
        assert "moved on" in confirmed, confirmed
    finally:
        browser.stop()
        shop.shutdown()


def test_c4_a_struck_out_old_price_does_not_stop_a_good_payment(bridges, fakes, tmp_path):
    # one item on promotion, its old price struck out (pharmacies, Cdiscount…), above the order's total
    html = page('<p>Crème solaire SPF50 <del>35,00 €</del> 27,97 €</p><p>Livraison offerte</p>'
                '<p><b>Total à payer 27,97 €</b></p>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and "the amount the person approved" in said, (said, lines(bridge))
    finally:
        browser.stop()
        shop.shutdown()


def test_c5_a_hidden_menu_with_prices_does_not_stop_a_good_payment(bridges, fakes, tmp_path):
    # the theme's mega-menu / cart drawer, laid out but hidden (visibility: hidden), with product prices in it
    html = page('<nav style="visibility:hidden;position:absolute;top:0"><p>Coffret soin 49,90 €</p>'
                '<p>Crème de nuit 39,90 €</p></nav><p><b>Total à payer 27,97 €</b></p>')
    shop, browser, bridge = start(bridges, fakes, tmp_path, html)
    try:
        result, _ = pay(bridge, APPROVED)
        said = result["content"][0]["text"]
        assert result["isError"] is False and "the amount the person approved" in said, (said, lines(bridge))
    finally:
        browser.stop()
        shop.shutdown()
