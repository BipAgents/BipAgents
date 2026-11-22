//! « Mets à jour l'app » (BipAgents 1.1, briefs/design-app-update-required.md): the oldest app builds this bridge
//! still works with (`app_floor` in `GET /v1/status` and `GET /health`), the apps' `X-Bip-App` header, and the 426
//! `app_too_old` of a route retired for old apps.
//!
//! Contract: additive only. An app of before 1.1 sends no `X-Bip-App`: it gets exactly today's answers (the same JSON,
//! byte for byte); `app_floor` (and the `app_floor` feature) go only to an app that says which build it is.

mod common;

use axum::http::StatusCode;
use axum::Router;
use bipbridge::app_floor::{self, Floor, Step};
use common::*;
use serde_json::{json, Value};

const IOS_APP: &str = "ios/202612011200/1.1";
const ANDROID_APP: &str = "android/263351200/1.1";

fn app_with(floor: Option<Value>) -> (tempfile::TempDir, TestApp) {
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    if let Some(floor) = floor {
        dict["app_floor"] = floor;
    }
    let config = bipbridge::config::parse_config(&dict, None).unwrap();
    (dir, TestApp::new(config))
}

async fn status(app: &TestApp, x_bip_app: Option<&str>) -> (StatusCode, Value, Vec<u8>) {
    let (k, v) = auth();
    let mut headers = vec![(k, v.as_str())];
    if let Some(x) = x_bip_app {
        headers.push(("x-bip-app", x));
    }
    let resp = app.get("/v1/status", &headers).await;
    let body = resp.body.clone();
    (resp.status, resp.json(), body)
}

async fn health(app: &TestApp, x_bip_app: Option<&str>) -> (Value, Vec<u8>) {
    let headers: Vec<(&str, &str)> = x_bip_app.map(|x| vec![("x-bip-app", x)]).unwrap_or_default();
    let resp = app.get("/health", &headers).await;
    assert_eq!(resp.status, StatusCode::OK);
    (resp.json(), resp.body.clone())
}

/// What the code itself announces (the constants), as the apps read it.
fn code_floor() -> Value {
    let (ios, android) = app_floor::code_floors();
    json!({"ios": ios.at(now()).to_json(), "android": android.at(now()).to_json()})
}

fn now() -> i64 {
    bipbridge::py::time_time() as i64
}

#[tokio::test]
async fn status_carries_app_floor() {
    let (_dir, app) = app_with(None);
    for who in [IOS_APP, ANDROID_APP] {
        let (code, body, _) = status(&app, Some(who)).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["app_floor"], code_floor(), "{who}");
        // both platforms, each with its `min` (0: nothing to check)
        assert!(body["app_floor"]["ios"]["min"].is_u64() && body["app_floor"]["android"]["min"].is_u64());
    }
    // the rest of the answer is today's
    let (_, plain, _) = status(&app, None).await;
    let (_, mut with, _) = status(&app, Some(IOS_APP)).await;
    with.as_object_mut().unwrap().remove("app_floor");
    assert_eq!(with, plain);
}

#[tokio::test]
async fn health_carries_app_floor_and_feature() {
    let (_dir, app) = app_with(None);
    let (plain, _) = health(&app, None).await;
    let (with, _) = health(&app, Some(ANDROID_APP)).await;
    assert_eq!(with["app_floor"], code_floor());
    let features: Vec<&str> = with["features"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(features.last(), Some(&"app_floor"));
    // the same features as before, then « app_floor »
    let mut expected = plain["features"].as_array().unwrap().clone();
    expected.push(json!("app_floor"));
    assert_eq!(with["features"], Value::Array(expected));
    let mut rest = with.clone();
    rest.as_object_mut().unwrap().remove("app_floor");
    rest["features"] = plain["features"].clone();
    assert_eq!(rest, plain);
}

#[tokio::test]
async fn requests_without_x_bip_app_still_work() {
    // An app of before 1.1 (it sends nothing): exactly today's answers, byte for byte, even with a floor set.
    let (_dir, bare) = app_with(None);
    let (_dir2, floored) =
        app_with(Some(json!({"ios": {"min": 209912311200i64}, "android": {"min": 999999999}})));
    let (code, status_bare, bytes_bare) = status(&bare, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(status_bare, json!({"agents": {"wellness": {"ok": false, "error": "ConnectError"}}, "model": null, "protected": false, "jobs": []}));
    let (_, _, bytes_floored) = status(&floored, None).await;
    assert_eq!(bytes_floored, bytes_bare);
    let (health_bare, hb) = health(&bare, None).await;
    let (_, hf) = health(&floored, None).await;
    assert_eq!(hf, hb);
    assert!(health_bare.get("app_floor").is_none());
    assert!(!health_bare["features"].as_array().unwrap().contains(&json!("app_floor")));
}

#[tokio::test]
async fn malformed_x_bip_app_is_ignored() {
    let (_dir, app) = app_with(None);
    let (_, plain, plain_bytes) = status(&app, None).await;
    let (_, health_plain) = health(&app, None).await;
    for bad in [
        "", "ios", "ios/", "ios/abc/1.1", "ios/-5/1.1", "ios/12/", "ios//1.1", "windows/12/1.1", "IOS/12/1.1",
        "ios/12/1.1/extra", "ios/12/1.1;x", "ios/12/1 1", "ios/1234567890123456/1.1",
        "ios/12/12345678901234567", "android/12/<script>",
    ] {
        let (code, body, bytes) = status(&app, Some(bad)).await;
        assert_eq!(code, StatusCode::OK, "{bad:?}");
        assert_eq!(body, plain, "{bad:?}");
        assert_eq!(bytes, plain_bytes, "{bad:?}");
        assert_eq!(health(&app, Some(bad)).await.1, health_plain, "{bad:?}");
    }
    for good in ["ios/1/1.1", "android/263351200/1.1.2", "ios/202612011200/1.1-beta", " ios/12/1.1 "] {
        assert!(status(&app, Some(good)).await.1.get("app_floor").is_some(), "{good}");
    }
}

#[tokio::test]
async fn config_floor_only_raises() {
    let (code_ios, code_android) = app_floor::code_floors();
    // lower than the code: ignored
    let (_dir, low) = app_with(Some(json!({"ios": {"min": 1}, "android": {"min": 1}})));
    let floor = status(&low, Some(IOS_APP)).await.1["app_floor"].clone();
    assert_eq!(floor["ios"]["min"], json!(code_ios.at(now()).min.max(1)));
    assert_eq!(floor["android"]["min"], json!(code_android.at(now()).min.max(1)));
    // higher: wins, for its platform only
    let (_dir2, high) = app_with(Some(json!({"ios": {"min": 209912311200i64}})));
    let floor = status(&high, Some(IOS_APP)).await.1["app_floor"].clone();
    assert_eq!(floor["ios"], json!({"min": 209912311200i64}));
    assert_eq!(floor["android"], code_android.at(now()).to_json());
    // a next announced in the config, its date to come: given as it is
    let later = now() + 40 * 86400;
    let (_dir3, next) = app_with(Some(json!({"android": {"min": 5, "next": 263351200, "next_from": later}})));
    let floor = status(&next, Some(ANDROID_APP)).await.1["app_floor"].clone();
    assert_eq!(floor["android"], json!({"min": code_android.at(now()).min.max(5), "next": 263351200, "next_from": later}));
    // nonsense in the config is ignored (a negative build, a next without its date, a string)
    let (_dir4, odd) =
        app_with(Some(json!({"ios": {"min": -4, "next": 12}, "android": {"min": "big", "next_from": later}})));
    let floor = status(&odd, Some(IOS_APP)).await.1["app_floor"].clone();
    assert_eq!(floor, code_floor());
}

#[tokio::test]
async fn next_turns_into_min_by_the_servers_clock() {
    // The server's own clock decides (the phone's never does): a next whose date passed is given as the min.
    let past = now() - 60;
    let (_dir, app) = app_with(Some(json!({"ios": {"next": 209912311200i64, "next_from": past}})));
    let floor = status(&app, Some(IOS_APP)).await.1["app_floor"].clone();
    assert_eq!(floor["ios"], json!({"min": 209912311200i64}));
    let health_floor = health(&app, Some(IOS_APP)).await.0["app_floor"].clone();
    assert_eq!(health_floor, floor);
}

#[tokio::test]
async fn gone_route_answers_426_app_too_old() {
    // the helper a retired route's handler is replaced with (any method), from the day it was retired
    let retired = now() - 10 * 86_400;
    let router: Router = Router::new()
        .route("/v1/old-thing", app_floor::gone_for_old_apps(retired))
        .route("/v1/long-gone", app_floor::gone_for_old_apps(now() - app_floor::GONE_FOR - 86_400));
    for method in ["GET", "POST", "PUT", "DELETE"] {
        let resp = tower::ServiceExt::oneshot(
            router.clone(),
            axum::http::Request::builder().method(method).uri("/v1/old-thing").body(axum::body::Body::empty()).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::UPGRADE_REQUIRED, "{method}");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"detail": "update the app", "code": "app_too_old"}));
    }
    // 3 months after: a plain 404, as any path no route serves (review 1, R3)
    let resp = tower::ServiceExt::oneshot(
        router.clone(),
        axum::http::Request::builder().uri("/v1/long-gone").body(axum::body::Body::empty()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // a path retired in bridge.toml (test journeys): 426 on the real app, whoever asks; other unknown paths stay 404
    let (_dir, app) = app_with(Some(json!({"gone": ["/v1/retired-for-test", "/v1/agents/{agent}/retired"]})));
    let (k, v) = auth();
    for (path, headers) in [
        ("/v1/retired-for-test", vec![]),
        ("/v1/retired-for-test", vec![(k, v.as_str())]),
        ("/v1/retired-for-test", vec![(k, v.as_str()), ("x-bip-app", IOS_APP)]),
        ("/v1/agents/wellness/retired", vec![(k, v.as_str())]),
    ] {
        let resp = app.get(path, &headers).await;
        assert_eq!(resp.status, StatusCode::UPGRADE_REQUIRED, "{path}");
        assert_eq!(resp.json(), json!({"detail": "update the app", "code": "app_too_old"}));
    }
    let resp = app.send_json("POST", "/v1/retired-for-test", &[(k, v.as_str())], &json!({})).await;
    assert_eq!(resp.status, StatusCode::UPGRADE_REQUIRED);
    for other in ["/v1/retired-for-tests", "/v1/agents/wellness/retired/more", "/v1/nothing-here"] {
        let resp = app.get(other, &[(k, v.as_str())]).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{other}");
    }
    // without the setting, the same path is a plain 404, as today
    let (_dir2, plain) = app_with(None);
    assert_eq!(plain.get("/v1/retired-for-test", &[(k, v.as_str())]).await.status, StatusCode::NOT_FOUND);
}

#[test]
fn floor_values_build_as_the_apps_read_them() {
    let f = Floor { min: 7, next: Some(Step { build: 9, from: 1_800_000_000 }) };
    assert_eq!(f.to_json(), json!({"min": 7, "next": 9, "next_from": 1_800_000_000i64}));
    assert_eq!(Floor::default().to_json(), json!({"min": 0}));
}

/// The answers that depend on `X-Bip-App` say so (review 1, R1): a cache between would never mix them up.
#[tokio::test]
async fn the_answers_vary_with_x_bip_app() {
    let (_dir, app) = app_with(None);
    let (k, v) = auth();
    for headers in [vec![(k, v.as_str())], vec![(k, v.as_str()), ("x-bip-app", IOS_APP)]] {
        let status = app.get("/v1/status", &headers).await;
        assert_eq!(status.header("vary").as_deref(), Some("x-bip-app"));
        let health = app.get("/health", &headers[1..]).await;
        assert_eq!(health.header("vary").as_deref(), Some("x-bip-app"));
    }
}
