//! Outlook's mailbox (outlook.rs) through the mail tools the agents already have (mailbox.rs, mail_tidy.rs): a
//! mailbox named « Outlook » (or by its address) beside the IMAP ones. The agent reads, searches, drafts and tidies
//! there as in any box; the bridge calls Microsoft Graph itself (`/me/mailFolders`, `/me/messages`):
//!
//!     mail_search      `$search` (KQL: the words, `from:`, `received>=`) or the newest, in a folder (Inbox by default)
//!     mail_read        one mail's text (Graph gives it as text), its headers, its attachments' names
//!     mail_attachment  one attachment's bytes (`$value`), stored in the agent's files as an IMAP box's (25 MB at most)
//!     mail_draft       a draft in Outlook's Drafts (`POST /me/messages`, a reply by `createReply` then `PATCH`), to
//!                      the address mailbox.rs' `reply_address` allows; one draft per mail answered. Never sent: the
//!                      bridge asks no `Mail.Send`
//!     mail_overview    the newest headers counted by sender domain (mail_tidy.rs' overview)
//!     mail_bulk        archive, mark_read, move:<folder>, trash (Deleted Items) by mail_tidy.rs' selectors, by the
//!                      person's approval level as an IMAP box's (1.1, Alex 24/11: approval_level.rs `at_once`):
//!                      « Seulement le critique » or their « Toujours » for that kind lets it through at once, its line
//!                      in « Validé pour toi » with « Annuler » (`Undo::OutlookJob`); else ONE card (person_ok.rs,
//!                      `bip-ok-`, kinds `mail_archive` / `mail_mark_read` / `mail_move` / `mail_trash`, « Toujours
//!                      autoriser » but for the trash) before anything moves; the trash always asks. Then done here,
//!                      `MAX_PER_CARD` at most, kept `UNDO_DAYS` for its undo (`mail_bulk(action: "undo", job)`).
//!                      `rule: true`, `rules`, `stop_rule`, `undo_rule`: the IMAP boxes' standing rules (outlook_rules.rs)
//!
//! Ids: `outlook|<folder>|<n>` (outlook.rs' short numbers for Graph's immutable ids, `is_mail_id` shaped). What a mail
//! says is someone else's text: between mailbox.rs' markers, defused (archives.rs). Flagged mails, drafts and the
//! person's own mails are never tidied; Sent Items and Drafts are never a source nor a destination. Several accounts
//! (outlook.rs): a call is for the account of the id it names, else the one its `mailbox` names (its address), else
//! the agent's first one; the answers name each one when there are several.
//!
//!     <data>/outlook/drafts.json   the drafts made in answer to a mail (30 days)
//!     <data>/outlook/jobs.json     the tidyings done, for their undo (30 days)
//!
//! Not a Python module.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::drives::{Api, Fault};
use crate::mail_tidy::{Action, Job, Row, Scan, Selector};
use crate::outlook::{self, AccountId, LABEL, PREFER_IMMUTABLE};
use crate::tools_knowledge::{self as tk, tool_text};
use crate::{py, Dict};

/// The newest headers an overview or a tidying reads (Graph pages of 500).
pub const SCAN_MAX: usize = 2000;
const PAGE: &str = "500";
/// Mails one card tidies at most (the rest: another call).
pub const MAX_PER_CARD: usize = 500;
const CACHE_SECONDS: f64 = 600.0;
const UNDO_DAYS: f64 = 30.0;
const DRAFTS_DAYS: f64 = 30.0;
/// Graph calls at once when tidying.
const AT_ONCE: usize = 4;
const PREFER_TEXT: &str = "outlook.body-content-type=\"text\"";

/// Graph's names for its own folders, and what people call them.
const WELL_KNOWN: [(&str, &[&str]); 6] = [
    ("inbox", &["inbox", "boîte de réception", "boite de reception", "réception", "posteingang", "bandeja de entrada"]),
    ("drafts", &["drafts", "brouillons", "entwürfe", "borradores"]),
    ("sentitems", &["sent", "sent items", "sentitems", "éléments envoyés", "envoyés", "gesendete elemente", "elementos enviados"]),
    ("deleteditems", &["trash", "deleted", "deleted items", "deleteditems", "corbeille", "éléments supprimés", "gelöschte elemente", "elementos eliminados", "papelera"]),
    ("archive", &["archive", "archives", "archiv", "archivo"]),
    ("junkemail", &["junk", "spam", "junkemail", "junk email", "courrier indésirable", "indésirables", "junk-e-mail", "correo no deseado"]),
];

// -- which calls are Outlook's -----------------------------------------------------------------------------------------

/// Whether a mail id is one of Outlook's (`outlook|…`).
pub fn is_ours(id: &str) -> bool {
    id.trim().to_lowercase().starts_with(&format!("{}|", outlook::BOX_ID))
}

/// Whether `wanted` (a `mailbox` argument) names Outlook: « Outlook », « … (Outlook) », an account's address or its
/// mailbox's id (`outlook-2`).
pub fn names_us(config: &Config, wanted: &str) -> bool {
    let wanted = wanted.trim().to_lowercase();
    wanted == "outlook"
        || wanted.ends_with("(outlook)")
        || outlook::key_account(&wanted).is_some()
        || outlook::all(config).iter().any(|a| a.link.account.as_deref().is_some_and(|x| x.trim().to_lowercase() == wanted))
}

/// The account a `mailbox` argument names among `agent`'s with mail: its address, its mailbox's id, « Outlook (its
/// address) »; « Outlook » or nothing: the first one. None: none of its.
pub fn named_account(config: &Config, agent: &str, wanted: &str) -> Option<AccountId> {
    let mine = outlook::accounts_with(config, agent, "mail");
    let wanted = wanted.trim().to_lowercase();
    let inner = wanted.strip_prefix("outlook (").and_then(|w| w.strip_suffix(')')).unwrap_or(&wanted).trim().to_string();
    let by_address = mine.iter().find(|a| a.link.account.as_deref().is_some_and(|x| x.trim().to_lowercase() == inner));
    if let Some(found) = by_address {
        return Some(found.id);
    }
    if let Some(id) = outlook::key_account(&inner) {
        return mine.iter().find(|a| a.id == id).map(|a| a.id);
    }
    mine.first().map(|a| a.id)
}

/// Whether this mail tool's call is for Outlook: `agent` uses its mail, and the call names it (an id of its, its name)
/// or the agent has no other mailbox (`imap_boxes`).
pub fn takes(config: &Config, agent: &str, name: &str, arguments: &Dict, imap_boxes: usize) -> bool {
    if !outlook::mail_for(config, agent) {
        return false;
    }
    if imap_boxes == 0 {
        return true;
    }
    let arg = |k: &str| tk::str_or_empty(arguments.get(k)).trim().to_string();
    match name {
        "mail_read" | "mail_attachment" => is_ours(&arg("id")),
        "mail_draft" if !arg("reply_to_id").is_empty() => is_ours(&arg("reply_to_id")),
        "mail_bulk" if arg("job").starts_with(JOB_PREFIX) => true,
        // one of its Outlook mailbox's standing rules (outlook_rules.rs)
        "mail_bulk" if crate::outlook_rules::account_of_rule(config, agent, &arg("rule_id")).is_some() => true,
        _ => names_us(config, &arg("mailbox")),
    }
}

/// A line for the IMAP boxes' answers when the agent also has Outlook (it would not know otherwise): its mailboxes'
/// names (`but`: the one this answer is of, left out).
pub fn also_line(config: &Config, agent: &str, but: Option<AccountId>) -> String {
    let mine = outlook::accounts_with(config, agent, "mail");
    let names: Vec<String> = mine
        .iter()
        .filter(|a| Some(a.id) != but)
        .map(|a| match a.link.account.as_deref().filter(|_| mine.len() > 1) {
            Some(address) => format!("mailbox \"{}\"", crate::archives::defused(address)),
            None => format!("mailbox \"{LABEL}\""),
        })
        .collect();
    match (names.as_slice(), but.is_some()) {
        ([], _) => String::new(),
        (_, true) => format!("(Your person's other {LABEL} mailbox too: {}.)", names.join(", ")),
        (_, false) => format!("(Your person's {LABEL} mailbox too: {}.)", names.join(", ")),
    }
}

// -- folders -----------------------------------------------------------------------------------------------------------

/// A mail folder of the account.
#[derive(Debug, Clone, PartialEq)]
pub struct MailFolder {
    pub id: String,
    /// As the person reads it (Graph's `displayName`; a sub-folder: « Parent/Name »).
    pub name: String,
    pub total: usize,
    /// Graph's well-known name when it was opened by one (`inbox`, `archive`…).
    pub key: String,
}

impl MailFolder {
    /// Its part of the mails' ids: the well-known name, else its name without spaces (an id is one word; the folder
    /// part is only read by the agent, the number finds the mail).
    pub fn token(&self) -> String {
        if !self.key.is_empty() {
            return self.key.clone();
        }
        id_part(&self.name)
    }
}

fn id_part(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("_").replace('|', "/")
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn folder_of(v: &Value, parent: Option<&str>) -> MailFolder {
    // the person's own names, defused all the same wherever they are said (review R1)
    let name = crate::archives::defused(&text(v, "displayName").replace(['|', '\n', '\r'], " "));
    MailFolder {
        id: text(v, "id"),
        name: match parent {
            Some(p) => format!("{p}/{name}"),
            None => name,
        },
        total: v.get("totalItemCount").and_then(Value::as_u64).unwrap_or(0) as usize,
        key: String::new(),
    }
}

/// Graph's well-known name a person's word means (« Brouillons » → drafts).
pub fn well_known(wanted: &str) -> Option<&'static str> {
    let low = wanted.trim().to_lowercase();
    WELL_KNOWN.iter().find(|(_, names)| names.contains(&low.as_str()) || low == names[0]).map(|(id, _)| *id)
}

async fn known_folder(api: &Api, id: &str) -> Result<MailFolder, Fault> {
    let url = format!("{}/me/mailFolders/{id}", outlook::base(api));
    let v = outlook::fetch(api, &url, &[("$select", "id,displayName,totalItemCount".to_string())], &[PREFER_IMMUTABLE])
        .await?;
    Ok(MailFolder { key: id.to_string(), ..folder_of(&v, None) })
}

/// The folders: the top ones and their sub-folders (one level down).
pub async fn folders(api: &Api) -> Result<Vec<MailFolder>, Fault> {
    let base = outlook::base(api);
    let select = ("$select", "id,displayName,totalItemCount,childFolderCount".to_string());
    let (top, _) = outlook::pages(api, &format!("{base}/me/mailFolders"), &[("$top", "100".into()), select.clone()],
                                  &[PREFER_IMMUTABLE], 200).await?;
    let mut out = Vec::new();
    for v in &top {
        let parent = folder_of(v, None);
        out.push(parent.clone());
        if v.get("childFolderCount").and_then(Value::as_u64).unwrap_or(0) > 0 && out.len() < 300 {
            let url = format!("{base}/me/mailFolders/{}/childFolders", parent.id);
            if let Ok((children, _)) = outlook::pages(api, &url, &[("$top", "100".into()), select.clone()],
                                                      &[PREFER_IMMUTABLE], 100).await {
                out.extend(children.iter().map(|c| folder_of(c, Some(&parent.name))));
            }
        }
    }
    Ok(out)
}

/// The folder `wanted` names: Inbox when empty, a well-known one by any of its names, else one by its path or its
/// last name when only one folder has it. Err: plain words (the folders listed), or Graph's fault.
pub async fn open_folder(api: &Api, wanted: &str) -> Result<MailFolder, Result<String, Fault>> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return known_folder(api, "inbox").await.map_err(Err);
    }
    if let Some(id) = well_known(wanted) {
        return match known_folder(api, id).await {
            Ok(f) => Ok(f),
            Err(Fault::NotFound) => Err(Ok(format!("this {LABEL} mailbox has no « {wanted} » folder"))),
            Err(fault) => Err(Err(fault)),
        };
    }
    let all = folders(api).await.map_err(Err)?;
    let low = wanted.to_lowercase();
    let by_path: Vec<&MailFolder> = all.iter().filter(|f| f.name.to_lowercase() == low).collect();
    let by_last: Vec<&MailFolder> =
        all.iter().filter(|f| f.name.rsplit('/').next().unwrap_or("").to_lowercase() == low).collect();
    match (by_path.as_slice(), by_last.as_slice()) {
        ([one], _) | ([], [one]) => Ok((*one).clone()),
        _ => {
            let names: Vec<String> = all.iter().take(60).map(|f| f.name.clone()).collect();
            Err(Ok(format!("no single folder « {wanted} » in {LABEL}; its folders: {}", names.join(", "))))
        }
    }
}

// -- reading -----------------------------------------------------------------------------------------------------------

fn defused(text: &str, max: usize) -> String {
    crate::archives::defused(tk::head(&text.split_whitespace().collect::<Vec<_>>().join(" "), max))
}

/// « Name <address> » of a Graph recipient.
pub fn person_of(v: &Value) -> String {
    let address = v.pointer("/emailAddress/address").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let name = v.pointer("/emailAddress/name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    match (name.is_empty() || name.eq_ignore_ascii_case(&address), address.is_empty()) {
        (_, true) => name,
        (true, false) => address,
        (false, false) => format!("{name} <{address}>"),
    }
}

fn people_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().map(person_of).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// A mail's id for the agents (its number names its account).
pub fn mail_ref(config: &Config, account: AccountId, folder: &str, graph_id: &str) -> String {
    format!("{}|{}|{}", outlook::BOX_ID, id_part(folder), outlook::short_id(config, account, graph_id))
}

/// The account and Graph's id of an agent's mail id. None: not one of Outlook's known ones.
pub fn graph_id(config: &Config, id: &str) -> Option<(AccountId, String)> {
    let mut parts = id.trim().split('|');
    let (boxed, _, n) = (parts.next()?, parts.next()?, parts.next()?);
    if !boxed.eq_ignore_ascii_case(outlook::BOX_ID) || parts.next().is_some() {
        return None;
    }
    outlook::long_id(config, n.trim().parse().ok()?)
}

fn unknown_id(given: &str) -> Dict {
    tool_text(
        &format!(
            "unknown e-mail id « {} »: give the id exactly as mail_search lists it, « outlook|folder|n » (e.g. \
outlook|Inbox|12)",
            tk::head(given, 80)
        ),
        true,
    )
}

/// The KQL `$search` of the words, a sender and a period (no quote of the agent's can close it).
pub fn kql(query: &str, sender: &str, since: Option<&str>) -> String {
    let clean = |s: &str| s.replace(['"', '\\'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts = Vec::new();
    if !clean(query).is_empty() {
        parts.push(clean(query));
    }
    if !clean(sender).is_empty() {
        parts.push(format!("from:{}", clean(sender).replace(' ', "")));
    }
    if let Some(day) = since {
        parts.push(format!("received>={day}"));
    }
    format!("\"{}\"", parts.join(" "))
}

fn since_day(days: Option<i64>) -> Option<String> {
    let days = days.filter(|d| *d > 0)?.min(36500);
    Some((chrono::Utc::now().date_naive() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string())
}

async fn search(config: &Config, account: AccountId, api: &Api, arguments: &Dict) -> Dict {
    let label = outlook::label(config, account);
    let arg = |k: &str| tk::str_or_empty(arguments.get(k));
    let folder = match open_folder(api, &arg("folder")).await {
        Ok(f) => f,
        Err(Ok(said)) => return tool_text(&said, true),
        Err(Err(fault)) => return outlook::fault_result(&fault, "I can't reach your Outlook mail just now."),
    };
    let limit = tk::as_py_int(arguments.get("limit")).unwrap_or(10).clamp(1, crate::mailbox::SEARCH_MAX as i64);
    let (query, sender) = (tk::strip(&arg("query")).to_string(), tk::strip(&arg("sender")).to_string());
    let since = since_day(tk::as_py_int(arguments.get("days")));
    let mut params: Vec<(&str, String)> = vec![
        ("$top", limit.to_string()),
        ("$select", "id,subject,from,receivedDateTime,bodyPreview,internetMessageId".to_string()),
    ];
    if !query.is_empty() || !sender.is_empty() {
        params.push(("$search", kql(&query, &sender, since.as_deref())));
    } else {
        params.push(("$orderby", "receivedDateTime desc".to_string()));
        if let Some(day) = &since {
            params.push(("$filter", format!("receivedDateTime ge {day}T00:00:00Z")));
        }
    }
    let url = format!("{}/me/mailFolders/{}/messages", outlook::base(api), folder.id);
    let page = match outlook::fetch(api, &url, &params, &[PREFER_IMMUTABLE]).await {
        Ok(page) => page,
        Err(fault) => return outlook::fault_result(&fault, "I can't search your Outlook mail just now."),
    };
    let items = page.get("value").and_then(Value::as_array).cloned().unwrap_or_default();
    if items.is_empty() {
        let said = if query.is_empty() && sender.is_empty() && since.is_none() {
            format!("No e-mail in {label} › « {} ».", folder.name)
        } else {
            format!("No e-mail found in {label} › « {} ».", folder.name)
        };
        return tool_text(&said, false);
    }
    let ids: Vec<String> = items.iter().map(|v| text(v, "id")).collect();
    let shorts = outlook::short_ids(config, account, &ids);
    let lines: Vec<String> = items
        .iter()
        .zip(shorts)
        .map(|(v, n)| {
            format!(
                "- id {}|{}|{n} · {} · from {} · {}\n  {}",
                outlook::BOX_ID,
                folder.token(),
                text(v, "receivedDateTime"),
                defused(&person_of(v.get("from").unwrap_or(&Value::Null)), 200),
                defused(&text(v, "subject"), 300),
                defused(&text(v, "bodyPreview"), crate::mailbox::SNIPPET_CHARS),
            )
        })
        .collect();
    tool_text(&format!("{}\n<<<\n{label} › {}:\n{}\n>>>", crate::mailbox::UNTRUSTED, folder.name, lines.join("\n")), false)
}

/// One mail as mail_read gives it (`from`, `reply_to`, `to`, `cc`, … as header lines), for `reply_address` too.
async fn message(api: &Api, id: &str) -> Result<Value, Fault> {
    let url = format!("{}/me/messages/{id}", outlook::base(api));
    let select = "id,subject,from,toRecipients,ccRecipients,replyTo,receivedDateTime,internetMessageId,body,\
hasAttachments,isDraft,parentFolderId,flag";
    outlook::fetch(api, &url, &[("$select", select.to_string())], &[PREFER_IMMUTABLE, PREFER_TEXT]).await
}

fn headers_of(v: &Value) -> Dict {
    let mut out = Dict::new();
    out.insert("from".into(), json!(person_of(v.get("from").unwrap_or(&Value::Null))));
    out.insert("reply_to".into(), json!(people_of(v, "replyTo")));
    out.insert("to".into(), json!(people_of(v, "toRecipients")));
    out.insert("cc".into(), json!(people_of(v, "ccRecipients")));
    out
}

async fn attachments(api: &Api, id: &str) -> Result<Vec<Value>, Fault> {
    let url = format!("{}/me/messages/{id}/attachments", outlook::base(api));
    let (items, _) =
        outlook::pages(api, &url, &[("$select", "id,name,size,contentType,isInline".to_string())], &[PREFER_IMMUTABLE], 100)
            .await?;
    Ok(items
        .into_iter()
        .filter(|a| {
            text(a, "@odata.type").ends_with("fileAttachment") && !a.get("isInline").and_then(Value::as_bool).unwrap_or(false)
        })
        .collect())
}

/// Graph's id of a mail id of `account` (another account's: unknown).
fn graph_of(config: &Config, account: AccountId, id: &str) -> Option<String> {
    graph_id(config, id).filter(|(of, _)| *of == account).map(|(_, long)| long)
}

async fn read(config: &Config, account: AccountId, api: &Api, id: &str) -> Dict {
    let Some(graph) = graph_of(config, account, id) else { return unknown_id(id) };
    let v = match message(api, &graph).await {
        Ok(v) => v,
        Err(fault) => return outlook::fault_result(&fault, "I can't read that Outlook mail just now."),
    };
    let h = headers_of(&v);
    let g = |k: &str| h.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut head = format!("From: {}\n", defused(&g("from"), 300));
    if !g("reply_to").is_empty()
        && crate::mailbox::addresses(&g("reply_to")) != crate::mailbox::addresses(&g("from"))
    {
        head += &format!("Reply-To: {}\n", defused(&g("reply_to"), 300));
    }
    head += &format!("To: {}\n", defused(&g("to"), 600));
    if !g("cc").is_empty() {
        head += &format!("Cc: {}\n", defused(&g("cc"), 600));
    }
    head += &format!(
        "Date: {}\nSubject: {}\nMessage-ID: {}\n",
        text(&v, "receivedDateTime"),
        defused(&text(&v, "subject"), 300),
        defused(&text(&v, "internetMessageId"), 300)
    );
    if v.get("hasAttachments").and_then(Value::as_bool).unwrap_or(false) {
        if let Ok(files) = attachments(api, &graph).await {
            let names: Vec<String> = files.iter().map(|a| defused(&text(a, "name"), 120)).collect();
            if !names.is_empty() {
                head += &format!(
                    "Attachments (not opened): {}\n(mail_attachment saves one into your files)\n",
                    names.join(", ")
                );
            }
        }
    }
    let body = v.pointer("/body/content").and_then(Value::as_str).unwrap_or("");
    let body = if v.pointer("/body/contentType").and_then(Value::as_str) == Some("html") {
        crate::mailbox::html_text(body)
    } else {
        body.to_string()
    };
    let body = body.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    let cut = tk::chars(&body) > crate::mailbox::READ_CHARS;
    let body = crate::archives::defused(tk::head(&body, crate::mailbox::READ_CHARS));
    let cut = if cut { "\n[… the rest is cut]" } else { "" };
    tool_text(&format!("{}\n<<<\n{head}\n{body}{cut}\n>>>", crate::mailbox::UNTRUSTED), false)
}

async fn save_attachment(config: &Config, account: AccountId, agent: &AgentConfig, api: &Api, arguments: &Dict) -> Dict {
    let id = tk::str_or_empty(arguments.get("id"));
    let Some(graph) = graph_of(config, account, &id) else { return unknown_id(&id) };
    if let Some(refused) = crate::mailbox::files_refused(agent) {
        return refused;
    }
    let wanted = match arguments.get("attachment") {
        Some(Value::Number(n)) => n.to_string(),
        other => tk::str_or_empty(other),
    };
    let (v, files) = match tokio::join!(message(api, &graph), attachments(api, &graph)) {
        (Ok(v), Ok(files)) => (v, files),
        (Err(fault), _) | (_, Err(fault)) => {
            return outlook::fault_result(&fault, "I can't reach that Outlook mail just now.")
        }
    };
    let names: Vec<String> = files.iter().map(|a| text(a, "name")).collect();
    let index = match crate::mailbox::choose_index(&names, &wanted) {
        Ok(i) => i,
        Err(e) => return tool_text(&e.0, true),
    };
    let chosen = &files[index];
    let max = crate::mailbox::ATTACHMENT_MAX;
    if chosen.get("size").and_then(Value::as_u64).is_some_and(|s| s > max + max / 3) {
        return tool_text(&format!("{} is too large (at most {} MB)", names[index], max / (1024 * 1024)), true);
    }
    let url = format!("{}/me/messages/{graph}/attachments/{}/$value", outlook::base(api), text(chosen, "id"));
    let data = match outlook::bytes(api, &url, max).await {
        Ok(data) => data,
        Err(Fault::TooLarge) => {
            return tool_text(&format!("{} is too large (at most {} MB)", names[index], max / (1024 * 1024)), true)
        }
        Err(fault) => return outlook::fault_result(&fault, "I can't get that attachment just now."),
    };
    // written by the sender: defused (review R9)
    let from = defused(&person_of(v.get("from").unwrap_or(&Value::Null)), 200);
    let content_type = Some(defused(&text(chosen, "contentType"), 100)).filter(|t| !t.is_empty()).unwrap_or_else(|| "application/octet-stream".into());
    let file = crate::tools_knowledge::email::Attachment { name: names[index].clone(), content_type, data };
    tracing::info!(agent = %agent.name, "outlook: mail attachment saved");
    crate::mailbox::store_attachment(config, agent, file, &from)
}

// -- drafts ------------------------------------------------------------------------------------------------------------

/// A draft made in answer to a mail (Graph's ids), to make only one per mail.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Made {
    original: String,
    draft: String,
    at: f64,
    #[serde(default = "first_account")]
    account: AccountId,
}

fn first_account() -> AccountId {
    outlook::FIRST
}

static FILES_LOCK: Mutex<()> = Mutex::new(());

fn made_drafts(config: &Config) -> Vec<Made> {
    let now = py::time_time();
    outlook::read_file(config, "drafts.json")
        .and_then(|b| serde_json::from_slice::<Vec<Made>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|d| now - d.at < DRAFTS_DAYS * 86400.0)
        .collect()
}

fn keep_draft(config: &Config, made: Made) {
    let _guard = FILES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = made_drafts(config);
    all.retain(|d| d.original != made.original);
    all.push(made);
    if let Err(e) = serde_json::to_vec(&all).map_err(anyhow::Error::from).and_then(|b| outlook::write_file(config, "drafts.json", &b)) {
        tracing::warn!(error = %e, "outlook: draft not noted");
    }
}

fn recipients(to: &str) -> Vec<Value> {
    crate::mailbox::header_addresses(to).into_iter().map(|a| json!({"emailAddress": {"address": a}})).collect()
}

async fn draft(config: &Config, account: AccountId, agent: &AgentConfig, api: &Api, arguments: &Dict) -> Dict {
    let label = outlook::label(config, account);
    let arg = |k: &str| tk::str_or_empty(arguments.get(k));
    let reply = arg("reply_to_id");
    let mut to = tk::strip(&arg("to")).to_string();
    let mut original: Option<String> = None;
    let mut sent_by: Vec<String> = Vec::new();
    if !reply.is_empty() {
        let Some(graph) = graph_of(config, account, &reply) else { return unknown_id(&reply) };
        let v = match message(api, &graph).await {
            Ok(v) => v,
            Err(fault) => return outlook::fault_result(&fault, "I can't reach that Outlook mail just now."),
        };
        let own = own(config, account);
        to = match crate::mailbox::reply_address(&headers_of(&v), &to, &own) {
            Ok(to) => to,
            Err(e) => return tool_text(&e.0, true),
        };
        sent_by = crate::mailbox::header_addresses(&person_of(v.get("from").unwrap_or(&Value::Null)));
        // one draft per mail answered, while it waits in Drafts
        if let Some(made) = made_drafts(config).into_iter().find(|d| d.original == graph) {
            if let Ok(d) = message(api, &made.draft).await {
                if d.get("isDraft").and_then(Value::as_bool).unwrap_or(false) {
                    return tool_text(
                        &format!(
                            "Not made: a draft answering this e-mail is already in {label} › Drafts. The person reads \
it and sends it; to change it, they edit that draft. Don't make another one."
                        ),
                        true,
                    );
                }
            }
        }
        original = Some(graph);
    }
    if to.is_empty() || recipients(&to).is_empty() {
        return tool_text(
            "mail_draft needs `to`, or reply_to_id (the reply then goes to that e-mail's Reply-To, else its sender)",
            true,
        );
    }
    let base = outlook::base(api);
    let mut body = json!({"body": {"contentType": "Text", "content": arg("body")}, "toRecipients": recipients(&to)});
    let subject = arg("subject");
    if !subject.is_empty() {
        body["subject"] = json!(subject);
    }
    let made = match &original {
        Some(graph) => {
            let created = outlook::send(api, reqwest::Method::POST, &format!("{base}/me/messages/{graph}/createReply"),
                                        Some(json!({})), None, &[PREFER_IMMUTABLE]).await;
            match created {
                Ok(d) if !text(&d, "id").is_empty() => {
                    let url = format!("{base}/me/messages/{}", text(&d, "id"));
                    outlook::send(api, reqwest::Method::PATCH, &url, Some(body), None, &[PREFER_IMMUTABLE]).await
                        .map(|_| d)
                }
                Ok(_) => Err(Fault::Unavailable),
                Err(fault) => Err(fault),
            }
        }
        None => outlook::send(api, reqwest::Method::POST, &format!("{base}/me/messages"), Some(body), None,
                              &[PREFER_IMMUTABLE]).await,
    };
    let made = match made {
        Ok(d) => d,
        Err(fault) => return outlook::fault_result(&fault, "I can't save the draft in your Outlook just now."),
    };
    tracing::info!(agent = %agent.name, "outlook: mail draft");
    if let Some(graph) = original {
        keep_draft(config, Made { original: graph, draft: text(&made, "id"), at: py::time_time(), account });
    }
    let elsewhere = !sent_by.is_empty() && crate::mailbox::header_addresses(&to).iter().any(|a| !sent_by.contains(a));
    let said = if elsewhere {
        format!(
            "Draft saved in {label} › Drafts, to {to} (its Reply-To; the e-mail came from {}). Tell the person both \
addresses: they check them, read the draft and send it themselves.",
            sent_by.join(", ")
        )
    } else {
        format!("Draft saved in {label} › Drafts, to {to}. Tell the person: they read it and send it themselves.")
    };
    tool_text(&said, false)
}

// -- tidying -----------------------------------------------------------------------------------------------------------

/// (place, account, folder)
type ScanKey = (String, AccountId, String);
static SCANS: LazyLock<Mutex<HashMap<ScanKey, Scan>>> = LazyLock::new(Default::default);

/// The scans kept of account `account` (disconnected, or its mails moved: read again).
pub fn forget_scans(account: AccountId) {
    SCANS.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| k.1 != account);
}

/// The drafts and tidyings kept of an account disconnected (the others' kept).
pub fn forget_account(config: &Config, account: AccountId) {
    let _guard = FILES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let drafts: Vec<Made> = made_drafts(config).into_iter().filter(|d| d.account != account).collect();
    let jobs: Vec<Done> = jobs(config).into_iter().filter(|j| j.account != account).collect();
    for (name, bytes) in [("drafts.json", serde_json::to_vec(&drafts)), ("jobs.json", serde_json::to_vec(&jobs))] {
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| outlook::write_file(config, name, &b)) {
            tracing::warn!(error = %e, "outlook: a disconnected account's notes not forgotten");
        }
    }
}

/// Graph's messages of `account` as mail_tidy.rs' rows (their `uid`: the short numbers, made by one lot;
/// `message_id`: Graph's id).
pub fn rows_of(config: &Config, account: AccountId, items: &[Value]) -> Vec<Row> {
    let ids: Vec<String> = items.iter().map(|v| text(v, "id")).collect();
    let shorts = outlook::short_ids(config, account, &ids);
    items.iter().zip(shorts).map(|(v, n)| row_with(v, n)).collect()
}

/// One message as a row (`rows_of` for a page).
pub fn row_of(config: &Config, account: AccountId, v: &Value) -> Row {
    row_with(v, outlook::short_id(config, account, &text(v, "id")))
}

fn row_with(v: &Value, n: u64) -> Row {
    let from = v.pointer("/from/emailAddress/address").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    Row {
        uid: n as u32,
        from,
        subject: tk::head(&text(v, "subject").split_whitespace().collect::<Vec<_>>().join(" "), 300).to_string(),
        date: text(v, "receivedDateTime").get(..10).unwrap_or("").to_string(),
        seen: v.get("isRead").and_then(Value::as_bool).unwrap_or(false),
        flagged: v.pointer("/flag/flagStatus").and_then(Value::as_str).is_some_and(|s| s == "flagged"),
        draft: v.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        message_id: text(v, "id"),
        ..Row::default()
    }
}

pub async fn scan(config: &Config, account: AccountId, api: &Api, folder: &MailFolder, fresh: bool) -> Result<Scan, Fault> {
    let key = (config.data_dir(), account, folder.id.clone());
    let now = py::time_time();
    if !fresh {
        if let Some(kept) = SCANS.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if now - kept.at < CACHE_SECONDS {
                return Ok(kept.clone());
            }
        }
    }
    let url = format!("{}/me/mailFolders/{}/messages", outlook::base(api), folder.id);
    let params = [
        ("$top", PAGE.to_string()),
        ("$orderby", "receivedDateTime desc".to_string()),
        ("$select", "id,from,subject,receivedDateTime,isRead,flag,isDraft".to_string()),
    ];
    let (items, cut) = outlook::pages(api, &url, &params, &[PREFER_IMMUTABLE], SCAN_MAX).await?;
    let rows: Vec<Row> = rows_of(config, account, &items);
    let scanned = Scan { folder: folder.name.clone(), uid_validity: None, total: folder.total.max(rows.len()), rows, cut, at: now };
    SCANS.lock().unwrap_or_else(|e| e.into_inner()).insert(key, scanned.clone());
    Ok(scanned)
}

/// The person's own addresses in account `account` (never tidied).
pub fn own(config: &Config, account: AccountId) -> Vec<String> {
    outlook::address(config, account).map(|a| crate::mailbox::addresses(&a)).unwrap_or_default()
}

/// The `mailbox` argument naming account `account` (its address when the place keeps several).
pub fn mailbox_word(config: &Config, account: AccountId) -> String {
    match outlook::address(config, account).filter(|_| outlook::several(config)) {
        Some(address) => crate::archives::defused(&address),
        None => LABEL.to_string(),
    }
}

/// mail_tidy.rs' overview of mailbox `label`, its next step naming the mailbox (`word`); its mailing lists aren't read
/// here, so standing rules name a sender or a domain.
pub fn overview_for(label: &str, word: &str, scanned: &Scan, own: &[String]) -> String {
    let text = crate::mail_tidy::overview_text(label, scanned, own, &[]);
    let kept = match text.rfind("\nNext:") {
        Some(at) => text[..at].to_string(),
        None => text,
    };
    format!(
        "{kept}\nNext: mail_bulk(action, groups or criteria, mailbox \"{word}\") makes a card for the person; nothing \
moves before their Oui. A sender that keeps coming (server providers, shops, banks, newsletters): add rule: true to file \
it from now on too (a sender or a domain; a folder rather than the trash; invoices to « Factures »)."
    )
}

async fn overview(config: &Config, account: AccountId, api: &Api, arguments: &Dict) -> Dict {
    let folder = match open_folder(api, &tk::str_or_empty(arguments.get("folder"))).await {
        Ok(f) => f,
        Err(Ok(said)) => return tool_text(&said, true),
        Err(Err(fault)) => return outlook::fault_result(&fault, "I can't reach your Outlook mail just now."),
    };
    let fresh = arguments.get("fresh").map(py::truthy).unwrap_or(false);
    match scan(config, account, api, &folder, fresh).await {
        Ok(scanned) => tool_text(
            &overview_for(&outlook::label(config, account), &mailbox_word(config, account), &scanned, &own(config, account)),
            false,
        ),
        Err(fault) => outlook::fault_result(&fault, "I can't read your Outlook mail just now."),
    }
}

/// The card's kind (approval_level.rs) of an action.
pub fn kind_of(action: &Action) -> &'static str {
    match action {
        Action::Archive => "mail_archive",
        Action::MarkRead | Action::MarkUnread => "mail_mark_read",
        Action::Move(_) | Action::Copy(_) => "mail_move",
        Action::Trash => "mail_trash",
    }
}

/// A tidying done, for its undo.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Done {
    pub id: String,
    pub agent: String,
    pub action: String,
    /// Where the mails were (their folder's id) and went.
    pub from: String,
    pub to: String,
    /// Graph's ids of the mails done (their ids after the move: immutable, the same).
    pub mails: Vec<String>,
    pub at: f64,
    pub undone: bool,
    /// The account it was done in.
    #[serde(default = "first_account")]
    pub account: AccountId,
}

pub const JOB_PREFIX: &str = "outlook-";

pub fn jobs(config: &Config) -> Vec<Done> {
    let now = py::time_time();
    outlook::read_file(config, "jobs.json")
        .and_then(|b| serde_json::from_slice::<Vec<Done>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|j| now - j.at < UNDO_DAYS * 86400.0)
        .collect()
}

fn keep_job(config: &Config, job: &Done) {
    let _guard = FILES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = jobs(config);
    all.retain(|j| j.id != job.id);
    all.push(job.clone());
    if let Err(e) = serde_json::to_vec(&all).map_err(anyhow::Error::from).and_then(|b| outlook::write_file(config, "jobs.json", &b)) {
        tracing::warn!(error = %e, "outlook: tidying not kept for its undo");
    }
}

/// The plan of a tidying, checked before its card: the job mail_tidy.rs' card words read, the destination's id.
#[derive(Debug)]
pub struct Planned {
    pub job: Job,
    pub source: String,
    pub dest: String,
}

/// The mails `selector` takes in `scanned` (mail_tidy.rs' rules: never the flagged ones, drafts nor the person's own;
/// for mark_read, the unread ones), the card's job for them. Err: plain words.
#[allow(clippy::too_many_arguments)]
pub fn plan(agent: &str, label: &str, scanned: &Scan, folder: &MailFolder, action: Action, selector: &Selector,
            own: &[String], dest: &str) -> Result<Planned, String> {
    selector.check(&|g| crate::mail_tidy::known_group(scanned, g))?;
    if !selector.list_id.is_empty() || selector.groups.iter().any(|g| g.starts_with("list:")) {
        return Err(format!("{LABEL}'s mailing lists aren't read here: name a sender (from) or a domain"));
    }
    let cutoff = selector.cutoff();
    let mut chosen: Vec<&Row> = scanned
        .rows
        .iter()
        .filter(|r| !crate::mail_tidy::protected(r, own) && selector.matches(r, &cutoff))
        .filter(|r| !(action == Action::MarkRead && r.seen))
        .collect();
    if chosen.is_empty() {
        return Err("no message matches (flagged ones, drafts and the person's own mails are left out)".into());
    }
    chosen.truncate(MAX_PER_CARD);
    let mut by_group: BTreeMap<String, usize> = BTreeMap::new();
    for row in &chosen {
        *by_group.entry(row.domain()).or_default() += 1;
    }
    let mut groups: Vec<(String, usize)> = by_group.into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let sample = chosen
        .iter()
        .take(crate::mail_tidy::SAMPLE)
        .map(|r| format!("{} · {} · {}", r.date, defused(&r.from, 120), defused(&r.subject, 70)))
        .collect();
    let job = Job {
        id: format!("{JOB_PREFIX}{}", py::token_hex(6)),
        agent: agent.to_string(),
        account: outlook::BOX_ID.into(),
        mailbox: label.to_string(),
        folder: folder.name.clone(),
        action: Some(action),
        dest: dest.to_string(),
        uids: chosen.iter().map(|r| r.uid).collect(),
        message_ids: chosen.iter().map(|r| r.message_id.clone()).collect(),
        groups,
        sample,
        total: scanned.total,
        everything: selector.everything,
        status: "asked".into(),
        at: py::time_time(),
        ..Job::default()
    };
    Ok(Planned { job, source: folder.id.clone(), dest: dest.to_string() })
}

/// What a held « Oui » grants exactly: the action, the folders and the very mails.
fn command_of(planned: &Planned) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(planned.job.message_ids.join(",").as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let action = planned.job.action.as_ref().map(Action::word).unwrap_or_default();
    format!("mail_bulk {action} {} {} › {} {hex}", planned.job.message_ids.len(), planned.job.mailbox, planned.job.folder)
}

/// Each mail moved (or marked) at Graph, `AT_ONCE` at a time: the ones done.
async fn act(api: &Api, action: &Action, dest: &str, mails: &[String]) -> Vec<String> {
    use futures::stream::{self, StreamExt};
    let base = outlook::base(api);
    let results: Vec<Option<String>> = stream::iter(mails.iter().cloned())
        .map(|id| {
            let base = base.clone();
            async move {
                let done = match action {
                    Action::MarkRead | Action::MarkUnread => {
                        let read = *action == Action::MarkRead;
                        outlook::send(api, reqwest::Method::PATCH, &format!("{base}/me/messages/{id}"),
                                      Some(json!({"isRead": read})), None, &[PREFER_IMMUTABLE]).await
                    }
                    _ => outlook::send(api, reqwest::Method::POST, &format!("{base}/me/messages/{id}/move"),
                                       Some(json!({"destinationId": dest})), None, &[PREFER_IMMUTABLE]).await,
                };
                done.ok().map(|_| id)
            }
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;
    results.into_iter().flatten().collect()
}

fn done_words(label: &str, action: &Action, n: usize, folder: &str, dest_name: &str) -> String {
    match action {
        Action::MarkRead => format!("{n} e-mails marked read in {label} › {folder}"),
        Action::Trash => format!("{n} e-mails put in {label}'s Deleted Items (from {folder})"),
        Action::Archive => format!("{n} e-mails archived in {label} (from {folder})"),
        _ => format!("{n} e-mails moved to {label} › {dest_name} (from {folder})"),
    }
}

async fn bulk(state: &AppState, agent: &AgentConfig, account: AccountId, api: &Api, arguments: &Dict) -> Dict {
    let config = state.config();
    let label = outlook::label(&config, account);
    let arg = |k: &str| tk::str_or_empty(arguments.get(k));
    let word = tk::strip(&arg("action")).to_lowercase();
    if crate::mail_rules::WORDS.contains(&word.as_str()) || arguments.get("rule").is_some_and(py::truthy) {
        // the standing rules: the IMAP boxes' (mail_rules.rs), on this mailbox (outlook_rules.rs)
        return crate::outlook_rules::call(&config, &agent.name, account, api, &word, arguments).await;
    }
    if word == "status" {
        let mut mine: Vec<String> = jobs(&config)
            .iter()
            .filter(|j| j.agent == agent.name && j.account == account)
            .rev()
            .take(5)
            .map(|j| format!("Job {} · {} · {} e-mails{}", j.id, j.action, j.mails.len(), if j.undone { " · undone" } else { "" }))
            .collect();
        mine.extend(crate::mail_rules::status_lines(&config, &agent.name).into_iter().filter(|l| l.contains(&label)));
        return tool_text(&if mine.is_empty() { format!("No {label} tidying yet.") } else { mine.join("\n") }, false);
    }
    if word == "undo" {
        return undo(&config, agent, api, &arg("job")).await;
    }
    let action = match Action::parse(&arg("action")) {
        Ok(a) => a,
        Err(why) => return tool_text(&why, true),
    };
    let selector = Selector::from_args(arguments);
    let folder = match open_folder(api, &arg("folder")).await {
        Ok(f) => f,
        Err(Ok(said)) => return tool_text(&said, true),
        Err(Err(fault)) => return outlook::fault_result(&fault, "I can't reach your Outlook mail just now."),
    };
    // never Sent Items nor Drafts, as a source or a destination; not known: nothing done (review R6)
    let mut protected = Vec::new();
    for id in ["sentitems", "drafts"] {
        match known_folder(api, id).await {
            Ok(f) => protected.push(f.id),
            Err(fault) => return outlook::fault_result(&fault, "I can't reach your Outlook mail just now."),
        }
    }
    if protected.contains(&folder.id) {
        return tool_text(&format!("{} is the person's sent mail or drafts: never touched", folder.name), true);
    }
    let (dest, dest_name) = match destination(api, &action, &folder, &protected).await {
        Ok(found) => found,
        Err(said) => return said,
    };
    let scanned = match scan(&config, account, api, &folder, false).await {
        Ok(s) => s,
        Err(fault) => return outlook::fault_result(&fault, "I can't read your Outlook mail just now."),
    };
    let planned =
        match plan(&agent.name, &label, &scanned, &folder, action.clone(), &selector, &own(&config, account), &dest) {
            Ok(p) => p,
            Err(why) => return tool_text(&why, true),
        };
    let lang = crate::mail_tidy::language(&config, &agent.name);
    let shown = crate::mail_tidy::shown_agent(&config, &agent.name);
    // a move to the Deleted Items is the trash, whatever its name (review R7): critical, as `trash`
    let to_trash = matches!(action, Action::Move(_)) && known_folder(api, "deleteditems").await.is_ok_and(|f| f.id == dest);
    let kind = if to_trash { "mail_trash" } else { kind_of(&action) };
    let summary = crate::mail_tidy::sentence(&lang, &shown, &planned.job);
    let address = outlook::address(&config, account);
    // 1.1 (Alex 24/11): the person's level as for an IMAP box's card (approval_level.rs `at_once`): « Seulement le
    // critique », or their « Toujours » for this kind, lets it through with no card; the trash never
    let at_once = crate::approval_level::at_once(&config, &agent.name, kind);
    if at_once.is_none() {
        let card = crate::person_ok::Card {
            kind,
            tool: "mail_bulk",
            command: command_of(&planned),
            summary: summary.clone(),
            detail: crate::mail_tidy::detail(&lang, &planned.job),
            // « Toujours autoriser » for that kind, as on the IMAP boxes' cards (never for the trash: critical)
            always: crate::approval_level::always_button(&lang, kind),
            always_key: None,
        };
        use crate::person_ok::Answer;
        match crate::person_ok::ask(state, agent, card).await {
            Answer::Once | Answer::Always => {}
            Answer::Deny => return tool_text("Your person said no: nothing was moved in their Outlook. Ask what they want instead.", true),
            Answer::NoAnswer => {
                return tool_text("Your person didn't answer the card in time: nothing was moved in their Outlook. Say it waits for their OK.", true)
            }
            Answer::Held => {
                return tool_text("Your person isn't there now: the card waits for them in their app. Nothing is moved until \
they say yes; say so in your report and go on with the rest.", false)
            }
        }
    }
    // the account, the part and the agent still there (review C4): else nothing moves
    let config = state.config();
    if let Err(why) = outlook::still_allowed(&config, &agent.name, "mail", account, address.as_deref()) {
        return tool_text(why, true);
    }
    let fresh = match outlook::signed_in(&config, account).await {
        Ok(api) => api,
        Err(fault) => return outlook::fault_result(&fault, "I can't reach your Outlook mail just now."),
    };
    let api = &fresh;
    // flagged since the scan: left out (review R6)
    let flagged = flagged_in(api, &folder.id).await;
    let mails: Vec<String> = planned.job.message_ids.iter().filter(|id| !flagged.contains(*id)).cloned().collect();
    let done = act(api, &action, &planned.dest, &mails).await;
    forget_scans(account);
    let job = Done {
        id: planned.job.id.clone(),
        agent: agent.name.clone(),
        action: action.word(),
        from: planned.source.clone(),
        to: planned.dest.clone(),
        mails: done.clone(),
        at: py::time_time(),
        undone: false,
        account,
    };
    keep_job(&config, &job);
    tracing::info!(agent = %agent.name, action = %job.action, done = done.len(), at_once = at_once.is_some(), "outlook: mails tidied");
    let skipped = planned.job.message_ids.len() - done.len();
    let skipped = if skipped > 0 { format!(" ({skipped} could not be done: gone or changed)") } else { String::new() };
    let more = if planned.job.uids.len() >= MAX_PER_CARD { format!(" At most {MAX_PER_CARD} per card: call again for the rest.") } else { String::new() };
    let mut said = format!(
        "Done: {}{skipped}. Job {}: mail_bulk(action: \"undo\", job: \"{}\") puts them back for 30 days.{more}",
        done_words(&label, &action, done.len(), &folder.name, &dest_name),
        job.id,
        job.id
    );
    if let Some(always) = at_once {
        // its line in « Validé pour toi », with « Annuler » (approval_level.rs `Undo::OutlookJob`)
        let undo = Some(crate::approval_level::Undo::OutlookJob { job: job.id.clone() });
        crate::approval_level::record_at_once(&config, &agent.name, kind, &summary, always, undo, Some(done.len() as u64));
        said.push('\n');
        said.push_str(&crate::approval_level::told(&[summary]));
    }
    tool_text(&said, false)
}

/// Where `action` sends the mails of `folder`: Graph's folder id and its name (none for marking read). Err: the
/// tool's answer.
async fn destination(api: &Api, action: &Action, folder: &MailFolder, protected: &[String]) -> Result<(String, String), Dict> {
    let wanted = match action {
        Action::MarkRead => return Ok((String::new(), String::new())),
        Action::Archive => "archive".to_string(),
        Action::Trash => "deleteditems".to_string(),
        Action::Move(f) => f.clone(),
        _ => return Err(tool_text("action: archive, mark_read, move:<folder> or trash (or status, undo)", true)),
    };
    let found = match well_known(&wanted) {
        Some(id) => known_folder(api, id).await.map_err(Err),
        None => open_folder(api, &wanted).await,
    };
    match found {
        Ok(f) if protected.contains(&f.id) => Err(tool_text(&format!("{} is the sent mail or drafts: not a destination", f.name), true)),
        Ok(f) if f.id == folder.id => Err(tool_text(&format!("the messages are already in {}", f.name), true)),
        Ok(f) => Ok((f.id, f.name)),
        Err(Ok(said)) => Err(tool_text(&said, true)),
        Err(Err(Fault::NotFound)) => {
            Err(tool_text(&format!("this {LABEL} mailbox has no such folder: use move:<one of its folders>"), true))
        }
        Err(Err(fault)) => Err(outlook::fault_result(&fault, "I can't reach your Outlook mail just now.")),
    }
}

/// [`destination`] for another module (outlook_rules.rs): Sent Items and Drafts read first, never one of them.
pub async fn destination_for(api: &Api, action: &Action, folder: &MailFolder) -> Result<(String, String), String> {
    let mut protected = Vec::new();
    for id in ["sentitems", "drafts"] {
        protected.push(known_folder(api, id).await.map_err(|f| outlook::words(&f))?.id);
    }
    if protected.contains(&folder.id) {
        return Err(format!("{} is the person's sent mail or drafts: never touched", folder.name));
    }
    destination(api, action, folder, &protected).await.map_err(|d| {
        d.get("content").and_then(|c| c.get(0)).and_then(|c| c.get("text")).and_then(Value::as_str).unwrap_or("").to_string()
    })
}

/// [`act`] for another module (outlook_rules.rs): the mails moved (or marked) at Graph, the ones done.
pub async fn act_on(api: &Api, action: &Action, dest: &str, mails: &[String]) -> Vec<String> {
    act(api, action, dest, mails).await
}

/// [`flagged_in`] for another module.
pub async fn flagged(api: &Api, folder: &str) -> std::collections::HashSet<String> {
    flagged_in(api, folder).await
}

/// [`still_in`] for another module.
pub async fn still_there(api: &Api, mails: &[String], folder: &str) -> Vec<String> {
    still_in(api, mails, folder).await
}

/// The inbox (Graph's well-known folder), for another module.
pub async fn inbox(api: &Api) -> Result<MailFolder, Fault> {
    known_folder(api, "inbox").await
}

/// The flagged mails of a folder now (Graph's `$filter`): never tidied, even when the scan saw them unflagged.
async fn flagged_in(api: &Api, folder: &str) -> std::collections::HashSet<String> {
    let url = format!("{}/me/mailFolders/{folder}/messages", outlook::base(api));
    let query = [
        ("$filter", "flag/flagStatus eq 'flagged'".to_string()),
        ("$select", "id".to_string()),
        ("$top", PAGE.to_string()),
    ];
    match outlook::pages(api, &url, &query, &[PREFER_IMMUTABLE], SCAN_MAX).await {
        Ok((items, _)) => items.iter().map(|v| text(v, "id")).collect(),
        Err(_) => std::collections::HashSet::new(),
    }
}

/// Among `mails`, those still in `folder` (review C1: an undo puts back only what the tidying left where it put it,
/// as the IMAP boxes' undo finds them in the destination; one the person filed elsewhere since stays there).
async fn still_in(api: &Api, mails: &[String], folder: &str) -> Vec<String> {
    use futures::stream::{self, StreamExt};
    let base = outlook::base(api);
    let found: Vec<Option<String>> = stream::iter(mails.iter().cloned())
        .map(|id| {
            let base = base.clone();
            async move {
                let url = format!("{base}/me/messages/{id}");
                let v = outlook::fetch(api, &url, &[("$select", "id,parentFolderId".to_string())], &[PREFER_IMMUTABLE]).await.ok()?;
                (text(&v, "parentFolderId") == folder).then_some(id)
            }
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;
    found.into_iter().flatten().collect()
}

async fn undo(config: &Config, agent: &AgentConfig, api: &Api, id: &str) -> Dict {
    match undo_job(config, &agent.name, api, id).await {
        Ok(said) => tool_text(&said, false),
        Err(said) => tool_text(&said, true),
    }
}

/// Job `id` of `agent` undone at Graph (`api`: its account's): the mails still where it put them put back. The
/// answer's words; Err: why not.
async fn undo_job(config: &Config, agent: &str, api: &Api, id: &str) -> Result<String, String> {
    let Some(mut job) = jobs(config).into_iter().find(|j| j.id == id.trim() && j.agent == agent) else {
        return Err("no such job of yours (mail_bulk action status lists them)".into());
    };
    if job.undone {
        return Err(format!("job {} was undone already", job.id));
    }
    let back = match Action::parse(&job.action) {
        Ok(Action::MarkRead) => Action::MarkUnread,
        Ok(_) => Action::Move(job.from.clone()),
        Err(_) => return Err("this job can't be undone".into()),
    };
    // only those still where the tidying left them: the destination, or the folder of mails marked read
    let left_in = if job.to.is_empty() { &job.from } else { &job.to };
    let still = still_in(api, &job.mails, left_in).await;
    let done = act(api, &back, &job.from, &still).await;
    job.undone = true;
    keep_job(config, &job);
    forget_scans(job.account);
    let moved = job.mails.len() - still.len();
    let moved = if moved > 0 { format!(" {moved} moved elsewhere since stay where they are.") } else { String::new() };
    tracing::info!(agent, job = %job.id, back = done.len(), "outlook: tidying undone");
    Ok(format!("Undone: {} of {} e-mails put back in {}.{moved}", done.len(), job.mails.len(), outlook::label(config, job.account)))
}

/// « Annuler » on the line of a tidying done at once (approval_level.rs `Undo::OutlookJob`): its account signed in
/// again, the job undone. Err: why not (its account disconnected, the job undone already…).
pub async fn undo_line(config: &Config, agent: &str, id: &str) -> Result<(), String> {
    let account = jobs(config)
        .into_iter()
        .find(|j| j.id == id && j.agent == agent)
        .map(|j| j.account)
        .ok_or_else(|| "this tidying is too old to be undone".to_string())?;
    if outlook::one(config, account).is_none() {
        return Err("this Outlook account is no longer connected".into());
    }
    let api = outlook::signed_in(config, account).await.map_err(|f| outlook::words(&f))?;
    undo_job(config, agent, &api, id).await.map(|_| ())
}

// -- the tools ---------------------------------------------------------------------------------------------------------

fn no_mailbox() -> Dict {
    tool_text(&format!("{LABEL}'s mail isn't yours: your person switches « Mails » on for you in Outlook's page of the app."), true)
}

/// The account a mail tool's call is for: the one of the id it names (a mail's, a job's, a rule's), else the one its
/// `mailbox` names, else `agent`'s first one with mail. Err: the tool's answer (an id of no account of its).
pub fn account_of_call(config: &Config, agent: &str, name: &str, arguments: &Dict) -> Result<AccountId, Dict> {
    let mine: Vec<AccountId> = outlook::accounts_with(config, agent, "mail").iter().map(|a| a.id).collect();
    if mine.is_empty() {
        return Err(no_mailbox());
    }
    let arg = |k: &str| tk::str_or_empty(arguments.get(k)).trim().to_string();
    let of_id = |id: &str| match graph_id(config, id) {
        Some((account, _)) if mine.contains(&account) => Ok(account),
        _ => Err(unknown_id(id)),
    };
    match name {
        "mail_read" | "mail_attachment" if is_ours(&arg("id")) => return of_id(&arg("id")),
        "mail_draft" if is_ours(&arg("reply_to_id")) => return of_id(&arg("reply_to_id")),
        "mail_bulk" if arg("job").starts_with(JOB_PREFIX) => {
            let job = jobs(config).into_iter().find(|j| j.id == arg("job") && j.agent == agent);
            return match job {
                Some(j) if mine.contains(&j.account) => Ok(j.account),
                _ => Err(tool_text("no such job of yours (mail_bulk action status lists them)", true)),
            };
        }
        "mail_bulk" if !arg("rule_id").is_empty() => {
            if let Some(account) = crate::outlook_rules::account_of_rule(config, agent, &arg("rule_id")) {
                if mine.contains(&account) {
                    return Ok(account);
                }
            }
        }
        _ => {}
    }
    named_account(config, agent, &arg("mailbox")).ok_or_else(no_mailbox)
}

/// One of the mail tools for Outlook (`takes` said it is), for `agent`.
pub async fn call(state: &AppState, agent: &AgentConfig, name: &str, arguments: &Dict) -> Dict {
    let config = state.config();
    let account = match account_of_call(&config, &agent.name, name, arguments) {
        Ok(account) => account,
        Err(said) => return said,
    };
    let api = match outlook::signed_in(&config, account).await {
        Ok(api) => api,
        Err(fault) => return outlook::fault_result(&fault, "I can't reach your Outlook mail just now."),
    };
    let mut result = match name {
        "mail_search" => search(&config, account, &api, arguments).await,
        "mail_read" => read(&config, account, &api, &tk::str_or_empty(arguments.get("id"))).await,
        "mail_attachment" => save_attachment(&config, account, agent, &api, arguments).await,
        "mail_draft" => draft(&config, account, agent, &api, arguments).await,
        "mail_overview" => overview(&config, account, &api, arguments).await,
        "mail_bulk" => bulk(state, agent, account, &api, arguments).await,
        _ => tool_text(&format!("unknown tool: {name}"), true),
    };
    if name == "mail_search" {
        // its other Outlook mailboxes named (it would not know they are there)
        let also = also_line(&config, &agent.name, Some(account));
        if !also.is_empty() {
            if let Some(Value::String(said)) = result.get_mut("content").and_then(|c| c.get_mut(0)).and_then(|c| c.get_mut("text")) {
                said.push('\n');
                said.push_str(&also);
            }
        }
    }
    result
}
