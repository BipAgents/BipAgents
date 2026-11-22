//! The sommaire ready to show (brick S4): stubs while the tests are written first.

use chrono::NaiveDate;
use serde_json::{json, Value};

use crate::config::Config;

pub const RECENT: usize = 20;
pub const FILTERS: [(&str, &[&str]); 4] = [
    ("papers", &["invoices", "contracts", "warranties"]),
    ("money", &["to_pay", "subscriptions", "purchases"]),
    ("dates", &["this_week", "this_month", "later"]),
    ("people", &["close", "others"]),
];

#[derive(Debug, PartialEq)]
pub enum Refused {
    SpaceUnknown,
    CategoryUnknown,
    FilterUnknown,
    HealthLocked,
    CategoryNotGiven,
    KeptFull,
    KeptUnknown,
    KeptEmpty,
    NumberRefused,
}

pub fn filter_title(_lang: &str, id: &str) -> String {
    id.to_string()
}

pub fn space_asked(_asked: Option<&str>) -> Result<&'static str, Refused> {
    Ok("all")
}

pub fn summary_at(_config: &Config, _space: &str, _langs: &[String], _today: NaiveDate, _now: f64) -> Value {
    json!({})
}

#[allow(clippy::too_many_arguments)]
pub fn category_at(_config: &Config, _id: &str, _space: &str, _filter: Option<&str>, _langs: &[String], _private: bool, _today: NaiveDate) -> Result<Value, Refused> {
    Ok(json!({}))
}

pub fn kept_at(_config: &Config, _space: &str, _langs: &[String], _today: NaiveDate) -> Value {
    json!({})
}

pub fn keep(_config: &Config, _q: &str, _title: Option<&str>, _space: &str, _category: Option<&str>) -> Result<String, Refused> {
    Ok(String::new())
}

pub fn rename(_config: &Config, _id: &str, _title: &str) -> Result<String, Refused> {
    Ok(String::new())
}

pub fn drop_kept(_config: &Config, _id: &str) -> Result<String, Refused> {
    Ok(String::new())
}

pub fn searched_at(_config: &Config, _q: &str, _category: Option<&str>, _space: &str, _langs: &[String], _today: NaiveDate) -> Result<bool, Refused> {
    Ok(false)
}

pub fn searched_path(config: &Config) -> std::path::PathBuf {
    super::root(config).join("searched.json")
}
