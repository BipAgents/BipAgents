//! « Mets à jour l'app » (BipAgents 1.1, briefs/design-app-update-required.md, Alex's OK of 21/11): the oldest app
//! builds this bridge still works with, per platform, and the routes retired for old apps.
//!
//! - `app_floor` in `GET /v1/status` (the channel every app reads at each refresh) and `GET /health`, and
//!   `"app_floor"` in `/health`'s `features`:
//!   `{"ios": {"min": 202611201200, "next": 202612011200, "next_from": 1798498800}, "android": {"min": …}}`.
//!   Under `min` the app is too old now (0: nothing to check); under `next` it will be from `next_from` (Unix
//!   seconds; both absent when nothing is planned). The SERVER turns `next` into `min` once its date has passed: a
//!   phone's clock decides nothing. An iPhone build (CFBundleVersion without its dot) and an Android versionCode do not
//!   compare: two floors.
//! - Only for an app that says which build it is (`X-Bip-App: ios/202612011200/1.1`, platform / build / version, sent
//!   by the 1.1 apps on every request to their bridge and to the guichet). An app of before 1.1 sends nothing and gets
//!   exactly today's answers, byte for byte (tests/app_floor.rs); a malformed header counts as none. The header names
//!   no one (no device, no account).
//! - The code's constants below are the floor; `[app_floor]` in bridge.toml (same shape: `[app_floor.ios] min, next,
//!   next_from`) can only raise it (test journeys, an urgent floor on one place). Raising the code's floor follows the
//!   rule of internal/notes/release-process.md « Plancher des apps » (4 weeks at least, the build at 100 % on both
//!   stores, never above what runs on the oldest system accepted).
//! - A route retired for old apps answers `426 {"detail": "update the app", "code": "app_too_old"}` instead of 404
//!   for 3 months: its handler replaced by [`gone_for_old_apps`]; `[app_floor] gone = ["/v1/…"]` does the same for
//!   paths no route serves (test journeys; `{name}` matches one segment). A 1.1 app takes a 426 as « too old » for
//!   that bridge at once.

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use serde_json::{json, Map, Value};

use crate::web::HttpError;

/// The oldest iPhone build (CFBundleVersion « 20261201.1200 » as 202612011200) this bridge works with. 0: none.
pub const IOS_MIN: u64 = 0;
/// The oldest Android versionCode (`%y%j%H%M`, 263351200) this bridge works with. 0: none.
pub const ANDROID_MIN: u64 = 0;

/// The next floor, announced ahead: from `from` (Unix seconds) the builds under these are too old. `announced`: when
/// it was committed (the 4 weeks are counted from it, `floor_dates_keep_four_weeks`). 0: nothing for that platform.
#[derive(Clone, Copy, Debug)]
pub struct Next {
    pub ios: u64,
    pub android: u64,
    pub from: i64,
    pub announced: i64,
}

/// Nothing planned yet (BipAgents 1.1 is the first app that reads the floor).
pub const NEXT: Option<Next> = None;

/// The least time between a floor's announcement and its date (Alex, 21/11: « 4 semaines au moins »).
pub const FOUR_WEEKS: i64 = 28 * 86_400;

/// A retired route keeps answering 426 this long, then a plain 404 (design §1).
pub const GONE_FOR: i64 = 92 * 86_400;

/// What `X-Bip-App` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Ios,
    Android,
}

/// An app as its `X-Bip-App` header says: `ios/202612011200/1.1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppId {
    pub platform: Platform,
    pub build: u64,
    pub version: String,
}

pub const HEADER: &str = "x-bip-app";

/// `ios/<build>/<version>` or `android/<build>/<version>`; anything else (another platform, a build that is not 1 to
/// 15 digits, a version not made of letters, digits and « . - + », more parts) is None: ignored.
pub fn parse_app(value: &str) -> Option<AppId> {
    let value = value.trim();
    if value.len() > 64 {
        return None;
    }
    let mut parts = value.split('/');
    let (platform, build, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let platform = match platform {
        "ios" => Platform::Ios,
        "android" => Platform::Android,
        _ => return None,
    };
    if build.is_empty() || build.len() > 15 || !build.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ok_version = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+');
    if version.is_empty() || version.len() > 16 || !version.bytes().all(ok_version) {
        return None;
    }
    Some(AppId { platform, build: build.parse().ok()?, version: version.to_string() })
}

/// The app a request comes from, when it says so (one `X-Bip-App`, well formed).
pub fn app_of(headers: &HeaderMap) -> Option<AppId> {
    let mut values = headers.get_all(HEADER).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    parse_app(value.to_str().ok()?)
}

/// A floor announced ahead: builds under `build` too old from `from` (Unix seconds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub build: u64,
    pub from: i64,
}

/// One platform's floor: too old under `min` now, and under `next.build` from `next.from`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Floor {
    pub min: u64,
    pub next: Option<Step>,
}

impl Floor {
    /// As it stands at `now` (the server's clock): a next whose date has come is the min; a next not above the min
    /// says nothing more.
    pub fn at(self, now: i64) -> Floor {
        match self.next {
            Some(step) if step.from <= now => Floor { min: self.min.max(step.build), next: None },
            Some(step) if step.build <= self.min => Floor { min: self.min, next: None },
            _ => self,
        }
    }

    /// The stricter of two floors (both [`Floor::at`] the same time): the higher min, and of the nexts still above
    /// it the highest build at the earliest date: every build under either step is warned in time, never told a date
    /// later than its real one (a build between the two steps may be told the earlier date; once it has passed, the
    /// later step is told with its own: review 1, R2).
    pub fn raised_by(self, other: Floor) -> Floor {
        let min = self.min.max(other.min);
        let steps: Vec<Step> = [self.next, other.next].into_iter().flatten().filter(|step| step.build > min).collect();
        let next = match (steps.iter().map(|s| s.build).max(), steps.iter().map(|s| s.from).min()) {
            (Some(build), Some(from)) => Some(Step { build, from }),
            _ => None,
        };
        Floor { min, next }
    }

    /// `{"min": …}`, with `next` and `next_from` when one is planned.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("min".into(), json!(self.min));
        if let Some(step) = self.next {
            out.insert("next".into(), json!(step.build));
            out.insert("next_from".into(), json!(step.from));
        }
        Value::Object(out)
    }
}

/// `[app_floor]`: floors that can only raise the code's, and paths retired for old apps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub ios: Floor,
    pub android: Floor,
    /// Paths answered 426 when no route serves them (`{name}`: one segment).
    pub gone: Vec<String>,
}

impl Settings {
    /// The same shape as the answer: `[app_floor.ios] min = …, next = …, next_from = …`, `[app_floor] gone = […]`.
    /// A value that is not a positive integer is ignored, and so is a `next` without its `next_from`.
    pub fn from_table(table: &Value) -> Settings {
        let platform = |key: &str| {
            let t = table.get(key).filter(|v| v.is_object());
            let int = |k: &str| t.and_then(|t| t.get(k)).and_then(Value::as_u64).filter(|v| *v > 0);
            let next = match (int("next"), int("next_from")) {
                (Some(build), Some(from)) => i64::try_from(from).ok().map(|from| Step { build, from }),
                _ => None,
            };
            Floor { min: int("min").unwrap_or(0), next }
        };
        let gone = table
            .get("gone")
            .and_then(Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|p| p.starts_with('/') && p.len() > 1)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Settings { ios: platform("ios"), android: platform("android"), gone }
    }
}

/// The code's own floors (the constants), before any date.
pub fn code_floors() -> (Floor, Floor) {
    let step = |build: u64| NEXT.filter(|_| build > 0).map(|n| Step { build, from: n.from });
    (
        Floor { min: IOS_MIN, next: NEXT.and_then(|n| step(n.ios)) },
        Floor { min: ANDROID_MIN, next: NEXT.and_then(|n| step(n.android)) },
    )
}

/// Both platforms' floors at `now`: the code's, raised by the config's.
pub fn floors(settings: &Settings, now: i64) -> (Floor, Floor) {
    let (ios, android) = code_floors();
    (ios.at(now).raised_by(settings.ios.at(now)), android.at(now).raised_by(settings.android.at(now)))
}

/// `app_floor` as the apps read it, at `now`.
pub fn json_at(settings: &Settings, now: i64) -> Value {
    let (ios, android) = floors(settings, now);
    json!({"ios": ios.to_json(), "android": android.to_json()})
}

/// `app_floor` for this request's answer: None for an app that does not say which build it is (its answer stays
/// today's).
pub fn for_request(settings: &Settings, headers: &HeaderMap) -> Option<Value> {
    app_of(headers)?;
    Some(json_at(settings, crate::py::time_time() as i64))
}

/// The rule's 4 weeks between a floor's announcement and its date.
pub fn keeps_four_weeks(next: &Next) -> bool {
    next.from - next.announced >= FOUR_WEEKS
}

/// The 426 of a route retired for old apps.
pub fn too_old() -> HttpError {
    HttpError::with_code(426, "update the app", "app_too_old")
}

/// A retired route's handler, for any method: 426 for 3 months ([`GONE_FOR`]) from `retired` (Unix seconds, the day
/// the route went), then a plain 404 as any path no route serves: `.route("/v1/old", app_floor::gone_for_old_apps(T))`.
pub fn gone_for_old_apps<S: Clone + Send + Sync + 'static>(retired: i64) -> MethodRouter<S> {
    axum::routing::any(move || async move {
        if crate::py::time_time() as i64 >= retired + GONE_FOR {
            HttpError::new(404, "Not Found").into_response()
        } else {
            too_old().into_response()
        }
    })
}

/// `Vary: X-Bip-App` on the answers that depend on it (`/v1/status`, `/health`): a cache between never mixes an old
/// app's answer with a new one's (review 1, R1).
pub fn vary(mut response: Response) -> Response {
    response.headers_mut().insert(axum::http::header::VARY, axum::http::HeaderValue::from_static("x-bip-app"));
    response
}

/// A path `[app_floor] gone` retires (`{name}` matches one segment, the rest exactly).
pub fn is_gone(settings: &Settings, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    settings.gone.iter().any(|pattern| {
        let pattern: Vec<&str> = pattern.split('/').collect();
        pattern.len() == parts.len()
            && pattern.iter().zip(&parts).all(|(p, part)| {
                (p.starts_with('{') && p.ends_with('}') && p.len() > 2 && !part.is_empty()) || p == part
            })
    })
}

/// The answer to a path no route serves, when `[app_floor] gone` retires it.
pub fn gone_answer(settings: &Settings, path: &str) -> Option<Response> {
    is_gone(settings, path).then(|| too_old().into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    const T: i64 = 1_798_498_800; // 29/12/2026 00:00 Paris

    fn floor(min: u64, next: Option<(u64, i64)>) -> Floor {
        Floor { min, next: next.map(|(build, from)| Step { build, from }) }
    }

    #[test]
    fn next_turns_into_min_on_its_date() {
        let f = floor(100, Some((200, T)));
        assert_eq!(f.at(T - 1), f); // the day before: announced
        assert_eq!(f.at(T), floor(200, None)); // from its date: the min
        assert_eq!(f.at(T + 30 * DAY), floor(200, None));
    }

    #[test]
    fn no_next_after_its_date() {
        assert_eq!(floor(0, Some((200, T))).at(T + 1).next, None);
        assert_eq!(floor(0, Some((200, T))).at(T + 1).to_json(), json!({"min": 200}));
        // a next not above the min says nothing more
        assert_eq!(floor(300, Some((200, T))).at(T - DAY), floor(300, None));
        assert_eq!(floor(300, Some((300, T))).at(T - DAY), floor(300, None));
    }

    #[test]
    fn the_strictest_floor_wins() {
        let now = T - 10 * DAY;
        assert_eq!(floor(5, None).raised_by(floor(9, None)), floor(9, None));
        assert_eq!(floor(9, None).raised_by(floor(5, None)), floor(9, None));
        // two steps ahead: the highest build at the earliest date, so every build under either is warned in time (never
        // a date later than its real one; review 1, R2: an earlier, lower step of bridge.toml hid the code's)
        let a = floor(5, Some((20, T)));
        let b = floor(5, Some((30, T + 7 * DAY)));
        assert_eq!(a.at(now).raised_by(b.at(now)), floor(5, Some((30, T))));
        assert_eq!(b.at(now).raised_by(a.at(now)), floor(5, Some((30, T))));
        // once the first date passed, the second step is told with its own date
        assert_eq!(a.at(T).raised_by(b.at(T)), floor(20, Some((30, T + 7 * DAY))));
        // same date: the higher build
        assert_eq!(floor(5, Some((20, T))).raised_by(floor(5, Some((30, T)))), floor(5, Some((30, T))));
        // a next under the other's min goes
        assert_eq!(floor(5, Some((20, T))).raised_by(floor(25, None)), floor(25, None));
    }

    #[test]
    fn floor_dates_keep_four_weeks() {
        // The rule (internal/notes/release-process.md « Plancher des apps »): an old path stays at least 4 weeks
        // after the next floor is announced.
        if let Some(next) = NEXT {
            assert!(keeps_four_weeks(&next), "NEXT.from - NEXT.announced must be at least 28 days: {next:?}");
            assert!(next.ios > IOS_MIN || next.ios == 0, "{next:?}");
            assert!(next.android > ANDROID_MIN || next.android == 0, "{next:?}");
        }
        let n = |announced: i64, from: i64| Next { ios: 2, android: 2, from, announced };
        assert!(keeps_four_weeks(&n(T - 28 * DAY, T)));
        assert!(!keeps_four_weeks(&n(T - 28 * DAY + 1, T)));
        assert!(!keeps_four_weeks(&n(T, T - DAY)));
    }

    #[test]
    fn the_apps_header_is_read_strictly() {
        assert_eq!(
            parse_app("ios/202612011200/1.1"),
            Some(AppId { platform: Platform::Ios, build: 202_612_011_200, version: "1.1".into() })
        );
        assert_eq!(
            parse_app("android/263351200/1.1.2"),
            Some(AppId { platform: Platform::Android, build: 263_351_200, version: "1.1.2".into() })
        );
        for bad in ["", "ios", "ios/1", "ios/x/1.1", "ios/1/", "mac/1/1.1", "ios/1/1.1/2", "ios/+1/1.1", "ios/1/é"] {
            assert_eq!(parse_app(bad), None, "{bad:?}");
        }
        let mut headers = HeaderMap::new();
        headers.insert("x-bip-app", "android/12/1.1".parse().unwrap());
        assert_eq!(app_of(&headers).map(|a| a.build), Some(12));
        assert_eq!(app_of(&HeaderMap::new()), None);
    }

    #[test]
    fn settings_read_the_same_shape_as_the_answer() {
        let s = Settings::from_table(&json!({
            "ios": {"min": 10, "next": 20, "next_from": T},
            "android": {"min": 3},
            "gone": ["/v1/a", "", 4, "no-slash"],
        }));
        assert_eq!(s.ios, floor(10, Some((20, T))));
        assert_eq!(s.android, floor(3, None));
        assert_eq!(s.gone, vec!["/v1/a".to_string()]);
        assert_eq!(Settings::from_table(&json!({})), Settings::default());
    }
}
