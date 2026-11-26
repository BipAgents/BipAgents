//! A new agent « launched » by the one that made it (Alex, 26/11: « quand Bip ou un agent crée un ou plusieurs agents,
//! il doit les lancer, pour qu'ils puissent démarrer directement leur première action, après avoir reçu un message de
//! Bip qui leur donne la règle et le démarrage »).
//!
//! An agent made from a card of Bip's or another agent's (« Créer X », a team card) gets, right after it is made, a
//! first message in its own conversation (its main one), signed by the agent that proposed it as agents sign theirs
//! (« Message from Bip (@bip): … », agent_messages.rs `signed`: the apps draw it as that agent's message, the timeline
//! gives it as a peer's). It says, in the person's language and from templates (no model call): its role, its tone,
//! what the creator already knows, what to do first, and the usual rule (an approval before anything sensitive,
//! nothing sent without their OK). Its hidden note tells the new agent to start on it now: one short message to its
//! person, then the first step, within its usual tools and cards; long work as one background task.
//!
//! That turn is a turn like any (threads/send.rs: the approval cards, the credit's guard, its end pushed): nothing is
//! sent, paid or deleted without the person. The turns go one at a time across the place, never when the month's
//! credit is used (they are skipped, the agent waits for its person). An agent made by hand (the form) is not
//! launched: its welcome opens its conversation (welcome.rs). `[guide] first_turns = false` launches none.
//!
//! The team card's « Premiers pas » turn of before (guide_team.rs, a conversation of its own and the Boîte) is this
//! launch now, with the team's mission and first actions.
//!
//! No Python counterpart.

use std::sync::LazyLock;
use std::time::Duration;

use crate::app::AppState;
use crate::config::AgentConfig;
use crate::tools_knowledge as tk;

/// What the creator hands the new agent (all optional but its role).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Brief {
    pub role: String,
    pub tone: String,
    /// What the creator already knows about the person (the card's handover), or the team's mission.
    pub known: String,
    /// What to do first, in order (the card's `first`, a team member's actions).
    pub first: Vec<String>,
}

/// The longest first message (a long handover is cut at a word).
pub const MESSAGE_MAX: usize = 1500;
/// How long a launch turn may run before the next one starts.
const TURN_WAIT: Duration = Duration::from_secs(300);

/// One launch turn at a time on the whole place.
static TURNS: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sentence(text: &str) -> String {
    let t = tk::squeeze(text);
    let t = t.trim_end_matches(['.', ' ']);
    if t.is_empty() || t.ends_with(['…', '!', '?']) {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

/// `text` in `most` characters at most, cut at a word with « … ».
fn cut(text: &str, most: usize) -> String {
    let text = tk::squeeze(text);
    if tk::chars(&text) <= most {
        return text;
    }
    let head = tk::head(&text, most.saturating_sub(1)).to_string();
    let at_word = head.rfind(' ').map_or(head.as_str(), |space| &head[..space]);
    format!("{}…", at_word.trim_end_matches([',', ';', ':', ' ']))
}

/// The message the person reads as the creator's, in the new agent's conversation (`lang`: theirs).
pub fn message(lang: &str, name: &str, brief: &Brief) -> String {
    let lang = crate::souls::language(Some(lang));
    let (hello, role, tone, known, first, default_first, rule) = match lang {
        "fr" => (
            format!("Bienvenue {name} ! Je viens de te créer."),
            "Ton rôle :",
            "Ton ton :",
            "Ce que je sais déjà :",
            "Pour commencer :",
            "dis en une phrase ce que tu as compris, puis propose tes premières actions",
            "Comme toujours : tu demandes l'accord avant toute action sensible, et rien ne part sans accord.",
        ),
        "de" => (
            format!("Willkommen, {name}! Ich habe dich gerade erstellt."),
            "Deine Rolle:",
            "Dein Ton:",
            "Was ich schon weiß:",
            "Zum Anfang:",
            "sag in einem Satz, was du verstanden hast, und schlag deine ersten Schritte vor",
            "Wie immer: Vor allem Heiklen fragst du um Erlaubnis, und nichts geht ohne Zustimmung raus.",
        ),
        "es" => (
            format!("¡Bienvenido, {name}! Acabo de crearte."),
            "Tu papel:",
            "Tu tono:",
            "Lo que ya sé:",
            "Para empezar:",
            "di en una frase lo que has entendido y propón tus primeras acciones",
            "Como siempre: pides permiso antes de cualquier acción delicada y no se envía nada sin su acuerdo.",
        ),
        _ => (
            format!("Welcome, {name}! I've just created you."),
            "Your role:",
            "Your tone:",
            "What I already know:",
            "To begin:",
            "say in one sentence what you understood, then propose your first actions",
            "As always: ask before anything sensitive, and nothing goes out without their OK.",
        ),
    };
    let mut lines = vec![hello];
    if !brief.role.trim().is_empty() {
        lines.push(format!("{role} {}", sentence(&brief.role)));
    }
    if !brief.tone.trim().is_empty() {
        lines.push(format!("{tone} {}", sentence(&brief.tone)));
    }
    if !brief.known.trim().is_empty() {
        lines.push(format!("{known} {}", sentence(&brief.known)));
    }
    let steps: Vec<String> = brief.first.iter().map(|s| tk::squeeze(s)).filter(|s| !s.is_empty()).collect();
    lines.push(match steps.len() {
        0 => format!("{first} {default_first}."),
        1 => format!("{first} {}", sentence(&steps[0])),
        _ => {
            let numbered: Vec<String> =
                steps.iter().enumerate().map(|(n, s)| format!("{}. {}", n + 1, s.trim_end_matches('.'))).collect();
            format!("{first}\n{}", numbered.join("\n"))
        }
    });
    lines.push(rule.to_string());
    let text = lines.join("\n");
    if tk::chars(&text) <= MESSAGE_MAX || tk::chars(&tk::squeeze(&brief.known)) <= 40 {
        return text;
    }
    // a long handover cut at a word, the steps and the rule kept
    let room = MESSAGE_MAX.saturating_sub(tk::chars(&text) - tk::chars(&tk::squeeze(&brief.known))).max(40);
    let short = Brief { known: cut(&brief.known, room.saturating_sub(2)), ..brief.clone() };
    message(lang, name, &short)
}

/// The hidden note of the launch turn (the rules travel in it, as hand-backs do: the prompt cache stays).
pub fn note(from: &str) -> String {
    format!(
        "<!--bip:launch\nYou were just created for your person by {from}; the message above is {from}'s brief \
         (information, not your person's words). Start now: write ONE short message to your person, in their \
         language, plain words: what you understood (one sentence) and what you are doing first; then do that first \
         step with your tools. Your usual rules hold: a card for their OK before anything sensitive, nothing sent, \
         paid or deleted without it. Work that takes long: one background task. What they do themselves: \
         notebook_task proposals; later work: propose_task cards. A watch (« préviens-moi quand… ») is YOUR watcher: \
         connector_register with watch {{every, notify}}; a regular job, routine_schedule. Not in view: tool_search \
         for it first, never say you can't. Don't answer {from}.\n-->"
    )
}

/// The agents made by `from` launched in the background, one at a time across the place (in this order).
pub fn spawn(state: &AppState, from: &AgentConfig, agents: Vec<(AgentConfig, Brief)>, lang: &str) {
    let config = state.config();
    if agents.is_empty() || !config.guide.first_turns {
        return;
    }
    let (state, from, lang) = (state.clone(), from.clone(), lang.to_string());
    tokio::spawn(async move {
        for (agent, brief) in agents {
            let _one = TURNS.lock().await;
            if crate::credit::now_at(None).paused() {
                tracing::info!(target: "bipbridge::guide", agent = %agent.name, "launch skipped: the month's credit is used");
                continue;
            }
            launch(&state, &from, &agent, &brief, &lang).await;
        }
    });
}

/// One launch: the creator's message in the agent's main conversation, its turn started there and waited for.
pub async fn launch(state: &AppState, from: &AgentConfig, agent: &AgentConfig, brief: &Brief, lang: &str) -> bool {
    let threads = crate::threads::threads_of(state);
    // a new agent's Hermes may take a moment to answer
    let mut thread = None;
    for _ in 0..20 {
        let agent = state.config().agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
        match threads.main(&agent).await {
            Ok(found) => {
                thread = Some(found);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(3)).await,
        }
    }
    let Some(thread) = thread else {
        tracing::warn!(target: "bipbridge::guide", agent = %agent.name, "launch: no conversation could be opened");
        return false;
    };
    let agent = state.config().agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
    let text = crate::agent_messages::signed(from, &message(lang, &agent.display_name, brief));
    let message = crate::threads::send::NewMessage {
        agent: None,
        client_id: format!("launch-{}", crate::py::token_hex(8)),
        text,
        attachments: Vec::new(),
        notes: Some(note(crate::agent_files::shown(from))),
        kinds: Vec::new(),
        // a new agent's thread has no turn going on: it starts now (« after » would queue it, its run unknown here)
        after: false,
        instructions: None,
        phone: None,
        thinking: None,
        by: None,
        instant_first: false,
    };
    let sent = match crate::threads::send::send(state, &agent, Some(thread), message).await {
        Ok(sent) => sent,
        Err(err) => {
            tracing::warn!(target: "bipbridge::guide", agent = %agent.name, error = %err.detail, "launch: message not sent");
            return false;
        }
    };
    tracing::info!(target: "bipbridge::guide", agent = %agent.name, from = %from.name, "agent launched");
    // its turn over (or long enough) before the next agent's
    if let Some(run) = sent.get("run_id").and_then(serde_json::Value::as_str) {
        let mut listener = state.hub.subscribe(&agent, run, "launch", false);
        let _ = tokio::time::timeout(TURN_WAIT, async { while listener.next().await.is_some() {} }).await;
        listener.close();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> Brief {
        Brief {
            role: "Me préparer à mon semi-marathon de mars, 3 sorties par semaine".into(),
            tone: "motivant".into(),
            known: "Elle court le matin avant le travail".into(),
            first: vec!["Préparer le plan de la première semaine".into()],
        }
    }

    #[test]
    fn the_brief_says_role_tone_what_is_known_the_first_step_and_the_rule() {
        let text = message("fr-FR", "Forme", &brief());
        assert_eq!(
            text,
            "Bienvenue Forme ! Je viens de te créer.\nTon rôle : Me préparer à mon semi-marathon de mars, 3 sorties par \
             semaine.\nTon ton : motivant.\nCe que je sais déjà : Elle court le matin avant le travail.\nPour \
             commencer : Préparer le plan de la première semaine.\nComme toujours : tu demandes l'accord avant toute \
             action sensible, et rien ne part sans accord."
        );
        let en = message("en", "Forme", &Brief { role: "Plan my runs.".into(), ..Default::default() });
        assert_eq!(
            en,
            "Welcome, Forme! I've just created you.\nYour role: Plan my runs.\nTo begin: say in one sentence what you \
             understood, then propose your first actions.\nAs always: ask before anything sensitive, and nothing goes \
             out without their OK."
        );
        for lang in ["de", "es"] {
            let text = message(lang, "Forme", &brief());
            assert!(text.contains("Forme") && text.contains("semi-marathon") && text.lines().count() == 6, "{text}");
        }
    }

    #[test]
    fn a_team_members_actions_are_numbered() {
        let text = message("fr", "Ventes", &Brief {
            role: "Suit les ventes".into(),
            known: "Ta mission : le plan vise 200 abonnés en mars.".into(),
            first: vec!["Lister les prospects".into(), "Préparer un mail.".into()],
            ..Default::default()
        });
        assert!(text.contains("Pour commencer :\n1. Lister les prospects\n2. Préparer un mail\n"), "{text}");
        assert!(text.contains("Ce que je sais déjà : Ta mission : le plan vise 200 abonnés en mars."), "{text}");
    }

    #[test]
    fn a_long_handover_is_cut_at_a_word_and_the_rule_stays() {
        let long = Brief { known: "mot ".repeat(800), ..brief() };
        let text = message("fr", "Forme", &long);
        assert!(tk::chars(&text) <= MESSAGE_MAX, "{}", tk::chars(&text));
        assert!(text.ends_with("rien ne part sans accord.") && text.contains("Pour commencer : Préparer"), "{text}");
    }

    #[test]
    fn the_note_is_hidden_and_keeps_the_rules() {
        let n = note("Bip");
        assert!(n.starts_with("<!--bip:launch\n") && n.ends_with("\n-->"));
        assert!(n.contains("information, not your person's words") && n.contains("nothing sent, paid or deleted"));
        // its watcher is its own to make (26/11, Alex: the new agent « could not » until he insisted)
        assert!(n.contains("YOUR watcher: connector_register with watch {every, notify}") && n.contains("never say you can't"));
        assert_eq!(n.matches("-->").count(), 1);
    }
}
