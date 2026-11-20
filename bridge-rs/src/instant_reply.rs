//! The agent's line said at once to a message sent while it works (Alex 19/11, « comme Grok Bot »): « Compris, plus de
//! langue ni de dents, je l'intègre. » within about a second, instead of silence until the turn reaches its next step
//! (a minute during a browser action or a long tool).
//!
//! When the person's words go into the turn going on (`steered`, Hermes' steer: threads/send.rs) or wait behind it
//! (`queued` while a turn runs: files, « Après », a family member's message during someone else's turn), the bridge
//! asks the agent's own model (its address and key, as the voice escalation does: the offer's GLM through EUrouter,
//! a self-install's own model) for ONE short sentence in the conversation's language that restates what was added and
//! says it is taken into account (or « right after » when it waits). No tools, no thinking, 60 tokens (GLM through EUrouter
//! thinks at its floor: 400), `TIMEOUT`; past it the request is dropped, its connection closed (a server of the
//! person's stops generating).
//! A failed, empty, too long or questioning answer, or a model the bridge can't call: a sentence from a small table
//! (`CANNED`), turning. A turn restarted with the words (turn_restart.rs) answers by itself at once: nothing here.
//!
//! **A row of the bridge's, never Hermes'.** The sentence is kept in `thread_instant` (agent, client_id, thread, text)
//! and served in the timeline as an agent message `i_<client_id>` with `instant: true`, right after the person's
//! message it answers (its `s_` item, then Hermes' stored row: matched by `client_id`), or at its time while that
//! message waits in the queue. It survives a reload and shows on every device, but it never enters Hermes' history:
//! the agent's memory and context hold no line it did not write. The agent knows of it from a hidden note joined to
//! the words it gets (`MARK`, a `<!--bip:answered …-->` note: every reader of a person's row strips such notes wherever
//! they are, also inside the one row Hermes makes of several steers held until its next step): it was answered, so it does
//! not acknowledge again (Alex's « never repeat »), it acts.
//!
//! **Taken (the check).** Hermes stores a steered message when its turn reads it (after the tool calls going on,
//! before its next model call's tools run): the person's item is then a stored row with `added_during_run`, no longer
//! the `s_` item. The bridge watches the turn's steps after a steer and tells the phones (`thread` hint `taken`) once
//! the row is there; the apps draw a small check on that bubble. A turn that ends first says it with its own hint.
//!
//! **Cost.** One small call per message, with the agent's own key: on the offer, the place's AI key (issued by the
//! guichet, metered by the router), so it counts in the place's AI usage like every call of its agents; its tokens are
//! logged. At most one sentence per conversation every `GAP_SECONDS`: a burst of messages gets one sentence, then one
//! again only once spaced (the others get no line, and no note: the turn takes them as before).
//!
//! **The first message of a turn** (Alex 20/11, « dès le premier message »): an app that says it draws such a line
//! (`instant_first` in the message) gets one for a message that starts a turn too. The agent's model is asked at once,
//! in parallel with what the send does before starting the turn (`first`), for 1 to 3 short sentences: what the agent
//! starts on with the person's specifics (« Je compare les trains Lyon–Paris de samedi matin… »), or a short direct
//! answer when general knowledge is enough; « - » when the message needs no words yet (thanks, ok, a greeting). The
//! turn starts once the line is there or `FIRST_WAIT` (2.5 s) after the send at the latest: with the line, its words go to the
//! turn in the note (`first_note`), so the real reply continues from it and never says it again, and the line is kept
//! and shown at once (before any word of the turn's: none can come before its start). Past `FIRST_WAIT`, nothing is
//! shown and the turn starts as before. No canned line here (a « Noté » to a first message says nothing). Never in a
//! family agent's common conversation, a read-only share's turn (`instructions`), a « thanks » or « ok » (`trivial`:
//! no call at all), Live, crons or tasks (none of them goes through threads/send.rs).
//!
//! **Shared conversations.** A family agent's common conversation: a member's words wait behind someone else's turn
//! (never steer it, family_thread.rs); her sentence answers her message alone and is asked without the conversation's
//! other lines, so it says nothing the timeline doesn't already show next to her message. A relative's conversation
//! with a shared agent is hers alone (agent_shares.rs).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::AgentConfig;
use crate::threads::{threads_of, Thread};

/// The hidden note's mark, joined to the words the agent gets (the timeline cuts a person's row at such a line).
pub const MARK: &str = "<!--bip:answered";
/// The agent's model is given this long; past it, a sentence of the table.
pub const TIMEOUT: Duration = Duration::from_millis(3000);
/// One sentence per conversation at most this often.
pub const GAP_SECONDS: f64 = 20.0;
/// The longest sentence kept (characters); a longer answer is not one short sentence.
pub const SENTENCE_MAX: usize = 140;
/// The sentence's tokens at most (a model that cannot be told not to think gets its floor instead: GLM via EUrouter).
pub const MAX_TOKENS: i64 = 60;
/// A first message's line is waited for this long at most after the send before its turn starts without it.
pub const FIRST_WAIT: Duration = Duration::from_millis(2500);
/// The longest first-message line kept (characters): 1 to 3 short sentences.
pub const FIRST_MAX: usize = 360;
/// A first-message line's tokens at most (non-GLM: no thinking).
pub const FIRST_MAX_TOKENS: i64 = 160;
/// A turn's steps watched for the stored row this long at most after a steer.
const TAKEN_WATCH: Duration = Duration::from_secs(15 * 60);

/// Where the person's words went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Into the turn going on: taken into account now.
    Steered,
    /// Behind it: dealt with right after.
    Queued,
}

/// The sentences said when the model gives none, by language: (into the turn, behind it).
pub const CANNED: [(&str, [&str; 3], [&str; 3]); 5] = [
    (
        "fr",
        ["Noté, j'en tiens compte.", "Bien reçu, je l'intègre.", "C'est noté, je le prends en compte."],
        ["Noté, je m'en occupe juste après.", "Bien reçu, je regarde ça juste après.", "C'est noté, je m'y mets dès que j'ai fini."],
    ),
    (
        "en",
        ["Noted, I'm taking that into account.", "Got it, I'm adding that in.", "Noted, I'll factor that in."],
        ["Noted, I'll get to it right after.", "Got it, I'll look at that right after this.", "Noted, I'll handle it as soon as I'm done."],
    ),
    (
        "de",
        ["Notiert, ich berücksichtige das.", "Verstanden, ich nehme das mit auf.", "Alles klar, das fließt mit ein."],
        ["Notiert, ich kümmere mich gleich danach darum.", "Verstanden, das schaue ich mir gleich danach an.", "Alles klar, ich mache das, sobald ich fertig bin."],
    ),
    (
        "es",
        ["Anotado, lo tengo en cuenta.", "Entendido, lo incorporo.", "Apuntado, lo añado a lo que hago."],
        ["Anotado, me ocupo justo después.", "Entendido, lo miro justo después.", "Apuntado, me pongo en cuanto termine."],
    ),
    (
        "nl",
        ["Genoteerd, ik neem het mee.", "Begrepen, ik verwerk het erin.", "Genoteerd, ik houd er rekening mee."],
        ["Genoteerd, ik pak het direct daarna op.", "Begrepen, ik kijk er direct daarna naar.", "Genoteerd, ik ga ermee aan de slag zodra ik klaar ben."],
    ),
];

static TURN: AtomicUsize = AtomicUsize::new(0);

/// A sentence of the table, turning (never the same twice in a row).
pub fn canned(lang: &str, mode: Mode) -> &'static str {
    let row = CANNED.iter().find(|(l, _, _)| *l == lang).unwrap_or(&CANNED[1]);
    let list = if mode == Mode::Steered { &row.1 } else { &row.2 };
    list[TURN.fetch_add(1, Ordering::Relaxed) % list.len()]
}

/// The note joined to the words the agent gets: it was answered already.
pub fn note(mode: Mode) -> String {
    match mode {
        Mode::Steered => format!(
            "{MARK} The app already answered this message at once, in your name, with one short line saying you take \
it into account. Don't acknowledge, thank for or restate it again: just use it in your work and your answer.-->"
        ),
        Mode::Queued => format!(
            "{MARK} The app already answered this message at once, in your name, with one short line saying you deal \
with it right after. Don't acknowledge or restate it again: start on it directly.-->"
        ),
    }
}

/// The note joined to a first message: the line already shown, word for word, so the reply continues from it.
pub fn first_note(line: &str) -> String {
    // the line inside an HTML note: never a `-->` of its own
    let line = line.replace("--", "–");
    format!(
        "{MARK} The app already showed this at once, in your name, as the start of your reply: « {line} ». Continue \
from it: don't repeat, restate or acknowledge it, and skip your opening sentence; go straight to the work and the rest \
of the answer. If it already answers the message fully, never give that answer again: add only something new, in one \
short line.-->"
    )
}

/// A first message's words with the line's note after them.
pub fn with_first_note(words: &str, line: &str) -> String {
    format!("{}\n\n{}", words.trim_end(), first_note(line))
}

/// Words that need no line (thanks, ok, a greeting, an emoji): no call at all. A message with a file always gets one.
pub fn trivial(words: &str, files: bool) -> bool {
    const SMALL: &[&str] = &[
        "merci", "beaucoup", "ok", "okay", "oki", "d'accord", "dac", "daccord", "super", "cool", "top", "parfait", "génial",
        "nickel", "oui", "non", "bien", "très", "bonjour", "salut", "coucou", "bonsoir", "bravo", "yes", "no", "thanks",
        "thank", "you", "thx", "great", "perfect", "nice", "good", "hi", "hello", "hey", "sure", "fine", "danke", "schön",
        "gut", "ja", "nein", "hallo", "prima", "gracias", "vale", "sí", "si", "hola", "genial", "perfecto", "bedankt",
        "dank", "je", "dankjewel", "dankje", "oké", "goed", "hoi", "top", "mais", "ah", "oh", "ha", "haha", "mdr", "lol",
        "à", "plus", "toi", "a", "so", "much", "vielen", "muchas", "wel", "c'est", "ça", "marche", "très", "bien",
    ];
    if files {
        return false;
    }
    let lower = words.to_lowercase().replace('’', "'");
    let parts: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, '!' | '.' | ',' | '?' | ';' | ':'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\''))
        .filter(|w| !w.is_empty())
        .collect();
    parts.len() <= 4 && parts.iter().all(|w| SMALL.contains(w))
}

/// The first message's instructions: what the agent says at once, in its language.
pub fn first_system(agent_name: &str, role: &str, language: &str, now: &str) -> String {
    let role = if role.trim().is_empty() { String::new() } else { format!(" ({})", role.trim()) };
    format!(
        "You are {agent_name}{role}, a personal assistant working for your person through a chat app. They just sent \
the message below. Your full answer (with your tools, your memory, their accounts and the web) starts right after; \
first write what you say at once, in {language} (or in the language of their message if it is another).\n\
- If the message asks for work (searching, comparing, booking, browsing, writing or sending something, their mail, \
calendar, files or memory): 1 or 2 short sentences saying what you are starting on, with their specifics (places, \
dates, people, criteria). Example: « Je compare les trains Lyon–Paris de samedi matin, en évitant les correspondances. » \
Say only what you start on now (searching, comparing, reading), never what you will deliver, prepare, send or book \
(no « je te prépare… », « je t'envoie… », « je réserve… »).\n\
- If you can fully answer it now in 1 to 3 short sentences from general knowledge and the conversation alone (a word, \
a definition, a quick calculation, a simple tip): answer it directly. Money, health and legal questions are never \
quick: say what you will look at.\n\
- If it needs no words from you yet (thanks, ok, a greeting, a short reaction) or you are unsure: reply with a single -\n\
Never invent what you cannot know here: prices, times, schedules, availability, opening hours, news, weather, search \
results, anything from their mail, calendar, files, accounts or your memory. Never say that something is done, found, \
sent or booked. Never promise a time. Never ask a question. No greeting, no name, no emoji, no markdown, no list. \
Reply with the text only.\n\
It is now {now}."
    )
}

static SENTENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!…](?:\s|$)").expect("valid regex"));

/// The model's answer to a first message as the line shown, or None (« - », empty, a question, too long, too short).
pub fn clean_first(answer: &str) -> Option<String> {
    let text = THINK.replace_all(answer, "");
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = flat.replace(['*', '`', '#'], "");
    let mut line = flat.trim().trim_start_matches(['-', '–']).trim();
    // quotes around the whole line only (« Grazie mille » se dit… keeps its own)
    for (open, close) in [('«', '»'), ('“', '”'), ('"', '"')] {
        if let Some(inner) = line.strip_prefix(open).and_then(|l| l.strip_suffix(close)) {
            if !inner.contains(open) && !inner.contains(close) {
                line = inner.trim();
            }
        }
    }
    if line.contains('?') || line.contains('¿') || !line.chars().any(char::is_alphabetic) {
        return None;
    }
    // its first three sentences
    let cut = SENTENCE.find_iter(line).nth(2).map_or(line.len(), |m| m.start() + m.as_str().trim_end().len());
    let line = line[..cut].trim();
    if line.chars().count() > FIRST_MAX || line.split_whitespace().count() < 3 {
        return None;
    }
    let mut out = line.to_string();
    if !out.ends_with(['.', '!', '…']) {
        out.push('.');
    }
    Some(out)
}

/// The first message's request body: 1 to 3 sentences, no thinking (GLM through EUrouter: its floor, with room).
pub fn first_body(call: &Call, system: &str, user: &str, provider_order: &[String]) -> Value {
    let mut b = body(call, system, user, provider_order);
    if !crate::agents_admin::keeps_thinking_on(&call.model, &call.base_url) {
        b["max_tokens"] = json!(FIRST_MAX_TOKENS);
    }
    b
}

/// A first message's line, asked at once in the background: its words and tokens (input, output), or None past
/// `FIRST_WAIT` (the request dropped then), « - », an answer cut by its token cap, a refusal or a model the bridge
/// can't call. Awaited before the turn
/// starts (threads/send.rs).
pub fn first(state: &AppState, agent: &AgentConfig, thread: &Thread, client_id: &str, words: &str) -> tokio::task::JoinHandle<Option<(String, i64, i64)>> {
    let (state, agent, thread, client_id, words) =
        (state.clone(), agent.clone(), thread.clone(), client_id.to_string(), words.to_string());
    tokio::spawn(async move {
        let started = Instant::now();
        let config = state.config();
        let asked = async {
            let model = crate::mail_scan::model_of(&config, &agent).ok()?;
            let call = Call { base_url: model["base_url"].clone(), key: model["key"].clone(), model: model["model"].clone() };
            let lang = crate::handback::language_of(&config, &agent);
            let lines = last_lines(&state, &agent, &thread, &client_id).await;
            let shown = if agent.display_name.is_empty() { agent.name.clone() } else { agent.display_name.clone() };
            let now = crate::dates::now::line(&config, &agent);
            let now = now.trim_start_matches(crate::dates::now::OPEN).trim_end_matches("-->");
            let system = first_system(&shown, agent.category.as_deref().unwrap_or(""), crate::souls::language_name(&lang), now);
            let body = first_body(&call, &system, &user_message(&lines, &words), &config.model_provider_order);
            ask_raw(&state.http, &call, &body).await
        };
        let got = tokio::time::timeout(FIRST_WAIT, asked).await;
        let ms = started.elapsed().as_millis() as u64;
        match got {
            Ok(Some((raw, input, output, cut))) => {
                // cut by its token cap (GLM thinking at length): half a sentence, never shown
                let said = if cut { None } else { clean_first(&raw) };
                tracing::info!(agent = %agent.name, tokens_in = input, tokens_out = output, ms, kept = said.is_some(),
                               "instant reply (first message): the agent's model asked");
                said.map(|s| (s, input, output))
            }
            Ok(None) => None,
            Err(_) => {
                tracing::info!(agent = %agent.name, ms, "instant reply (first message): too slow, none");
                None
            }
        }
    })
}

/// The words with the note (after them: the timeline cuts the row there).
pub fn with_note(words: &str, mode: Mode) -> String {
    format!("{}\n\n{}", words.trim_end(), note(mode))
}

static NOTE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n*<!--bip:answered[\s\S]*?(?:-->|$)").expect("valid regex"));

/// Words without the note (a steer the turn did not take, back in the queue: msg_queue.rs; what search, the Carnet
/// and the safety net read of a person's row).
pub fn without_note(text: &str) -> String {
    if !text.contains(MARK) {
        return text.to_string();
    }
    NOTE_LINE.replace_all(text, "").trim_end().to_string()
}

static LAST: LazyLock<Mutex<HashMap<(String, String), f64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether this conversation gets a sentence now (`GAP_SECONDS` since its last), noted at once when it does.
pub fn reserve(agent: &str, thread: &str, now: f64) -> bool {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let key = (agent.to_string(), thread.to_string());
    if last.get(&key).is_some_and(|at| now - at < GAP_SECONDS) {
        return false;
    }
    last.retain(|_, at| now - *at < GAP_SECONDS);
    last.insert(key, now);
    true
}

/// The slot taken by `reserve` given back (the words never went anywhere: no line was said).
pub fn release(agent: &str, thread: &str) {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).remove(&(agent.to_string(), thread.to_string()));
}

/// The model's instructions.
pub fn system(agent_name: &str, language: &str, mode: Mode) -> String {
    let what = match mode {
        Mode::Steered => "says you take it into account in what you are doing now",
        Mode::Queued => "says you deal with it right after what you are doing now",
    };
    format!(
        "You are {agent_name}, a personal assistant, busy working on a task for your person. They just sent a new \
message while you work. Write the ONE short sentence you say back at once, in {language} (or in the language of their \
new message if it is another), that restates in a few words what they just added and {what}.\n\
Rules: one sentence, at most 15 words, warm and plain like a chat message. Never promise a result, a time or that \
something is done. Never ask a question. No greeting, no name, no emoji, no quotes, no markdown. You use no tool. \
Reply with the sentence only."
    )
}

/// What the model reads: the last lines of the conversation (oldest first), then the new message.
pub fn user_message(lines: &[(String, String)], words: &str) -> String {
    let mut out = String::new();
    if !lines.is_empty() {
        out.push_str("The conversation so far (latest last):\n");
        for (who, text) in lines {
            out.push_str(&format!("{who}: {}\n", cut(text, 300)));
        }
        out.push('\n');
    }
    out.push_str(&format!("Their new message: {}", cut(words.trim(), 1200)));
    out
}

fn cut(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    }
}

static THINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<think>.*?(?:</think>|$)").expect("valid regex"));
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!…](?:\s|$)").expect("valid regex"));

/// The model's answer as the sentence said, or None (empty, a question, too long, not words).
pub fn clean(answer: &str) -> Option<String> {
    let text = THINK.replace_all(answer, "");
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_matches(|c: char| matches!(c, '"' | '«' | '»' | '“' | '”' | '*' | '`') || c.is_whitespace());
    // its first sentence
    let line = match SENTENCE_END.find(line) {
        Some(m) => line[..m.start() + m.as_str().trim_end().len()].trim(),
        None => line,
    };
    if line.is_empty() || line.contains('?') || line.contains('¿') || line.chars().count() > SENTENCE_MAX {
        return None;
    }
    if !line.chars().any(char::is_alphabetic) {
        return None;
    }
    let mut out = line.to_string();
    if !out.ends_with(['.', '!', '…']) {
        out.push('.');
    }
    Some(out)
}

/// The model to ask, as the voice escalation does: (base url, key, model).
pub struct Call {
    pub base_url: String,
    pub key: String,
    pub model: String,
}

/// The request's body: 60 tokens, no thinking (GLM through EUrouter can't switch it off: its floor, with room).
pub fn body(call: &Call, system: &str, user: &str, provider_order: &[String]) -> Value {
    let mut body = json!({"model": call.model, "temperature": 0.3, "max_tokens": MAX_TOKENS,
                          "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}]});
    if crate::agents_admin::keeps_thinking_on(&call.model, &call.base_url) {
        // « low »: about one token of thinking on so short a task (agents_admin.rs); room to answer all the same
        body["max_tokens"] = json!(400);
    } else {
        // asked not to think, as Hermes' patch 0010 and the rooms' router do (a thinking model answered nothing)
        body["chat_template_kwargs"] = json!({"enable_thinking": false, "thinking": false});
        // one sentence of 15 words at most: a self-hosted single GPU the running turn shares does little for it
        body["max_tokens"] = json!(MAX_TOKENS);
        if crate::model_route::pins(&call.base_url) {
            // the router ignores the template's switch: DeepSeek V4.1 Flash thought ~160 tokens (+1 s) without this
            // (bench 19/11); « none » there answers at once
            body["reasoning_effort"] = json!("none");
        }
    }
    crate::model_route::add_to_body(provider_order, &call.base_url, &mut body);
    body
}

/// One call: the sentence (cleaned) and the tokens, or None.
pub async fn ask(http: &reqwest::Client, call: &Call, body: &Value) -> Option<(String, i64, i64)> {
    let (raw, input, output, _) = ask_raw(http, call, body).await?;
    Some((clean(&raw)?, input, output))
}

/// One call: the model's words as they came, the tokens, and whether its token cap cut them; None (refused,
/// unreachable).
pub async fn ask_raw(http: &reqwest::Client, call: &Call, body: &Value) -> Option<(String, i64, i64, bool)> {
    let mut request = http.post(format!("{}/chat/completions", call.base_url.trim_end_matches('/'))).json(body);
    if !call.key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", call.key));
    }
    let response = request.send().await.ok()?;
    let status = response.status().as_u16();
    if status >= 400 {
        tracing::info!(status, "instant reply: the agent's model refused");
        return None;
    }
    let data: Value = response.json().await.ok()?;
    let n = |k: &str| data.pointer(&format!("/usage/{k}")).and_then(Value::as_i64).unwrap_or(0);
    let said = data.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("").to_string();
    let cut = data.pointer("/choices/0/finish_reason").and_then(Value::as_str) == Some("length");
    Some((said, n("prompt_tokens"), n("completion_tokens"), cut))
}

/// The conversation's last visible lines, for the model (none in a family agent's common conversation).
async fn last_lines(state: &AppState, agent: &AgentConfig, thread: &Thread, client_id: &str) -> Vec<(String, String)> {
    let Ok(items) = crate::threads::items_of(state, agent, thread).await else { return Vec::new() };
    let mut lines: Vec<(String, String)> = items
        .iter()
        .filter(|i| i.kind() == "message" && !i.text().trim().is_empty() && !is_instant(&i.body))
        .filter(|i| i.body.get("client_id").and_then(Value::as_str) != Some(client_id))
        .map(|i| (author(&i.body), i.text().to_string()))
        .collect();
    let from = lines.len().saturating_sub(6);
    lines.drain(..from);
    lines
}

/// Who wrote a line, as the model reads it: the person, the agent itself, or another agent (Bot Chat: its name).
pub fn author(body: &serde_json::Map<String, Value>) -> String {
    let s = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("");
    match s("role") {
        "agent" => "You".into(),
        "peer" => format!("Another agent ({})", if s("from_name").is_empty() { s("from") } else { s("from_name") }),
        _ => "Person".into(),
    }
}

/// An item that is one of these sentences.
pub fn is_instant(body: &serde_json::Map<String, Value>) -> bool {
    body.get("instant").and_then(Value::as_bool) == Some(true)
}

/// The sentence for a message just sent while a turn goes on: asked, kept, the phones told. In the background.
pub fn answer(state: &AppState, agent: &AgentConfig, thread: &Thread, client_id: &str, words: &str, mode: Mode, common: bool) {
    let (state, agent, thread, client_id, words) =
        (state.clone(), agent.clone(), thread.clone(), client_id.to_string(), words.to_string());
    tokio::spawn(async move {
        let started = Instant::now();
        let config = state.config();
        let lang = crate::handback::language_of(&config, &agent);
        let asked = async {
            let model = crate::mail_scan::model_of(&config, &agent).ok()?;
            let call = Call { base_url: model["base_url"].clone(), key: model["key"].clone(), model: model["model"].clone() };
            let lines = if common { Vec::new() } else { last_lines(&state, &agent, &thread, &client_id).await };
            let shown = if agent.display_name.is_empty() { agent.name.clone() } else { agent.display_name.clone() };
            let system = system(&shown, crate::souls::language_name(&lang), mode);
            let body = body(&call, &system, &user_message(&lines, &words), &config.model_provider_order);
            ask(&state.http, &call, &body).await
        };
        let (said, by_model) = match tokio::time::timeout(TIMEOUT, asked).await.ok().flatten() {
            Some((said, input, output)) => {
                // the place's AI usage counts it with its key (the router meters it): its tokens said here
                tracing::info!(agent = %agent.name, tokens_in = input, tokens_out = output, "instant reply: the agent's model asked");
                (said, true)
            }
            None => (canned(&lang, mode).to_string(), false),
        };
        let ms = started.elapsed().as_millis() as u64;
        tracing::info!(agent = %agent.name, thread = %thread.id, by_model, ms, "instant reply said");
        let threads = threads_of(&state);
        if let Err(exc) = threads.keep_instant(&agent.name, &thread.id, &client_id, &said, by_model).await {
            tracing::warn!(agent = %agent.name, error = %format!("{exc:#}"), "instant reply not kept");
            return;
        }
        let _ = threads.bump(&thread, "instant").await;
    });
}

/// After words were steered into `run`: once Hermes has stored them (its next step), the phones are told (`taken`),
/// so they draw the check without waiting for the turn's end. Watched at each new step of the turn, never past its end.
pub fn watch_taken(state: &AppState, agent: &AgentConfig, thread: &Thread, run: &str, client_id: &str) {
    let (state, agent, thread, run, client_id) =
        (state.clone(), agent.clone(), thread.clone(), run.to_string(), client_id.to_string());
    tokio::spawn(async move {
        let Some(sub) = state.hub.subscription(&agent.name, &run) else { return };
        let mut seen = sub.tool_steps();
        let until = Instant::now() + TAKEN_WATCH;
        while Instant::now() < until && !sub.finished() {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let steps = sub.tool_steps();
            if steps == seen {
                continue;
            }
            seen = steps;
            let Ok(items) = crate::threads::items_of(&state, &agent, &thread).await else { continue };
            let stored = items
                .iter()
                .any(|i| !i.id.starts_with("s_") && i.body.get("client_id").and_then(Value::as_str) == Some(client_id.as_str()));
            if stored {
                let _ = threads_of(&state).bump(&thread, "taken").await;
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_answer_becomes_one_short_sentence_or_nothing() {
        assert_eq!(clean("Compris, plus de langue ni de dents, je l'intègre.").as_deref(),
                   Some("Compris, plus de langue ni de dents, je l'intègre."));
        assert_eq!(clean("« Noté, sans gluten. » Je continue.\n").as_deref(), Some("Noté, sans gluten."));
        assert_eq!(clean("<think>hmm</think>\nGot it, adding Lisbon").as_deref(), Some("Got it, adding Lisbon."));
        assert_eq!(clean("Noté. Tu veux autre chose ?").as_deref(), Some("Noté."));
        assert!(clean("Tu veux que je regarde aussi Porto ?").is_none());
        assert!(clean("").is_none() && clean("   \n").is_none() && clean("…").is_none());
        assert!(clean(&"très long ".repeat(30)).is_none());
    }

    #[test]
    fn a_canned_sentence_in_every_language_turning() {
        for lang in ["fr", "en", "de", "es", "nl"] {
            let (a, b) = (canned(lang, Mode::Steered), canned(lang, Mode::Steered));
            assert_ne!(a, b, "{lang}: never the same twice in a row");
            assert!(!canned(lang, Mode::Queued).contains('?'));
        }
        assert!(CANNED[0].2.iter().any(|s| s.contains("juste après")));
        // a language without its table: English
        assert!(CANNED[1].1.contains(&canned("pt", Mode::Steered)));
    }

    #[test]
    fn the_note_is_hidden_from_the_timeline_and_taken_off_again() {
        let steer = with_note("Plus de langue ni de dents", Mode::Steered);
        assert!(steer.starts_with("Plus de langue ni de dents\n\n<!--bip:answered ") && steer.ends_with("-->"));
        // a note every reader strips wherever it is: two steers Hermes joined into one row keep both words
        let joined = format!("{steer}\net pas trop loin");
        let shown = crate::live_brief::visible(&joined);
        assert!(shown.starts_with("Plus de langue ni de dents") && shown.ends_with("et pas trop loin") && !shown.contains("bip:"));
        assert_eq!(without_note(&joined), "Plus de langue ni de dents\net pas trop loin");
        assert_eq!(without_note("rien à ôter"), "rien à ôter");
        assert_eq!(without_note(&steer), "Plus de langue ni de dents");
        assert!(note(Mode::Queued).contains("right after"));
    }

    #[test]
    fn one_sentence_per_conversation_every_gap() {
        assert!(reserve("vie", "t_gap", 1000.0));
        assert!(!reserve("vie", "t_gap", 1005.0));
        assert!(reserve("vie", "t_other", 1005.0));
        assert!(reserve("vie", "t_gap", 1000.0 + GAP_SECONDS + 0.5));
    }

    #[test]
    fn the_model_reads_the_last_lines_and_the_new_message() {
        let lines = vec![("Person".to_string(), "Trouve un dentiste".to_string()), ("You".to_string(), "Je cherche à Lyon.".to_string())];
        let user = user_message(&lines, "  plutôt le samedi ");
        assert_eq!(user, "The conversation so far (latest last):\nPerson: Trouve un dentiste\nYou: Je cherche à Lyon.\n\n\
Their new message: plutôt le samedi");
        assert_eq!(user_message(&[], "x"), "Their new message: x");
        let peer = json!({"role": "peer", "from": "vie", "from_name": "Vie"});
        assert_eq!(author(peer.as_object().unwrap()), "Another agent (Vie)");
        assert_eq!(author(json!({"role": "agent"}).as_object().unwrap()), "You");
        let s = system("Vie", "French", Mode::Queued);
        assert!(s.contains("right after") && s.contains("Never ask a question") && s.contains("in French"));
    }

    #[test]
    fn a_thinking_model_is_asked_not_to_think_glm_keeps_its_floor() {
        let eu = Call { base_url: "https://api.eurouter.ai/api/v1".into(), key: "k".into(), model: "glm-5.3-flash".into() };
        let b = body(&eu, "s", "u", &[]);
        assert_eq!(b["max_tokens"], 400);
        assert!(b.get("chat_template_kwargs").is_none());
        let ds = Call { base_url: "https://api.eurouter.ai/api/v1".into(), key: "k".into(), model: "deepseek-v4.1-flash".into() };
        let b = body(&ds, "s", "u", &[]);
        assert_eq!(b["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(b["reasoning_effort"], "none");
        // a server of the person's: only the template's switch (an unknown field may be refused)
        let own = Call { base_url: "http://192.168.1.20:8888/v1".into(), key: String::new(), model: "GLM-5.3-Flash".into() };
        let b = body(&own, "s", "u", &[]);
        assert!(b.get("reasoning_effort").is_none() && b["chat_template_kwargs"]["enable_thinking"] == false);
    }

    #[test]
    fn a_first_message_line_is_one_to_three_sentences_or_nothing() {
        assert_eq!(clean_first("Je compare les trains Lyon–Paris de samedi matin, en évitant les correspondances.").as_deref(),
                   Some("Je compare les trains Lyon–Paris de samedi matin, en évitant les correspondances."));
        // the model's « nothing to say yet », a question, too short: none
        assert!(clean_first("-").is_none() && clean_first(" - ").is_none() && clean_first("").is_none());
        assert!(clean_first("Tu pars de quelle gare ?").is_none());
        assert!(clean_first("D'accord.").is_none());
        // three sentences at most, kept on one line, markdown dropped
        let long = "Ciao se dit en italien.\nC'est informel. On l'emploie entre amis. Pour être poli, dis plutôt « buongiorno ».";
        assert_eq!(clean_first(long).as_deref(), Some("Ciao se dit en italien. C'est informel. On l'emploie entre amis."));
        assert_eq!(clean_first("<think>hm</think>**Je regarde** tes mails de la semaine").as_deref(),
                   Some("Je regarde tes mails de la semaine."));
        assert!(clean_first(&"mot ".repeat(120)).is_none());
        // quotes of its own kept, quotes around the whole line dropped
        assert_eq!(clean_first("« Merci beaucoup » se dit « grazie mille » en italien.").as_deref(),
                   Some("« Merci beaucoup » se dit « grazie mille » en italien."));
        assert_eq!(clean_first("« Je regarde tes mails de la semaine. »").as_deref(), Some("Je regarde tes mails de la semaine."));
    }

    #[test]
    fn thanks_and_ok_get_no_line_at_all() {
        for words in ["merci", "Merci beaucoup !", "ok super", "d’accord", "👍", "Thanks!", "danke schön", "top merci", "salut"] {
            assert!(trivial(words, false), "{words}");
        }
        for words in ["Trouve-moi un train pour samedi", "merci de regarder mes mails", "ok, et pour Lyon ?"] {
            assert!(!trivial(words, false), "{words}");
        }
        // a photo with « ok »: something to look at
        assert!(!trivial("ok", true));
    }

    #[test]
    fn the_first_note_carries_the_line_and_is_hidden_like_the_others() {
        let words = with_first_note("Trouve-moi un train pour samedi", "Je regarde les trains de samedi -- matin.");
        assert!(words.starts_with("Trouve-moi un train pour samedi\n\n<!--bip:answered "));
        assert!(words.contains("« Je regarde les trains de samedi – matin. »") && words.ends_with("-->"));
        assert_eq!(words.matches("-->").count(), 1, "the line never closes the note early");
        assert_eq!(without_note(&words), "Trouve-moi un train pour samedi");
        assert_eq!(crate::live_brief::visible(&words), "Trouve-moi un train pour samedi");
        let s = first_system("Vie", "voyage", "French", "vendredi 20 novembre 2026, 17:05");
        assert!(s.starts_with("You are Vie (voyage),") && s.contains("in French") && s.contains("single -")
                && s.contains("Never ask a question") && s.contains("Money, health and legal questions are never quick")
                && s.ends_with("It is now vendredi 20 novembre 2026, 17:05."));
        assert!(first_system("Vie", " ", "French", "x").starts_with("You are Vie, a personal"));
        let ds = Call { base_url: "https://api.eurouter.ai/api/v1".into(), key: "k".into(), model: "deepseek-v4.1-flash".into() };
        assert_eq!(first_body(&ds, "s", "u", &[])["max_tokens"], FIRST_MAX_TOKENS);
        let glm = Call { base_url: "https://api.eurouter.ai/api/v1".into(), key: "k".into(), model: "glm-5.3-flash".into() };
        assert_eq!(first_body(&glm, "s", "u", &[])["max_tokens"], 400);
    }
}
