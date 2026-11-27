//! The brake on a turn going round in circles (2026-10-27). In the token lab, GLM-5.3 Flash answered « open this
//! Wikipedia page in your browser » with 255 `calendar_add` and 7 `calendar_list` in one turn, for 15 minutes, until
//! the bench's timeout: 267 model calls, 10.5 M tokens. Its words kept saying it would stop. Nothing stopped it: Hermes
//! has no iteration cap by default (`agent.max_turns` unset is unlimited), and the long-work steer (runs.rs `handoff`)
//! is only advice.
//!
//! Two parts, independent of the model:
//!
//! - the run hub (runs.rs) counts the tool calls of every run it follows (`tool.started`). A turn that calls one tool
//!   more than `same_tool` times, makes more than `tool_calls` calls in all, or makes the same call (tool and
//!   arguments) `same_call` times is stopped (Hermes' `POST /v1/runs/{id}/stop`), and the conversation gets a short
//!   plain line (`bridge.notice`, code `loop_stopped`, in the agent's language): « Je me suis arrêté : je tournais en
//!   rond sur l'agenda. Dis-moi si tu veux que je réessaie. » The work tools a turn legitimately repeats (the terminal,
//!   code, files, web search, the visible browser) are not held to `same_tool`, only to the total. The visible
//!   browser has its own, lower limits (`browser_calls` calls, `page_reads` page reads or screenshots in a turn),
//!   well under Hermes' own cap of 150 (tool_diet.rs), so a page the agent can't read ends on « Je me suis arrêté : je
//!   n'arrivais pas à lire cette page… », not on a silent failure; a browsing turn's total is `tool_calls` +
//!   `browser_calls`. Calls a sub-agent makes while it works don't count (as for the steer);
//! - the bridge's own tools are counted at its MCP endpoint, per turn: the run the hub follows for the agent then
//!   (the one that just called this tool when several go on), keyed by its run id, never per agent across
//!   conversations. Its write tools (calendar, reminders, contacts, Carnet, mail drafts, pages, audio, family news)
//!   are refused past `writes` writes in that turn, and the very same write (same tool, same arguments) twice in it is
//!   refused at once with a plain answer the model can act on (« already done »). A write that only proposes a card
//!   the person approves (`PROPOSING_TOOLS`: mail_bulk) is held to neither `writes` nor `same_tool`. A turn no run of the hub carries
//!   (a routine, Hermes' cron) is the agent's calls until it has been quiet `idle_seconds`: there, the cap applies,
//!   one bridge tool more than `same_tool` times or more than `routine_tool_calls` bridge calls in all are refused
//!   with « stop and answer with what you have » (the bridge can't stop a cron turn: Hermes has no stop for it).
//!   A routine is work the person scheduled, like a background task: it is held to the task's limits (`task_*`), not
//!   a conversation's (2026-10-31, Alex's 23:00 « corrige tout le Carnet » stopped at 10 changes), except that one
//!   tool that writes nothing still goes to a conversation's `same_tool` (searching again and again is a loop whatever
//!   the job), and its calls in all go to `routine_tool_calls` (120, a task's). Past its writes, a
//!   routine or a task is told to end with a plain report: why it stopped (a cap on changes made at once, so that a
//!   mistake can't spread everywhere), what it changed, what is left, what happens next; never « the bridge » or a turn.
//!
//! Every run the bridge starts or relays is followed by the hub, whoever looks at it (2026-10-27 evening, the token
//! lab's bench): the app's runs through the bridge's `GET /v1/runs/{id}/events`, but also peers' messages (peers.rs),
//! Live, rooms, relatives, background tasks. Before, relayed runs were invisible to the hub: no stop at all, and their
//! writes counted per agent across conversations (a reminder refused at the first write).
//!
//! Background tasks (jobs.rs, « Tâches de fond »: work with a budget, many calls by design) have their own, higher
//! limits (`task_*`); they are stopped the same way, the task then ends as stopped. The mail scan (mail_scan.rs) makes
//! no tool calls at all (the agent's model reads mails with no tools), so nothing here concerns it.
//!
//! `[brake]` in bridge.toml (every value optional):
//!
//!     enabled = true
//!     same_tool = 10          tool_calls = 60        same_call = 3        same_lookup = 5      writes = 10
//!     browser_calls = 40      page_reads = 15        discovery_same = 5   discovery_calls = 8  same_failure = 3
//!     task_same_tool = 40     task_tool_calls = 120  task_same_call = 5   task_same_lookup = 8 task_writes = 40
//!     task_browser_calls = 80 task_page_reads = 30   task_discovery_same = 5 task_discovery_calls = 15 task_same_failure = 5
//!     web_searches = 20       task_web_searches = 40 live_web_searches = 6
//!     live_same_tool = 5      live_tool_calls = 12   live_same_call = 2   live_same_lookup = 3 live_writes = 5
//!     live_browser_calls = 5  live_page_reads = 3    live_discovery_same = 2 live_discovery_calls = 3 live_same_failure = 2
//!     routine_tool_calls = 120 idle_seconds = 120
//!     rescue = true           rescue_tool_calls = 5  rescue_seconds = 180
//!     offline_failures = 4    offline_seconds = 30   task_offline_failures = 6 task_offline_seconds = 120
//!     live_offline_failures = 2 live_offline_seconds = 8
//!
//! Searching its own tools (2026-10-28, real-day bench: 9 `tool_search` and 2 `tool_describe`, all different, in 3
//! minutes, unheld): Hermes' `tool_search` and `tool_describe` count apart. The same query (`discovery_query`: its
//! words, whatever the options) is told at 3 and stops the turn at `discovery_same` (5); different queries count
//! toward `discovery_calls` (8 in all, told at 5). The usual plain line follows (« je cherchais sans fin un outil
//! pour le faire »), then the rescue turn, which may not search its tools again.
//!
//! Read-only lookups (2026-10-28, Alex's home server: Vie diagnosing a nightly job was stopped after 5 calls, 3 of
//! them the same `session_search`): searching past conversations, the Carnet, the web, files, mails or the memory
//! the same way again is how a diagnosis goes, so it takes `same_lookup` identical calls (5), not `same_call` (3,
//! for writes and actions). A call made again right after the identical call failed (`tool.completed` with an
//! error) is a retry, never counted as « the same call ».
//!
//! The rescue turn (2026-10-28, Alex: a bare « I stopped » helps nobody): a person's conversation turn the brake
//! stopped is followed by ONE short turn of the same agent in the same conversation (runs.rs `loop_rescue`), told
//! out of sight (`rescue_input`, its row hidden by the apps: `[bipagents:loop_rescue]`) why it was stopped, not to
//! call that tool again, and to answer with what it has: what it tried, what it found, what blocked it, one or two
//! ways forward. That turn has tight limits (`rescue_limits`: `rescue_tool_calls` calls, the stopped tool refused at
//! the bridge's MCP and stopping the turn at its second call) and is never rescued itself. At its first limit it is
//! told once to stop calling tools and answer (`RESCUE_ANSWER_NOW`, 27/11: 4 rescue turns of 9 said nothing, stopped
//! for calling the tool again), and stopped only at the next. The person reads the brake's short line
//! (`notice_head`), then the agent's own reply; when the rescue fails too, the full plain line (in the conversation
//! too: `RESCUE_THEN`). Words the person added meanwhile are told to it and taken up right after it (runs.rs
//! `loop_rescue`, msg_queue.rs `carry`); a stopped `give_file` or page's answer is given to it (`rescue_answer_note`).
//!
//! Checking what is done is no loop (27/11: a preference noted, then « je vérifie… » stopped as « la même étape »):
//! Hermes' `memory` again with the same words is held to `same_lookup`, an answer « already there » is not counted and
//! tells the agent it is done (`Warning::Saved`); the bridge's write asked again answers « Already done », what it
//! answered, and is stopped at `same_lookup` (a refused one at `same_call`).
//! Background tasks and routines get no rescue turn.
//!
//! A tool failing again and again (20/11, Alex in a Live call: « Laisse-moi vérifier sur le web », then Hermes' own
//! guard at 50 web searches, its English line in the conversation): a failed call retried is no « same call » (above),
//! and a web search is a work tool (no `same_tool`), so only the total held it, past Hermes' own cap of 50 searches.
//! Now one tool that failed `same_failure` times in a row (3; a task 5, a Live call 2), whatever its arguments and
//! retries included, stops the turn with « Je n'arrive pas à chercher sur le web pour l'instant. Tu veux que je
//! réessaie plus tard ? » (`Over::Failing`), then the rescue turn. A Live call's turns are not runs the hub follows
//! (Hermes' `chat/stream`, live_turns.rs): their stream is braked there, with the `live` limits.
//!
//! The network away for a few seconds (27/11 08:52, Alex's aibox on Wi-Fi: `web_search` failed three times in three
//! seconds, Tavily's name not resolved then Hermes' keyless rescue's, and `same_failure` stopped the turn with « Je
//! n'arrive pas à chercher sur le web ») is no broken tool. A failure whose answer says the network failed
//! (`transient_failure`: a name not resolved, a connection refused or cut, a time-out, a 5xx, a 429, as Hermes and
//! urllib3 word them) neither counts toward `same_failure` nor clears it; the agent is told once to wait a few seconds
//! and try again (`Warning::Network`), and the turn is stopped only when such errors come `offline_failures` times in
//! a row over at least `offline_seconds` (or twice as many whatever the time), with « Je n'arrive pas à joindre
//! Internet pour l'instant. Tu veux que je réessaie plus tard ? » (`Over::Offline`), then the rescue turn.
//!
//! Searching the web again and again, every search answered (the same 20/11 call, seen in the place's log: no search
//! failed, the model kept trying new words; once already on 19/11): more than `web_searches` in a turn (20; a task 40,
//! a call 6: Alex, 20/11) stops it, told at about two thirds with « answer with what you found, or offer a background task »
//! (`Warning::Searches`), then « J'ai fait beaucoup de recherches sur le web sans trouver de réponse sûre. Dis-moi si
//! tu veux que j'en fasse une recherche approfondie en tâche de fond. » and the rescue turn (no more searching).
//!
//! No Python counterpart (owner group: push-runs).

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use crate::tools_knowledge::tool_text;
use crate::Dict;

/// One kind of turn's limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// One tool called more than this many times stops the turn.
    pub same_tool: u32,
    /// More calls than this in all stops it.
    pub tool_calls: u32,
    /// The same call (tool and arguments) this many times stops it…
    pub same_call: u32,
    /// … but a read-only lookup's (`lookup_tool`: past conversations, the Carnet, the web, files…) this many times.
    pub same_lookup: u32,
    /// The bridge's write tools refuse past this many writes.
    pub writes: u32,
    /// The visible browser: more calls than this stops the turn…
    pub browser_calls: u32,
    /// … and more page reads (its content, a screenshot, a snapshot) than this.
    pub page_reads: u32,
    /// Searching its own tools (Hermes' `tool_search`, `tool_describe`): the same query this many times stops the
    /// turn…
    pub discovery_same: u32,
    /// … and more discovery calls than this in all, whatever the queries.
    pub discovery_calls: u32,
    /// One tool that failed this many times in a row (whatever its arguments, retries included, no call of any tool
    /// that went through in between) stops the turn (20/11: a Live call's web search failing, retried 50 times). Not the work tools whose
    /// failure is work output (`FAILURE_TOLERANT_TOOLS`: a red test run), nor the visible browser (its own limits).
    pub same_failure: u32,
    /// More web searches than this in one turn, answered or not, stop it (20/11: Vie on the Spark searched 50 times with
    /// new words each time, every search answered, never settling: Hermes' own cap, twice that day and once the day
    /// before). A long research belongs in a background task (`task_web_searches`).
    pub web_searches: u32,
    /// A network error (a name not resolved, a connection refused or reset, a time-out, a 5xx, a 429:
    /// `transient_failure`) is no broken tool (27/11, Alex's aibox on Wi-Fi: three in three seconds stopped a turn at
    /// `same_failure`): counted apart, any tool, the turn stopped once they come this many times in a row over at
    /// least `offline_seconds`…
    pub offline_failures: u32,
    pub offline_seconds: f64,
}

/// `[brake]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub conversation: Limits,
    pub task: Limits,
    /// A Live call's turns (live_turns.rs): a person waits on the line, in silence while the agent works.
    pub live: Limits,
    /// A turn no followed run carries (a routine): more bridge tool calls than this are refused.
    pub routine_tool_calls: u32,
    /// With no run followed: the agent's calls form one turn until it has been quiet this long.
    pub idle_seconds: f64,
    /// A limit getting close (`soft_limit`, about two thirds) is told to the agent once, before any stop.
    pub warn: bool,
    /// A stopped conversation turn is followed by one short turn that explains (runs.rs `loop_rescue`)…
    pub rescue: bool,
    /// … with at most this many tool calls…
    pub rescue_tool_calls: u32,
    /// … and this long at most before it is stopped too.
    pub rescue_seconds: f64,
    /// A person's turn that ends on a promise with no work done gets one continuation (runs.rs `go_on`, go_on.rs).
    pub go_on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            conversation: Limits {
                same_tool: 10,
                tool_calls: 60,
                same_call: 3,
                same_lookup: 5,
                writes: 10,
                browser_calls: 40,
                page_reads: 15,
                discovery_same: 5,
                discovery_calls: 8,
                same_failure: 3,
                // the bench's longest legitimate research turn makes 12 (brake-traces `web_research_long`), the real
                // day's 5 or 6
                web_searches: 20,
                // a Wi-Fi that drops for a few seconds: told to wait, stopped after half a minute of it
                offline_failures: 4,
                offline_seconds: 30.0,
            },
            // under Hermes' own cap of 150 model calls (tool_diet.rs), so the brake speaks first
            task: Limits {
                same_tool: 40,
                tool_calls: 120,
                same_call: 5,
                same_lookup: 8,
                writes: 40,
                browser_calls: 80,
                page_reads: 30,
                discovery_same: 5,
                discovery_calls: 15,
                same_failure: 5,
                web_searches: 40,
                offline_failures: 6,
                offline_seconds: 120.0,
            },
            // a call: a few quick lookups, then speak (the media, long and browser tools are closed there:
            // live_turns.rs `LIVE_CLOSED_TOOLS`); a tool failing twice in a row is said out loud at once
            live: Limits {
                same_tool: 5,
                tool_calls: 12,
                same_call: 2,
                same_lookup: 3,
                writes: 5,
                browser_calls: 5,
                page_reads: 3,
                discovery_same: 2,
                discovery_calls: 3,
                same_failure: 2,
                // a call's good turn makes one search (the lab, 20/11), two or three to refine; Alex 20/11: 6
                web_searches: 6,
                // a person waits on the line: a few seconds at most
                offline_failures: 2,
                offline_seconds: 8.0,
            },
            // a routine is scheduled work (the task's limits): as many bridge calls as a task, under Hermes' cap of 150
            routine_tool_calls: 120,
            idle_seconds: 120.0,
            warn: true,
            rescue: true,
            rescue_tool_calls: 5,
            rescue_seconds: 180.0,
            go_on: true,
        }
    }
}

impl Limits {
    /// The person said « Continuer » at the brake's two thirds (turn_status.rs, runs.rs `brake_action`): this turn's
    /// limits half as high again, once.
    pub fn raised(self) -> Limits {
        let up = |n: u32| n + n.div_ceil(2);
        Limits {
            same_tool: up(self.same_tool),
            tool_calls: up(self.tool_calls),
            same_call: up(self.same_call),
            same_lookup: up(self.same_lookup),
            writes: up(self.writes),
            browser_calls: up(self.browser_calls),
            page_reads: up(self.page_reads),
            discovery_same: up(self.discovery_same),
            discovery_calls: up(self.discovery_calls),
            same_failure: up(self.same_failure),
            web_searches: up(self.web_searches),
            offline_failures: up(self.offline_failures),
            offline_seconds: self.offline_seconds * 1.5,
        }
    }
}

/// The stop a warning announces (« Arrêter » at the two thirds takes it at once: runs.rs `brake_action`).
pub fn over_of(warning: &Warning) -> Over {
    match warning {
        Warning::SameCall(tool, _, _) => Over::SameCall(tool.clone()),
        Warning::SameTool(tool, _, _) => Over::SameTool(tool.clone()),
        Warning::TooMany(..) => Over::TooMany,
        Warning::PageReads(..) => Over::PageReads,
        Warning::Browser(..) => Over::Browser,
        Warning::Searches(..) => Over::Searches,
        Warning::DiscoverySame(..) | Warning::Discovery(..) => Over::Discovery,
        Warning::Network(tool) => Over::Offline(tool.clone()),
        Warning::Saved(tool) => Over::SameCall(tool.clone()),
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let d = Settings::default();
        let n = |key: &str, default: u32, min: u32| -> u32 {
            match table.get(key).and_then(Value::as_i64) {
                Some(v) => v.clamp(min as i64, 100_000) as u32,
                None => default,
            }
        };
        let limits = |prefix: &str, d: Limits| Limits {
            same_tool: n(&format!("{prefix}same_tool"), d.same_tool, 2),
            tool_calls: n(&format!("{prefix}tool_calls"), d.tool_calls, 5),
            same_call: n(&format!("{prefix}same_call"), d.same_call, 2),
            same_lookup: n(&format!("{prefix}same_lookup"), d.same_lookup, 2),
            writes: n(&format!("{prefix}writes"), d.writes, 1),
            browser_calls: n(&format!("{prefix}browser_calls"), d.browser_calls, 5),
            page_reads: n(&format!("{prefix}page_reads"), d.page_reads, 2),
            discovery_same: n(&format!("{prefix}discovery_same"), d.discovery_same, 2),
            discovery_calls: n(&format!("{prefix}discovery_calls"), d.discovery_calls, 2),
            same_failure: n(&format!("{prefix}same_failure"), d.same_failure, 2),
            web_searches: n(&format!("{prefix}web_searches"), d.web_searches, 2),
            offline_failures: n(&format!("{prefix}offline_failures"), d.offline_failures, 2),
            offline_seconds: table
                .get(format!("{prefix}offline_seconds").as_str())
                .and_then(Value::as_f64)
                .unwrap_or(d.offline_seconds)
                .clamp(0.0, 3_600.0),
        };
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(d.enabled),
            conversation: limits("", d.conversation),
            task: limits("task_", d.task),
            live: limits("live_", d.live),
            routine_tool_calls: n("routine_tool_calls", d.routine_tool_calls, 5),
            idle_seconds: table.get("idle_seconds").and_then(Value::as_f64).unwrap_or(d.idle_seconds).clamp(10.0, 86_400.0),
            warn: table.get("warn").and_then(Value::as_bool).unwrap_or(d.warn),
            rescue: table.get("rescue").and_then(Value::as_bool).unwrap_or(d.rescue),
            rescue_tool_calls: n("rescue_tool_calls", d.rescue_tool_calls, 1),
            rescue_seconds: table
                .get("rescue_seconds")
                .and_then(Value::as_f64)
                .unwrap_or(d.rescue_seconds)
                .clamp(20.0, 1_800.0),
            go_on: table.get("go_on").and_then(Value::as_bool).unwrap_or(d.go_on),
        }
    }

    /// The rescue turn's limits: a few calls to answer, never a new round of work.
    pub fn rescue_limits(&self) -> Limits {
        let calls = self.rescue_tool_calls;
        Limits {
            same_tool: 3,
            tool_calls: calls,
            same_call: 2,
            same_lookup: 2,
            writes: 2,
            browser_calls: calls,
            page_reads: 2,
            discovery_same: 2,
            discovery_calls: 2,
            same_failure: 2,
            web_searches: 2,
            // the network still away: said at once
            offline_failures: 2,
            offline_seconds: 0.0,
        }
    }

    pub fn limits(&self, background: bool) -> Limits {
        if background {
            self.task
        } else {
            self.conversation
        }
    }
}

/// The bridge's tools as Hermes names them (`mcp__bipagents__calendar_add`, `mcp_bipagents_calendar_add`): the bare
/// name.
pub fn bare(tool: &str) -> &str {
    for prefix in ["mcp__bipagents__", "mcp_bipagents_"] {
        if let Some(rest) = tool.strip_prefix(prefix) {
            return rest;
        }
    }
    tool
}

/// Tools a turn legitimately calls many times: held to the total only, never to `same_tool` (nor to `same_call`
/// for the first ones, whose short preview repeats while the work differs: a test run again after a fix).
const WORK_TOOLS: [&str; 10] = [
    "terminal",
    "execute_code",
    "process",
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "web_search",
    "web_extract",
    "delegate_task",
];
const PREVIEW_ONLY_TOOLS: [&str; 3] = ["terminal", "execute_code", "process"];
/// Tools whose failure is work output, never a loop by itself (a red test run, a page that refuses to be read among
/// several): not held to `same_failure` (Hermes' `FAILURE_TOLERANT_TOOL_NAMES`, agent/tool_guardrails.py).
const FAILURE_TOLERANT_TOOLS: [&str; 5] = ["terminal", "execute_code", "process", "web_extract", "delegate_task"];

/// The visible browser's tools that read the page (`mcp__stealth__get_page_content`, a screenshot, Hermes'
/// `browser_snapshot`…): the bench's loop read the same page 141 and 145 times.
pub fn page_read_tool(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    ["page_content", "get_page", "page_text", "read_page", "screenshot", "snapshot", "browser_vision", "get_content"]
        .iter()
        .any(|t| tool.contains(t))
}

/// The bridge's tools that change something for the person.
pub const WRITE_TOOLS: [&str; 20] = [
    "calendar_add",
    "calendar_update",
    "calendar_delete",
    "reminders_add",
    "reminders_complete",
    "contacts_add",
    "contacts_update",
    "notebook_write",
    "notebook_task",
    "notebook_record",
    "mail_draft",
    "mail_bulk",
    "create_page",
    "delete_page",
    "create_audio",
    "create_podcast",
    "tell_family",
    "send_files_to_agent",
    "give_file",
    // a file saved in the person's online documents (drives_write.rs, review C8)
    "drive_write",
];

pub fn write_tool(tool: &str) -> bool {
    WRITE_TOOLS.contains(&bare(tool))
}

/// A tool that reads or writes by its arguments: `page_data` writes its page's store with `ops` (page_store.rs, the
/// agent's writing, 16/11), and reads it without.
pub const WRITES_WITH_OPS: &str = "page_data";

/// A call that changes something for the person: a write tool, or `page_data` with `ops` (a live page fed by a
/// looping routine is held like any other write: `writes`, `task_writes`).
pub fn write_call(tool: &str, arguments: &Dict) -> bool {
    write_tool(tool) || (bare(tool) == WRITES_WITH_OPS && arguments.get("ops").is_some_and(|o| !o.is_null()))
}

/// Write tools that only propose: each call makes or joins a card, and nothing changes before the person's « Oui »
/// (mail_bulk: a waiting card for the same action joins it, mail_tidy.rs). Not held to `same_tool` nor counted in
/// `writes`; still to the total, and the very same call twice is « already done ». 5/11, token lab (GLM-5.3 Flash,
/// mail_bulk in view): one call per sender, fourteen at once, and the turn was stopped at the eleventh.
pub const PROPOSING_TOOLS: [&str; 1] = ["mail_bulk"];

pub fn proposing_tool(tool: &str) -> bool {
    PROPOSING_TOOLS.contains(&bare(tool))
}

/// Read-only lookups: searching or reading what is there (Hermes' past conversations, the Carnet, the web, files,
/// mails, the calendar, contacts, the agent's pages, skills, its memory read), held to `same_lookup`.
const LOOKUP_TOOLS: [&str; 15] = [
    "session_search",
    "web_search",
    "web_extract",
    "read_file",
    "search_files",
    "notebook_search",
    "notebook_read",
    "read_page",
    "list_pages",
    "page_data",
    "skill_view",
    "skills_list",
    "tool_search",
    "tool_describe",
    "message_search",
];

/// `signature`: the call's arguments or preview (Hermes' `memory` both reads and writes: a read only by its action).
pub fn lookup_tool(tool: &str, signature: Option<&str>) -> bool {
    let name = bare(tool);
    if write_tool(name) {
        return false;
    }
    if name == WRITES_WITH_OPS && signature.is_some_and(|s| s.contains("\"ops\"")) {
        return false; // writing its page's store: a write, held to `same_call`
    }
    if name == "memory" {
        let s = signature.unwrap_or("").to_ascii_lowercase();
        return ["\"read\"", "\"view\"", "\"search\"", "\"list\""].iter().any(|a| s.contains(a))
            || s.starts_with("read")
            || s.starts_with("search");
    }
    LOOKUP_TOOLS.contains(&name)
        || name.starts_with("knowledge_")
        || ["_search", "_list", "_read", "_get"].iter().any(|end| name.ends_with(end))
}

/// Hermes' tool search (tool_diet.rs: the tools behind it): looking for a tool, not doing the work. The real-day
/// bench of 2026-10-28 (GLM « medium », « ask Coach » with no message_agent in view) made 9 `tool_search` and 2
/// `tool_describe` in 3 minutes, all different, and nothing held it: each stayed under `same_tool` and `same_lookup`.
pub const DISCOVERY_TOOLS: [&str; 2] = ["tool_search", "tool_describe"];

pub fn discovery_tool(tool: &str) -> bool {
    DISCOVERY_TOOLS.contains(&bare(tool))
}

/// What a discovery call looked for (its query or the tool it describes, in lower case, spaces folded): the same
/// query again with other options is the same search. A signature that isn't JSON (a preview, a hash) is used whole.
pub fn discovery_query(signature: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(signature).ok();
    let found = parsed.as_ref().and_then(|v| {
        ["query", "q", "name", "tool", "tool_name", "names"]
            .iter()
            .find_map(|k| v.get(*k))
            .map(|q| q.as_str().map(str::to_string).unwrap_or_else(|| q.to_string()))
    });
    let text = found.unwrap_or_else(|| signature.to_string()).to_lowercase();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tool's answer that says it failed (`tool.completed`: Hermes' `error`, or an error in its preview).
pub fn completed_with_error(data: &serde_json::Map<String, Value>) -> bool {
    if data.get("error").and_then(Value::as_bool).unwrap_or(false) {
        return true;
    }
    let preview = data.get("preview").and_then(Value::as_str).unwrap_or("").trim_start();
    let head = preview.chars().take(80).collect::<String>().to_ascii_lowercase();
    head.starts_with("error")
        || head.starts_with("{\"error\"")
        || crate::tool_answers::rendered_not_done(preview)
        || head.contains("\"success\": false")
        || head.contains("\"success\":false")
}

/// A failed tool's answer (`tool.completed`, its `error` or its preview: `completed_with_error`) that says the network
/// failed, not the tool: a name not resolved, a connection refused, reset or cut, a time-out, the other side busy (a
/// 5xx, a 429). Read in its preview as Hermes words it (urllib3's `NameResolutionError`, `Max retries exceeded`, the
/// keyless rescue's « request failed: … », `[Errno 111] Connection refused`, « HTTP 503 »…). A client error (HTTP 4xx
/// but 408 and 429) is the tool's: a wrong key, a wrong query, whatever else it says.
pub fn transient_failure(data: &serde_json::Map<String, Value>) -> bool {
    static STATUS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:http|status(?:\s+code)?|error\s+code)\b[\s:=']*(\d{3})\b").expect("valid regex")
    });
    if !completed_with_error(data) && data.get("is_error").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let preview = data.get("preview").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    // the first status said decides: a quoted query further on (« invalid query 'http 503' ») doesn't
    if let Some(code) = STATUS.captures(&preview).and_then(|c| c[1].parse::<u16>().ok()) {
        if code == 408 || code == 429 || (500..600).contains(&code) {
            return true;
        }
        if (400..500).contains(&code) {
            return false;
        }
    }
    const NETWORK: [&str; 28] = [
        // the name not resolved
        "nameresolutionerror", "name resolution", "failed to resolve", "name or service not known",
        "nodename nor servname", "getaddrinfo failed", "no address associated with hostname", "could not resolve host",
        // no connection, or cut
        "connection refused", "connectionrefusederror", "connection reset", "connectionreseterror",
        "connection aborted", "connectionabortederror", "remotedisconnected", "remote end closed connection",
        "network is unreachable", "no route to host", "newconnectionerror", "failed to establish a new connection",
        // too slow
        "timed out", "readtimeout", "connecttimeout", "timeouterror",
        // the other side busy
        "too many requests", "service unavailable", "bad gateway", "gateway timeout",
    ];
    NETWORK.iter().any(|m| preview.contains(m))
}

/// A tool's answer (`tool.completed`, not failed) that says what it was asked is there already: Hermes' `memory`
/// (« Entry already exists (no duplicate added). »), and the like. Read in its preview's first words.
pub fn already_there(data: &serde_json::Map<String, Value>) -> bool {
    let preview = data.get("preview").and_then(Value::as_str).unwrap_or("");
    let head = preview.chars().take(300).collect::<String>().to_ascii_lowercase();
    ["entry already exists", "no duplicate added", "already saved", "already exists in", "already in memory"]
        .iter()
        .any(|w| head.contains(w))
}

/// Why a turn was stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Over {
    /// One tool, too many times (its bare name).
    SameTool(String),
    /// Too many calls in all.
    TooMany,
    /// The same call again and again (its bare tool name).
    SameCall(String),
    /// The visible browser read the page too many times.
    PageReads,
    /// The visible browser, too many calls.
    Browser,
    /// Searching its own tools again and again (the same query, or too many searches).
    Discovery,
    /// One tool failing again and again (its bare name; « » for a tool Hermes did not name).
    Failing(String),
    /// Too many web searches in one turn, answered or not.
    Searches,
    /// The network away for a while: every call that needs it failing (`transient_failure`; the last one's bare name).
    Offline(String),
}

/// A turn's tool calls so far.
#[derive(Debug, Default, Clone)]
pub struct TurnCount {
    pub total: u32,
    pub browser: u32,
    pub page_reads: u32,
    per_tool: HashMap<String, u32>,
    per_call: HashMap<String, u32>,
    /// Discovery calls (`DISCOVERY_TOOLS`) in all, and by query.
    pub discovery: u32,
    per_query: HashMap<String, u32>,
    /// The turn's last call (tool and arguments), and whether it answered with an error: the identical call right
    /// after is a retry, not « the same call again ».
    last_call: Option<String>,
    last_tool: String,
    last_failed: bool,
    /// Each tool's failures since the turn's last call that went through (bare name): `same_failure`.
    failures: HashMap<String, u32>,
    /// The network errors in a row (any tool), and when the first of them came: `offline_failures`, `offline_seconds`.
    offline: u32,
    offline_since: f64,
    /// Web searches in this turn, answered or not: `web_searches`.
    pub searches: u32,
    /// A rescue turn's stopped tool (bare name): its second call stops the turn (the first is refused at the MCP for
    /// the bridge's tools, and the rescue's instruction says not to call it).
    pub banned: Option<String>,
    banned_calls: u32,
    /// The limits already brought close in this turn (told once each), and the one the last call brought close.
    warned: HashSet<String>,
    warning: Option<Warning>,
}

/// About two thirds of a hard limit: where the agent is told it gets close (same call 3 of 5 for a lookup, 2 of 3
/// for a write, 40 of 60 calls, 7 of 10 for one tool).
pub fn soft_limit(hard: u32) -> u32 {
    (2 * hard + 1) / 3
}

/// A limit the turn gets close to: the agent is told, in a steer (runs.rs `brake`), before being stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// The same call (tool, how many times, the hard limit it is stopped at).
    SameCall(String, u32, u32),
    /// One tool many times (tool, how many times, the hard limit it is stopped past).
    SameTool(String, u32, u32),
    /// Many calls in all, many page reads, many browser calls (how many, the hard limit).
    TooMany(u32, u32),
    PageReads(u32, u32),
    Browser(u32, u32),
    /// Many web searches (how many, the hard limit it is stopped past).
    Searches(u32, u32),
    /// Tool searches: the same query (how many times, the hard limit it is stopped at)…
    DiscoverySame(u32, u32),
    /// … or many in all (how many, the hard limit it is stopped past).
    Discovery(u32, u32),
    /// A tool couldn't reach the network (its bare name): wait a few seconds, then try again.
    Network(String),
    /// A tool answered that what it was asked is there already (its bare name: Hermes' `memory` « Entry already
    /// exists »): it is done, nothing to call again.
    Saved(String),
}

/// The steers the bridge's brake gives start with this (the apps hide such rows: `[bipagents:`).
pub const WARN_MARK: &str = "[bipagents:brake]";

impl Warning {
    fn key(&self) -> String {
        match self {
            Warning::SameCall(tool, _, _) => format!("call:{tool}"),
            Warning::SameTool(tool, _, _) => format!("tool:{tool}"),
            Warning::TooMany(..) => "total".into(),
            Warning::PageReads(..) => "page_reads".into(),
            Warning::Browser(..) => "browser".into(),
            Warning::Searches(..) => "web_searches".into(),
            Warning::DiscoverySame(..) => "discovery_same".into(),
            Warning::Discovery(..) => "discovery".into(),
            Warning::Network(_) => "network".into(),
            Warning::Saved(tool) => format!("saved:{tool}"),
        }
    }

    /// For the logs.
    pub fn reason(&self) -> String {
        match self {
            Warning::SameCall(tool, n, _) => format!("same call {n} times ({tool})"),
            Warning::SameTool(tool, n, _) => format!("same tool {n} times ({tool})"),
            Warning::TooMany(n, _) => format!("{n} tool calls"),
            Warning::PageReads(n, _) => format!("the browser read the page {n} times"),
            Warning::Browser(n, _) => format!("{n} browser calls"),
            Warning::Searches(n, _) => format!("{n} web searches"),
            Warning::DiscoverySame(n, _) => format!("the same tool search {n} times"),
            Warning::Discovery(n, _) => format!("{n} tool searches"),
            Warning::Network(tool) => format!("a network error ({tool})"),
            Warning::Saved(tool) => format!("already saved ({tool})"),
        }
    }

    /// The note the agent gets (a steer, after its current tool calls), in English, out of the person's sight.
    pub fn text(&self) -> String {
        if let Warning::Saved(tool) = self {
            return format!(
                "{WARN_MARK} A note from the app, not from your person: {tool} just told you this is saved already: it \
is done, and calling it again changes nothing (that is no way to check it). Don't call {tool} again for it: tell your \
person plainly that it is noted."
            );
        }
        if let Warning::Network(tool) = self {
            return format!(
                "{WARN_MARK} A note from the app, not from your person: {tool} couldn't reach the internet (a network \
error: a name not resolved, a connection refused or cut, a time-out). It is often brief: wait a few seconds before \
trying again (with the terminal, `sleep 5`, if you have it), then try once or twice more. If it still fails, tell your \
person plainly that you can't reach the internet right now and offer to try again later; don't work around it with \
other tools."
            );
        }
        let what = match self {
            Warning::SameCall(tool, n, hard) => {
                format!("you have made exactly the same {tool} call {n} times in this turn; at {hard} the turn is stopped")
            }
            Warning::SameTool(tool, count, hard) => {
                format!("you have called {tool} {count} times in this turn; past {hard} the turn is stopped")
            }
            Warning::TooMany(count, hard) => {
                format!("you have made {count} tool calls in this turn; past {hard} the turn is stopped")
            }
            Warning::PageReads(count, hard) => {
                format!("you have read the page {count} times in this turn; past {hard} the turn is stopped")
            }
            Warning::Searches(count, hard) => format!(
                "you have made {count} web searches in this turn; past {hard} the turn is stopped. Searching again with \
other words rarely settles it: answer the person now with what you found and say plainly what is still uncertain, or, \
for a long research, offer to carry it on as a background task (propose_task)"
            ),
            Warning::Browser(count, hard) => {
                format!("you have made {count} browser steps in this turn; past {hard} the turn is stopped")
            }
            Warning::DiscoverySame(count, hard) => format!(
                "you have searched your tools for the same thing {count} times in this turn; at {hard} the turn is \
stopped. If no tool does it, say so to the person"
            ),
            Warning::Discovery(count, hard) => format!(
                "you have searched your tools {count} times in this turn; past {hard} the turn is stopped. Use a tool \
you found, or tell the person plainly what you can't do"
            ),
            Warning::Network(_) | Warning::Saved(_) => String::new(), // above
        };
        format!(
            "{WARN_MARK} A note from the app, not from your person: {what}. Change approach (other words, another \
source, another tool), or answer the person now with what you have and say what is missing."
        )
    }
}

/// A call's arguments in a stable form (keys sorted), for telling the same call.
pub fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&map[k]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).unwrap_or_default()
}

impl TurnCount {
    /// One more call of `tool` (Hermes' name), with its arguments as the run's event tells them (`signature`: the
    /// arguments, else the event's short preview; None: nothing to tell calls apart). `browser`: one of the visible
    /// browser's tools. The first limit passed, if any.
    pub fn note(&mut self, tool: &str, signature: Option<&str>, browser: bool, limits: &Limits) -> Option<Over> {
        let name = bare(tool).to_string();
        self.total += 1;
        let key = signature.filter(|s| !s.trim().is_empty()).map(|s| format!("{name}\u{1f}{s}"));
        let retry = self.last_failed && key.is_some() && self.last_call == key;
        self.last_call = key;
        self.last_tool = name.clone();
        self.last_failed = false;
        if self.banned.as_deref() == Some(name.as_str()) {
            self.banned_calls += 1;
            if self.banned_calls > 1 {
                return Some(Over::SameCall(name));
            }
        }
        // the first limit getting close: told once before the stop (`warning`)
        let mut close: Option<Warning> = None;
        let mut near = |count: u32, hard: u32, at_hard: bool, warning: &dyn Fn() -> Warning| {
            // `at_hard`: the limit is reached at `hard` (else passed after it)
            let soft = soft_limit(hard);
            if close.is_none() && soft >= 2 && soft < hard + u32::from(!at_hard) && count == soft {
                close = Some(warning());
            }
        };
        if browser {
            self.browser += 1;
            if page_read_tool(&name) {
                self.page_reads += 1;
                if self.page_reads > limits.page_reads {
                    return Some(Over::PageReads);
                }
                near(self.page_reads, limits.page_reads, false, &|| Warning::PageReads(self.page_reads, limits.page_reads));
            }
            if self.browser > limits.browser_calls {
                return Some(Over::Browser);
            }
            near(self.browser, limits.browser_calls, false, &|| Warning::Browser(self.browser, limits.browser_calls));
        }
        if discovery_tool(&name) {
            self.discovery += 1;
            let discovery = self.discovery;
            if let Some(query) = signature.filter(|s| !s.trim().is_empty()).map(discovery_query) {
                let again = self.per_query.entry(query).or_insert(0);
                *again += 1;
                let again = *again;
                if again >= limits.discovery_same {
                    return Some(Over::Discovery);
                }
                near(again, limits.discovery_same, true, &|| Warning::DiscoverySame(again, limits.discovery_same));
            }
            if discovery > limits.discovery_calls {
                return Some(Over::Discovery);
            }
            near(discovery, limits.discovery_calls, false, &|| Warning::Discovery(discovery, limits.discovery_calls));
        }
        if name == "web_search" {
            self.searches += 1;
            if self.searches > limits.web_searches {
                return Some(Over::Searches);
            }
            let searches = self.searches;
            near(searches, limits.web_searches, false, &|| Warning::Searches(searches, limits.web_searches));
        }
        let work = browser || WORK_TOOLS.contains(&name.as_str()) || proposing_tool(&name);
        let same = self.per_tool.entry(name.clone()).or_insert(0);
        *same += 1;
        let same = *same;
        if !work && same > limits.same_tool {
            return Some(Over::SameTool(name));
        }
        if !work {
            near(same, limits.same_tool, false, &|| Warning::SameTool(name.clone(), same, limits.same_tool));
        }
        if let Some(signature) = signature.filter(|s| !s.trim().is_empty()) {
            if !browser && !PREVIEW_ONLY_TOOLS.contains(&name.as_str()) && !retry {
                let again = self.per_call.entry(format!("{name}\u{1f}{signature}")).or_insert(0);
                *again += 1;
                let again = *again;
                // Hermes' `memory` keeps an entry once (« Entry already exists »): the same write again is no harm, so
                // a lookup's limit (27/11: checked by adding it again)
                let idempotent = name == "memory";
                let limit =
                    if idempotent || lookup_tool(&name, Some(signature)) { limits.same_lookup } else { limits.same_call };
                if again >= limit {
                    return Some(Over::SameCall(name));
                }
                near(again, limit, true, &|| Warning::SameCall(name.clone(), again, limit));
            }
        }
        if self.total > limits.tool_calls {
            return Some(Over::TooMany);
        }
        near(self.total, limits.tool_calls, false, &|| Warning::TooMany(self.total, limits.tool_calls));
        if let Some(warning) = close {
            if self.warned.insert(warning.key()) {
                self.warning = Some(warning);
            }
        }
        None
    }

    /// The limit the last call brought close (once per kind and tool in a turn), for the note the agent gets.
    pub fn take_warning(&mut self) -> Option<Warning> {
        self.warning.take()
    }

    /// The turn's call of `tool` answered (`tool.completed`): `failed` when with an error. The limit it passed, if
    /// any: that tool failed `same_failure` times in a row.
    pub fn completed(&mut self, tool: &str, failed: bool, limits: &Limits) -> Option<Over> {
        let name = bare(tool).to_string();
        if name == self.last_tool {
            self.last_failed = failed;
        }
        if !failed {
            // any call that went through is progress (an edit, then the test run again): the failures start again
            // (review 20/11: Hermes' own guard resets on its progress tools; here on any success, Live included)
            self.failures.clear();
            self.offline = 0;
            return None;
        }
        if name.is_empty() || FAILURE_TOLERANT_TOOLS.contains(&name.as_str()) || crate::runs::browser_tool(&name) {
            return None;
        }
        let count = self.failures.entry(name.clone()).or_insert(0);
        *count += 1;
        (*count >= limits.same_failure).then_some(Over::Failing(name))
    }

    /// The turn's call of `tool` answered that what it was asked is there already (`already_there`: Hermes' `memory`
    /// « Entry already exists (no duplicate added) »): the same call made again to check it (27/11, Alex's aibox: a
    /// preference noted, then « je vérifie que ta préférence est bien enregistrée », stopped as « la même étape »
    /// twice) is not counted toward `same_call` (its other limits still hold: `same_tool`, the total), and the agent
    /// is told once that it is done.
    pub fn confirmed(&mut self, tool: &str) {
        let name = bare(tool).to_string();
        if name != self.last_tool {
            return;
        }
        if let Some(count) = self.last_call.clone().and_then(|key| self.per_call.get_mut(&key)) {
            *count = count.saturating_sub(1);
        }
        if self.warned.insert(Warning::Saved(name.clone()).key()) {
            self.warning = Some(Warning::Saved(name));
        }
    }

    /// The turn's call of `tool` failed on the network (`transient_failure`) at `now`: neither counted as the tool
    /// failing (`same_failure`) nor clearing those failures. The agent is told once in the turn to wait a few seconds
    /// before trying again (`Warning::Network`); the turn is stopped (`Over::Offline`) once such errors came
    /// `offline_failures` times in a row over at least `offline_seconds`, or twice as many times whatever the time.
    pub fn failed_transiently(&mut self, tool: &str, limits: &Limits, now: f64) -> Option<Over> {
        let name = bare(tool).to_string();
        if name == self.last_tool {
            self.last_failed = true;
        }
        if self.offline == 0 {
            self.offline_since = now;
        }
        self.offline += 1;
        let lasted = now - self.offline_since >= limits.offline_seconds;
        if (self.offline >= limits.offline_failures && lasted) || self.offline >= 2 * limits.offline_failures {
            return Some(Over::Offline(name));
        }
        if self.warned.insert(Warning::Network(String::new()).key()) {
            self.warning = Some(Warning::Network(name));
        }
        None
    }
}
/// What the person's subject was, in plain words: the area of the tool the turn kept calling.
fn area(tool: &str, lang: &str) -> Option<&'static str> {
    let tool = bare(tool);
    let pick = |fr, en, de, es| Some(match lang { "fr" => fr, "de" => de, "es" => es, _ => en });
    if tool.starts_with("calendar_") {
        pick("l'agenda", "the calendar", "dem Kalender", "la agenda")
    } else if tool.starts_with("reminders_") {
        pick("les rappels", "the reminders", "den Erinnerungen", "los recordatorios")
    } else if tool.starts_with("contacts_") {
        pick("les contacts", "the contacts", "den Kontakten", "los contactos")
    } else if tool.starts_with("notebook_") {
        pick("le Carnet", "the Notebook", "dem Notizbuch", "el Cuaderno")
    } else if tool.starts_with("mail_") {
        pick("les e-mails", "the e-mails", "den E-Mails", "los correos")
    } else if tool.ends_with("_page") || tool == "list_pages" {
        pick("une page", "a page", "einer Seite", "una página")
    } else if crate::runs::browser_tool(tool) {
        pick("le navigateur", "the browser", "dem Browser", "el navegador")
    } else if tool == "web_search" || tool == "web_extract" {
        pick("la recherche web", "the web search", "der Websuche", "la búsqueda web")
    } else {
        None
    }
}

/// A read-only lookup's purpose in plain words, as the apps word it (HermesKit ToolKind, the app's ToolKindText:
/// « Je cherche dans nos anciennes discussions… »): what the turn kept doing again, never a tool's name.
fn lookup_phrase(tool: &str, lang: &str) -> Option<&'static str> {
    let tool = bare(tool);
    let pick = |fr, en, de, es| Some(match lang { "fr" => fr, "de" => de, "es" => es, _ => en });
    if tool == "session_search" || tool == "message_search" {
        pick(
            "la même recherche dans nos anciennes discussions",
            "the same search in our past conversations",
            "dieselbe Suche in unseren früheren Unterhaltungen",
            "la misma búsqueda en nuestras conversaciones anteriores",
        )
    } else if tool.starts_with("notebook_") {
        pick("la même recherche dans le Carnet", "the same search in the Notebook", "dieselbe Suche im Notizbuch", "la misma búsqueda en el Cuaderno")
    } else if tool == "web_search" {
        pick("la même recherche sur le web", "the same web search", "dieselbe Websuche", "la misma búsqueda en la web")
    } else if tool == "web_extract" {
        pick(
            "la même lecture d'une page web",
            "the same reading of a web page",
            "dasselbe Lesen einer Webseite",
            "la misma lectura de una página web",
        )
    } else if tool == "read_file" || tool == "search_files" {
        pick("la même lecture de fichier", "the same file reading", "dasselbe Lesen einer Datei", "la misma lectura de un archivo")
    } else if tool.starts_with("mail_") {
        pick("la même recherche dans les e-mails", "the same search in the e-mails", "dieselbe Suche in den E-Mails", "la misma búsqueda en los correos")
    } else if tool.starts_with("calendar_") {
        pick("la même consultation de l'agenda", "the same look at the calendar", "denselben Blick in den Kalender", "la misma consulta de la agenda")
    } else if tool.starts_with("contacts_") {
        pick("la même recherche dans les contacts", "the same search in the contacts", "dieselbe Suche in den Kontakten", "la misma búsqueda en los contactos")
    } else if tool.starts_with("reminders_") {
        pick("la même consultation des rappels", "the same look at the reminders", "denselben Blick in die Erinnerungen", "la misma consulta de los recordatorios")
    } else if tool == "memory" || tool.starts_with("knowledge_") {
        pick("la même lecture de ma mémoire", "the same look into my memory", "denselben Blick in mein Gedächtnis", "la misma consulta de mi memoria")
    } else if tool == "read_page" || tool == "list_pages" || tool == "page_data" {
        pick("la même lecture de mes pages", "the same look at my pages", "denselben Blick in meine Seiten", "la misma consulta de mis páginas")
    } else {
        None
    }
}

fn language(lang: &str) -> String {
    lang.split(['-', '_']).next().unwrap_or("en").to_lowercase()
}

/// « the same call again » of a lookup: said as what it was for (« je relançais la même recherche dans nos anciennes
/// discussions »).
fn repeated_lookup(over: &Over, lang: &str) -> Option<&'static str> {
    match over {
        Over::SameCall(tool) if lookup_tool(tool, None) => lookup_phrase(tool, lang),
        _ => None,
    }
}

/// The line's first sentence: what happened. Alone above the rescue turn's reply (runs.rs `loop_rescue`).
pub fn notice_head(over: &Over, lang: &str) -> String {
    let lang = language(lang);
    let lang = lang.as_str();
    if let Over::Failing(tool) = over {
        return failing_head(tool, lang);
    }
    if let Over::Offline(_) = over {
        return match lang {
            "fr" => "Je n'arrive pas à joindre Internet pour l'instant.",
            "de" => "Ich komme gerade nicht ins Internet.",
            "es" => "Ahora mismo no consigo conectarme a internet.",
            "nl" => "Ik kan nu geen verbinding maken met internet.",
            _ => "I can't reach the internet right now.",
        }
        .to_string();
    }
    if let Over::Searches = over {
        return match lang {
            "fr" => "J'ai fait beaucoup de recherches sur le web sans trouver de réponse sûre.",
            "de" => "Ich habe viele Websuchen gemacht, ohne eine sichere Antwort zu finden.",
            "es" => "He hecho muchas búsquedas en la web sin encontrar una respuesta segura.",
            "nl" => "Ik heb veel op het web gezocht zonder een zeker antwoord te vinden.",
            _ => "I ran a lot of web searches without finding a sure answer.",
        }
        .to_string();
    }
    if let Some(what) = repeated_lookup(over, lang) {
        return match lang {
            "fr" => format!("Je me suis arrêté : je relançais {what}."),
            "de" => format!("Ich habe aufgehört: Ich habe immer wieder {what} gestartet."),
            "es" => format!("Me he detenido: repetía {what}."),
            _ => format!("I stopped: I kept repeating {what}."),
        };
    }
    match over {
        Over::SameTool(tool) | Over::SameCall(tool) => match (area(tool, lang), lang) {
            (Some(on), "fr") => format!("Je me suis arrêté : je tournais en rond sur {on}."),
            (None, "fr") => "Je me suis arrêté : je refaisais sans cesse la même étape.".to_string(),
            (Some(on), "de") => format!("Ich habe aufgehört: Ich habe mich bei {on} im Kreis gedreht."),
            (None, "de") => "Ich habe aufgehört: Ich habe immer wieder denselben Schritt gemacht.".to_string(),
            (Some(on), "es") => format!("Me he detenido: estaba dando vueltas con {on}."),
            (None, "es") => "Me he detenido: repetía el mismo paso una y otra vez.".to_string(),
            (Some(on), _) => format!("I stopped: I was going round in circles with {on}."),
            (None, _) => "I stopped: I kept repeating the same step.".to_string(),
        },
        Over::TooMany => match lang {
            "fr" => "Je me suis arrêté : cette demande me prenait beaucoup trop d'étapes d'un coup.",
            "de" => "Ich habe aufgehört: Diese Anfrage brauchte viel zu viele Schritte auf einmal.",
            "es" => "Me he detenido: esta petición me llevaba demasiados pasos de una vez.",
            _ => "I stopped: this request was taking far too many steps at once.",
        }
        .to_string(),
        Over::PageReads => match lang {
            "fr" => "Je me suis arrêté : je n'arrivais pas à lire cette page.",
            "de" => "Ich habe aufgehört: Ich konnte diese Seite nicht lesen.",
            "es" => "Me he detenido: no conseguía leer esta página.",
            _ => "I stopped: I couldn't read this page.",
        }
        .to_string(),
        Over::Browser => match lang {
            "fr" => "Je me suis arrêté : je n'arrivais pas à avancer dans le navigateur.",
            "de" => "Ich habe aufgehört: Ich bin im Browser nicht weitergekommen.",
            "es" => "Me he detenido: no conseguía avanzar en el navegador.",
            _ => "I stopped: I couldn't get anywhere in the browser.",
        }
        .to_string(),
        Over::Discovery => match lang {
            "fr" => "Je me suis arrêté : je cherchais sans fin un outil pour le faire.",
            "de" => "Ich habe aufgehört: Ich habe endlos nach einem Werkzeug dafür gesucht.",
            "es" => "Me he detenido: buscaba sin parar una herramienta para hacerlo.",
            _ => "I stopped: I kept looking for a tool to do it.",
        }
        .to_string(),
        Over::Failing(tool) => failing_head(tool, lang),
        Over::Searches | Over::Offline(_) => String::new(), // above
    }
}

/// What failed again and again, in plain words: what the person asked for that doesn't work right now (never a tool's
/// name). Also Hermes' own guard halt (hermes_halts.rs). Dutch too (its first agents, 2026-10-26).
fn failing_head(tool: &str, lang: &str) -> String {
    let tool = bare(tool);
    let pick = |fr: &str, en: &str, de: &str, es: &str, nl: &str| {
        match lang { "fr" => fr, "de" => de, "es" => es, "nl" => nl, _ => en }.to_string()
    };
    if tool == "web_search" {
        return pick(
            "Je n'arrive pas à chercher sur le web pour l'instant.",
            "I can't search the web right now.",
            "Ich kann gerade nicht im Web suchen.",
            "Ahora mismo no consigo buscar en la web.",
            "Ik kan nu niet op het web zoeken.",
        );
    }
    if tool == "web_extract" {
        return pick(
            "Je n'arrive pas à lire les pages web pour l'instant.",
            "I can't read web pages right now.",
            "Ich kann gerade keine Webseiten lesen.",
            "Ahora mismo no consigo leer páginas web.",
            "Ik kan nu geen webpagina's lezen.",
        );
    }
    match (area(tool, lang), lang) {
        (Some(on), "fr") => format!("Je n'arrive pas à utiliser {on} pour l'instant."),
        (Some(on), "de") => format!("Ich komme gerade mit {on} nicht weiter."),
        (Some(on), "es") => format!("Ahora mismo no consigo usar {on}."),
        (Some(on), "en") => format!("I can't use {on} right now."),
        _ => pick(
            "Une étape échoue à chaque essai, je m'arrête là pour l'instant.",
            "One step keeps failing, so I'm stopping here for now.",
            "Ein Schritt schlägt jedes Mal fehl, deshalb höre ich hier erst einmal auf.",
            "Un paso falla cada vez, así que me detengo aquí por ahora.",
            "Eén stap mislukt steeds, dus ik stop hier voorlopig.",
        ),
    }
}

/// The line's second sentence: what the person can do.
pub fn notice_tail(over: &Over, lang: &str) -> &'static str {
    let lang = language(lang);
    let lang = lang.as_str();
    if let Over::Searches = over {
        // a long research is a background task's (the long-work rule, runs.rs `handoff`)
        return match lang {
            "fr" => "Dis-moi si tu veux que j'en fasse une recherche approfondie en tâche de fond.",
            "de" => "Sag mir, ob ich daraus eine gründliche Suche im Hintergrund machen soll.",
            "es" => "Dime si quieres que haga una búsqueda más a fondo en segundo plano.",
            "nl" => "Zeg het als je wilt dat ik er op de achtergrond grondiger naar zoek.",
            _ => "Tell me if you want me to make it a deeper search in the background.",
        };
    }
    if let Over::Failing(_) | Over::Offline(_) = over {
        // what failed is outside: later, not « another way »
        return match lang {
            "fr" => "Tu veux que je réessaie plus tard ?",
            "de" => "Soll ich es später noch einmal versuchen?",
            "es" => "¿Quieres que lo intente más tarde?",
            "nl" => "Wil je dat ik het later opnieuw probeer?",
            _ => "Do you want me to try again later?",
        };
    }
    if repeated_lookup(over, lang).is_some() {
        return match lang {
            "fr" => "Dis-moi si tu veux que je réessaie autrement.",
            "de" => "Sag mir, ob ich es anders versuchen soll.",
            "es" => "Dime si quieres que lo intente de otra manera.",
            _ => "Tell me if you want me to try another way.",
        };
    }
    match over {
        Over::TooMany => match lang {
            "fr" => "Dis-moi si tu veux que je continue, ou que j'en fasse une tâche de fond.",
            "de" => "Sag mir, ob ich weitermachen oder eine Hintergrundaufgabe daraus machen soll.",
            "es" => "Dime si quieres que siga, o que la convierta en una tarea en segundo plano.",
            _ => "Tell me if you want me to go on, or to make it a background task.",
        },
        Over::PageReads => match lang {
            "fr" => "Dis-moi si tu veux que je réessaie, ou donne-moi un autre lien.",
            "de" => "Sag mir, ob ich es noch einmal versuchen soll, oder gib mir einen anderen Link.",
            "es" => "Dime si quieres que lo intente de nuevo, o dame otro enlace.",
            _ => "Tell me if you want me to try again, or give me another link.",
        },
        _ => match lang {
            "fr" => "Dis-moi si tu veux que je réessaie.",
            "de" => "Sag mir, ob ich es noch einmal versuchen soll.",
            "es" => "Dime si quieres que lo intente de nuevo.",
            _ => "Tell me if you want me to try again.",
        },
    }
}

/// The line the person reads in the conversation, in the agent's language (fr, en, de, es; else English).
pub fn notice_text(over: &Over, lang: &str) -> String {
    format!("{} {}", notice_head(over, lang), notice_tail(over, lang))
}

/// The rescue turn's input starts with this (the apps hide the row, but for its plain line: HermesKit StoppedTurn,
/// Android Hermes.kt); `person_request` skips it (`[bipagents:`).
pub const RESCUE_MARK: &str = "[bipagents:loop_rescue]";

/// A rescue turn at its limits (runs.rs `stop_looping`), told once instead of being stopped: a turn stopped says nothing.
pub const RESCUE_ANSWER_NOW: &str = "[bipagents:brake] A note from the app, not from your person: stop calling tools \
now. Answer your person in their language, in a few plain sentences, with what you already have: what you tried, what \
blocked you, and one or two ways forward.";

/// The tool a rescue turn may not call again (bare name): the one the turn kept calling.
pub fn stopped_tool(over: &Over) -> Option<String> {
    match over {
        Over::SameTool(tool) | Over::SameCall(tool) => Some(tool.clone()),
        Over::Discovery => Some("tool_search".into()),
        Over::Searches => Some("web_search".into()),
        Over::Failing(tool) | Over::Offline(tool) if !tool.is_empty() => Some(tool.clone()),
        _ => None,
    }
}

/// The rescue turn's input (runs.rs `loop_rescue`): the mark, the person's line (in their language: what the apps
/// show of this row), then the agent's instruction, in English, out of the person's sight.
pub fn rescue_input(over: &Over, lang: &str) -> String {
    rescue_input_with(over, lang, &[])
}

/// `rescue_input`, with the words the person added while the stopped turn went on (`added`): taken up right after it
/// as their next message (msg_queue.rs `carry`), so the rescue turn neither answers them nor forgets them.
pub fn rescue_input_with(over: &Over, lang: &str, added: &[String]) -> String {
    let base = rescue_input_alone(over, lang);
    let added: Vec<String> =
        added.iter().map(|w| w.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|w| !w.is_empty()).collect();
    if added.is_empty() {
        return base;
    }
    let quoted = added.iter().map(|w| format!("« {} »", crate::tools_knowledge::head(w, 400))).collect::<Vec<_>>().join(", ");
    format!(
        "{base}\n\nWhile you were working, your person also wrote: {quoted}. It is taken up right after this answer, as \
their next message: don't answer it here, just say in a few words that you'll see to it right after."
    )
}

fn rescue_input_alone(over: &Over, lang: &str) -> String {
    let what = match over {
        Over::SameCall(tool) => match lookup_phrase(tool, "en").filter(|_| lookup_tool(tool, None)) {
            Some(phrase) => format!("repeated {phrase} ({tool}) with the same arguments again and again"),
            None => format!("made exactly the same {tool} call again and again"),
        },
        Over::SameTool(tool) => format!("called {tool} too many times in one turn"),
        Over::TooMany => "made far too many tool calls in one turn".to_string(),
        Over::PageReads => "kept reading the same page in the browser without managing to".to_string(),
        Over::Browser => "made too many browser steps without getting anywhere".to_string(),
        Over::Discovery => "kept searching your tools (tool_search, tool_describe) for one to do it".to_string(),
        Over::Searches => "made far too many web searches in one turn, new words each time, without settling".to_string(),
        Over::Failing(tool) => format!("kept calling {tool} although it failed every time (it looks unavailable now)"),
        Over::Offline(tool) => format!(
            "couldn't reach the internet: {tool} and the calls before it failed on the network again and again, for a while"
        ),
    };
    let dont = match over {
        Over::SameCall(tool) | Over::SameTool(tool) => format!("Don't call {tool} again in this turn."),
        Over::PageReads | Over::Browser => "Don't use the browser again in this turn.".to_string(),
        Over::TooMany => "Don't start new work in this turn.".to_string(),
        Over::Discovery => {
            "Don't search your tools again in this turn: if none you have does it, say so plainly.".to_string()
        }
        Over::Searches => "Don't search the web again in this turn. Give what you found, with what is still \
uncertain, and offer to carry the research on as a background task (propose_task) if it needs more."
            .to_string(),
        Over::Failing(tool) => format!("Don't call {tool} again in this turn, nor work around it with another tool."),
        Over::Offline(_) => "Don't call any tool that needs the internet again in this turn.".to_string(),
    };
    format!(
        "{RESCUE_MARK} {head}\n{RESCUE_THEN}{tail}-->\n\n(A note from the app, not from your person.) You were stopped because you {what}. \
{dont} With what you already have, answer the person in their language: what you were trying to do, what you found \
so far, what blocked you, and propose one or two concrete ways forward (or ask one precise question). Use another \
tool only if it is really needed, two at most.",
        head = notice_head(over, lang),
        tail = notice_tail(over, lang)
    )
}

/// Before the line's second sentence in the rescue turn's input (`notice_tail`): the conversation adds it under the
/// brake's line when the rescue turn said nothing (threads/timeline.rs), as the stream does (`loop_rescue_failed`).
pub const RESCUE_THEN: &str = "<!--bip:then ";

/// For the logs.
pub fn reason(over: &Over) -> String {
    match over {
        Over::SameTool(tool) => format!("same tool too many times ({tool})"),
        Over::TooMany => "too many tool calls".to_string(),
        Over::SameCall(tool) => format!("same call repeated ({tool})"),
        Over::PageReads => "the browser read the page too many times".to_string(),
        Over::Browser => "too many browser calls".to_string(),
        Over::Discovery => "too many tool searches".to_string(),
        Over::Searches => "too many web searches".to_string(),
        Over::Failing(tool) => format!("the same tool failing again and again ({tool})"),
        Over::Offline(tool) => format!("the network unreachable ({tool})"),
    }
}

// -- the bridge's own tools, at its MCP endpoint ---------------------------------------------------------------------

/// The turn a call of the bridge's tools belongs to: the run the hub follows (`RunHub::turn_of`), or none (a routine).
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub run_id: String,
    /// A background task's run (jobs.rs): the task's limits.
    pub background: bool,
    /// A rescue turn's stopped tool (bare name): refused here.
    pub banned: Option<String>,
}

#[derive(Debug, Default)]
struct BridgeTurn {
    writes: u32,
    seen: HashSet<String>,
    /// What each write done in this turn answered (its first text, by signature): the same write asked again gets
    /// it back (a page's MEDIA: line), so the model has what it called for and goes on (10/11, the upgrade journey).
    answers: HashMap<String, String>,
    /// The same write asked again after its « already done »: its calls in all, by signature (the hub stops the run
    /// at `same_call`: `RunHub::same_write_again`).
    again: HashMap<String, u32>,
    /// The same write the tool refused (its signature: how many times, and the refusal's first line): stopped at its
    /// `REFUSED_AGAIN`th ask (`check`).
    refused: HashMap<String, (u32, String)>,
    /// A turn with no run (a routine): all its bridge calls, by tool and in all.
    calls: u32,
    per_tool: HashMap<String, u32>,
    last: f64,
    /// A routine's limit getting close: the note added to the call's result (`take_note`), once per limit.
    note: Option<String>,
    warned: HashSet<String>,
}

/// The very same write the tool refused this many times is refused by the brake at the next ask, and the hub told
/// (`same_write_calls`, `RunHub::same_write_again`). Refusals reach Hermes plain since 14/11 (tool_answers.rs), so
/// neither its breaker nor its failure warning stops a model asking again and again (review 1: 8 identical refused
/// notes, all answered).
pub const REFUSED_AGAIN: u32 = 2;

/// A refusal that invites the same call later (the place full, the browser held, a moment's wait) or once the person
/// has done something in the app (a switch, a share): not counted. The tools mark the latter themselves
/// (`tool_answers::lifted_in_the_app`); these words are the net for one they don't.
fn retry_later(text: &str) -> bool {
    let t = text.to_lowercase();
    ["try again", "again in a", "again later", "a bit later", "in a moment", "in a minute", "of the app", "in the app",
     "in their app", "bipagents app", "the person turns", "the person switches", "they switch", "they can turn",
     "the person connects", "you can allow it"]
        .iter()
        .any(|w| t.contains(w))
}

/// `note` added to a tool's result (its first text, else a text of its own).
pub fn add_note(result: &mut Dict, note: &str) {
    if let Some(Value::Array(content)) = result.get_mut("content") {
        if let Some(Value::String(text)) = content.first_mut().and_then(|c| c.get_mut("text")) {
            text.push_str("\n\n");
            text.push_str(note);
            return;
        }
        content.push(json!({"type": "text", "text": note}));
    }
}

/// Reading one given item (a mail, its attachment, a Carnet or app page): a routine reading 14 different mails is
/// working, not looping (2026-10-28, the replay's `mail_scan_busy_night`), so its limit on one tool counts the same
/// item read again, not reads of different items.
pub fn item_read(tool: &str) -> bool {
    let name = bare(tool);
    !write_tool(name) && (name.ends_with("_read") || name == "mail_attachment" || name == "read_page")
}

/// Each turn's calls of the bridge's tools: by `agent` + run id, or `agent` alone for the turn no run carries.
#[derive(Debug, Default)]
pub struct Turns {
    turns: Mutex<HashMap<String, BridgeTurn>>,
}

pub static TURNS: LazyLock<Turns> = LazyLock::new(Turns::default);

fn turn_key(agent: &str, turn: Option<&Turn>) -> String {
    format!("{agent}\u{1f}{}", turn.map(|t| t.run_id.as_str()).unwrap_or(""))
}

fn signature_of(tool: &str, arguments: &Dict) -> String {
    format!("{}\u{1f}{}", bare(tool), canonical(&Value::Object(arguments.clone())))
}

impl Turns {
    /// Whether this very write (tool and arguments) went through in `turn` (its repeats are answered « already done »,
    /// not refused: the hub stops them later, `RunHub::same_write_again`).
    pub fn already_done(&self, agent: &str, turn: Option<&Turn>, tool: &str, arguments: &Dict) -> bool {
        let turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        turns.get(&turn_key(agent, turn)).is_some_and(|t| t.seen.contains(&signature_of(tool, arguments)))
    }

    /// How many times `turn` has asked for this very write (tool and arguments), once it was refused as already done:
    /// 0 when it never was.
    pub fn same_write_calls(&self, agent: &str, turn: Option<&Turn>, tool: &str, arguments: &Dict) -> u32 {
        let turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        turns
            .get(&turn_key(agent, turn))
            .and_then(|t| t.again.get(&signature_of(tool, arguments)).copied())
            .unwrap_or(0)
    }

    /// A call of the bridge's tool `tool` by `agent` in `turn`: None when it may go on, else the refusal the model
    /// gets.
    pub fn check(
        &self,
        settings: &Settings,
        agent: &str,
        turn: Option<&Turn>,
        tool: &str,
        arguments: &Dict,
        now: f64,
    ) -> Option<Dict> {
        if !settings.enabled {
            return None;
        }
        if let Some(banned) = turn.and_then(|t| t.banned.as_deref()).filter(|b| *b == bare(tool)) {
            tracing::info!(agent = %agent, tool = %tool, "the tool the turn kept calling, in its rescue turn: refused");
            return Some(tool_text(
                &format!(
                    "Refused: you were stopped for calling {banned} again and again, so it isn't available in this \
turn. Answer the person now with what you already have."
                ),
                true,
            ));
        }
        // a background task's run, or a routine (no run followed): scheduled work, the task's limits
        let scheduled = turn.is_none_or(|t| t.background);
        let limits = settings.limits(scheduled);
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        if turns.len() > 2_000 {
            turns.retain(|_, t| now - t.last < 86_400.0);
        }
        let entry = turns.entry(turn_key(agent, turn)).or_default();
        if turn.is_none() && now - entry.last > settings.idle_seconds {
            *entry = BridgeTurn::default();
        }
        entry.last = now;
        if turn.is_none() {
            // a routine: Hermes can't stop it, so its calls of the bridge's tools are held here
            entry.calls += 1;
            let item = item_read(tool);
            let key = if item {
                signature_of(tool, arguments)
            } else if write_call(tool, arguments) && !write_tool(tool) {
                format!("{}\u{1f}ops", bare(tool)) // page_data's writes apart from its readings
            } else {
                bare(tool).to_string()
            };
            // a big job writes with one tool many times (40 pages corrected: the task's limit), but searching or
            // reading the same thing again and again is going round in circles whatever the job (the morning brief's
            // 35 Carnet searches): a conversation's limit for those
            let same_tool = if write_call(tool, arguments) { limits.same_tool } else { settings.conversation.same_tool };
            let same = entry.per_tool.entry(key).or_insert(0);
            *same += 1;
            let (same, calls) = (*same, entry.calls);
            // getting close (about two thirds): told in the call's result, once per limit (a routine can't be steered)
            let close = if same == soft_limit(same_tool) {
                Some((format!("tool:{}", bare(tool)), format!("you have called {} {same} times", bare(tool))))
            } else if calls == soft_limit(settings.routine_tool_calls) {
                Some(("total".to_string(), format!("you have made {calls} calls")))
            } else {
                None
            };
            if let (Some((key, what)), true) = (close, settings.warn) {
                if entry.warned.insert(key) {
                    tracing::info!(agent = %agent, tool = %tool, calls = calls, same = same,
                        "brake_warned: a routine told it is going round in circles");
                    entry.note = Some(format!(
                        "{WARN_MARK} Note from the app: {what} in this run, close to the limit. Change approach, or finish with what you have."
                    ));
                }
            }
            if same > same_tool || calls > settings.routine_tool_calls {
                tracing::info!(agent = %agent, tool = %tool, calls = calls, same = same,
                    "going round in circles in a turn no run carries: tool refused");
                return Some(tool_text(
                    &format!(
                        "Refused: {} in this run already, too many. Stop calling tools now: answer the person with \
what you have, and say plainly what you couldn't find or do (no limits or tools named).",
                        if same > same_tool {
                            format!("you called {} {} times", bare(tool), same - 1)
                        } else {
                            format!("you made {} calls", calls - 1)
                        }
                    ),
                    true,
                ));
            }
        }
        if !write_call(tool, arguments) {
            return None;
        }
        // the very same write the tool refused, asked again and again: stopped here, a routine's too (the refusal
        // would be the same; Hermes doesn't stop it any more)
        if let Some((times, why)) = entry.refused.get(&signature_of(tool, arguments)).filter(|(n, _)| *n >= REFUSED_AGAIN).cloned() {
            let calls = entry.again.entry(signature_of(tool, arguments)).or_insert(times);
            *calls += 1;
            tracing::info!(agent = %agent, tool = %tool, refused = times, "the same refused write again: stopped");
            return Some(tool_text(
                &format!(
                    "Not done: you asked for exactly this {times} times and it was refused each time (« {why} »). \
Don't ask for it again: tell your person in plain words what stops it, and go on without it."
                ),
                true,
            ));
        }
        // Only within a followed run: without one, the « turn » is a guess, and the person may well have asked for
        // the same thing again since (« refais le podcast »).
        if turn.is_some() && bare(tool) != WRITES_WITH_OPS && entry.seen.contains(&signature_of(tool, arguments)) {
            let calls = entry.again.entry(signature_of(tool, arguments)).or_insert(1);
            *calls += 1;
            tracing::info!(agent = %agent, tool = %tool, calls = *calls, "the same write again in this turn: refused");
            // a file given (27/11, Alex's aibox: `give_file` 3 times, stopped): « not done again » read as a failure,
            // « call that one now » as an invitation; the file is given, the reply is what is left to do
            if let (true, Some(answer)) = (bare(tool) == crate::give_file::TOOL_NAME, entry.answers.get(&signature_of(tool, arguments))) {
                return Some(tool_text(&given_again(answer), false));
            }
            // done: what it answered then, and what to do now (27/11: a note written, then « asked again to check »
            // until the brake stopped the turn; « not done again » read as a failure)
            let name = bare(tool);
            let done = format!(
                "Already done: this exact {name} call worked earlier in this turn, nothing failed, and doing it again \
changes nothing. Don't call {name} again for it: tell your person it is done."
            );
            return Some(tool_text(
                &match entry.answers.get(&signature_of(tool, arguments)) {
                    Some(answer) => format!("{done} What it answered then:\n{answer}"),
                    None => done,
                },
                false,
            ));
        }
        if entry.writes >= limits.writes && !proposing_tool(tool) {
            tracing::info!(agent = %agent, tool = %tool, writes = entry.writes, scheduled = scheduled,
                "too many writes in this turn: refused");
            return Some(tool_text(&writes_refused(entry.writes, scheduled), true));
        }
        None
    }

    /// What `tool` answered `agent` when it worked in the turn of run `run_id` (its first such answer).
    pub fn answer_of(&self, agent: &str, run_id: &str, tool: &str) -> Option<String> {
        let turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let turn = Turn { run_id: run_id.to_string(), background: false, banned: None };
        let prefix = format!("{}\u{1f}", bare(tool));
        turns.get(&turn_key(agent, Some(&turn)))?.answers.iter().find(|(sig, _)| sig.starts_with(&prefix)).map(|(_, a)| a.clone())
    }

    /// The note a routine's call gets with its result (its limits getting close), once.
    pub fn take_note(&self, agent: &str, turn: Option<&Turn>) -> Option<String> {
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        turns.get_mut(&turn_key(agent, turn)).and_then(|t| t.note.take())
    }

    /// A write tool's call that `check` let through went well: counted in its turn (a refused or failed one is not:
    /// the model may correct its arguments and call again).
    pub fn record(&self, agent: &str, turn: Option<&Turn>, tool: &str, arguments: &Dict) {
        if !write_call(tool, arguments) {
            return;
        }
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let entry = turns.entry(turn_key(agent, turn)).or_default();
        if !proposing_tool(tool) {
            entry.writes += 1;
        }
        entry.seen.insert(signature_of(tool, arguments));
    }

    /// A write the tool refused (its result's `isError`, before tool_answers.rs sends it plain): counted for `check`,
    /// unless it invites the same call later or was a real failure of the bridge (its own « don't retry »).
    pub fn record_refusal(&self, agent: &str, turn: Option<&Turn>, tool: &str, arguments: &Dict, result: &Dict) {
        if !write_tool(tool)
            || result.contains_key(crate::tool_answers::FAILURE_MARK)
            || result.contains_key(crate::tool_answers::LIFTED_MARK)
        {
            return;
        }
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| c.iter().find_map(|b| b.get("text").and_then(Value::as_str)))
            .unwrap_or("");
        if retry_later(text) {
            return;
        }
        let why = crate::tools_knowledge::head(text.lines().next().unwrap_or("").trim(), 200).to_string();
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let entry = turns.entry(turn_key(agent, turn)).or_default();
        // the very same refusal again: another one (the person's settings changed meanwhile) starts the count over
        let refused = entry.refused.entry(signature_of(tool, arguments)).or_insert((0, why.clone()));
        if refused.1 != why {
            *refused = (0, why);
        }
        refused.0 += 1;
    }

    /// [`record`](Self::record), with what the write answered: kept for the same write asked again (`check`). A page
    /// deleted in the turn forgets the calls that made it: making it again is new work, not a loop (10/11, the
    /// upgrade journey: GLM made two pages, deleted them, asked for them again and was told « already done » until
    /// the brake stopped the routine).
    pub fn record_answer(&self, agent: &str, turn: Option<&Turn>, tool: &str, arguments: &Dict, result: &Dict) {
        if !write_call(tool, arguments) {
            return;
        }
        self.record(agent, turn, tool, arguments);
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let entry = turns.entry(turn_key(agent, turn)).or_default();
        if bare(tool) == "delete_page" {
            let page = arguments.get("page").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if page.is_empty() {
                return;
            }
            let made = format!("/{page}.html");
            let gone: Vec<String> = entry
                .answers
                .iter()
                .filter(|(sig, answer)| sig.starts_with("create_page\u{1f}") && answer.contains(&made))
                .map(|(sig, _)| sig.clone())
                .collect();
            for sig in gone {
                entry.answers.remove(&sig);
                entry.seen.remove(&sig);
                entry.again.remove(&sig);
            }
            // the delete itself may come again later for a page made again
            entry.seen.remove(&signature_of(tool, arguments));
            return;
        }
        if !text.is_empty() {
            entry.answers.insert(signature_of(tool, arguments), crate::tools_knowledge::head(&text, 1500).to_string());
        }
    }
}

/// What `give_file` asked again for the same file in a turn answers: given already, and what to put in the reply.
pub fn given_again(answer: &str) -> String {
    format!(
        "Already given: this file is your person's already, nothing failed. Don't call give_file again for it: write \
your reply now, with what give_file answered you:\n{answer}"
    )
}

/// What the stopped turn's tool answered when it worked (its first answer, by `Turns::record_answer`), for its rescue
/// turn (runs.rs `loop_rescue`): a file given or a page made the person still needs from its answer.
pub fn rescue_answer_note(tool: &str, answer: &str) -> String {
    format!(
        "{tool} worked the first time in that turn; what it answered you is still good (put it in your answer when it \
is for your person, a MEDIA: line alone on its line):\n{answer}"
    )
}

/// What a write past the turn's writes gets. The model words its message from it, so it carries what the person must
/// hear in plain words: why it stopped (a cap on changes made at once, so that a mistake can't spread everywhere), how
/// many it changed, what is left, and what happens next; never « the bridge », a « turn » or a tool's name (2026-10-31:
/// « Bloqué par la limite du Carnet : 10 modifications par tour maximum — le bridge a refusé la suite »). `scheduled`: a
/// routine or a background task, whose person is not there to answer: it ends with its report, the rest for next time.
pub fn writes_refused(writes: u32, scheduled: bool) -> String {
    let why = "you make at most this many changes at once, so that a mistake can never spread to everything";
    let words = "Say it in plain words, in the person's language, with the reason; never mention the bridge, \
a turn, a tool's name or anything technical";
    if scheduled {
        format!(
            "Not done: this run has made {writes} changes, the most it makes at once ({why}). Make no more changes \
now. End with your report to the person: that you stopped at {writes} changes because of this cap and why, what is \
still left (how many, if you know: « {writes} sur N »), and that the rest is done at the next run if this task runs \
again, else that they can ask you to go on. {words}, e.g. « J'ai une limite sur le nombre de changements d'un coup, pour éviter de tout casser : \
j'ai corrigé {writes} notes sur N, je reprends au prochain passage. »"
        )
    } else {
        format!(
            "Not done: you have made {writes} changes, the most you make at once ({why}). Stop here: tell the person \
that you stopped because of this cap and why, what you changed and what is left, and ask whether you should go on. \
{words}, e.g. « J'ai une limite sur le nombre de changements d'un coup, pour éviter de tout casser : j'ai corrigé \
{writes} notes sur N. Dis-moi si je continue. »"
        )
    }
}

/// The run event's tool name (Hermes writes `tool`, `tool_name` or `name`).
pub fn event_tool(data: &serde_json::Map<String, Value>) -> String {
    ["tool", "tool_name", "name"]
        .iter()
        .filter_map(|k| data.get(*k).and_then(Value::as_str))
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_string()
}

/// The run event's call signature: its arguments when it has them, else its preview.
pub fn event_signature(data: &serde_json::Map<String, Value>) -> Option<String> {
    match data.get("args").or_else(|| data.get("arguments")) {
        Some(args @ Value::Object(map)) if !map.is_empty() => return Some(canonical(args)),
        Some(Value::String(text)) if !text.trim().is_empty() => return Some(text.clone()),
        _ => {}
    }
    data.get("preview").and_then(Value::as_str).filter(|p| !p.trim().is_empty()).map(str::to_string)
}

/// The notice event's data (`bridge.notice`, code `loop_stopped`). `rescue`: a rescue turn follows (runs.rs
/// `loop_rescue`), so only the line's first sentence (the agent's own reply says the rest).
pub fn notice(over: &Over, lang: &str, rescue: bool) -> serde_json::Map<String, Value> {
    let mut data = serde_json::Map::new();
    data.insert("type".into(), json!(crate::runs::NOTICE));
    data.insert("code".into(), json!("loop_stopped"));
    data.insert("reason".into(), json!(reason(over)));
    let text = if rescue { notice_head(over, lang) } else { notice_text(over, lang) };
    data.insert("text".into(), json!(text));
    if rescue {
        data.insert("rescue".into(), json!(true));
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, &str)]) -> Dict {
        pairs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()
    }

    /// Review 1 (14/11): refusals reach Hermes plain now, so neither its breaker nor its failure warning stops a model
    /// asking for the very same refused write again and again: the brake does, at the third ask (a routine's too).
    #[test]
    fn the_same_refused_write_is_stopped_at_its_third_ask() {
        let turns = Turns::default();
        let settings = Settings::default();
        let note = args(&[("page", "Camille Testeur"), ("facts", "Passeport : 12AB34567")]);
        let refused = tool_text("Not saved: ID numbers are never kept, and fact 1 holds one.", true);
        let said = |r: &Dict| r["content"][0]["text"].as_str().unwrap_or("").to_string();
        for (agent, turn) in [("routine", None), ("conversation", Some(Turn { run_id: "run_r".into(), background: false, banned: None }))] {
            for _ in 0..2 {
                assert!(turns.check(&settings, agent, turn.as_ref(), "notebook_write", &note, 0.0).is_none());
                turns.record_refusal(agent, turn.as_ref(), "notebook_write", &note, &refused);
            }
            let third = turns.check(&settings, agent, turn.as_ref(), "mcp__bipagents__notebook_write", &note, 1.0);
            let third = third.expect("the third identical refused write is stopped");
            assert!(said(&third).starts_with("Not done: you asked for exactly this 2 times") && said(&third).contains("ID numbers"), "{}", said(&third));
            assert_eq!(turns.same_write_calls(agent, turn.as_ref(), "notebook_write", &note), 3, "the hub stops the run at same_call");
            // another write goes on
            let other = args(&[("page", "Camille Testeur"), ("facts", "Aime le jazz")]);
            assert!(turns.check(&settings, agent, turn.as_ref(), "notebook_write", &other, 1.0).is_none());
        }
        // a different refusal for the same write (the person's settings changed meanwhile) starts the count over
        let first = tool_text("Not done: The person hasn't shared their reminders with you.", true);
        let then = tool_text("Not added: a reminder at a time isn't set on any phone.", true);
        let call = args(&[("title", "Appeler Paul"), ("due", "2026-11-07T14:00")]);
        for refusal in [&first, &then] {
            assert!(turns.check(&settings, "settings", None, "reminders_add", &call, 3.0).is_none());
            turns.record_refusal("settings", None, "reminders_add", &call, refusal);
        }
        assert!(turns.check(&settings, "settings", None, "reminders_add", &call, 3.0).is_none());
        // a refusal meant to be tried again later (the place full, the browser held) is not counted
        let later = tool_text("Not done: your place is full right now; try again later.", true);
        let page = args(&[("title", "Menu")]);
        for _ in 0..4 {
            assert!(turns.check(&settings, "later", None, "create_page", &page, 2.0).is_none());
            turns.record_refusal("later", None, "create_page", &page, &later);
        }
    }

    /// Review 2 (14/11) — d00246c8d: « a refusal sending the person to the app is not counted » (`retry_later`: « of the app », « in the
    /// app », « the person turns »…). The write tools' real refusals of that kind say « in the BipAgents app », « of the
    /// BipAgents app », « They can turn on », « they switch »: counted, so once the person has done it, the third identical
    /// call is refused by the brake (« you asked for exactly this 2 times »), a routine's and a conversation's alike.
    #[test]
    fn a_refusal_the_person_lifts_in_the_app_is_never_counted() {
        let lifted_in_the_app = [
            // reminders.rs (reminders_add, the person hasn't shared their reminders)
            ("reminders_add", "The person hasn't shared their reminders with you, so nothing was done. They can turn on the \
    « Rappels » tool on your page in the BipAgents app: tell them so in one sentence, without steps for a particular phone."),
            // calendar.rs (calendar_add, the phone's calendar not shared)
            ("calendar_add", "The person hasn't shared their phone calendar with you: they switch « Calendrier » on in your \
    page of the BipAgents app."),
            // contacts.rs (contacts_add, the address book not shared)
            ("contacts_add", "The person hasn't shared their address book with you: they switch « Contacts » on in your page \
    of the BipAgents app."),
            // mailbox.rs (mail_draft, no mailbox connected)
            ("mail_draft", "No mailbox is connected for you: the person connects one in the BipAgents app (your settings › \
    Boîtes mail)."),
        ];
        let turns = Turns::default();
        let settings = Settings::default();
        let mut blocked = Vec::new();
        for (tool, text) in lifted_in_the_app {
            let call = args(&[("title", "Appeler Paul"), ("due", "2026-11-20T14:00")]);
            let refused = tool_text(text, true);
            for _ in 0..2 {
                assert!(turns.check(&settings, "review2", None, tool, &call, 0.0).is_none());
                turns.record_refusal("review2", None, tool, &call, &refused);
            }
            // the person turned it on in the app meanwhile: the same call again must reach the tool
            if let Some(stopped) = turns.check(&settings, "review2", None, tool, &call, 1.0) {
                blocked.push((tool, stopped["content"][0]["text"].as_str().unwrap_or("").to_string()));
            }
        }
        assert!(blocked.is_empty(), "refused by the brake after the person lifted it in the app: {blocked:#?}");
    }

        /// The tools mark these refusals themselves (`tool_answers::lifted_in_the_app`): never counted, whatever their words.
        #[test]
        fn a_refusal_its_tool_marks_as_lifted_in_the_app_is_never_counted() {
            let turns = Turns::default();
            let settings = Settings::default();
            let call = args(&[("title", "Appeler Paul")]);
            let marked = crate::tool_answers::lifted_in_the_app(tool_text("Not done: not shared yet.", true));
            for _ in 0..3 {
                assert!(turns.check(&settings, "marked", None, "reminders_add", &call, 0.0).is_none());
                turns.record_refusal("marked", None, "reminders_add", &call, &marked);
            }
            assert!(turns.check(&settings, "marked", None, "reminders_add", &call, 0.0).is_none());
        }

    #[test]
    fn the_lab_loop_is_stopped_at_the_eleventh_add() {
        // 2026-10-27: calendar_add and calendar_list in turns, the adds' arguments unknown (no preview)
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let mut stopped = None;
        for i in 0..300 {
            let tool = if i % 2 == 0 { "mcp__bipagents__calendar_add" } else { "mcp__bipagents__calendar_list" };
            if let Some(over) = turn.note(tool, None, false, &limits) {
                stopped = Some((i, over));
                break;
            }
        }
        assert_eq!(stopped, Some((20, Over::SameTool("calendar_add".into()))));
    }

    #[test]
    fn a_page_s_data_written_by_its_agent_counts_as_a_write_and_read_as_a_lookup() {
        // 16/11 (page_store.rs, the agent's writing): `page_data` with `ops` writes its page's store
        let ops = |n: i64| json!({"page": "jardin", "ops": [{"op": "set", "key": "temp", "value": n}]}).as_object().cloned().unwrap();
        let read = json!({"page": "jardin"}).as_object().cloned().unwrap();
        assert!(write_call("mcp_bipagents_page_data", &ops(1)) && write_call("page_data", &ops(1)));
        assert!(!write_call("page_data", &read) && !write_call("page_data", &json!({"ops": null}).as_object().cloned().unwrap()));
        assert!(lookup_tool("page_data", Some(r#"{"page":"jardin"}"#)));
        assert!(!lookup_tool("page_data", Some(r#"{"ops":[{"op":"set"}],"page":"jardin"}"#)));
        // a conversation's turn: its writes counted and held at `writes`; its readings never
        let turns = Turns::default();
        let settings = Settings::default();
        let turn = Some(Turn { run_id: "run_jardin".into(), background: false, banned: None });
        for n in 0..settings.conversation.writes as i64 {
            assert!(turns.check(&settings, "vie", turn.as_ref(), "page_data", &ops(n), n as f64).is_none(), "{n}");
            turns.record_answer("vie", turn.as_ref(), "page_data", &ops(n), &tool_text("Written in page jardin.", false));
        }
        let held = turns.check(&settings, "vie", turn.as_ref(), "page_data", &ops(99), 99.0).expect("held");
        assert_eq!(held["isError"], json!(true));
        assert!(turns.check(&settings, "vie", turn.as_ref(), "page_data", &read, 100.0).is_none());
        // the same write again in the turn: no « already done » for a store (a value set back is a write), still held
        let again = turns.check(&settings, "vie", turn.as_ref(), "page_data", &ops(0), 101.0).expect("held");
        assert!(!again["content"][0]["text"].as_str().unwrap().starts_with("Already done"));
        assert_eq!(again["isError"], json!(true));
    }

    #[test]
    fn the_same_page_asked_again_gets_its_link_and_a_deleted_one_may_be_made_again() {
        // 10/11, the upgrade journey: GLM made its pages, deleted them, asked for them again, and was told « already
        // done » (no link) until the brake stopped the routine
        let turns = Turns::default();
        let settings = Settings::default();
        let turn = Some(Turn { run_id: "run_r".into(), background: true, banned: None });
        let page = json!({"title": "Point du jour", "html": "<p>Soleil</p>"}).as_object().cloned().unwrap();
        let made = tool_text("MEDIA:/bipagents/pages/daily/point-du-jour.html\nPage ready: id point-du-jour, version 1.", false);
        assert!(turns.check(&settings, "daily", turn.as_ref(), "create_page", &page, 1.0).is_none());
        turns.record_answer("daily", turn.as_ref(), "mcp__bipagents__create_page", &page, &made);
        let again = turns.check(&settings, "daily", turn.as_ref(), "create_page", &page, 2.0).expect("not made twice");
        let text = again["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Already done") && text.contains("MEDIA:/bipagents/pages/daily/point-du-jour.html"), "{text}");
        assert_eq!(again["isError"], json!(false));
        // deleted in the turn: making it again is new work
        let gone = json!({"page": "point-du-jour"}).as_object().cloned().unwrap();
        assert!(turns.check(&settings, "daily", turn.as_ref(), "delete_page", &gone, 3.0).is_none());
        turns.record_answer("daily", turn.as_ref(), "delete_page", &gone, &tool_text("Page point-du-jour deleted.", false));
        assert!(turns.check(&settings, "daily", turn.as_ref(), "create_page", &page, 4.0).is_none());
        assert_eq!(turns.same_write_calls("daily", turn.as_ref(), "create_page", &page), 0);
        turns.record_answer("daily", turn.as_ref(), "create_page", &page, &made);
        // and deleted again later in the same turn: allowed too
        assert!(turns.check(&settings, "daily", turn.as_ref(), "delete_page", &gone, 5.0).is_none());
    }

    /// 27/11, Alex's aibox: `same call repeated (give_file)` 5 times in a week (Vie, Bip). Asked again for the same file,
    /// the bridge answered « Already done … not done again. If you meant another tool, call that one now. », read as a
    /// failure and an invitation: the model asked again, and the brake stopped the turn; its rescue turn, told not to
    /// call give_file, had no file to give. Now: given already, what to put in the reply; the rescue gets the answer.
    #[test]
    fn a_file_given_twice_is_said_given_and_its_rescue_has_it() {
        let turns = Turns::default();
        let settings = Settings::default();
        let turn = Some(Turn { run_id: "run_g".into(), background: false, banned: None });
        let file = json!({"path": "/envoi/devis.pdf"}).as_object().cloned().unwrap();
        let given = tool_text("MEDIA:/envoi/devis.pdf\nPut the line above alone on its own line in your reply.", false);
        assert!(turns.check(&settings, "vie", turn.as_ref(), "mcp__bipagents__give_file", &file, 1.0).is_none());
        turns.record_answer("vie", turn.as_ref(), "mcp__bipagents__give_file", &file, &given);
        let again = turns.check(&settings, "vie", turn.as_ref(), "give_file", &file, 2.0).expect("given once");
        let said = again["content"][0]["text"].as_str().unwrap();
        assert!(said.starts_with("Already given") && said.contains("nothing failed"), "{said}");
        assert!(said.contains("Don't call give_file again") && said.contains("MEDIA:/envoi/devis.pdf"), "{said}");
        assert!(!said.contains("not done") && !said.contains("call that one now"), "{said}");
        assert_eq!(again["isError"], json!(false));
        // the hub still stops a third ask (`same_call`)
        assert_eq!(turns.same_write_calls("vie", turn.as_ref(), "give_file", &file), 2);
        // its rescue turn gets the first answer
        let answer = turns.answer_of("vie", "run_g", "mcp__bipagents__give_file").expect("kept");
        assert!(answer.starts_with("MEDIA:/envoi/devis.pdf"));
        assert!(rescue_answer_note("give_file", &answer).contains("MEDIA:/envoi/devis.pdf"));
        assert_eq!(turns.answer_of("vie", "run_other", "give_file"), None);
    }

    #[test]
    fn the_same_write_asked_again_is_counted_for_the_hub() {
        let turns = Turns::default();
        let settings = Settings::default();
        let turn = Turn { run_id: "run_1".into(), background: true, banned: None };
        let args = json!({"title": "Prononcer", "html": "<p>x</p>"}).as_object().cloned().unwrap();
        let other = json!({"title": "Autre", "html": "<p>y</p>"}).as_object().cloned().unwrap();
        assert_eq!(turns.check(&settings, "vie", Some(&turn), "mcp__bipagents__create_page", &args, 1.0), None);
        turns.record("vie", Some(&turn), "mcp__bipagents__create_page", &args);
        assert_eq!(turns.same_write_calls("vie", Some(&turn), "create_page", &args), 0, "done once, never refused");
        for calls in 2..=4u32 {
            assert!(turns.check(&settings, "vie", Some(&turn), "mcp__bipagents__create_page", &args, 2.0).is_some());
            assert_eq!(turns.same_write_calls("vie", Some(&turn), "mcp__bipagents__create_page", &args), calls);
        }
        assert_eq!(turns.same_write_calls("vie", Some(&turn), "create_page", &other), 0);
        let next = Turn { run_id: "run_2".into(), ..turn.clone() };
        assert_eq!(turns.same_write_calls("vie", Some(&next), "create_page", &args), 0, "per run");
    }

    #[test]
    fn the_same_call_three_times_stops() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let sig = canonical(&json!({"title": "Test", "start": "2026-10-28T10:00"}));
        let other = canonical(&json!({"start": "2026-10-28T10:00", "title": "Test"}));
        assert_eq!(sig, other, "keys sorted");
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(&sig), false, &limits), None);
        assert_eq!(turn.note("skill_view", Some("visible-browser"), false, &limits), None);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(&sig), false, &limits), None);
        assert_eq!(
            turn.note("mcp_bipagents_calendar_add", Some(&sig), false, &limits),
            Some(Over::SameCall("calendar_add".into()))
        );
    }

    /// Real-day bench 2026-10-28: « ask Coach » with no message_agent in view, 9 different tool_search and 2
    /// tool_describe in 3 minutes, nothing held it.
    #[test]
    fn searching_its_tools_again_and_again_is_braked() {
        let limits = Settings::default().conversation;
        // the same query (other options, other spacing): told at 3, stopped at 5
        let mut turn = TurnCount::default();
        let same = |n: u32| canonical(&json!({"query": "message  Agent", "limit": n}));
        for n in 1..=4u32 {
            assert_eq!(turn.note("tool_search", Some(&same(n)), false, &limits), None, "{n}");
            let warning = turn.take_warning();
            assert_eq!(warning.is_some(), n == 3, "{n}: {warning:?}");
            if let Some(w) = warning {
                assert_eq!(w, Warning::DiscoverySame(3, 5));
                assert!(w.text().contains("say so to the person"));
            }
        }
        assert_eq!(turn.note("tool_search", Some(&same(5)), false, &limits), Some(Over::Discovery));
        // different queries: past 8 discovery calls in all, tool_describe included
        let mut turn = TurnCount::default();
        let mut stopped = None;
        for n in 0..11u32 {
            let (tool, args) = if n % 5 == 4 { ("tool_describe", json!({"name": format!("t{n}")})) } else {
                ("tool_search", json!({"query": format!("agent {n}")}))
            };
            if let Some(over) = turn.note(tool, Some(&canonical(&args)), false, &limits) {
                stopped = Some((n, over));
                break;
            }
        }
        assert_eq!(stopped, Some((8, Over::Discovery)));
        assert_eq!(stopped_tool(&Over::Discovery).as_deref(), Some("tool_search"));
        assert_eq!(
            notice_text(&Over::Discovery, "fr"),
            "Je me suis arrêté : je cherchais sans fin un outil pour le faire. Dis-moi si tu veux que je réessaie."
        );
        assert!(rescue_input(&Over::Discovery, "fr").contains("Don't search your tools again"));
        // a normal turn: two searches, then the tool found is called
        let mut turn = TurnCount::default();
        for (tool, args) in [
            ("tool_search", json!({"query": "mail"})),
            ("tool_describe", json!({"name": "mcp__bipagents__mail_read"})),
            ("tool_call", json!({"name": "mcp__bipagents__mail_read", "arguments": {"id": 1}})),
            ("tool_call", json!({"name": "mcp__bipagents__mail_read", "arguments": {"id": 2}})),
        ] {
            assert_eq!(turn.note(tool, Some(&canonical(&args)), false, &limits), None);
            assert_eq!(turn.take_warning(), None);
        }
        assert_eq!(discovery_query("{\"query\": \"Mail  READ\", \"limit\": 3}"), "mail read");
        assert_eq!(discovery_query("44136fa355b3"), "44136fa355b3");
    }

    #[test]
    fn card_proposals_for_many_senders_are_not_a_loop() {
        // 5/11, token lab: mail_bulk once per sender, fourteen at once, stopped at the eleventh
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        for i in 0..20 {
            let args = format!("{{\"action\":\"archive\",\"domain\":\"d{i}.example\"}}");
            assert_eq!(turn.note("mcp__bipagents__mail_bulk", Some(&args), false, &limits), None, "{i}");
        }
        // the very same proposal again and again still is
        let mut turn = TurnCount::default();
        let same = "{\"action\":\"archive\",\"domain\":\"a.example\"}";
        let stopped = (0..10).find_map(|_| turn.note("mcp__bipagents__mail_bulk", Some(same), false, &limits));
        assert_eq!(stopped, Some(Over::SameCall("mail_bulk".into())));
        // other writes are still held to same_tool
        let mut turn = TurnCount::default();
        let stopped = (0..20).find_map(|i| turn.note("mcp__bipagents__calendar_add", Some(&format!("{i}")), false, &limits));
        assert_eq!(stopped, Some(Over::SameTool("calendar_add".into())));
    }

    #[test]
    fn work_tools_and_the_browser_only_count_for_the_total() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        for i in 0..29 {
            assert_eq!(turn.note("terminal", Some("cargo test"), false, &limits), None, "{i}");
            assert_eq!(turn.note("mcp__stealth__click_element", Some("{\"x\":1}"), true, &limits), None, "{i}");
        }
        assert_eq!(turn.total, 58);
        // the same web search: a lookup, held to same_lookup (5)
        let mut turn = TurnCount::default();
        for _ in 0..4 {
            assert_eq!(turn.note("web_search", Some("tour Eiffel"), false, &limits), None);
        }
        assert_eq!(turn.note("web_search", Some("tour Eiffel"), false, &limits), Some(Over::SameCall("web_search".into())));
        let mut turn = TurnCount::default();
        let stopped = (0..100).find_map(|i| turn.note("terminal", Some(&format!("ls {i}")), false, &limits));
        assert_eq!(stopped, Some(Over::TooMany));
        assert_eq!(turn.total, 61);
    }

    #[test]
    fn a_refused_write_says_why_how_many_and_what_next_in_plain_words() {
        // 2026-10-31, Alex: « Bloqué par la limite du Carnet : 10 modifications par tour maximum — le bridge a refusé
        // la suite ». The model words its message from this text: the reason, the count, what is left, what next.
        for (scheduled, next) in [(true, "je reprends au prochain passage"), (false, "Dis-moi si je continue")] {
            let text = writes_refused(40, scheduled);
            assert!(text.contains("40 changes") && text.contains("a mistake can never spread to everything"), "{text}");
            assert!(text.contains("pour éviter de tout casser") && text.contains("40 notes sur N") && text.contains(next));
            assert!(text.contains("what is") && text.contains("left"), "{text}");
            // nothing for the person to repeat that only the app's people understand
            let example = text.rsplit('«').next().unwrap();
            for jargon in ["bridge", "tour", "turn", "notebook_", "limite du Carnet"] {
                assert!(!example.contains(jargon), "{jargon} in the example: {example}");
            }
            assert!(!text.contains("Refused") && !text.contains("this turn"), "{text}");
        }
    }

    #[test]
    fn background_tasks_go_further() {
        let settings = Settings::default();
        let mut turn = TurnCount::default();
        let limits = settings.limits(true);
        for _ in 0..40 {
            assert_eq!(turn.note("mcp_bipagents_mail_read", None, false, &limits), None);
        }
        assert_eq!(turn.note("mcp_bipagents_mail_read", None, false, &limits), Some(Over::SameTool("mail_read".into())));
    }

    #[test]
    fn settings_from_the_config() {
        let s = Settings::from_table(&json!({"same_tool": 5, "task_writes": 0, "enabled": false, "idle_seconds": 1}));
        assert!(!s.enabled);
        assert_eq!(s.conversation.same_tool, 5);
        assert_eq!(s.task.writes, 1, "clamped");
        assert_eq!(s.idle_seconds, 10.0);
        assert_eq!(Settings::from_table(&Value::Null), Settings::default());
        let s = Settings::from_table(&json!({"page_reads": 8, "task_browser_calls": 60, "routine_tool_calls": 12}));
        assert_eq!((s.conversation.page_reads, s.task.browser_calls, s.routine_tool_calls), (8, 60, 12));
        assert_eq!(s.conversation.browser_calls, 40);
        // under Hermes' cap of 150 (tool_diet.rs): the brake speaks first
        let d = Settings::default();
        assert!(d.conversation.tool_calls + d.conversation.browser_calls < 150 && d.task.tool_calls < 150);
    }

    #[test]
    fn plain_words_in_four_languages() {
        let looping = Over::SameTool("calendar_add".into());
        assert_eq!(
            notice_text(&looping, "fr"),
            "Je me suis arrêté : je tournais en rond sur l'agenda. Dis-moi si tu veux que je réessaie."
        );
        assert!(notice_text(&looping, "en").contains("the calendar"));
        assert!(notice_text(&looping, "de-DE").contains("Kalender"));
        assert!(notice_text(&looping, "es").contains("la agenda"));
        assert!(notice_text(&Over::SameCall("memory".into()), "fr").contains("la même étape"));
        assert!(notice_text(&Over::TooMany, "it").starts_with("I stopped"));
    }

    #[test]
    fn writes_capped_and_the_same_write_refused() {
        let turns = Turns::default();
        let settings = Settings::default();
        let run = |id: &str| Some(Turn { run_id: id.to_string(), background: false, banned: None });
        let add = |n: usize| args(&[("title", &format!("Event {n}")), ("start", "2026-10-28")]);
        // a call let through and done (as the MCP route does)
        let call = |settings: &Settings, agent: &str, turn: Option<Turn>, tool: &str, a: &Dict, now: f64| {
            let refused = turns.check(settings, agent, turn.as_ref(), tool, a, now);
            if refused.is_none() {
                turns.record(agent, turn.as_ref(), tool, a);
            }
            refused
        };
        assert!(call(&settings, "coach", run("run_1"), "calendar_list", &Dict::new(), 0.0).is_none());
        assert!(call(&settings, "coach", run("run_1"), "calendar_add", &add(0), 0.0).is_none());
        let again = call(&settings, "coach", run("run_1"), "mcp__bipagents__calendar_add", &add(0), 1.0).unwrap();
        assert_eq!(again["isError"], json!(false));
        // a call checked but failed (not recorded) may come again
        assert!(turns.check(&settings, "coach", run("run_1").as_ref(), "calendar_add", &add(50), 1.0).is_none());
        for n in 1..10 {
            assert!(call(&settings, "coach", run("run_1"), "calendar_add", &add(n), 2.0).is_none(), "{n}");
        }
        // a card proposal (mail_bulk) past the writes: not refused, and not counted (5/11)
        for n in 0..12 {
            let a = args(&[("action", "archive"), ("domain", &format!("d{n}.example"))]);
            assert!(call(&settings, "coach", run("run_1"), "mcp__bipagents__mail_bulk", &a, 2.5).is_none(), "{n}");
        }
        let refused = call(&settings, "coach", run("run_1"), "notebook_write", &add(99), 3.0).unwrap();
        assert_eq!(refused["isError"], json!(true));
        // another agent, or the next turn: counted afresh
        assert!(call(&settings, "other", run("run_1"), "calendar_add", &add(0), 3.0).is_none());
        assert!(call(&settings, "coach", run("run_2"), "calendar_add", &add(0), 4.0).is_none());
        // and the first turn is still its own (two runs at once: each its count)
        assert!(call(&settings, "coach", run("run_1"), "calendar_add", &add(77), 4.0).is_some());
        // no run followed: one turn until quiet for idle_seconds
        // (a routine: scheduled work, the task's 40 writes)
        for n in 0..40 {
            let tool = if n % 2 == 0 { "create_audio" } else { "create_page" };
            assert!(call(&settings, "solo", None, tool, &add(0), n as f64).is_none(), "the same, no run: {n}");
        }
        let refused = call(&settings, "solo", None, "calendar_add", &add(1), 100.0).expect("the 41st write");
        assert_eq!(refused["content"][0]["text"].as_str().unwrap(), writes_refused(40, true));
        assert!(call(&settings, "solo", None, "calendar_add", &add(1), 100.0 + 121.0).is_none());
        // off
        let off = Settings { enabled: false, ..Settings::default() };
        assert!(call(&off, "coach", run("run_2"), "calendar_add", &add(0), 5.0).is_none());
    }

    // -- the token lab's bench of 2026-10-27 (internal/research/token-diet-step3-2026-10-27.md), its real sequences --

    /// A turn's tools as the bench saw them (`turns[].tools`), runs of the same tool.
    fn sequence(runs: &[(&str, usize)]) -> Vec<String> {
        runs.iter().flat_map(|(tool, n)| std::iter::repeat_n(tool.to_string(), *n)).collect()
    }

    /// The run hub's count fed the turn's tool.started events: where it stops (the call's index), and why.
    fn stopped(tools: &[String], signature: impl Fn(&str) -> Option<String>) -> Option<(usize, Over)> {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let mut browsing = false;
        tools.iter().enumerate().find_map(|(i, tool)| {
            let browser = crate::runs::browser_tool(tool);
            browsing |= browser;
            // runs.rs: a browsing turn's total is tool_calls + browser_calls
            let limits = if browsing { Limits { tool_calls: limits.tool_calls + limits.browser_calls, ..limits } } else { limits };
            turn.note(tool, signature(tool).as_deref(), browser, &limits).map(|over| (i, over))
        })
    }

    const NOTE: &str = "mcp__bipagents__notebook_write";
    const SEARCH: &str = "mcp__bipagents__notebook_search";
    const READ_PAGE: &str = "mcp__stealth__get_page_content";

    fn birthday() -> Vec<String> {
        // glm-all-2, birthday_lookup: 20 Carnet writes (6 identical), 11 min, no answer
        sequence(&[
            (SEARCH, 1), (NOTE, 1), ("tool_search", 1), (NOTE, 2), ("mcp__bipagents__notebook_read", 1), ("terminal", 1),
            (NOTE, 1), ("tool_call", 1), ("tool_search", 1), (NOTE, 1), ("terminal", 1), (NOTE, 4), ("terminal", 1),
            (NOTE, 3), ("terminal", 1), (NOTE, 8), ("terminal", 1),
        ])
    }

    #[test]
    fn the_bench_birthday_loop_is_stopped() {
        let tools = birthday();
        assert_eq!((tools.len(), tools.iter().filter(|t| *t == NOTE).count()), (30, 20));
        // the bench's events carry no arguments: at the 11th write
        let (at, over) = stopped(&tools, |_| None).unwrap();
        assert_eq!(over, Over::SameTool("notebook_write".into()));
        assert_eq!(tools[..=at].iter().filter(|t| *t == NOTE).count(), 11);
        assert_eq!(notice_text(&over, "fr"), "Je me suis arrêté : je tournais en rond sur le Carnet. Dis-moi si tu veux que je réessaie.");
        // with its arguments (the same note again): at the 3rd identical call
        let (at, over) = stopped(&tools, |t| (t == NOTE).then(|| "{\"title\":\"Julie\"}".to_string())).unwrap();
        assert_eq!(over, Over::SameCall("notebook_write".into()));
        assert_eq!(tools[..=at].iter().filter(|t| *t == NOTE).count(), 3);
        // and at the MCP: the 11th write of the turn refused, the same note again answered « already done »
        let turns = Turns::default();
        let settings = Settings::default();
        let turn = Some(Turn { run_id: "run_1d523504".into(), background: false, banned: None });
        let mut refused = Vec::new();
        for (i, tool) in tools.iter().enumerate().filter(|(_, t)| *t == NOTE) {
            let a = args(&[("title", "Julie"), ("text", &format!("note {i}"))]);
            match turns.check(&settings, "quotidien", turn.as_ref(), tool, &a, i as f64) {
                None => turns.record("quotidien", turn.as_ref(), tool, &a),
                Some(r) => refused.push(r),
            }
        }
        assert_eq!(refused.len(), 10);
        assert!(refused.iter().all(|r| r["isError"] == json!(true)));
        let same = args(&[("title", "Julie"), ("text", "note 1")]);
        assert_eq!(turns.check(&settings, "quotidien", turn.as_ref(), NOTE, &same, 40.0).unwrap()["isError"], json!(false));
    }

    #[test]
    fn the_bench_draft_loop_is_stopped() {
        // glm-concise-2, draft_reply: 25 notebook_search
        let tools = sequence(&[("mcp__bipagents__calendar_list", 1), (SEARCH, 2), ("tool_describe", 1), (SEARCH, 23)]);
        let (at, over) = stopped(&tools, |_| None).unwrap();
        assert_eq!(over, Over::SameTool("notebook_search".into()));
        assert_eq!(tools[..=at].iter().filter(|t| *t == SEARCH).count(), 11, "the 11th search");
    }

    #[test]
    fn the_bench_browser_loops_are_stopped_before_hermes_cap() {
        // glm-base-2 and glm-concise-2, browser_public: 141 and 145 page reads, cut by Hermes at 150, no answer
        let base = sequence(&[
            ("skill_view", 1), ("mcp__stealth__spawn_browser", 1), ("mcp__stealth__navigate", 1), (READ_PAGE, 1),
            ("tool_search", 1), ("search_files", 1), ("mcp__stealth__navigate", 1), ("tool_search", 1), (READ_PAGE, 3),
            ("tool_describe", 1), (READ_PAGE, 5), ("mcp__stealth__execute_script", 1), (READ_PAGE, 132),
        ]);
        let concise = sequence(&[
            ("skill_view", 1), ("mcp__stealth__spawn_browser", 1), ("mcp__stealth__navigate", 1), (READ_PAGE, 1),
            ("terminal", 1), ("tool_describe", 1), (READ_PAGE, 144),
        ]);
        for (tools, reads) in [(base, 141), (concise, 145)] {
            assert_eq!((tools.len(), tools.iter().filter(|t| *t == READ_PAGE).count()), (150, reads));
            // the bridge's events carry the page reads' arguments: the same each time, still not « same call » (the
            // browser's), but the page reads' own limit
            let (at, over) = stopped(&tools, |t| Some(format!("{t} {{}}"))).unwrap();
            assert_eq!(over, Over::PageReads);
            assert_eq!(tools[..=at].iter().filter(|t| *t == READ_PAGE).count(), 16);
            assert!(at < 30, "long before Hermes' 150: {at}");
        }
        assert_eq!(
            notice_text(&Over::PageReads, "fr"),
            "Je me suis arrêté : je n'arrivais pas à lire cette page. Dis-moi si tu veux que je réessaie, ou donne-moi un \
autre lien."
        );
        // browsing that goes on without reading the page: at its own limit, still under Hermes' cap
        let clicks = sequence(&[("mcp__stealth__click_element", 200)]);
        let (at, over) = stopped(&clicks, |_| None).unwrap();
        assert_eq!((at, over), (40, Over::Browser));
    }

    #[test]
    fn the_bench_legitimate_turns_go_on() {
        // the bench's good turns: browsing that reads a few pages, a long file delivery with the terminal
        let browsing = sequence(&[
            ("skill_view", 1), ("mcp__stealth__spawn_browser", 1), ("mcp__stealth__navigate", 6), (READ_PAGE, 6),
            ("mcp__bipagents__calendar_list", 1),
        ]);
        assert_eq!(stopped(&browsing, |_| None), None);
        let files = sequence(&[("terminal", 12), ("write_file", 10), ("read_file", 10)]);
        assert_eq!(stopped(&files, |_| None), None);
    }

    #[test]
    fn the_bench_false_positives_are_allowed() {
        // DeepSeek (reminder ×2, plan_day, map_walk) and the morning brief: refused at the first or second write of a
        // conversation, because earlier conversations' writes of the same agent were counted with them
        let turns = Turns::default();
        let settings = Settings::default();
        let call = |agent: &str, turn: Option<Turn>, tool: &str, a: &Dict, now: f64| {
            let refused = turns.check(&settings, agent, turn.as_ref(), tool, a, now);
            if refused.is_none() {
                turns.record(agent, turn.as_ref(), tool, a);
            }
            refused
        };
        let run = |id: &str| Some(Turn { run_id: id.to_string(), background: false, banned: None });
        // an earlier conversation of the agent wrote 20 notes (its last ones refused)
        for i in 0..20 {
            call("quotidien", run("run_birthday"), NOTE, &args(&[("text", &format!("n{i}"))]), i as f64);
        }
        // the next conversations, a minute later each
        let reminder = |text: &str| args(&[("title", text), ("due", "2026-10-28T09:00")]);
        assert!(call("quotidien", run("run_reminder"), "mcp__bipagents__reminders_add", &reminder("Pharmacie"), 60.0).is_none());
        assert!(call("quotidien", run("run_reminder"), "mcp__bipagents__reminders_add", &reminder("Banque"), 61.0).is_none());
        let page = args(&[("title", "Ma journée"), ("html", "<h1>Mardi</h1>")]);
        assert!(call("quotidien", run("run_plan_day"), "mcp__bipagents__create_page", &page, 120.0).is_none());
        assert!(call("quotidien", run("run_map_walk"), "mcp__bipagents__create_page", &page, 180.0).is_none());
        // the morning brief (a routine, no run): its page, after the bench's turns
        assert!(call("quotidien", None, "mcp__bipagents__create_page", &page, 240.0).is_none());
        // a routine's search loop (35 Carnet searches in the bench): refused from the 11th, in plain words (a routine
        // writes like a task, it doesn't search more than a conversation)
        let mut refused = None;
        for i in 0..35 {
            if let Some(r) = call("budget", None, SEARCH, &args(&[("query", &format!("ville {i}"))]), 1000.0 + i as f64) {
                refused = Some((i, r));
                break;
            }
        }
        let (i, r) = refused.unwrap();
        assert_eq!(i, 10);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("answer the person with what you have"));
        // and a routine's calls of many tools: at routine_tool_calls
        let mut at = None;
        for i in 0..150 {
            let tool = format!("mcp__bipagents__tool_{}", i % 16);
            if call("coach", None, &tool, &Dict::new(), 5000.0 + i as f64).is_some() {
                at = Some(i);
                break;
            }
        }
        assert_eq!(at, Some(120));
    }

    #[test]
    fn signatures_from_the_events() {
        let data = |v: Value| v.as_object().cloned().unwrap();
        assert_eq!(event_tool(&data(json!({"tool_name": "x"}))), "x");
        assert_eq!(event_tool(&data(json!({"tool": "", "name": "y"}))), "y");
        assert_eq!(event_signature(&data(json!({"args": {"b": 1, "a": 2}, "preview": "p"}))).unwrap(), "{\"a\":2,\"b\":1}");
        assert_eq!(event_signature(&data(json!({"args": {}, "preview": "p"}))).unwrap(), "p");
        assert_eq!(event_signature(&data(json!({"preview": " "}))), None);
    }

    // -- 2026-10-28 09:24, Alex's home server: Vie diagnosing the failing « scan-emails-fond-nuit » job was stopped
    // after 5 calls, « same call repeated (session_search) » --

    const SESSIONS: &str = "session_search";

    #[test]
    fn the_mornings_diagnosis_goes_on_now() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let query = "scan-emails-fond-nuit";
        let calls = [
            (SESSIONS, query),
            ("skill_view", "cronjob"),
            (SESSIONS, query),
            ("read_file", "~/.hermes/cron/jobs.json"),
            (SESSIONS, query),
        ];
        for (i, (tool, preview)) in calls.iter().enumerate() {
            assert_eq!(turn.note(tool, Some(preview), false, &limits), None, "call {i}");
        }
        assert_eq!(turn.total, 5);
        // the same lookup a 4th and a 5th time: stopped at the 5th
        assert_eq!(turn.note(SESSIONS, Some(query), false, &limits), None);
        assert_eq!(turn.note(SESSIONS, Some(query), false, &limits), Some(Over::SameCall(SESSIONS.into())));
    }

    #[test]
    fn five_identical_lookups_stop_and_are_said_in_plain_words() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let stopped = (0..10).find_map(|i| turn.note(SESSIONS, Some("nuit"), false, &limits).map(|o| (i, o)));
        assert_eq!(stopped, Some((4, Over::SameCall(SESSIONS.into()))));
        let over = Over::SameCall(SESSIONS.into());
        assert_eq!(
            notice_text(&over, "fr"),
            "Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions. Dis-moi si tu veux que je \
réessaie autrement."
        );
        assert_eq!(notice_head(&over, "fr"), "Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions.");
        assert!(notice_text(&over, "en").contains("the same search in our past conversations"));
        assert!(notice_text(&over, "de").contains("früheren Unterhaltungen"));
        assert!(notice_text(&over, "es-ES").contains("conversaciones anteriores"));
        for lang in ["fr", "en", "de", "es"] {
            assert!(!notice_text(&over, lang).contains("session_search"), "never a tool's name: {lang}");
        }
        assert!(notice_text(&Over::SameCall("mcp__bipagents__notebook_search".into()), "fr").contains("dans le Carnet"));
        assert!(notice_text(&Over::SameCall("web_search".into()), "fr").contains("la même recherche sur le web"));
        // writes and actions keep same_call (3)
        let mut turn = TurnCount::default();
        let stopped = (0..10).find_map(|i| turn.note("mcp_bipagents_calendar_add", Some("{\"t\":1}"), false, &limits).map(|o| (i, o)));
        assert_eq!(stopped, Some((2, Over::SameCall("calendar_add".into()))));
        // Hermes' memory: a read is a lookup, an addition is not
        assert!(lookup_tool("memory", Some("{\"action\": \"read\"}")) && !lookup_tool("memory", Some("add: le code du portail")));
        assert!(lookup_tool("mcp_bipagents_calendar_list", None) && !lookup_tool("mcp_bipagents_calendar_add", None));
    }

    #[test]
    fn a_retry_after_an_error_is_not_the_same_call() {
        // failures in a row are `same_failure`'s (the next test): out of the way here
        let limits = Limits { same_failure: 100, ..Settings::default().conversation };
        let mut turn = TurnCount::default();
        let failed = |turn: &mut TurnCount| turn.completed("mcp_bipagents_calendar_add", true, &limits);
        let call = "{\"title\":\"Dentiste\"}";
        // the same write five times, each right after the identical one failed: retries
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), None);
        for _ in 0..4 {
            failed(&mut turn);
            assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), None);
        }
        // it went through, then the same again: counted (2nd, then 3rd: stopped)
        turn.completed("mcp_bipagents_calendar_add", false, &limits);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), None);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), Some(Over::SameCall("calendar_add".into())));
        // an error, then another call in between: the identical call after is counted again
        let mut turn = TurnCount::default();
        for _ in 0..4 {
            assert_eq!(turn.note(SESSIONS, Some("nuit"), false, &limits), None);
            turn.completed(SESSIONS, false, &limits);
        }
        turn.completed(SESSIONS, true, &limits);
        assert_eq!(turn.note("skill_view", Some("cronjob"), false, &limits), None);
        assert_eq!(turn.note(SESSIONS, Some("nuit"), false, &limits), Some(Over::SameCall(SESSIONS.into())));
        // what tells an error
        let data = |v: Value| v.as_object().cloned().unwrap();
        assert!(completed_with_error(&data(json!({"tool": "x", "error": true}))));
        assert!(completed_with_error(&data(json!({"tool": "x", "preview": "{\"success\": false, \"error\": \"db locked\"}"}))));
        assert!(completed_with_error(&data(json!({"tool": "x", "preview": "Error: timeout"}))));
        assert!(!completed_with_error(&data(json!({"tool": "x", "error": false, "preview": "3 sessions found"}))));
    }

    #[test]
    fn the_rescue_turn_is_short_and_may_not_call_the_tool_again() {
        let settings = Settings::default();
        let limits = settings.rescue_limits();
        let over = Over::SameCall(SESSIONS.into());
        let mut turn = TurnCount { banned: stopped_tool(&over), ..TurnCount::default() };
        // once (Hermes' own tool: it runs; the bridge's: refused at the MCP), twice: stopped
        assert_eq!(turn.note(SESSIONS, Some("autre"), false, &limits), None);
        assert_eq!(turn.note(SESSIONS, Some("encore"), false, &limits), Some(Over::SameCall(SESSIONS.into())));
        // at most 5 calls
        let mut turn = TurnCount::default();
        let stopped = (0..10).find_map(|i| turn.note("read_file", Some(&format!("f{i}")), false, &limits).map(|o| (i, o)));
        assert_eq!(stopped, Some((5, Over::TooMany)));
        // its input: the mark, the person's line, then the agent's instruction (out of sight)
        let input = rescue_input(&over, "fr");
        assert!(input.starts_with(
            "[bipagents:loop_rescue] Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions.\n\
<!--bip:then Dis-moi si tu veux que je réessaie autrement.-->\n\n"
        ));
        assert!(input.contains("Don't call session_search again in this turn."));
        assert!(input.contains("answer the person in their language"));
        assert!(rescue_input(&Over::TooMany, "en").contains("Don't start new work"));
        // the bridge's own tool, in the rescue turn: refused, in plain words the model can act on
        let turns = Turns::default();
        let rescue = Turn { run_id: "run_r".into(), background: false, banned: Some("notebook_search".into()) };
        let refused = turns
            .check(&settings, "vie", Some(&rescue), "mcp__bipagents__notebook_search", &args(&[("words", "nuit")]), 0.0)
            .unwrap();
        assert_eq!(refused["isError"], json!(true));
        assert!(refused["content"][0]["text"].as_str().unwrap().contains("Answer the person now"));
        assert!(turns.check(&settings, "vie", Some(&rescue), "mcp__bipagents__calendar_list", &Dict::new(), 0.0).is_none());
        // the notice when a rescue follows: its first sentence only
        let data = notice(&over, "fr", true);
        assert_eq!(data["text"], json!(notice_head(&over, "fr")));
        assert_eq!(data["rescue"], json!(true));
        assert!(!notice(&over, "fr", false).contains_key("rescue"));
    }

    #[test]
    fn the_agent_is_told_before_the_stop_and_a_turn_that_changes_approach_goes_on() {
        let limits = Settings::default().conversation;
        assert_eq!((soft_limit(5), soft_limit(3), soft_limit(60), soft_limit(10)), (3, 2, 40, 7));
        let mut turn = TurnCount::default();
        // the same lookup: told at the 3rd (stopped at the 5th), once
        for i in 0..2 {
            assert_eq!(turn.note(SESSIONS, Some(NIGHT_Q), false, &limits), None);
            assert_eq!(turn.take_warning(), None, "{i}");
        }
        assert_eq!(turn.note(SESSIONS, Some(NIGHT_Q), false, &limits), None);
        let warning = turn.take_warning().unwrap();
        assert_eq!(warning, Warning::SameCall(SESSIONS.into(), 3, 5));
        assert!(warning.text().starts_with("[bipagents:brake] A note from the app, not from your person: you have made \
exactly the same session_search call 3 times in this turn; at 5 the turn is stopped. Change approach"));
        // it changes approach: other searches, other tools; never stopped, told once more when the total gets close
        let mut told = Vec::new();
        // (no web search: 13 of them would get the web searches' own note, `web_searches`)
        for i in 0..50 {
            let tool = ["web_extract", "read_file", "search_files", "terminal"][i % 4];
            assert_eq!(turn.note(tool, Some(&format!("autre piste {i}")), false, &limits), None, "{i}");
            told.extend(turn.take_warning());
        }
        assert_eq!(told, vec![Warning::TooMany(40, 60)]);
        assert_eq!(turn.total, 53);
        // a write: told at the 2nd identical one (stopped at the 3rd)
        let mut turn = TurnCount::default();
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some("{\"t\":1}"), false, &limits), None);
        assert_eq!(turn.take_warning(), None);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some("{\"t\":1}"), false, &limits), None);
        assert_eq!(turn.take_warning(), Some(Warning::SameCall("calendar_add".into(), 2, 3)));
        // one tool with different arguments: at the 7th of 10
        let mut turn = TurnCount::default();
        let told: Vec<_> = (0..7)
            .filter_map(|i| {
                turn.note("mcp_bipagents_reminders_add", Some(&format!("{i}")), false, &limits);
                turn.take_warning()
            })
            .collect();
        assert_eq!(told, vec![Warning::SameTool("reminders_add".into(), 7, 10)]);
    }

    const NIGHT_Q: &str = "scan-emails-fond-nuit";

    #[test]
    fn a_routine_reading_many_mails_goes_on_and_is_told_when_close() {
        // the replay's mail_scan_busy_night: 3 searches, 14 different mails read, 2 notes, in a routine
        let turns = Turns::default();
        let settings = Settings::default();
        let mut refused = Vec::new();
        let mut notes = Vec::new();
        let mut call = |tool: &str, a: Dict, i: usize| {
            match turns.check(&settings, "vie", None, tool, &a, i as f64) {
                Some(r) => refused.push((i, r)),
                None => turns.record("vie", None, tool, &a),
            }
            notes.extend(turns.take_note("vie", None).map(|n| (i, n)));
        };
        let mut i = 0;
        for q in ["facture", "banque", "école"] {
            call("mcp_bipagents_mail_search", args(&[("query", q)]), i);
            i += 1;
        }
        for m in 0..14 {
            call("mcp_bipagents_mail_read", args(&[("id", &format!("m{m}"))]), i);
            i += 1;
        }
        call("mcp_bipagents_mail_attachment", args(&[("id", "m3"), ("n", "1")]), i);
        i += 1;
        for n in 0..2 {
            call("mcp_bipagents_notebook_write", args(&[("text", &format!("n{n}"))]), i);
            i += 1;
        }
        assert!(refused.is_empty(), "{refused:?}");
        assert!(notes.is_empty(), "20 calls: under the total's two thirds (80 of 120): {notes:?}");
        // the same mail read again and again: told at the 7th, refused at the 11th
        let turns = Turns::default();
        let mut told = None;
        let refused_at = (0..20).find(|i| {
            let a = args(&[("id", "m1")]);
            let r = turns.check(&settings, "lea", None, "mcp_bipagents_mail_read", &a, 100.0 + *i as f64);
            if let Some(note) = turns.take_note("lea", None) {
                told.get_or_insert((*i, note));
            }
            r.is_some()
        });
        assert_eq!(refused_at, Some(10));
        let (at, note) = told.unwrap();
        assert_eq!(at, 6);
        assert!(note.starts_with("[bipagents:brake] Note from the app: you have called mail_read 7 times"));
        let mut result = crate::tools_knowledge::tool_text("Mail 1", false);
        add_note(&mut result, &note);
        assert!(result["content"][0]["text"].as_str().unwrap().starts_with("Mail 1\n\n[bipagents:brake]"));
        assert!(item_read("mcp_bipagents_mail_read") && item_read("mail_attachment") && !item_read("notebook_search"));
    }

    /// 20/11, Alex's Live call: a web search failing every time, retried with other words until Hermes' own guard
    /// stopped it at 50. The brake now stops a tool at its 3rd failure in a row (2nd in a call), whatever its words.
    #[test]
    fn a_tool_failing_again_and_again_stops_the_turn() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let queries = ["jeu DJ PS1", "jeu DJ PS1", "PS1 DJ game turntable", "beatmania playstation"];
        let mut stopped = None;
        for (i, query) in queries.iter().enumerate() {
            assert_eq!(turn.note("web_search", Some(query), false, &limits), None, "{i}");
            stopped = turn.completed("web_search", true, &limits);
            if stopped.is_some() {
                assert_eq!(i, 2, "the third failure in a row");
                break;
            }
        }
        assert_eq!(stopped, Some(Over::Failing("web_search".into())));
        // in a call: the second
        let live = Settings::default().live;
        let mut turn = TurnCount::default();
        assert_eq!(turn.completed("web_search", true, &live), None);
        assert_eq!(turn.completed("web_search", true, &live), Some(Over::Failing("web_search".into())));
        // a success of that tool in between starts again
        let mut turn = TurnCount::default();
        for ok in [false, false, true, false, false] {
            assert_eq!(turn.completed("web_search", !ok, &limits), None);
        }
        assert_eq!(turn.completed("web_search", true, &limits), Some(Over::Failing("web_search".into())));
        // so does any other call that went through (review 20/11: patch fails, the terminal runs, patch fails…)
        let mut turn = TurnCount::default();
        for _ in 0..5 {
            assert_eq!(turn.completed("patch", true, &limits), None);
            assert_eq!(turn.completed("terminal", false, &limits), None);
        }
        let mut turn = TurnCount::default();
        for _ in 0..5 {
            assert_eq!(turn.completed("web_search", true, &limits), None);
            assert_eq!(turn.completed("mcp_bipagents_calendar_list", false, &limits), None);
        }
        // another tool's failure is no success: it doesn't start them again
        let mut turn = TurnCount::default();
        turn.completed("web_search", true, &limits);
        turn.completed("web_extract", true, &limits);
        turn.completed("web_search", true, &limits);
        assert_eq!(turn.completed("web_search", true, &limits), Some(Over::Failing("web_search".into())));
        // the bridge's tools by their bare name
        let mut turn = TurnCount::default();
        turn.completed("mcp_bipagents_mail_search", true, &limits);
        turn.completed("mcp__bipagents__mail_search", true, &limits);
        assert_eq!(turn.completed("mail_search", true, &limits), Some(Over::Failing("mail_search".into())));
        // work whose failure is its output, and the browser (its own limits): never
        for tool in ["terminal", "execute_code", "process", "web_extract", "delegate_task", "browser_navigate",
            "mcp__stealth__navigate"]
        {
            let mut turn = TurnCount::default();
            for _ in 0..10 {
                assert_eq!(turn.completed(tool, true, &limits), None, "{tool}");
            }
        }
        // a task goes to 5; « Continuer » raises it once
        assert_eq!((Settings::default().task.same_failure, limits.raised().same_failure), (5, 5));
    }

    /// 20/11 (and 19/11): Vie on the Spark searched the web 50 times in one turn, every search answered, new words each
    /// time: Hermes' own cap stopped it. The brake stops a turn past 20 searches (a call past 6), told before at 13 (4).
    #[test]
    fn many_web_searches_stop_the_turn_even_when_they_all_answer() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let mut warned = None;
        for i in 1..=20 {
            assert_eq!(turn.note("web_search", Some(&format!("jeu DJ PS1 {i}")), false, &limits), None, "{i}");
            assert_eq!(turn.completed("web_search", false, &limits), None);
            if let Some(w) = turn.take_warning() {
                warned.get_or_insert((i, w));
            }
        }
        assert_eq!(warned, Some((13, Warning::Searches(13, 20))));
        assert!(Warning::Searches(13, 20).text().contains("propose_task"));
        assert_eq!(turn.note("web_search", Some("jeu DJ PS1 21"), false, &limits), Some(Over::Searches));
        // a call: the seventh
        let live = Settings::default().live;
        let mut turn = TurnCount::default();
        for i in 1..=6 {
            assert_eq!(turn.note("web_search", Some(&format!("q{i}")), false, &live), None, "{i}");
        }
        assert_eq!(turn.note("web_search", Some("q7"), false, &live), Some(Over::Searches));
        // a background task goes much further; « Continuer » raises it once
        let task = Settings::default().task;
        let mut turn = TurnCount::default();
        for i in 1..=40 {
            assert_eq!(turn.note("web_search", Some(&format!("q{i}")), false, &task), None, "{i}");
        }
        assert_eq!(limits.raised().web_searches, 30);
        // other lookups don't count as web searches
        let mut turn = TurnCount::default();
        for i in 1..=20 {
            assert_eq!(turn.note("web_extract", Some(&format!("u{i}")), false, &limits), None, "{i}");
        }
        // said plainly, the rescue turn may not search again
        assert_eq!(
            notice_text(&Over::Searches, "fr"),
            "J'ai fait beaucoup de recherches sur le web sans trouver de réponse sûre. Dis-moi si tu veux que j'en fasse \
une recherche approfondie en tâche de fond."
        );
        for lang in ["en", "de", "es", "nl", "it"] {
            assert!(!notice_text(&Over::Searches, lang).contains('_'), "{lang}");
        }
        assert_eq!(stopped_tool(&Over::Searches), Some("web_search".into()));
        assert!(rescue_input(&Over::Searches, "fr").contains("Don't search the web again"));
    }

    /// 27/11 09:2x, Alex: a preference noted (« Noté dans Podcast ›»), then « Je regarde comment configurer ça. » and
    /// « Je me suis arrêté : je refaisais sans cesse la même étape. »; « T'as réussi à noter ? » → « Je vérifie que ta
    /// préférence est bien enregistrée… » and the same stop in the next turn. Hermes' `memory` has no read: the model
    /// checks by adding the same entry again (« Entry already exists (no duplicate added). »), three identical calls,
    /// `same_call`. Now: such a confirmation is no loop, and the agent is told it is done; a write of the bridge's asked
    /// again says « already done », what it answered, and that nothing failed; a read never counts as a write.
    #[test]
    fn checking_a_note_is_no_loop() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let entry = "add: Podcast du matin : annoncer les rappels quelques jours avant, puis deux jours avant et la veille";
        let exists = json!({"tool": "memory", "error": false,
            "preview": "{\"success\": true, \"message\": \"Entry already exists (no duplicate added).\"}"}).as_object().cloned().unwrap();
        assert_eq!(turn.note("memory", Some(entry), false, &limits), None);
        assert_eq!(turn.completed("memory", false, &limits), None);
        for i in 0..4 {
            assert_eq!(turn.note("memory", Some(entry), false, &limits), None, "check {i}");
            assert_eq!(turn.completed("memory", false, &limits), None);
            assert!(already_there(&exists));
            turn.confirmed("memory");
        }
        let told: Vec<Warning> = std::iter::from_fn(|| turn.take_warning()).collect();
        assert!(told.contains(&Warning::Saved("memory".into())), "{told:?}");
        assert!(Warning::Saved("memory".into()).text().contains("it is done"));
        assert!(!already_there(&json!({"preview": "{\"success\": true, \"message\": \"Entry added.\"}"}).as_object().cloned().unwrap()));
        // the same entry added again and again, never « already there »: still a loop, at a lookup's limit (5)
        let mut turn = TurnCount::default();
        for _ in 1..limits.same_lookup {
            assert_eq!(turn.note("memory", Some(entry), false, &limits), None);
        }
        assert_eq!(turn.note("memory", Some(entry), false, &limits), Some(Over::SameCall("memory".into())));
        // the bridge's own write asked again: done, what it answered, nothing failed
        let turns = Turns::default();
        let settings = Settings::default();
        let t = Some(Turn { run_id: "run_p".into(), background: false, banned: None });
        let note = json!({"page": "Podcast", "facts": "Rappels : quelques jours avant, deux jours avant, la veille"}).as_object().cloned().unwrap();
        assert!(turns.check(&settings, "vie", t.as_ref(), "notebook_write", &note, 1.0).is_none());
        turns.record_answer("vie", t.as_ref(), "notebook_write", &note, &tool_text("Noté dans Podcast : 1 fait.", false));
        let again = turns.check(&settings, "vie", t.as_ref(), "notebook_write", &note, 2.0).expect("done once");
        let said = again["content"][0]["text"].as_str().unwrap();
        assert!(said.starts_with("Already done") && said.contains("nothing failed") && said.contains("Noté dans Podcast"), "{said}");
        assert!(!said.contains("not done") && !said.contains("call that one now"), "{said}");
        assert!(turns.already_done("vie", t.as_ref(), "notebook_write", &note));
        // reading it back to check: never a write
        let read = json!({"page": "Podcast"}).as_object().cloned().unwrap();
        assert!(!write_call("notebook_read", &read) && turns.check(&settings, "vie", t.as_ref(), "notebook_read", &read, 3.0).is_none());
        assert!(lookup_tool("notebook_read", None));
    }

    /// The preview of a `tool.completed` Hermes sends (its result's first 500 characters), failed.
    fn failed(preview: &str) -> serde_json::Map<String, Value> {
        json!({"tool": "web_search", "duration": 0.04, "error": true, "preview": preview}).as_object().cloned().unwrap()
    }

    /// 27/11 08:52, Alex's aibox on Wi-Fi (Famille): a few seconds without the network, `web_search` failed three times
    /// in three seconds (Tavily's name not resolved, then Hermes' keyless rescue's), and `same_failure` stopped the turn
    /// with « Je n'arrive pas à chercher sur le web pour l'instant. ». A network error is no broken tool: counted apart,
    /// the agent told once to wait a few seconds before trying again, and the turn stopped only when it lasts
    /// (`offline_failures` failures over at least `offline_seconds`), with the right words.
    #[test]
    fn a_network_cut_of_a_few_seconds_is_no_broken_tool() {
        let famille = "{\n  \"success\": false,\n  \"error\": \"Tavily search failed: HTTPSConnectionPool(host='api.tavily.com', \
port=443): Max retries exceeded with url: /search (Caused by NameResolutionError(\\\"<urllib3.connection.HTTPSConnection \
object at 0x7f2c>: Failed to resolve 'api.tavily.com' ([Errno -3] Temporary failure in name resolution)\\\")) (keyless rescue \
also failed: Keyless Parallel search failed: request failed: HTTPSConnectionPool(host='search.parallel.ai', port=443): Max \
retries exceeded with url: /mcp (Caused by NameResolutionError(\\\"<urllib3.connection.HTTPSConnection object>: Failed to resolve";
        for preview in [
            famille,
            "Error searching web: [Errno 111] Connection refused",
            "{\"error\": \"MCP call failed: ConnectionResetError(104, 'Connection reset by peer')\"}",
            "{\"success\": false, \"error\": \"Keyless Exa search failed: request failed: HTTPSConnectionPool(host='mcp.exa.ai', port=443): Read timed out. (read timeout=20)\"}",
            "{\"success\": false, \"error\": \"Keyless Parallel search failed: HTTP 503: Service Unavailable\"}",
            "{\"success\": false, \"error\": \"Tavily search failed: HTTP 429: Too Many Requests\"}",
            "Error: [Errno 101] Network is unreachable",
            "{\"error\": \"getaddrinfo failed: nodename nor servname provided, or not known\"}",
            "{\"error\": \"upstream connect error: 502 Bad Gateway\"}",
        ] {
            assert!(transient_failure(&failed(preview)), "{preview}");
        }
        for preview in [
            "{\"success\": false, \"error\": \"No web search provider configured. Run `hermes tools` to set one up.\"}",
            "{\"success\": false, \"error\": \"Tavily search failed: HTTP 401: invalid api key\"}",
            "Not done: give_file only hands your person a file you made, and needs `path`.",
            "{\"error\": \"Invalid date: 2026-13-45\"}",
            "{\"success\": false, \"error\": \"HTTP 400: invalid query 'http 503 timeout'\"}",
        ] {
            assert!(!transient_failure(&failed(preview)), "{preview}");
        }
        assert!(!transient_failure(&json!({"tool": "web_search", "error": false,
            "preview": "Connection refused: the shop's page says so"}).as_object().cloned().unwrap()), "a result, not a failure");

        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        // the morning of 27/11: three failures in three seconds, then the network is back
        for (i, at) in [0.0, 1.0, 2.0].iter().enumerate() {
            assert_eq!(turn.note("web_search", Some("gants merino"), false, &limits), None, "{i}");
            assert_eq!(turn.failed_transiently("web_search", &limits, *at), None, "{i}: a cut of a few seconds goes on");
        }
        // told once: wait a few seconds, then try again
        let told = turn.take_warning().expect("the agent is told to wait");
        assert_eq!(told, Warning::Network("web_search".into()));
        assert!(told.text().contains("wait a few seconds") && told.text().starts_with(WARN_MARK), "{}", told.text());
        assert_eq!(turn.take_warning(), None, "once");
        assert_eq!(turn.note("web_search", Some("gants merino"), false, &limits), None);
        assert_eq!(turn.completed("web_search", false, &limits), None, "back: the count starts again");
        // a cut that lasts: stopped once it has failed `offline_failures` times over `offline_seconds`
        let mut turn = TurnCount::default();
        let mut stopped = None;
        let mut at = 0.0;
        for i in 0..20 {
            stopped = turn.failed_transiently("web_search", &limits, at);
            if stopped.is_some() {
                assert!(i + 1 >= limits.offline_failures as usize && at >= limits.offline_seconds, "{i} at {at}");
                break;
            }
            at += 5.0;
        }
        assert_eq!(stopped, Some(Over::Offline("web_search".into())));
        // hammering at once never runs for ever: twice as many failures stop it whatever the time
        let mut turn = TurnCount::default();
        let hard = (1..=40).find(|_| turn.failed_transiently("web_search", &limits, 0.0).is_some());
        assert_eq!(hard, Some(2 * limits.offline_failures));
        // a broken tool still stops at its third failure; network errors in between neither count nor clear them
        let mut turn = TurnCount::default();
        assert_eq!(turn.completed("calendar_list", true, &limits), None);
        assert_eq!(turn.failed_transiently("web_search", &limits, 0.0), None);
        assert_eq!(turn.completed("calendar_list", true, &limits), None);
        assert_eq!(turn.completed("calendar_list", true, &limits), Some(Over::Failing("calendar_list".into())));
        // said plainly, in four languages (and Dutch), never « chercher sur le web »
        let offline = Over::Offline("web_search".into());
        assert_eq!(
            notice_text(&offline, "fr"),
            "Je n'arrive pas à joindre Internet pour l'instant. Tu veux que je réessaie plus tard ?"
        );
        assert_eq!(notice_text(&offline, "en"), "I can't reach the internet right now. Do you want me to try again later?");
        assert_eq!(notice_text(&offline, "de"), "Ich komme gerade nicht ins Internet. Soll ich es später noch einmal versuchen?");
        assert_eq!(notice_text(&offline, "es"), "Ahora mismo no consigo conectarme a internet. ¿Quieres que lo intente más tarde?");
        assert_eq!(reason(&offline), "the network unreachable (web_search)");
        assert_eq!(stopped_tool(&offline), Some("web_search".into()));
        assert!(rescue_input(&offline, "fr").contains("couldn't reach the internet"));
        // a task waits longer, a call shorter; the config may change them
        let d = Settings::default();
        assert!(d.task.offline_seconds > d.conversation.offline_seconds && d.live.offline_seconds < d.conversation.offline_seconds);
        let s = Settings::from_table(&json!({"offline_failures": 6, "offline_seconds": 45, "live_offline_failures": 1}));
        assert_eq!((s.conversation.offline_failures, s.conversation.offline_seconds, s.live.offline_failures), (6, 45.0, 2));
    }

    #[test]
    fn a_failing_tool_is_said_plainly_in_five_languages() {
        let web = Over::Failing("web_search".into());
        assert_eq!(
            notice_text(&web, "fr"),
            "Je n'arrive pas à chercher sur le web pour l'instant. Tu veux que je réessaie plus tard ?"
        );
        assert_eq!(notice_text(&web, "en"), "I can't search the web right now. Do you want me to try again later?");
        assert_eq!(notice_text(&web, "de-DE"), "Ich kann gerade nicht im Web suchen. Soll ich es später noch einmal versuchen?");
        assert_eq!(notice_text(&web, "es"), "Ahora mismo no consigo buscar en la web. ¿Quieres que lo intente más tarde?");
        assert_eq!(notice_text(&web, "nl"), "Ik kan nu niet op het web zoeken. Wil je dat ik het later opnieuw probeer?");
        assert_eq!(
            notice_text(&Over::Failing("calendar_list".into()), "fr"),
            "Je n'arrive pas à utiliser l'agenda pour l'instant. Tu veux que je réessaie plus tard ?"
        );
        for lang in ["fr", "en", "de", "es", "nl", "it"] {
            for tool in ["web_search", "web_extract", "calendar_list", "memory", ""] {
                let line = notice_text(&Over::Failing(tool.into()), lang);
                assert!(!line.contains('_') && !line.contains('`'), "{lang} {tool}: {line}");
            }
        }
        // the rescue turn: told not to call it again
        assert!(rescue_input(&web, "fr").contains("Don't call web_search again"));
        assert_eq!(stopped_tool(&web), Some("web_search".into()));
        assert_eq!(stopped_tool(&Over::Failing(String::new())), None);
    }

    #[test]
    fn the_new_settings_from_the_config() {
        let s = Settings::from_table(&json!({"same_lookup": 7, "task_same_lookup": 12, "rescue": false,
                                             "rescue_tool_calls": 3, "rescue_seconds": 5}));
        assert_eq!((s.conversation.same_lookup, s.task.same_lookup), (7, 12));
        assert_eq!(s.conversation.same_call, 3, "writes and actions unchanged");
        assert!(!s.rescue);
        assert_eq!(s.rescue_limits().tool_calls, 3);
        assert_eq!(s.rescue_seconds, 20.0, "clamped");
        let d = Settings::default();
        assert_eq!((d.conversation.same_lookup, d.conversation.same_call, d.rescue, d.rescue_tool_calls), (5, 3, true, 5));
        let s = Settings::from_table(&json!({"same_failure": 4, "live_same_failure": 1, "live_tool_calls": 20}));
        assert_eq!((s.conversation.same_failure, s.live.same_failure, s.live.tool_calls), (4, 2, 20), "clamped at 2");
        assert_eq!((d.conversation.same_failure, d.task.same_failure, d.live.same_failure), (3, 5, 2));
        // a call: well under a conversation's limits, under Hermes' own cap of 50 web searches
        assert!(d.live.tool_calls < d.conversation.tool_calls && d.live.tool_calls < 50);
        // web searches: all under Hermes' own cap of 50 (its English halt never comes first)
        assert_eq!((d.conversation.web_searches, d.task.web_searches, d.live.web_searches), (20, 40, 6));
        assert_eq!(Settings::from_table(&json!({"web_searches": 20, "live_web_searches": 6})).live.web_searches, 6);
    }
}
