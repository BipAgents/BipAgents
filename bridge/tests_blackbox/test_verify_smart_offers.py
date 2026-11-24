"""Verification of the fixes of review 1 of `smart-offers` (24/11, passation-pro briefs/review-smart-offers.md
« Vérification du correctif »): red tests of what stays of B2 (health), no fix.

The fix leaves out a health turn and all after it in ITS conversation, and a conversation with a health turn before the
cursor (`health_seen`). The figures still reach a pass: (1) through another conversation of the same agent (its own
memory, USER.md, holds them: no health tool there); (2) through the same conversation once Hermes compacted it in place
(the health turn's rows go inactive, `GET /api/sessions/{id}/messages` returns active rows only)."""
from bbkit.common import AUTH
from test_smart_offers import add, look, passes, smart_place, talk


def sent_of(fakes):
    return passes(fakes)[-1]["body"]["messages"][1]["content"]


HEALTH = [{"role": "user", "content": "Comment j'ai dormi cette semaine ?"},
          {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "mcp_bipagents_health", "arguments": "{}"}}]},
          {"role": "tool", "content": "{\"sleep\": \"5 h 12\"}"},
          {"role": "assistant", "content": "Tu as dormi 5 h 12 par nuit en moyenne, ton pouls au repos est à 74."}]

LATER = [{"role": "user", "content": "On fait le plan de la semaine ?"},
         {"role": "assistant", "content": "Vu tes 5 h 12 de sommeil et ton pouls au repos à 74, deux sorties seulement."},
         {"role": "user", "content": "Et samedi ?"},
         {"role": "assistant", "content": "Samedi, footing léger."},
         {"role": "user", "content": "Et dimanche ?"},
         {"role": "assistant", "content": "Dimanche, repos."}]


def test_verify_b2_health_figures_never_reach_a_pass_through_another_conversation(bridges, fakes):
    bridge = smart_place(bridges, fakes)
    assert bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"health": True}).status_code == 200
    look(bridge)
    add(fakes, "s1", talk(3) + HEALTH)
    # a new conversation the next day: no health tool, the figures from the agent's own memory
    add(fakes, "s2", talk(1, words="Nouvelle semaine") + LATER)
    [run] = look(bridge)
    assert run["model_called"] is True, run
    sent = sent_of(fakes)
    for secret in ("5 h 12", "pouls"):
        assert secret not in sent, f"health figures sent to a pass through another conversation: {secret!r} in\n{sent}"


def test_verify_b2_health_figures_never_reach_a_pass_once_the_conversation_is_compacted(bridges, fakes):
    bridge = smart_place(bridges, fakes, gap_hours=0)
    assert bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"health": True}).status_code == 200
    look(bridge)
    add(fakes, "s1", talk(3) + HEALTH)
    [run] = look(bridge)
    assert run["model_called"] is True, run
    assert "5 h 12" not in sent_of(fakes)
    # Hermes compacts in place (internal/notes/history-compaction.md): the API gives the active rows, a summary first,
    # the kept tail; the health turn's rows are inactive. Then the conversation goes on, the figures in the summary.
    summary = {"role": "user", "content": "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted: the "
                                          "person trains for the Lyon half; sleep 5 h 12 a night, resting pulse 74."}
    kept = [{"role": "assistant", "content": "D'accord (2)."}]
    add(fakes, "s1", [summary] + kept + talk(4, words="On reprend") + LATER)
    calls = len(passes(fakes))
    [run] = look(bridge)
    assert run["model_called"] is True and len(passes(fakes)) == calls + 1, run
    sent = sent_of(fakes)
    for secret in ("5 h 12", "pouls"):
        assert secret not in sent, f"health figures sent to a pass after a compaction: {secret!r} in\n{sent}"


def test_verify_b2_a_compaction_summary_never_brings_a_mails_figure_to_a_pass(bridges, fakes):
    """Strict B2 (24/11), an agent WITHOUT Santé: Hermes compacts a conversation and its summary (a `user` row it
    writes itself, « [CONTEXT COMPACTION — REFERENCE ONLY] … ») says again what a mail of the compacted turns said;
    so does a summary merged after a kept message, and the agent's reply right after it. Here the conversation goes
    on from the summary (a continuation read from its start, as a compression's child session is). None reaches a
    pass; the person's words after it do."""
    bridge = smart_place(bridges, fakes, gap_hours=0)
    look(bridge)
    add(fakes, "s1", talk(3))
    [run] = look(bridge)
    assert run["model_called"] is True, run
    summary = {"role": "user", "content": "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted into the "
                                          "summary below. The bank's mail says the overdraft is 1 234,56 €."}
    merged = {"role": "user", "content": "Merci\n\n[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]\n\n[CONTEXT "
                                         "COMPACTION — REFERENCE ONLY] The landlord's letter: rent 987 euros."}
    after = [{"role": "assistant", "content": "Ton découvert est de 1 234,56 €, ton loyer 987 euros."}]
    add(fakes, "s2", [summary, merged] + after + talk(4, words="On reprend le plan du semi"))
    calls = len(passes(fakes))
    [run] = look(bridge)
    assert run["model_called"] is True and len(passes(fakes)) == calls + 1, run
    sent = sent_of(fakes)
    assert "On reprend le plan du semi" in sent, "the person's words stay"
    for secret in ("1 234,56", "987", "COMPACTION"):
        assert secret not in sent, f"a compaction summary sent to a pass: {secret!r} in\n{sent}"
