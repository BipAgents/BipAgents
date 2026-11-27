"""The background turns' code word `[SILENT]` (bridge-rs/src/background_turn.rs `is_silent`, threads/timeline.rs
`quiet`). 27/11, Alex: after a watcher's routine, « [SILENT] » showed in the conversation as a SECOND message — the
agent wrote a first message (a report, or a hollow line before a call), then the code word alone. The main
conversation kept every one. Now in every conversation: the code word alone never shows, a hollow first line goes
with it (the whole turn is silent), a real report stays, without the code word."""
from bbkit.common import AUTH
from test_turn_restart import agent, page, setup, user

NOTE = "<!--bip:watch\nYour watcher « MacBook » saw a change.\n-->"


def watcher_turn(first, second):
    return [user(NOTE),
            {"role": "assistant", "content": first,
             "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "memory", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": "ok"},
            agent(second)]


def agent_texts(bridge, thread):
    return [i["text"] for i in page(bridge, thread)["items"] if i["kind"] == "message" and i["role"] == "agent"]


def shown_after(bridges, fakes, first, second):
    bridge, main = setup(bridges, fakes)
    fakes.hermes.messages[("", "api_main")] += watcher_turn(first, second)
    return agent_texts(bridge, main)


def test_the_code_word_as_a_second_message_never_shows(bridges, fakes):
    report = "Le MacBook Air M4 est à 899 € chez Boulanger."
    texts = shown_after(bridges, fakes, report, "[SILENT]")
    assert report in texts and not [t for t in texts if "SILENT" in t], texts


def test_a_hollow_first_line_and_the_code_word_the_whole_turn_is_silent(bridges, fakes):
    texts = shown_after(bridges, fakes, "Rien de nouveau sur les prix.", "**[SILENT]**")
    assert "Rien de nouveau sur les prix." not in texts and not [t for t in texts if "SILENT" in t], texts
    # the person's own turn before it keeps its answer
    assert "Salut" in texts


def test_the_code_word_after_the_words_leaves_the_words(bridges, fakes):
    texts = shown_after(bridges, fakes, "Je regarde.", "Darty le vend 849 €.\n\n[SILENT]")
    assert texts[-1] == "Darty le vend 849 €.", texts
