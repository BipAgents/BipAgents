"""Review of branch plans-cards (passation-pro briefs/review-plans-cards.md): its blockers as tests that fail on
3924c6f40 (no fix here).

B1 (plans/sources.rs `consent_cards`, from plans-sources e60e75831, merged by plans-cards): a mailbox's consent card is
visible (offers.rs `place_bridge` saves it) before its push is sent; `consent_cards` then writes the mailbox `asked` AFTER
`place_bridge(...).await`. A « Oui » given while the push is still going (`watch_on`: `on`, its trigger) is overwritten:
the mailbox is `asked` again, its trigger forgotten by the store but still running. The person said yes, nothing comes,
the switch shows off, and switching it off can't remove that trigger (`watch_off` reads the store's trigger: none). Seen
under load in test_review_plans_sources.py (c1 then c4 red, two parallel runs in a row, 24/11)."""
import asyncio
import time

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_mailbox import PASSWORD, add_mailbox
from test_plans import answer, place
from test_plans_sources import SOURCES, offer_cards, sources_kept
from test_reply_due import ME
from test_triggers import FAST

PUSH_SECONDS = 5


def test_review_b1_a_yes_given_while_the_consent_card_is_still_pushed_is_kept(bridges, fakes):
    def change(c):
        c["plans_sources"] = dict(SOURCES)
        c["triggers"] = dict(FAST)

    bridge = place(bridges, fakes, change=change)
    fakes.imap.users[ME] = PASSWORD

    async def slow_push(seen):
        # APNs answers late (a slow network, a relay): the card is already on the phones meanwhile
        if "/3/device/" in seen.path:
            await asyncio.sleep(PUSH_SECONDS)
        return None

    fakes.apns.override = slow_push
    try:
        added = add_mailbox(bridge, fakes)
        assert added.status_code == 201, added.text
        account = added.json()["id"]
        assert wait_until(lambda: offer_cards(bridge, source="replies"), timeout=20), sources_kept(bridge)
        [card] = offer_cards(bridge, source="replies")
        # the person reads the card in the app and says « Oui » before the push has gone
        yes = answer(bridge, card, "once")
        assert yes.status_code == 200 and yes.json().get("kind") == "reply_watch", yes.text
        time.sleep(PUSH_SECONDS + 2)   # the push ends, the consent pass goes on after it
    finally:
        fakes.apns.override = None
    kept = [m["state"] for m in sources_kept(bridge).get("mailboxes", []) if m["account"] == account]
    assert kept == ["on"], f"the « Oui » was lost: {sources_kept(bridge)}"
    shown = bridge.get("/v1/agents/coach/replies", headers=AUTH).json()
    assert [m["state"] for m in shown["mailboxes"] if m["account"] == account] == ["on"], shown
