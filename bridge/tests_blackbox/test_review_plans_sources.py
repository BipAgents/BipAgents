"""Review of branch plans-sources (passation-pro briefs/review-plans-sources.md): what the review found, as tests
that fail on 250562991. None of them is a blocker (see the review); each names its point."""
import time

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_plans_sources import event, offer_cards, share_calendar, sources_home_place, sources_kept
from test_reply_due import add_all, mail, reply_place, turn_on


def test_review_c1_a_removed_mailbox_forgets_its_headers_and_its_watch(bridges, fakes):
    bridge, account = reply_place(bridges, fakes)
    turn_on(bridge, fakes)
    fakes.imap.add("INBOX", 101, mail("Marie Dupont <marie@example.org>", "Devis cuisine", "<m1@example.org>"))

    def kept():
        return [m for m in sources_kept(bridge).get("mails", []) if m["account"] == account]
    assert wait_until(kept, timeout=30), sources_kept(bridge)
    # the person removes the mailbox (Réglages › Boîtes mail): its watch stops, its headers should go with it
    assert bridge.delete(f"/v1/mail/accounts/{account}", headers=AUTH).status_code == 200
    time.sleep(3)
    left = sources_kept(bridge)
    assert kept() == [], f"headers kept after the mailbox was removed: {kept()}"
    assert [m["state"] for m in left.get("mailboxes", []) if m["account"] == account] in ([], ["off"]), left


@pytest.mark.skip(reason="review C2: a follow-up, not fixed in this branch (briefs/status-plans-sources.md, 24/11)")
def test_review_c2_a_moved_appointment_gets_its_card_the_day_before_its_new_day(bridges, fakes):
    bridge = sources_home_place(bridges, fakes)
    share_calendar(bridge, event("e1", "Dentiste", 4))
    assert wait_until(lambda: sources_kept(bridge).get("hints"), timeout=20), sources_kept(bridge)
    time.sleep(1)
    assert offer_cards(bridge) == []
    # moved to tomorrow (the same event): no exchange, so its card is due now, at its new day and time
    share_calendar(bridge, event("e1", "Dentiste", 1, hour=11))
    assert wait_until(lambda: offer_cards(bridge, source="calendar"), timeout=20), sources_kept(bridge)
    [card] = offer_cards(bridge, source="calendar")
    assert "11:00" in card["bip_offer"]["label"], card["bip_offer"]["label"]


def test_review_c4_one_odd_message_id_does_not_stop_every_reply_due(bridges, fakes):
    bridge, account = reply_place(bridges, fakes)
    turn_on(bridge, fakes)
    # anyone may send a Message-ID that is not ASCII: the Sent search can't send it, the whole look gives up
    add_all(fakes.imap, "INBOX", [
        (101, mail("Zoé <zoe@example.org>", "Bonjour", "<zoé.1@example.org>")),
        (102, mail("Paul <paul@example.net>", "Dîner samedi ?", "<p1@example.net>"))])
    assert wait_until(lambda: len([m for m in sources_kept(bridge).get("mails", [])]) == 2, timeout=30), \
        sources_kept(bridge)
    assert wait_until(lambda: offer_cards(bridge, source="replies"), timeout=20), sources_kept(bridge)
    [card] = offer_cards(bridge, source="replies")
    assert "Paul" in [i["from"] for i in card["bip_offer"]["items"]], card["bip_offer"]["items"]


def test_review_c5_mail_that_came_before_the_yes_is_never_read(bridges, fakes):
    bridge, account = reply_place(bridges, fakes)
    # the mailbox was watched once before (an e-mail routine of the person's, or a plan's e-mail watch), then not
    made = bridge.post("/v1/agents/coach/triggers", headers=AUTH,
                       json={"name": "Banque", "prompt": "Résume.", "kind": "mail",
                             "mail": {"account": account, "from": "banque.example"}})
    assert made.status_code == 200, made.text
    assert wait_until(lambda: fakes.imap.idling >= 1, timeout=20), fakes.imap.commands
    assert bridge.delete(f"/v1/triggers/{made.json()['id']}", headers=AUTH).status_code in (200, 204)
    assert wait_until(lambda: fakes.imap.idling == 0, timeout=30)
    # meanwhile, weeks of mail nobody watched
    fakes.imap.add("INBOX", 101, mail("Marie Dupont <marie@example.org>", "Devis cuisine", "<m1@example.org>"))
    # then the consent card's « Oui »: « from its newest message: nothing older is ever read » (the contract)
    turn_on(bridge, fakes)
    time.sleep(3)
    kept = [m["subject"] for m in sources_kept(bridge).get("mails", [])]
    assert kept == [], f"mail from before the « Oui » read and kept: {kept}"
