//! The frame of what the agents do on their own (proactive agents, part 4: passation-pro
//! briefs/design-agents-proactifs.md §4; contracts/task-frame/cases.json; Alex 22/11: Q6 A, Q7 A, Q8 A). Every routine
//! (routines.rs), watcher (watchers.rs, connectors.rs) and trigger (triggers.rs) has:
//!
//! - an owner: the agent that does it, the person it is for (« you », or the family member who asked for it);
//! - an end: a date, a number of runs, its plan's end, « once » (a one-time routine; a watcher that stops once it told
//!   its person), or « until you stop it ». What its agent sets up on its own (not in a turn of the person's own
//!   conversation) with no end given ends `DEFAULT_END_DAYS` after; `KEEP_ASK_DAYS` before, a card « Garder 3 mois de
//!   plus ? » (`KEEP`). An end reached goes quietly: one Boîte line, not pushed, for what ran more than a week;
//! - a place where it answers: the conversation it was made in, the Boîte, or silently on its page (`deliver`).
//!
//! A « Oui » to a one-time thing is one time; a repeat asks its own « Oui », its card saying its rhythm and its end
//! (routines.rs `repeat_sentence`). The agent's routines run « vers » their time: a stable shift of −10 to +10 minutes
//! drawn from their id (`approx_seconds`); the person's stay exact.
//!
//! What fails is repaired in its frame or stopped with a word. A routine's scheduled run that fails a first time is
//! tried again in silence `RETRY_SECONDS` later; failed again, today's notice; `FAILURES_MAX` failed runs in a row: it
//! is paused, and the card « Réparer / Arrêter » asks (`bip-fix-<id>`, kind `task_fix`, critical: « Réparer » starts a
//! paid turn). A watcher « en panne » asks the same card in place of the bridge's line, with no model call before
//! « Réparer ». « Réparer » is ONE turn of its agent in its conversation, with what failed: it can change something only
//! through the existing cards (routine_schedule's proposals, connector_update). « Arrêter » pauses, never deletes. A
//! card nobody answers for `NO_ANSWER_DAYS`: gone, the task paused, one line.
//!
//! Kept with each one, in today's files: `routines.json` (each routine's `frame`, `approx`), `connectors.json` (a
//! watcher's `watch.until`, `watch.once`, `watch.default_end` and its connector's `frame`), `triggers.json` (`ends`).
//! The clock: `tick`, from the routines' scheduler.
//!
//! No Python counterpart.

use chrono::TimeZone;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::connectors;
use crate::routines::{self, Routine};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// The card « Réparer / Arrêter » or « Garder 3 mois de plus ? » (`/v1/approvals`, `/v1/approve`).
pub const RUN_PREFIX: &str = "bip-fix-";
/// Its kind (approval_level.rs: critical at every level).
pub const KIND: &str = "task_fix";
pub const REPAIR: &str = "repair";
pub const KEEP: &str = "keep";
/// What its agent sets up on its own with no end given: ends after this (Alex Q6 A).
pub const DEFAULT_END_DAYS: f64 = 90.0;
/// « Garder »: its end this much later.
pub const KEEP_DAYS_MORE: f64 = 90.0;
/// « Garder 3 mois de plus ? » this long before a default end.
pub const KEEP_ASK_DAYS: f64 = 3.0;
/// A card « Réparer / Arrêter » nobody answers: gone after this, the task paused.
pub const NO_ANSWER_DAYS: f64 = 7.0;
/// An end reached tells it (one line) only for what ran longer than this.
pub const ENDED_LINE_DAYS: f64 = 7.0;
/// A scheduled run that fails a first time is tried again this much later, in silence.
pub const RETRY_SECONDS: f64 = 900.0;
/// Failed runs in a row before the card.
pub const FAILURES_MAX: u32 = 3;
pub const DAY: f64 = 86_400.0;

/// What a frame belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Routine,
    Watcher,
}

impl Task {
    pub fn key(self) -> &'static str {
        match self {
            Task::Routine => "routine",
            Task::Watcher => "watcher",
        }
    }
}

/// How it ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum End {
    /// Its last moment (Unix seconds).
    Date(f64),
    /// The runs left.
    Count(u32),
    /// With its plan (plans.rs).
    Plan,
    Once,
    /// Until its person stops it.
    Never,
}

impl End {
    pub fn kind(&self) -> &'static str {
        match self {
            End::Date(_) => "date",
            End::Count(_) => "count",
            End::Plan => "plan",
            End::Once => "once",
            End::Never => "never",
        }
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_none_f(n: &f64) -> bool {
    *n <= 0.0
}

/// A routine's or a watcher's frame, kept with it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Frame {
    /// Its end (a routine's; a watcher's is in its `watch`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<f64>,
    /// `until` set by default (its agent's own idea, no end given): « Garder 3 mois de plus ? » before it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub default_end: bool,
    /// « Garder 3 mois de plus ? » asked for this end.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub keep_asked: bool,
    /// A card waiting: « Réparer / Arrêter » or « Garder ? ».
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    /// Failed runs in a row (a routine's; a watcher's are in its runtime), since when, the last one's reason.
    #[serde(skip_serializing_if = "is_zero")]
    pub failures: u32,
    #[serde(skip_serializing_if = "is_none_f")]
    pub failing_since: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Its first failure's retry, in silence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<f64>,
    /// « Réparer » answered: its agent's proposal turns it back on with the person's « Oui ».
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub repairing: bool,
}

impl Frame {
    pub fn is_empty(&self) -> bool {
        *self == Frame::default()
    }
}

/// The card waiting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fix {
    pub id: String,
    pub at: f64,
    /// `repair` or `keep`.
    pub variant: String,
    /// Its first failure in a row.
    pub since: f64,
    pub failures: u32,
    /// `looping`, `too_long`, `failed`, `not_started` (a routine's run), `timeout`, `secret`, `check` (a watcher's).
    pub reason: String,
}

/// « vers »: whole minutes, −10 to +10, stable for one id (the same digest as a plan's habits, plans.rs).
pub fn approx_seconds(id: &str) -> f64 {
    crate::plans::approx_shift(id) as f64 * 60.0
}

/// What a failed run leads to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum After {
    /// Tried again then, in silence.
    Retry(f64),
    /// Today's notice.
    Tell,
    /// Paused, the card « Réparer / Arrêter ».
    Repair,
}

/// A run failed at `now` (`scheduled`: by its clock, not « Lancer maintenant » nor its retry; `next`: its next run):
/// counted, and what follows.
pub fn after_failure(frame: &mut Frame, now: f64, scheduled: bool, next: Option<f64>) -> After {
    frame.failures += 1;
    if frame.failing_since <= 0.0 {
        frame.failing_since = now;
    }
    frame.retry_at = None;
    if frame.failures >= FAILURES_MAX && frame.fix.as_ref().is_none_or(|f| f.variant == KEEP) {
        return After::Repair;
    }
    let retry = now + RETRY_SECONDS;
    if scheduled && frame.failures == 1 && next.is_none_or(|n| retry < n) {
        frame.retry_at = Some(retry);
        return After::Retry(retry);
    }
    After::Tell
}

/// A run that worked: the count starts again; true when a card « Réparer / Arrêter » waited (gone now).
pub fn after_success(frame: &mut Frame) -> bool {
    frame.failures = 0;
    frame.failing_since = 0.0;
    frame.reason.clear();
    frame.retry_at = None;
    frame.repairing = false;
    match &frame.fix {
        Some(f) if f.variant == REPAIR => {
            frame.fix = None;
            true
        }
        _ => false,
    }
}

/// The person turned it back on: a card « Réparer / Arrêter » goes (its id), the count starts again.
pub fn resumed(frame: &mut Frame) -> Option<String> {
    let id = frame.fix.as_ref().filter(|f| f.variant == REPAIR).map(|f| f.id.clone());
    if id.is_some() {
        frame.fix = None;
    }
    frame.failures = 0;
    frame.failing_since = 0.0;
    frame.retry_at = None;
    frame.repairing = false;
    id
}

// -- words --------------------------------------------------------------------------------------------------------------

/// fr, en, de, es; another: English.
fn lang_of(lang: &str) -> &'static str {
    match lang {
        "fr" => "fr",
        "de" => "de",
        "es" => "es",
        _ => "en",
    }
}

/// The agent's language (names.rs).
pub fn lang_of_agent(config: &Config, agent: &str) -> &'static str {
    lang_of(crate::names::names(&crate::names::agent_language(config, agent)).lang)
}

/// « vers 8 h », « around 8 AM », « gegen 8 Uhr », « hacia las 8 » (with minutes: « vers 7 h 30 », « around 7:30 AM »…).
pub fn clock(lang: &str, h: u32, m: u32) -> String {
    match (lang_of(lang), m) {
        ("fr", 0) => format!("vers {h} h"),
        ("fr", _) => format!("vers {h} h {m:02}"),
        ("de", 0) => format!("gegen {h} Uhr"),
        ("de", _) => format!("gegen {h}:{m:02} Uhr"),
        ("es", 0) => format!("hacia las {h}"),
        ("es", _) => format!("hacia las {h}:{m:02}"),
        (_, m) => {
            let (h12, half) = (if h.is_multiple_of(12) { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" });
            if m == 0 {
                format!("around {h12} {half}")
            } else {
                format!("around {h12}:{m:02} {half}")
            }
        }
    }
}

/// A day: « 30/11 », « Nov 30 », « 30.11. », « 30/11 ».
pub fn day_words(lang: &str, at: f64, zone: Tz) -> String {
    let Some(d) = zone.timestamp_opt(at as i64, 0).single() else { return String::new() };
    match lang_of(lang) {
        "en" => d.format("%b %-d").to_string(),
        "de" => d.format("%-d.%-m.").to_string(),
        _ => d.format("%-d/%-m").to_string(),
    }
}

/// « Depuis le 21/11 », « Since Nov 21 », « Seit dem 21.11. », « Desde el 21/11 »: the words after « Depuis ».
pub fn since_words(lang: &str, at: f64, zone: Tz) -> String {
    let day = day_words(lang, at, zone);
    match lang_of(lang) {
        "fr" => format!("le {day}"),
        "de" => format!("dem {day}"),
        "es" => format!("el {day}"),
        _ => day,
    }
}

/// How it ends, in words (texts.<lang>.ends).
pub fn end_words(lang: &str, task: Task, end: &End, zone: Tz) -> String {
    let watcher = task == Task::Watcher;
    let lang = lang_of(lang);
    match (end, lang) {
        (End::Date(at), "fr") => format!("jusqu'au {}", day_words(lang, *at, zone)),
        (End::Date(at), "de") => format!("bis {}", day_words(lang, *at, zone)),
        (End::Date(at), "es") => format!("hasta el {}", day_words(lang, *at, zone)),
        (End::Date(at), _) => format!("until {}", day_words(lang, *at, zone)),
        (End::Count(n), "fr") => format!("{n} fois"),
        (End::Count(n), "de") => format!("{n}-mal"),
        (End::Count(1), "es") => "1 vez".into(),
        (End::Count(n), "es") => format!("{n} veces"),
        (End::Count(1), _) => "1 time".into(),
        (End::Count(n), _) => format!("{n} times"),
        (End::Plan, "fr") => "jusqu'à la fin du plan".into(),
        (End::Plan, "de") => "bis zum Ende des Plans".into(),
        (End::Plan, "es") => "hasta que acabe el plan".into(),
        (End::Plan, _) => "until the plan ends".into(),
        (End::Once, "fr") if watcher => "une seule fois, puis il s'arrête".into(),
        (End::Once, "fr") => "une seule fois, puis elle s'arrête".into(),
        (End::Once, "de") if watcher => "einmal, dann stoppt er".into(),
        (End::Once, "de") => "einmal, dann stoppt sie".into(),
        (End::Once, "es") => "una vez y se detiene".into(),
        (End::Once, _) => "once, then it stops".into(),
        (End::Never, "fr") => "jusqu'à ce que tu l'arrêtes".into(),
        (End::Never, "de") if watcher => "bis du ihn stoppst".into(),
        (End::Never, "de") => "bis du sie stoppst".into(),
        (End::Never, "es") if watcher => "hasta que lo detengas".into(),
        (End::Never, "es") => "hasta que la detengas".into(),
        (End::Never, _) => "until you stop it".into(),
    }
}

/// `ends` as the apps read it: {kind, at?, count?, text}.
pub fn ends_view(lang: &str, task: Task, end: &End, zone: Tz) -> Value {
    let text = end_words(lang, task, end, zone);
    match end {
        End::Date(at) => json!({"kind": end.kind(), "at": at, "text": text}),
        End::Count(n) => json!({"kind": end.kind(), "count": n, "text": text}),
        _ => json!({"kind": end.kind(), "text": text}),
    }
}

/// The end said on a card: a date or a number of runs (none: today's sentence).
pub fn card_end(lang: &str, task: Task, end: &End, zone: Tz) -> Option<String> {
    matches!(end, End::Date(_) | End::Count(_)).then(|| end_words(lang, task, end, zone))
}

/// « Te répond dans » (texts.<lang>.deliver).
pub fn deliver_words(lang: &str, kind: &str) -> &'static str {
    match (lang_of(lang), kind) {
        ("fr", "conversation") => "cette conversation",
        ("fr", "silent") => "en silence, sur sa page",
        ("fr", _) => "la Boîte",
        ("de", "conversation") => "diesem Chat",
        ("de", "silent") => "still, auf seiner Seite",
        ("de", _) => "dem Eingang",
        ("es", "conversation") => "esta conversación",
        ("es", "silent") => "en silencio, en su página",
        ("es", _) => "el Buzón",
        (_, "conversation") => "this conversation",
        (_, "silent") => "silently, on its page",
        _ => "the Inbox",
    }
}

/// The card's top line (§7.5).
pub fn fix_title(lang: &str, task: Task, who: &str, name: &str) -> String {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => format!("{who} · le veilleur « {name} » ne marche plus"),
        ("fr", Task::Routine) => format!("{who} · la routine « {name} » a échoué 3 fois de suite"),
        ("de", Task::Watcher) => format!("{who} · der Wächter „{name}“ funktioniert nicht mehr"),
        ("de", Task::Routine) => format!("{who} · die Routine „{name}“ ist dreimal hintereinander fehlgeschlagen"),
        ("es", Task::Watcher) => format!("{who} · el vigilante «{name}» ya no funciona"),
        ("es", Task::Routine) => format!("{who} · la rutina «{name}» ha fallado 3 veces seguidas"),
        (_, Task::Watcher) => format!("{who} · the watcher “{name}” stopped working"),
        (_, Task::Routine) => format!("{who} · the routine “{name}” failed 3 times in a row"),
    }
}

/// « Depuis {since} : {reason}. »
pub fn fix_since_line(lang: &str, since: &str, reason: &str) -> String {
    match lang_of(lang) {
        "fr" => format!("Depuis {since} : {reason}."),
        "de" => format!("Seit {since}: {reason}."),
        "es" => format!("Desde {since}: {reason}."),
        _ => format!("Since {since}: {reason}."),
    }
}

/// Why, in plain words (texts.<lang>.reasons).
pub fn reason_words(lang: &str, reason: &str) -> &'static str {
    match (lang_of(lang), reason) {
        ("fr", "looping") => "elle tournait en rond",
        ("fr", "too_long") => "elle prenait trop de temps",
        ("fr", "not_started") => "elle n'a pas pu démarrer",
        ("fr", "timeout") => "le site ne répond pas à temps",
        ("fr", "secret") => "il lui manque un mot de passe",
        ("fr", "check") => "sa vérification ne marche plus",
        ("fr", _) => "un problème l'a arrêtée",
        ("de", "looping") => "Sie hat sich im Kreis gedreht",
        ("de", "too_long") => "Sie hat zu lange gedauert",
        ("de", "not_started") => "Sie konnte nicht starten",
        ("de", "timeout") => "Die Seite antwortet nicht rechtzeitig",
        ("de", "secret") => "Ihm fehlt ein Passwort",
        ("de", "check") => "Seine Prüfung funktioniert nicht mehr",
        ("de", _) => "Ein Problem hat sie gestoppt",
        ("es", "looping") => "estaba dando vueltas en círculo",
        ("es", "too_long") => "estaba tardando demasiado",
        ("es", "not_started") => "no ha podido empezar",
        ("es", "timeout") => "el sitio no responde a tiempo",
        ("es", "secret") => "le falta una contraseña",
        ("es", "check") => "su comprobación ya no funciona",
        ("es", _) => "un problema la detuvo",
        (_, "looping") => "it was going round in circles",
        (_, "too_long") => "it was taking too long",
        (_, "not_started") => "it couldn't start",
        (_, "timeout") => "the site doesn't answer in time",
        (_, "secret") => "it is missing a password",
        (_, "check") => "its check doesn't work any more",
        _ => "something stopped it",
    }
}

pub fn fix_buttons(lang: &str) -> [&'static str; 2] {
    match lang_of(lang) {
        "fr" => ["Réparer", "Arrêter"],
        "de" => ["Reparieren", "Stoppen"],
        "es" => ["Reparar", "Detener"],
        _ => ["Repair", "Stop"],
    }
}

/// Under « i » (§7.5).
pub fn fix_under_i(lang: &str, task: Task, who: &str) -> String {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => format!(
            "Réparer : {who} le corrige une fois ; tu valideras le changement. Arrêter : il ne tourne plus et reste sur sa page."
        ),
        ("fr", Task::Routine) => format!(
            "Réparer : {who} la corrige une fois ; tu valideras le changement. Arrêter : elle ne tourne plus et reste sur sa page."
        ),
        ("de", Task::Watcher) => format!(
            "Reparieren: {who} korrigiert ihn einmal; du bestätigst die Änderung. Stoppen: Er läuft nicht mehr und bleibt auf seiner Seite."
        ),
        ("de", Task::Routine) => format!(
            "Reparieren: {who} korrigiert sie einmal; du bestätigst die Änderung. Stoppen: Sie läuft nicht mehr und bleibt auf ihrer Seite."
        ),
        ("es", Task::Watcher) => format!(
            "Reparar: {who} lo corrige una vez; tú aprobarás el cambio. Detener: deja de funcionar y se queda en su página."
        ),
        ("es", Task::Routine) => format!(
            "Reparar: {who} la corrige una vez; tú aprobarás el cambio. Detener: deja de funcionar y se queda en su página."
        ),
        _ => format!("Repair: {who} fixes it once; you'll approve the change. Stop: it no longer runs and stays on its page."),
    }
}

/// The app 1.0's whole card (`bip_summary`): « Oui » repairs, « Non » stops.
pub fn fix_summary(lang: &str, task: Task, who: &str, name: &str) -> String {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => format!("Le veilleur « {name} » de {who} ne marche plus. Le faire réparer ? (Non : il s'arrête.)"),
        ("fr", Task::Routine) => {
            format!("La routine « {name} » de {who} a échoué 3 fois de suite. La faire réparer ? (Non : elle s'arrête.)")
        }
        ("de", Task::Watcher) => {
            format!("{who}s Wächter „{name}“ funktioniert nicht mehr. Reparieren lassen? (Nein: Er stoppt.)")
        }
        ("de", Task::Routine) => format!(
            "{who}s Routine „{name}“ ist dreimal hintereinander fehlgeschlagen. Reparieren lassen? (Nein: Sie stoppt.)"
        ),
        ("es", Task::Watcher) => {
            format!("El vigilante «{name}» de {who} ya no funciona. ¿Hacer que lo repare? (No: se detiene.)")
        }
        ("es", Task::Routine) => {
            format!("La rutina «{name}» de {who} ha fallado 3 veces seguidas. ¿Hacer que la repare? (No: se detiene.)")
        }
        (_, Task::Watcher) => format!("{who}'s watcher “{name}” stopped working. Have it repaired? (No: it stops.)"),
        (_, Task::Routine) => format!("{who}'s routine “{name}” failed 3 times in a row. Have it repaired? (No: it stops.)"),
    }
}

/// A card unanswered for `NO_ANSWER_DAYS`: the line.
pub fn stopped_line(lang: &str, task: Task, name: &str) -> String {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => format!("« {name} » est arrêté : il ne marchait plus depuis 7 jours. Il reste sur sa page."),
        ("fr", Task::Routine) => {
            format!("« {name} » est arrêtée : elle ne marchait plus depuis 7 jours. Elle reste sur sa page.")
        }
        ("de", Task::Watcher) => {
            format!("„{name}“ ist gestoppt: Er hat 7 Tage lang nicht funktioniert. Er bleibt auf seiner Seite.")
        }
        ("de", Task::Routine) => {
            format!("„{name}“ ist gestoppt: Sie hat 7 Tage lang nicht funktioniert. Sie bleibt auf ihrer Seite.")
        }
        ("es", Task::Watcher) => format!("«{name}» está detenido: llevaba 7 días sin funcionar. Se queda en su página."),
        ("es", Task::Routine) => format!("«{name}» está detenida: llevaba 7 días sin funcionar. Se queda en su página."),
        _ => format!("“{name}” is stopped: it hadn't worked for 7 days. It stays on its page."),
    }
}

/// An end reached, for what ran more than a week: the line.
pub fn ended_line(lang: &str, task: Task, name: &str, end: &str) -> String {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => format!("Le veilleur « {name} » s'est arrêté comme prévu ({end})."),
        ("fr", Task::Routine) => format!("La routine « {name} » s'est arrêtée comme prévu ({end})."),
        ("de", Task::Watcher) => format!("Der Wächter „{name}“ hat wie geplant aufgehört ({end})."),
        ("de", Task::Routine) => format!("Die Routine „{name}“ hat wie geplant aufgehört ({end})."),
        ("es", Task::Watcher) => format!("El vigilante «{name}» se ha detenido como estaba previsto ({end})."),
        ("es", Task::Routine) => format!("La rutina «{name}» se ha detenido como estaba previsto ({end})."),
        (_, Task::Watcher) => format!("The watcher “{name}” stopped as planned ({end})."),
        (_, Task::Routine) => format!("The routine “{name}” stopped as planned ({end})."),
    }
}

/// « Garder 3 mois de plus ? » (§7.4).
pub fn keep_sentence(lang: &str, task: Task, name: &str, date: &str) -> String {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => format!("Le veilleur « {name} » s'arrête le {date}. Le garder 3 mois de plus ?"),
        ("fr", Task::Routine) => format!("La routine « {name} » s'arrête le {date}. La garder 3 mois de plus ?"),
        ("de", Task::Watcher) => format!("Der Wächter „{name}“ endet am {date}. Noch 3 Monate behalten?"),
        ("de", Task::Routine) => format!("Die Routine „{name}“ endet am {date}. Noch 3 Monate behalten?"),
        ("es", Task::Watcher) => format!("El vigilante «{name}» termina el {date}. ¿Mantenerlo 3 meses más?"),
        ("es", Task::Routine) => format!("La rutina «{name}» termina el {date}. ¿Mantenerla 3 meses más?"),
        (_, Task::Watcher) => format!("The watcher “{name}” stops on {date}. Keep it 3 more months?"),
        (_, Task::Routine) => format!("The routine “{name}” stops on {date}. Keep it 3 more months?"),
    }
}

pub fn keep_buttons(lang: &str, task: Task) -> [&'static str; 2] {
    match (lang_of(lang), task) {
        ("fr", Task::Watcher) => ["Garder", "Le laisser s'arrêter"],
        ("fr", Task::Routine) => ["Garder", "La laisser s'arrêter"],
        ("de", _) => ["Behalten", "Enden lassen"],
        ("es", _) => ["Mantener", "Dejar que termine"],
        _ => ["Keep", "Let it stop"],
    }
}

/// « you », or the shown name of the family member who asked for it.
pub fn owner(config: &Config, agent: &str, by: Option<&str>) -> Value {
    let person = match by {
        Some(household) if !household.is_empty() => crate::agent_shares::shown_household(config, household),
        _ => "you".to_string(),
    };
    json!({"agent": agent, "person": person})
}

// -- the card -----------------------------------------------------------------------------------------------------------

/// What a card is about.
pub struct About<'a> {
    pub task: Task,
    pub agent: &'a str,
    /// The routine's or the watcher's id.
    pub id: &'a str,
    pub name: &'a str,
    pub fix: &'a Fix,
    /// Its end (a « Garder ? »'s).
    pub ends_at: Option<f64>,
}

/// The card in `/v1/approvals`' shape (contracts/task-frame `cards.fix`, `cards.keep`).
pub fn card_item(config: &Config, about: &About) -> Dict {
    let lang = lang_of_agent(config, about.agent);
    let zone = routines::zone(config);
    let who = routines::flat(&routines::shown_agent(config, about.agent), routines::NAME_MAX);
    let name = routines::flat(about.name, routines::NAME_MAX);
    let (fix, task) = (about.fix, about.task);
    let keep = fix.variant == KEEP;
    let (summary, detail, bip) = if keep {
        let sentence = keep_sentence(lang, task, &name, &day_words(lang, about.ends_at.unwrap_or(fix.at), zone));
        let bip = json!({"kind": task.key(), "ref": about.id, "name": name, "variant": KEEP, "ends_at": about.ends_at,
                         "title": who, "text": sentence, "buttons": keep_buttons(lang, task)});
        (sentence.clone(), sentence, bip)
    } else {
        let since = if fix.since > 0.0 { fix.since } else { fix.at };
        let text = fix_since_line(lang, &since_words(lang, since, zone), reason_words(lang, &fix.reason));
        let bip = json!({"kind": task.key(), "ref": about.id, "name": name, "variant": REPAIR, "since": since,
                         "failures": fix.failures, "reason": fix.reason, "title": fix_title(lang, task, &who, &name),
                         "text": text, "buttons": fix_buttons(lang)});
        (fix_summary(lang, task, &who, &name), fix_under_i(lang, task, &who), bip)
    };
    let item = json!({
        "agent": about.agent, "run_id": format!("{RUN_PREFIX}{}", fix.id), "request_id": fix.id,
        "command": format!("{KIND} · {name}"), "description": summary, "choices": ["once", "deny"],
        "bip_choices": if keep { [KEEP, "stop"] } else { [REPAIR, "stop"] },
        "created_at": fix.at, "session_id": null, "bip_summary": summary, "bip_detail": detail, "bip_fix": bip,
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The cards waiting for `agent` (None: every agent), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut items: Vec<(f64, Dict)> = Vec::new();
    for r in routines::all(config, agent) {
        if let Some(fix) = &r.frame.fix {
            let about = About { task: Task::Routine, agent: &r.agent, id: &r.id, name: &r.name, fix, ends_at: r.frame.until };
            items.push((fix.at, card_item(config, &about)));
        }
    }
    for (name, conn) in connectors::all(config) {
        if agent.is_some_and(|a| a != name) {
            continue;
        }
        if let Some(fix) = &conn.frame.fix {
            let ends_at = conn.watch.as_ref().and_then(|w| w.until);
            let about = About { task: Task::Watcher, agent: &name, id: &conn.id, name: &conn.name, fix, ends_at };
            items.push((fix.at, card_item(config, &about)));
        }
    }
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    items.into_iter().map(|(_, item)| item).collect()
}

/// A card gone (answered elsewhere, its task gone or working again): no turn resumed for it, its places told.
pub fn withdraw(config: &Config, fix_id: &str) {
    crate::card_place::taken(config, &format!("{RUN_PREFIX}{fix_id}"));
}

/// A card just made: in the main conversation and the Boîte (no turn asked it), pushed when `push` as a request
/// (« Une demande attend ton accord », its sealed words `line`).
pub async fn raise(state: &AppState, agent: &AgentConfig, run_id: &str, line: &str, push: bool) {
    crate::card_place::place_for(state, agent, run_id, None).await;
    crate::card_place::taken(&state.config(), run_id);
    tracing::info!(target: "bipbridge::task_frame", agent = %agent.name, card = run_id, push, "task frame card raised");
    if push {
        state.push.notify_bridge_card(&agent.name, line, crate::tool_asks::PUSH_KEY, run_id).await;
    }
}

/// How an answer went.
#[derive(Debug, Clone, PartialEq)]
pub struct Settled {
    pub task: Task,
    /// The routine's or the watcher's id.
    pub id: String,
    pub variant: String,
    pub yes: bool,
}

/// The person answered card `fix_id` of `agent` (`yes`: « Réparer » / « Garder »): its task changed as the answer says;
/// None: no such card. « Réparer »'s turn is the caller's (`approve`).
pub fn settle(config: &Config, agent: &str, fix_id: &str, yes: bool) -> Option<Settled> {
    let found = routines::change(config, |b| {
        let r = b.routines.iter_mut().find(|r| r.agent == agent && r.frame.fix.as_ref().is_some_and(|f| f.id == fix_id))?;
        let fix = r.frame.fix.take()?;
        match (fix.variant.as_str(), yes) {
            (KEEP, true) => {
                r.frame.until = r.frame.until.map(|u| u + KEEP_DAYS_MORE * DAY);
                r.frame.keep_asked = false;
            }
            (KEEP, false) => {}
            (_, true) => r.frame.repairing = true,
            (_, false) => {
                // « Arrêter »: paused, on its page
                r.enabled = false;
                r.next_run = None;
                r.frame.repairing = false;
                r.frame.retry_at = None;
            }
        }
        Some(Settled { task: Task::Routine, id: r.id.clone(), variant: fix.variant, yes })
    });
    if found.is_some() {
        return found;
    }
    connectors::change(config, |store| {
        let conn = store.agents.get_mut(agent)?.iter_mut().find(|c| c.frame.fix.as_ref().is_some_and(|f| f.id == fix_id))?;
        let fix = conn.frame.fix.take()?;
        match (fix.variant.as_str(), yes) {
            (KEEP, true) => {
                if let Some(w) = conn.watch.as_mut() {
                    w.until = w.until.map(|u| u + KEEP_DAYS_MORE * DAY);
                }
                conn.frame.keep_asked = false;
            }
            (KEEP, false) | (_, true) => {}
            // « Arrêter »: switched off, on its page
            (_, false) => conn.enabled = false,
        }
        Some(Settled { task: Task::Watcher, id: conn.id.clone(), variant: fix.variant, yes })
    })
    .ok()
    .flatten()
}

/// `POST /v1/approve` for a card `bip-fix-<id>`: « Réparer » / « Garder » (once, repair, keep; session and always mean
/// once and keep nothing), « Arrêter » / « Le laisser s'arrêter » (deny and the other refusals, stop); anything else
/// changes nothing (422).
pub async fn approve(state: &AppState, agent: &AgentConfig, id: &str, choice: &str) -> ApiResult<axum::response::Response> {
    let choice = choice.trim().to_lowercase();
    let yes = matches!(choice.as_str(), "once" | "session" | "always" | REPAIR | KEEP);
    if !yes && !(routines::refuses(&choice) || choice == "stop") {
        return Err(HttpError::new(422, "choice: once or deny"));
    }
    let (config, name, fix) = (state.config(), agent.name.clone(), id.to_string());
    let settled = tokio::task::spawn_blocking(move || settle(&config, &name, &fix, yes)).await.map_err(anyhow::Error::from)?;
    let Some(settled) = settled else { return Err(HttpError::new(404, "no such request (answered already?)")) };
    tracing::info!(target: "bipbridge::task_frame", agent = %agent.name, task = settled.task.key(), id = %settled.id,
                   variant = %settled.variant, yes, "task frame card answered");
    if yes && settled.variant == REPAIR {
        // ONE turn of its agent, with what failed (no model call before this answer)
        let (state, agent, task_id) = (state.clone(), agent.clone(), settled.id.clone());
        tokio::spawn(async move {
            match settled.task {
                Task::Routine => routines::repair(&state, &agent, &task_id).await,
                Task::Watcher => crate::watchers::repair(&state, &agent, &task_id).await,
            }
        });
    }
    Ok(web::ok(json!({"ok": true, "choice": if yes { "once" } else { "deny" }})))
}

// -- the clock ----------------------------------------------------------------------------------------------------------

/// What the clock found to do with the state.
#[derive(Debug, Clone, PartialEq)]
pub enum Due {
    /// One Boîte line, not pushed, where it answers (a routine gone: the main conversation).
    Line { agent: String, task: Task, id: String, text: String },
    /// A card to place, pushed when `push`.
    Raise { agent: String, run_id: String, line: String, push: bool },
    /// A watcher at its end: removed (connectors.rs `remove`), after its line if any.
    Remove { agent: String, id: String },
}

/// Ran long enough for its end to be told.
fn ran_long(since: f64, ran: bool, now: f64) -> bool {
    ran && since > 0.0 && now - since > ENDED_LINE_DAYS * DAY
}

fn routine_ends_now(r: &Routine, now: f64) -> bool {
    r.frame.until.is_some_and(|u| now > u) || (routines::end_of(r) == End::Count(0) && r.next_run.is_none())
}

fn routine_asks_keep(r: &Routine, now: f64) -> bool {
    r.frame.default_end
        && !r.frame.keep_asked
        && r.frame.fix.is_none()
        && r.enabled
        && !r.awaits_start
        && r.frame.until.is_some_and(|u| u > now && u - now <= KEEP_ASK_DAYS * DAY)
}

fn unanswered(fix: &Option<Fix>, now: f64) -> bool {
    fix.as_ref().is_some_and(|f| f.variant != KEEP && now - f.at >= NO_ANSWER_DAYS * DAY)
}

fn watch_ends_now(conn: &connectors::Connector, now: f64) -> bool {
    conn.watch.as_ref().and_then(|w| w.until).is_some_and(|u| now > u)
}

fn watch_asks_keep(conn: &connectors::Connector, now: f64) -> bool {
    let Some(watch) = &conn.watch else { return false };
    watch.default_end
        && !conn.frame.keep_asked
        && conn.frame.fix.is_none()
        && conn.enabled
        && conn.status == connectors::APPROVED
        && watch.until.is_some_and(|u| u > now && u - now <= KEEP_ASK_DAYS * DAY)
}

/// One look at `now`: ends reached, « Garder ? » to ask, cards nobody answered for a week. The state changes are
/// made here (a routine at its end removed, a card asked or gone); what to tell and raise is returned (`tick`).
pub fn due(config: &Config, now: f64) -> Vec<Due> {
    let mut out = Vec::new();
    let zone = routines::zone(config);
    let mine = routines::all(config, None);
    let routine_work = mine.iter().any(|r| {
        !routines::is_running(&r.id)
            && (routine_ends_now(r, now) || routine_asks_keep(r, now) || unanswered(&r.frame.fix, now))
    });
    if routine_work {
        let mut gone: Vec<Routine> = Vec::new();
        let mut withdrawn: Vec<String> = Vec::new();
        routines::change(config, |b| {
            for r in b.routines.iter_mut().filter(|r| !routines::is_running(&r.id)) {
                if routine_ends_now(r, now) {
                    gone.push(r.clone());
                    continue;
                }
                let lang = lang_of_agent(config, &r.agent);
                if unanswered(&r.frame.fix, now) {
                    // a week without an answer: paused, its card gone, one line
                    withdrawn.extend(r.frame.fix.take().map(|f| f.id));
                    (r.enabled, r.next_run, r.frame.repairing, r.frame.retry_at) = (false, None, false, None);
                    let name = routines::flat(&r.name, routines::NAME_MAX);
                    let text = stopped_line(lang, Task::Routine, &name);
                    out.push(Due::Line { agent: r.agent.clone(), task: Task::Routine, id: r.id.clone(), text });
                }
                if routine_asks_keep(r, now) {
                    let fix = Fix { id: py::token_hex(6), at: now, variant: KEEP.into(), ..Fix::default() };
                    let run_id = format!("{RUN_PREFIX}{}", fix.id);
                    (r.frame.fix, r.frame.keep_asked) = (Some(fix), true);
                    out.push(Due::Raise { agent: r.agent.clone(), run_id, line: String::new(), push: false });
                }
            }
            b.routines.retain(|r| !gone.iter().any(|g| g.id == r.id));
        });
        for r in &gone {
            // its cards with it
            withdrawn.extend(r.frame.fix.as_ref().map(|f| f.id.clone()));
            routines::withdraw(config, r.card.as_ref().map(|c| c.id.clone()));
            routines::withdraw(config, r.change.as_ref().map(|c| c.card.id.clone()));
            tracing::info!(target: "bipbridge::task_frame", agent = %r.agent, routine = %r.id, "routine at its end: gone");
            if ran_long(r.created, r.last_run.is_some(), now) {
                let lang = lang_of_agent(config, &r.agent);
                let end = end_words(lang, Task::Routine, &routines::end_of(r), zone);
                let text = ended_line(lang, Task::Routine, &routines::flat(&r.name, routines::NAME_MAX), &end);
                out.push(Due::Line { agent: r.agent.clone(), task: Task::Routine, id: r.id.clone(), text });
            }
        }
        for id in &withdrawn {
            withdraw(config, id);
        }
    }
    let watchers = connectors::all(config);
    let watcher_work = watchers.iter().any(|(_, c)| {
        c.watch.is_some() && (watch_ends_now(c, now) || watch_asks_keep(c, now) || unanswered(&c.frame.fix, now))
    });
    if watcher_work {
        let mut gone: Vec<(String, connectors::Connector)> = Vec::new();
        let mut withdrawn: Vec<String> = Vec::new();
        let _ = connectors::change(config, |store| {
            for (agent, list) in store.agents.iter_mut() {
                for conn in list.iter_mut().filter(|c| c.watch.is_some()) {
                    if watch_ends_now(conn, now) {
                        gone.push((agent.clone(), conn.clone()));
                        continue;
                    }
                    let lang = lang_of_agent(config, agent);
                    if unanswered(&conn.frame.fix, now) {
                        withdrawn.extend(conn.frame.fix.take().map(|f| f.id));
                        conn.enabled = false;
                        let text = stopped_line(lang, Task::Watcher, &routines::flat(&conn.name, routines::NAME_MAX));
                        out.push(Due::Line { agent: agent.clone(), task: Task::Watcher, id: conn.id.clone(), text });
                    }
                    if watch_asks_keep(conn, now) {
                        let fix = Fix { id: py::token_hex(6), at: now, variant: KEEP.into(), ..Fix::default() };
                        let run_id = format!("{RUN_PREFIX}{}", fix.id);
                        (conn.frame.fix, conn.frame.keep_asked) = (Some(fix), true);
                        out.push(Due::Raise { agent: agent.clone(), run_id, line: String::new(), push: false });
                    }
                }
            }
        });
        for (agent, conn) in gone {
            withdrawn.extend(conn.frame.fix.as_ref().map(|f| f.id.clone()));
            let approved = conn.approved_at.as_deref().and_then(crate::place_move::epoch).unwrap_or(0.0);
            let checked = crate::watchers::load(config, &agent, &conn.id).checks > 0;
            if ran_long(approved, checked, now) {
                let lang = lang_of_agent(config, &agent);
                let end = end_words(lang, Task::Watcher, &crate::watchers::end_of(conn.watch.as_ref()), zone);
                let text = ended_line(lang, Task::Watcher, &routines::flat(&conn.name, routines::NAME_MAX), &end);
                out.push(Due::Line { agent: agent.clone(), task: Task::Watcher, id: conn.id.clone(), text });
            }
            out.push(Due::Remove { agent, id: conn.id });
        }
        for id in &withdrawn {
            withdraw(config, id);
        }
    }
    // a trigger at its end goes, quietly
    if crate::triggers::load(config).triggers.iter().any(|t| t.ends.is_some_and(|e| now > e)) {
        crate::triggers::change(config, |s| s.triggers.retain(|t| t.ends.is_none_or(|e| now <= e)));
    }
    out
}

/// Every look of the routines' clock (routines.rs `scheduler`): `due`, then its lines, cards and removals.
pub async fn tick(state: &AppState) {
    let (config, now) = (state.config(), py::time_time());
    let found = {
        let config = config.clone();
        tokio::task::spawn_blocking(move || due(&config, now)).await.unwrap_or_default()
    };
    for d in found {
        let name = match &d {
            Due::Line { agent, .. } | Due::Raise { agent, .. } | Due::Remove { agent, .. } => agent.clone(),
        };
        let Some(agent) = config.agent(Some(&name)).cloned() else { continue };
        match d {
            Due::Line { task: Task::Routine, id, text, .. } => routines::tell_line(state, &agent, &id, &text).await,
            Due::Line { task: Task::Watcher, id, text, .. } => crate::watchers::tell_line(state, &agent, &id, &text).await,
            Due::Raise { run_id, line, push, .. } => raise(state, &agent, &run_id, &line, push).await,
            Due::Remove { id, .. } => {
                connectors::remove(state, &agent, &id).await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
