"""The release battery's cards lab FAKE shop (scripts/release-battery/cards_shop/: its own shop.py and pages, as
cards_lab.py runs them in the agent's browser) in a real Chromium, through the watcher's urgent payment: the bridge
reads the shop's payment page « approved » for the lab's 45,00 €, so the payment is ASKED (a `bip-pay-` card, the
urgent push), and with the code the card is typed, pressed once and accepted by the shop (its own check, card=ok).

Battery of 21/11 on 4f0ea6832: « personne » and « code » red, no urgent payment card in 15 minutes. Since
fix-pay-total-check (Correction 3, 0b9d8ca94) a page whose total is not read « approved » for sure is handed over
before the person is asked; the lab's page said only « Paiement : 45,00 € » (no total's label), while its copy among
the checkout fixtures (selects.html) got its « Total à payer » then. This test keeps the lab's page and the bridge's
reading of totals in step before a lab server is paid for.

Needs a Chromium on this machine (`BIP_CHROME`…): skipped without one. Only Visa's published test number is used."""
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

import httpx
import pytest

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from test_card_payments import CARD, CODE, NUMBER, tool
from test_card_payments_chrome import CHROME, Browser, pytestmark  # noqa: F401 (skipped without Chromium)
from test_watcher_urgent_pay import ARGS, HermesSide, answered, approvals, setup, unattended_ask, urgent_pushes

LAB_SHOP = Path(__file__).resolve().parents[2] / "scripts" / "release-battery" / "cards_shop"
# The public repository has no release battery (its cards lab shop stays private): skipped there.
pytestmark = [*(pytestmark if isinstance(pytestmark, list) else [pytestmark]),
              pytest.mark.skipif(not LAB_SHOP.is_dir(), reason="no release-battery/cards_shop (public export)")]
PRICE = "45,00"   # cards_lab.py PRICE: the watcher's purchase, the amount the person approves


class LabShop:
    """The lab's shop.py on a port of its own, from a copy (it writes requests.log and reads stock.txt beside it)."""

    def __init__(self, tmp_path):
        self.dir = tmp_path / "shop"
        shutil.copytree(LAB_SHOP, self.dir)
        (self.dir / "stock.txt").write_text("en stock\n")
        port = free_port()
        self.proc = subprocess.Popen([sys.executable, str(self.dir / "shop.py"), str(port)],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.base = f"http://127.0.0.1:{port}"
        assert wait_until(lambda: httpx.get(self.base + "/produit.html", timeout=2).status_code == 200, timeout=20)

    def log(self):
        path = self.dir / "requests.log"
        return path.read_text().splitlines() if path.exists() else []

    def stop(self):
        self.proc.terminate()
        self.proc.wait(10)


def test_the_lab_shops_payment_page_is_asked_urgently_and_paid_once_with_the_code(bridges, fakes, tmp_path):
    shop = LabShop(tmp_path)
    browser = Browser(shop.base + "/produit.html")
    try:
        assert "En stock" in httpx.get(shop.base + "/produit.html").text
        bridge, _ = setup(bridges, fakes, chrome=browser)
        made = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "label": "Billets", "sites": ["127.0.0.1"]})
        assert made.status_code == 201, made.text
        # the watcher's turn: the agent's browser on the shop's payment page (cart and delivery done), pay_with_card
        browser.open(shop.base + "/paiement.html")
        side = HermesSide(bridge, {**ARGS, "card": "billets", "amount": PRICE})
        assert wait_until(lambda: side.got_question.is_set() or side.done.is_set(), timeout=60), "pay_with_card hangs"
        assert side.asked, (f"the lab shop's payment page was handed over before the person was asked (its total not "
                            f"read « approved »): {side.result}")
        payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", side.asked).group(1)
        gate = unattended_ask(fakes, bridge, side.asked, "r1")
        assert wait_until(lambda: answered(fakes, "r1") == ["once"], timeout=20), fakes.hermes.approvals
        side.answer("accept")
        # what cards_lab.py's « personne » and « code » wait for: the urgent card, and its push
        assert wait_until(lambda: [c for c in approvals(bridge) if c["run_id"] == f"bip-pay-{payment}"], timeout=10)
        assert wait_until(lambda: urgent_pushes(fakes), timeout=10)
        [push] = urgent_pushes(fakes)
        assert push["aps"]["interruption-level"] == "time-sensitive" and push["run_id"] == f"bip-pay-{payment}"
        # « Oui, payer » with the code: typed into the lab's form (two selects for the expiry), pressed once
        assert bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json={"code": CODE}).status_code == 204
        yes = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": f"bip-pay-{payment}",
                                                             "request_id": payment, "choice": "once"})
        assert yes.status_code == 200, yes.text
        assert side.done.wait(60) and side.result["isError"] is False, side.result
        said = side.result["content"][0]["text"]
        assert "the amount the person approved. Now call confirm_payment" in said, said
        values = browser.values()["paiement.html"]
        assert values["numero"] == NUMBER and values["crypto"] == CODE and values["mois"] == "12", values
        confirmed = tool(bridge, "confirm_payment", {"button": "Valider le paiement", "total": f"{PRICE} €"})
        assert "moved on" in confirmed["content"][0]["text"], confirmed
        # the shop's own verdict: the test card accepted once, its confirmation shown once (cards_lab.py `confirmations`)
        assert wait_until(lambda: "POST /payer card=ok" in shop.log(), timeout=10), shop.log()
        time.sleep(1)
        log = shop.log()
        assert log.count("POST /payer card=ok") == 1 and "POST /payer card=bad" not in log, log
        assert len([x for x in log if x.split(" ")[:2] == ["GET", "/done.html"] and "unpaid" not in x]) == 1, log
        gate.set()
    finally:
        browser.stop()
        shop.stop()
