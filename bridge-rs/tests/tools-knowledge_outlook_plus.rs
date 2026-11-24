//! Outlook's four additions for 1.1 (Alex 24/11), against the small fake Microsoft (tests/common/outlook_graph.rs):
//!
//! 1. its mails tidied by the person's approval level exactly as the IMAP boxes' (« Seulement le critique »: done at
//!    once, its line in « Validé pour toi » with « Annuler »; a « Toujours » for that kind; the trash still asked);
//! 2. its standing sort rules (`mail_bulk rule: true`, the IMAP boxes' `bip-mailrule-` card) — their new-mail side and
//!    the routines started by a new Outlook mail are in tools-knowledge_outlook_watch.rs;
//! 3. several Microsoft accounts on one place, each its own tokens, parts, agents, « Toujours » and « Déconnecter »;
//! 4. Outlook's contacts added and changed (never deleted), on cards by the approval level.
//!
//! No real Microsoft account is ever called.

mod common;
#[path = "common/outlook_graph.rs"]
mod graph;

use common::auth;
use graph::*;
use serde_json::{json, Value};

async fn level(app: &common::TestApp, h: &[(&str, &str)], level: &str) {
    assert_eq!(app.send_json("PUT", "/v1/approval-level", h, &json!({"level": level})).await.status, 200);
}

/// The bridge's cards waiting now (`/v1/approvals`), by their run id's prefix.
async fn cards(app: &common::TestApp, h: &[(&str, &str)], prefix: &str) -> Vec<Value> {
    let items = app.get("/v1/approvals", h).await.json()["items"].clone();
    items
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i["run_id"].as_str().is_some_and(|r| r.starts_with(prefix)))
        .collect()
}

/// « Ce que tes agents font tout seuls » › « Validé pour toi »: Coach's lines.
async fn approved_lines(app: &common::TestApp, h: &[(&str, &str)]) -> Vec<Value> {
    app.get("/v1/approvals/auto?agent=coach", h).await.json()["items"].as_array().cloned().unwrap_or_default()
}

// -- 1. tidied by the level, with « Annuler » ---------------------------------------------------------------------------

/// Under « Seulement le critique » an Outlook tidying is done at once, no card, as an IMAP box's: its line in « Validé
/// pour toi » with « Annuler », which puts back only the mails still where it put them. Under « Tout demander » its card
/// offers « Toujours autoriser » for that kind (as the IMAP cards do), and a « Toujours » given lets the next one
/// through. The trash stays a card at every level. A `bip-ok-` card is still never answered by another call's level.
#[tokio::test]
async fn tidying_follows_the_level_as_the_imap_boxes_and_its_line_undoes_it() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);

    level(&app, &h, "critical").await;
    let (failed, said) = tool(&app, "mail_bulk", json!({"action": "archive", "domain": "shop.example"}), "h1").await;
    assert!(!failed && said.contains("2 e-mails archived in Outlook"), "{said}");
    assert!(said.contains("Approved at once") || said.contains("at once"), "the agent told it went through: {said}");
    assert!(cards(&app, &h, "bip-ok-").await.is_empty(), "a card under « Seulement le critique »");
    {
        let g = graph.lock().unwrap();
        assert_eq!(g.messages["m2"]["parentFolderId"], "archive-id");
        assert_eq!(g.messages["m3"]["parentFolderId"], "archive-id");
        assert_eq!(g.messages["m4"]["parentFolderId"], "inbox-id", "the flagged one moved");
        assert_eq!(g.messages["m6"]["parentFolderId"], "inbox-id", "the person's own moved");
    }
    let lines = approved_lines(&app, &h).await;
    let line = lines.iter().find(|l| l["kind"] == "mail_archive").cloned().unwrap_or_else(|| panic!("no line: {lines:?}"));
    assert_eq!(line["summary"], "Coach wants to archive 2 e-mails (from shop.example)", "{line}");
    assert_eq!(line["undo"], true, "{line}");
    // the person files one of the two elsewhere herself, then « Annuler » on the line
    graph.lock().unwrap().messages.get_mut("m2").unwrap()["parentFolderId"] = json!("factures-id");
    let undone = app.send_json("POST", &format!("/v1/approvals/auto/{}/undo", line["id"].as_str().unwrap()), &h, &json!({})).await;
    assert_eq!(undone.status, 200, "{}", undone.text());
    {
        let g = graph.lock().unwrap();
        assert_eq!(g.messages["m3"]["parentFolderId"], "inbox-id", "« Annuler » put nothing back");
        assert_eq!(g.messages["m2"]["parentFolderId"], "factures-id", "the person's own filing undone");
    }
    let line = approved_lines(&app, &h).await.into_iter().find(|l| l["kind"] == "mail_archive").unwrap();
    assert_eq!((line["undo"].clone(), line["undone"].clone()), (json!(false), json!(true)), "{line}");
    let again = app.send_json("POST", &format!("/v1/approvals/auto/{}/undo", line["id"].as_str().unwrap()), &h, &json!({})).await;
    assert_ne!(again.status, 200, "undone twice");

    // the person puts hers back in the inbox herself
    graph.lock().unwrap().messages.get_mut("m2").unwrap()["parentFolderId"] = json!("inbox-id");
    // the trash: critical, a card at every level
    let ((failed, _), card) = tokio::join!(
        tool(&app, "mail_bulk", json!({"action": "trash", "from": "deals@mail.shop.example"}), "h1"),
        common::answer_card(&app, "bip-ok-", "deny")
    );
    assert!(failed);
    assert_eq!(card["bip_ok"]["kind"], "mail_trash");
    assert!(card.get("bip_always").is_none(), "« Toujours » offered for the trash: {card}");

    // « Tout demander »: a card, with « Toujours autoriser » for that kind, as the IMAP boxes' cards
    level(&app, &h, "all").await;
    let ((failed, said), card) = tokio::join!(
        tool(&app, "mail_bulk", json!({"action": "mark_read", "domain": "shop.example"}), "h1"),
        common::answer_card(&app, "bip-ok-", "always")
    );
    assert!(!failed && said.contains("marked read"), "{said}");
    assert_eq!((card["bip_ok"]["kind"].as_str(), card["bip_always_kind"].as_str()), (Some("mail_mark_read"), Some("mail_mark_read")), "{card}");
    assert_eq!(card["bip_always"], "Always allow");
    // that « Toujours » lets the next one through, its line marked
    graph.lock().unwrap().messages.get_mut("m2").unwrap()["isRead"] = json!(false);
    let (failed, said) = tool(&app, "mail_bulk", json!({"action": "mark_read", "from": "news@shop.example"}), "h2").await;
    assert!(!failed && said.contains("marked read"), "{said}");
    assert!(cards(&app, &h, "bip-ok-").await.is_empty());
    let marked = approved_lines(&app, &h).await.into_iter().find(|l| l["kind"] == "mail_mark_read").unwrap();
    assert_eq!(marked["always"], true, "{marked}");
    // its « Annuler »: unread again
    let undone = app.send_json("POST", &format!("/v1/approvals/auto/{}/undo", marked["id"].as_str().unwrap()), &h, &json!({})).await;
    assert_eq!(undone.status, 200, "{}", undone.text());
    assert_eq!(graph.lock().unwrap().messages["m2"]["isRead"], false);
    // another kind is still asked (its « Toujours » is for marking read only)
    let ((_, _), card) = tokio::join!(
        tool(&app, "mail_bulk", json!({"action": "archive", "domain": "shop.example"}), "h3"),
        common::answer_card(&app, "bip-ok-", "deny")
    );
    assert_eq!(card["bip_ok"]["kind"], "mail_archive");
}

/// The rule of the review (B1) still holds with the « Toujours » of 1.1: under « Tout demander », with a « Toujours »
/// for another kind (so another call's `settle_new` looks at the cards), an Outlook tidying card that waits for the
/// person is never answered in her place.
#[tokio::test]
async fn a_tidying_card_is_still_never_answered_by_another_calls_level() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    bipbridge::approval_level::grant_always(&config, "coach", "mail_move").unwrap();
    let state = app.app.state.clone();
    let coach = config.agent(Some("coach")).cloned().unwrap();
    let before = bipbridge::card_place::waiting(&state, "coach");
    let writes_before = writes(&graph).len();
    let bulk = tool(&app, "mail_bulk", json!({"action": "archive", "domain": "shop.example"}), "h1");
    let other = async {
        card_shown(&app, &h, "bip-ok-").await;
        bipbridge::approval_level::settle_new(&state, &coach, &before, None).await
    };
    let ((failed, said), auto) = tokio::join!(bulk, other);
    assert!(auto.is_empty(), "answered by the level: {auto:?} / {said}");
    assert!(failed && said.contains("didn't answer"), "{said}");
    assert_eq!(writes(&graph).len(), writes_before);
}

// -- 2. standing sort rules -------------------------------------------------------------------------------------------

/// `mail_bulk rule: true` on Outlook: the IMAP boxes' rule card (`bip-mailrule-`), the rule listed on the agent's page
/// with its mailbox; on « Oui » the mails already there filed (never the flagged one nor the person's own), its last run
/// undone from the page, stopped. Under « Seulement le critique » the card goes through as an IMAP box's.
#[tokio::test]
async fn standing_rules_on_outlook_as_on_the_imap_boxes() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    let (failed, said) =
        tool(&app, "mail_bulk", json!({"action": "archive", "rule": true, "domain": "shop.example", "mailbox": "Outlook"}), "h1").await;
    assert!(!failed && said.contains("Rule card"), "{said}");
    let [card] = cards(&app, &h, "bip-mailrule-").await.try_into().unwrap_or_else(|c: Vec<Value>| panic!("{c:?}"));
    assert!(card["bip_summary"].as_str().unwrap().contains("shop.example"), "{card}");
    assert!(card["bip_detail"].as_str().unwrap().starts_with("Outlook › "), "{card}");
    assert_eq!(card["bip_mail_rule"]["existing"], 2, "{card}");
    assert!(writes(&graph).is_empty(), "filed before the « Oui »");
    answer(&app, &h, &card, "once").await;
    // the mails already there, filed in the background
    for _ in 0..100 {
        if graph.lock().unwrap().messages["m3"]["parentFolderId"] == "archive-id" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    {
        let g = graph.lock().unwrap();
        assert_eq!(g.messages["m2"]["parentFolderId"], "archive-id");
        assert_eq!(g.messages["m3"]["parentFolderId"], "archive-id");
        assert_eq!(g.messages["m4"]["parentFolderId"], "inbox-id", "flagged: never");
        assert_eq!(g.messages["m6"]["parentFolderId"], "inbox-id", "the person's own: never");
    }
    let rules = app.get("/v1/mail/rules?agent=coach", &h).await.json();
    let rule = rules["items"][0].clone();
    assert_eq!(rule["account"], "outlook-1", "{rules}");
    assert_eq!(rule["mailbox"], "Outlook", "{rules}");
    for _ in 0..100 {
        if app.get("/v1/mail/rules?agent=coach", &h).await.json()["items"][0]["applied"] == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let rule = app.get("/v1/mail/rules?agent=coach", &h).await.json()["items"][0].clone();
    assert_eq!((rule["applied"].clone(), rule["can_undo"].clone()), (json!(2), json!(true)), "{rule}");
    // its last run put back from the page
    let id = rule["id"].as_str().unwrap().to_string();
    let undone = app.send_json("POST", &format!("/v1/mail/rules/{id}/undo"), &h, &json!({})).await;
    assert_eq!(undone.status, 200, "{}", undone.text());
    assert_eq!(graph.lock().unwrap().messages["m2"]["parentFolderId"], "inbox-id");
    assert_eq!(graph.lock().unwrap().messages["m3"]["parentFolderId"], "inbox-id");
    // the agent's own words on it
    let (failed, listed) = tool(&app, "mail_bulk", json!({"action": "rules", "mailbox": "Outlook"}), "h1").await;
    assert!(!failed && listed.contains(&id), "{listed}");
    let (failed, said) = tool(&app, "mail_bulk", json!({"action": "stop_rule", "rule_id": id, "mailbox": "Outlook"}), "h1").await;
    assert!(!failed && said.contains("stopped"), "{said}");
    assert_eq!(app.get("/v1/mail/rules?agent=coach", &h).await.json()["items"], json!([]));
    // mailing lists aren't read on Outlook: refused plainly, no card
    let (failed, said) = tool(&app, "mail_bulk", json!({"action": "archive", "rule": true, "list_id": "news.shop.example", "mailbox": "Outlook"}), "h1").await;
    assert!(failed && said.contains("sender"), "{said}");

    // « Seulement le critique »: the rule's card goes through as an IMAP box's, its line with « Annuler » (stops it)
    level(&app, &h, "critical").await;
    let (failed, said) = tool(&app, "mail_bulk", json!({"action": "mark_read", "rule": true, "from": "help@service.example", "mailbox": "Outlook"}), "h1").await;
    assert!(!failed && said.contains("Approved at once"), "{said}");
    assert!(cards(&app, &h, "bip-mailrule-").await.is_empty());
    let line = approved_lines(&app, &h).await.into_iter().find(|l| l["kind"] == "mail_rule").expect("its line");
    assert_eq!(line["undo"], true);
    for _ in 0..100 {
        if graph.lock().unwrap().messages["m5"]["isRead"] == true {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(graph.lock().unwrap().messages["m5"]["isRead"], true);
    let undone = app.send_json("POST", &format!("/v1/approvals/auto/{}/undo", line["id"].as_str().unwrap()), &h, &json!({})).await;
    assert_eq!(undone.status, 200, "{}", undone.text());
    assert_eq!(app.get("/v1/mail/rules?agent=coach", &h).await.json()["items"], json!([]), "stopped by « Annuler »");
    assert_eq!(graph.lock().unwrap().messages["m5"]["isRead"], false, "its run put back");
}

// -- 3. several Microsoft accounts ------------------------------------------------------------------------------------

/// Two Microsoft accounts on one place: the second added (never replacing the first), each its own tokens (never in an
/// answer), agents and parts; the first still shown in the old fields (an app that knows one account stays right); the
/// mail of each by its address, the calendars of both listed and named, the cards naming the account, « Toujours » per
/// account (its default calendar); one disconnected, the other goes on untouched.
#[tokio::test]
async fn two_accounts_each_its_own_and_one_disconnected() {
    let (url, graph) = fake_graph().await;
    let second = add_second(&graph);
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    let (status, shown) = connect(&app, &h, "second").await;
    assert_eq!(status, 200, "{shown}");
    // the first in the old fields; both in `accounts`
    assert_eq!((shown["account"].as_str(), shown["state"].as_str()), (Some(ACCOUNT), Some("connected")), "{shown}");
    let accounts = shown["accounts"].as_array().cloned().unwrap_or_default();
    assert_eq!(accounts.len(), 2, "{shown}");
    assert_eq!((accounts[0]["id"].clone(), accounts[0]["account"].clone()), (json!(1), json!(ACCOUNT)));
    assert_eq!((accounts[1]["id"].clone(), accounts[1]["account"].clone()), (json!(2), json!(SECOND)));
    assert_eq!(accounts[1]["mailbox"], "outlook-2");
    assert_eq!(accounts[1]["agents"], json!(["coach"]));
    assert!(shown["max_accounts"].as_u64().is_some_and(|n| n >= 2), "{shown}");
    let text = shown.to_string();
    assert!(!text.contains("at-1") && !text.contains("bt-1") && !text.contains("brt-1"), "a key in the answer: {text}");
    // the same account signed in again: still two
    let (_, again) = connect(&app, &h, "second").await;
    assert_eq!(again["accounts"].as_array().map(Vec::len), Some(2), "{again}");

    // each mailbox by its address; the first stays the default one
    let (failed, first) = tool(&app, "mail_search", json!({}), "h1").await;
    assert!(!failed && first.contains("Votre facture") && !first.contains("Relevé de Sam"), "{first}");
    assert!(first.contains(SECOND), "the agent told of the other mailbox: {first}");
    let (failed, theirs) = tool(&app, "mail_search", json!({"mailbox": SECOND}), "h1").await;
    assert!(!failed && theirs.contains("Relevé de Sam") && !theirs.contains("Votre facture"), "{theirs}");
    let sam = id_of(&theirs, "Relevé de Sam");
    let (failed, read) = tool(&app, "mail_read", json!({"id": sam}), "h1").await;
    assert!(!failed && read.contains("info@banque.example"), "its id finds its account: {read}");
    // a tidying card names the account when there are several
    let ((_, _), card) = tokio::join!(
        tool(&app, "mail_bulk", json!({"action": "archive", "domain": "shop.example", "mailbox": SECOND}), "h1"),
        common::answer_card(&app, "bip-ok-", "once")
    );
    assert!(card["bip_detail"].as_str().unwrap().contains(SECOND), "{card}");
    assert_eq!(second.lock().unwrap().messages["s2"]["parentFolderId"], "archive-id");
    assert_eq!(graph.lock().unwrap().messages["m2"]["parentFolderId"], "inbox-id", "the other account touched");

    // both calendars listed, each named
    let (_, listed) = tool(&app, "calendar_list", json!({"from": "2026-11-23", "to": "2026-11-29"}), "h1").await;
    assert!(listed.contains("Dentiste") && listed.contains("Match de Sam"), "{listed}");
    assert!(listed.contains(SECOND), "{listed}");
    // « Toujours » for the second account's default calendar: the first's still asked
    let ((failed, _), card) = tokio::join!(
        tool(&app, "calendar_add", json!({"title": "Vélo", "start": "2026-12-01", "calendar": SECOND}), "h1"),
        common::answer_card(&app, "bip-ok-", "always")
    );
    assert!(!failed);
    assert!(card["bip_summary"].as_str().unwrap().contains(SECOND), "the card names the account: {card}");
    let (failed, said) = tool(&app, "calendar_add", json!({"title": "Piscine", "start": "2026-12-02", "calendar": SECOND}), "h2").await;
    assert!(!failed && !said.contains("card"), "{said}");
    assert!(second.lock().unwrap().events.values().any(|e| e["subject"] == "Piscine"), "the « Toujours » did nothing");
    let ((_, _), card) = tokio::join!(
        tool(&app, "calendar_add", json!({"title": "Yoga", "start": "2026-12-03", "calendar": "Outlook"}), "h3"),
        common::answer_card(&app, "bip-ok-", "deny")
    );
    assert_eq!(card["bip_ok"]["kind"], "calendar_write:outlook", "the first account's still asked: {card}");
    assert!(!graph.lock().unwrap().events.values().any(|e| e["subject"] == "Yoga"));

    // the second account's agents and parts set on their own
    let off = app
        .send_json("PUT", "/v1/outlook/agents", &h, &json!({"account": 2, "agents": ["coach"], "parts": {"mail": true, "calendar": false, "contacts": false}}))
        .await
        .json();
    assert_eq!(off["accounts"][1]["parts"]["calendar"], false, "{off}");
    assert_eq!(off["accounts"][0]["parts"]["calendar"], true, "the first's changed: {off}");

    // the second disconnected: the first untouched, its ids still known
    let (_, mine) = tool(&app, "mail_search", json!({}), "h1").await;
    let facture = id_of(&mine, "Votre facture");
    let gone = app.send_json("DELETE", "/v1/outlook?account=2", &h, &json!({})).await;
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert_eq!(gone.json()["account"], 2);
    let listed = app.get("/v1/outlook", &h).await.json();
    assert_eq!(listed["accounts"].as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed["account"], ACCOUNT);
    let store = bipbridge::connections::store_of(&config).read().unwrap();
    assert_eq!(store.outlook.as_ref().and_then(|a| a.link.account.clone()).as_deref(), Some(ACCOUNT));
    let (failed, read) = tool(&app, "mail_read", json!({"id": facture}), "h1").await;
    assert!(!failed && read.contains("EDF"), "the first account's ids forgotten: {read}");
    let (failed, said) = tool(&app, "mail_read", json!({"id": sam}), "h1").await;
    assert!(failed && said.contains("unknown e-mail id"), "{said}");
    let (failed, said) = tool(&app, "mail_search", json!({"mailbox": SECOND}), "h1").await;
    assert!(failed || !said.contains("Relevé de Sam"), "{said}");
    let always = app.get("/v1/approval-level", &h).await.json()["always"]["coach"].clone();
    assert!(!always.to_string().contains("outlook:2"), "the second's « Toujours » kept: {always}");
    // an older app's « Déconnecter » (no account): the first one, as before
    let gone = app.send_json("DELETE", "/v1/outlook", &h, &json!({})).await;
    assert_eq!(gone.status, 200);
    assert_eq!(app.get("/v1/outlook", &h).await.json()["state"], "not_connected");
}

/// The first account disconnected while a second stays: the second becomes the first (the old fields show it, an older
/// bridge reading the store sees it), its tokens still work.
#[tokio::test]
async fn the_first_account_gone_the_second_goes_on() {
    let (url, graph) = fake_graph().await;
    add_second(&graph);
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    assert_eq!(connect(&app, &h, "second").await.0, 200);
    let gone = app.send_json("DELETE", "/v1/outlook?account=1", &h, &json!({})).await;
    assert_eq!(gone.status, 200, "{}", gone.text());
    let listed = app.get("/v1/outlook", &h).await.json();
    assert_eq!((listed["account"].as_str(), listed["state"].as_str()), (Some(SECOND), Some("connected")), "{listed}");
    let store = bipbridge::connections::store_of(&config).read().unwrap();
    assert_eq!(store.outlook.as_ref().and_then(|a| a.link.account.clone()).as_deref(), Some(SECOND));
    let (failed, text) = tool(&app, "mail_search", json!({}), "h1").await;
    assert!(!failed && text.contains("Relevé de Sam") && !text.contains("Votre facture"), "{text}");
}

// -- 4. contacts added and changed -------------------------------------------------------------------------------------

/// « Ajouter et changer des contacts » switched on: the sign-in asks `Contacts.ReadWrite` (not `Contacts.Read`), the
/// agent gets `contacts_add` and `contacts_update` (never a deletion); a contact added on a card (« Tout demander »),
/// then changed at once (« Seulement le critique », its line kept); what it writes is checked first (a changed contact
/// is not written over). Switched off: read only again.
#[tokio::test]
async fn a_contact_created_then_changed_on_cards_by_the_level() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let parts = json!({"mail": false, "calendar": false, "contacts": true, "contacts_write": true});
    let (status, shown) = connect_with(&app, &h, "good", json!({"agents": ["coach"], "parts": parts})).await;
    assert_eq!(status, 200, "{shown}");
    let asked = graph.lock().unwrap().requests.iter().find(|r| r.1 == "/token").map(|r| form(&r.3)["scope"].clone()).unwrap();
    assert_eq!(asked, "offline_access User.Read Contacts.ReadWrite");
    assert_eq!((shown["parts"].clone(), shown["missing"].clone()), (parts.clone(), json!([])), "{shown}");
    let mut named = tools_named(&app, "contacts_").await;
    named.sort();
    assert_eq!(named, vec!["contacts_add", "contacts_birthdays", "contacts_list_group", "contacts_search", "contacts_update"]);

    // added: a card first (« Non »: nothing written), then « Oui »
    let add = json!({"name": "Nina Durand", "phones": ["+33 6 00 00 00 01"], "emails": ["nina@example.org"],
                     "organization": "Kerfy", "birthday": "1991-04-02", "group": "Famille"});
    let ((failed, said), card) =
        tokio::join!(tool(&app, "contacts_add", add.clone(), "h1"), common::answer_card(&app, "bip-ok-", "deny"));
    assert!(failed && said.contains("said no"), "{said}");
    assert_eq!(card["bip_ok"]["kind"], "contacts_write", "{card}");
    assert!(card["bip_summary"].as_str().unwrap().contains("Nina Durand"), "{card}");
    assert!(card["bip_always"].is_string(), "{card}");
    assert!(writes(&graph).is_empty(), "written with no « Oui »");
    let ((failed, said), _) = tokio::join!(tool(&app, "contacts_add", add, "h1"), common::answer_card(&app, "bip-ok-", "once"));
    assert!(!failed && said.contains("Added"), "{said}");
    let made = graph.lock().unwrap().contacts.iter().find(|c| c["displayName"] == "Nina Durand").cloned().expect("not added");
    assert_eq!(made["mobilePhone"], "+33 6 00 00 00 01");
    assert_eq!(made["emailAddresses"], json!([{"address": "nina@example.org", "name": "Nina Durand"}]));
    assert_eq!((made["companyName"].as_str(), made["categories"].clone()), (Some("Kerfy"), json!(["Famille"])));
    assert!(made["birthday"].as_str().unwrap().starts_with("1991-04-02"), "{made}");
    // found by the search, its id an Outlook one
    let (failed, found) = tool(&app, "contacts_search", json!({"query": "nina"}), "h1").await;
    assert!(!failed && found.contains("Nina Durand"), "{found}");
    let id = id_of(&found, "Nina Durand");
    assert!(id.starts_with("outlook:"), "{id} / {found}");

    // changed at once under « Seulement le critique », its line kept; a phone added, never one removed
    level(&app, &h, "critical").await;
    let (failed, said) =
        tool(&app, "contacts_update", json!({"id": id, "job": "Designer", "add_phones": ["+33 1 00 00 00 02"]}), "h2").await;
    assert!(!failed && said.contains("Changed"), "{said}");
    assert!(cards(&app, &h, "bip-ok-").await.is_empty());
    {
        let g = graph.lock().unwrap();
        let c = g.contacts.iter().find(|c| c["displayName"] == "Nina Durand").unwrap();
        assert_eq!(c["jobTitle"], "Designer");
        assert_eq!(c["mobilePhone"], "+33 6 00 00 00 01", "a phone removed");
        assert_eq!(c["homePhones"], json!(["+33 1 00 00 00 02"]));
    }
    let patch = graph.lock().unwrap().requests.iter().rev().find(|r| r.0 == "PATCH").cloned().unwrap();
    assert!(!patch.4.is_empty(), "written without its etag: {patch:?}");
    let line = approved_lines(&app, &h).await.into_iter().find(|l| l["kind"] == "contacts_write").expect("its line");
    assert!(line["summary"].as_str().unwrap().contains("Nina Durand"), "{line}");
    // never a deletion, never someone else's contact id
    let (failed, said) = tool(&app, "contacts_update", json!({"id": "outlook:99999", "job": "x"}), "h2").await;
    assert!(failed && said.contains("unknown contact id"), "{said}");
    // its notes are never read: never written over
    let (failed, said) = tool(&app, "contacts_update", json!({"id": id, "notes": "écrasé"}), "h2").await;
    assert!(failed && said.contains("notes"), "{said}");

    // the switch off: read only again
    let off = json!({"agents": ["coach"], "parts": {"mail": false, "calendar": false, "contacts": true, "contacts_write": false}});
    assert_eq!(app.send_json("PUT", "/v1/outlook/agents", &h, &off).await.status, 200);
    let mut named = tools_named(&app, "contacts_").await;
    named.sort();
    assert_eq!(named, vec!["contacts_birthdays", "contacts_list_group", "contacts_search"]);
    let (failed, said) = tool(&app, "contacts_add", json!({"name": "Autre"}), "h3").await;
    assert!(failed && said.contains("read only"), "{said}");
}

/// The write switched on after a read-only sign-in: Microsoft hasn't given `Contacts.ReadWrite` yet, so it is
/// « missing » (no write tool) until « Reconnecter » asks it.
#[tokio::test]
async fn the_contacts_write_switched_on_later_is_missing_until_reconnected() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    let shown = app.get("/v1/outlook", &h).await.json();
    assert_eq!(shown["parts"]["contacts_write"], false, "{shown}");
    let on = json!({"agents": ["coach"], "parts": {"mail": true, "calendar": true, "contacts": true, "contacts_write": true}});
    let on = app.send_json("PUT", "/v1/outlook/agents", &h, &on).await.json();
    assert_eq!(on["missing"], json!(["contacts_write"]), "{on}");
    assert!(!tools_named(&app, "contacts_").await.contains(&"contacts_add".to_string()));
    // an older app's PUT without `contacts_write`: kept as it was
    let old = json!({"agents": ["coach"], "parts": {"mail": true, "calendar": true, "contacts": true}});
    let kept = app.send_json("PUT", "/v1/outlook/agents", &h, &old).await.json();
    assert_eq!(kept["parts"]["contacts_write"], true, "{kept}");
    let (status, shown) = connect(&app, &h, "good").await;
    assert_eq!(status, 200, "{shown}");
    let asked = graph.lock().unwrap().requests.iter().rev().find(|r| r.1 == "/token").map(|r| form(&r.3)["scope"].clone()).unwrap();
    assert!(asked.contains("Contacts.ReadWrite") && !asked.contains("Contacts.Read "), "{asked}");
    assert_eq!(shown["missing"], json!([]));
    assert!(tools_named(&app, "contacts_").await.contains(&"contacts_add".to_string()));
}

