"""Review of `smart-offers` (24/11, passation-pro briefs/review-smart-offers.md): red tests of its blockers, no fix.

B2: what a day's pass reads (offer_ideas.rs `new_turns`, `reads_private`): a health turn is left out, but not the next
turns that carry its figures; nor what `mail_search` (headers and a body snippet) or `drive_search` (the files' texts)
gave the agent. B5: two looks at once (the scheduler's tick and `POST /v1/proactive/passes`, or two POSTs) each make
their pass: the pre-filter and the day's budget are read before either is counted."""
import threading

import httpx

from bbkit.common import AUTH
from test_smart_offers import add, look, passes, smart_place, talk


def sent_of(fakes):
    return passes(fakes)[-1]["body"]["messages"][1]["content"]


def test_review_b2_a_pass_never_reads_the_health_figures_of_the_turns_after_a_health_turn(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    # the agent has « Santé » on (it called the health tool): the passes never read the health part (§3b.7)
    assert bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"health": True}).status_code == 200
    look(bridge)
    health = [{"role": "user", "content": "Comment j'ai dormi cette semaine ?"},
              {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "mcp_bipagents_health", "arguments": "{}"}}]},
              {"role": "tool", "content": "{\"sleep\": \"5 h 12\"}"},
              {"role": "assistant", "content": "Tu as dormi 5 h 12 par nuit en moyenne, ton pouls au repos est à 74."},
              # the next turn: no tool called, the same figures
              {"role": "user", "content": "Et du coup, avec mes 5 h 12 de sommeil, je cours quand même ?"},
              {"role": "assistant", "content": "Avec 5 h 12 de sommeil et un pouls au repos à 74, lève le pied cette semaine."}]
    add(fakes, "s1", talk(3) + health)
    [run] = look(bridge)
    assert run["model_called"] is True, run
    sent = sent_of(fakes)
    assert "Je prépare le semi" in sent, sent
    for secret in ("5 h 12", "pouls"):
        assert secret not in sent, f"health figures sent to a pass: {secret!r} in\n{sent}"


def test_review_b2_a_pass_never_reads_what_a_mail_or_document_search_gave_the_agent(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    look(bridge)
    mail = [{"role": "user", "content": "J'ai reçu un mail de la banque ?"},
            {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "mcp_bipagents_mail_search", "arguments": "{}"}}]},
            {"role": "tool", "content": "[{\"from\": \"banque\", \"snippet\": \"Votre découvert atteint 1 234,56 €\"}]"},
            {"role": "assistant", "content": "Oui : la banque écrit que ton découvert atteint 1 234,56 €."}]
    document = [{"role": "user", "content": "Tu retrouves mon bail ?"},
                {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "mcp_bipagents_drive_search", "arguments": "{}"}}]},
                {"role": "tool", "content": "[{\"name\": \"Bail.pdf\", \"text\": \"loyer mensuel 987 euros\"}]"},
                {"role": "assistant", "content": "Oui, ton bail dit : loyer mensuel 987 euros."}]
    add(fakes, "s1", talk(3) + mail + document)
    [run] = look(bridge)
    assert run["model_called"] is True, run
    sent = sent_of(fakes)
    assert "J'ai reçu un mail de la banque" in sent, "the person's words stay"
    for secret in ("1 234,56", "987 euros"):
        assert secret not in sent, f"a mail's or a document's content sent to a pass: {secret!r} in\n{sent}"


def test_review_b5_two_looks_at_once_make_one_pass_not_two(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    look(bridge)
    add(fakes, "s1", talk(4))
    fakes.model.idea_delay = 1.5   # the model takes a moment (a real one takes seconds; the person's own 75 to 100 s)
    codes = []

    def one():
        with httpx.Client(base_url=bridge.url, timeout=60.0) as http:
            codes.append(http.post("/v1/proactive/passes", headers=AUTH).status_code)

    threads = [threading.Thread(target=one) for _ in range(3)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(60)
    assert codes == [200, 200, 200], codes
    calls = len(passes(fakes))
    view = bridge.get("/v1/proactive/passes", headers=AUTH).json()
    assert calls == 1 and view["model_calls"] == 1, \
        f"{calls} model calls and {view['model_calls']} counted for ONE pass due (2 h apart, the day's budget read once)"
