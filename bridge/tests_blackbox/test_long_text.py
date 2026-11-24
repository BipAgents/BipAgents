"""23/11 (Alex): a long text pasted (a blog article) and sent to his agent. The agent answered it, yet the message stayed
at the end of the conversation, « Pas envoyé ». Measured on the real Hermes v0.23.1 with BipAgents' patches (build
server, a fake model; briefs/status-fix-long-text.md): Hermes stores the person's row exactly as the bridge sent it
(its note line, then the words), 20 000, 60 000 and 150 000 characters alike, compaction in place or not, and the model
gets every character. The bridge then looked for the words sent in the row *as the timeline shows it*, where blank lines
in threes or more are two (`tidy`): an article copied from a web page has such gaps, so its row never held the words
as sent, and its `s_` item stayed at the end, « Pas envoyé » once the turn was over.

Texts over the bridge's 20 000 characters for one message's words were refused (422, « Pas envoyé » with an error,
nothing sent): they now go whole as a text file joined to the message, the agent told to read it, up to 200 000
characters (Alex 24/11); the file lives with its conversation.

Synthetic articles only, never anyone's real text."""
import random
import threading
import time

import pytest

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until
from test_threads_send import agent, page, send, setup, stored

WORDS = ("le vélo électrique change la façon dont on traverse une ville été comme hiver même sous la pluie les pistes "
         "cyclables à Lyon Nantes ou Strasbourg ont doublé depuis cinq ans d'après l'enquête publiée cet automne où l'on "
         "apprend que près d'un trajet sur dix se fait désormais à deux roues").split()
NOW = "<!--bip:now lundi 23 novembre, 19:00-->"
LINKS = ["https://exemple.org/velo/enquete-2026?source=blog&page=2", "[le rapport complet](https://exemple.org/rapport.pdf)",
         "<https://exemple.org/a-propos>"]


def article(chars, web=True, seed=2311):
    """A blog article as copied from a web page: headings, lists, links, accents, an emoji; with `web`, blank lines in
    threes or fours between some blocks and spaces at the end of some lines."""
    rnd = random.Random(seed)
    out = "Pourquoi le vélo électrique a gagné la ville\n\n"
    n = 0
    while len(out) < chars:
        n += 1
        if n % 7 == 0:
            block = f"## Partie {n // 7} : « ce qu'il faut retenir »"
        elif n % 7 == 3:
            block = "\n".join(f"- point {k} : " + " ".join(rnd.choice(WORDS) for _ in range(8)) for k in (1, 2, 3))
        else:
            block = " ".join(" ".join(rnd.choice(WORDS) for _ in range(14)).capitalize() + "." for _ in range(4))
            if n % 5 == 0:
                block += f" Voir {rnd.choice(LINKS)} pour les détails."
            if n % 11 == 0:
                block += " Ça roule 🚲."
            if web and n % 4 == 0:
                block += "   "
        out += block + ("\n" * rnd.choice([2, 3, 4]) if web else "\n\n")
    return out[:chars].rstrip() + "\n\nFin de l'article."


def test_a_long_article_pasted_is_the_message_its_agent_answered(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    text = article(19_000)
    assert "\n\n\n" in text and len(text) <= 20_000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Bien lu.")]
    client_id = "C1D00000-0000-4000-8000-00000000A271"
    sent = send(bridge, thread, client_id, text)
    assert sent.status_code == 200, sent.text
    assert sent.json()["state"] == "started"
    # what Hermes got: the words whole
    given = fakes.hermes.started["r1"]["input"]
    assert given.endswith(text.strip())
    # Hermes (v0.23.1, measured) stores the person's row as it was sent, the time's note before it (patch 0015)
    stored(fakes, {"role": "user", "content": f"{NOW}\n\n{given}"}, agent("Bien lu."))
    gate.set()
    assert wait_until(lambda: any(i.get("text") == "Bien lu." for i in page(bridge, thread)["items"])
                      and not page(bridge, thread).get("pending", {}).get("run"), timeout=15)
    time.sleep(11)   # past send.rs `LOST_AFTER`: a message still unmatched is « Pas envoyé » then
    items = page(bridge, thread)["items"]
    shown = [(i["id"][:40], (i.get("text") or "")[:30], i.get("client_id"), i.get("sending"), i.get("not_sent"))
             for i in items]
    assert [i["id"] for i in items if i["id"].startswith("s_")] == [], f"« Pas envoyé » left at the end: {shown}"
    row = items[-2]
    assert row["role"] == "person" and row.get("client_id") == client_id, shown
    # the row shows the article (its blank lines tidied, nothing cut)
    assert row["text"].startswith("Pourquoi le vélo électrique") and row["text"].endswith("Fin de l'article.")
    assert " ".join(row["text"].split()) == " ".join(text.split())


def test_an_article_with_its_lines_as_written_is_its_message_too(bridges, fakes):
    """The same without the web page's gaps (the words found as they were before): still its message."""
    bridge, thread = setup(bridges, fakes)
    text = article(19_000, web=False)
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("run.completed", run_id="r1", output="Lu.")]
    assert send(bridge, thread, "C1D00000-0000-4000-8000-00000000A272", text).status_code == 200
    stored(fakes, {"role": "user", "content": f"{NOW}\n\n{fakes.hermes.started['r1']['input']}"}, agent("Lu."))
    assert wait_until(lambda: page(bridge, thread)["items"][-1].get("text") == "Lu.", timeout=15)
    items = page(bridge, thread)["items"]
    assert [i["id"] for i in items if i["id"].startswith("s_")] == []
    assert items[-2].get("client_id") == "C1D00000-0000-4000-8000-00000000A272"


def uploads(bridge):
    return bridge.root / "uploads" / "wellness"


@pytest.mark.parametrize("n, chars", [(0, 60_000), (1, 150_000)])
def test_a_text_over_twenty_thousand_characters_goes_whole_as_a_text_file(bridges, fakes, n, chars):
    bridge, thread = setup(bridges, fakes)
    text = article(chars, seed=n)
    run = "r1"
    gate = threading.Event()
    fakes.hermes.runs[run] = [sse("run.started", run_id=run), gate, sse("run.completed", run_id=run, output="Lu en entier.")]
    client_id = f"C1D00000-0000-4000-8000-00000000B{n:03d}"
    sent = send(bridge, thread, client_id, text)
    assert sent.status_code == 200, sent.text[:300]
    assert sent.json()["state"] == "started"
    given = fakes.hermes.started[run]["input"]
    assert isinstance(given, str) and len(given) < 2_000, "the words go as a file, not in the message"
    # one reference line to a text file holding the whole text, the agent told to read it
    assert "[Pièce jointe : " in given and "(text/plain, " in given, given
    assert "<!--bip:long_text" in given, given
    path = given.split(" → ")[-1].rstrip("]").strip()
    name = path.rsplit("/", 1)[-1]
    files = [p for p in uploads(bridge).rglob("*") if p.is_file() and p.name == name]
    assert len(files) == 1, (path, list(uploads(bridge).rglob("*")))
    assert files[0].read_text(encoding="utf-8") == text.strip()
    # named after its first line
    assert "Pourquoi" in name and name.endswith(".txt"), name
    # at once on every phone: the message with its file, no words
    last = page(bridge, thread)["items"][-1]
    assert last["id"] == f"s_{client_id}" and last["text"] == ""
    assert [(a["kind"], a["mime"]) for a in last["attachments"]] == [("document", "text/plain")]
    # stored by Hermes as sent: its row, matched by its id, the file, no note shown
    stored(fakes, {"role": "user", "content": f"{NOW}\n\n{given}"}, agent("Lu en entier."))
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["items"][-1].get("text") == "Lu en entier.", timeout=15)
    items = page(bridge, thread)["items"]
    assert [i["id"] for i in items if i["id"].startswith("s_")] == []
    row = items[-2]
    assert row.get("client_id") == client_id and row["text"] == ""
    assert [(a["kind"], a["mime"]) for a in row["attachments"]] == [("document", "text/plain")]


def long_files(bridge):
    return sorted(p for p in uploads(bridge).rglob("*.txt") if p.is_file())


def test_two_hundred_thousand_characters_go_whole_as_a_text_file(bridges, fakes):
    """Alex 24/11: 200 000 characters at most (two of Hermes' `read_file` reads, about 50 000 tokens)."""
    bridge, thread = setup(bridges, fakes)
    text = article(210_000, seed=3)[:199_990].rstrip()
    text += "x" * (199_999 - len(text)) + "."
    assert len(text) == 200_000
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("run.completed", run_id="r1", output="Lu.")]
    sent = send(bridge, thread, "C1D00000-0000-4000-8000-00000000C200", text)
    assert sent.status_code == 200, sent.text[:300]
    given = fakes.hermes.started["r1"]["input"]
    assert "[Pièce jointe : " in given and "<!--bip:long_text" in given and len(given) < 2_000, given[:300]
    files = long_files(bridge)
    assert len(files) == 1 and files[0].read_text(encoding="utf-8") == text


def test_a_text_over_two_hundred_thousand_characters_is_refused_as_before(bridges, fakes):
    """Refused as a text over 20 000 characters was before (the same 422, an app 1.0 shows « Pas envoyé »)."""
    bridge, thread = setup(bridges, fakes)
    too_long = send(bridge, thread, "C1D00000-0000-4000-8000-00000000C001", "a" * 200_001)
    assert too_long.status_code == 422, too_long.status_code
    detail = too_long.json()["detail"]
    assert [(d["type"], d["loc"]) for d in detail] == [("string_too_long", ["body", "text"])], detail
    assert not fakes.hermes.started
    assert long_files(bridge) == []


def test_a_long_texts_file_goes_with_its_conversation(bridges, fakes):
    """Alex 24/11: the file of a long text lives with its conversation and is deleted with it; the agent's other
    files stay."""
    bridge, _ = setup(bridges, fakes)
    other = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                        files={"file": ("notes.pdf", b"%PDF-1.4 x", "application/pdf")})
    assert other.status_code in (200, 201), other.text
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("run.completed", run_id="r1", output="Lu.")]
    sent = send(bridge, "new", "C1D00000-0000-4000-8000-00000000D001", article(60_000, seed=4))
    assert sent.status_code == 200, sent.text[:300]
    thread = sent.json()["thread"]
    files = long_files(bridge)
    assert len(files) == 1, files
    kept = [p for p in uploads(bridge).rglob("*") if p.is_file() and p.suffix == ".pdf"]
    assert len(kept) == 1
    deleted = bridge.delete(f"/v1/threads/{thread}", headers=AUTH, params={"agent": "wellness"})
    assert deleted.status_code == 204, deleted.text
    assert long_files(bridge) == [], "the long text's file stayed after its conversation was deleted"
    assert kept[0].exists(), "another file of the agent went with the conversation"
