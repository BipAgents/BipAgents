//! Hermes' own words when it halts a turn, kept from the person (20/11, Alex in a Live call with Vie: after « Laisse-moi
//! vérifier sur le web, deux secondes », the call's conversation got « I stopped retrying because I kept running
//! web_search 50 times without making progress. The last result above shows what blocked it. Tell me how you'd like to
//! proceed, or send `continue` and I'll try a different approach. »). Hermes writes these lines for a developer at its
//! terminal: English, a tool's name, a count, slash commands the apps don't have. They are the turn's reply (streamed as
//! its last delta, stored as the assistant's message), so they reach the conversation, a push, the call record and a
//! Live call's voice unless the bridge swaps them, as it does for « ⚠️ No reply: » (no_reply.rs) and the file-mutation
//! verifier (hermes_footer.rs).
//!
//! The halts of Hermes v0.23.1 told here (all English only, none in Hermes' `locales/`):
//!
//! - its tool-loop guardrail's (`run_agent.py` `_toolguard_controlled_halt_response`): « I stopped retrying because I
//!   kept running <tool> <n> times without making progress. … »;
//! - a reply stopped for repeating itself (`agent/turn_truncation.py` `repetition_copy`): « ⚠️ **Response Stopped —
//!   Repetition Detected** … `/model` … »;
//! - a reply whose thinking used every output token (`_THINKING_EXHAUSTED`): « ⚠️ **Thinking Budget Exhausted** …
//!   `/reasoning low` … `/model` ».
//!
//! Not « Operation interrupted: waiting for model response (12.3s elapsed). » (`INTERRUPT_WAITING_FOR_MODEL_PREFIX`):
//! a person's own Stop ends on it, and the apps tell it already (`isInterruptionLine`); a Live turn the bridge's brake
//! stopped ends on it too, and there the brake's line replaces it (live_turns.rs `LiveBrake`, live_calls.rs).
//!
//! The guard's halt of `web_search` is its per-turn cap (`loop_web_search_cap`: 50, the only halt of a person's turn,
//! whose hard stops are off) unless its count is small: the searches answered, the model kept searching, so it reads
//! as the brake's « too many searches » line; a small count (a routine's failures) reads « can't search the web ».
//!
//! Only a reply that IS one of them (its text starts with it, or it is the last paragraph after the agent's own words)
//! is swapped: an agent quoting one mid-sentence keeps its words.

use serde_json::Value;

use crate::hermes::SSEEvent;
use crate::loop_brake::bare;

/// Which of Hermes' halts a reply is.
#[derive(Debug, Clone, PartialEq)]
pub enum Halt {
    /// The tool-loop guardrail stopped the turn: the tool it kept calling (bare name, « » when Hermes named none) and
    /// how many times.
    ToolLoop(String, u32),
    /// The reply kept repeating itself and was cut.
    Repetition,
    /// The model's thinking used all its output tokens: no answer.
    ThinkingExhausted,
}

/// Hermes' guard halting `web_search` this many times or more: its per-turn cap, the searches answered (above the
/// brake's own web search limits, 20 and 6, and Hermes' failure halts, 5 and 8).
const MANY_SEARCHES: u32 = 10;

const TOOL_LOOP: &str = "I stopped retrying because I kept running ";
const TOOL_LOOP_END: &str = " times without making progress";
const REPETITION: &str = "⚠️ **Response Stopped — Repetition Detected**";
const THINKING: &str = "⚠️ **Thinking Budget Exhausted**";

/// The halt `text` starts with (spaces before it aside), if any.
fn starts_as(text: &str) -> Option<Halt> {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix(TOOL_LOOP) {
        let named = rest.find(TOOL_LOOP_END).map(|end| &rest[..end])?;
        // « <tool> <n> »: Hermes' `decision.tool_name or 'the same tool'`, then the count
        let (tool, count) = named.rsplit_once(' ')?;
        let count = count.parse::<u32>().ok()?;
        let tool = if tool == "the same tool" { "" } else { tool };
        return Some(Halt::ToolLoop(bare(tool).to_string(), count));
    }
    if text.starts_with(REPETITION) {
        return Some(Halt::Repetition);
    }
    if text.starts_with(THINKING) {
        return Some(Halt::ThinkingExhausted);
    }
    None
}

/// Which halt `text` is or ends on: (the agent's own words before it, trimmed; the halt). None: an answer.
pub fn find(text: &str) -> Option<(&str, Halt)> {
    if let Some(halt) = starts_as(text) {
        return Some(("", halt));
    }
    // Hermes' halt after words the agent streamed first (« Laisse-moi vérifier… » then the halt): its last paragraph
    let at = text.match_indices("\n\n").map(|(i, _)| i + 2).filter(|&i| starts_as(&text[i..]).is_some()).last()?;
    let halt = starts_as(&text[at..])?;
    Some((text[..at].trim_end(), halt))
}

fn pick(lang: &str, fr: &str, en: &str, de: &str, es: &str, nl: &str) -> String {
    match lang.split(['-', '_']).next().unwrap_or("en").to_lowercase().as_str() {
        "fr" => fr,
        "de" => de,
        "es" => es,
        "nl" => nl,
        _ => en,
    }
    .to_string()
}

/// The line the person reads (and hears in a Live call) instead, in `lang` (fr, en, de, es, nl; others read English).
pub fn plain(halt: &Halt, lang: &str) -> String {
    match halt {
        Halt::ToolLoop(tool, count) if tool == "web_search" && *count >= MANY_SEARCHES => {
            crate::loop_brake::notice_text(&crate::loop_brake::Over::Searches, lang)
        }
        Halt::ToolLoop(tool, _) => crate::loop_brake::notice_text(&crate::loop_brake::Over::Failing(tool.clone()), lang),
        Halt::Repetition => pick(
            lang,
            "Ma réponse partait en boucle, je l'ai arrêtée. Tu peux me reposer la question ?",
            "My answer was going round in circles, so I stopped it. Can you ask me again?",
            "Meine Antwort hat sich ständig wiederholt, deshalb habe ich sie abgebrochen. Kannst du mich noch einmal fragen?",
            "Mi respuesta se repetía en bucle, así que la he detenido. ¿Me lo puedes volver a preguntar?",
            "Mijn antwoord bleef zich herhalen, dus ik heb het gestopt. Kun je het me opnieuw vragen?",
        ),
        Halt::ThinkingExhausted => pick(
            lang,
            "Je n'ai pas réussi à formuler de réponse cette fois. Tu peux me reposer la question ?",
            "I couldn't put an answer together this time. Can you ask me again?",
            "Ich habe diesmal keine Antwort zustande gebracht. Kannst du mich noch einmal fragen?",
            "Esta vez no he conseguido formular una respuesta. ¿Me lo puedes volver a preguntar?",
            "Het is me deze keer niet gelukt een antwoord te geven. Kun je het me opnieuw vragen?",
        ),
    }
}

/// `text` as the person reads it when it is (or ends on) one of Hermes' halts: the agent's own words kept, the halt in
/// plain words. None: an answer, kept as it is.
pub fn in_plain_words(text: &str, lang: &str) -> Option<String> {
    let (words, halt) = find(text)?;
    let line = plain(&halt, lang);
    Some(if words.is_empty() { line } else { format!("{words}\n\n{line}") })
}

/// The text fields of a final answer's event (as hermes_footer.rs reads them) and a delta's.
const TEXT_KEYS: [&str; 7] = ["output", "content", "text", "final_output", "response", "result", "delta"];

/// Whether an event (a delta, a final answer) carries one of Hermes' halts: the cheap test before the language is read.
pub fn carries(event: &SSEEvent) -> bool {
    let final_answer = crate::hermes::is_terminal(&event.r#type) || event.r#type == "assistant.completed";
    (final_answer || event.r#type.ends_with(".delta"))
        && TEXT_KEYS.iter().any(|k| event.data.get(*k).and_then(Value::as_str).is_some_and(|t| find(t).is_some()))
}

/// An event whose text is one of Hermes' halts (a delta that carries it whole, as Hermes streams it, or a final
/// answer's), with the plain line instead, its raw JSON written again; any other event as it is.
pub fn scrub_event(event: SSEEvent, lang: &str) -> SSEEvent {
    let final_answer = crate::hermes::is_terminal(&event.r#type) || event.r#type == "assistant.completed";
    if !(final_answer || event.r#type.ends_with(".delta")) {
        return event;
    }
    let mut data = event.data;
    let mut changed = false;
    for key in TEXT_KEYS {
        let Some(Value::String(text)) = data.get(key) else { continue };
        if let Some(plain) = in_plain_words(text, lang) {
            // a delta keeps the paragraph break Hermes put before it
            let lead: String = if key == "delta" { text.chars().take_while(|c| c.is_whitespace()).collect() } else { String::new() };
            data.insert(key.into(), Value::String(format!("{lead}{plain}")));
            changed = true;
        }
    }
    if !changed {
        return SSEEvent { data, ..event };
    }
    tracing::info!(kind = %event.r#type, "Hermes' halt in plain words");
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(event.r#type, data, raw)
}

/// A conversation's agent messages that are Hermes' halts, in plain words (threads/mod.rs `items`: the stored reply).
pub fn fold(items: &mut [crate::threads::timeline::Item], lang: &str) {
    for item in items.iter_mut() {
        if item.body.get("kind").and_then(Value::as_str) != Some("message")
            || item.body.get("role").and_then(Value::as_str) != Some("agent")
        {
            continue;
        }
        let Some(plain) = item.body.get("text").and_then(Value::as_str).and_then(|t| in_plain_words(t, lang)) else {
            continue;
        };
        item.body.insert("text".into(), Value::String(plain));
        item.body.remove("spoken");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What Alex read on 20/11 after a Live call (Hermes v0.23.1's `_toolguard_controlled_halt_response`).
    const SEEN: &str = "I stopped retrying because I kept running web_search 50 times without making progress. The last \
result above shows what blocked it. Tell me how you'd like to proceed, or send `continue` and I'll try a different approach.";
    /// The lab's GLM on 20/11, a search refused: Hermes' cut of a reply repeating itself.
    const REPEATED: &str = "⚠️ **Response Stopped — Repetition Detected**\n\nThe model fell into a repetition loop while \
writing this response, so the stream was stopped instead of running on. The partial response was discarded.\n\n→ Switch \
to a different model with `/model`\n→ Or resend your message (your conversation history is preserved)";

    #[test]
    fn hermes_halts_are_told_and_answers_are_not() {
        assert_eq!(find(SEEN), Some(("", Halt::ToolLoop("web_search".into(), 50))));
        assert_eq!(find(&format!("\n\n{SEEN}")), Some(("", Halt::ToolLoop("web_search".into(), 50))));
        assert_eq!(
            find("I stopped retrying because I kept running the same tool 8 times without making progress. x"),
            Some(("", Halt::ToolLoop(String::new(), 8)))
        );
        assert_eq!(
            find("I stopped retrying because I kept running mcp_bipagents_calendar_add 5 times without making progress."),
            Some(("", Halt::ToolLoop("calendar_add".into(), 5)))
        );
        assert_eq!(find(REPEATED), Some(("", Halt::Repetition)));
        assert_eq!(find("⚠️ **Thinking Budget Exhausted**\n\nThe model used all its output tokens"), Some(("", Halt::ThinkingExhausted)));
        // a person's own Stop: the apps tell it (`isInterruptionLine`), as it is here
        assert_eq!(find("Operation interrupted: waiting for model response (12.3s elapsed)."), None);
        // after the agent's own words, in the same reply
        assert_eq!(
            find(&format!("Laisse-moi vérifier sur le web, deux secondes.\n\n{SEEN}")),
            Some(("Laisse-moi vérifier sur le web, deux secondes.", Halt::ToolLoop("web_search".into(), 50)))
        );
        for said in [
            "Voilà, c'est Beatmania.",
            "Hermes écrit « I stopped retrying because I kept running web_search 50 times without making progress ».",
            "I stopped retrying because the shop was closed.",
            "⚠️ Attention : il pleut.",
        ] {
            assert_eq!(find(said), None, "{said}");
        }
    }

    #[test]
    fn the_person_reads_a_plain_line_in_their_language() {
        // Alex's 20/11: Hermes' cap of 50, every search answered (the place's log): the « too many searches » line
        let many = "J'ai fait beaucoup de recherches sur le web sans trouver de réponse sûre. Dis-moi si tu veux que j'en fasse une recherche approfondie en tâche de fond.";
        assert_eq!(in_plain_words(SEEN, "fr").as_deref(), Some(many));
        assert_eq!(in_plain_words(&format!("Je regarde.\n\n{SEEN}"), "fr"), Some(format!("Je regarde.\n\n{many}")));
        // a few failures (a routine's hard stop at 8): « can't search the web »
        assert_eq!(
            in_plain_words("I stopped retrying because I kept running web_search 8 times without making progress.", "fr")
                .as_deref(),
            Some("Je n'arrive pas à chercher sur le web pour l'instant. Tu veux que je réessaie plus tard ?")
        );
        for lang in ["fr", "en", "de", "es", "nl", "it"] {
            for halt in [Halt::ToolLoop("web_search".into(), 50), Halt::ToolLoop("web_search".into(), 3),
                Halt::ToolLoop(String::new(), 8), Halt::Repetition, Halt::ThinkingExhausted]
            {
                let line = plain(&halt, lang);
                assert!(!line.contains('`') && !line.contains("/model") && !line.contains("web_search"), "{lang} {halt:?}");
                assert!(!line.is_empty());
            }
        }
        assert_eq!(plain(&Halt::Repetition, "nl"), "Mijn antwoord bleef zich herhalen, dus ik heb het gestopt. Kun je het me opnieuw vragen?");
    }

    #[test]
    fn a_stream_says_the_plain_line_in_its_delta_and_its_end() {
        let delta = SSEEvent::new("assistant.delta", json!({"delta": format!("\n\n{SEEN}")}).as_object().unwrap().clone(), String::new());
        let said = scrub_event(delta, "fr");
        assert!(said.data["delta"].as_str().unwrap().starts_with("\n\nJ'ai fait beaucoup de recherches sur le web"));
        assert!(said.raw.contains("recherches sur le web"));
        let end = SSEEvent::new("assistant.completed", json!({"content": SEEN, "run_id": "r"}).as_object().unwrap().clone(), String::new());
        assert!(scrub_event(end, "en").data["content"].as_str().unwrap().starts_with("I ran a lot of web searches"));
        // an answer's delta, and a tool's event, as they are
        let words = SSEEvent::new("assistant.delta", json!({"delta": "Beatmania"}).as_object().unwrap().clone(), "raw");
        assert_eq!(scrub_event(words, "fr").raw, "raw");
        let tool = SSEEvent::new("tool.started", json!({"preview": SEEN}).as_object().unwrap().clone(), "raw");
        assert_eq!(scrub_event(tool, "fr").raw, "raw");
    }
}
