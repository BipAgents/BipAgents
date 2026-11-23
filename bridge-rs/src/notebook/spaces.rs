//! Perso and Pro (plan « Le sommaire » §2.3, brick S5; contracts/carnet/sommaire.md §3c). A shell until the code:
//! the tests are written first.

use serde_json::Value;

use super::categories::{self as c, Target};
use super::{Page, Part, Who};
use crate::config::Config;

/// The spaces a note may be in.
pub const SPACES: [&str; 2] = ["perso", "pro"];

/// Why « Ranger dans… » is refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refused {
    Category(c::Refused),
    SpaceUnknown,
}

impl Refused {
    pub fn code(&self) -> &'static str {
        match self {
            Refused::Category(r) => r.code(),
            Refused::SpaceUnknown => "space_unknown",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Refused::Category(c::Refused::NotFound) => 404,
            Refused::Category(c::Refused::HandWritten) => 409,
            _ => 400,
        }
    }
}

/// « Séparer Perso, Pro et Famille ».
pub fn on(_config: &Config) -> bool {
    false
}

pub fn set_on(_config: &Config, _on: bool) -> std::io::Result<()> {
    Ok(())
}

/// A Pro agent's new page, when on.
pub fn on_write(_config: &Config, _part: Part, _path: &str, _page: &mut Page, _who: &Who, _created: bool) -> bool {
    false
}

/// « Ranger ailleurs » / « Ranger dans… ».
pub fn categorize(
    config: &Config,
    target: &Target,
    categories: &[String],
    _space: Option<&str>,
    lang: &str,
    who: &Who,
) -> anyhow::Result<Result<Value, Refused>> {
    Ok(c::categorize(config, target, categories, who)?
        .map(|v| super::categories_pass::with_toast(v, lang))
        .map_err(Refused::Category))
}

pub fn undo(config: &Config, id: &str, _lang: &str, who: &Who) -> anyhow::Result<Option<Value>> {
    c::undo(config, id, who)
}

pub fn today(config: &Config, lang: &str) -> Value {
    super::categories_pass::today(config, lang)
}
