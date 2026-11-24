//! The plans' and offers' sources the bridge reads itself (passation-pro briefs/design-agents-proactifs.md §3.2 n° 4
//! and 5, §3.4 « Réponses dues » and « L'agenda », §3.7, §6 row 7, §7.3; Alex 22/11: Q10 A, Q11 A, replies due turned
//! on mailbox by mailbox, the trip offered at the next exchange, « vers 8 h », 90 days for what the bridge sets up on
//! its own; contracts/plans-sources/cases.json). No model anywhere here: a look once a day, cards built by the bridge.
//!
//! - **The calendars** an agent reads (the phone's copy, Google Calendar, a mailbox's CalDAV calendars), « vers 8 h »
//!   (`morning`): an appointment in 1 to 7 days, with a place, not all-day, not repeating, with no plan, becomes a
//!   hint (`Hint`). The person's next message to that agent carries ONE hidden line about it (live_brief.rs
//!   `message_notes`, `take_note`): no turn of its own. No exchange by the day before: ONE offer card of the bridge
//!   (`appointment`: « Dentiste jeu. 26 à 15:00 · Je prépare le trajet et un rappel ? »), whose « Oui » sets the plan
//!   up without the model (`appointment_yes`: a reminder the day before at 19:00, when to leave), following its event.
//! - **The Carnet's trips** within 7 days with no plan: the same hint; no exchange by two days before, ONE card
//!   (`trip`), whose « Oui » is one turn of the agent that may propose its plan (`granted`: that plan is the person's
//!   ask).
//! - **Replies due** (`replies`): off for every mailbox; ONE consent card per mailbox (`reply_watch`); its « Oui » (or
//!   `PUT /v1/agents/{agent}/replies`) makes a trigger of the bridge (triggers.rs `REPLIES`) whose IMAP watch reads the
//!   new messages' headers only (`seen_mail`: mass mailings, lists, robots and the person's own left out); « vers 18 h »
//!   (`evening`) those waiting more than a day without a reply in Sent (`due`) come on ONE offer card built from their
//!   headers; its « Oui » is one turn that reads those e-mails only and drafts their replies (`gate`), never sends.
//! - **Following** an event of Google Calendar or CalDAV a plan is about (`follow`, the `calendar_event` watch): read
//!   again every hour; moved, the moments move; cancelled, the plan goes (plans.rs `apply_news`).
//! - **Who**: an agent that proposes (« Initiative » not off, never one other people talk to) and whose model is not
//!   outside (`outside`); the one the person writes to most (feature_tips.rs `talks_of`).
//!
//! Kept here (headers only for the e-mails; a hint's words until it is told or carded):
//!
//!     <data>/plans-sources.json (0600)   {hints: [...], mailboxes: [...], mails: [...], done: {key: day},
//!                                         grants: [{agent, until}]}
//!
//! No Python counterpart.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::offers::{self, Offer};
use super::{fill, lang_of, person_zone, shown_agent, EventRef, Moment, Plan};
use crate::app::AppState;
use crate::background_turn::{Delivery, Pipeline};
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const NOTE_OPEN: &str = "<!--bip:plans\n";
/// `bip_offer.source` of the bridge's own cards.
pub const CALENDAR: &str = "calendar";
pub const TRIP: &str = "trip";
pub const REPLIES: &str = "replies";
/// The bridge's own offer kinds (never in the agent's offer_act: plans/offers.rs `KINDS`).
pub const APPOINTMENT: &str = "appointment";
pub const REPLY_WATCH: &str = "reply_watch";
pub const DRAFT: &str = "draft";
/// The days looked at ahead, and the hints an agent gets at most.
pub const LOOK_DAYS: i64 = 7;
pub const HINTS_PER_AGENT: usize = 3;
/// A replies card's e-mails, at most.
pub const ITEMS_MAX: usize = 8;
/// An e-mail's headers kept after its answer, a « Oui » or a « Non merci »; any after being seen.
pub const KEEP_DAYS: f64 = 14.0;
pub const SEEN_DAYS: f64 = 30.0;
/// A consent card asked once more after this long, `ASKS_MAX` times in all.
pub const ASK_AGAIN_DAYS: f64 = 14.0;
pub const ASKS_MAX: u32 = 2;
/// A trip's « Oui »: the plan proposed within this long is the person's ask.
pub const GRANT_SECONDS: f64 = 900.0;
/// On foot up to this long, else by transit (an appointment's « Oui »).
pub const WALK_MAX_MINUTES: u32 = 30;
/// The reminder the day before, at this hour.
pub const EVE_HOUR: u32 = 19;
const DAY: f64 = 86400.0;

// -- the settings ---------------------------------------------------------------------------------------------------------

/// `[plans_sources]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// How often the sources are looked at (a local file read: cheap).
    pub tick_seconds: f64,
    /// The morning look (calendars, trips, consent cards): « vers 8 h » on the person's clock; None: every tick (tests).
    pub morning: Option<(u32, u32)>,
    /// The evening look (replies due): « vers 18 h »; None: every tick (tests).
    pub evening: Option<(u32, u32)>,
    /// An e-mail is due after this long with no reply.
    pub due_hours: f64,
    /// A Google or CalDAV event a plan follows is read again this often.
    pub follow_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tick_seconds: 60.0, morning: Some((8, 0)), evening: Some((18, 0)), due_hours: 24.0, follow_seconds: 3600.0 }
    }
}

fn clock_of(value: Option<&Value>, default: Option<(u32, u32)>) -> Option<(u32, u32)> {
    match value.and_then(Value::as_str).map(str::trim) {
        Some("any") => None,
        Some(text) => text
            .split_once(':')
            .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
            .filter(|(h, m)| *h < 24 && *m < 60)
            .or(default),
        None => default,
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let d = Settings::default();
        let f = |key: &str, default: f64, min: f64, max: f64| -> f64 {
            table.get(key).and_then(Value::as_f64).map(|v| v.clamp(min, max)).unwrap_or(default)
        };
        Settings {
            tick_seconds: f("tick_seconds", d.tick_seconds, 0.2, 3600.0),
            morning: clock_of(table.get("morning"), d.morning),
            evening: clock_of(table.get("evening"), d.evening),
            due_hours: f("due_hours", d.due_hours, 0.0, 24.0 * 14.0),
            follow_seconds: f("follow_seconds", d.follow_seconds, 0.5, 86400.0),
        }
    }
}

// -- the store ------------------------------------------------------------------------------------------------------------

/// Something to come the bridge noticed for an agent: an appointment of its calendars, a trip of the Carnet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    pub agent: String,
    /// `CALENDAR` or `TRIP`.
    pub source: String,
    /// What it is known by (`calendar:<source>:<id>`, `trip:<path>:<start>`): never told nor carded twice.
    pub key: String,
    /// When it starts (Unix seconds; a trip: its first day's midnight).
    pub start: f64,
    /// A trip's last day's midnight.
    #[serde(default)]
    pub end: f64,
    /// Its words, until it is told or carded (then dropped): the event's title, place; the trip's city, page.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub place: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<EventRef>,
    #[serde(default)]
    pub told: bool,
    #[serde(default)]
    pub carded: bool,
    pub at: f64,
}

/// A mailbox's replies-due watch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mailbox {
    pub account: String,
    pub agent: String,
    /// Its address (the consent card's « dans {boîte} »).
    pub address: String,
    /// `asked`, `on`, `declined` or `off`.
    pub state: String,
    #[serde(default)]
    pub asks: u32,
    #[serde(default)]
    pub asked_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default)]
    pub ends: f64,
}

pub const ASKED: &str = "asked";
pub const ON: &str = "on";
pub const DECLINED: &str = "declined";
pub const OFF: &str = "off";

/// An e-mail of a watched mailbox, its headers only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mail {
    pub account: String,
    pub uid: u64,
    pub message_id: String,
    /// The sender as shown: its name, else its address.
    pub from: String,
    pub subject: String,
    /// When the watch saw it arrive.
    pub seen: f64,
    /// `waiting` (not on a card yet), `shown`, `answered`, `drafted`, `declined`.
    pub state: String,
    #[serde(default)]
    pub done_at: f64,
}

pub const WAITING: &str = "waiting";
pub const SHOWN: &str = "shown";
pub const ANSWERED: &str = "answered";
pub const DRAFTED: &str = "drafted";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub agent: String,
    pub until: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub hints: Vec<Hint>,
    #[serde(default)]
    pub mailboxes: Vec<Mailbox>,
    #[serde(default)]
    pub mails: Vec<Mail>,
    /// The looks done: `morning` → its day, `evening:<account>` → its day.
    #[serde(default)]
    pub done: BTreeMap<String, String>,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

static LOCK: Mutex<()> = Mutex::new(());
static LAST_TICK: Mutex<f64> = Mutex::new(0.0);
static LAST_FOLLOW: Mutex<f64> = Mutex::new(0.0);

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("plans-sources.json")
}

pub fn load(config: &Config) -> Store {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// What is past its keeping dropped (`forget`): headers 14 days after they were dealt with, 30 after they were seen;
/// a hint once its moment has passed; a grant past its time.
pub fn forget(store: &mut Store, now: f64) {
    store.mails.retain(|m| {
        let done = m.state != WAITING && m.state != SHOWN;
        now - m.seen < SEEN_DAYS * DAY && !(done && now - m.done_at >= KEEP_DAYS * DAY)
    });
    store.hints.retain(|h| h.start > now);
    store.grants.retain(|g| g.until > now);
}

/// Read, change, write (0600), under one lock.
pub fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    let out = f(&mut store);
    forget(&mut store, py::time_time());
    let file = path(config);
    let body = serde_json::to_string_pretty(&store).unwrap_or_default();
    if let Err(e) = crate::profile_files::write_atomic_with(&file, body.as_bytes(), 0o600) {
        tracing::warn!(file = %file.display(), error = %e, "plans' sources not saved");
    }
    out
}

// -- who -------------------------------------------------------------------------------------------------------------------

/// The hook of « Modèle extérieur » (branch shared-memory, `outside_model::place`): an agent whose model is outside our
/// European providers and the person's own never has their calendar or e-mails read to propose. Until that branch is
/// in, no agent is (fill in: `matches!(outside_model::place(config, agent), Place::Outside { .. } | Place::Unknown)`).
pub fn outside(config: &Config, agent: &str) -> bool {
    let _ = (config, agent);
    false
}

/// The sources are read for `agent`: it exists, proposes (« Initiative » not off, never an agent other people talk
/// to), and its model is not outside.
pub fn may(config: &Config, agent: &str) -> bool {
    config.agent(Some(agent)).is_some() && crate::agent_proactive::proposes(config, agent) && !outside(config, agent)
}

/// The one of `agents` the person writes to most (the last 14 days), then by name.
pub fn most_written(config: &Config, mut agents: Vec<String>) -> Option<String> {
    agents.sort();
    agents.dedup();
    let mut best: Option<(String, u32)> = None;
    for a in agents {
        let n = crate::feature_tips::talks_of(config, &a);
        if best.as_ref().is_none_or(|(_, b)| n > *b) {
            best = Some((a, n));
        }
    }
    best.map(|b| b.0)
}

/// `agent` reads a calendar: the phone's copy shared with it to read, Google Calendar, a mailbox's CalDAV.
pub fn reads_calendar(config: &Config, agent: &str) -> bool {
    crate::calendar::reads(config, agent)
        || crate::google_calendar::offered(config, agent)
        || crate::caldav::shared_with(config, agent)
}

fn plans_ok(config: &Config, agent: &str) -> bool {
    may(config, agent) && crate::routines::enabled(config, agent)
}

/// The calendars' agent: one that reads a calendar, may propose plans; several, the one written to most.
pub fn calendar_agent(config: &Config) -> Option<String> {
    let names: Vec<String> =
        config.agents.keys().filter(|a| plans_ok(config, a) && reads_calendar(config, a)).cloned().collect();
    most_written(config, names)
}

/// The trips' agent: « Quotidien » (category « daily ») with the Carnet, else the calendars' agent with it, else the
/// one with the Carnet the person writes to most.
pub fn trip_agent(config: &Config) -> Option<String> {
    let carnet = |a: &str| plans_ok(config, a) && crate::notebook::enabled(config, a);
    let daily: Vec<String> = config
        .agents
        .values()
        .filter(|a| a.category.as_deref() == Some("daily") && carnet(&a.name))
        .map(|a| a.name.clone())
        .collect();
    most_written(config, daily)
        .or_else(|| calendar_agent(config).filter(|a| carnet(a)))
        .or_else(|| most_written(config, config.agents.keys().filter(|a| carnet(a)).cloned().collect()))
}

/// A mailbox's agent: among those it is given to that may propose, the one written to most.
pub fn mailbox_agent(config: &Config, account: &Dict) -> Option<String> {
    let given: Vec<String> = account
        .get("agents")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).map(|a| a.trim().to_lowercase()).collect())
        .unwrap_or_default();
    let names: Vec<String> = config.agents.keys().filter(|a| given.contains(&a.to_lowercase()) && may(config, a)).cloned().collect();
    most_written(config, names)
}

// -- the texts (contracts/plans-sources/cases.json `texts`) -----------------------------------------------------------------

pub struct Texts {
    pub lang: &'static str,
    pub calendar_offer: &'static str,
    pub trip_offer: &'static str,
    pub consent: &'static str,
    pub replies_top: &'static str,
    pub replies_line: &'static str,
    pub replies_question: &'static str,
    pub replies_under_i: &'static str,
    pub replies_notification: &'static str,
    pub replies_top_one: &'static str,
    pub replies_notification_one: &'static str,
    pub replies_about: &'static str,
    pub since_one: &'static str,
    pub since_many: &'static str,
    pub consent_under_i: &'static str,
    pub watch_name: &'static str,
    pub watch_prompt: &'static str,
    pub calendar_reminder: &'static str,
    pub calendar_reminder_what: &'static str,
    pub calendar_detail_reminder: &'static str,
    pub calendar_detail_route: &'static str,
    pub calendar_detail_follow: &'static str,
    pub trip_prompt: &'static str,
    /// Short month names, for a trip's dates.
    pub months: [&'static str; 12],
}

pub static TEXTS: [Texts; 4] = [
    Texts {
        lang: "fr",
        calendar_offer: "{title} {day} à {time} · Je prépare le trajet et un rappel ?",
        trip_offer: "{city}, {dates} · Je prépare ton voyage ?",
        consent: "Je te dis quand un e-mail attend ta réponse depuis plus d’un jour, dans {mailbox} ? Je ne lis que l’expéditeur et l’objet.",
        replies_top: "{agent} · {n} e-mails attendent ta réponse",
        replies_line: "{from} — {subject} · depuis {since}",
        replies_question: "Je prépare les brouillons ?",
        replies_under_i: "Les brouillons vont dans tes Brouillons ; rien n’est envoyé.",
        replies_notification: "{n} e-mails attendent ta réponse",
        replies_top_one: "{agent} · 1 e-mail attend ta réponse",
        replies_notification_one: "1 e-mail attend ta réponse",
        replies_about: "Réponses dues",
        since_one: "{n} j",
        since_many: "{n} j",
        consent_under_i: "Le soir, s’il y en a, une carte te les montre, et je prépare les brouillons si tu veux. Tu l’arrêtes dans « Ce que tes agents font tout seuls ».",
        watch_name: "Réponses dues · {mailbox}",
        watch_prompt: "Le soir, je te dis quels e-mails attendent ta réponse depuis plus d’un jour (l’expéditeur et l’objet seulement).",
        calendar_reminder: "{title} demain à {time}",
        calendar_reminder_what: "Rappel la veille",
        calendar_detail_reminder: "La veille à 19:00 : « {say} ».",
        calendar_detail_route: "Le jour même, l’heure de partir pour {place}, calculée à ton « Oui » depuis chez toi.",
        calendar_detail_follow: "S’il bouge, les rappels suivent.",
        trip_prompt: "Je regarde ton voyage dans le Carnet et je te propose un plan : les bons rappels au bon moment. Rien n’est réservé.",
        months: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    },
    Texts {
        lang: "en",
        calendar_offer: "{title} {day} at {time} · Shall I prepare the route and a reminder?",
        trip_offer: "{city}, {dates} · Shall I get your trip ready?",
        consent: "Shall I tell you when an e-mail in {mailbox} has waited over a day for your reply? I only read the sender and subject.",
        replies_top: "{agent} · {n} e-mails are waiting for your reply",
        replies_line: "{from} — {subject} · {since}",
        replies_question: "Shall I draft the replies?",
        replies_under_i: "The drafts go to your Drafts folder; nothing is sent.",
        replies_notification: "{n} e-mails are waiting for your reply",
        replies_top_one: "{agent} · 1 e-mail is waiting for your reply",
        replies_notification_one: "1 e-mail is waiting for your reply",
        replies_about: "Replies due",
        since_one: "{n} day",
        since_many: "{n} days",
        consent_under_i: "In the evening, if there are any, a card shows them, and I draft the replies if you want. You stop it in “What your agents do on their own”.",
        watch_name: "Replies due · {mailbox}",
        watch_prompt: "In the evening, I tell you which e-mails have waited over a day for your reply (the sender and subject only).",
        calendar_reminder: "{title} tomorrow at {time}",
        calendar_reminder_what: "Reminder the day before",
        calendar_detail_reminder: "The day before at 19:00: “{say}”.",
        calendar_detail_route: "On the day, when to leave for {place}, worked out at your “Yes” from your home.",
        calendar_detail_follow: "If it moves, the reminders follow.",
        trip_prompt: "I look at your trip in the Notebook and suggest a plan: the right reminders at the right time. Nothing is booked.",
        months: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    },
    Texts {
        lang: "de",
        calendar_offer: "{title} {day} um {time} · Soll ich den Weg und eine Erinnerung vorbereiten?",
        trip_offer: "{city}, {dates} · Soll ich deine Reise vorbereiten?",
        consent: "Soll ich dir sagen, wenn eine E-Mail in {mailbox} seit über einem Tag auf deine Antwort wartet? Ich lese nur Absender und Betreff.",
        replies_top: "{agent} · {n} E-Mails warten auf deine Antwort",
        replies_line: "{from} — {subject} · seit {since}",
        replies_question: "Soll ich die Antworten entwerfen?",
        replies_under_i: "Die Entwürfe landen in deinem Entwürfe-Ordner; nichts wird gesendet.",
        replies_notification: "{n} E-Mails warten auf deine Antwort",
        replies_top_one: "{agent} · 1 E-Mail wartet auf deine Antwort",
        replies_notification_one: "1 E-Mail wartet auf deine Antwort",
        replies_about: "Fällige Antworten",
        since_one: "{n} Tag",
        since_many: "{n} Tagen",
        consent_under_i: "Am Abend zeigt dir eine Karte sie, falls es welche gibt, und ich entwerfe die Antworten, wenn du willst. Du stoppst das unter „Was deine Agenten von selbst tun“.",
        watch_name: "Fällige Antworten · {mailbox}",
        watch_prompt: "Am Abend sage ich dir, welche E-Mails seit über einem Tag auf deine Antwort warten (nur Absender und Betreff).",
        calendar_reminder: "{title} morgen um {time}",
        calendar_reminder_what: "Erinnerung am Vortag",
        calendar_detail_reminder: "Am Vortag um 19:00: „{say}“.",
        calendar_detail_route: "Am Tag selbst, wann du nach {place} aufbrechen solltest, berechnet bei deinem „Ja“ ab deinem Zuhause.",
        calendar_detail_follow: "Verschiebt er sich, ziehen die Erinnerungen mit.",
        trip_prompt: "Ich sehe mir deine Reise im Notizbuch an und schlage dir einen Plan vor: die richtigen Erinnerungen zur richtigen Zeit. Nichts wird gebucht.",
        months: ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
    },
    Texts {
        lang: "es",
        calendar_offer: "{title} el {day} a las {time} · ¿Preparo el trayecto y un recordatorio?",
        trip_offer: "{city}, {dates} · ¿Preparo tu viaje?",
        consent: "¿Te aviso cuando un correo de {mailbox} lleve más de un día esperando tu respuesta? Solo leo el remitente y el asunto.",
        replies_top: "{agent} · {n} correos esperan tu respuesta",
        replies_line: "{from} — {subject} · desde hace {since}",
        replies_question: "¿Preparo los borradores?",
        replies_under_i: "Los borradores van a tu carpeta de Borradores; no se envía nada.",
        replies_notification: "{n} correos esperan tu respuesta",
        replies_top_one: "{agent} · 1 correo espera tu respuesta",
        replies_notification_one: "1 correo espera tu respuesta",
        replies_about: "Respuestas pendientes",
        since_one: "{n} día",
        since_many: "{n} días",
        consent_under_i: "Por la tarde, si hay alguno, una tarjeta te los muestra, y preparo los borradores si quieres. Lo detienes en «Lo que tus agentes hacen solos».",
        watch_name: "Respuestas pendientes · {mailbox}",
        watch_prompt: "Por la tarde, te digo qué correos llevan más de un día esperando tu respuesta (solo el remitente y el asunto).",
        calendar_reminder: "{title} mañana a las {time}",
        calendar_reminder_what: "Recordatorio la víspera",
        calendar_detail_reminder: "La víspera a las 19:00: «{say}».",
        calendar_detail_route: "El mismo día, cuándo salir hacia {place}, calculado con tu «Sí» desde tu casa.",
        calendar_detail_follow: "Si cambia, los recordatorios se ajustan.",
        trip_prompt: "Miro tu viaje en el Cuaderno y te propongo un plan: los recordatorios justos en el momento justo. No se reserva nada.",
        months: ["ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sept.", "oct.", "nov.", "dic."],
    },
];

/// The texts in `lang` (another language: English).
pub fn texts(lang: &str) -> &'static Texts {
    TEXTS.iter().find(|t| t.lang == lang).unwrap_or(&TEXTS[1])
}

/// « jeu. 26 » (de: « Do. 26. »).
pub fn day_words(lang: &str, at: f64, zone: Tz) -> String {
    let t = super::texts(lang);
    match zone.timestamp_opt(at as i64, 0).single() {
        Some(local) => {
            let day = t.days_short[local.weekday().num_days_from_monday() as usize];
            if lang == "de" {
                format!("{day} {}.", local.day())
            } else {
                format!("{day} {}", local.day())
            }
        }
        None => String::new(),
    }
}

fn clock(at: f64, zone: Tz) -> String {
    zone.timestamp_opt(at as i64, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

/// A trip's dates: « 25–28 nov. », « 30 nov.–2 déc. » (fr, es), « Nov 25–28 » (en), « 25.–28. Nov. » (de).
pub fn dates_words(lang: &str, start: NaiveDate, end: NaiveDate) -> String {
    let t = texts(lang);
    let month = |d: NaiveDate| t.months[d.month0() as usize];
    let same = start.month() == end.month() && start.year() == end.year();
    match (t.lang, same, start == end) {
        ("en", _, true) => format!("{} {}", month(start), start.day()),
        ("en", true, _) => format!("{} {}–{}", month(start), start.day(), end.day()),
        ("en", false, _) => format!("{} {}–{} {}", month(start), start.day(), month(end), end.day()),
        ("de", _, true) => format!("{}. {}", start.day(), month(start)),
        ("de", true, _) => format!("{}.–{}. {}", start.day(), end.day(), month(end)),
        ("de", false, _) => format!("{}. {}–{}. {}", start.day(), month(start), end.day(), month(end)),
        (_, _, true) => format!("{} {}", start.day(), month(start)),
        (_, true, _) => format!("{}–{} {}", start.day(), end.day(), month(end)),
        (_, false, _) => format!("{} {}–{} {}", start.day(), month(start), end.day(), month(end)),
    }
}

/// « 2 j », « 1 day », « 2 Tagen », « 1 día ».
pub fn since_words(lang: &str, days: u32) -> String {
    let t = texts(lang);
    fill(if days == 1 { t.since_one } else { t.since_many }, &[("n", &days.to_string())])
}

/// Text a card shows that came from outside (an event's title, a sender, a subject): one plain line, no markup.
pub fn clean(text: &str, max: usize) -> String {
    let flat: String =
        text.chars().map(|c| if c.is_control() || matches!(c, '<' | '>' | '`') { ' ' } else { c }).collect();
    let flat = flat.replace("](", " (").replace("**", "");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max.saturating_sub(1)).collect::<String>().trim_end())
    }
}

// -- the calendars ------------------------------------------------------------------------------------------------------------

/// An event of a calendar, as the look keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub source: String,
    pub id: String,
    pub calendar: String,
    pub title: String,
    pub place: String,
    pub start: f64,
}

/// An event's source by its id: `google:…` Google Calendar, `mail:…` a mailbox's CalDAV, else the phone's.
pub fn source_of(id: &str) -> &'static str {
    if id.starts_with(crate::google_calendar::ID_PREFIX) {
        "google"
    } else if id.starts_with("mail:") {
        "caldav"
    } else {
        "phone"
    }
}

/// The events of `from`..`to` in every calendar `agent` reads (the phone's copy, CalDAV, Google), and whether all
/// answered.
pub async fn events_of(config: &Config, agent: &str, from: NaiveDate, to: NaiveDate) -> (Vec<Value>, crate::caldav::Remote) {
    let mut events: Vec<Value> = Vec::new();
    if crate::calendar::reads(config, agent) {
        let copy = crate::calendar::read(config);
        events.extend(copy.get("events").and_then(Value::as_array).cloned().unwrap_or_default());
    }
    let (c, a) = (config.clone(), agent.to_string());
    let sources = tokio::task::spawn_blocking(move || crate::caldav::sources(&c, &a)).await.unwrap_or_default();
    let mut remote = crate::caldav::listed(&sources, from, to).await;
    if crate::google_calendar::offered(config, agent) {
        crate::google_calendar::listed(config, &mut remote, from, to).await;
    }
    events.extend(remote.events.iter().cloned());
    (events, remote)
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// The events worth a plan among `events`: in 1 to 7 days (its day after `today`), with a place, not all-day, not
/// repeating (marked so, or several occurrences of one id), the soonest first.
pub fn upcoming(events: &[Value], zone: Tz, today: NaiveDate) -> Vec<Found> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in events {
        *counts.entry(text_of(e, "id")).or_default() += 1;
    }
    let mut out: Vec<Found> = events
        .iter()
        .filter_map(|e| {
            let id = text_of(e, "id");
            let start = super::event_start(e, zone)?;
            let day = zone.timestamp_opt(start as i64, 0).single()?.date_naive();
            let ahead = (day - today).num_days();
            let repeats = counts.get(&id).copied().unwrap_or(0) > 1 || e.get("recurring").is_some_and(py::truthy);
            let all_day = e.get("all_day").is_some_and(py::truthy);
            let (title, place) = (text_of(e, "title"), text_of(e, "location"));
            (!id.is_empty() && !repeats && !all_day && !title.is_empty() && !place.is_empty() && (1..=LOOK_DAYS).contains(&ahead))
                .then(|| Found { source: source_of(&id).into(), id, calendar: text_of(e, "calendar"), title, place, start })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// A plan of the place follows the event, or is about it (an appointment of the same title with a moment within the
/// day before it).
pub fn planned(plans: &super::Store, found: &Found) -> bool {
    let fold = |t: &str| crate::tools_knowledge::ascii_fold(&t.to_lowercase()).split_whitespace().collect::<Vec<_>>().join(" ");
    plans.plans.iter().any(|p| {
        p.event.as_ref().is_some_and(|e| e.id == found.id)
            || (p.kind == "appointment"
                && fold(&p.title) == fold(&found.title)
                && p.moments.iter().any(|m| m.at <= found.start && found.start - m.at <= 1.5 * DAY))
    })
}

/// A trip of the Carnet, as the look keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct TripFound {
    pub path: String,
    pub city: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
}

/// The Carnet's trips starting in 1 to 7 days, not called off.
pub fn trips(config: &Config, today: NaiveDate) -> Vec<TripFound> {
    let mut out = Vec::new();
    for (path, page) in crate::notebook::all_pages(config, crate::notebook::Part::Main) {
        if page.kind() != "trip" || crate::notebook::trip::cancelled(&page) {
            continue;
        }
        let (Some(start), end) = crate::notebook::trip::shown_dates(&page) else { continue };
        let ahead = (start - today).num_days();
        if !(1..=LOOK_DAYS).contains(&ahead) {
            continue;
        }
        let city = page
            .front
            .get("destination")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| page.title.clone());
        out.push(TripFound { path, city, start, end: end.unwrap_or(start) });
    }
    out.sort_by_key(|t| t.start);
    out
}

fn midnight(day: NaiveDate, zone: Tz) -> f64 {
    day.and_hms_opt(0, 0, 0)
        .and_then(|t| zone.from_local_datetime(&t).earliest())
        .map(|d| d.timestamp() as f64)
        .unwrap_or(0.0)
}

/// A plan of kind trip of the place whose moments fall within the trip (a day before its start to its end).
pub fn trip_planned(plans: &super::Store, trip: &TripFound, zone: Tz) -> bool {
    let (from, to) = (midnight(trip.start, zone) - DAY, midnight(trip.end, zone) + DAY);
    plans.plans.iter().any(|p| p.kind == "trip" && p.moments.iter().any(|m| m.at >= from && m.at <= to))
}

/// A hint whose event or trip has a plan now.
pub fn hint_planned(plans: &super::Store, hint: &Hint, zone: Tz) -> bool {
    match (&hint.event, hint.source.as_str()) {
        (Some(e), _) => planned(
            plans,
            &Found { source: e.source.clone(), id: e.id.clone(), calendar: e.calendar.clone(), title: hint.title.clone(),
                     place: String::new(), start: hint.start },
        ),
        (None, TRIP) => match (day_of(hint.start, zone), day_of(hint.end, zone)) {
            (Some(start), Some(end)) => {
                trip_planned(plans, &TripFound { path: String::new(), city: String::new(), start, end: end.max(start) }, zone)
            }
            _ => false,
        },
        _ => false,
    }
}

fn day_of(at: f64, zone: Tz) -> Option<NaiveDate> {
    zone.timestamp_opt(at as i64, 0).single().map(|d| d.date_naive())
}

/// A hint's card is due today: no exchange told it, and it is the day before an appointment (or later, before it), two
/// days before a trip (or later, before it starts).
pub fn card_due(hint: &Hint, today: NaiveDate, zone: Tz, now: f64) -> bool {
    if hint.told || hint.carded || hint.start <= now {
        return false;
    }
    let Some(day) = day_of(hint.start, zone) else { return false };
    let before = if hint.source == TRIP { 2 } else { 1 };
    (day - today).num_days() <= before && (hint.source == TRIP || hint.start - now > 2.0 * 3600.0)
}

// -- the hint at the next exchange --------------------------------------------------------------------------------------------

fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// Whether others read `session` of `agent`: a live read-only share of it (or of the agent's whole thread,
/// read_shares.rs), or one of the agent's rooms (rooms.rs).
fn seen_by_others(config: &Config, agent: &str, session: &str) -> bool {
    !crate::read_shares::shares_for(config, agent, session).is_empty()
        || !crate::read_shares::thread_shares(config, agent).is_empty()
        || crate::rooms::all(config).iter().any(|r| r.sessions.get(agent).is_some_and(|s| s == session))
}

/// The hidden line the person's next message to `agent` carries: what is coming with no plan, told once (its words
/// dropped then). None: nothing to tell, or not here (a routine's conversation, the family's common one).
pub fn take_note(config: &Config, agent: &str, session: &str) -> Option<String> {
    if session.starts_with("cron_") || crate::family_thread::is_common(config, agent) || !may(config, agent) {
        return None;
    }
    let now = py::time_time();
    let waiting = |h: &Hint| h.agent == agent && !h.told && !h.carded && h.start > now;
    if !path(config).exists() || !load(config).hints.iter().any(waiting) {
        return None;
    }
    // a conversation others see (shown read-only to friends, a room of agents): kept for a conversation of the
    // person's own, as the agent's private tools there (review C7)
    if seen_by_others(config, agent, session) {
        return None;
    }
    let zone = person_zone(config);
    let lang = lang_of(config, agent);
    let hints = change(config, |store| {
        let mut taken = Vec::new();
        for h in store.hints.iter_mut().filter(|h| waiting(h)) {
            taken.push(h.clone());
            h.told = true;
            (h.title, h.place) = (String::new(), String::new());
        }
        taken
    });
    if hints.is_empty() {
        return None;
    }
    let mut blocks = Vec::new();
    let mut trips = Vec::new();
    for (i, h) in hints.iter().enumerate() {
        if h.source == TRIP {
            let (start, end) = (day_of(h.start, zone), day_of(h.end, zone));
            trips.push(format!(
                "Trip in the Carnet: « {} », {} to {}, its page « {} ».",
                tk::in_note_line(&h.title),
                start.map(|d| d.to_string()).unwrap_or_default(),
                end.map(|d| d.to_string()).unwrap_or_default(),
                tk::in_note_line(&h.place),
            ));
            continue;
        }
        let event = h.event.as_ref();
        blocks.push(format!(
            "=== event {n} ===\nTitle: {}\nWhen: {} · {}\nWhere: {}\nEvent: {{\"source\": {}, \"id\": {}}}\n=== end {n} ===",
            tk::in_note_line(&h.title),
            day_words(lang, h.start, zone),
            clock(h.start, zone),
            tk::in_note_line(&h.place),
            quoted(event.map(|e| e.source.as_str()).unwrap_or("phone")),
            quoted(event.map(|e| e.id.as_str()).unwrap_or("")),
            n = i + 1,
        ));
    }
    let mut lines = vec![
        "Not from your person's words: the app noticed this is coming for them, with nothing planned for it yet. After \
answering what they asked, and only if it fits, you may propose ONE plan for it (plan_propose: for an appointment, give \
its `event` so the plan follows it) or offer to help (offer_act). Otherwise say nothing about it, and never say you got \
this note."
            .to_string(),
    ];
    if !blocks.is_empty() {
        lines.push(crate::calendar::UNTRUSTED.to_string());
        lines.extend(blocks);
    }
    lines.extend(trips);
    tracing::info!(target: "bipbridge::plans", agent, hints = hints.len(), "plans' sources told at the next exchange");
    Some(format!("{NOTE_OPEN}{}\n-->", lines.join("\n")))
}

// -- the bridge's cards -----------------------------------------------------------------------------------------------------------

/// One of the bridge's own offers (`when`: made, expires).
fn bridge_offer(agent: &str, kind: &str, source: &str, label: String, about: String, prompt: String, when: (f64, f64)) -> Offer {
    let (now, expires) = when;
    Offer {
        id: py::uuid4_hex()[..12].to_string(),
        agent: agent.to_string(),
        kind: kind.to_string(),
        label,
        about,
        prompt,
        params: None,
        site: None,
        plan: None,
        pill: false,
        made: now,
        expires: expires.min(now + offers::EXPIRES_SECONDS),
        asked: false,
        first: false,
        subject: String::new(),
        outside: false,
        source: source.to_string(),
        items: Vec::new(),
        top: String::new(),
        mailbox: String::new(),
    }
}

/// What the bridge would do at an appointment's « Oui », for its « i »: the reminder the day before when still to
/// come, when to leave, and that it follows the event.
pub fn appointment_detail(lang: &str, title: &str, place: &str, start: f64, zone: Tz, now: f64) -> String {
    let t = texts(lang);
    let mut parts = Vec::new();
    if eve_of(start, zone).is_some_and(|e| e > now + 60.0) {
        let say = fill(t.calendar_reminder, &[("title", title), ("time", &clock(start, zone))]);
        parts.push(fill(t.calendar_detail_reminder, &[("say", &say)]));
    }
    parts.push(fill(t.calendar_detail_route, &[("place", place)]));
    parts.push(t.calendar_detail_follow.to_string());
    parts.join(" ")
}

/// The day before `start` at 19:00, on `zone`'s clock.
pub fn eve_of(start: f64, zone: Tz) -> Option<f64> {
    let day = day_of(start, zone)?.pred_opt()?;
    zone.from_local_datetime(&day.and_hms_opt(EVE_HOUR, 0, 0)?).earliest().map(|d| d.timestamp() as f64)
}

/// The card for an appointment (`hint`).
pub fn appointment_offer(config: &Config, hint: &Hint, now: f64) -> Offer {
    let lang = lang_of(config, &hint.agent);
    let zone = person_zone(config);
    let title = clean(&hint.title, 40);
    let label = fill(
        texts(lang).calendar_offer,
        &[("title", &title), ("day", &day_words(lang, hint.start, zone)), ("time", &clock(hint.start, zone))],
    );
    let place = clean(&hint.place, 120);
    let detail = appointment_detail(lang, &title, &place, hint.start, zone, now);
    let mut offer = bridge_offer(&hint.agent, APPOINTMENT, CALENDAR, label, clean(&title, offers::ABOUT_MAX), detail, (now, hint.start - 2.0 * 3600.0));
    let event = hint.event.clone().unwrap_or_default();
    offer.params = Some(json!({"event": {"source": event.source, "id": event.id, "calendar": event.calendar,
                                         "start": hint.start}, "title": title, "place": place}));
    offer.subject = hint.key.clone();
    offer
}

/// The card for a trip (`hint`).
pub fn trip_offer(config: &Config, hint: &Hint, now: f64) -> Offer {
    let lang = lang_of(config, &hint.agent);
    let zone = person_zone(config);
    let (start, end) = (day_of(hint.start, zone).unwrap_or_default(), day_of(hint.end, zone).unwrap_or_default());
    let city = clean(&hint.title, 40);
    let label = fill(texts(lang).trip_offer, &[("city", &city), ("dates", &dates_words(lang, start, end.max(start)))]);
    let mut offer =
        bridge_offer(&hint.agent, TRIP, TRIP, label, clean(&city, offers::ABOUT_MAX), texts(lang).trip_prompt.into(), (now, hint.start));
    offer.params = Some(json!({"page": hint.place, "city": city, "start": start.to_string(), "end": end.max(start).to_string()}));
    offer.subject = hint.key.clone();
    offer
}

/// The consent card of a mailbox (`address`) for `agent`.
pub fn consent_offer(config: &Config, agent: &str, account: &str, address: &str, now: f64) -> Offer {
    let lang = lang_of(config, agent);
    let t = texts(lang);
    let mut offer = bridge_offer(
        agent,
        REPLY_WATCH,
        REPLIES,
        fill(t.consent, &[("mailbox", address)]),
        address.to_string(),
        t.consent_under_i.into(),
        (now, now + offers::EXPIRES_SECONDS),
    );
    offer.params = Some(json!({"account": account}));
    offer.mailbox = address.to_string();
    offer.subject = format!("replies:{account}:consent:{}", now as i64);
    offer
}

/// A replies card of `agent` for `mails` (due, the longest waiting first), its headers only.
pub fn replies_offer(config: &Config, agent: &str, account: &str, address: &str, mails: &[Mail], now: f64) -> Offer {
    let lang = lang_of(config, agent);
    let t = texts(lang);
    let shown = shown_agent(config, agent);
    let n = mails.len();
    let top = if n == 1 {
        fill(t.replies_top_one, &[("agent", &shown)])
    } else {
        fill(t.replies_top, &[("agent", &shown), ("n", &n.to_string())])
    };
    let items: Vec<Value> = mails
        .iter()
        .map(|m| {
            let days = (((now - m.seen) / DAY).floor() as u32).max(1);
            json!({"from": m.from, "subject": m.subject, "since": since_words(lang, days)})
        })
        .collect();
    let mut lines: Vec<String> = items
        .iter()
        .map(|i| {
            let s = |k: &str| i[k].as_str().unwrap_or("").to_string();
            fill(t.replies_line, &[("from", &s("from")), ("subject", &s("subject")), ("since", &s("since"))])
        })
        .collect();
    lines.push(t.replies_under_i.to_string());
    let mut offer =
        bridge_offer(agent, DRAFT, REPLIES, t.replies_question.into(), t.replies_about.into(), lines.join("\n"), (now, now + offers::EXPIRES_SECONDS));
    offer.items = items;
    offer.top = top;
    offer.mailbox = address.to_string();
    offer.subject = format!("replies:{account}:{}", now as i64);
    let mails: Vec<Value> = mails.iter().map(|m| json!({"uid": m.uid, "message_id": m.message_id})).collect();
    offer.params = Some(json!({"account": account, "mails": mails}));
    offer
}

/// The push words of the bridge's own offer card (None: the offers' own).
pub fn notification(lang: &str, offer: &Offer) -> Option<String> {
    (offer.source == REPLIES && offer.kind == DRAFT).then(|| {
        let t = texts(lang);
        if offer.items.len() == 1 {
            t.replies_notification_one.to_string()
        } else {
            fill(t.replies_notification, &[("n", &offer.items.len().to_string())])
        }
    })
}

// -- the looks -------------------------------------------------------------------------------------------------------------------

/// Whether the look `key` is due now (`at`: its time of day; None, every tick), once a day; marked done.
fn due_look(config: &Config, key: &str, at: Option<(u32, u32)>, zone: Tz, now: f64) -> bool {
    let Some((h, m)) = at else { return true };
    let Some(local) = zone.timestamp_opt(now as i64, 0).single() else { return false };
    if (local.hour(), local.minute()) < (h, m) {
        return false;
    }
    let today = local.date_naive().to_string();
    change(config, |s| {
        if s.done.get(key) == Some(&today) {
            false
        } else {
            s.done.insert(key.to_string(), today);
            true
        }
    })
}

/// The morning look: the calendars and the trips (new hints), the hints' cards due today, the consent cards.
pub async fn morning(state: &AppState) {
    let config = state.config();
    let now = py::time_time();
    let zone = person_zone(&config);
    let Some(today) = day_of(now, zone) else { return };
    let plans = super::load(&config);
    let mut new: Vec<Hint> = Vec::new();
    if let Some(agent) = calendar_agent(&config) {
        let (events, _) = events_of(&config, &agent, today, today + chrono::Duration::days(LOOK_DAYS)).await;
        for f in upcoming(&events, zone, today).into_iter().filter(|f| !planned(&plans, f)).take(HINTS_PER_AGENT) {
            new.push(Hint {
                agent: agent.clone(),
                source: CALENDAR.into(),
                key: format!("calendar:{}:{}", f.source, f.id),
                start: f.start,
                end: 0.0,
                title: f.title.clone(),
                place: f.place.clone(),
                event: Some(EventRef { source: f.source.clone(), id: f.id.clone(), start: f.start, calendar: f.calendar.clone() }),
                told: false,
                carded: false,
                at: now,
            });
        }
    }
    if let Some(agent) = trip_agent(&config) {
        for t in trips(&config, today).into_iter().filter(|t| !trip_planned(&plans, t, zone)).take(HINTS_PER_AGENT) {
            new.push(Hint {
                agent: agent.clone(),
                source: TRIP.into(),
                key: format!("trip:{}:{}", t.path, t.start),
                start: midnight(t.start, zone),
                end: midnight(t.end, zone),
                title: t.city.clone(),
                place: t.path.clone(),
                event: None,
                told: false,
                carded: false,
                at: now,
            });
        }
    }
    let due: Vec<Hint> = change(&config, |s| {
        // an agent that no longer may: its hints go; one planned since (its agent proposed it meanwhile) too
        s.hints.retain(|h| may(&config, &h.agent) && !hint_planned(&plans, h, zone));
        for h in new {
            if !s.hints.iter().any(|o| o.key == h.key) {
                s.hints.push(h);
            }
        }
        s.hints.iter().filter(|h| card_due(h, today, zone, now)).cloned().collect()
    });
    for hint in due {
        card_for(state, &hint, now).await;
    }
    consent_cards(state, now).await;
}

/// The card of a hint not told by its day: placed when the caps allow (an appointment's only with the person's home
/// known and the streets on: its card offers the route).
async fn card_for(state: &AppState, hint: &Hint, now: f64) {
    let config = state.config();
    let Some(agent) = config.agent(Some(&hint.agent)).cloned() else { return };
    let offer = if hint.source == TRIP {
        trip_offer(&config, hint, now)
    } else {
        let home = super::home_address(state, &config, &agent.name).is_some();
        if !home || config.maps.streets == "off" {
            tracing::info!(target: "bipbridge::plans", agent = %agent.name, "an appointment's card not made: no home or no streets");
            change(&config, |s| s.hints.iter_mut().filter(|h| h.key == hint.key).for_each(|h| h.carded = true));
            return;
        }
        appointment_offer(&config, hint, now)
    };
    match offers::place_bridge(state, &agent, offer).await {
        Ok(id) => {
            tracing::info!(target: "bipbridge::plans", agent = %agent.name, offer = %id, source = %hint.source, "a source's card placed");
            change(&config, |s| {
                s.hints.iter_mut().filter(|h| h.key == hint.key).for_each(|h| {
                    h.carded = true;
                    (h.title, h.place) = (String::new(), String::new());
                })
            });
        }
        Err((why, held)) => {
            tracing::info!(target: "bipbridge::plans", agent = %agent.name, why = %why, held, "a source's card not placed");
            if !held {
                change(&config, |s| s.hints.iter_mut().filter(|h| h.key == hint.key).for_each(|h| h.carded = true));
            }
        }
    }
}

/// Whether a mailbox's consent card is asked now: never asked; asked, unanswered (`pending`: its card still waits),
/// 14 days ago and fewer than `ASKS_MAX` times; never once declined, on or off.
pub fn ask_consent(known: Option<&Mailbox>, pending: bool, now: f64) -> bool {
    match known {
        None => true,
        Some(m) if m.state == ASKED => m.asks < ASKS_MAX && now - m.asked_at >= ASK_AGAIN_DAYS * DAY && !pending,
        Some(_) => false,
    }
}

/// The consent cards: once per mailbox (once more after 14 days without an answer), for its agent that may propose.
async fn consent_cards(state: &AppState, now: f64) {
    let config = state.config();
    let Ok(accounts) = crate::mailbox::of(&config).read() else { return };
    let store = load(&config);
    let waiting = offers::load(&config);
    for account in &accounts {
        let id = tk::str_or_empty(account.get("id"));
        let Some(agent) = mailbox_agent(&config, account) else { continue };
        let known = store.mailboxes.iter().find(|m| m.account == id);
        let pending = known.is_some_and(|m| waiting.offers.iter().any(|o| o.kind == REPLY_WATCH && o.mailbox == m.address));
        if !ask_consent(known, pending, now) {
            continue;
        }
        let Some(a) = config.agent(Some(&agent)).cloned() else { continue };
        let address = tk::str_or_empty(account.get("username"));
        let offer = consent_offer(&config, &agent, &id, &address, now);
        match offers::place_bridge(state, &a, offer).await {
            Ok(_) => change(&config, |s| {
                let asks = s.mailboxes.iter().find(|m| m.account == id).map(|m| m.asks).unwrap_or(0) + 1;
                s.mailboxes.retain(|m| m.account != id);
                s.mailboxes.push(Mailbox {
                    account: id.clone(),
                    agent: agent.clone(),
                    address: address.clone(),
                    state: ASKED.into(),
                    asks,
                    asked_at: now,
                    trigger: None,
                    ends: 0.0,
                });
            }),
            Err((why, _)) => {
                tracing::info!(target: "bipbridge::plans", agent = %agent, why = %why, "a mailbox's consent card not placed")
            }
        }
    }
}

// -- the replies-due watch -------------------------------------------------------------------------------------------------------

/// Its mailbox's replies-due watch on: a trigger of the bridge (triggers.rs `REPLIES`, kind mail, its headers only),
/// on at once, ending in 90 days. Its id.
pub fn watch_on(config: &Config, agent: &str, account: &str) -> Result<String, String> {
    let accounts = crate::mailbox::of(config).read().map_err(|e| e.to_string())?;
    let found = accounts.iter().find(|a| tk::str_or_empty(a.get("id")) == account).ok_or("no such mailbox")?;
    let address = tk::str_or_empty(found.get("username"));
    let existing = crate::triggers::load(config)
        .triggers
        .into_iter()
        .find(|t| t.by == crate::triggers::REPLIES && t.mail.as_ref().is_some_and(|m| m.account == account));
    // « from its newest message: nothing older is ever read » (the contract): a mailbox nothing watches now keeps the
    // cursor an earlier watch left (an e-mail routine, a plan's watch, weeks ago): forgotten, so the watch starts at the
    // « Oui » (review C5)
    if !crate::triggers_mail::needed(config).contains_key(account) {
        crate::triggers_mail::forget_cursor(config, account);
    }
    let lang = lang_of(config, agent);
    let t = texts(lang);
    let trigger = match existing {
        Some(t) => t,
        None => {
            let fields = crate::triggers::Fields {
                name: Some(fill(t.watch_name, &[("mailbox", &address)]).chars().take(crate::triggers::NAME_MAX).collect()),
                prompt: Some(t.watch_prompt.to_string()),
                kind: Some(crate::triggers::MAIL.into()),
                mail: Some(crate::triggers::MailFilter { account: account.to_string(), from: String::new(), words: String::new() }),
                enabled: None,
                budget_tokens: Some(crate::triggers::BUDGET_MIN),
            };
            crate::triggers::create(config, agent, &fields, crate::triggers::REPLIES)?
        }
    };
    let now = py::time_time();
    let ends = trigger.ends.unwrap_or(now + crate::task_frame::DEFAULT_END_DAYS * DAY);
    change(config, |s| {
        let asks = s.mailboxes.iter().find(|m| m.account == account).map(|m| m.asks).unwrap_or(0);
        s.mailboxes.retain(|m| m.account != account);
        s.mailboxes.push(Mailbox {
            account: account.to_string(),
            agent: agent.to_string(),
            address: address.clone(),
            state: ON.into(),
            asks,
            asked_at: now,
            trigger: Some(trigger.id.clone()),
            ends,
        });
    });
    tracing::info!(target: "bipbridge::plans", agent, trigger = %trigger.id, "replies due watched");
    Ok(trigger.id)
}

/// Its watch off (`state`: `off`, or `declined`): the trigger, every header of the mailbox and its replies card still
/// waiting go.
pub fn watch_off(config: &Config, account: &str, state: &str) {
    let known = load(config).mailboxes.into_iter().find(|m| m.account == account);
    if let Some(t) = known.as_ref().and_then(|m| m.trigger.clone()) {
        crate::triggers::remove(config, &t);
    }
    // its replies card still waiting goes too (its e-mails' headers with it)
    if let Some(address) = known.map(|m| m.address).filter(|a| !a.is_empty()) {
        if offers::load(config).offers.iter().any(|o| o.source == REPLIES && o.kind == DRAFT && o.mailbox == address) {
            offers::change(config, |s| s.offers.retain(|o| !(o.source == REPLIES && o.kind == DRAFT && o.mailbox == address)));
        }
    }
    change(config, |s| {
        s.mails.retain(|m| m.account != account);
        match s.mailboxes.iter_mut().find(|m| m.account == account) {
            Some(m) => {
                m.state = state.to_string();
                m.trigger = None;
                m.ends = 0.0;
            }
            None => s.mailboxes.push(Mailbox { account: account.into(), state: state.into(), ..Mailbox::default() }),
        }
    });
}

static ROBOT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)no-?reply|ne-?pas-?repondre|donotreply|do-not-reply|notifications?@|newsletters?@|mailer-daemon|postmaster@|bounces?@")
        .expect("valid regex")
});

/// The sender as a card shows it: its name, else its address.
pub fn sender_shown(from: &str) -> String {
    let name = match from.rfind('<') {
        Some(i) => from[..i].trim().trim_matches('"').trim(),
        None => "",
    };
    let shown = if name.is_empty() { crate::triggers::address_of(from).unwrap_or_else(|| from.trim().to_string()) } else { name.to_string() };
    clean(&shown, 60)
}

/// Whether a new message is one a reply may be due to: not a mass mailing (`bulk`: its list headers), not from a
/// robot, not the person's own (`own`: the mailbox's address).
pub fn worth(event: &crate::triggers::Event, own: &str) -> bool {
    let address = crate::triggers::address_of(&event.from).unwrap_or_default();
    !event.bulk
        && !address.is_empty()
        && !ROBOT.is_match(&address)
        && address != own.trim().to_lowercase()
        && !event.message_id.trim().is_empty()
}

/// A new message of a mailbox watched for its replies due (triggers_mail.rs `offer`): its headers kept when it may
/// need one. True: kept.
pub fn seen_mail(config: &Config, trigger: &crate::triggers::Trigger, event: &crate::triggers::Event) -> bool {
    let Some(account) = trigger.mail.as_ref().map(|m| m.account.clone()) else { return false };
    if !may(config, &trigger.agent) {
        return false;
    }
    // a mailbox removed, or no longer the agent's: nothing kept (review C1; `reconcile` stops its watch)
    let Some(own) = crate::mailbox::of(config)
        .of_agent(&trigger.agent)
        .ok()
        .and_then(|l| l.into_iter().find(|a| tk::str_or_empty(a.get("id")) == account))
        .map(|a| tk::str_or_empty(a.get("username")))
    else {
        return false;
    };
    if !worth(event, &own) {
        return false;
    }
    let uid: u64 = event.mail_ref.rsplit('|').next().and_then(|u| u.parse().ok()).unwrap_or(0);
    let mail = Mail {
        account: account.clone(),
        uid,
        message_id: event.message_id.trim().to_string(),
        from: sender_shown(&event.from),
        subject: clean(&event.subject, 80),
        seen: py::time_time(),
        state: WAITING.into(),
        done_at: 0.0,
    };
    change(config, |s| {
        let on = s.mailboxes.iter().any(|m| m.account == account && m.state == ON);
        if !on || s.mails.iter().any(|m| m.account == account && m.message_id == mail.message_id) {
            return false;
        }
        s.mails.push(mail);
        true
    })
}

/// The e-mails of `account` that may be due now (seen more than `due_hours` ago, not dealt with), the longest waiting
/// first.
pub fn candidates(store: &Store, account: &str, due_hours: f64, now: f64) -> Vec<Mail> {
    let mut out: Vec<Mail> = store
        .mails
        .iter()
        .filter(|m| m.account == account && (m.state == WAITING || m.state == SHOWN) && now - m.seen >= due_hours * 3600.0)
        .cloned()
        .collect();
    out.sort_by(|a, b| a.seen.total_cmp(&b.seen));
    out
}

fn numbers(data: &[crate::tools_knowledge::imap::Data]) -> Vec<u64> {
    use crate::tools_knowledge::imap::Data;
    let mut out = Vec::new();
    for item in data {
        if let Data::Line(l) | Data::Literal(l, _) = item {
            out.extend(String::from_utf8_lossy(l).split_whitespace().filter_map(|w| w.parse::<u64>().ok()));
        }
    }
    out
}

/// The Sent folder: its special-use flag, else its usual names.
fn sent_folder(rows: &[(String, String)]) -> Option<String> {
    if let Some((_, name)) = rows.iter().find(|(flags, _)| flags.to_lowercase().contains("\\sent")) {
        return Some(name.clone());
    }
    let usual = ["sent", "sent items", "sent mail", "sent messages", "envoyés", "éléments envoyés", "gesendet", "enviados", "inbox.sent"];
    rows.iter().find(|(_, n)| usual.contains(&n.to_lowercase().as_str())).map(|(_, n)| n.clone())
}

/// Whether a Message-ID can be asked of the Sent folder: printable ASCII only. Anyone may send another one (`<zoé.1@…>`,
/// a tab inside), which an IMAP argument can't carry (review C4): that e-mail is judged by its flags only.
fn searchable(message_id: &str) -> bool {
    !message_id.is_empty() && message_id.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// What the evening look found of its e-mails (`dealt_with`).
#[derive(Debug, Default, PartialEq)]
pub struct Dealt {
    /// The Message-IDs dealt with already.
    pub done: Vec<String>,
    /// Those whose Sent search failed: not due tonight, looked at again the next evening (never the others' fault).
    pub unchecked: Vec<String>,
}

/// Of `mails`, the Message-IDs dealt with already: answered (a message of Sent cites it: In-Reply-To or References;
/// flagged \Answered) or gone from the inbox. Err: the mailbox or its Sent folder couldn't be read (nothing is due).
/// One e-mail's search failing sets that e-mail aside only (`unchecked`, review C4).
pub fn dealt_with(conn: &mut crate::mailbox::Connection, mails: &[Mail]) -> Result<Dealt, String> {
    use crate::tools_knowledge::imap::Data;
    let mut done = Vec::new();
    let (status, _) = conn.select(&crate::mailbox::quote_box(crate::triggers_mail::FOLDER), true).map_err(|e| e.to_string())?;
    if status != "OK" {
        return Err("no inbox".into());
    }
    let uids: Vec<String> = mails.iter().map(|m| m.uid.to_string()).collect();
    let (status, data) = conn.uid("FETCH", &[&uids.join(","), "(UID FLAGS)"]).map_err(|e| e.to_string())?;
    if status != "OK" {
        return Err("the inbox's flags were not read".into());
    }
    let uid_re = regex::Regex::new(r"UID (\d+)").expect("valid regex");
    let mut present: HashMap<u64, bool> = HashMap::new();
    for item in &data {
        if let Data::Line(l) | Data::Literal(l, _) = item {
            let line = String::from_utf8_lossy(l);
            if let Some(c) = uid_re.captures(&line) {
                present.insert(c[1].parse().unwrap_or(0), line.contains("\\Answered"));
            }
        }
    }
    for m in mails {
        match present.get(&m.uid) {
            None | Some(true) => done.push(m.message_id.clone()),
            Some(false) => {}
        }
    }
    let (status, rows) = conn.list().map_err(|e| e.to_string())?;
    let rows = if status == "OK" { crate::mailbox::list_rows(&rows) } else { Vec::new() };
    let sent = sent_folder(&rows).ok_or("no Sent folder")?;
    let (status, _) = conn.select(&crate::mailbox::quote_box(&sent), true).map_err(|e| e.to_string())?;
    if status != "OK" {
        return Err("the Sent folder could not be opened".into());
    }
    let open: Vec<&Mail> = mails.iter().filter(|m| !done.contains(&m.message_id) && searchable(&m.message_id)).collect();
    let mut unchecked = Vec::new();
    for m in open {
        let id = crate::mailbox::quote(&m.message_id);
        for header in ["In-Reply-To", "References"] {
            match conn.uid("SEARCH", &["HEADER", header, &id]) {
                Ok((status, data)) if status == "OK" && !numbers(&data).is_empty() => {
                    done.push(m.message_id.clone());
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::info!(target: "bipbridge::plans", why = %e, "one reply due not checked");
                    unchecked.push(m.message_id.clone());
                    break;
                }
            }
        }
    }
    Ok(Dealt { done, unchecked })
}

/// The evening look of each mailbox watched: its e-mails due, dealt with ones set aside (IMAP, headers only), then
/// ONE card when some are new.
pub async fn evening(state: &AppState) {
    let config = state.config();
    let now = py::time_time();
    let zone = person_zone(&config);
    let settings = config.plans_sources.clone();
    let store = load(&config);
    let triggers = crate::triggers::load(&config).triggers;
    for mailbox in store.mailboxes.iter().filter(|m| m.state == ON) {
        let enabled = mailbox.trigger.as_ref().is_some_and(|id| triggers.iter().any(|t| &t.id == id && t.enabled));
        if !enabled || !may(&config, &mailbox.agent) {
            continue;
        }
        let list = candidates(&store, &mailbox.account, settings.due_hours, now);
        if !list.iter().any(|m| m.state == WAITING) {
            continue;
        }
        if !due_look(&config, &format!("evening:{}", mailbox.account), settings.evening, zone, now) {
            continue;
        }
        let waiting = offers::load(&config);
        if waiting.offers.iter().any(|o| o.source == REPLIES && o.kind == DRAFT && o.mailbox == mailbox.address) {
            continue;
        }
        let Ok(accounts) = crate::mailbox::of(&config).read() else { continue };
        let Some(account) = accounts.into_iter().find(|a| tk::str_or_empty(a.get("id")) == mailbox.account) else { continue };
        let checked = list.clone();
        let dealt = tokio::task::spawn_blocking(move || {
            let mut conn = crate::mailbox::connect(&account).map_err(|e| e.0)?;
            let out = dealt_with(&mut conn, &checked);
            let _ = conn.logout();
            out
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let Dealt { done: dealt, unchecked } = match dealt {
            Ok(d) => d,
            Err(why) => {
                tracing::info!(target: "bipbridge::plans", account = %mailbox.account, why = %why, "replies due not checked");
                continue;
            }
        };
        change(&config, |s| {
            for m in s.mails.iter_mut().filter(|m| m.account == mailbox.account && dealt.contains(&m.message_id)) {
                m.state = ANSWERED.into();
                m.done_at = now;
            }
        });
        let due: Vec<Mail> = list
            .into_iter()
            .filter(|m| !dealt.contains(&m.message_id) && !unchecked.contains(&m.message_id))
            .take(ITEMS_MAX)
            .collect();
        if !due.iter().any(|m| m.state == WAITING) {
            continue;
        }
        let Some(agent) = config.agent(Some(&mailbox.agent)).cloned() else { continue };
        let offer = replies_offer(&config, &agent.name, &mailbox.account, &mailbox.address, &due, now);
        match offers::place_bridge(state, &agent, offer).await {
            Ok(id) => {
                let ids: Vec<String> = due.iter().map(|m| m.message_id.clone()).collect();
                change(&config, |s| {
                    for m in s.mails.iter_mut().filter(|m| m.account == mailbox.account && ids.contains(&m.message_id)) {
                        m.state = SHOWN.into();
                    }
                });
                tracing::info!(target: "bipbridge::plans", agent = %agent.name, offer = %id, n = due.len(), "replies due card placed");
            }
            Err((why, _)) => tracing::info!(target: "bipbridge::plans", agent = %agent.name, why = %why, "replies due card held"),
        }
    }
}

// -- the answers ------------------------------------------------------------------------------------------------------------------

/// One of the bridge's own cards answered (plans/offers.rs `approve`): a replies card's e-mails drafted or declined, a
/// mailbox declined.
pub fn answered(config: &Config, offer: &Offer, yes: bool) {
    let now = py::time_time();
    let account = offer.params.as_ref().and_then(|p| p.get("account")).and_then(Value::as_str).unwrap_or("").to_string();
    match (offer.source.as_str(), offer.kind.as_str()) {
        (REPLIES, DRAFT) => {
            let ids: Vec<String> = offer
                .params
                .as_ref()
                .and_then(|p| p.get("mails"))
                .and_then(Value::as_array)
                .map(|l| l.iter().filter_map(|m| m.get("message_id").and_then(Value::as_str).map(str::to_string)).collect())
                .unwrap_or_default();
            change(config, |s| {
                for m in s.mails.iter_mut().filter(|m| m.account == account && ids.contains(&m.message_id)) {
                    m.state = if yes { DRAFTED.into() } else { "declined".into() };
                    m.done_at = now;
                }
            });
        }
        (REPLIES, REPLY_WATCH) if !yes => watch_off(config, &account, DECLINED),
        _ => {}
    }
}

/// A trip's « Oui »: the plan the agent proposes within `GRANT_SECONDS` is the person's ask.
pub fn grant(config: &Config, agent: &str, now: f64) {
    change(config, |s| s.grants.push(Grant { agent: agent.to_string(), until: now + GRANT_SECONDS }));
}

/// Whether `agent` holds a grant now.
pub fn granted(config: &Config, agent: &str, now: f64) -> bool {
    path(config).exists() && load(config).grants.iter().any(|g| g.agent == agent && g.until > now)
}

/// The grant used (one plan).
pub fn use_grant(config: &Config, agent: &str) {
    change(config, |s| {
        if let Some(i) = s.grants.iter().position(|g| g.agent == agent) {
            s.grants.remove(i);
        }
    });
}

/// What the agent reads at a trip's or a replies card's « Oui » (plans/offers.rs `yes_note`; None: the offers' own).
pub fn yes_note(_config: &Config, offer: &Offer) -> Option<String> {
    let head = format!(
        "<!--bip:offer\nNot from the person's words: they answered « Oui » in the app to your offer « {} ».\n",
        tk::in_note_line(&offer.label)
    );
    match (offer.source.as_str(), offer.kind.as_str()) {
        (TRIP, TRIP) => {
            let p = offer.params.clone().unwrap_or(Value::Null);
            let s = |k: &str| tk::in_note_line(p.get(k).and_then(Value::as_str).unwrap_or(""));
            Some(format!(
                "{head}Your person said yes to your offer: « {} ». Do exactly that now: read their trip in the Carnet \
(its page « {} », {} to {}) and propose ONE plan for it with plan_propose (its card asks them « Prévoir »: nothing runs \
before). Never book, pay or send anything. Then say in one short sentence that the plan waits for their answer.\n-->",
                tk::in_note_line(&offer.prompt),
                s("page"),
                s("start"),
                s("end"),
            ))
        }
        (REPLIES, DRAFT) => {
            let p = offer.params.clone().unwrap_or(Value::Null);
            let account = p.get("account").and_then(Value::as_str).unwrap_or("").to_string();
            let mails = p.get("mails").and_then(Value::as_array).cloned().unwrap_or_default();
            let blocks: Vec<String> = mails
                .iter()
                .zip(offer.items.iter())
                .enumerate()
                .map(|(i, (m, item))| {
                    let uid = m.get("uid").and_then(Value::as_u64).unwrap_or(0);
                    format!(
                        "=== e-mail {n} ===\nid: {account}|{}|{uid}\nFrom: {}\nSubject: {}\n=== end {n} ===",
                        crate::triggers_mail::FOLDER,
                        tk::in_note_line(item.get("from").and_then(Value::as_str).unwrap_or("")),
                        tk::in_note_line(item.get("subject").and_then(Value::as_str).unwrap_or("")),
                        n = i + 1,
                    )
                })
                .collect();
            Some(format!(
                "{head}Your person said yes: draft a reply to each of these e-mails, and only these. Read each one with \
mail_read (its id), then write ONE draft with mail_draft (reply_to_id: its id), short, in your person's language and \
voice; when you can't tell what to answer, a short draft that says so. Never send anything: the drafts wait in their \
Drafts folder (there is no tool to send). Nothing else in this turn: no search, no other e-mail. Then tell them in one \
short line which drafts are ready.\n{}\n{}\n-->",
                crate::mailbox::UNTRUSTED,
                blocks.join("\n")
            ))
        }
        _ => None,
    }
}

// -- the turn's gate (a replies card's « Oui ») ---------------------------------------------------------------------------------

/// The runs of a replies card's « Oui »: (agent, run) → the e-mails' ids it may open and answer.
type Gates = HashMap<(String, String), Vec<String>>;
static GATES: Mutex<Option<Gates>> = Mutex::new(None);

/// The e-mails' ids a replies card's « Oui » may open (`<account>|INBOX|<uid>`).
pub fn mail_ids(offer: &Offer) -> Vec<String> {
    let p = offer.params.clone().unwrap_or(Value::Null);
    let account = p.get("account").and_then(Value::as_str).unwrap_or("").to_string();
    p.get("mails")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .map(|m| format!("{account}|{}|{}", crate::triggers_mail::FOLDER, m.get("uid").and_then(Value::as_u64).unwrap_or(0)))
                .collect()
        })
        .unwrap_or_default()
}

/// The run `run` of `agent` opens only `ids` (`gate`); None closes it.
pub fn set_gate(agent: &str, run: &str, ids: Option<Vec<String>>) {
    let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    let gates = gates.get_or_insert_with(HashMap::new);
    let key = (agent.to_string(), run.to_string());
    match ids {
        Some(ids) => {
            gates.insert(key, ids);
        }
        None => {
            gates.remove(&key);
        }
    }
}

/// In a replies card's « Oui » turn (`run`), a mail tool's refusal: `mail_read` opens those e-mails only, `mail_draft`
/// answers those only, `mail_search` and `mail_attachment` are closed. None: allowed (or not such a turn).
/// A call whose run the hub can't tell (`run` None: a sub-agent's session, a Hermes without patch 0028, the bridge
/// restarted mid-turn) while one of the agent's replies turns is gated: closed the same way, its e-mails only (review
/// C3: in doubt, closed).
pub fn gate(agent: &str, run: Option<&str>, tool: &str, arguments: &Dict) -> Option<String> {
    let gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    let gates = gates.as_ref()?;
    let ids: Vec<String> = match run {
        Some(run) => gates.get(&(agent.to_string(), run.to_string()))?.clone(),
        None => gates.iter().filter(|((a, _), _)| a == agent).flat_map(|(_, ids)| ids.iter().cloned()).collect(),
    };
    if ids.is_empty() {
        return None;
    }
    refusal(&ids, tool, arguments)
}

/// A call whose run the hub can't tell, after a restart (`GATES` lost, its turn still going on in Hermes): the replies
/// « Oui » the offers still do for `agent` (plans/offers.rs `doing`, on disk) close its mail tools the same way.
pub fn gate_unknown_run(config: &Config, agent: &str, tool: &str, arguments: &Dict) -> Option<String> {
    let doing = offers::replies_doing(config, agent);
    if doing.is_empty() {
        return None;
    }
    let ids: Vec<String> = doing.iter().flat_map(mail_ids).collect();
    refusal(&ids, tool, arguments)
}

fn refusal(ids: &[String], tool: &str, arguments: &Dict) -> Option<String> {
    let arg = |k: &str| arguments.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let listed = |id: &str| ids.iter().any(|i| i == id);
    match tool {
        "mail_read" if listed(&arg("id")) => None,
        "mail_draft" if listed(&arg("reply_to_id")) => None,
        "mail_read" | "mail_draft" => Some(format!(
            "Not done: in this turn you only open and answer the e-mails your person said yes to ({}): a draft replying \
             to one of them (reply_to_id), nothing else.",
            ids.join(", ")
        )),
        _ => Some(
            "Not done: in this turn you only read the e-mails your person said yes to (mail_read with their ids) and \
             write their drafts (mail_draft): no search, no attachment."
                .into(),
        ),
    }
}

/// The tools a replies card's « Oui » turn may not run (Hermes patch 0024 `closed_tools`, refused in its dispatcher
/// whatever the bridge knows of the turn: a sub-agent's call, a restart; review C3): every mail tool but `mail_read`
/// and `mail_draft`, and what could carry an e-mail's words elsewhere or let another agent read them (the web, the
/// browsers, commands, code, sub-agents). Its note asks for nothing else.
pub const REPLIES_CLOSED_TOOLS: [&str; 14] = [
    "browser_*",
    "computer_use",
    "delegate_task",
    "execute_code",
    "mcp__bipagents__ask_agent",
    "mcp__bipagents__mail_attachment",
    "mcp__bipagents__mail_bulk",
    "mcp__bipagents__mail_overview",
    "mcp__bipagents__mail_search",
    "mcp__bipagents__start_task",
    "mcp__stealth__*",
    "terminal",
    "web_extract",
    "web_search",
];

// -- an appointment's « Oui » -------------------------------------------------------------------------------------------------------

/// An appointment card's « Oui »: its plan set up by the bridge, no model, no second card (that card was its yes): the
/// reminder the day before at 19:00 when still to come, when to leave (on foot up to 30 min, else by transit:
/// the place and the home to the geocoder, then the router, only now), following its event.
pub async fn appointment_yes(state: &AppState, agent: &AgentConfig, offer: &Offer) {
    let config = state.config();
    let now = py::time_time();
    let zone = person_zone(&config);
    let lang = lang_of(&config, &agent.name);
    let p = offer.params.clone().unwrap_or(Value::Null);
    let e = p.get("event").cloned().unwrap_or(Value::Null);
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let start = e.get("start").and_then(Value::as_f64).unwrap_or(0.0);
    let (title, place) = (s(&p, "title"), s(&p, "place"));
    let event = EventRef { source: s(&e, "source"), id: s(&e, "id"), start, calendar: s(&e, "calendar") };
    let t = texts(lang);
    let mut moments = Vec::new();
    if let Some(eve) = eve_of(start, zone).filter(|eve| *eve > now + 60.0) {
        moments.push(Moment {
            id: "m1".into(),
            at: eve,
            zone: zone.name().to_string(),
            what: t.calendar_reminder_what.into(),
            say: fill(t.calendar_reminder, &[("title", &title), ("time", &clock(start, zone))]),
            urgent: false,
            route: false,
            leg: None,
            offer: None,
            routine: None,
        });
    }
    let home = super::home_address(state, &config, &agent.name);
    let mut leg = super::Leg { place: place.clone(), from: None, mode: "walking".into(), arrive: start };
    let mut minutes = super::leg_minutes(&config, home.as_deref(), &leg).await;
    if matches!(minutes, Ok(m) if m > WALK_MAX_MINUTES) {
        leg.mode = "transit".into();
        minutes = super::leg_minutes(&config, home.as_deref(), &leg).await;
    }
    let mut failed = false;
    match minutes {
        Ok(m) if super::leave_at(start, m) > now => {
            let words = super::route_words(lang, &place, m, &leg.mode);
            moments.push(Moment {
                id: "m2".into(),
                at: super::leave_at(start, m),
                zone: zone.name().to_string(),
                what: words.clone(),
                say: words,
                urgent: true,
                route: true,
                leg: None,
                offer: None,
                routine: None,
            });
        }
        Ok(_) => {}
        Err(why) => {
            tracing::info!(target: "bipbridge::plans", agent = %agent.name, why = %why, "an appointment's way not timed");
            failed = true;
        }
    }
    let plan = Plan {
        id: py::uuid4_hex()[..12].to_string(),
        agent: agent.name.clone(),
        kind: "appointment".into(),
        title: clean(&title, super::TITLE_MAX),
        city: String::new(),
        zone: zone.name().to_string(),
        moments,
        repeat: None,
        watch: Vec::new(),
        event: Some(event),
        state: super::WAITING.into(),
        made: now,
        asked: false,
        first: false,
        origin: String::new(),
        routines: Vec::new(),
        trigger: None,
        ends: 0.0,
    };
    if !plan.moments.is_empty() {
        let (c, name, id) = (config.clone(), agent.name.clone(), plan.id.clone());
        super::change(&config, |s| s.plans.push(plan.clone()));
        let set = tokio::task::spawn_blocking(move || super::answer(&c, &name, &id, true, &[], "", py::time_time())).await;
        tracing::info!(target: "bipbridge::plans", agent = %agent.name, plan = %plan.id, set = set.is_ok(), "an appointment's plan set up by the bridge");
    }
    if failed {
        let told = super::get(&config, &plan.id).unwrap_or_else(|| plan.clone());
        super::tell(state, &told, super::route_failed_line(lang, &place)).await;
    }
}

// -- following Google's and CalDAV's events (`calendar_event`) -----------------------------------------------------------------------

/// The event `id` (`source` google or caldav) in the calendars `agent` reads now, for a plan to follow: found, repeating,
/// or missing (not read: missing).
pub async fn remote_event(config: &Config, agent: &str, id: &str, zone: Tz) -> super::PhoneEvent {
    let now = py::time_time();
    let Some(today) = day_of(now, zone) else { return super::PhoneEvent::Missing };
    let (events, _) = events_of(config, agent, today - chrono::Duration::days(1), today + chrono::Duration::days(60)).await;
    let copy: Dict = [("events".to_string(), Value::Array(events))].into_iter().collect();
    super::phone_event(&copy, id, zone)
}

/// What the calendars say now of a plan's Google or CalDAV event: moved, cancelled (gone while its calendar answered
/// and is still listed), or the same (anything not read changes nothing).
pub async fn remote_news(config: &Config, agent: &str, event: &EventRef, zone: Tz) -> super::EventNews {
    let now = py::time_time();
    let Some(today) = day_of(now, zone) else { return super::EventNews::Same };
    let from = today - chrono::Duration::days(1);
    let to = day_of(event.start.max(now), zone).unwrap_or(today) + chrono::Duration::days(60);
    let (events, remote) = events_of(config, agent, from, to).await;
    let ours: Vec<&Value> = events.iter().filter(|e| text_of(e, "id") == event.id).collect();
    if let Some(start) = ours.iter().find_map(|e| super::event_start(e, zone)) {
        if ours.len() > 1 {
            return super::EventNews::Same;
        }
        return if (start - event.start).abs() >= 60.0 {
            super::EventNews::Moved { start, delta: start - event.start }
        } else {
            super::EventNews::Same
        };
    }
    let listed = remote.calendars.iter().any(|c| text_of(c, "id") == event.calendar);
    if remote.failed.is_empty() && listed && !event.calendar.is_empty() {
        super::EventNews::Cancelled
    } else {
        super::EventNews::Same
    }
}

/// Every `follow_seconds`: the plans' Google and CalDAV events read again (plans.rs `apply_news`).
pub async fn follow(state: &AppState) {
    let config = state.config();
    let now = py::time_time();
    {
        let mut last = LAST_FOLLOW.lock().unwrap_or_else(|e| e.into_inner());
        if now - *last < config.plans_sources.follow_seconds {
            return;
        }
        *last = now;
    }
    let zone = person_zone(&config);
    for plan in super::load(&config).plans {
        let Some(event) = plan.event.clone().filter(|e| e.source == "google" || e.source == "caldav") else { continue };
        let news = remote_news(&config, &plan.agent, &event, zone).await;
        super::apply_news(state, &plan, news, zone).await;
    }
}

// -- the tick ---------------------------------------------------------------------------------------------------------------------

/// The watched mailboxes removed, or taken from their agent (review C1): their watch off at once, its headers and its
/// card gone (mailbox.rs calls it when a mailbox is removed or changed; `reconcile` too). Their accounts. A mailbox
/// list that can't be read decides nothing.
pub fn mailboxes_gone(config: &Config) -> Vec<String> {
    if !path(config).exists() {
        return Vec::new();
    }
    let on: Vec<Mailbox> = load(config).mailboxes.into_iter().filter(|m| m.state == ON).collect();
    if on.is_empty() {
        return Vec::new();
    }
    let Ok(accounts) = crate::mailbox::of(config).read() else { return Vec::new() };
    let mut gone = Vec::new();
    for m in on {
        let kept = accounts.iter().any(|a| {
            tk::str_or_empty(a.get("id")) == m.account && tk::py_in(&m.agent, a.get("agents").unwrap_or(&json!([])))
        });
        if !kept {
            watch_off(config, &m.account, OFF);
            tracing::info!(target: "bipbridge::plans", agent = %m.agent, "replies due watch stopped: its mailbox is gone");
            gone.push(m.account);
        }
    }
    gone
}

/// The watches that stopped (their trigger deleted, paused, ended, the mailbox no longer the agent's): their headers
/// forgotten; an end reached: one Boîte line, the consent card may come again.
async fn reconcile(state: &AppState) {
    let config = state.config();
    let now = py::time_time();
    let store = load(&config);
    if !store.mailboxes.iter().any(|m| m.state == ON) {
        return;
    }
    let triggers = crate::triggers::load(&config).triggers;
    let gone = mailboxes_gone(&config);
    for m in store.mailboxes.iter().filter(|m| m.state == ON && !gone.contains(&m.account)) {
        let trigger = m.trigger.as_ref().and_then(|id| triggers.iter().find(|t| &t.id == id));
        match trigger {
            None => watch_off(&config, &m.account, OFF),
            Some(_) if m.ends > 0.0 && now > m.ends => {
                watch_off(&config, &m.account, OFF);
                change(&config, |s| s.mailboxes.retain(|x| x.account != m.account));
                if let Some(agent) = config.agent(Some(&m.agent)).cloned() {
                    let lang = lang_of(&config, &agent.name);
                    let end = crate::task_frame::end_words(lang, crate::task_frame::Task::Routine, &crate::task_frame::End::Date(m.ends), person_zone(&config));
                    let name = fill(texts(lang).watch_name, &[("mailbox", &m.address)]);
                    let pipeline = Pipeline::of(state);
                    let session = pipeline.tip(&agent, "main").await;
                    let line = crate::task_frame::ended_line(lang, crate::task_frame::Task::Routine, &name, &end);
                    let delivery = Delivery {
                        agent,
                        session,
                        key: format!("replies-end:{}", m.account),
                        title: name,
                        text: line,
                        as_written: false,
                        run_id: None,
                        notify: false,
                        files: Vec::new(),
                    };
                    pipeline.deliver(&delivery).await;
                }
            }
            Some(t) if !t.enabled || !may(&config, &m.agent) => {
                if store.mails.iter().any(|x| x.account == m.account) {
                    change(&config, |s| s.mails.retain(|x| x.account != m.account));
                }
            }
            Some(_) => {}
        }
    }
}

/// One look (plans.rs `watch`, every few seconds; at most every `tick_seconds` here).
pub async fn tick(state: &AppState) {
    let config = state.config();
    let now = py::time_time();
    {
        let mut last = LAST_TICK.lock().unwrap_or_else(|e| e.into_inner());
        if now - *last < config.plans_sources.tick_seconds {
            return;
        }
        *last = now;
    }
    reconcile(state).await;
    let zone = person_zone(&config);
    if due_look(&config, "morning", config.plans_sources.morning, zone, now) {
        morning(state).await;
    }
    evening(state).await;
    follow(state).await;
}

// -- the routes ---------------------------------------------------------------------------------------------------------------------

fn agent_of(config: &Config, id: &str) -> ApiResult<AgentConfig> {
    config.agent(Some(id)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

/// The agent's mailboxes and their replies-due watch.
async fn list_route(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
) -> ApiResult<axum::response::Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let accounts = crate::mailbox::of(&config).of_agent(&agent.name).unwrap_or_default();
    let store = load(&config);
    let now = py::time_time();
    let due_hours = config.plans_sources.due_hours;
    let listed: Vec<Value> = accounts
        .iter()
        .map(|a| {
            let id = tk::str_or_empty(a.get("id"));
            let known = store.mailboxes.iter().find(|m| m.account == id);
            json!({"account": id, "mailbox": tk::str_or_empty(a.get("username")),
                   "state": known.map(|m| m.state.clone()).unwrap_or_else(|| OFF.into()),
                   "trigger": known.and_then(|m| m.trigger.clone()),
                   "ends": known.map(|m| m.ends).filter(|e| *e > 0.0),
                   "due": candidates(&store, &id, due_hours, now).len()})
        })
        .collect();
    Ok(web::ok(json!({"mailboxes": listed})))
}

#[derive(Debug, Default, serde::Deserialize)]
struct SwitchBody {
    account: Option<String>,
    on: Option<bool>,
}

/// The person's own switch, mailbox by mailbox.
async fn switch_route(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<axum::response::Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let body: SwitchBody = serde_json::from_slice(&body).unwrap_or_default();
    let (Some(account), Some(on)) = (body.account.filter(|a| !a.trim().is_empty()), body.on) else {
        return Err(HttpError::new(422, "account and on"));
    };
    let mine = crate::mailbox::of(&config).of_agent(&agent.name).unwrap_or_default();
    if !mine.iter().any(|a| tk::str_or_empty(a.get("id")) == account) {
        return Err(HttpError::new(404, "no such mailbox for this agent"));
    }
    let (c, name, acc) = (config.clone(), agent.name.clone(), account.clone());
    if on {
        if !may(&config, &agent.name) {
            return Err(HttpError::new(409, "this agent proposes nothing on its own"));
        }
        tokio::task::spawn_blocking(move || watch_on(&c, &name, &acc))
            .await
            .map_err(anyhow::Error::from)?
            .map_err(|why| HttpError::new(409, why))?;
    } else {
        tokio::task::spawn_blocking(move || watch_off(&c, &acc, OFF)).await.map_err(anyhow::Error::from)?;
    }
    Ok(web::ok(json!({"ok": true, "account": account, "state": if on { ON } else { OFF }})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/agents/{agent_id}/replies", axum::routing::get(list_route).put(switch_route))
}

#[cfg(test)]
mod tests;
