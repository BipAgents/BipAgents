//! The bridge's own line in the conversation when a card payment is given up (fix-pay-total-check, 19/11): « Paiement
//! annulé : la page demandait 27,97 €, tu avais accepté 24,50 €. Rien n'a été payé. » when the bridge saw another total
//! on the page (card_pay.rs `total_differs`), « Paiement annulé à la demande de Vie. Rien n'a été payé. » when the
//! agent called `cancel_payment`. The person reads what happened from the bridge, not an explanation the agent makes up
//! (Vie, 19/11: « la banque refuse que je remplisse la carte automatiquement », invented).
//!
//! Like the vault's line (vault_line.rs), the line is the bridge's testimony: its words are the bridge's only (never
//! the agent's reason: Relecture 1, 19/11), recorded when the bridge gives the payment up, with the turn of that very
//! call (the one turn of the agent the hub sees inside a payment tool's call, or inside the tool search's call that
//! wraps the Link tools; none or several: recorded with no conversation, shown nowhere, and the agent is told to say
//! the sentence itself): `<data_dir>/pay-lines.jsonl`, one line `{at, agent, session, run, text}` (no
//! card data; 0600, the last `KEEP` lines). The thread shows each as a `notice` item (code `card_pay`) right after the
//! last item of its turn written before it, which every app already shows as the bridge wrote it: no app change.
//!
//! No Python counterpart.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::config::Config;
use crate::threads::timeline::Item;

/// The notice's code.
pub const CODE: &str = "card_pay";
/// The bridge's record, in its data folder.
pub const FILE: &str = "pay-lines.jsonl";
/// Lines kept at most (the most recent).
pub const KEEP: usize = 500;
/// The payment tools whose call a line belongs to (card_pay.rs, link_wallet.rs).
const TOOLS: &[&str] = &[
    crate::card_pay::PAY,
    crate::card_pay::CONFIRM,
    crate::card_pay::CANCEL,
    crate::link_wallet::PAY,
];

static WRITING: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(FILE)
}

/// A turn's tool is one of the payment tools (`pay_with_card`, `mcp_bipagents_pay_pay_with_card`,
/// `mcp__bipagents_link__cancel_payment`…).
pub fn is_pay_tool(name: &str) -> bool {
    TOOLS.iter().any(|tool| name == *tool || name.ends_with(&format!("_{tool}")))
}

/// One line of the record, kept with the last `KEEP`.
pub fn record(file: &Path, line: &Value) {
    let _one_at_a_time = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut lines: Vec<String> = std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    lines.push(line.to_string());
    let skip = lines.len().saturating_sub(KEEP);
    let text: String = lines[skip..].iter().map(|l| format!("{l}\n")).collect();
    let fresh = file.with_extension("jsonl.new");
    let written = crate::py::write_with_mode(&fresh.to_string_lossy(), text.as_bytes(), 0o600)
        .and_then(|()| std::fs::rename(&fresh, file));
    if let Err(err) = written {
        tracing::warn!(error = %err, "card payment: the conversation's line not recorded");
    }
}

/// A call the line may belong to: a payment tool, by its own name or by the one the tool search's `tool_call` wraps
/// (the Link tools are reached through it: tool_diet.rs keeps only `bipagents_pay` direct; the hub reads the wrapped
/// calls' names, runs.rs `wrapped_names`, never « any tool_call »: Relecture 2, 5). Only one turn of the agent inside
/// such a call counts (`turn_of_call`): never another conversation's.
fn in_pay_call(name: &str) -> bool {
    is_pay_tool(name)
}

/// The bridge says `text` in the conversation of the payment tool's call going on for `agent` now. False when that
/// conversation is not known for sure (a routine's turn the hub doesn't follow, two turns in a call at once): the line
/// is then shown nowhere, and the agent must say it itself (card_pay.rs `told`).
pub fn note(state: &crate::app::AppState, agent: &str, text: &str) -> bool {
    let calling = state.hub.runs_in_call(agent, in_pay_call);
    let (run, session) = crate::vault_line::turn_of_call(&calling);
    if session.is_none() {
        tracing::info!(agent, turns = calling.len(), "card payment line recorded with no conversation");
    }
    let placed = session.is_some();
    let line = json!({"at": crate::py::time_time(), "agent": agent, "session": session, "run": run, "text": text});
    record(&path(&state.config()), &line);
    placed
}

/// A line recorded: when, in which Hermes session, its words.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    pub at: f64,
    pub session: String,
    pub text: String,
}

/// The lines recorded for `agent` with a conversation, oldest first.
pub fn recorded(file: &Path, agent: &str) -> Vec<Recorded> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("agent").and_then(Value::as_str) == Some(agent))
        .filter_map(|e| {
            Some(Recorded {
                at: e.get("at").and_then(Value::as_f64)?,
                session: e.get("session").and_then(Value::as_str)?.to_string(),
                text: e.get("text").and_then(Value::as_str).filter(|t| !t.is_empty())?.to_string(),
            })
        })
        .collect()
}

pub fn recorded_for(config: &Config, agent: &str) -> Vec<Recorded> {
    recorded(&path(config), agent)
}

/// Each recorded line as a `notice` item, right after the last item of its session's turn written before it (a second
/// of slack). A line whose turn is not among `items` (another page of the thread) is left out.
pub fn with_pay_lines(items: &mut Vec<Item>, recorded: &[Recorded]) {
    let mut lines: Vec<(usize, Item)> = Vec::new();
    for (n, r) in recorded.iter().enumerate() {
        let Some(index) = items.iter().rposition(|i| !i.turn.is_empty() && i.segment == r.session && i.at <= r.at + 1.0)
        else {
            continue;
        };
        let at = &items[index];
        // the turn this item belongs to started before the line (its first item)
        if !items.iter().any(|i| i.turn == at.turn && i.at <= r.at + 1.0) {
            continue;
        }
        let body = json!({"kind": "notice", "notice": {"code": CODE, "text": r.text}});
        let Value::Object(body) = body else { continue };
        let pos = (at.pos.0, at.pos.1, at.pos.2 + 1 + n as u32);
        let id = format!("{}.pay{}", at.turn, (r.at * 1000.0) as i64);
        lines.push((index, Item { id, pos, at: r.at, turn: at.turn.clone(), segment: at.segment.clone(), row: at.row, body }));
    }
    // from the end, so each index still points at its item; two lines after one item keep their order
    lines.sort_by_key(|(index, item)| (std::cmp::Reverse(*index), std::cmp::Reverse(item.pos.2)));
    for (index, line) in lines {
        items.insert(index + 1, line);
    }
}

// --- the words -------------------------------------------------------------------------------------------------------

/// The language's key among the agents' (souls.rs `LANGUAGE_NAMES`): « fr », « en », « de », « es », « nl ».
fn lang_key(lang: &str) -> &str {
    lang.split(['-', '_']).next().unwrap_or("en")
}

/// The bridge cancelled: the page showed `shown` as its total, the person approved `approved` (amounts as the person
/// reads them, urgent_pay.rs `money`).
pub fn total_differs(lang: &str, shown: &str, approved: &str) -> String {
    match lang_key(lang) {
        "fr" => format!("Paiement annulé : la page demandait {shown}, tu avais accepté {approved}. Rien n'a été payé."),
        "de" => format!(
            "Zahlung abgebrochen: Die Seite verlangte {shown}, du hattest {approved} genehmigt. Es wurde nichts bezahlt."
        ),
        "es" => format!("Pago cancelado: la página pedía {shown} y tú habías aceptado {approved}. No se ha pagado nada."),
        "nl" => format!("Betaling geannuleerd: de pagina vroeg {shown}, jij had {approved} goedgekeurd. Er is niets betaald."),
        _ => format!("Payment cancelled: the page asked for {shown}, you had approved {approved}. Nothing was paid."),
    }
}

/// The agent called `cancel_payment` (its reason is never quoted here: the line is the bridge's own, Relecture 1, 9).
/// `pressed`: the pay button had been pressed (the payment may have gone through); `taken_over`: the person took the
/// browser over meanwhile (they may have paid themselves: « nothing was paid » is not said).
pub fn agent_cancelled(lang: &str, agent: &str, pressed: bool, taken_over: bool) -> String {
    let lang = lang_key(lang);
    if pressed {
        return match lang {
            "fr" => format!(
                "Paiement arrêté à la demande de {agent}, après l'appui sur le bouton de paiement : vérifie sur le site \
                 ou auprès de ta banque s'il est passé."
            ),
            "de" => format!(
                "Zahlung auf Wunsch von {agent} gestoppt, nachdem der Bezahlknopf gedrückt wurde: Prüfe auf der Seite \
                 oder bei deiner Bank, ob sie durchgegangen ist."
            ),
            "es" => format!(
                "Pago detenido a petición de {agent}, después de pulsar el botón de pago: comprueba en el sitio o con \
                 tu banco si se ha realizado."
            ),
            "nl" => format!(
                "Betaling gestopt op verzoek van {agent}, nadat op de betaalknop was gedrukt: kijk op de site of bij je \
                 bank of ze gelukt is."
            ),
            _ => format!(
                "Payment stopped at {agent}'s request, after the pay button was pressed: check on the site or with your \
                 bank whether it went through."
            ),
        };
    }
    let nothing = !taken_over;
    match lang {
        "fr" => format!("Paiement annulé à la demande de {agent}.{}", if nothing { " Rien n'a été payé." } else { "" }),
        "de" => format!("Zahlung auf Wunsch von {agent} abgebrochen.{}", if nothing { " Es wurde nichts bezahlt." } else { "" }),
        "es" => format!("Pago cancelado a petición de {agent}.{}", if nothing { " No se ha pagado nada." } else { "" }),
        "nl" => format!("Betaling geannuleerd op verzoek van {agent}.{}", if nothing { " Er is niets betaald." } else { "" }),
        _ => format!("Payment cancelled at {agent}'s request.{}", if nothing { " Nothing was paid." } else { "" }),
    }
}

/// The person's « Suspendre tous les paiements » stopped it (the agent's cancel came after).
pub fn suspended(lang: &str) -> String {
    match lang_key(lang) {
        "fr" => "Paiement arrêté : tu as suspendu les paiements. Rien n'a été payé.",
        "de" => "Zahlung gestoppt: Du hast die Zahlungen ausgesetzt. Es wurde nichts bezahlt.",
        "es" => "Pago detenido: has suspendido los pagos. No se ha pagado nada.",
        "nl" => "Betaling gestopt: je hebt de betalingen gepauzeerd. Er is niets betaald.",
        _ => "Payment stopped: you suspended payments. Nothing was paid.",
    }
    .to_string()
}

/// Right before the press, the amount on the page was no longer the one read when the card was typed.
pub fn total_changed(lang: &str) -> String {
    match lang_key(lang) {
        "fr" => "Paiement arrêté : le montant affiché sur la page a changé au dernier moment. Rien n'a été payé.",
        "de" => "Zahlung gestoppt: Der Betrag auf der Seite hat sich im letzten Moment geändert. Es wurde nichts bezahlt.",
        "es" => "Pago detenido: el importe de la página cambió en el último momento. No se ha pagado nada.",
        "nl" => "Betaling gestopt: het bedrag op de pagina veranderde op het laatste moment. Er is niets betaald.",
        _ => "Payment stopped: the amount on the page changed at the last moment. Nothing was paid.",
    }
    .to_string()
}

/// The page showed an amount above the approved one, not labelled as its total (Relecture 2, 1).
pub fn higher_amount(lang: &str, shown: &str, approved: &str) -> String {
    match lang_key(lang) {
        "fr" => format!(
            "Paiement arrêté : la page affichait un montant plus élevé ({shown}) que celui que tu avais accepté ({approved}). Rien n'a été payé."
        ),
        "de" => format!(
            "Zahlung gestoppt: Die Seite zeigte einen höheren Betrag ({shown}) als den, den du genehmigt hattest ({approved}). Es wurde nichts bezahlt."
        ),
        "es" => format!(
            "Pago detenido: la página mostraba un importe mayor ({shown}) que el que habías aceptado ({approved}). No se ha pagado nada."
        ),
        "nl" => format!(
            "Betaling gestopt: de pagina toonde een hoger bedrag ({shown}) dan het bedrag dat je had goedgekeurd ({approved}). Er is niets betaald."
        ),
        _ => format!(
            "Payment stopped: the page showed a higher amount ({shown}) than the one you had approved ({approved}). Nothing was paid."
        ),
    }
}

/// The page's total could not be checked against the approved amount (amounts that disagree in a way the bridge can't
/// name).
pub fn unverified(lang: &str) -> String {
    match lang_key(lang) {
        "fr" => "Paiement arrêté : BipAgents n'a pas pu vérifier le total de la page. Rien n'a été payé.",
        "de" => "Zahlung gestoppt: BipAgents konnte den Gesamtbetrag der Seite nicht prüfen. Es wurde nichts bezahlt.",
        "es" => "Pago detenido: BipAgents no pudo comprobar el total de la página. No se ha pagado nada.",
        "nl" => "Betaling gestopt: BipAgents kon het totaal van de pagina niet controleren. Er is niets betaald.",
        _ => "Payment stopped: BipAgents could not check the page's total. Nothing was paid.",
    }
    .to_string()
}

/// The total the agent gave `confirm_payment` was above the approved amount (its own reading: « donné par Vie »).
pub fn stated_differs(lang: &str, agent: &str, stated: &str, approved: &str) -> String {
    match lang_key(lang) {
        "fr" => format!(
            "Paiement arrêté : le total donné par {agent} ({stated}) n'est pas celui que tu avais accepté ({approved}). Rien n'a été payé."
        ),
        "de" => format!(
            "Zahlung gestoppt: Der von {agent} genannte Gesamtbetrag ({stated}) ist nicht der, den du genehmigt hattest ({approved}). Es wurde nichts bezahlt."
        ),
        "es" => format!(
            "Pago detenido: el total indicado por {agent} ({stated}) no es el que habías aceptado ({approved}). No se ha pagado nada."
        ),
        "nl" => format!(
            "Betaling gestopt: het totaal dat {agent} opgaf ({stated}) is niet het bedrag dat je had goedgekeurd ({approved}). Er is niets betaald."
        ),
        _ => format!(
            "Payment stopped: the total given by {agent} ({stated}) is not the one you had approved ({approved}). Nothing was paid."
        ),
    }
}

/// The pay button asked for another currency (`shown`, e.g. « 32,10 USD »).
pub fn foreign_currency(lang: &str, shown: &str) -> String {
    match lang_key(lang) {
        "fr" => format!("Paiement arrêté : la page demandait une autre devise ({shown}). Rien n'a été payé."),
        "de" => format!("Zahlung gestoppt: Die Seite verlangte eine andere Währung ({shown}). Es wurde nichts bezahlt."),
        "es" => format!("Pago detenido: la página pedía otra moneda ({shown}). No se ha pagado nada."),
        "nl" => format!("Betaling gestopt: de pagina vroeg een andere munteenheid ({shown}). Er is niets betaald."),
        _ => format!("Payment stopped: the page asked for another currency ({shown}). Nothing was paid."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, turn: &str, at: f64, kind: &str) -> Item {
        let Value::Object(body) = json!({"kind": kind}) else { unreachable!() };
        Item { id: id.into(), pos: (1, at as i64, 0), at, turn: turn.into(), segment: "s1".into(), row: 1, body }
    }

    #[test]
    fn the_payment_tools_are_known_by_every_name_hermes_gives_them() {
        for name in ["pay_with_card", "mcp_bipagents_pay_pay_with_card", "mcp__bipagents_pay__cancel_payment",
                     "mcp_bipagents_link_pay_with_link", "mcp_bipagents_pay_confirm_payment"]
        {
            assert!(is_pay_tool(name), "{name}");
        }
        for name in ["list_cards", "mcp_bipagents_pay_list_cards", "browser_click", "cancel_payments_now"] {
            assert!(!is_pay_tool(name), "{name}");
        }
        // the Link tools come through the tool search (tool_diet.rs): its call counts, one turn only (`turn_of_call`)
        assert!(!in_pay_call("tool_call") && !in_pay_call("browser_click"));
        let wrapped = |args: Value| {
            let Value::Object(data) = json!({"tool": "tool_call", "args": args}) else { unreachable!() };
            crate::runs::wrapped_names(&data)
        };
        assert_eq!(wrapped(json!({"calls": [{"name": "mcp_bipagents_link_cancel_payment", "arguments": {}}]})), ["link_cancel_payment"]);
        assert!(wrapped(json!({"name": "mcp__bipagents_link__pay_with_link"})).iter().any(|name| in_pay_call(name)));
        // several calls at once: the run is inside each of them (still that one turn)
        assert_eq!(wrapped(json!({"calls": [{"name": "a"}, {"name": "b"}]})), ["a", "b"]);
        assert_eq!(wrapped(json!(r#"{"calls": [{"name": "mcp_bipagents_link_confirm_payment"}]}"#)), ["link_confirm_payment"]);
    }

    #[test]
    fn a_line_goes_right_after_its_turns_item_before_it_never_in_another_conversation() {
        let mut items = vec![
            item("u1", "u1", 100.0, "message"),
            item("t1", "u1", 101.0, "tool"),
            item("a1", "u1", 140.0, "message"),
            item("u2", "u2", 200.0, "message"),
        ];
        let recorded = vec![
            Recorded { at: 120.0, session: "s1".into(), text: "Paiement annulé à la demande de Vie. Rien n'a été payé.".into() },
            Recorded { at: 130.0, session: "other".into(), text: "elsewhere".into() },
            Recorded { at: 50.0, session: "s1".into(), text: "before any turn".into() },
        ];
        with_pay_lines(&mut items, &recorded);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["u1", "t1", "u1.pay120000", "a1", "u2"]);
        let line = &items[2];
        assert_eq!(line.kind(), "notice");
        assert_eq!(line.body["notice"]["code"], CODE);
        assert_eq!(line.body["notice"]["text"], "Paiement annulé à la demande de Vie. Rien n'a été payé.");
        assert_eq!(line.turn, "u1");
        assert!(line.pos > items[1].pos && line.pos < items[3].pos);
    }

    #[test]
    fn only_lines_with_a_conversation_are_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        record(&file, &json!({"at": 1.0, "agent": "vie", "session": null, "run": null, "text": "x"}));
        record(&file, &json!({"at": 2.0, "agent": "vie", "session": "s1", "run": "r1", "text": "y"}));
        record(&file, &json!({"at": 3.0, "agent": "leo", "session": "s1", "run": "r2", "text": "z"}));
        assert_eq!(recorded(&file, "vie"), vec![Recorded { at: 2.0, session: "s1".into(), text: "y".into() }]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn the_words_in_each_language() {
        assert_eq!(
            total_differs("fr", "27,97 €", "24,50 €"),
            "Paiement annulé : la page demandait 27,97 €, tu avais accepté 24,50 €. Rien n'a été payé."
        );
        assert_eq!(
            total_differs("en", "€27.97", "€24.50"),
            "Payment cancelled: the page asked for €27.97, you had approved €24.50. Nothing was paid."
        );
        for lang in ["de", "es", "nl", "fr-FR"] {
            let line = total_differs(lang, "27,97 €", "24,50 €");
            assert!(line.contains("27,97 €") && line.contains("24,50 €"), "{lang}: {line}");
        }
        assert_eq!(agent_cancelled("fr", "Vie", false, false), "Paiement annulé à la demande de Vie. Rien n'a été payé.");
        assert_eq!(agent_cancelled("en", "Vie", false, true), "Payment cancelled at Vie's request.");
        assert!(agent_cancelled("de", "Vie", true, false).contains("Bank"));
        assert!(!agent_cancelled("nl", "Vie", true, false).contains("niets betaald"));
        for lang in ["fr", "en", "de", "es", "nl"] {
            assert!(suspended(lang).len() > 20 && total_changed(lang).len() > 20, "{lang}");
            assert!(foreign_currency(lang, "32,10 $").contains("32,10 $"), "{lang}");
            assert!(higher_amount(lang, "32,87 €", "27,97 €").contains("32,87 €"), "{lang}");
            assert!(stated_differs(lang, "Vie", "32,87 €", "27,97 €").contains("Vie"), "{lang}");
            assert!(unverified(lang).len() > 20, "{lang}");
        }
        assert_eq!(suspended("fr"), "Paiement arrêté : tu as suspendu les paiements. Rien n'a été payé.");
    }
}
