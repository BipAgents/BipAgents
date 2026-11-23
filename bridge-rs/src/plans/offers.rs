//! Offers to act in basic « Initiative » (passation-pro briefs/design-agents-proactifs.md §2.1, §3.1–3.5, §3.3,
//! §3b.4 « la même règle vaut en basique, par sorte », §6 row 6, §7.3; Alex 22/11: D1 A, D2 A, D4 B;
//! contracts/offers/cases.json). The agent offers to DO something, not only to remind: « Je regarde les hôtels ? »,
//! « Je te mets un rappel ? », « Je cherche le trajet ? ». ONE card (`bip-offer-<id>`), « Oui » / « Non merci ».
//!
//! - **What it can offer**: a closed list of seven kinds (`KINDS`), only those this agent can do here, now (`can`,
//!   `kinds_of`): its tool's description and its schema's enum list only those, and the others are refused
//!   (« Not done: you can't … here; don't offer it »). A kind its person keeps saying « Non merci » to is quiet for a
//!   while (`learning`): out of the list too.
//! - **Nothing before « Oui »**: no model, no lookup, nothing paid for. Then the bridge does it itself (`BY_BRIDGE`:
//!   a route timed by the streets with its map, a one-time reminder said as written, the event written as shown: no
//!   model), or the agent gets ONE turn in the offer's conversation with exactly the words under « i » (`yes_note`):
//!   search, draft, page, cart; drafts and searches only, nothing booked, paid or sent without its own card.
//! - **Only the person answers**: `offer` is critical (approval_level.rs: never « Seulement le critique », never a
//!   « Toujours »), a `calendar` offer is that write's own card (`calendar_write`); an agent other people talk to makes
//!   none; the call took the card (`card_place::taken`), so nothing resumes on its own.
//! - **The caps** (D2 A, shared with plans.rs): 2 offers per reply in the person's own turn (a plan's offers on its
//!   card count); unasked, one card waiting per agent and 2 a day, 4 a day for the place, plans and offers together;
//!   never twice the same offer (its kind, its subject) in 14 days; it expires after 24 h, or when its moment has
//!   passed, without a word (its decision kept as « expired », the apps 1.1 fold it into « Passée »).
//! - **The learning, per kind** (§3b.4): counters over 30 days, no text; two « Non merci » in a row, or at least 5
//!   offers with less than 20 % yes (an expired one half a no): quiet 30 days; a subject declined is not offered
//!   again for 30 days; the agent reads one line on what its person often declines or accepts (`learning_line`).
//! - **A plan's offers** (§3, n° 2): shown on its card as pills (their own generic cards for an app 1.0), gone with
//!   its « Non merci »; a moment that offers places its card at its time, no model (`at_moment`).
//!
//! Kept here (no text of an answered offer: counters and prints only):
//!
//!     <data>/offers.json (0600)   {offers: [...], made: [{agent, at, asked, run, pill}], answers: [{agent, kind, at,
//!                                  outcome}], same: [{agent, print, at}], topics: [{agent, print, at}],
//!                                  quiet: [{agent, kind, until}]}
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::TimeZone;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{domain_ok, fill, lang_of, person_zone, plain, shown_agent, text_arg, Called};
use crate::app::AppState;
use crate::background_turn::{Delivery, Pipeline, Turn};
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "offer_act";
/// The offer's card (`/v1/approvals`, `/v1/approve`).
pub const RUN_PREFIX: &str = "bip-offer-";
/// Its notification's key (notifications.rs `Kind::of_card`: a request's neutral line, as a plan's;
/// contracts/notifications/types/card.json `offer`).
pub const PUSH_KEY: &str = "offer_request";
/// Its validation's kind (approval_level.rs): critical. A calendar offer is `calendar_write`.
pub const KIND: &str = "offer";

/// The closed list (§3.3), in the order the tool lists them.
pub const KINDS: [&str; 7] = ["route", "reminder", "calendar", "search", "draft", "page", "cart"];
/// Done by the bridge itself at « Oui », no model.
pub const BY_BRIDGE: [&str; 3] = ["route", "reminder", "calendar"];
/// The caps (D2 A, §3.2).
pub const PER_REPLY: usize = 2;
pub const SAME_DAYS: f64 = 14.0;
pub const EXPIRES_SECONDS: f64 = 86400.0;
/// The learning (§3b.4): its window, and how long a kind or a subject stays quiet.
pub const QUIET_DAYS: f64 = 30.0;
pub const LOW_RATE: f64 = 0.2;
pub const LOW_RATE_MIN: usize = 5;
pub const LABEL_MAX: usize = 80;
/// A plan's offer, a pill on its card (§3.4).
pub const PILL_MAX: usize = 60;
pub const ABOUT_MAX: usize = 40;
pub const PROMPT_MAX: usize = 600;
pub const SAY_MAX: usize = 240;
pub const TITLE_MAX: usize = 120;
pub const PLACE_MAX: usize = 200;
/// An answer's outcome.
pub const YES: &str = "yes";
pub const NO: &str = "no";
pub const EXPIRED: &str = "expired";
/// The yes turn's budget (card_place.rs' resumed turn's) and how long it may run (a search, a browsing).
const TURN_BUDGET: i64 = crate::card_place::RESUME_BUDGET;
const TURN_SECONDS: f64 = 900.0;
const DAY: f64 = 86400.0;

// -- the store -----------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub agent: String,
    pub kind: String,
    /// The question on the card, the agent's words.
    pub label: String,
    /// What it is about: the card's top line « {agent} propose · {about} ».
    pub about: String,
    /// What the agent will be asked at « Oui » (search, draft, page, cart), exactly.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// What the bridge will do at « Oui » (route, reminder, calendar), checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// A cart's shop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// The plan it belongs to (plans.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Shown inside its plan's card (a pill): never pushed, never a card of its own in the caps.
    #[serde(default)]
    pub pill: bool,
    pub made: f64,
    /// Unix seconds: 24 h after it was made, or its moment (a reminder's time, an event's start) when sooner.
    pub expires: f64,
    /// Made in a turn of the person's own conversation.
    #[serde(default)]
    pub asked: bool,
    /// The agent's first plan or offer card: its « i » says where its suggestions are set.
    #[serde(default)]
    pub first: bool,
}

/// An offer card made (the caps count them).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Made {
    pub agent: String,
    pub at: f64,
    pub asked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default)]
    pub pill: bool,
}

/// An answer, for the learning: its kind and outcome only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub agent: String,
    pub kind: String,
    pub at: f64,
    pub outcome: String,
}

/// A print (never the words): an offer made (`same`, 14 days), a subject declined (`topics`, 30 days).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Print {
    pub agent: String,
    pub print: String,
    pub at: f64,
}

/// A kind quiet for an agent until then.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quiet {
    pub agent: String,
    pub kind: String,
    pub until: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub offers: Vec<Offer>,
    #[serde(default)]
    pub made: Vec<Made>,
    #[serde(default)]
    pub answers: Vec<Answer>,
    #[serde(default)]
    pub same: Vec<Print>,
    #[serde(default)]
    pub topics: Vec<Print>,
    #[serde(default)]
    pub quiet: Vec<Quiet>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("offers.json")
}

pub fn load(config: &Config) -> Store {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Read, change, write (0600), under one lock; what is older than its keeping dropped.
pub fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    let out = f(&mut store);
    let now = py::time_time();
    store.made.retain(|m| now - m.at < 2.0 * DAY);
    store.same.retain(|p| now - p.at < SAME_DAYS * DAY);
    store.topics.retain(|p| now - p.at < QUIET_DAYS * DAY);
    store.answers.retain(|a| now - a.at < QUIET_DAYS * DAY);
    store.quiet.retain(|q| q.until > now);
    let file = path(config);
    let body = serde_json::to_string_pretty(&store).unwrap_or_default();
    if let Err(e) = crate::profile_files::write_atomic_with(&file, body.as_bytes(), 0o600) {
        tracing::warn!(file = %file.display(), error = %e, "offers not saved");
    }
    out
}

// -- what it can do here, now ------------------------------------------------------------------------------------------

/// The calendar a `calendar` offer writes to.
#[derive(Debug, Clone, PartialEq)]
pub enum Calendar {
    /// The phone's, through its queue (calendar.rs).
    Phone,
    /// Google Calendar's primary calendar (google_calendar.rs).
    Google,
}

/// What an agent has here, now, for the kinds' needs (§3.3 « Il faut »).
#[derive(Debug, Clone, PartialEq)]
pub struct Can {
    /// The street routing on (`[maps] streets`).
    pub streets: bool,
    /// Its « Routines » on.
    pub routines: bool,
    pub calendar: Option<Calendar>,
    /// Hermes' `web` toolset not known off.
    pub web: bool,
    /// Its own browser on.
    pub browser: bool,
    /// Payments allowed to it with a card of its own (card_pay.rs `shopping`).
    pub cart: bool,
}

/// The kinds `can` allows, in `KINDS`' order.
pub fn kinds_of(can: &Can) -> Vec<&'static str> {
    KINDS
        .iter()
        .copied()
        .filter(|k| match *k {
            "route" => can.streets,
            "reminder" => can.routines,
            "calendar" => can.calendar.is_some(),
            "search" => can.web || can.browser,
            "draft" | "page" => true,
            "cart" => can.cart && can.browser,
            _ => false,
        })
        .collect()
}

/// What `agent` has here, now.
pub fn can(config: &Config, agent: &AgentConfig) -> Can {
    let name = agent.name.as_str();
    let calendar = if crate::calendar::writes(config, name) {
        Some(Calendar::Phone)
    } else if crate::google_calendar::offered(config, name) {
        Some(Calendar::Google)
    } else {
        None
    };
    let browser = crate::vault::browser_on(config, agent);
    let web = crate::tool_asks::item_of(config, agent, "web")
        .is_none_or(|item| crate::tool_asks::is_on(config, agent, &item) != Some(false));
    Can {
        streets: config.maps.streets != "off",
        routines: crate::routines::enabled(config, name),
        calendar,
        web,
        browser,
        // its card read only when it could shop at all (its browser on)
        cart: browser && matches!(crate::card_pay::shopping(config, agent), crate::souls::Shopping::Card(_)),
    }
}

/// The kinds `agent` may offer now: it proposes (« Initiative » not off, its own person's agent), it can do them,
/// and its person did not quiet them. Empty: no `offer_act`.
pub fn kinds_for(config: &Config, agent: &AgentConfig) -> Vec<&'static str> {
    if !crate::agent_proactive::proposes(config, &agent.name) {
        return Vec::new();
    }
    available(&load(config), &agent.name, &kinds_of(&can(config, agent)), py::time_time())
}

/// `offer_act` for `agent` (None: not listed).
pub fn tool_for(config: &Config, agent: &AgentConfig) -> Option<Value> {
    let kinds = kinds_for(config, agent);
    if kinds.is_empty() {
        return None;
    }
    let line = learning_line(&load(config), &agent.name, &kinds, py::time_time());
    Some(tool(&kinds, line.as_deref()))
}

// -- the learning (§3b.4, per kind) --------------------------------------------------------------------------------------

/// `kinds` without those quiet for `agent`.
pub fn available(store: &Store, agent: &str, kinds: &[&'static str], now: f64) -> Vec<&'static str> {
    kinds.iter().copied().filter(|k| !quiet(store, agent, k, now)).collect()
}

/// Whether `kind` is quiet for `agent` now.
pub fn quiet(store: &Store, agent: &str, kind: &str, now: f64) -> bool {
    store.quiet.iter().any(|q| q.agent == agent && q.kind == kind && now < q.until)
}

fn folded(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn print(parts: &[&str]) -> String {
    hex::encode(&Sha256::digest(parts.join("\u{1f}").as_bytes())[..12])
}

/// An offer's print (its kind and subject, case and spaces aside): never the same in `SAME_DAYS`.
pub fn same_print(agent: &str, kind: &str, about: &str) -> String {
    print(&[agent, kind, &folded(about)])
}

/// A subject's print: declined, not offered again in `QUIET_DAYS`.
pub fn topic_print(agent: &str, about: &str) -> String {
    print(&[agent, "topic", &folded(about)])
}

/// An offer made: its print kept `SAME_DAYS`.
pub fn remember(store: &mut Store, offer: &Offer, now: f64) {
    store.same.push(Print { agent: offer.agent.clone(), print: same_print(&offer.agent, &offer.kind, &offer.about), at: now });
}

/// The answers to `kind` of `agent` in the window: (their number, the share of yes, an expired one half a no).
fn rate_of(store: &Store, agent: &str, kind: &str, now: f64) -> (usize, f64) {
    let seen: Vec<&Answer> =
        store.answers.iter().filter(|a| a.agent == agent && a.kind == kind && now - a.at < QUIET_DAYS * DAY).collect();
    let yes = seen.iter().filter(|a| a.outcome == YES).count() as f64;
    let no = seen.iter().filter(|a| a.outcome == NO).count() as f64;
    let expired = seen.iter().filter(|a| a.outcome == EXPIRED).count() as f64;
    let weight = yes + no + expired / 2.0;
    (seen.len(), if weight > 0.0 { yes / weight } else { 0.0 })
}

/// An answer counted: a « Non merci » keeps its subject's print; two in a row for a kind (an expiry between breaks
/// nothing, a « Oui » starts again), or a low share of yes, quiets the kind `QUIET_DAYS` for this agent.
pub fn record(store: &mut Store, agent: &str, kind: &str, about: &str, outcome: &str, now: f64) {
    store.answers.push(Answer { agent: agent.into(), kind: kind.into(), at: now, outcome: outcome.into() });
    if outcome == NO {
        store.topics.push(Print { agent: agent.into(), print: topic_print(agent, about), at: now });
    }
    let series: Vec<&str> = store
        .answers
        .iter()
        .filter(|a| a.agent == agent && a.kind == kind && a.outcome != EXPIRED && now - a.at < QUIET_DAYS * DAY)
        .map(|a| a.outcome.as_str())
        .collect();
    let two_no = series.len() >= 2 && series[series.len() - 2..] == [NO, NO];
    let (n, rate) = rate_of(store, agent, kind, now);
    let low = n >= LOW_RATE_MIN && rate < LOW_RATE;
    if two_no || low {
        store.quiet.retain(|q| !(q.agent == agent && q.kind == kind));
        store.quiet.push(Quiet { agent: agent.into(), kind: kind.into(), until: now + QUIET_DAYS * DAY });
        tracing::info!(target: "bipbridge::offers", agent, kind, two_no, low, "a kind of offers quiet for 30 days");
    }
}

/// A kind's name in the learning line.
fn plural(kind: &str) -> &'static str {
    match kind {
        "route" => "routes",
        "reminder" => "reminders",
        "calendar" => "calendar events",
        "search" => "searches",
        "draft" => "drafts",
        "page" => "pages",
        _ => "shopping",
    }
}

/// « Your person often declines: pages, shopping. Often accepts: search, routes. » (§3b.4): of the kinds it may offer
/// now, each with 3 answers or more in the window: under a third yes, declines; two thirds or more, accepts. It
/// changes only when a threshold passes (the prompt cache holds). None: nothing to say.
pub fn learning_line(store: &Store, agent: &str, kinds: &[&'static str], now: f64) -> Option<String> {
    let (mut declines, mut accepts) = (Vec::new(), Vec::new());
    for kind in kinds.iter().filter(|k| !quiet(store, agent, k, now)) {
        let (n, rate) = rate_of(store, agent, kind, now);
        if n < 3 {
            continue;
        }
        if rate < 1.0 / 3.0 {
            declines.push(plural(kind));
        } else if rate >= 2.0 / 3.0 {
            accepts.push(plural(kind));
        }
    }
    match (declines.is_empty(), accepts.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format!("Your person often declines: {}.", declines.join(", "))),
        (true, false) => Some(format!("Your person often accepts: {}.", accepts.join(", "))),
        (false, false) => {
            Some(format!("Your person often declines: {}. Often accepts: {}.", declines.join(", "), accepts.join(", ")))
        }
    }
}

// -- the texts (contracts/offers/cases.json `texts`) -------------------------------------------------------------------

pub struct Texts {
    pub lang: &'static str,
    pub top: &'static str,
    pub buttons: [&'static str; 2],
    pub after_once: &'static str,
    pub after_deny: &'static str,
    pub expired: &'static str,
    pub notification: &'static str,
    pub summary: &'static str,
    pub first_card: &'static str,
    pub route_done: &'static str,
    pub route_failed: &'static str,
    pub calendar_failed: &'static str,
    pub not_started: &'static str,
    pub detail_route: &'static str,
    pub detail_reminder: &'static str,
    pub detail_calendar: &'static str,
    pub home: &'static str,
    pub phone_calendar: &'static str,
}

pub static TEXTS: [Texts; 4] = [
    Texts {
        lang: "fr",
        top: "{agent} propose · {about}",
        buttons: ["Oui", "Non merci"],
        after_once: "✓ Oui · {question}",
        after_deny: "Non merci · {question}",
        expired: "Passée · {question}",
        notification: "Propose : {question}",
        summary: "{agent} propose : {question}",
        first_card: "Tu règles ses propositions en bas de sa page.",
        route_done: "Trajet pour {place} · {n} min {mode}",
        route_failed: "Je n’ai pas pu calculer le trajet pour {place}.",
        calendar_failed: "Je n’ai pas pu ajouter « {title} » à ton agenda.",
        not_started: "Je n’ai pas pu m’y mettre pour l’instant. Redemande-le-moi quand tu veux.",
        detail_route: "Le trajet {mode} de {from} à {to}",
        detail_reminder: "Un rappel {when} : « {say} »",
        detail_calendar: "Dans {calendar} : « {title} », {when}",
        home: "chez toi",
        phone_calendar: "l’agenda de ton téléphone",
    },
    Texts {
        lang: "en",
        top: "{agent} suggests · {about}",
        buttons: ["Yes", "No thanks"],
        after_once: "✓ Yes · {question}",
        after_deny: "No thanks · {question}",
        expired: "Expired · {question}",
        notification: "Suggests: {question}",
        summary: "{agent} suggests: {question}",
        first_card: "You can set its suggestions at the bottom of its page.",
        route_done: "Route to {place} · {n} min {mode}",
        route_failed: "I couldn’t work out the route to {place}.",
        calendar_failed: "I couldn’t add “{title}” to your calendar.",
        not_started: "I couldn’t get to it just now. Ask me again whenever you like.",
        detail_route: "The route {mode} from {from} to {to}",
        detail_reminder: "A reminder {when}: “{say}”",
        detail_calendar: "In {calendar}: “{title}”, {when}",
        home: "home",
        phone_calendar: "your phone’s calendar",
    },
    Texts {
        lang: "de",
        top: "{agent} schlägt vor · {about}",
        buttons: ["Ja", "Nein danke"],
        after_once: "✓ Ja · {question}",
        after_deny: "Nein danke · {question}",
        expired: "Abgelaufen · {question}",
        notification: "Schlägt vor: {question}",
        summary: "{agent} schlägt vor: {question}",
        first_card: "Seine Vorschläge stellst du unten auf seiner Seite ein.",
        route_done: "Weg nach {place} · {n} Min. {mode}",
        route_failed: "Ich konnte den Weg nach {place} nicht berechnen.",
        calendar_failed: "Ich konnte „{title}“ nicht in deinen Kalender eintragen.",
        not_started: "Ich konnte mich gerade nicht darum kümmern. Frag mich einfach noch einmal.",
        detail_route: "Der Weg {mode} von {from} nach {to}",
        detail_reminder: "Eine Erinnerung {when}: „{say}“",
        detail_calendar: "In {calendar}: „{title}“, {when}",
        home: "zu Hause",
        phone_calendar: "dem Kalender deines Telefons",
    },
    Texts {
        lang: "es",
        top: "{agent} propone · {about}",
        buttons: ["Sí", "No, gracias"],
        after_once: "✓ Sí · {question}",
        after_deny: "No, gracias · {question}",
        expired: "Caducada · {question}",
        notification: "Propone: {question}",
        summary: "{agent} propone: {question}",
        first_card: "Puedes ajustar sus propuestas al final de su página.",
        route_done: "Trayecto hasta {place} · {n} min {mode}",
        route_failed: "No pude calcular el trayecto hasta {place}.",
        calendar_failed: "No pude añadir «{title}» a tu agenda.",
        not_started: "No he podido ponerme con ello ahora. Pídemelo de nuevo cuando quieras.",
        detail_route: "El trayecto {mode} desde {from} hasta {to}",
        detail_reminder: "Un recordatorio {when}: «{say}»",
        detail_calendar: "En {calendar}: «{title}», {when}",
        home: "tu casa",
        phone_calendar: "la agenda de tu teléfono",
    },
];

/// The texts in `lang` (another language: English).
pub fn texts(lang: &str) -> &'static Texts {
    TEXTS.iter().find(|t| t.lang == lang).unwrap_or(&TEXTS[1])
}

/// The card's sentence for an app 1.0 (its whole card): « Vie propose : Je regarde les hôtels ? »
pub fn summary(lang: &str, agent: &str, question: &str) -> String {
    fill(texts(lang).summary, &[("agent", agent), ("question", question)])
}

pub fn notification(lang: &str, question: &str) -> String {
    fill(texts(lang).notification, &[("question", question)])
}

/// The card's top line in an app 1.1: « Vie propose · Lisbonne ».
pub fn top(lang: &str, agent: &str, about: &str) -> String {
    fill(texts(lang).top, &[("agent", agent), ("about", about)])
}

fn mode_words(lang: &str, mode: &str) -> &'static str {
    let t = super::texts(lang);
    t.modes[super::MODES.iter().position(|m| *m == mode).unwrap_or(0)]
}

fn param<'a>(offer: &'a Offer, key: &str) -> &'a str {
    offer.params.as_ref().and_then(|p| p.get(key)).and_then(Value::as_str).unwrap_or("")
}

fn param_at(offer: &Offer) -> f64 {
    offer.params.as_ref().and_then(|p| p.get("at")).and_then(Value::as_f64).unwrap_or(0.0)
}

/// What the card says under « i »: the prompt, whole (the agent's turn), or what the bridge will do, exactly.
pub fn detail(lang: &str, offer: &Offer, person: Tz) -> String {
    let t = texts(lang);
    match offer.kind.as_str() {
        "route" => {
            let from = match param(offer, "from") {
                "" => t.home,
                from => from,
            };
            fill(t.detail_route, &[("mode", mode_words(lang, param(offer, "mode"))), ("from", from), ("to", param(offer, "to"))])
        }
        "reminder" => {
            let when = super::when_text(lang, param_at(offer), person, person, "");
            fill(t.detail_reminder, &[("when", &when), ("say", param(offer, "say"))])
        }
        "calendar" => {
            let event = offer.params.as_ref().and_then(|p| p.get("event")).cloned().unwrap_or(Value::Null);
            let field = |k: &str| event.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let start = super::when_text(lang, param_at(offer), person, person, "");
            let when = if event.get("all_day").and_then(Value::as_bool).unwrap_or(false) {
                start.split(" · ").next().unwrap_or("").to_string()
            } else {
                let (s, e) = (field("start"), field("end"));
                match e.get(11..16) {
                    Some(clock) if e.get(..10) == s.get(..10) => format!("{start}–{clock}"),
                    _ => start,
                }
            };
            let when = match field("location") {
                place if place.is_empty() => when,
                place => format!("{when} · {place}"),
            };
            let calendar = if param(offer, "calendar") == "google" { crate::google_calendar::LABEL } else { t.phone_calendar };
            fill(t.detail_calendar, &[("calendar", calendar), ("title", &field("title")), ("when", &when)])
        }
        _ => offer.prompt.clone(),
    }
}

// -- the tool -----------------------------------------------------------------------------------------------------------

/// What each kind does, in its line of the tool's description.
fn kind_line(kind: &str) -> &'static str {
    match kind {
        "route" => "the way to a place by the streets, worked out by the app and shown on a map: params {to, from \
(default: your person's home), mode: walking | driving | transit}",
        "reminder" => "one reminder at a time, said as you write it: params {at (YYYY-MM-DDTHH:MM, your person's time), \
say}",
        "calendar" => "one event added to their calendar exactly as the card shows it: params {title, start, end or \
duration_minutes, location}",
        "search" => "you look things up and report back, nothing booked: prompt",
        "draft" => "you write drafts, never sent (in their mailbox's Drafts, else in a ```draft block): prompt",
        "page" => "you make a page for them (a list, a small tracker): prompt",
        _ => "you fill a basket on a shop's site up to its payment page, never paying (their card asks them then): \
prompt and site (the shop's host name)",
    }
}

/// What « you can't … » says of a kind.
fn can_not_words(kind: &str) -> &'static str {
    match kind {
        "route" => "work out a route by the streets",
        "reminder" => "set reminders (your routines are off)",
        "calendar" => "write in your person's calendar",
        "search" => "search the web",
        "draft" => "write drafts",
        "page" => "make pages",
        _ => "fill a shopping basket (no payments, no card or no browser for you)",
    }
}

/// `offer_act` listing only `kinds`, with what its person often declines or accepts (`learning`).
pub fn tool(kinds: &[&str], learning: Option<&str>) -> Value {
    let lines: Vec<String> = kinds.iter().map(|k| format!("- {k}: {}", kind_line(k))).collect();
    let mut description = format!(
        "Offer your person to do something for them, on ONE card they answer « Oui » or « Non merci »: when they speak \
of something to come in their own life and a clear next step would help, or to end a routine's, a watcher's or a \
trigger's turn when what you found calls for an action, rather than with a question. label: the question, short, in \
their language (« Je regarde les hôtels près de Baixa ? »); about: what it is about, a few words (« Lisbonne »). \
Nothing is done before their yes; then the app does it itself, or you get your prompt to do, exactly as written: drafts \
and searches only, never book, pay or send (each of those keeps its own card later). Never for the past, a « maybe », \
someone else's life, or what they just refused; at most 2 offers per reply; don't ask them again in words. What you can \
offer here, now:\n{}",
        lines.join("\n")
    );
    if let Some(line) = learning {
        description.push('\n');
        description.push_str(line);
    }
    json!({
        "name": TOOL_NAME,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": kinds},
                "label": {"type": "string", "maxLength": LABEL_MAX, "description": "the question on the card"},
                "about": {"type": "string", "maxLength": ABOUT_MAX, "description": "what it is about (« Mariage samedi »)"},
                "prompt": {"type": "string", "maxLength": PROMPT_MAX, "description": "what you'll do at their yes, \
exactly, plain text (shown to them under « i »): for the kinds you do"},
                "params": {"type": "object", "description": "for the kinds the app does itself (shown to them \
under « i »)", "properties": {
                    "to": {"type": "string"}, "from": {"type": "string"},
                    "mode": {"type": "string", "enum": super::MODES},
                    "at": {"type": "string"}, "say": {"type": "string"},
                    "title": {"type": "string"}, "start": {"type": "string"}, "end": {"type": "string"},
                    "duration_minutes": {"type": "integer"}, "location": {"type": "string"}}},
                "site": {"type": "string", "description": "a basket's shop (« www.decathlon.fr »)"},
            },
            "required": ["kind", "label", "about"],
        },
    })
}

/// The schema of an offer in plan_propose (`offers`, a moment's `offer`): one of `kinds`.
pub fn plan_schema(kinds: &[&str], label_max: usize) -> Value {
    json!({"type": "object", "properties": {
        "kind": {"type": "string", "enum": kinds},
        "label": {"type": "string", "maxLength": label_max},
        "prompt": {"type": "string", "maxLength": PROMPT_MAX},
        "params": {"type": "object"},
        "site": {"type": "string"}},
        "required": ["kind", "label"]})
}

// -- reading an offer ----------------------------------------------------------------------------------------------------

fn not_done(why: impl std::fmt::Display) -> String {
    format!("Not done: {why}")
}

/// `offer_act`'s arguments read and checked against the kinds `agent` can do (`kinds`): Err, the refusal.
pub fn draft(arguments: &Dict, agent: &str, kinds: &[&str], person: Tz, now: f64) -> Result<Offer, String> {
    let spec = Value::Object(arguments.clone());
    let kind = text_arg(arguments.get("kind")).to_lowercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err(not_done(format!("kind: one of {}", kinds.join(", "))));
    }
    if !kinds.contains(&kind.as_str()) {
        return Err(not_done(format!("you can't {} here; don't offer it.", can_not_words(&kind))));
    }
    let about = plain(arguments.get("about"), "about", ABOUT_MAX).map_err(not_done)?;
    spec_of(&spec, agent, &about, kinds, person, now, LABEL_MAX)
}

/// An offer from its spec (`offer_act`'s arguments, a plan's offer, a moment's): `about` given, `label_max` its
/// question's length. Err, the refusal (« Not done: … »).
pub fn spec_of(
    spec: &Value,
    agent: &str,
    about: &str,
    kinds: &[&str],
    person: Tz,
    now: f64,
    label_max: usize,
) -> Result<Offer, String> {
    let kind = text_arg(spec.get("kind")).to_lowercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err(not_done(format!("kind: one of {}", kinds.join(", "))));
    }
    if !kinds.contains(&kind.as_str()) {
        return Err(not_done(format!("you can't {} here; don't offer it.", can_not_words(&kind))));
    }
    let label = plain(spec.get("label"), "label", label_max).map_err(not_done)?;
    let given_params = spec.get("params").filter(|p| !p.is_null());
    let mut until: Option<f64> = None;
    let (prompt, params, site) = if BY_BRIDGE.contains(&kind.as_str()) {
        if !text_arg(spec.get("prompt")).is_empty() {
            return Err(not_done(format!("prompt: not for a {kind} offer, the app does it itself: give params")));
        }
        let Some(p) = given_params.filter(|p| p.is_object()) else {
            return Err(not_done(format!("params: a {kind} offer needs them ({})", kind_line(&kind))));
        };
        let params = match kind.as_str() {
            "route" => {
                let to = plain(p.get("to"), "params.to", PLACE_MAX).map_err(not_done)?;
                let mode = match text_arg(p.get("mode")).to_lowercase() {
                    m if m.is_empty() => "walking".to_string(),
                    m => m,
                };
                if !super::MODES.contains(&mode.as_str()) {
                    return Err(not_done("params.mode: walking, driving or transit"));
                }
                let mut out = json!({"to": to, "mode": mode});
                if let Some(from) = p.get("from").filter(|v| !v.is_null()) {
                    out["from"] = json!(plain(Some(from), "params.from", PLACE_MAX).map_err(not_done)?);
                }
                out
            }
            "reminder" => {
                let at = super::parse_at(&text_arg(p.get("at")), person)
                    .ok_or_else(|| not_done("params.at: « YYYY-MM-DDTHH:MM » on your person's clock"))?;
                if at <= now {
                    return Err(not_done("params.at: its time is not to come"));
                }
                let say = plain(p.get("say"), "params.say", SAY_MAX).map_err(not_done)?;
                until = Some(at);
                json!({"at": at, "say": say})
            }
            _ => {
                let title = plain(p.get("title"), "params.title", TITLE_MAX).map_err(not_done)?;
                let mut fields = p.as_object().cloned().unwrap_or_default();
                fields.insert("title".into(), json!(title));
                fields.remove("calendar");
                if let Some(place) = fields.get("location").filter(|v| !v.is_null()).cloned() {
                    let place = plain(Some(&place), "params.location", PLACE_MAX).map_err(not_done)?;
                    fields.insert("location".into(), json!(place));
                }
                let event = crate::calendar::new_event(&fields).map_err(|why| not_done(format!("params: {why}")))?;
                let start = event.get("start").and_then(Value::as_str).unwrap_or("");
                let at = super::parse_at(start, person)
                    .or_else(|| {
                        let day = chrono::NaiveDate::parse_from_str(start.get(..10)?, "%Y-%m-%d").ok()?;
                        person.from_local_datetime(&day.and_hms_opt(0, 0, 0)?).earliest().map(|d| d.timestamp() as f64)
                    })
                    .ok_or_else(|| not_done("params.start: an ISO date-time or a date"))?;
                let all_day = event.get("all_day").and_then(Value::as_bool).unwrap_or(false);
                if (all_day && at + DAY <= now) || (!all_day && at <= now) {
                    return Err(not_done("params.start: the event is not to come"));
                }
                until = Some(if all_day { at + DAY } else { at });
                json!({"event": event, "at": at})
            }
        };
        (String::new(), Some(params), None)
    } else {
        if given_params.is_some() {
            return Err(not_done(format!("params: only for route, reminder and calendar offers; a {kind} offer gives its prompt")));
        }
        let prompt = plain(spec.get("prompt"), "prompt", PROMPT_MAX).map_err(not_done)?;
        let site = if kind == "cart" {
            let site = text_arg(spec.get("site")).to_lowercase();
            let site = site.trim_end_matches('.').to_string();
            if !domain_ok(&site) {
                return Err(not_done("site: the shop's host name (www.decathlon.fr)"));
            }
            Some(site)
        } else {
            None
        };
        (prompt, None, site)
    };
    let expires = until.map_or(now + EXPIRES_SECONDS, |u| u.min(now + EXPIRES_SECONDS));
    Ok(Offer {
        id: py::uuid4_hex()[..12].to_string(),
        agent: agent.to_string(),
        kind,
        label,
        about: about.to_string(),
        prompt,
        params,
        site,
        plan: None,
        pill: false,
        made: now,
        expires,
        asked: false,
        first: false,
    })
}

/// Done by the bridge itself at « Oui ».
pub fn by_bridge(offer: &Offer) -> bool {
    BY_BRIDGE.contains(&offer.kind.as_str())
}

// -- the caps -----------------------------------------------------------------------------------------------------------

fn day_of(person: Tz, at: f64) -> Option<chrono::NaiveDate> {
    person.timestamp_opt(at as i64, 0).single().map(|d| d.date_naive())
}

/// Offer cards made today that nobody asked for (pills aside): `agent`'s, or the place's (None).
pub fn unasked_today(store: &Store, agent: Option<&str>, person: Tz, now: f64) -> usize {
    let today = day_of(person, now);
    store
        .made
        .iter()
        .filter(|m| !m.asked && !m.pill && agent.is_none_or(|a| m.agent == a) && day_of(person, m.at) == today)
        .count()
}

fn plans_unasked_today(plans: &super::Store, agent: Option<&str>, person: Tz, now: f64) -> usize {
    let today = day_of(person, now);
    plans.made.iter().filter(|m| !m.asked && agent.is_none_or(|a| m.agent == a) && day_of(person, m.at) == today).count()
}

/// Why a card nobody asked for (a plan's or an offer's) may not be made now for `agent`, plans and offers together.
pub fn unasked_refusal(store: &Store, plans: &super::Store, agent: &str, person: Tz, now: f64) -> Option<String> {
    let waiting = store.offers.iter().any(|o| o.agent == agent && !o.pill && o.expires > now)
        || plans.plans.iter().any(|p| p.agent == agent && p.state == super::WAITING);
    if waiting {
        return Some(
            "Not done: one of your cards already waits for your person's answer. Don't make another one today.".into(),
        );
    }
    let mine = unasked_today(store, Some(agent), person, now) + plans_unasked_today(plans, Some(agent), person, now);
    if mine >= super::UNASKED_PER_AGENT {
        return Some(format!(
            "Not done: you already made {} cards today that your person didn't ask for. Not more before tomorrow.",
            super::UNASKED_PER_AGENT
        ));
    }
    if unasked_today(store, None, person, now) + plans_unasked_today(plans, None, person, now) >= super::UNASKED_PER_PLACE {
        return Some(format!(
            "Not done: your person's agents made {} cards today they didn't ask for. Not more before tomorrow.",
            super::UNASKED_PER_PLACE
        ));
    }
    None
}

/// Why `offer` may not be made now (None: it may), in the agent's words; `run`: the person's turn it is made in.
pub fn refused(store: &Store, plans: &super::Store, offer: &Offer, run: Option<&str>, person: Tz, now: f64) -> Option<String> {
    let agent = offer.agent.as_str();
    let same = same_print(agent, &offer.kind, &offer.about);
    if store.same.iter().any(|p| p.agent == agent && p.print == same && now - p.at < SAME_DAYS * DAY) {
        return Some(format!(
            "Not done: you offered this ({}, « {} ») in the last {} days. Don't offer it again.",
            offer.kind, offer.about, SAME_DAYS as u32
        ));
    }
    let topic = topic_print(agent, &offer.about);
    if store.topics.iter().any(|p| p.agent == agent && p.print == topic && now - p.at < QUIET_DAYS * DAY) {
        return Some(format!(
            "Not done: your person said no to an offer about « {} » lately. Don't offer it again for now.",
            offer.about
        ));
    }
    if quiet(store, agent, &offer.kind, now) {
        return Some(format!(
            "Not done: your person said no thanks to your {} lately: don't offer them for now (it is not « Initiative » \
             off: just that kind, for a while).",
            plural(&offer.kind)
        ));
    }
    if offer.asked {
        let in_reply = store.made.iter().filter(|m| m.agent == agent && m.asked && m.run.as_deref() == run).count();
        if run.is_some() && in_reply >= PER_REPLY {
            return Some(format!(
                "Not done: two offers per reply at most; you made {PER_REPLY} in this reply already."
            ));
        }
        return None;
    }
    unasked_refusal(store, plans, agent, person, now)
}

// -- the card -----------------------------------------------------------------------------------------------------------

/// The offer's card (`/v1/approvals`' shape).
pub fn card_item(config: &Config, offer: &Offer) -> Dict {
    let lang = lang_of(config, &offer.agent);
    let shown = shown_agent(config, &offer.agent);
    let sentence = summary(lang, &shown, &offer.label);
    let mut under_i = detail(lang, offer, person_zone(config));
    if offer.first {
        under_i.push('\n');
        under_i.push_str(texts(lang).first_card);
    }
    let item = json!({
        "agent": offer.agent, "run_id": format!("{RUN_PREFIX}{}", offer.id), "request_id": offer.id,
        "command": format!("{TOOL_NAME} · {}", offer.label), "description": sentence, "choices": ["once", "deny"],
        "created_at": offer.made, "session_id": null, "bip_summary": sentence, "bip_detail": under_i,
        "bip_offer": {"id": offer.id, "kind": offer.kind, "about": offer.about, "label": offer.label, "agent": shown,
                      "plan": offer.plan, "pill": offer.pill, "mode": "basic", "expires_at": offer.expires},
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The offer cards waiting for the person (not expired), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let now = py::time_time();
    let mut offers: Vec<Offer> = load(config)
        .offers
        .into_iter()
        .filter(|o| o.expires > now && agent.is_none_or(|a| o.agent == a))
        .collect();
    offers.sort_by(|a, b| a.made.total_cmp(&b.made));
    offers.iter().map(|o| card_item(config, o)).collect()
}

/// The ids of a plan's offers shown on its card, still waiting (its `bip_plan.offers`).
pub fn pills_of(config: &Config, plan: &str) -> Vec<String> {
    let now = py::time_time();
    load(config)
        .offers
        .into_iter()
        .filter(|o| o.pill && o.plan.as_deref() == Some(plan) && o.expires > now)
        .map(|o| o.id)
        .collect()
}

/// The validation's kind of an offer's card (approval_level.rs): a calendar offer is that write's own card.
pub fn kind_of(item: &Dict) -> &'static str {
    let kind = item.get("bip_offer").and_then(|o| o.get("kind")).and_then(Value::as_str);
    if kind == Some("calendar") {
        "calendar_write"
    } else {
        KIND
    }
}

// -- the call -----------------------------------------------------------------------------------------------------------

fn refusal(text: &str) -> Called {
    Called { result: tool_text(text, true), card: None, offers: Vec::new() }
}

/// A cart's shop allowed by one of `agent`'s payment cards (a card with no sites allows any: cards.rs).
fn site_ok(config: &Config, agent: &str, site: &str) -> bool {
    let vault = crate::vault::Vault::new(&py::join(&config.data_dir(), "vault"));
    vault.cards_of(agent).iter().any(|(_, card)| {
        let sites: Vec<String> = card
            .get("sites")
            .and_then(Value::as_array)
            .map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        crate::cards::site_allowed(&sites, site)
    })
}

/// What the bridge checks beyond the arguments, on the place: a route's start (the person's home when no `from`:
/// read here, sent nowhere before « Oui »), a calendar offer's calendar, a cart's shop.
fn prepare(state: &AppState, config: &Config, agent: &AgentConfig, can: &Can, offer: &mut Offer) -> Result<(), String> {
    match offer.kind.as_str() {
        "route" if param(offer, "from").is_empty() && super::home_address(state, config, &agent.name).is_none() => {
            Err(not_done(
                "params.from: none of your person's contact details you may read is labelled as their home \
                 (« Domicile », « Perso », « Home »…): give params.from, or don't offer the route",
            ))
        }
        "calendar" => {
            if let Some(p) = offer.params.as_mut() {
                p["calendar"] = json!(if can.calendar == Some(Calendar::Google) { "google" } else { "phone" });
            }
            Ok(())
        }
        "cart" => match offer.site.as_deref() {
            Some(site) if site_ok(config, &agent.name, site) => Ok(()),
            Some(site) => Err(not_done(format!(
                "site: none of the payment cards you may use allows {site}: don't offer a basket there"
            ))),
            None => Err(not_done("site: the shop's host name")),
        },
        _ => Ok(()),
    }
}

/// The agent's first plan or offer card (plans.rs `hinted`): its « i » says where its suggestions are set.
fn first_card(config: &Config, agent: &str) -> bool {
    super::change(config, |p| {
        let first = !p.hinted.iter().any(|a| a == agent);
        if first {
            p.hinted.push(agent.to_string());
        }
        first
    })
}

/// `offer_act` by `agent`; `asked`: in a turn of its person's own conversation, `run` that turn.
pub async fn call(state: &AppState, agent: &AgentConfig, arguments: &Dict, asked: bool, run: Option<&str>) -> Called {
    let config = state.config();
    if !crate::agent_proactive::proposes(&config, &agent.name) {
        return refusal(
            "Not done: your person turned your suggestions off (« Initiative », at the bottom of your page), or other \
             people talk to you: offer nothing on your own. Do what they ask.",
        );
    }
    let now = py::time_time();
    let person = person_zone(&config);
    let can = can(&config, agent);
    let mut offer = match draft(arguments, &agent.name, &kinds_of(&can), person, now) {
        Ok(o) => o,
        Err(why) => return refusal(&format!("{why} Nothing was made.")),
    };
    if let Err(why) = prepare(state, &config, agent, &can, &mut offer) {
        return refusal(&format!("{why}. Nothing was made."));
    }
    offer.asked = asked;
    let plans = super::load(&config);
    let made = change(&config, |s| {
        if let Some(why) = refused(s, &plans, &offer, run, person, now) {
            return Err(why);
        }
        remember(s, &offer, now);
        s.made.push(Made { agent: offer.agent.clone(), at: now, asked, run: run.map(str::to_string), pill: false });
        s.offers.push(offer.clone());
        Ok(())
    });
    if let Err(why) = made {
        return refusal(&why);
    }
    if first_card(&config, &agent.name) {
        change(&config, |s| s.offers.iter_mut().filter(|o| o.id == offer.id).for_each(|o| o.first = true));
    }
    tracing::info!(target: "bipbridge::offers", agent = %agent.name, offer = %offer.id, kind = %offer.kind, asked,
                   "offer card made");
    let text = format!(
        "Offer card made: « {} ». It waits for your person's « Oui » or « Non merci »: nothing is done before. Don't \
         ask them again in words.",
        offer.label
    );
    let lang = lang_of(&config, &agent.name);
    Called {
        result: tool_text(&text, false),
        card: Some((notification(lang, &offer.label), offer.id.clone())),
        offers: Vec::new(),
    }
}

/// A plan's offers shown on its card (`plan_propose.offers`), read and checked (`kinds`: what `agent` may offer
/// now): Err, the plan's refusal.
pub fn plan_offers(
    specs: &[Value],
    agent: &str,
    title: &str,
    kinds: &[&str],
    person: Tz,
    now: f64,
) -> Result<Vec<Offer>, String> {
    if specs.len() > PER_REPLY {
        return Err(not_done(format!("offers: {PER_REPLY} at most")));
    }
    specs
        .iter()
        .map(|spec| {
            spec_of(spec, agent, title, kinds, person, now, PILL_MAX).map_err(|why| {
                format!("Not done: offers: {}", why.strip_prefix("Not done: ").unwrap_or(&why))
            })
        })
        .collect()
}

/// A plan's offers made with it (pills on its card, `plan` its id): the bridge's checks, the caps of the reply
/// (`run`, when asked), nothing pushed. Err: the plan's refusal (nothing made).
pub fn make_pills(
    state: &AppState,
    agent: &AgentConfig,
    pills: &mut [Offer],
    asked: bool,
    run: Option<&str>,
    person: Tz,
    now: f64,
) -> Result<(), String> {
    let config = state.config();
    let can = can(&config, agent);
    for p in pills.iter_mut() {
        prepare(state, &config, agent, &can, p).map_err(|why| format!("Not done: offers: {}", why.trim_start_matches("Not done: ")))?;
        p.asked = true;
        p.pill = true;
    }
    let plans = super::Store::default();
    let mut seen = load(&config);
    for p in pills.iter() {
        let run = if asked { run } else { None };
        if let Some(why) = refused(&seen, &plans, p, run, person, now) {
            return Err(format!("Not done: offers: {}", why.trim_start_matches("Not done: ")));
        }
        seen.made.push(Made { agent: p.agent.clone(), at: now, asked: true, run: run.map(str::to_string), pill: true });
    }
    Ok(())
}

/// The pills kept, with their plan: made now.
pub fn keep_pills(config: &Config, pills: &[Offer], plan: &str, asked: bool, run: Option<&str>, now: f64) -> Vec<String> {
    change(config, |s| {
        pills
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p.plan = Some(plan.to_string());
                remember(s, &p, now);
                s.made.push(Made {
                    agent: p.agent.clone(),
                    at: now,
                    asked: true,
                    run: if asked { run.map(str::to_string) } else { None },
                    pill: true,
                });
                s.offers.push(p.clone());
                p.id
            })
            .collect()
    })
}

/// A plan's « Non merci »: its offers shown on its card go with it (counted for nothing).
pub fn withdraw_plan(config: &Config, plan: &str) -> Vec<Offer> {
    if !load(config).offers.iter().any(|o| o.pill && o.plan.as_deref() == Some(plan)) {
        return Vec::new();
    }
    change(config, |s| {
        let (gone, kept): (Vec<Offer>, Vec<Offer>) =
            std::mem::take(&mut s.offers).into_iter().partition(|o| o.pill && o.plan.as_deref() == Some(plan));
        s.offers = kept;
        gone
    })
}

// -- the answer ---------------------------------------------------------------------------------------------------------

/// The answer kept: the offer taken out (None: no such offer of `agent` waiting), counted for its kind.
pub fn answer(config: &Config, agent: &str, id: &str, yes: bool, now: f64) -> Option<Offer> {
    change(config, |s| {
        let at = s.offers.iter().position(|o| o.agent == agent && o.id == id && o.expires > now)?;
        let offer = s.offers.remove(at);
        record(s, agent, &offer.kind, &offer.about, if yes { YES } else { NO }, now);
        Some(offer)
    })
}

/// The offers past their time taken out, counted as expired.
pub fn expire(config: &Config, now: f64) -> Vec<Offer> {
    if !load(config).offers.iter().any(|o| o.expires <= now) {
        return Vec::new();
    }
    change(config, |s| {
        let (gone, kept): (Vec<Offer>, Vec<Offer>) = std::mem::take(&mut s.offers).into_iter().partition(|o| o.expires <= now);
        s.offers = kept;
        for o in &gone {
            record(s, &o.agent, &o.kind, &o.about, EXPIRED, now);
        }
        gone
    })
}

/// What the turn at « Oui » reads, hidden (design §3.3).
pub fn yes_note(offer: &Offer) -> String {
    format!(
        "<!--bip:offer\nNot from the person's words: they answered « Oui » in the app to your offer « {} ».\nYour person \
         said yes to your offer: « {} ». Do exactly that now. Drafts only: never book, pay or send; anything like that \
         goes through its own card.\n-->",
        offer.label, offer.prompt
    )
}

/// An offer's card answered (`/v1/approve`): « Oui » (`once`; `session` and `always` mean the same: it offers no
/// « Toujours ») or « Non merci »; 422 for any other word, 404 for no such card waiting.
pub async fn approve(state: &AppState, agent: &AgentConfig, id: &str, choice: &str) -> ApiResult<axum::response::Response> {
    let choice = choice.trim().to_lowercase();
    let yes = matches!(choice.as_str(), "once" | "session" | "always");
    if !yes && !crate::routines::refuses(&choice) {
        return Err(HttpError::new(422, "choice: once or deny"));
    }
    let config = state.config();
    let place = crate::card_place::of(&config, &format!("{RUN_PREFIX}{id}")).unwrap_or_default();
    let (c, name, oid) = (config.clone(), agent.name.clone(), id.to_string());
    let taken = tokio::task::spawn_blocking(move || answer(&c, &name, &oid, yes, py::time_time()))
        .await
        .map_err(anyhow::Error::from)?;
    let Some(offer) = taken else {
        return Err(HttpError::new(404, "no such request (answered already?)"));
    };
    tracing::info!(target: "bipbridge::offers", agent = %agent.name, offer = %offer.id, kind = %offer.kind, yes,
                   "offer answered");
    if !yes {
        return Ok(web::ok(json!({"ok": true, "choice": "deny"})));
    }
    let mut body = json!({"ok": true, "choice": "once", "kind": offer.kind,
                          "by": if by_bridge(&offer) { "bridge" } else { "agent" }});
    let kind = offer.kind.clone();
    match kind.as_str() {
        "reminder" => {
            // where the card showed for the person: its line lands there (never a hidden conversation)
            let origin = if offer.asked { place.report.clone().or(place.session.clone()) } else { None };
            body["routine"] = json!(reminder(&config, &offer, origin.as_deref()));
        }
        "route" | "calendar" => {
            let (state, agent) = (state.clone(), agent.clone());
            tokio::spawn(async move {
                if offer.kind == "route" {
                    route(&state, &agent, &offer, &place).await;
                } else {
                    calendar(&state, &agent, &offer, &place).await;
                }
            });
        }
        _ => {
            let (state, agent) = (state.clone(), agent.clone());
            tokio::spawn(async move { turn(&state, &agent, &offer, &place).await });
        }
    }
    Ok(web::ok(body))
}

/// A reminder's « Oui »: a one-time routine whose words are the offer's (said as written, no model). Its id.
fn reminder(config: &Config, offer: &Offer, origin: Option<&str>) -> String {
    let name: String = offer.about.chars().take(crate::routines::NAME_MAX).collect();
    let made = crate::routines::create_said(
        config,
        &offer.agent,
        &name,
        crate::routines::Schedule::Once { at: param_at(offer) },
        param(offer, "say"),
        false,
        offer.plan.as_deref().unwrap_or(""),
        origin,
        Some(1),
        py::time_time(),
    );
    if offer.plan.is_none() {
        let _ = crate::routines::update(config, &made.id, |r| {
            r.plan = None;
            Ok(())
        });
    }
    tracing::info!(target: "bipbridge::offers", agent = %offer.agent, offer = %offer.id, routine = %made.id, "reminder set");
    made.id
}

/// One Boîte line where the card showed for the person (the main conversation for a card nobody asked for), pushed:
/// no model. `files`: the work's (a map).
async fn tell(state: &AppState, agent: &AgentConfig, place: &crate::card_place::Place, offer: &Offer, text: String, files: Vec<String>) {
    let pipeline = Pipeline::of(state);
    let shown = if offer.asked { place.report.clone().or(place.session.clone()) } else { None };
    let session = match shown {
        Some(s) => crate::card_place::report_of(state, agent, &s).await.unwrap_or(s),
        None => pipeline.tip(agent, "main").await,
    };
    let delivery = Delivery {
        agent: agent.clone(),
        session,
        key: format!("offer:{}:{}", offer.id, py::time_time() as i64),
        title: offer.about.clone(),
        text,
        as_written: false,
        run_id: None,
        notify: true,
        files,
    };
    pipeline.deliver(&delivery).await;
}

/// A route's « Oui »: both places found (Nominatim), the time by the streets (OSRM, coordinates only), one line with
/// its map (maps.rs, the same card as `map_image`); not found: one line. No model.
async fn route(state: &AppState, agent: &AgentConfig, offer: &Offer, place: &crate::card_place::Place) {
    let config = state.config();
    let lang = lang_of(&config, &agent.name);
    let t = texts(lang);
    let (to, mode) = (param(offer, "to").to_string(), param(offer, "mode").to_string());
    let from = Some(param(offer, "from").to_string()).filter(|f| !f.is_empty());
    let home = if from.is_none() { super::home_address(state, &config, &agent.name) } else { None };
    let leg = super::Leg { place: to.clone(), from: from.clone(), mode: mode.clone(), arrive: 0.0 };
    let minutes = match super::leg_minutes(&config, home.as_deref(), &leg).await {
        Ok(m) => m,
        Err(why) => {
            tracing::info!(target: "bipbridge::offers", offer = %offer.id, why = %why, "route not timed");
            tell(state, agent, place, offer, fill(t.route_failed, &[("place", &to)]), Vec::new()).await;
            return;
        }
    };
    let words = fill(t.route_done, &[("place", &to), ("n", &minutes.to_string()), ("mode", mode_words(lang, &mode))]);
    // its map: the way between both places, along the streets
    let start = from.or(home).unwrap_or_default();
    let travel = if mode == "walking" { "walking" } else { "driving" };
    let arguments = json!({"markers": [{"place": start}, {"place": to}], "line": true, "travel": travel, "title": to});
    let drawn = crate::maps::call(&config, &agent.name, arguments.as_object().unwrap_or(&Dict::new())).await;
    let shown = drawn.get("content").and_then(|c| c.get(0)).and_then(|c| c.get("text")).and_then(Value::as_str).unwrap_or("");
    let files: Vec<String> = match drawn.get("isError").and_then(Value::as_bool) {
        Some(false) => shown.lines().find_map(|l| l.strip_prefix("MEDIA:")).map(|p| vec![p.trim().to_string()]).unwrap_or_default(),
        _ => Vec::new(),
    };
    tell(state, agent, place, offer, words, files).await;
}

/// A calendar offer's « Oui »: the event on its card written, no other card (this one was the write's): the phone's
/// calendar through its queue (the phone woken), or Google Calendar's primary one. Not written: one line.
async fn calendar(state: &AppState, agent: &AgentConfig, offer: &Offer, place: &crate::card_place::Place) {
    let config = state.config();
    let event = offer.params.as_ref().and_then(|p| p.get("event")).cloned().unwrap_or(Value::Null);
    let done = if param(offer, "calendar") == "google" {
        crate::google_calendar::add_accepted(&config, event.as_object().unwrap_or(&Dict::new())).await
    } else if crate::calendar::writes(&config, &agent.name) {
        match crate::calendar::queue_event(&config, &agent.name, &event) {
            Ok(_) => {
                state.push.notify_silent(&agent.name, "calendar", Dict::new()).await;
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    } else {
        Err("the phone's calendar is not shared with it any more".to_string())
    };
    if let Err(why) = done {
        tracing::info!(target: "bipbridge::offers", offer = %offer.id, why = %why, "event not written");
        let lang = lang_of(&config, &agent.name);
        let title = event.get("title").and_then(Value::as_str).unwrap_or("").to_string();
        tell(state, agent, place, offer, fill(texts(lang).calendar_failed, &[("title", &title)]), Vec::new()).await;
    }
}

/// « Oui » to an offer the agent does: ONE turn in the offer's conversation with exactly its prompt (`yes_note`), its
/// reply there; it could not run (the month's credit, busy, unreachable): one line, no model.
async fn turn(state: &AppState, agent: &AgentConfig, offer: &Offer, place: &crate::card_place::Place) {
    let pipeline = Pipeline::of(state);
    let conversation = if offer.asked { crate::card_place::resume_in(place) } else { "main".to_string() };
    let mut turn = Turn::new(agent, crate::background_turn::Kind::Card, &conversation, yes_note(offer));
    turn.budget = Some(TURN_BUDGET);
    turn.max_seconds = TURN_SECONDS;
    let config = state.config();
    let title = shown_agent(&config, &agent.name);
    match pipeline.run(&turn).await {
        Ok(ran) if ran.answered() && !crate::background_turn::is_silent(&ran.text) => {
            let delivery = Delivery {
                agent: agent.clone(),
                session: ran.session.clone(),
                key: format!("handback:offer-{}", ran.run_id),
                title,
                text: ran.text.clone(),
                as_written: true,
                run_id: Some(ran.run_id.clone()),
                notify: true,
                files: Vec::new(),
            };
            pipeline.deliver(&delivery).await;
        }
        Ok(ran) => {
            let told = !ran.answered() && pipeline.tell_stopped(agent, &ran, &title, "handback:offer").await;
            tracing::info!(target: "bipbridge::offers", agent = %agent.name, status = %ran.status, told, "offer turn: nothing to tell");
        }
        Err(why) => {
            tracing::info!(target: "bipbridge::offers", agent = %agent.name, offer = %offer.id, why = %why, "offer turn not run");
            let lang = lang_of(&config, &agent.name);
            tell(state, agent, place, offer, texts(lang).not_started.to_string(), Vec::new()).await;
        }
    }
}

// -- a moment that offers, the end --------------------------------------------------------------------------------------

/// A plan's moment that offers, at its time (routines.rs `fire`): its card placed, no model (None: routine `r` is no
/// such moment). Unasked: the daily caps hold it (not placed, logged); « Initiative » off, or a kind it can't do any
/// more: not placed.
pub async fn at_moment(state: &AppState, agent: &AgentConfig, r: &crate::routines::Routine) -> Option<String> {
    let plan_id = r.plan.as_deref().filter(|p| !p.is_empty())?;
    let config = state.config();
    let plan = super::get(&config, plan_id)?;
    let spec = plan.moments.iter().find(|m| m.routine.as_deref() == Some(r.id.as_str()))?.offer.clone()?;
    if !crate::agent_proactive::proposes(&config, &agent.name) {
        tracing::info!(target: "bipbridge::offers", agent = %agent.name, plan = %plan.id, "moment's offer not placed: Initiative off");
        return Some("offer not placed: off".into());
    }
    let (now, person) = (py::time_time(), person_zone(&config));
    let can = can(&config, agent);
    let kinds = available(&load(&config), &agent.name, &kinds_of(&can), now);
    let mut offer = match spec_of(&spec, &agent.name, &plan.title, &kinds, person, now, LABEL_MAX) {
        Ok(o) => o,
        Err(why) => {
            tracing::info!(target: "bipbridge::offers", agent = %agent.name, plan = %plan.id, why = %why, "moment's offer not placed");
            return Some("offer not placed".into());
        }
    };
    if let Err(why) = prepare(state, &config, agent, &can, &mut offer) {
        tracing::info!(target: "bipbridge::offers", agent = %agent.name, plan = %plan.id, why = %why, "moment's offer not placed");
        return Some("offer not placed".into());
    }
    offer.plan = Some(plan.id.clone());
    let plans = super::load(&config);
    let made = change(&config, |s| {
        if let Some(why) = refused(s, &plans, &offer, None, person, now) {
            return Err(why);
        }
        remember(s, &offer, now);
        s.made.push(Made { agent: offer.agent.clone(), at: now, asked: false, run: None, pill: false });
        s.offers.push(offer.clone());
        Ok(())
    });
    if let Err(why) = made {
        tracing::info!(target: "bipbridge::offers", agent = %agent.name, plan = %plan.id, why = %why, "moment's offer held by the caps");
        return Some("offer held".into());
    }
    let run_id = format!("{RUN_PREFIX}{}", offer.id);
    // made with no turn: the main conversation shows it; nothing to tell the agent when it is answered
    crate::card_place::taken(&config, &run_id);
    let lang = lang_of(&config, &agent.name);
    state.push.notify_bridge_card(&agent.name, &notification(lang, &offer.label), PUSH_KEY, &run_id).await;
    tracing::info!(target: "bipbridge::offers", agent = %agent.name, plan = %plan.id, offer = %offer.id, "moment's offer placed");
    Some("offer placed".into())
}

/// One look (plans.rs `watch`): the offers past their time go without a word (their decision kept as « expired »,
/// the apps 1.1 fold them into « Passée »); those of an agent turned off go too, uncounted.
pub async fn tick(state: &AppState) {
    let config = state.config();
    if !path(&config).exists() {
        return;
    }
    let now = py::time_time();
    let store = load(&config);
    if store.offers.iter().any(|o| !crate::agent_proactive::proposes(&config, &o.agent)) {
        let gone = change(&config, |s| {
            let (gone, kept): (Vec<Offer>, Vec<Offer>) = std::mem::take(&mut s.offers)
                .into_iter()
                .partition(|o| !crate::agent_proactive::proposes(&config, &o.agent));
            s.offers = kept;
            gone.len()
        });
        tracing::info!(target: "bipbridge::offers", gone, "offers withdrawn: Initiative off");
    }
    for offer in expire(&config, now) {
        tracing::info!(target: "bipbridge::offers", agent = %offer.agent, offer = %offer.id, "offer expired");
        let Some(agent) = config.agent(Some(&offer.agent)).cloned() else { continue };
        let run_id = format!("{RUN_PREFIX}{}", offer.id);
        // its decision kept where the card showed: the main conversation's tip for one nobody asked for, even before
        // anything was said there (card_place.rs `answered_via` then only finds no conversation)
        let place = crate::card_place::of(&config, &run_id).unwrap_or_default();
        let session = match place.report.clone().or(place.session.clone()) {
            Some(s) => s,
            None => Pipeline::of(state).tip(&agent, "main").await,
        };
        if !session.is_empty() {
            let mut fields = serde_json::Map::new();
            fields.insert("run_id".into(), json!(run_id));
            fields.insert("request_id".into(), json!(offer.id));
            fields.insert("choice".into(), json!(EXPIRED));
            fields.insert("session_id".into(), json!(session));
            if let Err(err) = crate::approvals_log::record(&config, &agent.name, &fields) {
                tracing::debug!(target: "bipbridge::offers", agent = %agent.name, error = %err, "expiry not kept");
            }
        }
        crate::card_place::answered_via(state, &agent, &run_id, EXPIRED, None).await;
    }
}

#[cfg(test)]
mod tests;
