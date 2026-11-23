//! The family Carnet's sommaire (plan « Le sommaire » §4, brick S6; contracts/carnet/sommaire.md §3d). Shell: the
//! tests come first.

use serde_json::Value;

use crate::notebook::categories::Rule;
use crate::notebook::finder::Entry;
use crate::Dict;

/// The categories a family note's words give (shell).
pub fn decide(_title: &str, _text: &str, _rules: &[Rule]) -> (Vec<String>, Option<String>) {
    (vec!["other".into()], None)
}

/// The family Carnet's notes as the phone's copy holds them (shell).
pub fn entries(_data: &Dict) -> Vec<Entry> {
    Vec::new()
}

/// The pages and lists filed at a write (shell).
pub fn file_all(_data: &mut Dict) {}

#[allow(dead_code)]
fn unused(_: Value) {}

#[cfg(test)]
#[path = "family_carnet_tests.rs"]
mod tests;
