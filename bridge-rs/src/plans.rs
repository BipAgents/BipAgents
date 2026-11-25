//! Plans an agent proposes on its own (passation-pro briefs/design-agents-proactifs.md §3, §6 row 5; Alex 22/11: Q1 B,
//! Q3 A, Q4 A, Q9, D2 A; contracts/plans/cases.json). When its person speaks of something to come (a trip, an
//! appointment, a habit they want), the agent calls `plan_propose` in the same turn: ONE card (`bip-plan-<id>`) for the
//! whole plan, its moments one per line, what it watches, « Prévoir » / « Non merci ». Nothing runs before « Prévoir ».
//!
//! - **The moments** are written beforehand (`say`, plain text, links in https only) and become one-time routines on
//!   « Prévoir » (routines.rs `create_said`): at its time the bridge delivers the words as written, one Boîte line,
//!   WITHOUT the model; an urgent one at the system's urgent level (background_turn.rs `deliver_urgent`). A habit is one moment on a rhythm: one routine on a clock per time of day, « vers » that time
//!   (a stable shift of −10 to +10 min from the plan's id, Alex Q7 A), ending after its weeks (its number of runs).
//! - **A route moment** is written by the bridge, and found and timed only once the person says « Prévoir » (reviews 1
//!   and 2, R1, R9: `Leg`, `time_legs`): while the card waits, and after « Non merci », nothing goes to a geocoder or a
//!   router; the card says « avant 15:00 ». At « Prévoir »: the place to go and where from (the agent's `from`, else
//!   the person's home: the « Coordonnées » entry this agent reads that is labelled as home, `home_of`) to Nominatim as
//!   `map_image` finds places (cached in memory); then both points rounded to about 100 m (`ROUTED_DECIMALS`) to
//!   FOSSGIS's OSRM for the time only (map_route.rs `seconds`: no path, nothing on disk); the moment at the arrival
//!   less the way and 5 minutes, urgent. Not timed (the home gone, a place not found, no way): left out, one line
//!   (`route_failed`).
//! - **What it watches** costs nothing while nothing moves: the senders' e-mails are an e-mail trigger on their domains
//!   (triggers.rs, made by the plan: on with « Prévoir », gone with the plan), its headers only (`headers_only`: the
//!   sender's address, the subject, the date; one small turn per e-mail of theirs, §3.8); not made, one line
//!   (`watch_failed`). An event of the phone's calendar copy (calendar.rs) is compared with each copy the phone sends
//!   (`tick`): moved, the moments move by as much, one line; cancelled, the plan goes, one line; no model either way.
//!   A repeating event is never followed (`phone_event`: all its occurrences share one id on both phones); an event
//!   whose calendar left the copy is not cancelled. A Google or CalDAV event is read again every hour by the bridge's
//!   `calendar_event` watch (plans/sources.rs `follow`): the same lines, no model.
//! - **Only from an agent that may propose** (agent_proactive.rs `proposes`): « Initiative » not off
//!   (contracts/proactive), the person's own agent (never one other people talk to); its « Routines » on.
//! - **The caps** (D2 A): one plan per reply in the person's own turn (agent_tools.rs `persons_own_call`, as
//!   « Initiative »'s off switch); unasked (a routine's, a watcher's, a trigger's, a task's turn, another agent's ask,
//!   a relative's turn, or none) one card waiting per agent, 2 a day per agent, 4 a day for the place. Its card shows
//!   where the person sees it (card_place.rs `report`: a hidden conversation's card in the main one), and so do its
//!   moments and lines (B1). « Non merci » forgets the plan and keeps its title's print 14 days: the same plan is
//!   refused.
//! - **The end**: a day after its last moment, the plan, its routines and its trigger go without a word. A card still
//!   waiting when its last moment has passed, or when « Initiative » is turned off, goes without a word; a plan set
//!   goes on whatever the level (the person asked for it).
//! - **Validation**: `plan` is critical (approval_level.rs): asked at every level, pushed, never « Toujours ».
//!
//! Kept here (no title outside `plans`: a declined plan's print only):
//!
//!     <data>/plans.json (0600)   {plans: [...], declined: [{agent, print, at}], made: [{agent, at, asked, run}],
//!                                 hinted: [agent]}
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::background_turn::{Delivery, Pipeline};
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "plan_propose";
/// The plan's card (`/v1/approvals`, `/v1/approve`).
pub const RUN_PREFIX: &str = "bip-plan-";
/// Its notification's key (notifications.rs `Kind::of_card`: a request's neutral line, as a tool asked for; its id the
/// card's run id, so an older app's « Refuser » answers it: contracts/notifications/types/card.json `plan`).
pub const PUSH_KEY: &str = "plan_request";
/// Who made an e-mail trigger of a plan (triggers.rs: on at once, the person said « Prévoir »).
pub const MAKER: &str = "plan";

pub const MAX_MOMENTS: usize = 6;
pub const TITLE_MAX: usize = 60;
pub const CITY_MAX: usize = 40;
pub const WHAT_MAX: usize = 80;
pub const SAY_MAX: usize = 240;
pub const PLACE_MAX: usize = 200;
pub const NAME_MAX: usize = 40;
pub const SENDERS_MAX: usize = 3;
pub const TIMES_MAX: usize = 4;
pub const WEEKS_MAX: u32 = 12;
/// A habit's weeks when its agent gives none (bench 1.1: `weeks` forgotten in most of GLM's habits, the design's
/// example says 2 weeks): it stops on its own (§4.1 rule 2), the card says how long.
pub const DEFAULT_WEEKS: u32 = 2;
/// A habit's rhythm as the tool takes it, added to a refusal of its shape so the model fixes it at its next call.
const REPEAT_EXAMPLE: &str = "e.g. \"repeat\": {\"days\": [\"mon\", \"wed\", \"fri\"], \"times\": [\"18:30\"], \"weeks\": 4} \
beside `moments`, and one moment {\"what\", \"say\"}";
/// Urgent moments of a plan, at most (leaving, a check-in): never a habit's.
pub const URGENT_MAX: usize = 2;
/// The caps (D2 A, §3.2).
pub const PER_REPLY: usize = 1;
pub const UNASKED_PER_AGENT: usize = 2;
pub const UNASKED_PER_PLACE: usize = 4;
pub const DECLINED_DAYS: f64 = 14.0;
/// Minutes kept before a route moment's arrival, on top of the way.
pub const MARGIN_MINUTES: f64 = 5.0;
/// Transit is not routed (OSRM has no timetables): by car, this much longer.
pub const TRANSIT_FACTOR: f64 = 1.5;
/// A set plan stays this long after its last moment.
pub const END_AFTER_SECONDS: f64 = 86400.0;
/// How often `watch` looks (a local file: free).
const TICK_SECONDS: u64 = 5;
/// An event moved by less than this has not moved.
const MOVED_MIN_SECONDS: f64 = 60.0;

pub const WAITING: &str = "waiting";
pub const SET: &str = "set";
pub const KINDS: [&str; 3] = ["trip", "appointment", "habit"];
/// The calendars a plan's `event` may be of (plans/sources.rs).
pub const EVENT_SOURCES: [&str; 3] = ["phone", "google", "caldav"];
pub const MODES: [&str; 3] = ["walking", "driving", "transit"];

// -- the store -----------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    pub id: String,
    /// Unix seconds.
    pub at: f64,
    /// Its IANA zone.
    pub zone: String,
    pub what: String,
    pub say: String,
    #[serde(default)]
    pub urgent: bool,
    /// Written by the bridge from a route (« Pars pour … »).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub route: bool,
    /// A route, found and timed at « Prévoir » (its `at` the arrival until then: review 2, R9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leg: Option<Leg>,
    /// A moment that offers instead of saying (plans/offers.rs): its offer as the agent gave it, checked; at its time
    /// the bridge places its card, no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<Value>,
    /// Its routine, once set (« Prévoir »).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
}

/// A route moment waiting for « Prévoir » to be found and timed (review 2, R9: nothing goes to a geocoder or a router
/// before): where to as the agent wrote it, from where when it gave one (else the person's home, read from
/// « Coordonnées » then, never kept here), how, the arrival.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub place: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub mode: String,
    pub arrive: f64,
}

/// A habit's rhythm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repeat {
    /// The cron weekday field: « * », « 1-5 », « 0,6 », « 1,3,5 ».
    pub days: String,
    /// How it was said: « daily », « weekdays », « weekends », or the days (« mon,wed,fri »).
    pub said: String,
    /// « HH:MM » as asked (the routines run « vers » them).
    pub times: Vec<String>,
    pub weeks: u32,
    /// Its first day (YYYY-MM-DD, the routines' zone).
    pub first: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sender {
    pub name: String,
    pub domain: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventRef {
    /// « phone » (calendar.rs), « google » (google_calendar.rs), « caldav » (caldav.rs: a mailbox's calendar):
    /// plans/sources.rs.
    pub source: String,
    pub id: String,
    /// Its start as last seen (Unix seconds).
    pub start: f64,
    /// Its calendar's id: a calendar taken out of the copy cancels nothing (`event_news`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub agent: String,
    pub kind: String,
    pub title: String,
    pub city: String,
    pub zone: String,
    pub moments: Vec<Moment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<Repeat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<Sender>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<EventRef>,
    /// `WAITING` for the person's answer, or `SET`.
    pub state: String,
    pub made: f64,
    /// Made in a turn of the person's own conversation.
    #[serde(default)]
    pub asked: bool,
    /// The agent's first plan card: its « i » says where its suggestions are set.
    #[serde(default)]
    pub first: bool,
    /// The conversation its card showed in: its lines land there.
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub routines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// When a set plan goes (Unix seconds).
    #[serde(default)]
    pub ends: f64,
    /// Its offers shown on its card (pills) once answered: the card keeps drawing them, « ✓ <its words> » for a
    /// « Oui » (plans/offers.rs `pill_views`; J68, 24/11: an accepted pill left the card).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pills: Vec<PillDone>,
    /// The ids of its pills in the card's order, kept when the first one is answered: an answered pill stays where it
    /// was on the card.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pill_order: Vec<String>,
}

/// A pill of a plan's card answered (or gone past its time): its words as the card showed them, its outcome
/// (plans/offers.rs `YES`, `NO`, `EXPIRED`), when it was made (the pills' order on the card).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PillDone {
    pub id: String,
    pub label: String,
    pub outcome: String,
    pub made: f64,
}

/// A plan card made (the caps count them).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Made {
    pub agent: String,
    pub at: f64,
    pub asked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

/// A plan declined: its title's print only (`print`), kept `DECLINED_DAYS`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Print {
    pub agent: String,
    pub print: String,
    pub at: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub plans: Vec<Plan>,
    #[serde(default)]
    pub declined: Vec<Print>,
    #[serde(default)]
    pub made: Vec<Made>,
    /// The agents whose first plan card was shown (its line on where to set its suggestions).
    #[serde(default)]
    pub hinted: Vec<String>,
}

static LOCK: Mutex<()> = Mutex::new(());
/// When the phone's calendar copy was last read by `tick` (its file's time).
static COPY_SEEN: Mutex<Option<std::time::SystemTime>> = Mutex::new(None);

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("plans.json")
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
    store.declined.retain(|d| now - d.at < DECLINED_DAYS * 86400.0);
    store.made.retain(|m| now - m.at < 2.0 * 86400.0);
    let file = path(config);
    let body = serde_json::to_string_pretty(&store).unwrap_or_default();
    if let Err(e) = crate::profile_files::write_atomic_with(&file, body.as_bytes(), 0o600) {
        tracing::warn!(file = %file.display(), error = %e, "plans not saved");
    }
    out
}

pub fn get(config: &Config, id: &str) -> Option<Plan> {
    load(config).plans.into_iter().find(|p| p.id == id)
}

// -- who may propose ---------------------------------------------------------------------------------------------------

/// `plan_propose` is in `agent`'s tools: it may propose (« Initiative » not off, never an agent other people talk to:
/// agent_proactive.rs `proposes`), and its « Routines » are on (the moments are routines).
pub fn offered(config: &Config, agent: &str) -> bool {
    crate::agent_proactive::proposes(config, agent) && crate::routines::enabled(config, agent)
}

// -- the texts (contracts/plans/cases.json `texts`) ---------------------------------------------------------------------

pub struct Texts {
    pub lang: &'static str,
    pub other_zone: &'static str,
    pub route: &'static str,
    pub modes: [&'static str; 3],
    pub watch: &'static str,
    pub under_i: &'static str,
    pub first_card: &'static str,
    pub moved: &'static str,
    pub cancelled: &'static str,
    pub route_pending: &'static str,
    pub before: &'static str,
    pub route_failed: &'static str,
    pub watch_failed: &'static str,
    pub notification: &'static str,
    pub summary: &'static str,
    pub one_moment: (&'static str, &'static str),
    pub and: &'static str,
    pub days_short: [&'static str; 7],
    pub days_long: [&'static str; 7],
    pub daily: &'static str,
    pub weekdays: &'static str,
    pub weekends: &'static str,
    pub around: &'static str,
    pub weeks: (&'static str, &'static str),
    /// Before a plan's offers (§7.2, validated): its « i » says each one whole.
    pub offers: &'static str,
    /// The card's top line in an app 1.1 (§7.2, validated; `bip_plan.top`): a trip's or an appointment's, a habit's.
    pub top: &'static str,
    pub top_habit: &'static str,
}

pub static TEXTS: [Texts; 4] = [
    Texts {
        lang: "fr",
        other_zone: "· heure de {city}",
        route: "Pars pour {place} · {n} min {mode}",
        modes: ["à pied", "en voiture", "en transports"],
        watch: "Je surveille les e-mails de {name} : un changement, je te le dis.",
        under_i: "Rien n’est envoyé ni réservé : des rappels, au bon moment.",
        first_card: "Tu règles ses propositions en bas de sa page.",
        moved: "Ton rendez-vous « {title} » a bougé : {day} à {time}. J’ai décalé les rappels.",
        cancelled: "Ton rendez-vous « {title} » est annulé : j’ai retiré ses rappels.",
        route_pending: "Pars pour {place} · {mode}",
        before: "avant {time}",
        route_failed: "Je n’ai pas pu calculer le trajet pour {place} : pas de rappel pour partir.",
        watch_failed: "Je ne surveille pas les e-mails de {name} : la veille n’a pas pu être créée. Les rappels sont prévus.",
        notification: "Propose un plan · {title}",
        summary: "{agent} propose un plan pour {title} : {n} moments. Le prévoir ?",
        one_moment: ("1 moments", "1 moment"),
        and: "et",
        days_short: ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
        days_long: ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"],
        daily: "Chaque jour",
        weekdays: "En semaine",
        weekends: "Le week-end",
        around: "vers",
        weeks: ("semaine", "semaines"),
        offers: "Je peux aussi :",
        top: "{agent} propose un plan · {title}",
        top_habit: "{agent} propose une habitude · {title}",
    },
    Texts {
        lang: "en",
        other_zone: "· {city} time",
        route: "Leave for {place} · {n} min {mode}",
        modes: ["on foot", "by car", "by transit"],
        watch: "I’m watching e-mails from {name}: if anything changes, I’ll tell you.",
        under_i: "Nothing is sent or booked: just reminders at the right time.",
        first_card: "You can set its suggestions at the bottom of its page.",
        moved: "Your appointment “{title}” moved: {day} at {time}. I’ve moved the reminders.",
        cancelled: "Your appointment “{title}” was cancelled: I’ve removed its reminders.",
        route_pending: "Leave for {place} · {mode}",
        before: "before {time}",
        route_failed: "I couldn’t time the way to {place}: no reminder to leave.",
        watch_failed: "I’m not watching e-mails from {name}: the watch couldn’t be set up. The reminders are set.",
        notification: "Suggests a plan · {title}",
        summary: "{agent} suggests a plan for {title}: {n} moments. Set it up?",
        one_moment: ("1 moments", "1 moment"),
        and: "and",
        days_short: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        days_long: ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
        daily: "Every day",
        weekdays: "Weekdays",
        weekends: "Weekends",
        around: "around",
        weeks: ("week", "weeks"),
        offers: "I can also:",
        top: "{agent} suggests a plan · {title}",
        top_habit: "{agent} suggests a habit · {title}",
    },
    Texts {
        lang: "de",
        other_zone: "· Ortszeit {city}",
        route: "Brich nach {place} auf · {n} Min. {mode}",
        modes: ["zu Fuß", "mit dem Auto", "mit Bus und Bahn"],
        watch: "Ich behalte E-Mails von {name} im Blick: Ändert sich etwas, sage ich es dir.",
        under_i: "Nichts wird gesendet oder gebucht: nur Erinnerungen zur richtigen Zeit.",
        first_card: "Seine Vorschläge stellst du unten auf seiner Seite ein.",
        moved: "Dein Termin „{title}“ hat sich verschoben: {day} um {time}. Ich habe die Erinnerungen verschoben.",
        cancelled: "Dein Termin „{title}“ wurde abgesagt: Ich habe seine Erinnerungen entfernt.",
        route_pending: "Brich nach {place} auf · {mode}",
        before: "vor {time}",
        route_failed: "Ich konnte den Weg nach {place} nicht berechnen: keine Erinnerung zum Aufbrechen.",
        watch_failed: "Ich behalte E-Mails von {name} nicht im Blick: Die Überwachung ließ sich nicht einrichten. Die Erinnerungen stehen.",
        notification: "Schlägt einen Plan vor · {title}",
        summary: "{agent} schlägt einen Plan für {title} vor: {n} Momente. Einplanen?",
        one_moment: ("1 Momente", "1 Moment"),
        and: "und",
        days_short: ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."],
        days_long: ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"],
        daily: "Täglich",
        weekdays: "Werktags",
        weekends: "Am Wochenende",
        around: "gegen",
        weeks: ("Woche", "Wochen"),
        offers: "Ich kann auch:",
        top: "{agent} schlägt einen Plan vor · {title}",
        top_habit: "{agent} schlägt eine Gewohnheit vor · {title}",
    },
    Texts {
        lang: "es",
        other_zone: "· hora de {city}",
        route: "Sal hacia {place} · {n} min {mode}",
        modes: ["a pie", "en coche", "en transporte público"],
        watch: "Vigilo los correos de {name}: si algo cambia, te lo digo.",
        under_i: "No se envía ni se reserva nada: solo recordatorios en el momento justo.",
        first_card: "Puedes ajustar sus propuestas al final de su página.",
        moved: "Tu cita «{title}» ha cambiado: {day} a las {time}. He movido los recordatorios.",
        cancelled: "Tu cita «{title}» se ha cancelado: he quitado sus recordatorios.",
        route_pending: "Sal hacia {place} · {mode}",
        before: "antes de las {time}",
        route_failed: "No pude calcular el trayecto hasta {place}: no habrá aviso para salir.",
        watch_failed: "No vigilo los correos de {name}: no se pudo activar la vigilancia. Los recordatorios están programados.",
        notification: "Propone un plan · {title}",
        summary: "{agent} propone un plan para {title}: {n} momentos. ¿Programarlo?",
        one_moment: ("1 momentos", "1 momento"),
        and: "y",
        days_short: ["lun.", "mar.", "mié.", "jue.", "vie.", "sáb.", "dom."],
        days_long: ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"],
        daily: "Cada día",
        weekdays: "Entre semana",
        weekends: "Los fines de semana",
        around: "hacia",
        weeks: ("semana", "semanas"),
        offers: "También puedo:",
        top: "{agent} propone un plan · {title}",
        top_habit: "{agent} propone un hábito · {title}",
    },
];

/// The texts in `lang` (another language: English).
pub fn texts(lang: &str) -> &'static Texts {
    TEXTS.iter().find(|t| t.lang == lang).unwrap_or(&TEXTS[1])
}

/// `template` with its `{key}`s filled.
fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    pairs.iter().fold(template.to_string(), |text, (key, value)| text.replace(&format!("{{{key}}}"), value))
}

/// « a, b et c ».
fn joined(items: &[String], and: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} {and} {last}", head.join(", ")),
    }
}

/// The plan's sentence for an app 1.0 (its whole card): « Vie propose un plan pour Lisbonne : 3 moments. Le prévoir ? »
pub fn summary(lang: &str, agent: &str, title: &str, n: usize) -> String {
    let t = texts(lang);
    let text = fill(t.summary, &[("agent", agent), ("title", title), ("n", &n.to_string())]);
    if n == 1 {
        text.replace(t.one_moment.0, t.one_moment.1)
    } else {
        text
    }
}

/// The card's top line in an app 1.1, in the agent's language like the rest of the card: « Vie propose un plan ·
/// Lisbonne », « Vie propose une habitude · Boire plus d’eau » (an app 1.0 ignores it; an app 1.1 before it: the phone's).
pub fn top(lang: &str, agent: &str, title: &str, kind: &str) -> String {
    let t = texts(lang);
    fill(if kind == "habit" { t.top_habit } else { t.top }, &[("agent", agent), ("title", title)])
}

pub fn notification(lang: &str, title: &str) -> String {
    fill(texts(lang).notification, &[("title", title)])
}

/// « Je surveille les e-mails de TAP : un changement, je te le dis. »
pub fn watch_text(lang: &str, senders: &[Sender]) -> Option<String> {
    if senders.is_empty() {
        return None;
    }
    let t = texts(lang);
    let names: Vec<String> = senders.iter().map(|s| s.name.clone()).collect();
    Some(fill(t.watch, &[("name", &joined(&names, t.and))]))
}

/// « Pars pour Cabinet du Dr Martin · 25 min à pied ».
pub fn route_words(lang: &str, place: &str, minutes: u32, mode: &str) -> String {
    let t = texts(lang);
    let way = t.modes[MODES.iter().position(|m| *m == mode).unwrap_or(0)];
    fill(t.route, &[("place", place), ("n", &minutes.to_string()), ("mode", way)])
}

/// « Pars pour Cabinet du Dr Martin · à pied », a route from home before « Prévoir » (its minutes come then).
pub fn route_pending_words(lang: &str, place: &str, mode: &str) -> String {
    let t = texts(lang);
    let way = t.modes[MODES.iter().position(|m| *m == mode).unwrap_or(0)];
    fill(t.route_pending, &[("place", place), ("mode", way)])
}

pub fn route_failed_line(lang: &str, place: &str) -> String {
    fill(texts(lang).route_failed, &[("place", place)])
}

/// « Je ne surveille pas les e-mails de TAP : … »
pub fn watch_failed_line(lang: &str, senders: &[Sender]) -> String {
    let t = texts(lang);
    let names: Vec<String> = senders.iter().map(|s| s.name.clone()).collect();
    fill(t.watch_failed, &[("name", &joined(&names, t.and))])
}

/// « mer. 25 · 10:50 », then « · heure de Lisbonne » when its zone's clock is not the person's.
pub fn when_text(lang: &str, at: f64, zone: Tz, person: Tz, city: &str) -> String {
    when_with(lang, at, zone, person, city, false)
}

/// « jeu. 26 · avant 15:00 »: a route from home before « Prévoir », at its arrival.
pub fn before_text(lang: &str, at: f64, zone: Tz, person: Tz, city: &str) -> String {
    when_with(lang, at, zone, person, city, true)
}

fn when_with(lang: &str, at: f64, zone: Tz, person: Tz, city: &str, before: bool) -> String {
    let t = texts(lang);
    let Some(local) = zone.timestamp_opt(at as i64, 0).single() else { return String::new() };
    let day = t.days_short[local.weekday().num_days_from_monday() as usize];
    let clock = local.format("%H:%M").to_string();
    let clock = if before { fill(t.before, &[("time", &clock)]) } else { clock };
    let mut text = format!("{day} {} · {clock}", local.day());
    let theirs = person.timestamp_opt(at as i64, 0).single().map(|d| d.naive_local());
    if theirs != Some(local.naive_local()) {
        text.push(' ');
        text.push_str(&fill(t.other_zone, &[("city", city)]));
    }
    text
}

/// A time of day in words: « 10 h », « 10 AM », « 10 », « las 10 » (German says « Uhr » once, after the last).
fn clock_words(lang: &str, hour: u32, minute: u32) -> String {
    match lang {
        "fr" if minute == 0 => format!("{hour} h"),
        "fr" => format!("{hour} h {minute:02}"),
        "en" => {
            let (h, half) = match hour {
                0 => (12, "AM"),
                1..=11 => (hour, "AM"),
                12 => (12, "PM"),
                _ => (hour - 12, "PM"),
            };
            if minute == 0 {
                format!("{h} {half}")
            } else {
                format!("{h}:{minute:02} {half}")
            }
        }
        "es" => {
            let article = if hour == 1 { "la" } else { "las" };
            if minute == 0 {
                format!("{article} {hour}")
            } else {
                format!("{article} {hour}:{minute:02}")
            }
        }
        _ if minute == 0 => format!("{hour}"),
        _ => format!("{hour}:{minute:02}"),
    }
}

/// « En semaine vers 10 h et 15 h · 2 semaines ».
pub fn rhythm_text(lang: &str, r: &Repeat) -> String {
    let t = texts(lang);
    let days = match r.said.as_str() {
        "daily" => t.daily.to_string(),
        "weekdays" => t.weekdays.to_string(),
        "weekends" => t.weekends.to_string(),
        list => {
            let named: Vec<String> = list
                .split(',')
                .filter_map(|d| DAY_WORDS.iter().position(|w| *w == d))
                .map(|i| t.days_short[i].to_string())
                .collect();
            let text = joined(&named, t.and);
            let mut chars = text.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
    };
    let clocks: Vec<String> =
        r.times.iter().filter_map(|c| hour_minute(c)).map(|(h, m)| clock_words(t.lang, h, m)).collect();
    let mut times = joined(&clocks, t.and);
    if t.lang == "de" {
        times.push_str(" Uhr");
    }
    let weeks = format!("{} {}", r.weeks, if r.weeks == 1 { t.weeks.0 } else { t.weeks.1 });
    format!("{days} {} {times} · {weeks}", t.around)
}

/// « vendredi 27 » and « 16:00 », for a moved appointment's line.
fn day_and_time(lang: &str, at: f64, zone: Tz) -> (String, String) {
    let t = texts(lang);
    match zone.timestamp_opt(at as i64, 0).single() {
        Some(local) => {
            let day = t.days_long[local.weekday().num_days_from_monday() as usize];
            let day = if lang == "de" { format!("{day}, {}.", local.day()) } else { format!("{day} {}", local.day()) };
            (day, local.format("%H:%M").to_string())
        }
        None => (String::new(), String::new()),
    }
}

pub fn moved_line(lang: &str, title: &str, start: f64, zone: Tz) -> String {
    let (day, time) = day_and_time(lang, start, zone);
    fill(texts(lang).moved, &[("title", title), ("day", &day), ("time", &time)])
}

pub fn cancelled_line(lang: &str, title: &str) -> String {
    fill(texts(lang).cancelled, &[("title", title)])
}

// -- reading the agent's plan ------------------------------------------------------------------------------------------

const DAY_WORDS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A day of the week as models write it (« mon », « Mon. », « monday », « Mondays », « thurs »): its index from Monday.
/// Only a day's whole word, or its plural: « monthly » is not Monday.
fn day_of(word: &str) -> Option<usize> {
    let word = word.trim().trim_end_matches('.').to_lowercase();
    let word = match word.strip_suffix("days") {
        Some(day) if !day.is_empty() => format!("{day}day"),
        _ => word,
    };
    const FULL: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    const ALSO: [(&str, usize); 4] = [("tues", 1), ("thur", 3), ("thurs", 3), ("weds", 2)];
    DAY_WORDS
        .iter()
        .position(|w| *w == word)
        .or_else(|| FULL.iter().position(|w| *w == word))
        .or_else(|| ALSO.iter().find(|(w, _)| *w == word).map(|(_, i)| *i))
}

fn hour_minute(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// A date-time as the agent writes it: with its offset, or « YYYY-MM-DDTHH:MM » on `zone`'s clock.
pub fn parse_at(text: &str, zone: Tz) -> Option<f64> {
    let text = text.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(text) {
        return Some(d.timestamp() as f64);
    }
    ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"].iter().find_map(|f| {
        let naive = NaiveDateTime::parse_from_str(text, f).ok()?;
        zone.from_local_datetime(&naive).earliest().map(|d| d.timestamp() as f64)
    })
}

/// The zone's city, when the agent named none: « Europe/Lisbon » → « Lisbon ».
fn zone_city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

fn text_arg(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).map(str::trim).unwrap_or("").to_string()
}

/// `text` within `max` characters, cut at a word with « … » when longer (a card's short line a model wrote too long:
/// bench of proactive-tune, a moment's `what` and an offer's `about` refused for a few characters, then written again).
pub(crate) fn shortened(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        if out.chars().count() + word.chars().count() + 2 > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = text.chars().take(max - 1).collect();
    }
    format!("{}…", out.trim_end_matches([',', ';', ':', '-', '—', '·', ' ']))
}

/// Plain text a card shows and the bridge says: no markup, no line break, links in https only.
fn plain(value: Option<&Value>, field: &str, max: usize) -> Result<String, String> {
    let text = text_arg(value);
    let n = text.chars().count();
    if n == 0 || n > max {
        return Err(format!("{field}: 1 to {max} characters"));
    }
    if text.contains(['\n', '\r', '<', '>', '`']) || text.contains("](") || text.contains("**") {
        return Err(format!("{field}: plain text on one line (no markup)"));
    }
    if text.to_lowercase().contains("http://") {
        return Err(format!("{field}: links in https only"));
    }
    Ok(text)
}

fn domain_ok(domain: &str) -> bool {
    static HOST: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$").expect("valid regex")
    });
    domain.len() <= 100 && HOST.is_match(domain)
}

/// A route the bridge must time (`resolve_routes`).
#[derive(Debug, Clone, PartialEq)]
pub struct RouteAsk {
    pub moment: usize,
    pub to: String,
    pub from: Option<String>,
    pub arrive: f64,
    pub mode: String,
}

/// The agent's plan read and checked, before the bridge's own lookups (routes, the event, the mailbox).
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub plan: Plan,
    pub routes: Vec<RouteAsk>,
}

/// `plan_propose`'s arguments read (`person`: the person's zone; `routines`: the routines' clock): Err, the reason.
pub fn draft(arguments: &Dict, agent: &str, person: Tz, routines: Tz, now: f64) -> Result<Draft, String> {
    let kind = text_arg(arguments.get("kind")).to_lowercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err("kind: trip, appointment or habit".into());
    }
    let title = plain(arguments.get("title"), "title", TITLE_MAX)?;
    let zone_name = match text_arg(arguments.get("zone")) {
        z if z.is_empty() => person.name().to_string(),
        z => z,
    };
    let zone = crate::dates::parse_zone(&zone_name).ok_or("zone: an IANA zone such as Europe/Lisbon")?;
    let city = match arguments.get("city").filter(|v| !v.is_null()) {
        Some(v) => plain(Some(v), "city", CITY_MAX)?,
        None => zone_city(zone.name()),
    };
    let mut list = arguments.get("moments").and_then(Value::as_array).cloned().unwrap_or_default();
    let habit = kind == "habit";
    // a habit's rhythm as models write it (bench 1.1, 6 refusals of 8 for GLM): `repeat` inside its moment rather
    // than beside it, taken from there; its moment's `at` is its first day
    let mut repeat_arg = arguments.get("repeat").filter(|v| !v.is_null()).cloned();
    if habit && repeat_arg.is_none() {
        repeat_arg = list.iter().find_map(|m| m.get("repeat").filter(|v| v.is_object()).cloned());
    }
    // a habit written as one moment per time or day (bench of proactive-tune: GLM and DeepSeek, one of each two
    // habits): one moment, its rhythm from the moments when it gives none. Only when it is one thing said the same way
    // at each moment (the review of 24/11: « course » Monday 18:30 and « piscine » Wednesday 07:00 became « course »
    // both days at both times), on days × times; else refused with the shape, never guessed
    if habit && list.len() > 1 && list.len() <= MAX_MOMENTS * 2 {
        let merged = merge_habit_moments(&list, repeat_arg.as_ref(), person)?;
        repeat_arg = Some(merged.1);
        list = vec![merged.0];
    }
    if list.is_empty() || list.len() > MAX_MOMENTS || (habit && list.len() != 1) {
        return Err(if habit {
            format!("moments: exactly one for a habit, its times of day in repeat.times ({REPEAT_EXAMPLE})")
        } else {
            format!("moments: 1 to {MAX_MOMENTS}")
        });
    }
    if habit {
        // a habit's line and words: one given stands for the other, its title for both
        if let Some(Value::Object(m)) = list.first_mut() {
            let given = |m: &serde_json::Map<String, Value>, k: &str| text_arg(m.get(k)).chars().count() > 0;
            if !given(m, "what") {
                let what = if given(m, "say") { text_arg(m.get("say")) } else { title.clone() };
                m.insert("what".into(), json!(what.chars().take(WHAT_MAX).collect::<String>()));
            }
            if !given(m, "say") {
                let say = text_arg(m.get("what"));
                m.insert("say".into(), json!(say));
            }
        }
    }
    let mut moments = Vec::new();
    let mut routes = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let id = format!("m{}", i + 1);
        let moment_zone = match text_arg(item.get("zone")) {
            z if z.is_empty() => zone,
            z => crate::dates::parse_zone(&z).ok_or_else(|| format!("moment {}: zone: an IANA zone", i + 1))?,
        };
        if let Some(route) = item.get("route").filter(|r| r.is_object()) {
            if habit {
                return Err("a habit has no route".into());
            }
            let mode = text_arg(route.get("mode")).to_lowercase();
            let mode = if mode.is_empty() { "walking".to_string() } else { mode };
            if !MODES.contains(&mode.as_str()) {
                return Err(format!("moment {}: route.mode: walking, driving or transit", i + 1));
            }
            let to = plain(route.get("to"), "route.to", PLACE_MAX)?;
            let from = match route.get("from").filter(|v| !v.is_null()) {
                Some(v) => Some(plain(Some(v), "route.from", PLACE_MAX)?),
                None => None,
            };
            let arrive = parse_at(&text_arg(route.get("arrive_by")), moment_zone)
                .ok_or_else(|| format!("moment {}: route.arrive_by: « YYYY-MM-DDTHH:MM »", i + 1))?;
            if arrive <= now {
                return Err(format!("moment {}: the arrival is not to come", i + 1));
            }
            routes.push(RouteAsk { moment: i, to, from, arrive, mode });
            moments.push(Moment {
                id,
                at: 0.0,
                zone: moment_zone.name().to_string(),
                what: String::new(),
                say: String::new(),
                urgent: true,
                route: true,
                leg: None,
                offer: None,
                routine: None,
            });
            continue;
        }
        let what = plain(Some(&json!(shortened(&text_arg(item.get("what")), WHAT_MAX))), &format!("moment {}: what", i + 1), WHAT_MAX)?;
        // a moment that offers says nothing: a line to tell, a card to decide (plans/offers.rs)
        let offer = item.get("offer").filter(|o| !o.is_null()).cloned();
        if let Some(o) = &offer {
            if habit || !o.is_object() {
                return Err(format!("moment {}: offer: an object {{kind, label, prompt | params}}, never for a habit", i + 1));
            }
            if item.get("say").is_some_and(|v| !v.is_null()) || item.get("link").is_some_and(|v| !v.is_null()) {
                return Err(format!(
                    "moment {}: offer: a moment that offers says nothing: give `what` and `offer`, no `say` nor \
                     `link` (a line to tell, a card to decide)",
                    i + 1
                ));
            }
        }
        let mut say = if offer.is_some() {
            String::new()
        } else {
            plain(item.get("say"), &format!("moment {}: say", i + 1), SAY_MAX)?
        };
        let link = text_arg(item.get("link"));
        if !link.is_empty() {
            if !link.starts_with("https://") || link.contains(char::is_whitespace) || link.len() > 300 {
                return Err(format!("moment {}: link: an https URL", i + 1));
            }
            if !say.contains(&link) {
                say = format!("{say} {link}");
            }
        }
        let at_text = text_arg(item.get("at"));
        let at = if at_text.is_empty() && habit {
            0.0
        } else {
            let at = parse_at(&at_text, moment_zone)
                .ok_or_else(|| format!("moment {}: at: « YYYY-MM-DDTHH:MM »", i + 1))?;
            if at <= now && !habit {
                return Err(format!("moment {}: its time is not to come", i + 1));
            }
            at
        };
        let urgent = item.get("urgent").is_some_and(py::truthy);
        if urgent && habit {
            return Err("moment 1: urgent: never for a habit (every day at the phone's urgent level)".into());
        }
        moments.push(Moment {
            id,
            at,
            zone: moment_zone.name().to_string(),
            what,
            say,
            urgent,
            route: false,
            leg: None,
            offer,
            routine: None,
        });
    }
    if moments.iter().filter(|m| m.urgent).count() > URGENT_MAX {
        return Err(format!(
            "urgent: {URGENT_MAX} moments at most (when to leave, a check-in opening: what is missed if late); a route \
             is urgent already"
        ));
    }
    let repeat = match (habit, repeat_arg.as_ref()) {
        (true, Some(r)) => {
            let r = read_repeat(r, moments[0].at, routines, now).map_err(|e| format!("{e} ({REPEAT_EXAMPLE})"))?;
            // its moment: its first run (the card says its rhythm)
            let after = first_midnight(&r, routines).max(now) - 1.0;
            moments[0].at = habit_schedules(&r, 0)
                .into_iter()
                .filter_map(|(expr, _)| crate::routines::next_after(&crate::routines::Schedule::Cron { expr }, after, routines))
                .fold(f64::INFINITY, f64::min);
            if !moments[0].at.is_finite() {
                moments[0].at = first_midnight(&r, routines);
            }
            Some(r)
        }
        (true, None) => return Err(format!("repeat: a habit needs its rhythm ({REPEAT_EXAMPLE})")),
        (false, Some(_)) => return Err("repeat: for a habit only (kind habit, one moment)".into()),
        (false, None) => None,
    };
    let watch = match arguments.get("watch").filter(|v| !v.is_null()) {
        None => Vec::new(),
        Some(w) => {
            let senders = w.get("mail_from").and_then(Value::as_array).cloned().unwrap_or_default();
            if senders.is_empty() || senders.len() > SENDERS_MAX {
                return Err(format!("watch.mail_from: 1 to {SENDERS_MAX} senders"));
            }
            let mut out = Vec::new();
            for s in senders {
                let name = plain(s.get("name"), "watch name", NAME_MAX)?;
                let domain = text_arg(s.get("domain")).to_lowercase();
                if !domain_ok(&domain) {
                    return Err(format!("watch: « {domain} » is not a plain host name (flytap.com)"));
                }
                out.push(Sender { name, domain });
            }
            out
        }
    };
    let event = match arguments.get("event").filter(|v| !v.is_null()) {
        None => None,
        Some(e) => {
            let source = text_arg(e.get("source")).to_lowercase();
            let id = text_arg(e.get("id"));
            if !EVENT_SOURCES.contains(&source.as_str()) || id.is_empty() || id.len() > 300 {
                return Err("event: {source: phone | google | caldav, id} (an event of a calendar you read, its id as \
                            calendar_list gives it)"
                    .into());
            }
            Some(EventRef { source, id, start: 0.0, calendar: String::new() })
        }
    };
    let id = py::uuid4_hex()[..12].to_string();
    let plan = Plan {
        id,
        agent: agent.to_string(),
        kind,
        title,
        city,
        zone: zone.name().to_string(),
        moments,
        repeat,
        watch,
        event,
        state: WAITING.into(),
        made: now,
        asked: false,
        first: false,
        origin: String::new(),
        routines: Vec::new(),
        trigger: None,
        ends: 0.0,
        pills: Vec::new(),
        pill_order: Vec::new(),
    };
    Ok(Draft { plan, routes })
}

/// A habit's line without what tells its moments apart (« Course (lundi) » → « course »), to compare them.
fn habit_words(text: &str) -> String {
    let t = text.trim();
    let t = match t.rfind(" (") {
        Some(i) if t.ends_with(')') => &t[..i],
        _ => t,
    };
    t.trim().to_lowercase()
}

/// A habit written as one moment per time or day, made one moment and its rhythm (`repeat` as given, its times and
/// days from the moments when it gives none). Err, the shape, when the moments are not one thing said the same way
/// (their `what` and `say`), when they are not each of their days at each of their times, or when all are on one day
/// and no days are given (every day, or that day each week?).
fn merge_habit_moments(list: &[Value], given: Option<&Value>, person: Tz) -> Result<(Value, Value), String> {
    let one_habit = || format!("moments: a habit is one moment, said the same way at each of its times (repeat.times); \
for another activity or other words, one plan_propose each ({REPEAT_EXAMPLE})");
    for field in ["what", "say"] {
        let mut said: Vec<String> = list.iter().map(|m| habit_words(&text_arg(m.get(field)))).filter(|w| !w.is_empty()).collect();
        said.dedup();
        if said.len() > 1 {
            return Err(one_habit());
        }
    }
    let at_of = |m: &Value| parse_at(&text_arg(m.get("at")), person);
    let pairs: Vec<(usize, String)> = list
        .iter()
        .filter_map(at_of)
        .filter_map(|a| person.timestamp_opt(a as i64, 0).single())
        .map(|d| (d.weekday().num_days_from_monday() as usize, d.format("%H:%M").to_string()))
        .collect();
    let mut rhythm = given.cloned().unwrap_or_else(|| json!({}));
    let mut times: Vec<String> = pairs.iter().map(|(_, t)| t.clone()).collect();
    times.sort();
    times.dedup();
    let mut days: Vec<usize> = pairs.iter().map(|(d, _)| *d).collect();
    days.sort();
    days.dedup();
    if !pairs.is_empty() {
        let mut distinct = pairs.clone();
        distinct.sort();
        distinct.dedup();
        if distinct.len() != days.len() * times.len() {
            return Err(format!("moments: each day at the same times; for other times on other days, one plan_propose \
each ({REPEAT_EXAMPLE})"));
        }
    }
    if rhythm.get("times").is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())) && !times.is_empty() {
        rhythm["times"] = json!(times);
    }
    if rhythm.get("days").is_none() && !days.is_empty() {
        if days.len() == 1 {
            return Err(format!("repeat.days: which days (daily, weekdays, weekends or a list) ({REPEAT_EXAMPLE})"));
        }
        rhythm["days"] = json!(days.iter().map(|d| DAY_WORDS[*d]).collect::<Vec<_>>());
    }
    // its words: the first moment's, without what told one day from another
    let mut moment = list[0].clone();
    let first = text_arg(list[0].get("what"));
    let first = first.trim();
    if list.iter().any(|m| text_arg(m.get("what")).trim() != first) {
        if let Some(i) = first.rfind(" (").filter(|_| first.ends_with(')')) {
            moment["what"] = json!(first[..i].trim());
        }
    }
    Ok((moment, rhythm))
}

/// A habit's rhythm read: its first day is its moment's `at` (else tomorrow, on the routines' clock).
fn read_repeat(value: &Value, at: f64, routines: Tz, now: f64) -> Result<Repeat, String> {
    // one day or a list in one text (« mon, wed, fri »): its list
    let listed = match value.get("days") {
        Some(Value::String(word)) if word.contains([',', ' ']) && !matches!(word.trim(), "every day") => Some(Value::Array(
            word.split([',', ' ']).map(str::trim).filter(|w| !w.is_empty()).map(|w| json!(w)).collect(),
        )),
        Some(Value::String(word)) if day_of(word).is_some() => Some(json!([word])),
        Some(Value::Number(n)) => Some(json!([n])),
        other => other.cloned(),
    };
    let (days, said) = match listed.as_ref() {
        Some(Value::String(word)) => match word.trim().to_lowercase().as_str() {
            "daily" | "every day" | "everyday" => ("*".to_string(), "daily".to_string()),
            "weekdays" => ("1-5".to_string(), "weekdays".to_string()),
            "weekends" => ("0,6".to_string(), "weekends".to_string()),
            // « monthly », « every other week »…: not a habit's rhythm (it used to be « mon », every Monday)
            _ => return Err("repeat.days: daily, weekdays, weekends or a list of days of the week".into()),
        },
        Some(Value::Array(list)) => {
            let mut picked: Vec<usize> = Vec::new();
            for d in list {
                // a day's number as models write it, ISO's: 1 Monday … 7 Sunday. 0 is refused, never guessed: Sunday
                // for cron, Monday for Python (the review of 24/11: [0, 2, 4] read Sunday, Tuesday, Thursday)
                let i = match d.as_u64() {
                    Some(n @ 1..=7) => Some(n as usize - 1),
                    Some(_) => None,
                    None => d.as_str().and_then(day_of),
                };
                let i = i.ok_or("repeat.days: mon, tue, wed, thu, fri, sat, sun (or 1 Monday … 7 Sunday)")?;
                if !picked.contains(&i) {
                    picked.push(i);
                }
            }
            if picked.is_empty() {
                return Err("repeat.days: at least one day".into());
            }
            picked.sort();
            // cron: Monday 1 … Saturday 6, Sunday 0
            let cron: Vec<String> = picked.iter().map(|i| ((i + 1) % 7).to_string()).collect();
            let said: Vec<&str> = picked.iter().map(|i| DAY_WORDS[*i]).collect();
            (cron.join(","), said.join(","))
        }
        _ => return Err("repeat.days: daily, weekdays, weekends or a list of days".into()),
    };
    let times: Vec<String> = match value.get("times") {
        Some(Value::String(one)) => one.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        other => other
            .and_then(Value::as_array)
            .map(|l| l.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
    };
    if times.is_empty() || times.len() > TIMES_MAX || times.iter().any(|t| hour_minute(t).is_none()) {
        return Err(format!("repeat.times: 1 to {TIMES_MAX} times « HH:MM »"));
    }
    let weeks = match value.get("weeks").filter(|v| !v.is_null()) {
        None => u64::from(DEFAULT_WEEKS),
        Some(w) => w.as_u64().or_else(|| w.as_str().and_then(|t| t.trim().parse().ok())).unwrap_or(0),
    };
    if weeks == 0 || weeks > u64::from(WEEKS_MAX) {
        return Err(format!("repeat.weeks: 1 to {WEEKS_MAX}"));
    }
    let today = routines
        .timestamp_opt(now as i64, 0)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());
    let first = if at > 0.0 {
        routines.timestamp_opt(at as i64, 0).single().map(|d| d.date_naive()).unwrap_or(today)
    } else {
        today.succ_opt().unwrap_or(today)
    };
    Ok(Repeat { days, said, times, weeks: weeks as u32, first: first.format("%Y-%m-%d").to_string() })
}

/// A habit's routines: one per time of day, « vers » it (`shift` minutes, the same for all), each with its number of
/// runs over its weeks. (cron expression, runs)
pub fn habit_schedules(r: &Repeat, shift: i64) -> Vec<(String, u32)> {
    let Ok(first) = NaiveDate::parse_from_str(&r.first, "%Y-%m-%d") else { return Vec::new() };
    let days = i64::from(r.weeks) * 7;
    let runs = (0..days)
        .filter_map(|d| first.checked_add_signed(chrono::Duration::days(d)))
        .filter(|day| {
            let Some(at) = day.and_hms_opt(12, 0, 0) else { return false };
            crate::routines::cron_matches(&format!("0 12 * * {}", r.days), &at).unwrap_or(false)
        })
        .count() as u32;
    r.times
        .iter()
        .filter_map(|t| hour_minute(t))
        .map(|(h, m)| {
            let minutes = (i64::from(h) * 60 + i64::from(m) + shift).rem_euclid(24 * 60);
            (format!("{} {} * * {}", minutes % 60, minutes / 60, r.days), runs)
        })
        .collect()
}

/// The stable shift of a routine « vers » its time (Alex Q7 A): −10 to +10 minutes, from the plan's id.
pub fn approx_shift(id: &str) -> i64 {
    let digest = Sha256::digest(id.as_bytes());
    i64::from(digest[0]) % 21 - 10
}

/// The first midnight of a habit (its first day, on the routines' clock).
fn first_midnight(r: &Repeat, routines: Tz) -> f64 {
    NaiveDate::parse_from_str(&r.first, "%Y-%m-%d")
        .ok()
        .and_then(|d| routines.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest())
        .map(|d| d.timestamp() as f64)
        .unwrap_or(0.0)
}

/// How many times the plan says something: its moments, a habit's runs.
pub fn count(plan: &Plan) -> usize {
    match &plan.repeat {
        Some(r) => habit_schedules(r, 0).iter().map(|(_, n)| *n as usize).sum(),
        None => plan.moments.len(),
    }
}

/// A title's print (« Non merci » keeps only this): case and spaces aside.
pub fn print_of(agent: &str, title: &str) -> String {
    let folded = title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    hex::encode(&Sha256::digest(format!("{agent}\u{1f}{folded}").as_bytes())[..12])
}

// -- the caps -----------------------------------------------------------------------------------------------------------

/// Why a new plan card may not be made now (None: it may), in the agent's words.
pub fn refused(store: &Store, plan: &Plan, run: Option<&str>, person: Tz, now: f64) -> Option<String> {
    let print = print_of(&plan.agent, &plan.title);
    if store.declined.iter().any(|d| d.agent == plan.agent && d.print == print && now - d.at < DECLINED_DAYS * 86400.0) {
        return Some(format!(
            "Not done: your person said no to the plan « {} » in the last {} days. Don't propose it again.",
            plan.title, DECLINED_DAYS as u32
        ));
    }
    if let Some(same) = store.plans.iter().find(|p| p.agent == plan.agent && print_of(&p.agent, &p.title) == print) {
        return Some(if same.state == WAITING {
            format!("Not done: a plan « {} » already waits for your person's answer.", plan.title)
        } else {
            format!("Not done: a plan « {} » is already set.", plan.title)
        });
    }
    if plan.asked {
        let in_reply = store.made.iter().filter(|m| m.agent == plan.agent && m.run.as_deref() == run).count();
        if run.is_some() && in_reply >= PER_REPLY {
            return Some("Not done: one plan per reply; you made one in this reply already.".into());
        }
        return None;
    }
    if store.plans.iter().any(|p| p.agent == plan.agent && p.state == WAITING) {
        return Some(
            "Not done: one of your cards already waits for your person's answer. Don't propose another one today."
                .into(),
        );
    }
    let day = |at: f64| person.timestamp_opt(at as i64, 0).single().map(|d| d.date_naive());
    let today = day(now);
    let unasked_today = |m: &&Made| !m.asked && day(m.at) == today;
    if store.made.iter().filter(unasked_today).filter(|m| m.agent == plan.agent).count() >= UNASKED_PER_AGENT {
        return Some(format!(
            "Not done: you already made {UNASKED_PER_AGENT} cards today that your person didn't ask for. Not \
             more before tomorrow."
        ));
    }
    if store.made.iter().filter(unasked_today).count() >= UNASKED_PER_PLACE {
        return Some(format!(
            "Not done: your person's agents made {UNASKED_PER_PLACE} cards today they didn't ask for. Not more \
             before tomorrow."
        ));
    }
    None
}

// -- the card -----------------------------------------------------------------------------------------------------------

fn shown_agent(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| agent.into())
}

fn lang_of(config: &Config, agent: &str) -> &'static str {
    crate::names::names(&crate::names::agent_language(config, agent)).lang
}

fn person_zone(config: &Config) -> Tz {
    crate::dates::load(config).zone()
}

fn zone_of(name: &str) -> Tz {
    crate::dates::parse_zone(name).unwrap_or(Tz::UTC)
}

/// Each moment as the card shows it.
fn moment_views(config: &Config, plan: &Plan, lang: &str) -> Vec<Value> {
    let person = person_zone(config);
    let rhythm = plan.repeat.as_ref().map(|r| rhythm_text(lang, r));
    plan.moments
        .iter()
        .map(|m| {
            let when = rhythm.clone().unwrap_or_else(|| match m.leg {
                Some(_) => before_text(lang, m.at, zone_of(&m.zone), person, &plan.city),
                None => when_text(lang, m.at, zone_of(&m.zone), person, &plan.city),
            });
            let offer = m.offer.as_ref().map(|o| json!({"kind": o.get("kind"), "label": o.get("label"), "prompt": o.get("prompt")}));
            json!({"id": m.id, "at": m.at, "zone": m.zone, "when_text": when, "what": m.what, "say": m.say,
                   "urgent": m.urgent, "route": m.route, "pending": m.leg.is_some(), "offer": offer})
        })
        .collect()
}

/// The plan's card (`/v1/approvals`' shape).
pub fn card_item(config: &Config, plan: &Plan) -> Dict {
    let lang = lang_of(config, &plan.agent);
    let t = texts(lang);
    let shown = shown_agent(config, &plan.agent);
    let sentence = summary(lang, &shown, &plan.title, count(plan));
    let moments = moment_views(config, plan, lang);
    let watch = watch_text(lang, &plan.watch);
    let mut detail: Vec<String> = moments
        .iter()
        .map(|m| {
            let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
            // a moment that offers: its question and what will be asked (its card comes at its time: review, remark)
            let said = match (m["offer"]["label"].as_str(), m["offer"]["prompt"].as_str()) {
                (Some(label), Some(prompt)) => format!("{label} — {prompt}"),
                (Some(label), None) => label.to_string(),
                _ => s("say"),
            };
            format!("{} — {} : {said}", s("when_text"), s("what"))
        })
        .collect();
    detail.extend(watch.clone());
    // its offers shown on its card, each whole (design §3.1; review, remark)
    detail.extend(offers::pill_lines(config, &plan.id, lang));
    detail.push(t.under_i.to_string());
    if plan.first {
        detail.push(t.first_card.to_string());
    }
    let item = json!({
        "agent": plan.agent, "run_id": format!("{RUN_PREFIX}{}", plan.id), "request_id": plan.id,
        "command": format!("{TOOL_NAME} · {}", plan.title), "description": sentence, "choices": ["once", "deny"],
        "created_at": plan.made, "session_id": null, "bip_summary": sentence, "bip_detail": detail.join("\n"),
        "bip_plan": {"id": plan.id, "kind": plan.kind, "title": plan.title, "agent": shown, "moments": moments,
                     "top": top(lang, &shown, &plan.title, &plan.kind),
                     "rhythm_text": plan.repeat.as_ref().map(|r| rhythm_text(lang, r)), "watch_text": watch,
                     "offers": offers::pills_of(config, &plan.id),
                     "pills": offers::pill_views(config, plan)},
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The plan cards waiting for the person, oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut plans: Vec<Plan> = load(config)
        .plans
        .into_iter()
        .filter(|p| p.state == WAITING && agent.is_none_or(|a| p.agent == a))
        .collect();
    plans.sort_by(|a, b| a.made.total_cmp(&b.made));
    plans.iter().map(|p| card_item(config, p)).collect()
}

// -- the tool -----------------------------------------------------------------------------------------------------------

/// `plan_propose` without offers.
pub fn tool() -> Value {
    tool_with(&[])
}

/// `plan_propose`, its offers of `kinds` (what offer_act lists for this agent, plans/offers.rs `kinds_for`): on its
/// card (`offers`) and at a moment (a moment's `offer`); none: neither.
pub fn tool_with(kinds: &[&str]) -> Value {
    let mut tool = plan_tool();
    if kinds.is_empty() {
        return tool;
    }
    let line = format!(
        " You can also offer to do things, of these kinds only: {} (as offer_act's): up to 2 `offers` shown at once on \
the card (« Je regarde les hôtels ? »), or a moment that offers at its time instead of saying (`offer`, no `say`).",
        kinds.join(", ")
    );
    if let Some(Value::String(d)) = tool.get_mut("description") {
        d.push_str(&line);
    }
    let props = &mut tool["inputSchema"]["properties"];
    props["offers"] = json!({"type": "array", "maxItems": offers::PER_REPLY,
                             "items": offers::plan_schema(kinds, offers::PILL_MAX)});
    props["moments"]["items"]["properties"]["offer"] = offers::plan_schema(kinds, offers::LABEL_MAX);
    tool
}

fn plan_tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Propose a plan to your person, on ONE card they answer « Prévoir » (set it up) or « Non merci »: \
when they speak of something to come in their own life — a trip, an appointment, a habit they want — give the moments \
that would help (a reminder the day before, check-in opening, when to leave, their room ready…), each with the words \
you'll say then. Nothing runs before their yes; then each moment is said at its time, as written, and nothing is ever \
sent, booked or paid. Never for the past, a « maybe », someone else's life, or what they just refused; at most one plan \
per reply. Say it in one sentence in your reply; the card shows the rest.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": KINDS},
                "title": {"type": "string", "description": "a few words: « Lisbonne », « Dentiste », « Boire plus d'eau »"},
                "zone": {"type": "string", "description": "the plan's IANA zone (Europe/Lisbon); default your person's"},
                "city": {"type": "string", "description": "the place's name in your person's language, for « heure de Lisbonne »"},
                "moments": {"type": "array", "maxItems": MAX_MOMENTS, "description": "1 to 6 (a habit: 1, with repeat)",
                    "items": {"type": "object", "properties": {
                        "at": {"type": "string", "description": "YYYY-MM-DDTHH:MM in the moment's zone"},
                        "zone": {"type": "string", "description": "its IANA zone when not the plan's"},
                        "what": {"type": "string", "description": "its line on the card (≤ 80)"},
                        "say": {"type": "string", "description": "what you'll say then, plain text (≤ 240), in your person's language"},
                        "link": {"type": "string", "description": "an https link added to it"},
                        "urgent": {"type": "boolean", "description": "only for a moment that must be on time, else \
the thing is missed: when to leave, a check-in opening. It reaches the phone at the system's urgent level, through Do Not \
Disturb. 2 at most per plan (a route counts), never for a habit"},
                        "route": {"type": "object", "description": "instead of at/what/say: the bridge times the way and says when to leave (looked up only once your person says « Prévoir »)",
                            "properties": {"to": {"type": "string"}, "arrive_by": {"type": "string"},
                                           "mode": {"type": "string", "enum": MODES},
                                           "from": {"type": "string", "description": "default: your person's home \
(their contact details' entry labelled as home), timed once they say « Prévoir »"}},
                            "required": ["to", "arrive_by"]}}}},
                "repeat": {"type": "object", "description": "a habit's rhythm, here beside moments",
                    "properties": {"days": {"description": "daily, weekdays, weekends, or [mon, wed, fri]"},
                                   "times": {"type": "array", "items": {"type": "string"}, "description": "HH:MM, 1 to 4"},
                                   "weeks": {"type": "integer", "minimum": 1, "maximum": WEEKS_MAX, "description": "default 2"}}},
                "watch": {"type": "object", "description": "e-mails to watch for a change (an airline's, a hotel's), by \
their sender's domain: you'll see only their sender, subject and date",
                    "properties": {"mail_from": {"type": "array", "maxItems": SENDERS_MAX, "items": {"type": "object",
                        "properties": {"name": {"type": "string"}, "domain": {"type": "string"}}}}}},
                "event": {"type": "object", "description": "the event of your person's calendar it is about, its \
id as calendar_list gives it (phone: their phone's calendar; google: Google Calendar; caldav: a mailbox's calendar). One \
that doesn't repeat: a repeating one is not followed",
                    "properties": {"source": {"type": "string", "enum": EVENT_SOURCES}, "id": {"type": "string"}}},
            },
            "required": ["kind", "title", "moments"],
        },
    })
}

/// What a call made: the agent's answer, and the card's push (its words, its id).
pub struct Called {
    pub result: Dict,
    pub card: Option<(String, String)>,
    /// The offers made with it, shown on its card (plans/offers.rs): their ids.
    pub offers: Vec<String>,
}

fn refusal(text: &str) -> Called {
    Called { result: tool_text(text, true), card: None, offers: Vec::new() }
}

/// A place found by the geocoder (map_image's), coordinates only.
async fn locate(config: &Config, place: &str) -> Result<(f64, f64), String> {
    let params = [("q", place), ("format", "jsonv2"), ("limit", "1")];
    let answer = crate::weather::nominatim(config, "search", &params).await?;
    crate::maps::found_of(&answer).map(|f| (f.lat, f.lon)).ok_or_else(|| format!("« {place} » was not found"))
}

/// Decimals of the points the router gets: 3, about 100 m (110 m north-south, 75 m east-west in Paris). The router
/// snaps them to the nearest street; a minute's walk more or less at most, inside the 5 minutes kept (review 2, R9).
pub const ROUTED_DECIMALS: i32 = 3;

/// A point as the router gets it (`ROUTED_DECIMALS`).
pub fn rounded((lat, lon): (f64, f64)) -> (f64, f64) {
    let k = 10f64.powi(ROUTED_DECIMALS);
    ((lat * k).round() / k, (lon * k).round() / k)
}

/// The way's minutes by the streets (map_route.rs `seconds`: both points rounded, no path asked, nothing kept on
/// disk), rounded up; transit by car, longer.
async fn way_minutes(config: &Config, from: (f64, f64), to: (f64, f64), mode: &str) -> Result<u32, String> {
    if config.maps.streets == "off" {
        return Err("the street routing is off on this place".into());
    }
    let travel = if mode == "walking" { "walking" } else { "driving" };
    let seconds = crate::map_route::seconds(config, travel, &[rounded(from), rounded(to)]).await?;
    let seconds = if mode == "transit" { seconds * TRANSIT_FACTOR } else { seconds };
    Ok(((seconds / 60.0).ceil() as u32).max(1))
}

/// Words a « Coordonnées » entry's label (or its « Indications ») says home with, in the languages a label may be
/// written in (fr, en, de, es, it; accents and case aside). One whose label says work, someone else's home or a holiday
/// one never is (`NOT_HOME_WORDS`; review 2, R13).
const HOME_WORDS: [&str; 38] = [
    // fr
    "domicile", "maison", "chez moi", "perso", "personnel", "personnelle", "appartement", "appart", "mon adresse",
    "residence", // en too
    // en
    "home", "house", "personal", "apartment", "flat", "my address",
    // de
    "zuhause", "zu hause", "daheim", "wohnung", "privat", "personlich", "meine adresse", "wohnsitz",
    // es
    "casa", "hogar", "domicilio", "piso", "apartamento", "departamento", "mi direccion", "residencia",
    // it
    "abitazione", "personale", "appartamento", "il mio indirizzo", "residenza", "mia casa",
];
const NOT_HOME_WORDS: [&str; 49] = [
    // work
    "pro", "travail", "bureau", "boulot", "work", "office", "arbeit", "buro", "trabajo", "oficina", "lavoro", "ufficio",
    // a holiday or second home
    "vacances", "secondaire", "campagne", "holiday", "holidays", "vacation", "second", "cottage", "ferien", "urlaub",
    "zweitwohnung", "vacaciones", "segunda", "vacanze", "vacanza", "seconda", "mare",
    // someone else's
    "mere", "pere", "parents", "frere", "soeur", "mother", "mom", "mum", "father", "dad", "mutter", "vater",
    "eltern", "madre", "padre", "padres", "mamma", "papa", "genitori", "nonna",
];

/// `text` says one of `words` (whole words, accents and case aside).
fn says_any(text: &str, words: &[&str]) -> bool {
    let folded = crate::tools_knowledge::ascii_fold(&text.to_lowercase()).replace(|c: char| !c.is_alphanumeric(), " ");
    let folded = format!(" {} ", folded.split_whitespace().collect::<Vec<_>>().join(" "));
    words.iter().any(|w| folded.contains(&format!(" {w} ")))
}

/// The person's home among the « Coordonnées » entries an agent reads: the first one with an address whose label (or
/// else its « Indications ») says home, never just the first entry (review 1, R1: maybe the office). Its address on one
/// line (« 3 rue du Test, 69001 Lyon, FR »).
pub fn home_of(entries: &[crate::vault_contacts::Entry]) -> Option<String> {
    let address = |e: &crate::vault_contacts::Entry| {
        let field = |k: &str| e.secret.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let line = field("address_line1");
        (!line.is_empty()).then(|| {
            [line, format!("{} {}", field("postal_code"), field("city")).trim().to_string(), field("country")]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        })
    };
    let home = |e: &&crate::vault_contacts::Entry| {
        !says_any(&e.label, &NOT_HOME_WORDS)
            && (says_any(&e.label, &HOME_WORDS) || e.hint.as_deref().is_some_and(|h| says_any(h, &HOME_WORDS)))
    };
    entries.iter().filter(home).find_map(address)
}

/// The person's home address `agent` may read (`home_of`): read here, sent nowhere.
fn home_address(state: &AppState, config: &Config, agent: &str) -> Option<String> {
    home_of(&state.vault.contacts_for(config, agent).ok()?)
}

/// When to leave: the arrival less the way and the margin, rounded down to 5 minutes.
fn leave_at(arrive: f64, minutes: u32) -> f64 {
    ((arrive - (f64::from(minutes) + MARGIN_MINUTES) * 60.0) / 300.0).floor() * 300.0
}

/// Each route moment written by the bridge, to be found and timed at « Prévoir » only (`time_legs`; review 2, R9):
/// while the card waits nothing goes to a geocoder or a router, neither the place to go nor where from (the agent's
/// `from`, else the person's home: only checked here, on the place, that there is one). The card says « avant 15:00 ».
fn resolve_routes(state: &AppState, config: &Config, draft: &mut Draft, lang: &str) -> Result<(), String> {
    let routes = std::mem::take(&mut draft.routes);
    for r in routes {
        if r.from.is_none() && home_address(state, config, &draft.plan.agent).is_none() {
            return Err("route: no start address: none of your person's contact details you may read is labelled as \
                        their home (« Domicile », « Perso », « Home »…): give route.from (like the rest of the route, \
                        looked up only once your person says « Prévoir »), or leave the route out"
                .into());
        }
        if config.maps.streets == "off" {
            return Err("route: the street routing is off on this place".into());
        }
        let m = &mut draft.plan.moments[r.moment];
        let words = route_pending_words(lang, &r.to, &r.mode);
        let leg = Leg { place: r.to.clone(), from: r.from.clone(), mode: r.mode.clone(), arrive: r.arrive };
        (m.at, m.what, m.say, m.urgent, m.leg) = (r.arrive, words.clone(), words, true, Some(leg));
    }
    Ok(())
}

/// The way's minutes from the leg's `from` (else the person's home, `home`; None: gone from « Coordonnées ») to its
/// place: each found by the geocoder (cached in memory), then the router.
async fn leg_minutes(config: &Config, home: Option<&str>, leg: &Leg) -> Result<u32, String> {
    let start = leg.from.as_deref().or(home).ok_or("no home address any more")?;
    let to = locate(config, &leg.place).await?;
    let from = locate(config, start).await?;
    way_minutes(config, from, to, &leg.mode).await
}

/// « Prévoir » to a plan with routes from home: each one timed now (the home's address to the geocoder, then its
/// coordinates to the router) and written into the waiting plan; those not timed (the home gone from « Coordonnées »,
/// not found, no way) returned (their moment id and place): left out, said in one line.
async fn time_legs(state: &AppState, config: &Config, plan: &Plan, skip: &[String], lang: &str) -> Vec<(String, String)> {
    let legs: Vec<(String, Leg)> = plan
        .moments
        .iter()
        .filter(|m| !skip.contains(&m.id))
        .filter_map(|m| m.leg.clone().map(|l| (m.id.clone(), l)))
        .collect();
    if legs.is_empty() {
        return Vec::new();
    }
    let home = if legs.iter().all(|(_, l)| l.from.is_some()) { None } else { home_address(state, config, &plan.agent) };
    let mut timed = Vec::new();
    let mut failed = Vec::new();
    for (id, leg) in legs {
        match leg_minutes(config, home.as_deref(), &leg).await {
            Ok(minutes) => timed.push((id, leave_at(leg.arrive, minutes), route_words(lang, &leg.place, minutes, &leg.mode))),
            Err(why) => {
                tracing::info!(target: "bipbridge::plans", plan = %plan.id, moment = %id, why = %why, "route not timed");
                failed.push((id, leg.place));
            }
        }
    }
    change(config, |store| {
        let Some(p) = store.plans.iter_mut().find(|p| p.id == plan.id && p.state == WAITING) else { return };
        for (id, at, words) in &timed {
            if let Some(m) = p.moments.iter_mut().find(|m| &m.id == id) {
                (m.at, m.what, m.say, m.leg) = (*at, words.clone(), words.clone(), None);
            }
        }
    });
    failed
}

/// The first mailbox `agent` reads (an e-mail trigger's).
fn mailbox_of(config: &Config, agent: &str) -> Option<String> {
    let accounts = crate::mailbox::of(config).read().ok()?;
    accounts.iter().find_map(|a| {
        crate::tools_knowledge::py_in(agent, a.get("agents").unwrap_or(&json!([])))
            .then(|| crate::tools_knowledge::str_or_empty(a.get("id")))
            .filter(|id| !id.is_empty())
    })
}

/// When an event of the phone's calendar copy starts (Unix seconds; an all-day one: its day's midnight, `zone`).
fn event_start(event: &Value, zone: Tz) -> Option<f64> {
    let start = event.get("start").and_then(Value::as_str)?;
    if let Ok(d) = DateTime::parse_from_rfc3339(start) {
        return Some(d.timestamp() as f64);
    }
    if let Ok(day) = NaiveDate::parse_from_str(start, "%Y-%m-%d") {
        return zone.from_local_datetime(&day.and_hms_opt(0, 0, 0)?).earliest().map(|d| d.timestamp() as f64);
    }
    parse_at(start, zone)
}

/// `plan_propose` by `agent`; `asked`: in a turn of its person's own conversation, `run` that turn.
pub async fn call(state: &AppState, agent: &AgentConfig, arguments: &Dict, asked: bool, run: Option<&str>) -> Called {
    let config = state.config();
    if !crate::agent_proactive::proposes(&config, &agent.name) {
        return refusal(
            "Not done: your person turned your suggestions off (« Initiative », at the bottom of your page), or other \
             people talk to you: propose nothing on your own. Do what they ask.",
        );
    }
    if !crate::routines::enabled(&config, &agent.name) {
        return refusal("Not done: your routines are off, so a plan can't remind anything. Don't propose one.");
    }
    let now = py::time_time();
    let (person, routines) = (person_zone(&config), crate::routines::zone(&config));
    let mut draft = match draft(arguments, &agent.name, person, routines, now) {
        Ok(d) => d,
        Err(why) => return refusal(&format!("Not done: {why}. Nothing was made.")),
    };
    let lang = lang_of(&config, &agent.name);
    if let Err(why) = resolve_routes(state, &config, &mut draft, lang) {
        return refusal(&format!("Not done: {why}. Nothing was made."));
    }
    // its offers (plans/offers.rs): only the kinds offer_act lists for this agent; a wrong one refuses the plan
    let kinds = offers::kinds_for(&config, agent);
    let specs = arguments.get("offers").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut pills = match offers::plan_offers(&specs, &agent.name, &draft.plan.title, &kinds, person, now) {
        Ok(p) => p,
        Err(why) => return refusal(&format!("{why} Nothing was made.")),
    };
    for (i, m) in draft.plan.moments.iter().enumerate() {
        let Some(spec) = &m.offer else { continue };
        if let Err(why) = offers::spec_of(spec, &agent.name, &draft.plan.title, &kinds, person, now, offers::LABEL_MAX) {
            let why = why.strip_prefix("Not done: ").unwrap_or(&why).to_string();
            return refusal(&format!("Not done: moment {}: offer: {why} Nothing was made.", i + 1));
        }
    }
    if let Err(why) = offers::make_pills(state, agent, &mut pills, asked, run, person, now) {
        return refusal(&format!("{why} Nothing was made."));
    }
    let mut notes = Vec::new();
    let mut plan = draft.plan;
    if let Some(event) = plan.event.clone() {
        let found = match event.source.as_str() {
            "phone" if crate::calendar::reads(&config, &agent.name) => {
                phone_event(&crate::calendar::read(&config), &event.id, person)
            }
            // Google's and CalDAV's: read now from the calendars this agent reads (plans/sources.rs)
            "google" | "caldav" if sources::reads_calendar(&config, &agent.name) => {
                sources::remote_event(&config, &agent.name, &event.id, person).await
            }
            _ => PhoneEvent::Missing,
        };
        match found {
            PhoneEvent::Found { start, calendar } => plan.event = Some(EventRef { start, calendar, ..event }),
            PhoneEvent::Repeating => {
                // every occurrence has the same id on both phones: which one is the plan's can't be told
                notes.push(
                    "Not followed: this event repeats in your person's calendar (its occurrences share one id), so \
                     the plan doesn't follow it: if it moves or is cancelled, its reminders stay as they are."
                        .into(),
                );
                plan.event = None;
            }
            PhoneEvent::Missing => {
                return refusal("Not done: event: no such event in your person's calendars that you may read (its id as \
                                calendar_list gives it). Propose the plan without `event`, or not at all.")
            }
        }
    }
    if !plan.watch.is_empty() {
        let names: Vec<String> = plan.watch.iter().map(|s| s.name.clone()).collect();
        let why = if mailbox_of(&config, &agent.name).is_none() {
            Some("you read no mailbox here (no mailbox given to you)")
        } else if crate::triggers::of_agent(&config, &agent.name).len() >= crate::triggers::MAX_PER_AGENT {
            Some("you have as many routines that start on an e-mail or a link as you may")
        } else {
            None
        };
        if let Some(why) = why {
            notes.push(format!("Not watched: {why}, so the e-mails of {} are not watched.", names.join(", ")));
            plan.watch.clear();
        }
    }
    // a trip card's « Oui » (plans/sources.rs `grant`): the plan proposed now is the person's ask
    let granted = !asked && sources::granted(&config, &agent.name, now);
    let asked = asked || granted;
    plan.asked = asked;
    // unasked: its offers' cards count with the plans' (one waiting, the day's caps)
    let offered = offers::load(&config);
    let made = change(&config, |store| {
        if let Some(why) = refused(store, &plan, run, person, now) {
            return Err(why);
        }
        if !asked {
            if let Some(why) = offers::unasked_refusal(&offered, store, &plan.agent, person, now) {
                return Err(why);
            }
        }
        plan.first = !store.hinted.contains(&plan.agent);
        if plan.first {
            store.hinted.push(plan.agent.clone());
        }
        store.made.push(Made { agent: plan.agent.clone(), at: now, asked, run: run.map(str::to_string) });
        store.plans.push(plan.clone());
        Ok(plan.clone())
    });
    let plan = match made {
        Ok(plan) => plan,
        Err(why) => return refusal(&why),
    };
    if granted {
        sources::use_grant(&config, &agent.name);
    }
    tracing::info!(target: "bipbridge::plans", agent = %agent.name, plan = %plan.id, kind = %plan.kind,
                   moments = plan.moments.len(), asked, "plan card made");
    let mut text = format!(
        "Plan card made: « {} », {} moment{}. It waits for your person's « Prévoir » or « Non merci »: nothing runs \
         before, and then each moment is said at its time as you wrote it. Don't ask them again in words.",
        plan.title,
        count(&plan),
        if count(&plan) == 1 { "" } else { "s" }
    );
    for note in notes {
        text.push(' ');
        text.push_str(&note);
    }
    let pills = offers::keep_pills(&config, &pills, &plan.id, asked, run, now);
    if !pills.is_empty() {
        text.push_str(&format!(
            " Its card shows your {} offer{} too: done only if your person taps it.",
            pills.len(),
            if pills.len() == 1 { "" } else { "s" }
        ));
    }
    let line = notification(lang, &plan.title);
    Called { result: tool_text(&text, false), card: Some((line, plan.id)), offers: pills }
}

// -- the answer ---------------------------------------------------------------------------------------------------------

/// The person answered the plan's card: « Prévoir » (`watch`: its e-mail watch made, None when it watches nothing), or
/// « Non merci ».
pub enum Answer {
    Set { kept: usize, of: usize, passed: usize, watch: Option<bool> },
    Declined,
}

/// A plan's card answered (`/v1/approve`): « Prévoir » (`once`, `session`, `always`) with the moments `skip`ped,
/// or « Non merci » (a refusal); 422 for any other word, 404 for no such card waiting.
pub async fn approve(state: &AppState, agent: &AgentConfig, id: &str, choice: &str, skip: &[String]) -> ApiResult<axum::response::Response> {
    let choice = choice.trim().to_lowercase();
    let yes = matches!(choice.as_str(), "once" | "session" | "always");
    if !yes && !crate::routines::refuses(&choice) {
        return Err(HttpError::new(422, "choice: once or deny"));
    }
    let config = state.config();
    let place = crate::card_place::of(&config, &format!("{RUN_PREFIX}{id}")).unwrap_or_default();
    let waiting = get(&config, id).filter(|p| p.agent == agent.name && p.state == WAITING);
    // where the card showed for the person (card_place.rs `report`): the conversation they asked in; none, the main
    // one, for a card nobody asked for (a routine's run, another agent's ask in their pair's hidden conversation):
    // its moments and lines never go to a hidden conversation (review 1, B1)
    let asked = waiting.as_ref().is_some_and(|p| p.asked);
    let origin = if asked { place.report.unwrap_or_default() } else { String::new() };
    let lang = lang_of(&config, &agent.name);
    let mut skip = skip.to_vec();
    // a route from home: timed now, never before « Prévoir » (R1)
    let untimed = match (&waiting, yes) {
        (Some(plan), true) => time_legs(state, &config, plan, &skip, lang).await,
        _ => Vec::new(),
    };
    skip.extend(untimed.iter().map(|(id, _)| id.clone()));
    let (c, name, plan_id, kept_skip) = (config.clone(), agent.name.clone(), id.to_string(), skip.clone());
    let answered =
        tokio::task::spawn_blocking(move || answer(&c, &name, &plan_id, yes, &kept_skip, &origin, py::time_time()))
            .await
            .map_err(anyhow::Error::from)?;
    match answered {
        Some(Answer::Set { kept, of, passed, watch }) => {
            // what the card announced and couldn't be set up: one line each, no model
            if let Some(plan) = get(&config, id) {
                for (_, place) in &untimed {
                    tell(state, &plan, route_failed_line(lang, place)).await;
                }
                if watch == Some(false) {
                    let senders = waiting.as_ref().map(|p| p.watch.clone()).unwrap_or_default();
                    tell(state, &plan, watch_failed_line(lang, &senders)).await;
                }
            }
            let mut body = json!({"ok": true, "choice": "once", "moments": kept, "of": of, "passed": passed});
            if let Some(watch) = watch {
                body["watch"] = json!(watch);
            }
            Ok(web::ok(body))
        }
        Some(Answer::Declined) => Ok(web::ok(json!({"ok": true, "choice": "deny"}))),
        None => Err(HttpError::new(404, "no such request (answered already?)")),
    }
}

/// The answer kept: « Prévoir » makes the routines (and the trigger) and sets the plan; « Non merci » forgets it, its
/// title's print kept.
pub fn answer(config: &Config, agent: &str, id: &str, yes: bool, skip: &[String], origin: &str, now: f64) -> Option<Answer> {
    let taken = change(config, |store| {
        let at = store.plans.iter().position(|p| p.agent == agent && p.id == id && p.state == WAITING)?;
        if !yes {
            let plan = store.plans.remove(at);
            store.declined.push(Print { agent: plan.agent.clone(), print: print_of(&plan.agent, &plan.title), at: now });
            return Some(Some(plan));
        }
        let plan = &mut store.plans[at];
        plan.state = SET.into();
        plan.origin = origin.to_string();
        Some(Some(plan.clone()))
    })?;
    let plan = taken?;
    if !yes {
        // its offers shown on its card go with it (plans/offers.rs)
        offers::withdraw_plan(config, &plan.id);
        tracing::info!(target: "bipbridge::plans", agent, plan = id, "plan declined");
        return Some(Answer::Declined);
    }
    let (made, kept, passed) = set_up(config, &plan, skip, now);
    let of = plan.moments.len();
    let ends = match &plan.repeat {
        Some(r) => first_midnight(r, crate::routines::zone(config)) + f64::from(r.weeks) * 7.0 * 86400.0 + END_AFTER_SECONDS,
        None => plan.moments.iter().map(|m| m.at).fold(0.0, f64::max) + END_AFTER_SECONDS,
    };
    change(config, |store| {
        if let Some(p) = store.plans.iter_mut().find(|p| p.id == plan.id) {
            p.routines = made.0.clone();
            p.trigger = made.1.clone();
            p.ends = ends;
            for (moment, routine) in &made.2 {
                if let Some(m) = p.moments.iter_mut().find(|m| &m.id == moment) {
                    m.routine = Some(routine.clone());
                }
            }
        }
    });
    let watch = (!plan.watch.is_empty()).then_some(made.1.is_some());
    tracing::info!(target: "bipbridge::plans", agent, plan = id, kept, passed, ?watch, "plan set");
    Some(Answer::Set { kept, of, passed, watch })
}

/// The routines, the trigger and the moments that offer (their moment and routine ids) of a plan set up.
type SetUp = (Vec<String>, Option<String>, Vec<(String, String)>);

/// « Prévoir »: each moment kept a routine (a habit, its routines on a clock), the watch its trigger. ((routines,
/// trigger, moments that offer), kept, passed)
fn set_up(config: &Config, plan: &Plan, skip: &[String], now: f64) -> (SetUp, usize, usize) {
    let mut routines = Vec::new();
    let mut offered = Vec::new();
    let (mut kept, mut passed) = (0, 0);
    let origin = Some(plan.origin.as_str()).filter(|o| !o.is_empty());
    for m in &plan.moments {
        if skip.contains(&m.id) {
            continue;
        }
        let name: String = format!("{} · {}", plan.title, m.what).chars().take(crate::routines::NAME_MAX).collect();
        if let Some(r) = &plan.repeat {
            let after = first_midnight(r, crate::routines::zone(config)).max(now) - 1.0;
            for (expr, runs) in habit_schedules(r, approx_shift(&plan.id)) {
                let made = crate::routines::create_said(
                    config,
                    &plan.agent,
                    &name,
                    crate::routines::Schedule::Cron { expr },
                    &m.say,
                    m.urgent,
                    &plan.id,
                    origin,
                    Some(runs),
                    after,
                );
                routines.push(made.id);
            }
            kept += 1;
            continue;
        }
        if m.at <= now {
            passed += 1;
            continue;
        }
        // a moment that offers: its routine carries its line on the card, its card placed at its time
        // (plans/offers.rs `at_moment`), never a turn
        let words = if m.offer.is_some() { &m.what } else { &m.say };
        let made = crate::routines::create_said(
            config,
            &plan.agent,
            &name,
            crate::routines::Schedule::Once { at: m.at },
            words,
            m.urgent,
            &plan.id,
            origin,
            Some(1),
            now,
        );
        if m.offer.is_some() {
            offered.push((m.id.clone(), made.id.clone()));
        }
        routines.push(made.id);
        kept += 1;
    }
    let trigger = (!plan.watch.is_empty()).then(|| watch_trigger(config, plan)).flatten();
    ((routines, trigger, offered), kept, passed)
}

/// The senders' e-mails watched: an e-mail trigger on their domains, on at once (the person said « Prévoir »).
fn watch_trigger(config: &Config, plan: &Plan) -> Option<String> {
    let account = mailbox_of(config, &plan.agent)?;
    let names: Vec<String> = plan.watch.iter().map(|s| s.name.clone()).collect();
    let domains: Vec<String> = plan.watch.iter().map(|s| s.domain.clone()).collect();
    let fields = crate::triggers::Fields {
        name: Some(format!("{} · {}", plan.title, names.join(", ")).chars().take(crate::triggers::NAME_MAX).collect()),
        prompt: Some(format!(
            "An e-mail from {} about the plan « {} » you set up with your person: you see its sender, subject and date \
             only, never its text, and you don't open it (the mailbox's tools are closed here). If they say the plan \
             changed (a new time, a gate, a delay, a cancellation), tell them in one short line what the subject says; \
             else answer [SILENT].",
            names.join(", "),
            plan.title
        )),
        kind: Some(crate::triggers::MAIL.into()),
        mail: Some(crate::triggers::MailFilter { account, from: domains.join(", "), words: String::new() }),
        enabled: None,
        // headers only: a few k tokens (§3.8), the smallest budget
        budget_tokens: Some(crate::triggers::BUDGET_MIN),
    };
    match crate::triggers::create(config, &plan.agent, &fields, MAKER) {
        Ok(t) => Some(t.id),
        Err(why) => {
            tracing::warn!(target: "bipbridge::plans", plan = %plan.id, why = %why, "plan's e-mail watch not made");
            None
        }
    }
}

/// A plan gone, however (its event cancelled, its end, « Initiative » off): its routines, its trigger and its offers
/// still waiting with it (review R5), counted for nothing.
fn remove(config: &Config, plan: &Plan) {
    offers::withdraw_plan(config, &plan.id);
    for id in &plan.routines {
        crate::routines::delete(config, id);
    }
    if let Some(t) = &plan.trigger {
        crate::triggers::remove(config, t);
    }
    change(config, |store| store.plans.retain(|p| p.id != plan.id));
}

// -- what it watches, and its end --------------------------------------------------------------------------------------

/// What the phone's calendar copy says of a plan's event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventNews {
    Same,
    Moved { start: f64, delta: f64 },
    Cancelled,
}

/// What the phone's copy says of an event a plan would follow (`plan_propose`'s `event`).
#[derive(Debug, Clone, PartialEq)]
pub enum PhoneEvent {
    /// One event with that id: its start and its calendar.
    Found { start: f64, calendar: String },
    /// It repeats (marked `recurring`, or several occurrences with that id: both phones give every occurrence the
    /// same id, iPhone `CalendarSync.swift` `eventIdentifier`, Android `CalendarSync.kt` `EVENT_ID`): which one is the
    /// plan's can't be told, nor a moved occurrence from a cancelled one (Android gives a moved one an id of its own).
    Repeating,
    Missing,
}

fn same_id<'a>(copy: &'a Dict, id: &str) -> Vec<&'a Value> {
    copy.get("events")
        .and_then(Value::as_array)
        .map(|events| events.iter().filter(|e| e.get("id").and_then(Value::as_str) == Some(id)).collect())
        .unwrap_or_default()
}

fn repeats(events: &[&Value]) -> bool {
    events.len() > 1 || events.iter().any(|e| e.get("recurring").is_some_and(py::truthy))
}

/// The event `id` of the phone's copy, for a plan to follow: a repeating one never (review 1, B2).
pub fn phone_event(copy: &Dict, id: &str, zone: Tz) -> PhoneEvent {
    let events = same_id(copy, id);
    if repeats(&events) {
        return PhoneEvent::Repeating;
    }
    let calendar = |e: &Value| e.get("calendar").and_then(Value::as_str).unwrap_or("").to_string();
    match events.first().and_then(|e| Some((event_start(e, zone)?, calendar(e)))) {
        Some((start, calendar)) => PhoneEvent::Found { start, calendar },
        None => PhoneEvent::Missing,
    }
}

/// The plan's event in the copy (calendar.rs `read`): moved, gone from a copy that covers its day (its calendar still
/// in it: a calendar the person took out of the copy cancels nothing, R5), or the same. One that became a repeating
/// event since: only an occurrence on its own day is its own (never a move of a week, B2); none there, nothing said.
pub fn event_news(event: &EventRef, copy: &Dict, zone: Tz) -> EventNews {
    let events = same_id(copy, &event.id);
    let day_of = |at: f64| zone.timestamp_opt(at as i64, 0).single().map(|d| d.date_naive());
    let mut starts = events.iter().filter_map(|e| event_start(e, zone));
    let found = if repeats(&events) {
        starts
            .filter(|s| day_of(*s) == day_of(event.start))
            .min_by(|a, b| (a - event.start).abs().total_cmp(&(b - event.start).abs()))
    } else {
        starts.next()
    };
    match found {
        Some(start) if (start - event.start).abs() >= MOVED_MIN_SECONDS => {
            EventNews::Moved { start, delta: start - event.start }
        }
        Some(_) => EventNews::Same,
        None if !events.is_empty() => EventNews::Same,
        None => {
            let listed = copy.get("calendars").and_then(Value::as_array);
            let taken_out = !event.calendar.is_empty()
                && listed.is_some_and(|l| !l.iter().any(|c| c.get("id").and_then(Value::as_str) == Some(&event.calendar)));
            let day = |k: &str| copy.get(k).and_then(Value::as_str).and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok());
            match (day("from"), day("to"), day_of(event.start)) {
                (Some(from), Some(to), Some(at)) if from <= at && at <= to && !taken_out => EventNews::Cancelled,
                _ => EventNews::Same,
            }
        }
    }
}

/// A set plan past its end (a day after its last moment): it goes without a word.
pub fn over(plan: &Plan, now: f64) -> bool {
    plan.state == SET && plan.ends > 0.0 && now > plan.ends
}

/// A card still waiting when all its moments have passed: it goes without a word.
pub fn lapsed(plan: &Plan, now: f64) -> bool {
    plan.state == WAITING && plan.repeat.is_none() && plan.moments.iter().all(|m| m.at <= now)
}

/// One line in the plan's conversation (else the main one: never a hidden one, card_place.rs `report_of`), pushed: no
/// model.
async fn tell(state: &AppState, plan: &Plan, text: String) {
    let config = state.config();
    let Some(agent) = config.agent(Some(&plan.agent)).cloned() else { return };
    let pipeline = Pipeline::of(state);
    let origin = match plan.origin.as_str() {
        "" => None,
        o => crate::card_place::report_of(state, &agent, o).await,
    };
    let session = match origin {
        Some(o) => o,
        None => pipeline.tip(&agent, "main").await,
    };
    let delivery = Delivery {
        agent,
        session,
        key: format!("plan:{}:{}", plan.id, py::time_time() as i64),
        title: plan.title.clone(),
        text,
        as_written: false,
        run_id: None,
        notify: true,
        files: Vec::new(),
    };
    pipeline.deliver(&delivery).await;
}

/// The plan's moments moved by `delta` (its routines with them), its event's start now `start`.
fn shift(config: &Config, plan: &Plan, start: f64, delta: f64) {
    change(config, |store| {
        if let Some(p) = store.plans.iter_mut().find(|p| p.id == plan.id) {
            for m in p.moments.iter_mut() {
                m.at += delta;
                if let Some(leg) = m.leg.as_mut() {
                    leg.arrive += delta;
                }
            }
            if let Some(e) = p.event.as_mut() {
                e.start = start;
            }
            if p.ends > 0.0 {
                p.ends += delta;
            }
        }
    });
    for id in &plan.routines {
        crate::routines::shift_once(config, id, delta);
    }
}

/// One look: plans at their end, cards whose moments all passed, the phone events moved or cancelled.
pub async fn tick(state: &AppState) {
    let config = state.config();
    if !path(&config).exists() {
        return;
    }
    let now = py::time_time();
    let plans = load(&config).plans;
    if plans.is_empty() {
        return;
    }
    let mut gone = Vec::new();
    for plan in &plans {
        let (over, lapsed) = (over(plan, now), lapsed(plan, now));
        // « Initiative » turned off: its cards still waiting withdrawn without a word (§2.1); what was set goes on
        let withdrawn = plan.state == WAITING && !crate::agent_proactive::proposes(&config, &plan.agent);
        if over || lapsed || withdrawn {
            tracing::info!(target: "bipbridge::plans", agent = %plan.agent, plan = %plan.id, over, lapsed, withdrawn,
                           "plan gone");
            remove(&config, plan);
            gone.push(plan.id.clone());
        }
    }
    let watched: Vec<&Plan> = plans
        .iter()
        .filter(|p| !gone.contains(&p.id) && p.event.as_ref().is_some_and(|e| e.source == "phone"))
        .collect();
    if watched.is_empty() {
        return;
    }
    // the phone's copy read again only when the phone sent another one (its file changed)
    let changed = std::fs::metadata(PathBuf::from(config.data_dir()).join("calendar.json")).and_then(|m| m.modified()).ok();
    {
        let mut seen = COPY_SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if changed.is_some() && *seen == changed {
            return;
        }
        *seen = changed;
    }
    let copy = crate::calendar::read(&config);
    let person = person_zone(&config);
    for plan in watched {
        let Some(event) = &plan.event else { continue };
        apply_news(state, plan, event_news(event, &copy, person), person).await;
    }
}

/// What the calendar says of a plan's event, done: moved, its moments move by as much, one line; cancelled, the plan
/// goes, one line; no model either way (the phone's copy here, Google's and CalDAV's: plans/sources.rs `follow`).
async fn apply_news(state: &AppState, plan: &Plan, news: EventNews, person: Tz) {
    let config = state.config();
    let lang = lang_of(&config, &plan.agent);
    match news {
        EventNews::Same => {}
        EventNews::Moved { start, delta } => {
            shift(&config, plan, start, delta);
            tracing::info!(target: "bipbridge::plans", plan = %plan.id, delta, "plan's event moved");
            if plan.state == SET {
                tell(state, plan, moved_line(lang, &plan.title, start, person)).await;
            }
        }
        EventNews::Cancelled => {
            remove(&config, plan);
            tracing::info!(target: "bipbridge::plans", plan = %plan.id, "plan's event cancelled");
            if plan.state == SET {
                tell(state, plan, cancelled_line(lang, &plan.title)).await;
            }
        }
    }
}

/// Every `TICK_SECONDS`: `tick`.
pub async fn watch(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(TICK_SECONDS)).await;
        tick(&state).await;
        // the offers' end: expired, or their agent turned off (plans/offers.rs)
        offers::tick(&state).await;
        // the calendars, the Carnet's trips and the replies due (plans/sources.rs)
        sources::tick(&state).await;
    }
}

/// Offers to act (`offer_act`, the `bip-offer-` card; plans/offers.rs).
pub mod offers;

/// The sources the bridge reads itself: the calendars, the Carnet's trips, the replies due (plans/sources.rs).
pub mod sources;

#[cfg(test)]
mod tests;
