"""The household that scans an invitation through the relay takes the inviter's calls as soon as it gives its pass
(20/11: test_review7_relais_famille's withdrawal failed 1 run in 25 alone, 6 in 25 under load).

The relay keeps what a mailbox says it accepts (`POST /v1/family/passes`) only once that mailbox polls; until then the
statement is dropped (relay/ family.rs `take_from`). The scanning bridge turned its poll on first, then made its pass:
its first poll, numbered before the pass, said « none », and when the pass's own statement reached the relay before
that poll, it was dropped. The mailbox then took no pass for a whole poll (25 s): the inviter's calls were refused as
« away » (its new pass after a link removed never got through, given again only a minute later).

Here the relay is slowed where the race is: the pass takes 1 s to make, a mailbox's first poll 2.5 s to arrive."""
import asyncio
import json

import httpx
from starlette.requests import Request
from starlette.responses import Response

from bbkit.common import AUTH
from bbkit.kerfy_relay import RelayProxy
from bbkit.servers import wait_until
from test_family_relay_anywhere import echo, install, link, named, relay  # noqa: F401


class SlowFirstPoll(RelayProxy):
    def __init__(self, upstream: str):
        self.polled = set()
        super().__init__(upstream)

    async def handle(self, request: Request) -> Response:
        path = request.url.path
        token = request.headers.get("authorization", "")
        if path == "/v1/family/pass":
            await asyncio.sleep(1.0)
        elif path == "/v1/family/next" and token not in self.polled:
            self.polled.add(token)
            await asyncio.sleep(2.5)
        return await super().handle(request)


def households(bridge):
    return json.loads((bridge.state_dir / "households.json").read_text())["households"]


def reaches(relay_url, box, token, pass_):
    """A call with this pass to this mailbox, taken by the relay (anything but `away` at once: 503)."""
    try:
        got = httpx.post(f"{relay_url}/v1/family/call/{box}", content=b"\x01" + b"x" * 64, timeout=5,
                         headers={"Authorization": f"Bearer {token}", "x-bip-kind": "call", "x-bip-pass": pass_,
                                  "content-type": "application/octet-stream"})
    except httpx.TimeoutException:
        return True   # taken, its answer awaited
    return got.status_code != 503


def test_the_scanner_takes_the_inviters_calls_once_it_gave_its_pass(bridges, fakes, relay):  # noqa: F811
    proxy = SlowFirstPoll(relay.url)
    try:
        fakes.hermes.override = echo
        lea = install(bridges, fakes, "lea", ["cuisine"], proxy.url)
        paul = install(bridges, fakes, "paul", ["coach"], proxy.url)
        named(lea, "Léa")
        named(paul, "Paul")
        link(paul, lea)
        # Léa holds Paul's pass for his mailbox (given through the link)
        assert wait_until(lambda: households(lea)["paul"].get("pass"), timeout=30), lea.log_tail()
        entry = households(lea)["paul"]
        token = json.loads((lea.state_dir / "family-relay.json").read_text())["mailbox"]["token"]
        assert wait_until(lambda: reaches(relay.url, entry["mailbox"], token, entry["pass"]), timeout=8), (
            "Paul's mailbox refuses the pass he gave Léa: his first poll said « none » after his pass's statement, "
            "dropped by the relay, reached it")
    finally:
        proxy.stop()
