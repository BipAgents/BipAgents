//! How the bridge's own MCP servers answer a tool call, so that Hermes' circuit breaker never pauses them
//! (bridge-tools-no-breaker, 14/11).
//!
//! Hermes (v0.23.1, tools/mcp_tool_handlers.py `_render_call_tool_result`, `_record_call_outcome`,
//! `_check_circuit_breaker`; its #10447) hands an `isError` answer to the model as `{"error": …}` and counts it as a
//! strike against the whole server: three strikes in a row and every tool of that server is refused for ~60 s
//! (« MCP server 'bipagents' rejected the last 3 calls… Paused for ~58s »), re-armed by the next failure; any other
//! answer resets the count. Its count is per server and per agent's profile. On the sign-up bench of 14/11 (DeepSeek,
//! 2 runs of 9) the Carnet refused one note three times in a row (« Not saved: ID numbers are never kept ») and all
//! the bridge's tools were paused for a minute; fix-signup-hold had met the same with the browser (browser_mcp.rs).
//!
//! So, on the wire (`settle`), for every server the bridge answers (`bipagents`, `bipagents_files`, `bipagents_vault`,
//! `bipagents_pay`, `bipagents_checkout`, `bipagents_link`, `bipagents_connectors`, each connector's entry):
//! - a refusal, a policy decision, a limit, « not found », a closed tool, a held browser, a refused approval (every
//!   `isError` the tools write: the tools keep that flag inside the bridge, where the brake, the files made and the
//!   pushes read it) goes out as a plain answer, `isError` false, its text opening with « Not done: » unless it already
//!   says so (« Not saved: », « Not started: »…): what happened and what to do instead;
//! - a real failure of the bridge itself (an error its handler could not answer: a store that cannot be read or
//!   written, a task that died; `internal_failure`) keeps `isError`, says not to retry, and never three times in a
//!   row for one agent: the third goes out plain (`STREAK`), so the bridge alone never trips the breaker.
//!
//! None of these tools declares an `outputSchema` (a test checks it): a text answer is valid for each. The browser's
//! proxy (browser_mcp.rs `soften`) answers its own way.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

use crate::tools_knowledge::tool_text;
use crate::Dict;

/// The mark of a real failure (`internal_failure`), taken off before the answer goes out.
pub const FAILURE_MARK: &str = "_bipagents_failure";

/// `isError` answers in a row a server may send one agent: Hermes pauses the server at 3.
pub const STREAK: u32 = 2;

/// What opens a refusal on the wire.
pub const NOT_DONE: &str = "Not done: ";

/// What a real failure says (`internal_failure`).
pub const FAILED: &str = "Not done: the bridge failed on its side (an internal error, logged for its maintainers). \
Don't retry this call in this turn: tell your person simply that it didn't work this time, and go on without it.";

/// Added when the bridge failed again just after (sent plain: `STREAK`).
pub const FAILED_AGAIN: &str = " It failed again: stop calling these tools in this turn.";

/// The `isError` answers in a row each server sent each agent (server, agent).
static STREAKS: LazyLock<Mutex<HashMap<(String, String), u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A real failure of the bridge itself (not a refusal): `isError` kept on the wire, with `FAILED`. `what` goes to the
/// log only (paths and the person's words stay out of the agent's view).
pub fn internal_failure(server: &str, tool: &str, what: &str) -> Dict {
    tracing::error!(server = server, tool = tool, error = %what, "bridge tool failed: answered « don't retry »");
    let mut out = tool_text(FAILED, true);
    out.insert(FAILURE_MARK.into(), Value::Bool(true));
    out
}

/// Where Hermes is to put the id of the call a `tools/call` request is (the request's `params._meta`): a small Hermes
/// patch to come; v0.23.1 sends none, and the hub then pairs a request with the earliest call of its tool.
pub const CALL_ID_META: &str = "hermes/tool_call_id";

/// The id of the call a `tools/call` request is (`params`), when Hermes gives it (`CALL_ID_META`): the hub pairs the
/// request with exactly that call (`RunHub::claim_call_with`).
pub fn call_id_of(params: &serde_json::Map<String, Value>) -> Option<String> {
    params.get("_meta")?.get(CALL_ID_META)?.as_str().map(str::trim).filter(|id| !id.is_empty() && id.len() <= 200).map(str::to_string)
}

/// The mark of a refusal the person lifts in the app (`lifted_in_the_app`), taken off before the answer goes out.
pub const LIFTED_MARK: &str = "_bipagents_lifted";

/// A refusal the person lifts in the app (a share, a switch, a right, a mailbox to connect), marked by its tool: the
/// same call may well go once she has done it, so the brake never counts it (loop_brake.rs `record_refusal`; review 2,
/// 14/11: two « not shared » refusals, the person turned « Rappels » on, and the third identical call was stopped).
pub fn lifted_in_the_app(mut result: Dict) -> Dict {
    result.insert(LIFTED_MARK.into(), Value::Bool(true));
    result
}

/// The mark of a final refusal (`final_refusal`), taken off before the answer goes out.
pub const FINAL_MARK: &str = "_bipagents_final";

/// A refusal that asking again never changes (a hidden or secret-looking file, give_file.rs `never_given`), marked by
/// its tool: the very same call again in the turn is answered « already refused » at once (loop_brake.rs
/// `Turns::check`), not passed to the tool again.
pub fn final_refusal(mut result: Dict) -> Dict {
    result.insert(FINAL_MARK.into(), Value::Bool(true));
    result
}

/// The mark of a refusal that is no new failure (`uncounted`), taken off before the answer goes out.
pub const UNCOUNTED_MARK: &str = "_bipagents_uncounted";

/// A refusal the turn's brake does not count as the tool failing again (loop_brake.rs `same_failure`): the very same
/// call asked again after a final refusal (`Turns::check`). Its step is still relayed not done (`settle_for`).
pub fn uncounted(mut result: Dict) -> Dict {
    result.insert(UNCOUNTED_MARK.into(), Value::Bool(true));
    result
}

/// A failure the bridge relays as one (a connector's own `isError` answer): `isError` kept, under `STREAK`.
pub fn mark_failure(result: &mut Dict) {
    if result.get("isError").is_some_and(crate::py::truthy) {
        result.insert(FAILURE_MARK.into(), Value::Bool(true));
    }
}

/// What a failure with no text of its own says when it goes out plain (`settle`).
pub const FAILED_SILENTLY: &str = "this call failed (its answer said nothing more).";

/// A tool call's answer as it goes to Hermes, from `server` to `agent` (module doc): a refusal plain, a real failure
/// with `isError` at most `STREAK` times in a row. True when it goes out plain saying nothing was done: Hermes then
/// reports the call as done (`tool.completed` `error` false), so the caller tells the hub (`settle_for`).
pub fn settle(server: &str, agent: &str, result: &mut Dict) -> bool {
    let failure = result.remove(FAILURE_MARK).is_some_and(|v| v == Value::Bool(true));
    result.remove(LIFTED_MARK);
    result.remove(FINAL_MARK);
    result.remove(UNCOUNTED_MARK);
    let error = result.get("isError").is_some_and(crate::py::truthy);
    let key = (server.to_string(), agent.to_string());
    let mut streaks = STREAKS.lock().unwrap_or_else(|e| e.into_inner());
    if !error {
        streaks.remove(&key);
        return false;
    }
    if failure {
        let streak = streaks.entry(key.clone()).or_insert(0);
        if *streak < STREAK {
            *streak += 1;
            result.insert("isError".into(), Value::Bool(true));
            return false;
        }
        streaks.remove(&key);
        drop(streaks);
        tracing::warn!(server = server, agent = agent, "a third failure in a row: sent plain (Hermes' breaker)");
        result.insert("isError".into(), Value::Bool(false));
        // a failure without a text (a connector's empty content, an image, only structuredContent): one that says so
        if first_text(result).is_none() {
            let content = result.entry("content").or_insert_with(|| Value::Array(Vec::new()));
            if !content.is_array() {
                *content = Value::Array(Vec::new());
            }
            if let Value::Array(blocks) = content {
                blocks.insert(0, serde_json::json!({"type": "text", "text": FAILED_SILENTLY}));
            }
        }
        say_not_done(result);
        add_text(result, FAILED_AGAIN);
        return true;
    }
    streaks.remove(&key);
    drop(streaks);
    result.insert("isError".into(), Value::Bool(false));
    say_not_done(result);
    true
}

/// [`settle`], and when the answer goes out plain saying nothing was done, the hub told which call it was (`claim`,
/// `RunHub::claim_call` when the request came): it relays that call's `tool.completed` as failed (the apps' step,
/// the safety net, the brake).
pub fn settle_for(hub: &crate::runs::RunHub, claim: Option<&crate::runs::CallClaim>, server: &str, agent: &str, result: &mut Dict) {
    let counted = !result.contains_key(UNCOUNTED_MARK);
    if settle(server, agent, result) {
        if let Some(claim) = claim {
            // what it said goes with it: the rescue turn of a turn stopped for it gets it (runs.rs `loop_rescue`)
            hub.refuse_saying(claim, first_text(result).unwrap_or(""), counted);
        }
    }
}

/// [`settle_for`] on a JSON-RPC result value (an object answer; anything else as it is).
pub fn settle_value_for(
    hub: &crate::runs::RunHub,
    claim: Option<&crate::runs::CallClaim>,
    server: &str,
    agent: &str,
    result: Value,
) -> Value {
    match result {
        Value::Object(mut map) => {
            settle_for(hub, claim, server, agent, &mut map);
            Value::Object(map)
        }
        other => other,
    }
}

/// What Hermes kept of a call (`{"result": …}` for a plain answer, `_render_call_tool_result`): one of the bridge's
/// tools (or a connector's) that said nothing was done. For what the hub can't have been told: the history
/// (threads/timeline.rs), a run event's preview.
pub fn refused_in_hermes(tool: &str, rendered: &str) -> bool {
    let ours = tool.contains("bipagents") || tool.contains(crate::connectors::ENTRY_PREFIX);
    ours && rendered_not_done(rendered)
}

/// A result as Hermes renders it whose text says nothing was done (`{"result": "Not …"}`); a cut one too.
pub fn rendered_not_done(rendered: &str) -> bool {
    let rendered = rendered.trim_start();
    match serde_json::from_str::<Value>(rendered) {
        Ok(value) => value.get("result").and_then(Value::as_str).is_some_and(|t| says_not_done(t.trim_start())),
        Err(_) => rendered.starts_with("{\"result\": \"Not ") || rendered.starts_with("{\"result\":\"Not "),
    }
}

fn first_text(result: &Dict) -> Option<&str> {
    result.get("content")?.as_array()?.iter().find_map(|b| b.get("text").and_then(Value::as_str))
}


/// The first text of a refusal opened with `NOT_DONE`, unless it already says nothing was done.
fn say_not_done(result: &mut Dict) {
    let Some(Value::Array(content)) = result.get_mut("content") else {
        result.insert("content".into(), serde_json::json!([{"type": "text", "text": NOT_DONE.trim_end()}]));
        return;
    };
    let Some(text) = content.iter_mut().find_map(|b| b.get_mut("text").filter(|t| t.is_string())) else {
        content.insert(0, serde_json::json!({"type": "text", "text": NOT_DONE.trim_end()}));
        return;
    };
    let said = text.as_str().unwrap_or("").trim_start().to_string();
    if !says_not_done(&said) {
        *text = Value::String(format!("{NOT_DONE}{said}"));
    }
}

/// A text that already says nothing was done (« Not done: », « Not saved: », « Not started: »…). « Nothing… » is no
/// sign: « Nothing found. » is an answer.
pub fn says_not_done(text: &str) -> bool {
    text.starts_with("Not ")
}

/// `more` added to the first text of `result`.
pub fn add_text(result: &mut Dict, more: &str) {
    if let Some(Value::Array(content)) = result.get_mut("content") {
        if let Some(text) = content.iter_mut().find_map(|b| b.get_mut("text").filter(|t| t.is_string())) {
            *text = Value::String(format!("{}{more}", text.as_str().unwrap_or("")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(result: &Dict) -> &str {
        result["content"][0]["text"].as_str().unwrap()
    }

    #[test]
    fn a_refusal_goes_out_plain_and_says_nothing_was_done() {
        let mut refused = tool_text("unknown page id: nope (see list_pages)", true);
        settle("bipagents", "a-refusal", &mut refused);
        assert_eq!(refused["isError"], json!(false));
        assert_eq!(text(&refused), "Not done: unknown page id: nope (see list_pages)");
        // already said: left as it is
        let mut id = tool_text("Not saved: ID numbers are never kept, and fact 2 holds one.", true);
        settle("bipagents", "a-refusal", &mut id);
        assert_eq!((id["isError"].clone(), text(&id)), (json!(false), "Not saved: ID numbers are never kept, and fact 2 holds one."));
        // an answer that went well: untouched
        let mut done = tool_text("Saved: Léa Martin › tastes (1 fact).", false);
        settle("bipagents", "a-refusal", &mut done);
        assert_eq!(done, tool_text("Saved: Léa Martin › tastes (1 fact).", false));
    }

    #[test]
    fn hermes_breaker_never_gets_three_strikes_in_a_row_from_the_bridge() {
        // Hermes' rule: an `isError` answer is a strike, anything else resets; 3 in a row pause the server
        let mut strikes = 0;
        let mut paused = false;
        let mut send = |mut result: Dict| {
            settle("bipagents", "a-streak", &mut result);
            assert!(!result.contains_key(FAILURE_MARK));
            strikes = if result["isError"] == json!(true) { strikes + 1 } else { 0 };
            paused |= strikes >= 3;
            result
        };
        // refusals, any number of them
        for _ in 0..5 {
            assert_eq!(send(tool_text("Not saved: ID numbers are never kept.", true))["isError"], json!(false));
        }
        // real failures: two with isError, the third plain, and so on
        let failed: Vec<Dict> = (0..7).map(|_| send(internal_failure("bipagents", "notebook_write", "disk full"))).collect();
        let flags: Vec<Value> = failed.iter().map(|r| r["isError"].clone()).collect();
        assert_eq!(flags, [true, true, false, true, true, false, true].map(Value::Bool));
        assert!(text(&failed[0]).starts_with(FAILED) && !text(&failed[0]).ends_with(FAILED_AGAIN));
        assert!(text(&failed[2]).starts_with(FAILED) && text(&failed[2]).ends_with(FAILED_AGAIN), "{:?}", failed[2]);
        // a refusal between two failures resets Hermes' count, and the bridge's
        send(tool_text("Not saved: ID numbers are never kept.", true));
        assert_eq!(send(internal_failure("bipagents", "x", "y"))["isError"], json!(true));
        assert!(!paused);
        // another agent, another server: a count of their own
        let mut other = internal_failure("bipagents", "x", "y");
        settle("bipagents", "another-agent", &mut other);
        assert_eq!(other["isError"], json!(true));
        let mut vault = internal_failure("bipagents_vault", "x", "y");
        settle("bipagents_vault", "a-streak", &mut vault);
        assert_eq!(vault["isError"], json!(true));
    }

    /// A tool with an `outputSchema` needs a `structuredContent` fitting it in every answer, refusals included, or
    /// Hermes' MCP client refuses the answer (and counts it): the browser's proxy does it (browser_mcp.rs `fitting`);
    /// none of the bridge's own tools declares one, and a connector's entry lists its program's tools without theirs
    /// (connectors.rs `served_tools`).
    #[test]
    fn no_tool_of_the_bridge_declares_an_output_schema() {
        fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !["browser_mcp.rs", "tool_answers.rs", "connectors.rs"].contains(&path.file_name().unwrap().to_str().unwrap())
                    && std::fs::read_to_string(&path).unwrap().contains("outputSchema")
                {
                    found.push(path.display().to_string());
                }
            }
        }
        let mut found = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.is_empty(), "an outputSchema: its refusals need a fitting structuredContent: {found:?}");
    }

    /// Review 1 (14/11): a connector program's own failure may carry no text (an empty content, an image, only
    /// structuredContent: valid MCP); the third in a row, sent plain, must still say it failed.
    #[test]
    fn a_third_failure_in_a_row_without_a_text_still_says_it_failed() {
        for (n, failed) in [
            json!({"content": [], "isError": true}),
            json!({"isError": true, "structuredContent": {"error": "quota"}}),
            json!({"content": [{"type": "image", "data": "AAAA", "mimeType": "image/png"}], "isError": true}),
        ]
        .into_iter()
        .enumerate()
        {
            let server = format!("bipconn_textless{n}");
            let mut sent = Vec::new();
            for _ in 0..3 {
                let mut result = failed.as_object().cloned().unwrap();
                mark_failure(&mut result);
                settle(&server, "a-textless", &mut result);
                sent.push(result);
            }
            let third = &sent[2];
            let said = third["content"].as_array().and_then(|c| c.iter().find_map(|b| b["text"].as_str())).unwrap_or("");
            assert_eq!(third["isError"], json!(false), "{third:?}");
            assert!(said.starts_with(NOT_DONE) && said.contains("failed"), "the third failure says nothing: {third:?}");
        }
    }

    #[test]
    fn a_failure_says_dont_retry_and_keeps_its_details_in_the_log() {
        let failed = internal_failure("bipagents", "notebook_write", "Permission denied: /srv/carnet/Personnes/Léa Martin.md");
        assert!(text(&failed).contains("Don't retry") && !text(&failed).contains("Léa"));
        assert_eq!(failed["isError"], json!(true));
    }
}
