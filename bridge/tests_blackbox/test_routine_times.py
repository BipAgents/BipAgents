"""A routine's time in words, never a cron expression on a screen (bridge-rs/src/schedule_words.rs; Alex 25/11 on
« Préparer Santé et sport »: « Ça veut dire quoi ces chiffres » about « 0 12 * * 1-5 »). The catalogue, the walk after
« Adopter » (`display`), an agent's routines (`schedule_shown`, Hermes' `schedule_display` kept for the editor) and
« Ce que tes agents font tout seuls » (`when.shown`) carry words the apps show as they are, in the agent's language
(the catalogue's in the language asked); several days read « Le lundi, le mercredi et le vendredi » (French hours « 12 h », Alex 25/11), anything else
« Régulièrement »."""
import json
import re

from bbkit.common import AUTH
from test_guide import place

CRON = re.compile(r"(^|\s)[\d*,/-]+\s+[\d*,/-]+\s+[\d*,/?-]+\s+[\d*,/?-]+\s+[\w*,/?-]+($|\s)")


def shown_texts(value, keys=("display", "schedule_shown", "shown", "text")):
    """Every text the apps show as it is, anywhere in `value`."""
    if isinstance(value, dict):
        for k, v in value.items():
            if k in keys and isinstance(v, str):
                yield v
            yield from shown_texts(v, keys)
    elif isinstance(value, list):
        for v in value:
            yield from shown_texts(v, keys)


def assert_no_cron(value):
    crons = [t for t in shown_texts(value) if CRON.search(t)]
    assert not crons, crons


def test_a_models_routines_and_an_agents_say_their_times_in_words(bridges, fakes):
    bridge, _ = place(bridges, fakes, agents=("coach",))
    # the catalogue, in the language asked
    for lang, words in (("fr", ["En semaine à 12\u00a0h", "Le dimanche à 19\u00a0h"]),
                        ("en", ["Weekdays at 12:00 PM", "Every Sunday at 7:00 PM"]),
                        ("de", ["Werktags um 12:00 Uhr", "Jeden Sonntag um 19:00 Uhr"]),
                        ("es", ["Entre semana a las 12:00", "Cada domingo a las 19:00"])):
        listed = bridge.get("/v1/templates/catalogue", headers=AUTH, params={"lang": lang}).json()["templates"]
        sante = next(t for t in listed if t["id"] == "sante")
        assert [r["display"] for r in sante["routines"]] == words, lang
        assert_no_cron(listed)

    # « Adopter » Santé et sport: the walk's words, both off
    adopted = bridge.post("/v1/templates/import", headers=AUTH, json={"catalogue": "sante", "lang": "fr"})
    assert adopted.status_code == 201, adopted.text
    out = adopted.json()
    agent = out["agent"]["id"]
    assert [(r["name"], r["display"], r["on"]) for r in out["routines"]] == [
        ("Un moment pour bouger", "En semaine à 12\u00a0h", False), ("Bilan de la semaine", "Le dimanche à 19\u00a0h", False)]
    walk = bridge.get(f"/v1/agents/{agent}/template-routines", headers=AUTH).json()["routines"]
    assert [r["display"] for r in walk] == ["En semaine à 12\u00a0h", "Le dimanche à 19\u00a0h"]
    assert not any(r["on"] for r in walk)
    assert_no_cron(out)

    # its routines' page: the words, Hermes' expression kept for the editor
    jobs = bridge.get(f"/v1/agents/{agent}/routines", headers=AUTH).json()["jobs"]
    assert sorted(j["schedule_shown"] for j in jobs) == ["En semaine à 12\u00a0h", "Le dimanche à 19\u00a0h"]
    assert sorted(j["schedule_display"] for j in jobs) == ["0 12 * * 1-5", "0 19 * * 0"]
    assert all(j["enabled"] is False for j in jobs)

    # several days, a rhythm the apps' fields cannot say, and the rest
    for schedule, words in (("30 7 * * 1,3,5", "Le lundi, le mercredi et le vendredi à 7\u00a0h\u00a030"),
                            ("0 10 * * 6,0", "Le week-end à 10\u00a0h"),
                            ("0 */3 * * *", "Toutes les 3 h"),
                            ("0 8-18/2 * * *", "Régulièrement")):
        made = bridge.post(f"/v1/agents/{agent}/routines", headers=AUTH,
                           json={"name": f"R {schedule}", "schedule": schedule, "prompt": "Fais le point."})
        assert made.status_code in (200, 201), made.text
        assert made.json()["job"]["schedule_shown"] == words, schedule

    # « Ce que tes agents font tout seuls »: `when.shown`, the fields kept for the apps from before
    items = [i for i in bridge.get("/v1/autonomous", headers=AUTH, params={"agent": agent}).json()["items"]
             if i["kind"] == "routine"]
    by_title = {i["title"]: i["when"] for i in items}
    assert by_title["Un moment pour bouger"]["shown"] == "En semaine à 12\u00a0h"
    assert by_title["Un moment pour bouger"]["kind"] == "weekdays"
    days = by_title["R 30 7 * * 1,3,5"]
    assert (days["kind"], days["raw"], days["shown"]) == ("custom", "30 7 * * 1,3,5", "Le lundi, le mercredi et le vendredi à 7\u00a0h\u00a030")
    assert by_title["R 0 */3 * * *"]["kind"] == "every_hours" and by_title["R 0 */3 * * *"]["hours"] == 3
    assert by_title["R 0 8-18/2 * * *"]["shown"] == "Régulièrement"
    assert all(i["when"].get("shown") for i in items), json.dumps(items, ensure_ascii=False)[:2000]
