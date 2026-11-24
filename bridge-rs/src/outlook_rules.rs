//! Standing mail rules on an Outlook mailbox (Alex 24/11, « comme pour les autres boîtes »): the IMAP boxes' rules
//! (mail_rules.rs) — the same `mail_bulk(rule: true)` proposal, the same card (`bip-mailrule-`, the generic one: the
//! person's level lets it through as an IMAP box's, `settle_new`), the same store, page (`GET /v1/mail/rules`), stop,
//! « Annuler » of the last run and weekly line — whose mail side is Graph's instead of IMAP's:
//!
//! - a rule's `account` is the Outlook mailbox's id (outlook.rs `mailbox_key`, `outlook-<n>`), its `folder` the inbox
//!   (by Graph's name: new mail is only watched there), its `dest` the destination's name (resolved at each run: its
//!   archive, its Deleted Items, or a folder by its name);
//! - on « Oui » the mails already there are filed by a task of the bridge (`start`), then each new matching mail as it
//!   comes: outlook_watch.rs looks at the inbox and hands its new mails' rows to `sort_new` — no model, no card;
//! - a rule chooses by a sender or a domain only (Outlook's mailing lists aren't read here); never a flagged mail, a
//!   draft, the person's own mail, Sent Items or Drafts; one rule per mail, a mail a rule filed never filed again (by
//!   Graph's immutable id: put back by the undo or the person, it stays where they put it);
//! - « Annuler »: the mails still where the last run put them go back to the inbox, those it marked read are unread.
//!
//! Not a Python module.

use serde_json::Value;

use crate::config::Config;
use crate::drives::Api;
use crate::mail_rules::{self, Match, Rule, Run};
use crate::mail_tidy::{self, Action, Row, Selector};
use crate::outlook::{self, AccountId};
use crate::outlook_mail;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::{py, Dict};

/// The account of a rule's (or a rule card's) mailbox. None: an IMAP box's, or no such rule.
pub fn account_of(rule: &Rule) -> Option<AccountId> {
    outlook::key_account(&rule.account)
}

fn find(config: &Config, id: &str) -> Option<Rule> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    mail_rules::rules_of(config, None).into_iter().chain(mail_rules::cards_of(config, None)).find(|r| r.id == id)
}

/// Whether rule (or card) `id` is an Outlook mailbox's.
pub fn is_ours(config: &Config, id: &str) -> bool {
    find(config, id).as_ref().and_then(account_of).is_some()
}

/// The Outlook account of `agent`'s rule `id`. None: not one of its Outlook rules.
pub fn account_of_rule(config: &Config, agent: &str, id: &str) -> Option<AccountId> {
    find(config, id).filter(|r| r.agent == agent).as_ref().and_then(account_of)
}

/// `mail_bulk` with `rule: true`, or `rules`, `stop_rule`, `undo_rule`, on Outlook account `account`.
pub async fn call(config: &Config, agent: &str, account: AccountId, api: &Api, word: &str, arguments: &Dict) -> Dict {
    let rule_id = tk::strip(&tk::str_or_empty(arguments.get("rule_id"))).to_string();
    match word {
        "rules" => {
            let lines = mail_rules::status_lines(config, agent);
            let text = if lines.is_empty() { "No mail rule.".to_string() } else { lines.join("\n") };
            return tool_text(&text, false);
        }
        "stop_rule" => {
            return match mail_rules::stop(config, Some(agent), &rule_id) {
                Ok(Some(r)) => tool_text(
                    &format!("Rule {} stopped: new mail stays where it lands. What it filed stays filed.", r.id),
                    false,
                ),
                Ok(None) => tool_text("no such rule (action rules lists them)", true),
                Err(e) => tool_text(&e.to_string(), true),
            };
        }
        "undo_rule" => {
            return match undo_last(config, Some(agent), &rule_id).await {
                Ok(n) => tool_text(&format!("{n} e-mails put back as they were. The rule stays on for new mail."), false),
                Err(e) => tool_text(&e, true),
            };
        }
        _ => {}
    }
    // several senders in one call: one rule card each (as mail_rules.rs `call`)
    let groups: Vec<Value> = arguments.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();
    let calls: Vec<Dict> = if groups.len() > 1 {
        groups
            .iter()
            .take(mail_rules::MAX_PER_AGENT)
            .map(|g| {
                let mut one = arguments.clone();
                one.insert("groups".into(), Value::Array(vec![g.clone()]));
                one
            })
            .collect()
    } else {
        vec![arguments.clone()]
    };
    let mut texts = Vec::new();
    let mut made = 0;
    for one in &calls {
        match propose(config, agent, account, api, one).await {
            Ok(card) => {
                made += 1;
                texts.push(format!(
                    "Rule card {} made: {} ({} already there). It waits in the person's approvals; nothing changes \
before their Oui. Tell them in one sentence.",
                    card.id,
                    mail_rules::line("en", &card, true),
                    card.existing
                ));
            }
            Err(why) if calls.len() > 1 => texts.push(format!("{}: {why}", py::py_str(one.get("groups").unwrap_or(&Value::Null)))),
            Err(why) => texts.push(why),
        }
    }
    tool_text(&texts.join("\n"), made == 0)
}

/// One rule's card (`mail_bulk rule: true` naming one sender or domain), on the inbox. Err: plain words for the agent.
async fn propose(config: &Config, agent: &str, account: AccountId, api: &Api, arguments: &Dict) -> Result<Rule, String> {
    let parsed = Action::parse(&tk::str_or_empty(arguments.get("action")))?;
    let wants_read = arguments.get("mark_read").is_some_and(py::truthy);
    let (action, mark_read) = match parsed {
        Action::MarkRead => (None, true),
        Action::Archive | Action::Trash | Action::Move(_) => (Some(parsed), wants_read),
        _ => return Err("action: move:<folder>, archive, mark_read or trash".into()),
    };
    let selector = Selector::from_args(arguments);
    if !selector.list_id.is_empty() || selector.groups.iter().any(|g| g.starts_with("list:")) {
        return Err(format!(
            "{}'s mailing lists aren't read here: name a sender (from) or a domain",
            outlook::LABEL
        ));
    }
    let matcher = Match::from_selector(&selector)?;
    let wanted = tk::str_or_empty(arguments.get("folder"));
    let inbox = outlook_mail::inbox(api).await.map_err(|f| outlook::words(&f))?;
    if !wanted.trim().is_empty() && outlook_mail::well_known(&wanted) != Some("inbox") {
        return Err(format!("a rule on {} files the new mail of the inbox: leave out `folder`", outlook::LABEL));
    }
    let dest = match &action {
        None => String::new(),
        Some(a) => outlook_mail::destination_for(api, a, &inbox).await?.1,
    };
    let key = outlook::mailbox_key(account);
    let rules = mail_rules::rules_of(config, None);
    if let Some(r) = rules.iter().find(|r| r.account == key && r.matcher == matcher) {
        return Err(format!("there is already a rule for {} (rule {}): stop it first to change it", matcher.label(), r.id));
    }
    if rules.iter().filter(|r| r.agent == agent).count() >= mail_rules::MAX_PER_AGENT {
        return Err(format!("at most {} rules: stop one first", mail_rules::MAX_PER_AGENT));
    }
    let scanned = outlook_mail::scan(config, account, api, &inbox, false).await.map_err(|f| outlook::words(&f))?;
    let own = outlook_mail::own(config, account);
    let mut chosen: Vec<&Row> = scanned
        .rows
        .iter()
        .filter(|r| !mail_tidy::protected(r, &own) && matcher.matches(r))
        .filter(|r| action.is_some() || !r.seen)
        .collect();
    chosen.sort_by_key(|r| std::cmp::Reverse(r.date.clone()));
    let defused = |s: &str, n: usize| crate::archives::defused(tk::head(s, n));
    let sample =
        chosen.iter().take(10).map(|r| format!("{} · {} · {}", r.date, defused(&r.from, 120), defused(&r.subject, 70))).collect();
    let card = Rule {
        id: py::token_hex(6),
        agent: agent.to_string(),
        account: key,
        mailbox: outlook::label(config, account),
        folder: inbox.name.clone(),
        matcher,
        action,
        mark_read,
        dest,
        existing: chosen.len(),
        sample,
        status: "asked".into(),
        at: py::time_time(),
        ..Rule::default()
    };
    mail_rules::add_card(config, card.clone()).map_err(|e| e.to_string())?;
    tracing::info!(agent, card = %card.id, existing = card.existing, "outlook: mail rule card");
    Ok(card)
}

/// The rule's action on these rows of the inbox (already chosen: matching, not the person's own), the flags read again
/// just before. The run.
async fn apply_rows(api: &Api, rule: &Rule, inbox: &str, rows: &[Row]) -> Result<Run, String> {
    let mut run = Run { at: py::time_time(), ..Run::default() };
    let rows: Vec<&Row> = rows.iter().filter(|r| !r.draft && !r.flagged && !rule.filed_ids.contains(&r.message_id)).collect();
    if rows.is_empty() {
        return Ok(run);
    }
    // flagged since the rows were read: never
    let flagged = outlook_mail::flagged(api, inbox).await;
    let rows: Vec<&Row> = rows.into_iter().filter(|r| !flagged.contains(&r.message_id)).collect();
    if rule.mark_read {
        let unread: Vec<String> = rows.iter().filter(|r| !r.seen).map(|r| r.message_id.clone()).collect();
        run.marked_ids = outlook_mail::act_on(api, &Action::MarkRead, "", &unread).await;
    }
    match &rule.action {
        Some(action) => {
            let folder = outlook_mail::inbox(api).await.map_err(|f| outlook::words(&f))?;
            let moved = match action {
                Action::Move(_) | Action::Archive | Action::Trash => action.clone(),
                _ => return Err("this rule's action can't be done".into()),
            };
            let (dest, _) = outlook_mail::destination_for(api, &moved, &folder).await?;
            let ids: Vec<String> = rows.iter().map(|r| r.message_id.clone()).collect();
            run.moved_ids = outlook_mail::act_on(api, &moved, &dest, &ids).await;
            run.count = run.moved_ids.len();
            if run.moved_ids.len() < ids.len() {
                run.refused = format!("{} of them could not be filed (gone or changed)", ids.len() - run.moved_ids.len());
            }
        }
        None => run.count = run.marked_ids.len(),
    }
    Ok(run)
}

/// The mails already there of rule `id`, just turned on, filed by a task of the bridge (once at a time). From
/// mail_rules.rs `start`, on the runtime it runs in (a blocking thread of it).
pub fn start(config: &Config, id: &str) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!(rule = id, "outlook: a rule's mails already there not filed (no runtime)");
        return;
    };
    let (config, id) = (config.clone(), id.to_string());
    handle.spawn(async move {
        if let Err(why) = run_existing(&config, &id).await {
            tracing::warn!(rule = %id, why = %why, "outlook: a rule's mails already there not filed yet");
            let _ = mail_rules::record_run(&config, &id, Run::default(), &why);
        }
    });
}

async fn signed_in_for(config: &Config, rule: &Rule) -> Result<(AccountId, Api), String> {
    let account = account_of(rule).ok_or("not an Outlook rule")?;
    if !outlook::parts_of(config, &rule.agent, account).is_some_and(|p| p.mail) {
        return Err("this Outlook mailbox is no longer given to the agent".into());
    }
    let api = outlook::signed_in(config, account).await.map_err(|f| outlook::words(&f))?;
    Ok((account, api))
}

async fn run_existing(config: &Config, id: &str) -> Result<(), String> {
    let Some(rule) = mail_rules::rules_of(config, None).into_iter().find(|r| r.id == id) else { return Ok(()) };
    if rule.existing_done {
        return Ok(());
    }
    let (account, api) = signed_in_for(config, &rule).await?;
    let inbox = outlook_mail::inbox(&api).await.map_err(|f| outlook::words(&f))?;
    let scanned = outlook_mail::scan(config, account, &api, &inbox, true).await.map_err(|f| outlook::words(&f))?;
    let own = outlook_mail::own(config, account);
    let rows: Vec<Row> =
        scanned.rows.iter().filter(|r| !mail_tidy::protected(r, &own) && rule.matcher.matches(r)).cloned().collect();
    let run = apply_rows(&api, &rule, &inbox.id, &rows).await?;
    outlook_mail::forget_scans(account);
    tracing::info!(rule = %rule.id, agent = %rule.agent, count = run.count, "outlook: a rule's mails already there filed");
    let refused = run.refused.clone();
    mail_rules::record_run(config, id, run, &refused).map_err(|e| e.to_string())?;
    mail_rules::existing_filed(config, id).map_err(|e| e.to_string())?;
    Ok(())
}

/// New mails of account `account`'s inbox (their rows, outlook_watch.rs) sorted by its rules: one rule per mail, the
/// oldest rule first. The number filed.
pub async fn sort_new(config: &Config, account: AccountId, api: &Api, inbox: &str, rows: &[Row]) -> usize {
    let key = outlook::mailbox_key(account);
    let rules: Vec<Rule> = mail_rules::rules_of(config, None).into_iter().filter(|r| r.account == key).collect();
    if rules.is_empty() || rows.is_empty() {
        return 0;
    }
    let own = outlook_mail::own(config, account);
    let mut taken = std::collections::HashSet::new();
    let mut filed = 0;
    for rule in rules {
        if !outlook::parts_of(config, &rule.agent, account).is_some_and(|p| p.mail) {
            continue;
        }
        let mine: Vec<Row> = rows
            .iter()
            .filter(|r| !taken.contains(&r.message_id) && !mail_tidy::protected(r, &own) && rule.matcher.matches(r))
            .filter(|r| !rule.filed_ids.contains(&r.message_id))
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        taken.extend(mine.iter().map(|r| r.message_id.clone()));
        match apply_rows(api, &rule, inbox, &mine).await {
            Ok(run) => {
                filed += run.count;
                tracing::info!(rule = %rule.id, agent = %rule.agent, count = run.count, "outlook: new mail filed by a rule");
                let refused = run.refused.clone();
                let _ = mail_rules::record_run(config, &rule.id, run, &refused);
            }
            Err(why) => {
                tracing::warn!(rule = %rule.id, why = %why, "outlook: new mail not filed");
                let _ = mail_rules::record_run(config, &rule.id, Run::default(), &why);
            }
        }
    }
    if filed > 0 {
        outlook_mail::forget_scans(account);
    }
    filed
}

/// The undo of an Outlook rule's last run: the mails still where it put them back in the inbox, those it marked read
/// unread again. The number put back.
pub async fn undo_last(config: &Config, agent: Option<&str>, id: &str) -> Result<usize, String> {
    let Some(rule) = mail_rules::rules_of(config, None).into_iter().find(|r| r.id == id && agent.is_none_or(|a| a == r.agent))
    else {
        return Err("no such rule (mail_bulk action rules lists them)".into());
    };
    let Some(run) = rule.last.clone().filter(|r| !r.undone && (r.count > 0 || !r.marked_ids.is_empty())) else {
        return Err("nothing to undo: its last run was undone already, or it filed nothing yet".into());
    };
    let (account, api) = signed_in_for(config, &rule).await?;
    let inbox = outlook_mail::inbox(&api).await.map_err(|f| outlook::words(&f))?;
    let mut back = 0;
    if let Some(action) = rule.action.as_ref().filter(|_| !run.moved_ids.is_empty()) {
        let (dest, _) = outlook_mail::destination_for(&api, action, &inbox).await?;
        let still = outlook_mail::still_there(&api, &run.moved_ids, &dest).await;
        back = outlook_mail::act_on(&api, &Action::Move(inbox.id.clone()), &inbox.id, &still).await.len();
    }
    if !run.marked_ids.is_empty() {
        back = back.max(outlook_mail::act_on(&api, &Action::MarkUnread, "", &run.marked_ids).await.len());
    }
    outlook_mail::forget_scans(account);
    mail_rules::last_undone(config, id).map_err(|e| e.to_string())?;
    tracing::info!(rule = %id, back, "outlook: a rule's last run undone");
    Ok(back)
}
