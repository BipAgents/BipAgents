//! Review of `outlook-plus` (24/11, briefs/review-outlook-plus.md): its blockers shown red against the small fake
//! Microsoft (tests/common/outlook_graph.rs), and a few checks of what holds. Red first, then fixed on `outlook-plus`. No real Microsoft account.
//!
//! - B1: a standing rule that files every new mail of a sender into the Deleted Items is the trash (outlook_mail.rs
//!   says so for a tidying, review R7), yet its card goes through under « Seulement le critique »;
//! - B2: an account's number is used again for the next account connected: the rules and routines of the account
//!   disconnected then act on the new one, a mailbox the person never set them on.
//! - « à corriger » (24/11 fixes): a burst of 50 mails or more within two minutes stopped the watch for good (C1); a
//!   mailbox or an address book named that isn't the agent's became its first account (C2); an empty string in
//!   `contacts_update` erased a field (C4).

mod common;
#[path = "common/outlook_graph.rs"]
mod graph;

use common::auth;
use graph::*;
use serde_json::{json, Value};

async fn level(app: &common::TestApp, h: &[(&str, &str)], level: &str) {
    assert_eq!(app.send_json("PUT", "/v1/approval-level", h, &json!({"level": level})).await.status, 200);
}

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

async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
}

/// B1. « La corbeille demande toujours une carte »: a rule `move:Éléments supprimés` (or `move:corbeille`, `move:trash`:
/// outlook_mail.rs' names of the Deleted Items) sends every new mail of that sender to the trash. Its card must wait for
/// the person at every level, as a `trash` rule's (`mail_rule_trash`) and as a tidying into the Deleted Items (R7).
/// Today `classify_card` reads `move:…` as `mail_rule` (not critical): `settle_new` answers it under « Seulement le
/// critique » and the sender's mail goes to the Deleted Items without the person.
#[tokio::test]
async fn review_a_rule_into_the_deleted_items_waits_for_the_person_at_every_level() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    level(&app, &h, "critical").await;

    // the control: a `trash` rule waits for its card (critical)
    let (failed, said) =
        tool(&app, "mail_bulk", json!({"action": "trash", "rule": true, "from": "help@service.example", "mailbox": "Outlook"}), "h1").await;
    assert!(!failed && !said.contains("Approved at once"), "{said}");
    assert_eq!(cards(&app, &h, "bip-mailrule-").await.len(), 1, "the trash rule's card");

    // the same trash, by the Deleted Items' name
    let (failed, said) = tool(
        &app,
        "mail_bulk",
        json!({"action": "move:Éléments supprimés", "rule": true, "from": "news@shop.example", "mailbox": "Outlook"}),
        "h2",
    )
    .await;
    assert!(!failed, "{said}");
    settle().await;
    let moved = graph.lock().unwrap().messages["m2"]["parentFolderId"].clone();
    let waiting = cards(&app, &h, "bip-mailrule-").await.len();
    assert!(
        moved == "inbox-id" && waiting == 2 && !said.contains("Approved at once"),
        "a rule into the Deleted Items let through by the level, without the person: m2 now in {moved}, {waiting} rule \
cards waiting (2 expected), the agent was told: {said}"
    );
}

/// B2. Account numbers are `max + 1` (outlook.rs `finish`): the last account disconnected, the next one connected gets
/// its number again, and so its mailbox's id `outlook-<n>`. Nothing takes the rules and routines of that mailbox away
/// at « Déconnecter »: Léa's rule (archive the shop's mail) and her routine, set on her personal mailbox, act on the
/// work account she connects afterwards. They must not: a rule or a routine acts only on the mailbox it was set on.
#[tokio::test]
async fn review_a_disconnected_accounts_rules_and_routines_never_act_on_the_next_account() {
    let (url, graph) = fake_graph().await;
    let second = add_second(&graph);
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let state = app.app.state.clone();
    // Léa's personal account: a rule and a routine on its inbox
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    let (failed, said) =
        tool(&app, "mail_bulk", json!({"action": "archive", "rule": true, "from": "news@shop.example", "mailbox": "Outlook"}), "h1").await;
    assert!(!failed && said.contains("Rule card"), "{said}");
    common::answer_card(&app, "bip-mailrule-", "once").await;
    let made = app
        .send_json("POST", "/v1/agents/coach/triggers", &h, &json!({
            "name": "Promos", "prompt": "Résume la promo.", "kind": "mail",
            "mail": {"account": "outlook-1", "from": "shop.example", "words": ""}}))
        .await;
    assert_eq!(made.status, 200, "{}", made.text());
    settle().await;
    // she disconnects it, then connects her work account (another address) for the same agent
    let gone = app.send_json("DELETE", "/v1/outlook", &h, &json!({})).await;
    assert_eq!(gone.status, 200, "{}", gone.text());
    let (status, shown) = connect(&app, &h, "second").await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["account"], SECOND, "{shown}");
    let number = shown["accounts"][0]["id"].clone();
    // its inbox watched (from its newest mail), then a shop's mail comes in
    bipbridge::outlook_watch::look(&state).await;
    arrives(&second, "n9", ("Shop", "news@shop.example"), "Promo pour Sam", "-20 %");
    bipbridge::outlook_watch::look(&state).await;
    settle().await;
    let filed = second.lock().unwrap().messages["n9"]["parentFolderId"].clone();
    let queued = bipbridge::triggers::load(&config).triggers.iter().map(|t| t.pending.len()).sum::<usize>();
    let rules = app.get("/v1/mail/rules?agent=coach", &h).await.json()["items"].clone();
    assert!(
        filed == "inbox-id" && queued == 0,
        "the disconnected account's rule and routine acted on the next account (number {number} again): its new mail \
now in {filed}, {queued} queued for the routine set on the other mailbox; rules shown: {rules}"
    );
}

/// What holds (verified, green): a sixth Microsoft account is refused with 409 `too_many` and nothing of it is kept;
/// the same address as one kept updates it, never a sixth.
#[tokio::test]
async fn review_a_sixth_account_is_refused_and_nothing_of_it_kept() {
    let (url, _graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let store = bipbridge::connections::store_of(&config);
    let mut stored = store.read().unwrap();
    for n in 1..=5u32 {
        let account = bipbridge::outlook::Account {
            link: bipbridge::drives::Link {
                tokens: bipbridge::connections::Tokens {
                    access_token: format!("other-{n}"),
                    scope: Some("Mail.ReadWrite User.Read".into()),
                    ..Default::default()
                },
                account: Some(format!("other{n}@example.org")),
                connected_at: 1.0,
                state: "connected".into(),
                agents: vec!["coach".into()],
                ..Default::default()
            },
            parts: bipbridge::outlook::Parts { mail: true, calendar: false, contacts: false, contacts_write: false },
            id: n,
        };
        if n == 1 {
            stored.outlook = Some(account);
        } else {
            stored.outlook_more.push(account);
        }
    }
    store.write(&stored).unwrap();
    let (status, said) = connect(&app, &h, "good").await;
    assert_eq!(status, 409, "{said}");
    assert_eq!(said["detail"]["code"], "too_many", "{said}");
    let after = bipbridge::outlook::all(&config);
    assert_eq!(after.len(), 5);
    assert!(after.iter().all(|a| a.link.account.as_deref() != Some(ACCOUNT)), "the sixth kept");
    let shown = app.get("/v1/outlook", &h).await.json();
    assert_eq!(shown["max_accounts"], 5);
    assert!(!shown.to_string().contains("other-1"), "a token in the view: {shown}");
}

/// C1: 50 mails or more received within two minutes never stop the watch. Each look reads from `after − 2 min`, the
/// oldest first: a page all seen, the next pages (`@odata.nextLink`) are read too, so `after` moves on and every later
/// mail of that inbox is offered to its routines and sorted by its rules.
#[tokio::test]
async fn review_a_burst_of_mail_never_stops_the_watch() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let state = app.app.state.clone();
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    let (failed, said) =
        tool(&app, "mail_bulk", json!({"action": "mark_read", "rule": true, "from": "alert@burst.example", "mailbox": "Outlook"}), "h1").await;
    assert!(!failed, "{said}");
    common::answer_card(&app, "bip-mailrule-", "once").await;
    settle().await;
    bipbridge::outlook_watch::look(&state).await; // from the newest mail on
    for i in 0..55 {
        arrives(&graph, &format!("b{i:02}"), ("Alerte", "alert@burst.example"), &format!("Alerte {i}"), "serveur");
    }
    // one more, a minute after the burst
    arrives(&graph, "late", ("Alerte", "alert@burst.example"), "Alerte tardive", "serveur");
    let later = (chrono::Utc::now() + chrono::Duration::seconds(60)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    graph.lock().unwrap().messages.get_mut("late").unwrap()["receivedDateTime"] = json!(later);
    for _ in 0..4 {
        bipbridge::outlook_watch::look(&state).await;
    }
    let g = graph.lock().unwrap();
    let left: Vec<String> =
        g.messages.iter().filter(|(id, m)| (id.starts_with('b') || *id == "late") && m["isRead"] == false).map(|(id, _)| id.clone()).collect();
    assert!(left.is_empty(), "never looked at again: {left:?}");
}

/// C2. A mailbox or an address book the agent names that isn't one of its (another account's address, another
/// account's `outlook-<n>`, an address of no account) is refused with plain words: never the agent's first account
/// instead. Coach has the first account's mail and contacts (written), the second account's contacts read only.
#[tokio::test]
async fn review_a_mailbox_or_book_named_that_isnt_the_agents_is_refused_never_the_first() {
    let (url, graph) = fake_graph().await;
    let second = add_second(&graph);
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let first = json!({"agents": ["coach"], "parts": {"mail": true, "calendar": false, "contacts": true, "contacts_write": true}});
    assert_eq!(connect_with(&app, &h, "good", first).await.0, 200);
    let read_only = json!({"agents": ["coach"], "parts": {"mail": false, "calendar": false, "contacts": true, "contacts_write": false}});
    let (status, shown) = connect_with(&app, &h, "second", read_only).await;
    assert_eq!(status, 200, "{shown}");
    level(&app, &h, "critical").await;

    // the mail: another account's address or number, an address of no account
    for wanted in [SECOND, "outlook-2", "travail@example.org", "Outlook (travail@example.org)"] {
        let (failed, said) =
            tool(&app, "mail_bulk", json!({"action": "archive", "domain": "shop.example", "mailbox": wanted}), "h1").await;
        assert!(failed, "{wanted}: tidied somewhere: {said}");
        let (failed, said) = tool(&app, "mail_search", json!({"mailbox": wanted}), "h1").await;
        assert!(failed && !said.contains("Votre facture"), "{wanted}: the first account searched instead: {said}");
    }
    assert_eq!(graph.lock().unwrap().messages["m2"]["parentFolderId"], "inbox-id", "the first account tidied");
    assert_eq!(second.lock().unwrap().messages["s2"]["parentFolderId"], "inbox-id", "the second account tidied");

    // the contacts: the second's book is read only for Coach, an unknown book is no book of its
    let before = graph.lock().unwrap().contacts.len();
    for book in [SECOND, "travail@example.org"] {
        let (failed, said) = tool(&app, "contacts_add", json!({"name": "Nina Durand", "book": book}), "h2").await;
        assert!(failed, "{book}: added somewhere: {said}");
    }
    assert_eq!(graph.lock().unwrap().contacts.len(), before, "added to the first account's contacts");
    assert_eq!(second.lock().unwrap().contacts.len(), 1, "added to the second account's contacts");
    // the control: its own book by its address
    let (failed, said) = tool(&app, "contacts_add", json!({"name": "Nina Durand", "book": ACCOUNT}), "h2").await;
    assert!(!failed && said.contains("Added"), "{said}");
    assert_eq!(graph.lock().unwrap().contacts.len(), before + 1);
}

/// C4. `contacts_update` only adds and changes: an empty `organization`, `job` or `nickname` changes nothing (no card,
/// no « Annuler » would bring it back).
#[tokio::test]
async fn review_an_empty_string_never_erases_a_contacts_field() {
    let (url, graph) = fake_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(config_for(dir.path(), &url, true));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let parts = json!({"agents": ["coach"], "parts": {"mail": false, "calendar": false, "contacts": true, "contacts_write": true}});
    assert_eq!(connect_with(&app, &h, "good", parts).await.0, 200);
    level(&app, &h, "critical").await;
    {
        let mut g = graph.lock().unwrap();
        let paul = g.contacts.iter_mut().find(|c| c["id"] == "c2").unwrap();
        paul["jobTitle"] = json!("Designer");
        paul["nickName"] = json!("Polo");
    }
    let (failed, found) = tool(&app, "contacts_search", json!({"query": "Paul"}), "h1").await;
    assert!(!failed, "{found}");
    let id = id_of(&found, "Paul");
    let (_, said) = tool(&app, "contacts_update", json!({"id": id, "organization": "", "job": " ", "nickname": ""}), "h1").await;
    let (_, said_too) = tool(&app, "contacts_update", json!({"id": id, "organization": "", "add_to_group": "Amis"}), "h1").await;
    let g = graph.lock().unwrap();
    let paul = g.contacts.iter().find(|c| c["id"] == "c2").unwrap();
    assert!(
        paul["companyName"] == "Kerfy" && paul["jobTitle"] == "Designer" && paul["nickName"] == "Polo",
        "a field erased by an empty string: {paul} (told: {said} / {said_too})"
    );
    assert_eq!(paul["categories"], json!(["Travail", "Amis"]), "the group still added: {said_too}");
}
