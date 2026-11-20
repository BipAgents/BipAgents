//! A Live call's turns go through the bridge (3/11): the apps no longer call Hermes for them. Each turn runs in the
//! call's own short session (made by `POST /v1/live/calls/start`, live_brief.rs) on Hermes' `chat/stream`, whose SSE
//! answer is passed on as it comes (one request, nothing waited for: the first words as early as before), with the
//! agent's quick-turn thinking (`quick_turn_options`: off, or the least its model allows) and the media and long tools
//! refused (`LIVE_CLOSED_TOOLS`, `live_turn_options`), `clarify` too: the call's rules say to ask out loud
//! (`with_call_rules`).
//!
//!     POST /v1/live/turns       {agent, live_session, text, attachments?}   → text/event-stream (Hermes' events)
//!     POST /v1/live/turns/stop  {agent, run_id}                             → {stopped}
//!
//! The first turn's text starts with the call's prelude (the time, the call's rules, its brief, the greeting said: a
//! hidden note the apps write, LiveLines.prelude); photos come inline and documents by path, as in a written message
//! (threads/send.rs). Approvals are answered through `POST /v1/approve`.
//!
//! A call's turn is braked here (20/11, Alex: « Laisse-moi vérifier sur le web, deux secondes », then minutes of
//! silence while the web search failed again and again, until Hermes' own guard at 50 searches and its English line in
//! the conversation). Its stream is no run the hub follows (runs.rs), so the bridge counts its tool calls as they pass
//! (loop_brake.rs, the `live` limits: a tool failing twice in a row, 6 web searches, 12 calls…; told to the agent at
//! about two thirds, a steer: answer with what you have): past one, Hermes' run is stopped and
//! the agent says the brake's plain line out loud (« Je n'arrive pas à chercher sur le web pour l'instant. Tu veux que
//! je réessaie plus tard ? »: an `assistant.delta`, then the turn's end with it as its text), Hermes' words after the
//! stop left out. Hermes' own halts (its guard's « I stopped retrying… », a reply cut for repeating itself:
//! hermes_halts.rs) and its « ⚠️ No reply: » (no_reply.rs) are said in the same plain words. The stream is read to its
//! end whoever listens: a call hung up while the agent works is still braked (the turn goes on, live_calls.rs waits).
//!
//! A turn the phone's voice decider sent to be filed, not answered (`noter`, 9/11), starts with a hidden
//! `<!--bip:note … -->`: whatever the app wrote in it (the 9/11 apps named a Carnet v1 tool, `notebook_note`, that is
//! gone), the bridge puts its own words there (`NOTE`), so the tools' names live here only.

use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::{AppState, RUN_ID, SESSION_ID};
use crate::threads::send::{input_of, Attachment};
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};

#[derive(Debug, Deserialize)]
pub struct Turn {
    pub agent: String,
    pub live_session: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

impl Validate for Turn {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "agent", Some(&self.agent), 200);
        check_max_len(errors, "text", Some(&self.text), crate::live_brief::WARM_PRELUDE_MAX + crate::msg_queue::TEXT_MAX);
        if !SESSION_ID.is_match(&self.live_session) {
            errors.push(field("live_session", "a session id"));
        }
        if self.text.trim().is_empty() && self.attachments.is_empty() {
            errors.push(field("text", "words or a file"));
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Stop {
    pub agent: String,
    pub run_id: String,
}

impl Validate for Stop {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if !RUN_ID.is_match(&self.run_id) {
            errors.push(field("run_id", "a run id"));
        }
    }
}

fn field(name: &str, msg: &str) -> FieldError {
    FieldError::new("value_error", vec![json!("body"), json!(name)], msg, Value::Null)
}

/// The tools a Live turn may not run (4/11: asked for help with a pronunciation in a call, an agent started making an
/// audio file while the person waited on the line): what makes media or long outputs (audio and podcasts, Hermes'
/// speech, images and videos, pages, files written, sub-agents, background and heavy work started, the browsers).
/// The call's rules (the apps' prelude, LiveLines `callRules`) say to speak it instead and note what is to be made with
/// `after_call`, made once the call ends (live_calls.rs). Quick ones stay: web search, the Carnet, calendar and
/// reminders, mail, maps, messages to other agents, a background task proposed (`propose_task`), `after_call`.
/// Hermes patch 0024 (`model_options.closed_tools`, a trailing `*` closes a prefix) refuses them in its dispatcher
/// (`tool_call` included) with « say it instead, offer it for after the call »; the request still lists every tool, so
/// a call's prompt is its conversations' one in the model server's cache. A Hermes without the patch ignores the field:
/// the rules still say it. `clarify` too (12/11: a question card asked in a call, which the call screens never show: the
/// call waited on it until Hermes gave up): the bridge's `ASK_ALOUD` in the call's rules says to ask out loud instead.
pub const LIVE_CLOSED_TOOLS: [&str; 18] = [
    "browser_*",
    "clarify",
    "computer_use",
    "delegate_task",
    "image_generate",
    "mcp__bipagents__create_audio",
    "mcp__bipagents__create_page",
    "mcp__bipagents__create_podcast",
    "mcp__bipagents__heavy_work",
    "mcp__bipagents__start_task",
    "mcp__bipagents_files__shared_files_write",
    "mcp__stealth__*",
    "patch",
    "text_to_speech",
    "video_generate",
    "write_file",
    "xai_video_edit",
    "xai_video_extend",
];

/// The `model_options` of a Live call's turns and of its warm-up (live_brief.rs `warm_up`): the agent's quick-turn
/// thinking, `LIVE_CLOSED_TOOLS` closed.
pub fn live_turn_options(config: &crate::config::Config, agent: &crate::config::AgentConfig) -> Value {
    let mut options = crate::agents_admin::quick_turn_options(config, agent);
    options["closed_tools"] = json!(LIVE_CLOSED_TOOLS);
    options
}

/// The call's rule for a question (12/11, `clarify` closed in a call): the bridge's words, put first in the call's
/// rules (`with_call_rules`), so a call's prompt stays the same at every turn and in the warm-up (the cache holds).
pub const ASK_ALOUD: &str = "No clarify tool in this call (the person sees no card): to ask something, ask it out loud in \
one short sentence and wait for the person's spoken answer. ";

const LIVE_OPEN: &str = "<!--bip:live\n";

/// `text` with `ASK_ALOUD` first in its call's rules (the prelude's `<!--bip:live` note: the first turn and the
/// warm-up); a text without them, or that has it already, unchanged.
pub fn with_call_rules(text: &str) -> std::borrow::Cow<'_, str> {
    match text.find(LIVE_OPEN) {
        Some(at) if !text.contains(ASK_ALOUD) => {
            let end = at + LIVE_OPEN.len();
            format!("{}{ASK_ALOUD}{}", &text[..end], &text[end..]).into()
        }
        _ => text.into(),
    }
}

/// What the agent reads for a turn to file (`with_note`): one notebook_write per page, no search first, the label
/// said again replacing the old fact, never a code; then two words.
pub const NOTE: &str = "<!--bip:note\nYour person said this to have it filed, not answered. File it now with notebook_write, \
one call per page (its name, a first name is enough, no search first; a place or subject is not a person: give its \
kind): « Label : value », the same label replaces the \
old fact, a correction too. Never a code or password: say in a few words you don't keep those. Then answer only « OK, \
noté » in your language.\n-->";

/// `text` with its `<!--bip:note … -->` (wherever it is: the first turn has the call's prelude before it) in the
/// bridge's words; other hidden notes (`<!--bip:notes-->`…) and a text without one unchanged.
pub fn with_note(text: &str) -> std::borrow::Cow<'_, str> {
    const OPEN: &str = "<!--bip:note";
    let mut from = 0;
    while let Some(at) = text[from..].find(OPEN).map(|i| i + from) {
        let after = &text[at + OPEN.len()..];
        if after.starts_with(char::is_whitespace) || after.starts_with("-->") {
            let end = after.find("-->").map_or(text.len(), |i| at + OPEN.len() + i + 3);
            return format!("{}{NOTE}{}", &text[..at], &text[end..]).into();
        }
        from = at + OPEN.len();
    }
    text.into()
}

const UNREACHABLE: &str = "your agent isn't answering right now: try again in a moment";

async fn turn(State(state): State<AppState>, Json(body): Json<Turn>) -> ApiResult<Response> {
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&body.agent))?;
    // `chat/stream` takes the parts of one message (photos), not a list of messages as `/v1/runs` does
    let input = match input_of(None, &with_call_rules(&with_note(&body.text)), &body.attachments) {
        Value::Array(mut messages) => messages.swap_remove(0)["content"].take(),
        words => words,
    };
    let request = json!({"input": input, "model_options": live_turn_options(&config, &agent)});
    let upstream = state.hermes.chat_stream(&agent, &body.live_session, request).await.map_err(|exc| match exc.status() {
        Some(status @ (400..=499)) => HttpError::new(status, "your agent refused this message"),
        _ => HttpError::new(502, UNREACHABLE),
    })?;
    let lang = crate::handback::language_of(&config, &agent);
    let name = if agent.display_name.trim().is_empty() { &agent.name } else { &agent.display_name };
    let brake = LiveBrake::new(&config.brake, &lang, name);
    let events = without_media(upstream.bytes_stream(), agent.clone(), brake, state.hermes.clone());
    // read to its end whoever listens (the brake still works after a hang-up); the phone gets what it can
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move {
        futures::pin_mut!(events);
        let mut listening = true;
        while let Some(item) = events.next().await {
            if listening && tx.send(item).await.is_err() {
                listening = false;
            }
        }
    });
    let mut response = Response::new(axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)));
    for (name, value) in [("content-type", "text/event-stream; charset=utf-8"), ("cache-control", "no-store"), ("x-accel-buffering", "no")] {
        response.headers_mut().insert(name, axum::http::HeaderValue::from_static(value));
    }
    Ok(response)
}

/// A call's turn's brake (see the module's doc): its tool calls counted against the `live` limits.
pub struct LiveBrake {
    enabled: bool,
    limits: crate::loop_brake::Limits,
    turn: crate::loop_brake::TurnCount,
    lang: String,
    /// The agent's shown name (its « No reply » line).
    name: String,
    run_id: Option<String>,
    /// The line said once the turn was stopped: Hermes' words after it are left out, its end says this.
    said: Option<String>,
}

impl LiveBrake {
    pub fn new(settings: &crate::loop_brake::Settings, lang: &str, name: &str) -> Self {
        LiveBrake {
            enabled: settings.enabled,
            limits: settings.live,
            turn: Default::default(),
            lang: lang.to_string(),
            name: name.to_string(),
            run_id: None,
            said: None,
        }
    }

    /// One of Hermes' events: the limit it passed, if any (the first only).
    pub fn event(&mut self, event: &crate::hermes::SSEEvent) -> Option<crate::loop_brake::Over> {
        if let Some(run) = event.data.get("run_id").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            self.run_id.get_or_insert_with(|| run.to_string());
        }
        if !self.enabled || self.said.is_some() {
            return None;
        }
        let tool = crate::loop_brake::event_tool(&event.data);
        if tool.is_empty() {
            return None;
        }
        match event.r#type.as_str() {
            "tool.started" => {
                let signature = crate::loop_brake::event_signature(&event.data);
                self.turn.note(&tool, signature.as_deref(), crate::runs::browser_tool(&tool), &self.limits)
            }
            // `chat/stream` tells a failed call apart; a run's events mark it in `tool.completed`
            "tool.failed" => self.turn.completed(&tool, true, &self.limits),
            "tool.completed" => {
                let failed = event.data.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                    || crate::loop_brake::completed_with_error(&event.data);
                self.turn.completed(&tool, failed, &self.limits)
            }
            _ => None,
        }
    }

    /// A limit the last call brought close: the note for the agent (a steer, out of the person's sight), once per kind.
    pub fn warning(&mut self) -> Option<String> {
        let warning = self.turn.take_warning()?;
        (self.enabled && self.said.is_none()).then(|| warning.text())
    }

    /// The turn stopped for `over`: the line to say (kept for its end).
    pub fn stopped(&mut self, over: &crate::loop_brake::Over) -> String {
        let line = crate::loop_brake::notice_text(over, &self.lang);
        self.said = Some(line.clone());
        line
    }

    /// An event as the phone gets it once braked (None: left out), else with Hermes' halts in plain words.
    pub fn shown(&self, event: crate::hermes::SSEEvent) -> Option<crate::hermes::SSEEvent> {
        let final_answer = crate::hermes::is_terminal(&event.r#type) || event.r#type == "assistant.completed";
        if let Some(line) = &self.said {
            if event.r#type.ends_with(".delta") {
                return None; // Hermes' words after the stop (« Operation interrupted… »)
            }
            if final_answer && event.data.get("content").is_some_and(Value::is_string) {
                let mut data = event.data;
                data.insert("content".into(), json!(line));
                let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
                return Some(crate::hermes::SSEEvent::new(event.r#type, data, raw));
            }
            return Some(event);
        }
        let mut event = crate::hermes_halts::scrub_event(event, &self.lang);
        if event.r#type == "assistant.completed" {
            let told = event.data.get("content").and_then(Value::as_str).and_then(|c| {
                crate::no_reply::in_plain_words(c, &self.name, &self.lang)
            });
            if let Some(told) = told {
                event.data.insert("content".into(), json!(told));
                let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(event.data.clone()));
                event = crate::hermes::SSEEvent::new(event.r#type, event.data, raw);
            }
        }
        Some(event)
    }
}

/// Hermes' events as the apps get them: a reply's `MEDIA:` lines out (media.rs), the files at the end as its
/// `attachments`, its end without Hermes' file-mutation verifier (hermes_footer.rs: never said aloud), the call's brake
/// (`LiveBrake`); its keepalives passed on.
fn without_media<S, E>(
    chunks: S,
    agent: crate::config::AgentConfig,
    mut brake: LiveBrake,
    hermes: crate::hermes::HermesClient,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async_stream::stream! {
        let lines = crate::hermes::lines_of(chunks);
        futures::pin_mut!(lines);
        let (mut parser, mut media) = (crate::hermes::SSEParser::new(), crate::media::MediaFilter::default());
        let mut done = false;
        while !done {
            let event = match lines.next().await {
                Some(Ok(line)) if line.starts_with(':') => {
                    yield Ok(bytes::Bytes::from(format!("{line}\n\n")));
                    continue;
                }
                Some(Ok(line)) => parser.feed_line(&line),
                Some(Err(exc)) => {
                    yield Err(std::io::Error::other(exc));
                    return;
                }
                None => {
                    done = true;
                    parser.finish()
                }
            };
            let Some(event) = event else { continue };
            let event = crate::hermes_footer::scrub_event(event, &agent.name);
            if let Some(over) = brake.event(&event) {
                let line = brake.stopped(&over);
                tracing::warn!(agent = %agent.name, reason = %crate::loop_brake::reason(&over),
                    "live call going round in circles: turn stopped, said out loud");
                // said now, after what the agent said so far; then Hermes' run stopped
                let mut said = media.flush_events("assistant.delta", event.data.get("run_id").unwrap_or(&Value::Null));
                let data = json!({"delta": format!("\n\n{line}"), "bip_brake": true});
                let raw = crate::py::Dumps::new().unicode().dumps(&data);
                said.push(crate::hermes::SSEEvent::new("assistant.delta", data.as_object().cloned().unwrap_or_default(), raw));
                for event in said {
                    yield Ok(frame(&event.r#type, &event.raw));
                }
                if let Some(run_id) = brake.run_id.clone() {
                    let (hermes, agent) = (hermes.clone(), agent.clone());
                    tokio::spawn(async move {
                        if let Err(err) = hermes.stop_run(&agent, &run_id).await {
                            tracing::warn!(agent = %agent.name, run_id = %run_id, error = %err, "live call's stop refused");
                        }
                    });
                }
            }
            // a limit getting close (the third web search…): the agent told to answer with what it has
            if let (Some(text), Some(run_id)) = (brake.warning(), brake.run_id.clone()) {
                let (hermes, agent) = (hermes.clone(), agent.clone());
                tokio::spawn(async move {
                    if let Err(err) = hermes.steer_run(&agent, &run_id, &text).await {
                        tracing::info!(agent = %agent.name, run_id = %run_id, error = %err, "live call's steer refused");
                    }
                });
            }
            let Some(event) = brake.shown(event) else { continue };
            let mut shown = Vec::new();
            if crate::hermes::is_terminal(&event.r#type) || event.r#type == "assistant.completed" {
                shown = media.flush_events("assistant.delta", event.data.get("run_id").unwrap_or(&Value::Null));
            }
            shown.extend(media.event(event));
            for event in shown {
                yield Ok(frame(&event.r#type, &event.raw));
            }
        }
    }
}

fn frame(kind: &str, data: &str) -> bytes::Bytes {
    bytes::Bytes::from(format!("event: {kind}\ndata: {}\n\n", data.replace('\n', "\ndata: ")))
}

async fn stop(State(state): State<AppState>, Json(body): Json<Stop>) -> ApiResult<Response> {
    let agent = crate::app::agent_or_404(&state.config(), Some(&body.agent))?;
    match state.hermes.stop_run(&agent, &body.run_id).await {
        Ok(()) => Ok(web::ok(json!({"stopped": true}))),
        Err(exc) if matches!(exc.status(), Some(404 | 409)) => Ok(web::ok(json!({"stopped": false}))),
        Err(_) => Err(HttpError::new(502, UNREACHABLE)),
    }
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/live/turns", post(turn)).route("/live/turns/stop", post(stop))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closes(name: &str) -> bool {
        LIVE_CLOSED_TOOLS.iter().any(|c| match c.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == *c,
        })
    }

    /// A Hermes answering every request with 200 `{}` (the stop), each request's first line sent on `seen`.
    async fn fake_hermes(seen: tokio::sync::mpsc::UnboundedSender<String>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut head = [0u8; 4096];
                    let n = socket.read(&mut head).await.unwrap_or(0);
                    let first = String::from_utf8_lossy(&head[..n]).lines().next().unwrap_or("").to_string();
                    let _ = seen.send(first);
                    let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn sse(events: &[(&str, Value)]) -> Vec<Result<bytes::Bytes, std::io::Error>> {
        events.iter().map(|(kind, data)| Ok(bytes::Bytes::from(format!("event: {kind}\ndata: {data}\n\n")))).collect()
    }

    /// What the phone gets of `events`: (kind, data) in order.
    async fn through(events: &[(&str, Value)], lang: &str, hermes_url: &str) -> Vec<(String, Value)> {
        let agent = crate::config::AgentConfig { name: "vie".into(), display_name: "Vie".into(), hermes_url: hermes_url.into(),
            ..Default::default() };
        let brake = LiveBrake::new(&crate::loop_brake::Settings::default(), lang, "Vie");
        let http = reqwest::Client::new();
        let out = without_media(futures::stream::iter(sse(events)), agent, brake, crate::hermes::HermesClient::new(http));
        let bytes: Vec<u8> = out.map(|c| c.unwrap().to_vec()).concat().await;
        let text = String::from_utf8(bytes).unwrap();
        text.split("\n\n")
            .filter(|f| f.starts_with("event:"))
            .map(|f| {
                let (kind, data) = f.split_once('\n').unwrap();
                let data = data.lines().map(|l| l.strip_prefix("data: ").unwrap_or(l)).collect::<Vec<_>>().join("\n");
                (kind.trim_start_matches("event: ").to_string(), serde_json::from_str(&data).unwrap_or(Value::Null))
            })
            .collect()
    }

    /// 20/11, Alex's call with Vie: « Laisse-moi vérifier sur le web », then the web search failing again and again.
    /// The call now stops at its second failure in a row, Hermes' run stopped, the plain line said out loud; Hermes'
    /// words after the stop never reach the phone and the turn's end says the line.
    #[tokio::test]
    async fn a_web_search_failing_in_a_call_is_stopped_and_said_out_loud() {
        let (tx, mut seen) = tokio::sync::mpsc::unbounded_channel();
        let url = fake_hermes(tx).await;
        let failed = json!({"tool_name": "web_search", "preview": ""});
        let events = [
            ("assistant.delta", json!({"delta": "Laisse-moi vérifier sur le web, deux secondes.", "run_id": "run_1"})),
            ("tool.started", json!({"tool_name": "web_search", "args": {"query": "jeu DJ PS1"}, "run_id": "run_1"})),
            ("tool.failed", failed.clone()),
            ("tool.started", json!({"tool_name": "web_search", "args": {"query": "PS1 DJ game"}})),
            ("tool.failed", failed.clone()),
            ("tool.started", json!({"tool_name": "web_search", "args": {"query": "beatmania"}})),
            ("tool.failed", failed),
            ("assistant.delta", json!({"delta": "\n\nOperation interrupted: waiting for model response (3.1s elapsed)."})),
            ("assistant.completed", json!({"content": "Operation interrupted: waiting for model response (3.1s elapsed).",
                                           "run_id": "run_1", "interrupted": true})),
            ("done", json!({"run_id": "run_1"})),
        ];
        let got = through(&events, "fr", &url).await;
        let line = "Je n'arrive pas à chercher sur le web pour l'instant. Tu veux que je réessaie plus tard ?";
        let said: String = got.iter().filter(|(k, _)| k == "assistant.delta").map(|(_, d)| d["delta"].as_str().unwrap().to_string()).collect();
        assert_eq!(said, format!("Laisse-moi vérifier sur le web, deux secondes.\n\n{line}"));
        let end = got.iter().find(|(k, _)| k == "assistant.completed").unwrap();
        assert_eq!(end.1["content"], json!(line));
        assert!(got.iter().any(|(k, _)| k == "done"));
        // the third search is never started by the agent's words: the stop went at the second failure
        let stop = tokio::time::timeout(std::time::Duration::from_secs(5), seen.recv()).await.unwrap().unwrap();
        assert_eq!(stop, "POST /v1/runs/run_1/stop HTTP/1.1");
    }

    /// 20/11, the real place: every search answered, new words each time, 50 of them. A call stops past 6.
    #[tokio::test]
    async fn a_call_searching_again_and_again_is_stopped_even_when_searches_answer() {
        let (tx, mut seen) = tokio::sync::mpsc::unbounded_channel();
        let url = fake_hermes(tx).await;
        let mut events = vec![("assistant.delta", json!({"delta": "Laisse-moi vérifier.", "run_id": "run_2"}))];
        for i in 0..8 {
            events.push(("tool.started", json!({"tool_name": "web_search", "args": {"query": format!("jeu DJ PS1 {i}")}})));
            events.push(("tool.completed", json!({"tool_name": "web_search", "preview": ""})));
        }
        events.push(("assistant.completed", json!({"content": "Operation interrupted: waiting for model response (1.0s elapsed)."})));
        let got = through(&events, "fr", &url).await;
        let end = got.iter().find(|(k, _)| k == "assistant.completed").unwrap();
        assert!(end.1["content"].as_str().unwrap().starts_with("J'ai fait beaucoup de recherches sur le web"));
        let mut asked = Vec::new();
        while let Ok(Some(line)) = tokio::time::timeout(std::time::Duration::from_millis(500), seen.recv()).await {
            asked.push(line);
        }
        // told at the fourth search (a steer: answer with what you have), stopped at the seventh
        assert!(asked.contains(&"POST /v1/runs/run_2/steer HTTP/1.1".to_string()), "{asked:?}");
        assert!(asked.contains(&"POST /v1/runs/run_2/stop HTTP/1.1".to_string()), "{asked:?}");
    }

    #[tokio::test]
    async fn hermes_own_halts_are_said_in_plain_words_in_a_call() {
        let guard = "I stopped retrying because I kept running web_search 50 times without making progress. The last \
result above shows what blocked it. Tell me how you'd like to proceed, or send `continue` and I'll try a different approach.";
        let events = [
            ("assistant.delta", json!({"delta": format!("\n\n{guard}")})),
            ("assistant.completed", json!({"content": guard})),
        ];
        // no stop asked: an address nobody answers
        let got = through(&events, "en", "http://127.0.0.1:9").await;
        assert_eq!(got[0].1["delta"], json!("\n\nI can't search the web right now. Do you want me to try again later?"));
        assert_eq!(got[1].1["content"], json!("I can't search the web right now. Do you want me to try again later?"));
        let no_reply = [("assistant.completed", json!({"content": "⚠️ No reply: GLM-5.3-Flash-EXL3 didn't produce a reply this time."}))];
        let got = through(&no_reply, "fr", "http://127.0.0.1:9").await;
        assert_eq!(got[0].1["content"], json!("Vie n'a pas réussi à répondre cette fois. Réessaie, ou écris « continue »."));
        // a turn that searches once and answers: as it is
        let fine = [
            ("tool.started", json!({"tool_name": "web_search", "args": {"query": "jeu DJ PS1"}})),
            ("tool.completed", json!({"tool_name": "web_search", "preview": ""})),
            ("assistant.completed", json!({"content": "C'est Beatmania !"})),
        ];
        let got = through(&fine, "fr", "http://127.0.0.1:9").await;
        assert_eq!(got.last().unwrap().1["content"], json!("C'est Beatmania !"));
        assert!(!got.iter().any(|(_, d)| d.get("bip_brake").is_some()));
    }

    #[test]
    fn a_note_to_file_gets_the_bridges_words() {
        // what the 9/11 apps send (VoiceDecider noteTurn), a v1 tool's name in it
        let old = "<!--bip:note\nYour person said this to have it filed, not answered: keep it in the Carnet (notebook_note), then \
answer only « OK, noté » in your language, nothing more.\n-->\npour info le plombier vient jeudi";
        assert_eq!(with_note(old), format!("{NOTE}\npour info le plombier vient jeudi"));
        // after the call's prelude, in the first turn
        let first = "<!--bip:now x-->\n<!--bip:live\nrules <!--bip:end-->\n\n<!--bip:note\nkeep it\n-->\nnote que Léa chausse du 36";
        assert_eq!(with_note(first), format!("<!--bip:now x-->\n<!--bip:live\nrules <!--bip:end-->\n\n{NOTE}\nnote que Léa chausse du 36"));
        for same in ["bonjour", "<!--bip:notes-->[Request]", "<!--bip:notebook x-->", "je note que <!--bip:noted"] {
            assert_eq!(with_note(same), same);
        }
        assert!(NOTE.contains(crate::notebook::tools::WRITE) && !NOTE.contains("notebook_note"));
        assert!(NOTE.ends_with("-->") && NOTE.matches("-->").count() == 1);
        // a note never closed: the rest is the note's
        assert_eq!(with_note("<!--bip:note\nkeep it"), NOTE);
    }

    #[test]
    fn a_calls_rules_say_to_ask_out_loud() {
        let prelude = "<!--bip:now x-->\n<!--bip:live\nbrief\n\nrules <!--bip:end-->";
        let ruled = with_call_rules(prelude);
        assert_eq!(ruled, format!("<!--bip:now x-->\n<!--bip:live\n{ASK_ALOUD}brief\n\nrules <!--bip:end-->"));
        // once only, and the next turns (no prelude) unchanged
        assert_eq!(with_call_rules(&ruled), ruled);
        for same in ["bonjour", "<!--bip:lively-->"] {
            assert_eq!(with_call_rules(same), same);
        }
        assert!(ASK_ALOUD.contains("clarify") && !ASK_ALOUD.contains("-->"));
    }

    #[test]
    fn a_call_closes_the_media_and_long_tools_and_keeps_the_quick_ones() {
        let bridge = |name: &str| format!("{}{name}", crate::tool_diet::BRIDGE_PREFIX);
        for made in [crate::agent_tools::CREATE_AUDIO, crate::agent_tools::CREATE_PODCAST, crate::agent_tools::CREATE_PAGE,
            crate::jobs::START_TASK, crate::heavy_work::HEAVY_WORK]
        {
            assert!(closes(&bridge(made)), "{made}");
        }
        for native in ["text_to_speech", "image_generate", "write_file", "browser_navigate", "mcp__stealth__navigate"] {
            assert!(closes(native), "{native}");
        }
        for quick in [crate::agent_tools::PROPOSE_TASK, crate::agent_tools::KNOWLEDGE_SEARCH, crate::maps::TOOL_NAME,
            crate::routes::TOOL_NAME, crate::weather::TOOL_NAME, crate::agent_messages::TOOL_NAME,
            crate::notebook::tools::SEARCH, crate::notebook::tools::WRITE, "calendar_list", "reminders_add", "mail_search",
            "mail_read", crate::agent_tools::LIST_PAGES, crate::agent_tools::READ_PAGE, crate::live_calls::AFTER_CALL]
        {
            assert!(!closes(&bridge(quick)), "{quick}");
        }
        for quick in ["web_search", "web_extract", "memory", "read_file", "terminal", "tool_search"] {
            assert!(!closes(quick), "{quick}");
        }
        // a question card nobody sees in a call (12/11): closed, asked out loud
        assert!(closes("clarify"));
        // Hermes 0024 takes at most 128 names without spaces; one fixed list, in order
        assert!(LIVE_CLOSED_TOOLS.len() <= 128 && LIVE_CLOSED_TOOLS.iter().all(|n| !n.contains(char::is_whitespace)));
        assert!(LIVE_CLOSED_TOOLS.windows(2).all(|w| w[0] < w[1]));
    }
}
