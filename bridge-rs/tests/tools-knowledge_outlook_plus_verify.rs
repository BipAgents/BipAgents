//! Verification of the `outlook-plus` review fixes (24/11, briefs/review-outlook-plus.md « Vérification du correctif »):
//! what still fails of B2 against the small fake Microsoft (tests/common/outlook_graph.rs). Red, no fix here. No real
//! Microsoft account.
//!
//! B2's fix keeps the highest number given in `Stored.outlook_last`, set only when an account is connected by this
//! code. A number is still given again when that field is 0 or lost:
//! - an account kept before the field (connected by a bridge before `outlook-plus`, or by `outlook-plus` before the
//!   fix), then disconnected: the next account gets its number, and its routines, kept paused, are the new one's;
//! - a bridge before `outlook-plus` run again for a while (a rollback): it rewrites the encrypted store without
//!   `outlook_more` and `outlook_last`; back on this bridge, the next account gets the number of an account it never
//!   disconnected, whose standing rules (never stopped) then file the new account's mail.

mod common;
#[path = "common/outlook_graph.rs"]
mod graph;

use common::auth;
use graph::*;
use serde_json::json;

async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
}

/// What a bridge without the field leaves in the store: `outlook_last` 0 (and, for a rollback, no `outlook_more`).
fn as_an_older_bridge_left_it(config: &bipbridge::config::Config, drop_more: bool) {
    let store = bipbridge::connections::store_of(config);
    let mut stored = store.read().unwrap();
    stored.outlook_last = 0;
    if drop_more {
        stored.outlook_more.clear();
    }
    store.write(&stored).unwrap();
}

/// B2, an account kept before `outlook_last`: Léa's account (number 1) was connected before the fix; she sets a
/// routine on it, disconnects it, then connects her work account. The work account must get a number never given
/// (not 1), so that the routine kept paused for her personal mailbox is never the work mailbox's.
#[tokio::test]
async fn verify_an_account_kept_before_the_counter_never_gives_its_number_again() {
    let (url, graph) = fake_graph().await;
    add_second(&graph);
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    as_an_older_bridge_left_it(&config, false);
    let made = app
        .send_json("POST", "/v1/agents/coach/triggers", &h, &json!({
            "name": "Promos", "prompt": "Résume la promo.", "kind": "mail",
            "mail": {"account": "outlook-1", "from": "shop.example", "words": ""}}))
        .await;
    assert_eq!(made.status, 200, "{}", made.text());
    let gone = app.send_json("DELETE", "/v1/outlook", &h, &json!({})).await;
    assert_eq!(gone.status, 200, "{}", gone.text());
    let (status, shown) = connect(&app, &h, "second").await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["account"], SECOND, "{shown}");
    let number = shown["accounts"][0]["id"].clone();
    let on_new: Vec<String> = bipbridge::triggers::load(&config)
        .triggers
        .iter()
        .filter(|t| t.mail.as_ref().is_some_and(|m| m.account == format!("outlook-{number}")))
        .map(|t| t.name.clone())
        .collect();
    assert!(
        number != json!(1) && on_new.is_empty(),
        "number {number} given again: the routine set on the disconnected mailbox is now the new one's: {on_new:?}"
    );
}

/// B2, after a rollback: Léa (number 1) and Sam (number 2) connected, a standing rule on Sam's mailbox (archive the
/// shop's mail). A bridge before `outlook-plus` runs for a while and rewrites the store (no `outlook_more`, no
/// `outlook_last`); back on this bridge, Tom connects his own Microsoft account. Tom must not get number 2: Sam's rule
/// (never stopped, Sam never disconnected) would file Tom's mail.
#[tokio::test]
async fn verify_after_a_rollback_no_number_is_given_again() {
    let (url, graph) = fake_graph().await;
    let second = add_second(&graph);
    let dir = tempfile::tempdir().unwrap();
    let config = config_for(dir.path(), &url, true);
    let app = common::TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let state = app.app.state.clone();
    assert_eq!(connect(&app, &h, "good").await.0, 200);
    let (status, shown) = connect(&app, &h, "second").await;
    assert_eq!(status, 200, "{shown}");
    let (failed, said) =
        tool(&app, "mail_bulk", json!({"action": "archive", "rule": true, "from": "news@shop.example", "mailbox": SECOND}), "h1").await;
    assert!(!failed && said.contains("Rule card"), "{said}");
    common::answer_card(&app, "bip-mailrule-", "once").await;
    settle().await;
    assert_eq!(bipbridge::mail_rules::rules_of(&config, None).len(), 1, "Sam's rule kept");
    // the older bridge's write; then another person's Microsoft account signs in (the fake's second slot)
    as_an_older_bridge_left_it(&config, true);
    second.lock().unwrap().account = "tom@kerfy.example".to_string();
    second.lock().unwrap().messages.clear();
    let (status, shown) = connect(&app, &h, "second").await;
    assert_eq!(status, 200, "{shown}");
    let number = shown["accounts"].as_array().unwrap().iter().find(|a| a["account"] == "tom@kerfy.example").unwrap()["id"].clone();
    bipbridge::outlook_watch::look(&state).await;
    arrives(&second, "t9", ("Shop", "news@shop.example"), "Promo pour Tom", "-20 %");
    bipbridge::outlook_watch::look(&state).await;
    settle().await;
    let filed = second.lock().unwrap().messages["t9"]["parentFolderId"].clone();
    assert!(
        number != json!(2) && filed == "inbox-id",
        "number {number} given again after a rollback: Sam's rule filed Tom's new mail into {filed}"
    );
}
