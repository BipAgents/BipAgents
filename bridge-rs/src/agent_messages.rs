//! Asking another agent of the same person, from any conversation (real-day bench 2026-10-28: « Demande à Coach… »
//! failed with Quotidien linked to Coach).
//!
//! Hermes injects its own `message_agent` only in an agent's canonical « Bot Chat » (tools/bot_mode_dm.py:
//! `message_agent_authorized`, the session's title): in the person's other conversations, the bench's, a routine's
//! or a background task's, the agent had no way to reach another one, whatever links the person turned on, and it
//! hunted for one (18 calls, tool_search ×9). So the bridge offers its own `message_agent` (`mcp__bipagents__
//! message_agent`) to an agent that may write to another agent of this place (the links of « Qui peut écrire à
//! qui », peers.rs; the same ones as `send_files_to_agent`). It checks the link and asks the target in the two agents'
//! own conversation « A ⇄ B » (agent_pairs.rs, since 2026-10-29: not in the target's Bot Chat any more, the person's
//! main conversation with it), signed as Hermes signs a peer's message (« Message from Quotidien (@quotidien): … »,
//! peers.rs `SIGNATURE`), and gives back the target's answer in the same call: the asker can answer its person in
//! the same turn. **Another household's agent** too, where both households' rules allow it (Alex 21/11): through the
//! two bridges (household_asks.rs), its answer back in the same call, never Hermes' Bot Mode (off everywhere since,
//! peers.rs `drop_bot_mode`: no agent leaves the shared hub for it). During a relative's turn or in a shared
//! conversation the tool is closed like the person's other data (agent_shares.rs `RELATIVE_TOOLS`).
//! Since 6/11 (Hermes 0.23 study §4.4-3): at most `agent_hops::MAX_HOPS` messages between agents since the person
//! last spoke, counted by the bridge, not the model (agent_hops.rs); past them the call is refused at once (`CAPPED`)
//! and the person gets a « Continuer » card. A message to an agent waiting for this very exchange is answered at once
//! too (agent_pairs.rs `Cycle`): never a wait on the pair's turn.
//! The family's agent gets it too since 21/11 (Alex: « si la personne donne le droit à écrire, même pas besoin de
//! carte »), for its creator's agents the links allow (family_agent.rs `closed_call`): the target's note says it is
//! the family's agent asking for that member, its answer read by the whole family (agent_pairs.rs `framed_for`); its
//! texts name the creator, the only one who can allow another agent. Another household's agents stay out of its reach.
//!
//! No Python counterpart.

use serde_json::{json, Value};

use crate::agent_pairs::FamilyAsk;
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "message_agent";
/// Hermes' own cap on a DM (tools/bot_mode_dm.py `MESSAGE_MAX_CHARS`).
pub const MAX_CHARS: usize = 16000;
/// How long the target may take to answer (one turn of its own: a search, a look at its data).
pub const ANSWER_SECONDS: f64 = 600.0;

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Ask another agent your person lets you write to (see your contacts: one of your person's, \
e.g. bip/coach, or of a relative's household, e.g. bip/cuisine__lea) and get its answer back in this same call. Write \
the message yourself: the point first, the precise question or what you need. Then answer your person with what it \
said.",
        "inputSchema": {"type": "object", "properties": {
            "target": {"type": "string", "description": "the other agent, as in your contacts (e.g. bip/coach) or by its name"},
            "message": {"type": "string", "description": "what you ask or tell it"},
        }, "required": ["target", "message"]},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
    })
}

/// The tool as the family's agent gets it (family_agent.rs, Alex 21/11): the agents of the member who made it that
/// they let it write to; what it then says, the whole family reads.
pub fn family_tool() -> Value {
    let mut tool = tool();
    tool["description"] = json!(
        "Ask one of the agents of the member of the family who made you, among those they let you write to (see your \
contacts, e.g. bip/vie), and get its answer back in this same call. Write the message yourself: the point first, the \
precise question, and for whom in the family. Then answer here with what it said: every member of the family reads it."
    );
    tool
}

/// Whether `agent` gets the tool: the rules let it write to another agent of this place, or of a linked household.
pub fn offered(config: &Config, agent: &str) -> bool {
    crate::agent_files::offered(config, agent) || {
        let world = crate::rules::World::of(config, false);
        crate::rules::writes_to_households(&world, &crate::rules::load(config), agent)
    }
}

/// The message as the target reads it: signed like a peer's (peers.rs `SIGNATURE`).
pub fn signed(sender: &AgentConfig, message: &str) -> String {
    format!("Message from {} (@{}): {}", crate::agent_files::shown(sender), sender.name, message.trim())
}

/// What the agent is told at the cap (agent_hops.rs): the label the person may see, then what to do.
pub const CAPPED: &str = "Limite d'échanges entre agents atteinte : réponds à la personne ou demande-lui de continuer.";

/// The card a call at the cap made for the person (agent_hops.rs): placed by the caller in the conversation the chain
/// started from.
#[derive(Debug, Clone, PartialEq)]
pub struct CapCard {
    /// The card's run id (`bip-hops-…`).
    pub run_id: String,
    /// The agent whose conversation shows it, and the turn there (None: its main conversation).
    pub agent: String,
    pub root_run: Option<String>,
}

/// One call's result, and the card it made.
pub struct Called {
    pub result: Dict,
    pub card: Option<CapCard>,
}

/// One call, for `agent` (`family`: it is the family's agent, asking for that member: agent_pairs.rs `FamilyAsk`).
pub async fn call(
    http: &reqwest::Client,
    hub: Option<std::sync::Arc<crate::runs::RunHub>>,
    config: &Config,
    agent: &AgentConfig,
    arguments: &Dict,
    family: Option<&FamilyAsk>,
) -> Called {
    let mut card = None;
    let result = answer(http, hub, config, agent, arguments, family, &mut card).await;
    Called { result, card }
}

async fn answer(
    http: &reqwest::Client,
    hub: Option<std::sync::Arc<crate::runs::RunHub>>,
    config: &Config,
    agent: &AgentConfig,
    arguments: &Dict,
    family: Option<&FamilyAsk>,
    card: &mut Option<CapCard>,
) -> Dict {
    let wanted = arguments.get("target").or_else(|| arguments.get("agent")).map(crate::py::py_str).unwrap_or_default();
    let message = arguments.get("message").or_else(|| arguments.get("text")).map(crate::py::py_str).unwrap_or_default();
    if message.trim().is_empty() {
        return tool_text("Give the message to send (`message`).", true);
    }
    if message.chars().count() > MAX_CHARS {
        return tool_text(&format!("The message is too long ({MAX_CHARS} characters at most): shorten it."), true);
    }
    let bare = wanted.trim().trim_start_matches("bip/").trim_start_matches('@').to_lowercase();
    if config.agent(Some(&bare)).is_none() && crate::households::split(&bare).is_some() {
        // the family's agent reaches only its creator's agents (rules.rs leaves it out of the households' ways)
        if let Some(f) = family {
            return tool_failure(
                "Another household's agents aren't reached from here.",
                &format!("I can only write to the agents of {} from here.", f.creator_or("the member who made me")),
            );
        }
        return to_household(http, hub, config, agent, &bare, &message, card).await;
    }
    let Some(target) = crate::agent_files::recipient(config, &wanted).cloned() else {
        let mine: Vec<String> = crate::agent_files::targets(config, &agent.name).map(|t| format!("bip/{t}")).collect();
        let allowed = if mine.is_empty() { "none yet".to_string() } else { mine.join(", ") };
        return tool_text(&format!("No agent {wanted:?} here. You may write to: {allowed}."), true);
    };
    if target.name == agent.name {
        return tool_text("That is you: name another agent.", true);
    }
    let shown = crate::agent_files::shown(&target).to_string();
    if !crate::agent_files::may_write(config, &agent.name, &target.name) {
        // the family's agent: only its creator allows it (the member asking can't)
        let plain = match family {
            Some(f) => format!(
                "I can't write to {shown}: {} can allow it in the app, under « Qui peut écrire à qui ».",
                f.creator_or("the member of the family who made me")
            ),
            None => format!("I can't write to {shown} yet: you can allow it in the app, under « Qui peut écrire à qui »."),
        };
        return crate::tool_answers::lifted_in_the_app(tool_failure(&format!("The person hasn't let you write to {shown}."), &plain));
    }
    // a path of the sender's own folders: the target sees none of them (not counted as a hop: nothing is sent)
    if let Some(path) = own_path(config, agent, &target, &message) {
        tracing::info!(agent = %agent.name, target = %target.name, "message to an agent: refused, a path of its own folders");
        return tool_text(
            &format!(
                "Nothing was sent: {shown} sees none of your folders, so {path} means nothing to it. Give it the file \
with {} first (it tells you the path {shown} reads it at), then write again with that path.",
                crate::agent_files::TOOL_NAME
            ),
            true,
        );
    }
    // the hop cap (agent_hops.rs): this turn's chain of messages between agents, since the person last spoke
    let run = hub.as_ref().and_then(|hub| hub.turn_of(&agent.name, TOOL_NAME)).map(|t| t.run_id);
    let chain = match crate::agent_hops::step(&agent.name, run.as_deref(), &target.name, crate::py::time_time()) {
        crate::agent_hops::Step::Go { chain, hop } => {
            tracing::info!(agent = %agent.name, target = %target.name, hop, "message to an agent");
            chain
        }
        crate::agent_hops::Step::Capped(chain) => {
            tracing::info!(agent = %agent.name, target = %target.name, chain = %chain.id, "agent hops: at the cap");
            *card = crate::agent_hops::card_for(config, &chain).map(|run_id| CapCard {
                run_id,
                agent: chain.root_agent.clone(),
                root_run: chain.root_run.clone(),
            });
            return capped();
        }
    };
    let asked =
        crate::agent_pairs::ask_from(http, hub, config, agent, &target, &message, ANSWER_SECONDS, Some(&chain), family)
            .await;
    // where its answer is kept when it comes later: the person opens the pair's conversation, the family can't
    let kept = match family {
        Some(f) => format!(
            "tell the family {shown} is on it: its answer will be kept in your conversation with {shown}, which {} can \
open in the app",
            f.creator_or("the member of the family who made you")
        ),
        None => format!("its answer will be in your conversation with {shown}, which your person can open"),
    };
    match asked {
        crate::agent_pairs::Answer::Text(reply) => {
            tracing::info!(agent = %agent.name, target = %target.name, "message to an agent answered");
            tool_text(&format!("{shown} (bip/{}) answered:\n{}", target.name, reply.trim()), false)
        }
        crate::agent_pairs::Answer::Empty => tool_text(&format!("{shown} got your message but answered nothing."), false),
        crate::agent_pairs::Answer::Waiting => tool_text(
            &format!(
                "{shown} got your message and is still on it (it may be waiting for your person's approval). Don't \
send it again: {kept}."
            ),
            false,
        ),
        crate::agent_pairs::Answer::Unreachable => unreachable(&shown),
        crate::agent_pairs::Answer::NotDelivered => not_delivered(&shown),
        crate::agent_pairs::Answer::Cycle => tool_text(
            &format!(
                "{shown} is itself waiting for your answer in this same exchange: it can't read another message now, \
and nothing was sent. Don't write to it: put what you want to tell it in your answer (end your turn with it), {shown} \
gets it."
            ),
            true,
        ),
        crate::agent_pairs::Answer::Queued => tool_text(
            &format!(
                "{shown} is busy with another exchange: your message waits its turn, nothing to do. Don't send it \
again: {kept}."
            ),
            false,
        ),
    }
}

/// `agent` asks `remote`, an agent of a linked household (`cuisine__lea`), through the two bridges
/// (household_asks.rs), where both households' rules allow it: checked here, then there.
async fn to_household(
    http: &reqwest::Client,
    hub: Option<std::sync::Arc<crate::runs::RunHub>>,
    config: &Config,
    agent: &AgentConfig,
    remote: &str,
    message: &str,
    card: &mut Option<CapCard>,
) -> Dict {
    let shown = crate::household_asks::shown_remote(config, remote);
    if !crate::rules::allows(config, &agent.name, remote) {
        return tool_failure(
            &format!("The person hasn't let you write to {shown}."),
            &format!("I can't write to {shown}: you can allow it in the app, under « Entre tes agents »."),
        );
    }
    if let Some(path) = own_folder_path(message) {
        tracing::info!(agent = %agent.name, target = %remote, "message to another household's agent: refused, a path of its own folders");
        return tool_text(
            &format!("Nothing was sent: {shown} sees none of your folders, so {path} means nothing to it (and another \
household's agents get no files): write what it needs in the message itself."),
            true,
        );
    }
    let run = hub.as_ref().and_then(|hub| hub.turn_of(&agent.name, TOOL_NAME)).map(|t| t.run_id);
    match crate::agent_hops::step(&agent.name, run.as_deref(), remote, crate::py::time_time()) {
        crate::agent_hops::Step::Go { hop, .. } => {
            tracing::info!(agent = %agent.name, target = %remote, hop, "message to another household's agent");
        }
        crate::agent_hops::Step::Capped(chain) => {
            tracing::info!(agent = %agent.name, target = %remote, chain = %chain.id, "agent hops: at the cap");
            *card = crate::agent_hops::card_for(config, &chain).map(|run_id| CapCard {
                run_id,
                agent: chain.root_agent.clone(),
                root_run: chain.root_run.clone(),
            });
            return capped();
        }
    }
    let kept = format!("its answer will be in your conversation with {shown}, which your person can open");
    match crate::household_asks::ask(http, config, agent, remote, message).await {
        crate::household_asks::Outcome::Answered(crate::agent_pairs::Answer::Text(reply)) => {
            tool_text(&format!("{shown} (bip/{remote}) answered:\n{}", reply.trim()), false)
        }
        crate::household_asks::Outcome::Answered(crate::agent_pairs::Answer::Empty) => {
            tool_text(&format!("{shown} got your message but answered nothing."), false)
        }
        crate::household_asks::Outcome::Answered(
            crate::agent_pairs::Answer::Waiting | crate::agent_pairs::Answer::Queued,
        ) => tool_text(
            &format!(
                "{shown} got your message and is still on it (it may be waiting for its person). Don't send it again: \
{kept}."
            ),
            false,
        ),
        crate::household_asks::Outcome::Answered(crate::agent_pairs::Answer::NotDelivered)
        | crate::household_asks::Outcome::Closed => not_delivered(&shown),
        crate::household_asks::Outcome::Answered(_) => unreachable(&shown),
        crate::household_asks::Outcome::Refused => tool_failure(
            &format!("{shown}'s person hasn't allowed your message (nothing was sent)."),
            &format!("{shown} can't get messages from me yet: its person has to allow it in their app."),
        ),
    }
}

/// A path of the sender's own folders in a message to another household's agent: it sees none of them.
fn own_folder_path(message: &str) -> Option<String> {
    static PATH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?:^|[\s«"'(\[`<])(/(?:envoi|partage|workspace|root|fichiers)/[^\s»"')\]`>]+)"#)
            .expect("valid regex")
    });
    PATH.captures(message).map(|c| c[1].trim_end_matches(['.', ',', ';', ':', '!', '?']).to_string())
}

/// A path in a message that only the sender sees: its outbox, shared folder or sandbox folders (token lab of
/// 2026-11-07: « dans /envoi/analyses.pdf », four times, the file never given; the owner's agents the same morning,
/// with a copy in its shared folder). Its shared folder still counts when `target` reads it (shares.rs: it is
/// `/lecture/<sender>` there), or, for a group discussion's folder, when both take part in it. Its received files
/// (`/fichiers/…`) are left alone: that is also where the target reads what it was given.
fn own_path(config: &Config, sender: &AgentConfig, target: &AgentConfig, message: &str) -> Option<String> {
    static PATH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?:^|[\s«"'(\[`<])(/(?:envoi|partage|workspace|root)/[^\s»"')\]`>]+)"#)
            .expect("valid regex")
    });
    let reads = crate::rules::reads_of(config).iter().any(|(r, s)| *r == target.name && *s == sender.name);
    let rooms = crate::rooms::mates(config, &sender.name).remove(&target.name).unwrap_or_default();
    let shared = |path: &str| {
        let Some(rest) = path.strip_prefix("/partage/") else { return false };
        let room = rest.strip_prefix(&format!("{}/", crate::rooms::SHARED_SUBFOLDER)).and_then(|r| r.split('/').next());
        reads || room.is_some_and(|id| rooms.contains(id))
    };
    PATH.captures_iter(message)
        .map(|c| c[1].trim_end_matches(['.', ',', ';', ':', '!', '?']).to_string())
        .find(|path| !shared(path))
}

/// The cap reached (agent_hops.rs): refused, and what to do instead.
fn capped() -> Dict {
    tool_failure(
        &format!(
            "{CAPPED}\n({} messages between agents since your person last spoke: the most there may be.) Don't write \
to another agent now and don't retry: answer your person with what you have, or ask them whether to go on. A \
« Continuer » card is in their conversation; once they tap it, you may write again.",
            crate::agent_hops::MAX_HOPS
        ),
        "Nous avons échangé plusieurs messages entre agents : je m'arrête là. Dis-moi si on continue.",
    )
}

/// A refusal of ours (agent_pairs.rs `Answer::NotDelivered`): said as it is, never as the other agent's silence.
fn not_delivered(shown: &str) -> Dict {
    tool_failure(
        &format!(
            "Your message did not reach {shown}: a problem on the app's side, not {shown}'s (it has been noted). \
Don't retry now."
        ),
        &format!("I couldn't pass your message on to {shown} just now: a problem on my side, not {shown}'s."),
    )
}

fn unreachable(shown: &str) -> Dict {
    tool_failure(
        &format!("{shown} didn't answer (its server is busy or down). Don't retry now."),
        &format!("{shown} isn't answering right now. Try again a bit later."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_is_signed_as_a_peers() {
        let sender = AgentConfig { name: "quotidien".into(), display_name: "Quotidien".into(), ..Default::default() };
        let text = signed(&sender, "  combien d'heures ?  ");
        assert_eq!(text, "Message from Quotidien (@quotidien): combien d'heures ?");
        assert_eq!(&crate::peers::SIGNATURE.captures(&text).unwrap()[1], "quotidien");
        assert_eq!(tool()["inputSchema"]["required"], json!(["target", "message"]));
    }

    #[test]
    fn a_path_of_the_senders_own_folders_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        let vie = AgentConfig { name: "vie".into(), display_name: "Vie".into(), ..Default::default() };
        let wellness = AgentConfig { name: "wellness".into(), display_name: "Wellness".into(), ..Default::default() };
        let caught = |text: &str| own_path(&config, &vie, &wellness, text);
        assert_eq!(caught("Le PDF est dans /envoi/analyses-2026.pdf."), Some("/envoi/analyses-2026.pdf".into()));
        let shared = Some("/partage/bloodwork/eurofins.pdf".to_string());
        assert_eq!(caught("copié dans « /partage/bloodwork/eurofins.pdf »"), shared);
        assert_eq!(caught("(/workspace/out/graph.png)"), Some("/workspace/out/graph.png".into()));
        // where the target reads what it was given, a web address, a mere mention of the folder: left alone
        assert_eq!(caught("tu le trouves ici : /fichiers/2026-11/092a3c29dbc1-eurofins.pdf"), None);
        assert_eq!(caught("voir https://exemple.fr/envoi/x.pdf"), None);
        assert_eq!(caught("mon dossier /partage est à moi"), None);
    }

    #[test]
    fn the_cap_is_a_clear_error() {
        let refused = capped();
        assert_eq!(refused["isError"], json!(true));
        let text = refused["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Limite d'échanges entre agents atteinte : réponds à la personne"), "{text}");
        assert!(text.contains("4 messages between agents") && text.contains("« Continuer »"), "{text}");
    }
}
